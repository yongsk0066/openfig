//! `hook.sock`: sessions of zsh shells that report through `fig.zsh` instead of the pty binary
//! (docs/15-architecture-study.md §3.3).
//!
//! Each connection becomes a [`SessionKind::ZshHook`] session in the same [`FigtermState`] the
//! pty binary's sessions live in, and its lines become the same hook requests
//! [`crate::remote`] decodes from protobuf, handed to the same [`dispatch_hook`]. What the desktop
//! sends a session ([`FigtermCommand`]) is carried out here instead of in a pty binary: key
//! interception and insertion become lines for the shell, and processes run in this process.

mod insert;
mod keys;
mod process;
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
use fig_proto::figterm::InsertTextRequest;
use fig_proto::local::{
    EditBufferHook,
    InterceptedKeyHook,
    PostExecHook,
    PreExecHook,
    PromptHook,
    ShellContext,
};
use fig_proto::remote::hostbound::request::Request;
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
    MAX_LINE_LEN,
    ShellLine,
};

use crate::RemoteHookHandler;
use crate::figterm::{
    EditBuffer,
    FigtermCommand,
    FigtermSession,
    FigtermState,
    InterceptMode,
    SessionKind,
};
use crate::remote::dispatch_hook;

/// How long a new connection has to say `HELLO`. `fig.zsh` sends it before reading anything.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Script versions a refusal has been logged for. Every tab opened before a rebuild refuses once
/// per shell, so the warning is written once per version and the rest go to `debug`.
static REFUSED_VERSIONS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

pub async fn start_hook_ipc(
    socket_path: PathBuf,
    figterm_state: Arc<FigtermState>,
    hook: impl RemoteHookHandler + Send + Clone + 'static,
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
        tokio::spawn(handle_connection(stream, figterm_state.clone(), hook.clone()));
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

async fn write_lines(mut writer: OwnedWriteHalf, mut lines: mpsc::UnboundedReceiver<DaemonLine>) {
    while let Some(line) = lines.recv().await {
        let mut text = line.to_string();
        text.push('\n');
        if let Err(err) = writer.write_all(text.as_bytes()).await {
            debug!(%err, "hook session write failed");
            break;
        }
    }
}

/// What a hook session has told the desktop so far. The pty binary keeps the same things in its
/// `ShellState` and builds every hook's `ShellContext` from them.
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
    /// The command line accepted by the last `X`, and when.
    running: Option<(String, SystemTime)>,
    interceptor: keys::Interceptor,
    /// The key set last sent as `INTERCEPT`.
    forwarded: BTreeSet<String>,
}

impl Session {
    /// With `full`, the environment and aliases too; the pty binary sends them with every hook but
    /// the edit-buffer ones, which carry them only after they changed.
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
    figterm_state: Arc<FigtermState>,
    mut hook: impl RemoteHookHandler + Send,
) {
    let (reader, writer) = stream.into_split();
    let mut lines = Lines {
        reader: BufReader::new(reader),
        buf: Vec::new(),
    };
    let (out, out_rx) = mpsc::unbounded_channel();
    let writer_task = tokio::spawn(write_lines(writer, out_rx));

    let hello = match tokio::time::timeout(HELLO_TIMEOUT, lines.next()).await {
        Ok(Ok(Some(line))) if line == wire::PING => {
            let _ = out.send(DaemonLine::Pong);
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
            let _ = out.send(DaemonLine::Refuse {
                reason: format!("protocol version {version}, this desktop speaks {HOOK_PROTOCOL_VERSION}"),
            });
            drop(out);
            let _ = writer_task.await;
            return;
        },
        other => {
            warn!(?other, "hook connection did not start with HELLO");
            let _ = out.send(DaemonLine::Refuse {
                reason: "expected HELLO".into(),
            });
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
        running: None,
        interceptor: keys::Interceptor::default(),
        forwarded: BTreeSet::new(),
    }));

    let (command_tx, command_rx) = flume::unbounded();
    figterm_state.insert(FigtermSession {
        id: session_id,
        kind: SessionKind::ZshHook,
        sender: command_tx,
        writer: None,
        edit_buffer: EditBuffer::default(),
        context: None,
        terminal_cursor_coordinates: None,
        response_map: HashMap::new(),
        nonce_counter: 0,
        intercept: InterceptMode::Unlocked,
        intercept_global: InterceptMode::Unlocked,
    });
    let command_task = tokio::spawn(handle_commands(command_rx, session.clone(), out.clone()));
    let _ = out.send(DaemonLine::Welcome);
    info!(%session_id, "zsh hook session connected");

    loop {
        let line = match lines.next().await {
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
        let request = match message {
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
                    let full = std::mem::take(&mut session.context_dirty);
                    session.context(full)
                };
                Request::EditBuffer(EditBufferHook {
                    context: Some(context),
                    text,
                    cursor: cursor as i64,
                    histno: 0,
                    terminal_cursor_coordinates: None,
                })
            },
            ShellLine::Prompt { exit_code, cwd } => {
                // As the pty binary: the prompt first, then the command that just finished.
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
                    Request::Prompt(PromptHook {
                        context: Some(context.clone()),
                    }),
                    session_id,
                    &figterm_state,
                    &mut hook,
                )
                .await;
                let Some(finished) = finished else {
                    continue;
                };
                let post_exec = Request::PostExec(PostExecHook {
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
                    // The pty binary stops intercepting when a command starts.
                    session.interceptor.reset();
                    (session.context(true), session.intercept_update())
                };
                if let Some(update) = update {
                    let _ = out.send(update);
                }
                Request::PreExec(PreExecHook {
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
                    let _ = out.send(update);
                }
                // The shell waits for this before doing anything else with the key; when
                // interception ended after its last `INTERCEPT`, zsh keeps the key after all.
                let _ = out.send(DaemonLine::Key {
                    name: name.clone(),
                    taken: action.is_some(),
                });
                let Some(action) = action else {
                    continue;
                };
                Request::InterceptedKey(InterceptedKeyHook {
                    context: Some(context),
                    key: name,
                    action,
                })
            },
        };
        dispatch_hook(request, session_id, &figterm_state, &mut hook).await;
    }

    figterm_state.remove_id(&session_id);
    command_task.abort();
    drop(out);
    let _ = writer_task.await;
    info!(%session_id, "zsh hook session disconnected");
}

async fn handle_commands(
    commands: flume::Receiver<FigtermCommand>,
    session: Arc<Mutex<Session>>,
    out: mpsc::UnboundedSender<DaemonLine>,
) {
    while let Ok(command) = commands.recv_async().await {
        match command {
            FigtermCommand::InterceptFigJs {
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
                    let _ = out.send(update);
                }
            },
            FigtermCommand::InterceptFigJSVisible { visible } => {
                let update = {
                    let mut session = session.lock();
                    session.interceptor.set_window_visible(visible);
                    session.intercept_update()
                };
                if let Some(update) = update {
                    let _ = out.send(update);
                }
            },
            FigtermCommand::InsertText {
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
                    let _ = out.send(DaemonLine::Insert(edit));
                }
            },
            FigtermCommand::RunProcess {
                channel,
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
                tokio::spawn(async move {
                    let response = process::run(
                        process::Request {
                            executable,
                            arguments,
                            working_directory,
                            env,
                            timeout,
                        },
                        shell,
                    )
                    .await;
                    let _ = channel.send(response);
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
    impl RemoteHookHandler for NoHooks {
        type Error = std::convert::Infallible;

        async fn edit_buffer(&mut self, _: &EditBufferHook, _: Uuid, _: &Arc<FigtermState>) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn prompt(&mut self, _: &PromptHook, _: Uuid, _: &Arc<FigtermState>) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn pre_exec(&mut self, _: &PreExecHook, _: Uuid, _: &Arc<FigtermState>) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn post_exec(&mut self, _: &PostExecHook, _: Uuid, _: &Arc<FigtermState>) -> Result<(), Self::Error> {
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
        let state = Arc::new(FigtermState::new());
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
        assert_eq!(lines, vec![format!("WELCOME 1 {HOOK_PROTOCOL_VERSION}")]);
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
