//! `hook.sock`: sessions of zsh shells that report through `fig.zsh`
//! (docs/15-architecture-study.md §3.3).
//!
//! Each connection becomes a [`ShellSession`] in [`ShellSessions`], and its lines become
//! [`HookRequest`]s handed to [`dispatch_hook`]. What the desktop sends a session
//! ([`ShellCommand`]) is carried out here: key interception and insertion become lines for the
//! shell, and processes run in this process.

mod insert;
mod keys;
use crate::process;
pub mod wire;

use std::collections::{
    BTreeSet,
    HashMap,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{
    Duration,
    SystemTime,
};

use anyhow::{
    Context as _,
    Result,
};
use fig_proto::fig::EnvironmentVariable;
use fig_proto::local::{
    EditBufferHook,
    InterceptedKeyHook,
    PostExecHook,
    PreExecHook,
    PromptHook,
    ShellContext,
};
use fig_proto::shell::InsertTextRequest;
use fig_settings::history::CommandInfo;
use parking_lot::Mutex;
use tokio::io::{
    AsyncBufReadExt as _,
    AsyncReadExt as _,
    AsyncWriteExt as _,
    BufReader,
};
use tokio::net::unix::{
    OwnedReadHalf,
    OwnedWriteHalf,
};
use tokio::net::{
    UnixListener,
    UnixStream,
};
use tokio::sync::mpsc;
use tracing::{
    debug,
    info,
    warn,
};
use uuid::Uuid;
pub use wire::HOOK_PROTOCOL_VERSION;
use wire::{
    DaemonLine,
    KeyVerdict,
    MAX_LINE_LEN,
    ShellLine,
};

use crate::session::{
    EditBuffer,
    InterceptMode,
    ShellCommand,
    ShellSession,
    ShellSessions,
};
use crate::{
    HookRequest,
    ShellHookHandler,
    dispatch_hook,
};

/// How long a new connection has to say `HELLO`. `fig.zsh` sends it before reading anything.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Script versions a refusal has been logged for. Every tab opened before a rebuild refuses once
/// per shell, so the warning is written once per version and the rest go to `debug`.
static REFUSED_VERSIONS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

pub async fn start_hook_ipc(
    socket_path: PathBuf,
    shell_sessions: Arc<ShellSessions>,
    hook: impl ShellHookHandler + Send + Clone + 'static,
) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).context("Failed creating socket path")?;
        }
        {
            use std::fs::Permissions;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, Permissions::from_mode(0o700))?;
        }
    }

    tokio::fs::remove_file(&socket_path).await.ok();
    let listener = UnixListener::bind(&socket_path).with_context(|| format!("binding {}", socket_path.display()))?;

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, shell_sessions.clone(), hook.clone()));
    }
    Ok(())
}

/// Lines off the socket, at most [`MAX_LINE_LEN`] bytes each. A longer line is skipped whole.
struct Lines {
    reader: BufReader<OwnedReadHalf>,
    buf: Vec<u8>,
}

impl Lines {
    async fn next(&mut self) -> std::io::Result<Option<String>> {
        loop {
            self.buf.clear();
            let limit = MAX_LINE_LEN as u64 + 1;
            let read = (&mut self.reader).take(limit).read_until(b'\n', &mut self.buf).await?;
            if read == 0 {
                return Ok(None);
            }
            if self.buf.last() == Some(&b'\n') {
                self.buf.pop();
                return Ok(Some(String::from_utf8_lossy(&self.buf).into_owned()));
            }
            if read < limit as usize {
                // End of stream in the middle of a line: the shell went away mid-write.
                return Ok(None);
            }
            warn!(limit = MAX_LINE_LEN, "skipping an overlong hook line");
            loop {
                self.buf.clear();
                if (&mut self.reader).take(limit).read_until(b'\n', &mut self.buf).await? == 0 {
                    return Ok(None);
                }
                if self.buf.last() == Some(&b'\n') {
                    break;
                }
            }
        }
    }
}

async fn write_lines(mut writer: OwnedWriteHalf, mut lines: mpsc::Receiver<DaemonLine>) {
    while let Some(line) = lines.recv().await {
        let mut text = line.to_string();
        text.push('\n');
        if let Err(err) = tokio::time::timeout(Duration::from_secs(1), writer.write_all(text.as_bytes()))
            .await
            .unwrap_or_else(|_| Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "hook socket stalled")))
        {
            debug!(%err, "hook session write failed");
            break;
        }
    }
}

/// What a hook session has told the desktop so far; every hook's `ShellContext` is built from it.
struct Session {
    id: Uuid,
    shell: String,
    pid: Option<i32>,
    tty: String,
    user: String,
    cwd: Option<String>,
    env: Vec<EnvironmentVariable>,
    alias: Option<String>,
    /// A new `C` has not reached the desktop in an edit-buffer hook yet.
    context_dirty: bool,
    preexec: bool,
    /// The last `E` text, what an insertion is reconciled against.
    buffer: String,
    revision: Option<u64>,
    cursor: usize,
    pending_edits: HashMap<String, tokio::sync::oneshot::Sender<Result<(), String>>>,
    /// The command line accepted by the last `X`, and when.
    running: Option<(String, SystemTime)>,
    interceptor: keys::Interceptor,
    /// The key set last sent as `INTERCEPT`.
    forwarded: BTreeSet<String>,
}

impl Session {
    /// With `full`, the environment and aliases too: every hook but the edit-buffer ones carries
    /// them, and those only after they changed.
    fn context(&self, full: bool) -> ShellContext {
        let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        ShellContext {
            pid: self.pid,
            ttys: non_empty(&self.tty),
            process_name: non_empty(&self.shell),
            current_working_directory: self.cwd.clone(),
            session_id: Some(self.id.to_string()),
            hostname: non_empty(&self.user),
            environment_variables: if full { self.env.clone() } else { vec![] },
            alias: if full { self.alias.clone() } else { None },
            preexec: Some(self.preexec),
            context_snapshot: Some(full),
            input_revision: self.revision,
            ..Default::default()
        }
    }

    /// `INTERCEPT`, if the keys to forward changed.
    fn intercept_update(&mut self) -> Option<DaemonLine> {
        let forwarded = self.interceptor.forwarded();
        (forwarded != self.forwarded).then(|| {
            self.forwarded.clone_from(&forwarded);
            DaemonLine::Intercept(forwarded)
        })
    }
}

async fn handle_connection(
    stream: UnixStream,
    shell_sessions: Arc<ShellSessions>,
    mut hook: impl ShellHookHandler + Send,
) {
    let (reader, writer) = stream.into_split();
    let mut lines = Lines {
        reader: BufReader::new(reader),
        buf: Vec::new(),
    };
    let (out, out_rx) = mpsc::channel(64);
    let mut writer_task = tokio::spawn(write_lines(writer, out_rx));

    let hello = match tokio::time::timeout(HELLO_TIMEOUT, lines.next()).await {
        Ok(Ok(Some(line))) if line == wire::PING => {
            let _ = out.send(DaemonLine::Pong).await;
            drop(out);
            let _ = writer_task.await;
            return;
        },
        Ok(Ok(Some(line))) => ShellLine::parse(&line),
        Ok(Ok(None) | Err(_)) | Err(_) => {
            debug!("hook connection closed before HELLO");
            return;
        },
    };
    let (shell, pid, tty, user) = match hello {
        Ok(ShellLine::Hello {
            version,
            shell,
            pid,
            tty,
            user,
            ..
        }) if version == HOOK_PROTOCOL_VERSION => (shell, pid, tty, user),
        Ok(ShellLine::Hello { version, .. }) => {
            if REFUSED_VERSIONS.lock().insert(version) {
                warn!(
                    peer = version,
                    ours = HOOK_PROTOCOL_VERSION,
                    "Refusing a zsh hook built for another protocol version; its shell runs without autocomplete until it is restarted"
                );
            } else {
                debug!(peer = version, ours = HOOK_PROTOCOL_VERSION, "Refusing a zsh hook");
            }
            let _ = out
                .send(DaemonLine::Refuse {
                    reason: format!("protocol version {version}, this desktop speaks {HOOK_PROTOCOL_VERSION}"),
                })
                .await;
            drop(out);
            let _ = writer_task.await;
            return;
        },
        other => {
            warn!(?other, "hook connection did not start with HELLO");
            let _ = out
                .send(DaemonLine::Refuse {
                    reason: "expected HELLO".into(),
                })
                .await;
            drop(out);
            let _ = writer_task.await;
            return;
        },
    };

    let session_id = Uuid::new_v4();
    let session = Arc::new(Mutex::new(Session {
        id: session_id,
        shell,
        pid,
        tty,
        user,
        cwd: None,
        env: vec![],
        alias: None,
        context_dirty: false,
        preexec: false,
        buffer: String::new(),
        revision: None,
        cursor: 0,
        pending_edits: HashMap::new(),
        running: None,
        interceptor: keys::Interceptor::default(),
        forwarded: BTreeSet::new(),
    }));

    let (command_tx, command_rx) = flume::bounded(64);
    let (disconnect, mut disconnected) = tokio::sync::watch::channel(false);
    shell_sessions.insert(ShellSession {
        id: session_id,
        sender: crate::session::ShellSender::new(command_tx, disconnect),
        edit_buffer: EditBuffer::default(),
        context: None,
        terminal_cursor_coordinates: None,
        intercept: InterceptMode::Unlocked,
        intercept_global: InterceptMode::Unlocked,
    });
    let command_task = tokio::spawn(handle_commands(command_rx, session.clone(), out.clone()));
    let _ = out.send(DaemonLine::Welcome).await;
    info!(%session_id, "zsh hook session connected");

    loop {
        let line = match tokio::select! {
            _ = disconnected.wait_for(|closed| *closed) => break,
            _ = &mut writer_task => {
                shell_sessions.remove_id(&session_id);
                command_task.abort();
                return;
            },
            line = lines.next() => line,
        } {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(err) => {
                debug!(%err, "hook session read failed");
                break;
            },
        };
        let message = match ShellLine::parse(&line) {
            Ok(message) => message,
            Err(err) => {
                warn!(%err, "ignoring a malformed hook line");
                continue;
            },
        };
        let message = match message {
            ShellLine::EditBufferV2 { revision, text, cursor } => {
                session.lock().revision = Some(revision);
                ShellLine::EditBuffer { text, cursor }
            },
            ShellLine::EditAck { id, status } => {
                if let Some(response) = session.lock().pending_edits.remove(&id) {
                    let result = match status {
                        0 => Ok(()),
                        1 => Err("stale input: edit rejected".into()),
                        _ => Err("shell cannot apply this edit".into()),
                    };
                    let _ = response.send(result);
                }
                continue;
            },
            message => message,
        };
        let request = match message {
            ShellLine::EditBufferV2 { .. } | ShellLine::EditAck { .. } => unreachable!("normalized above"),
            ShellLine::Hello { .. } => {
                warn!("ignoring a second HELLO");
                continue;
            },
            ShellLine::Context { env, alias } => {
                let mut session = session.lock();
                session.env = env
                    .into_iter()
                    .map(|(key, value)| EnvironmentVariable {
                        key,
                        value: Some(value),
                    })
                    .collect();
                session.alias = Some(alias);
                session.context_dirty = true;
                continue;
            },
            ShellLine::EditBuffer { text, cursor } => {
                let context = {
                    let mut session = session.lock();
                    session.buffer.clone_from(&text);
                    session.cursor = cursor;
                    let full = std::mem::take(&mut session.context_dirty);
                    session.context(full)
                };
                HookRequest::EditBuffer(EditBufferHook {
                    context: Some(context),
                    text,
                    cursor: cursor as i64,
                    histno: 0,
                    terminal_cursor_coordinates: None,
                })
            },
            ShellLine::Prompt { exit_code, cwd } => {
                // The prompt first, then the command that just finished.
                let (context, finished) = {
                    let mut session = session.lock();
                    session.preexec = false;
                    session.cwd = Some(cwd);
                    let context = session.context(true);
                    let finished = session.running.take().map(|(command, start_time)| CommandInfo {
                        command: Some(command),
                        shell: Some(session.shell.clone()),
                        pid: session.pid,
                        session_id: Some(session.id.to_string()),
                        cwd: session.cwd.clone(),
                        start_time: Some(start_time),
                        end_time: Some(SystemTime::now()),
                        hostname: Some(session.user.clone()),
                        exit_code: Some(exit_code),
                    });
                    (context, finished)
                };
                dispatch_hook(
                    HookRequest::Prompt(PromptHook {
                        context: Some(context.clone()),
                    }),
                    session_id,
                    &shell_sessions,
                    &mut hook,
                )
                .await;
                let Some(finished) = finished else {
                    continue;
                };
                let post_exec = HookRequest::PostExec(PostExecHook {
                    context: Some(context),
                    command: finished.command.clone(),
                    exit_code: finished.exit_code,
                });
                hook.command_finished(finished).await;
                post_exec
            },
            ShellLine::PreExec { command } => {
                let (context, update) = {
                    let mut session = session.lock();
                    session.preexec = true;
                    session.running = Some((command.trim().to_owned(), SystemTime::now()));
                    // A running command gets its keys: interception stops.
                    session.interceptor.reset();
                    (session.context(true), session.intercept_update())
                };
                if let Some(update) = update {
                    let _ = out.send(update).await;
                }
                HookRequest::PreExec(PreExecHook {
                    context: Some(context),
                    command: None,
                })
            },
            ShellLine::Key { name } => {
                let (action, context, update) = {
                    let mut session = session.lock();
                    let action = session.interceptor.action(&name).map(str::to_owned);
                    if action.is_some() && name == "esc" {
                        session.interceptor.reset();
                    }
                    (action, session.context(true), session.intercept_update())
                };
                if let Some(update) = update {
                    let _ = out.send(update).await;
                }
                // The shell waits for this before doing anything else with the key; when
                // interception ended after its last `INTERCEPT`, zsh keeps the key after all.
                let verdict = match action.as_deref() {
                    None => KeyVerdict::Pass,
                    Some(action) if keys::may_insert(action) => KeyVerdict::MayInsert,
                    Some(_) => KeyVerdict::Taken,
                };
                let _ = out
                    .send(DaemonLine::Key {
                        name: name.clone(),
                        verdict,
                    })
                    .await;
                let Some(action) = action else {
                    continue;
                };
                HookRequest::InterceptedKey(InterceptedKeyHook {
                    context: Some(context),
                    key: name,
                    action,
                })
            },
        };
        dispatch_hook(request, session_id, &shell_sessions, &mut hook).await;
    }

    shell_sessions.remove_id(&session_id);
    command_task.abort();
    drop(out);
    let _ = writer_task.await;
    info!(%session_id, "zsh hook session disconnected");
}

async fn handle_commands(
    commands: flume::Receiver<ShellCommand>,
    session: Arc<Mutex<Session>>,
    out: mpsc::Sender<DaemonLine>,
) {
    let mut processes = tokio::task::JoinSet::new();
    loop {
        let command = tokio::select! {
            command = commands.recv_async() => match command { Ok(command) => command, Err(_) => break },
            _ = processes.join_next(), if !processes.is_empty() => continue,
        };
        match command {
            ShellCommand::ApplyEdit {
                expires_at,
                revision,
                cursor_utf16,
                buffer,
                edit,
                response,
            } => {
                // Reserve transport capacity before validating: waiting must not renew the lease.
                let permit = match tokio::time::timeout_at(expires_at.into(), out.reserve()).await {
                    Ok(Ok(permit)) => permit,
                    Ok(Err(_)) => break,
                    Err(_) => {
                        let _ = response.send(Err("edit expired before dispatch".into()));
                        continue;
                    },
                };
                if response.is_closed() || std::time::Instant::now() >= expires_at {
                    let _ = response.send(Err("edit cancelled or expired before dispatch".into()));
                    continue;
                }
                let line = {
                    let mut session = session.lock();
                    session.pending_edits.retain(|_, response| !response.is_closed());
                    if session.preexec
                        || session.revision != Some(revision)
                        || session.buffer != buffer
                        || session.buffer[..session.cursor].encode_utf16().count() as u64 != cursor_utf16
                    {
                        let _ = response.send(Err("stale input: edit rejected".into()));
                        continue;
                    }
                    if session.pending_edits.len() >= 16 {
                        let _ = response.send(Err("too many pending edits".into()));
                        continue;
                    }
                    let id = Uuid::new_v4().to_string();
                    session.pending_edits.insert(id.clone(), response);
                    DaemonLine::AtomicEdit {
                        id,
                        revision,
                        deadline_ms: SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis()
                            + expires_at
                                .saturating_duration_since(std::time::Instant::now())
                                .as_millis(),
                        cursor: session.buffer[..session.cursor].chars().count(),
                        buffer,
                        edit,
                    }
                };
                permit.send(line);
            },
            ShellCommand::InterceptFigJs {
                intercept_keystrokes,
                intercept_global_keystrokes,
                actions,
                override_actions,
            } => {
                let update = {
                    let mut session = session.lock();
                    session.interceptor.set_intercept_global(intercept_global_keystrokes);
                    session.interceptor.set_intercept(intercept_keystrokes);
                    session.interceptor.set_actions(&actions, override_actions);
                    session.intercept_update()
                };
                if let Some(update) = update {
                    let _ = out.send(update).await;
                }
            },
            ShellCommand::InterceptFigJSVisible { visible } => {
                let update = {
                    let mut session = session.lock();
                    session.interceptor.set_window_visible(visible);
                    session.intercept_update()
                };
                if let Some(update) = update {
                    let _ = out.send(update).await;
                }
            },
            ShellCommand::InsertText {
                insertion,
                deletion,
                offset,
                immediate,
                insertion_buffer,
                insert_during_command,
            } => {
                let request = InsertTextRequest {
                    insertion,
                    deletion: deletion.map(|deletion| deletion as u64),
                    offset,
                    immediate,
                    insertion_buffer,
                    insert_during_command,
                };
                let edits = {
                    let session = session.lock();
                    // A running command has no line editor to type into, and zsh would apply the
                    // edit to the next prompt instead.
                    if session.preexec {
                        debug!("dropping an insertion while a command runs");
                        continue;
                    }
                    insert::edits(&request, &session.buffer)
                };
                for edit in edits {
                    let _ = out.send(DaemonLine::Insert(edit)).await;
                }
            },
            ShellCommand::RunProcess {
                mut channel,
                executable,
                arguments,
                working_directory,
                env,
                timeout,
            } => {
                let shell = {
                    let session = session.lock();
                    process::Shell {
                        env: session.env.clone(),
                        cwd: session.cwd.clone(),
                    }
                };
                processes.spawn(async move {
                    let run = process::run(
                        process::Request {
                            executable,
                            arguments,
                            working_directory,
                            env,
                            timeout,
                        },
                        shell,
                    );
                    tokio::select! {
                        _ = channel.closed() => {},
                        response = run => { let _ = channel.send(response); },
                    }
                });
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct NoHooks;

    #[async_trait::async_trait]
    impl ShellHookHandler for NoHooks {
        type Error = std::convert::Infallible;

        async fn edit_buffer(
            &mut self,
            _: &EditBufferHook,
            _: Uuid,
            _: &Arc<ShellSessions>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn prompt(&mut self, _: &PromptHook, _: Uuid, _: &Arc<ShellSessions>) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn pre_exec(&mut self, _: &PreExecHook, _: Uuid, _: &Arc<ShellSessions>) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn post_exec(&mut self, _: &PostExecHook, _: Uuid, _: &Arc<ShellSessions>) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn intercepted_key(&mut self, _: InterceptedKeyHook, _: Uuid) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Writes `first` to a fresh connection and returns every line the desktop sends until it hangs
    /// up or a second passes, and whether it created a session.
    async fn exchange(first: &str) -> (Vec<String>, bool) {
        let (desktop, shell) = UnixStream::pair().unwrap();
        let state = Arc::new(ShellSessions::new());
        tokio::spawn(handle_connection(desktop, state.clone(), NoHooks));
        let (reader, mut writer) = shell.into_split();
        writer.write_all(format!("{first}\n").as_bytes()).await.unwrap();
        let mut lines = BufReader::new(reader).lines();
        let mut got = Vec::new();
        while let Ok(Ok(Some(line))) = tokio::time::timeout(Duration::from_secs(1), lines.next_line()).await {
            got.push(line);
        }
        let session = state.most_recent().is_some();
        (got, session)
    }

    #[tokio::test]
    async fn hello_with_our_version_starts_a_session() {
        let (lines, session) = exchange(&format!("HELLO {HOOK_PROTOCOL_VERSION} 1.2 zsh 1 /dev/ttys001 me@host")).await;
        assert_eq!(lines, vec![format!("WELCOME 1 {HOOK_PROTOCOL_VERSION} edits-v2")]);
        assert!(session);
    }

    #[tokio::test]
    async fn another_version_is_refused_and_hung_up_on() {
        for version in [0, HOOK_PROTOCOL_VERSION + 1] {
            let (lines, session) = exchange(&format!("HELLO {version} whatever")).await;
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert!(lines[0].starts_with("WELCOME 0 "), "{lines:?}");
            assert!(!session);
        }
        let (lines, session) = exchange("E 0 0 ").await;
        assert_eq!(lines, vec!["WELCOME 0 expected\\sHELLO".to_owned()]);
        assert!(!session);
    }

    #[tokio::test]
    async fn ping_is_answered_without_a_session() {
        let (lines, session) = exchange(wire::PING).await;
        assert_eq!(lines, vec![format!("PONG {HOOK_PROTOCOL_VERSION}")]);
        assert!(!session);
    }
}
