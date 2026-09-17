//! The zsh hook path in a real pty (docs/15-architecture-study.md §3.3, stage 3).
//!
//! [`HookShell`] runs `/bin/zsh -i` with a throwaway `HOME` and `ZDOTDIR` whose `.zshrc` loads this
//! build's integration through `init zsh pre` / `init zsh post`, so the shell sources `fig.zsh`
//! exactly as a user's would and no pty binary is involved. On the other end of
//! `<TMPDIR>/figrun/hook.sock` is a [`Daemon`]: the desktop's own server code with a handler that
//! records what reached it, or a bare stub for the failure cases.

use std::io::{
    Read as _,
    Write as _,
};
use std::path::{
    Path,
    PathBuf,
};
use std::sync::Arc;
use std::time::{
    Duration,
    Instant,
};

use anyhow::{
    Context as _,
    Result,
    bail,
};
use fig_proto::local::{
    EditBufferHook,
    InterceptedKeyHook,
    PostExecHook,
    PreExecHook,
    PromptHook,
};
use fig_remote_ipc::RemoteHookHandler;
use fig_remote_ipc::figterm::{
    FigtermCommand,
    FigtermState,
    SessionKind,
};
use fig_util::RUNTIME_DIR_NAME;
use portable_pty::{
    Child,
    CommandBuilder,
    PtyPair,
    PtySize,
    native_pty_system,
};
use tempfile::TempDir;
use tokio::io::{
    AsyncBufReadExt as _,
    AsyncWriteExt as _,
    BufReader,
};
use tokio::net::UnixListener;
use tokio::sync::mpsc::{
    UnboundedReceiver,
    UnboundedSender,
    unbounded_channel,
};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::golden;

/// How much terminal output [`HookShell::output`] keeps.
const OUTPUT_TAIL: usize = 256 * 1024;

/// What reached the other end of `hook.sock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// Decoded by the desktop's server and handed to `RemoteHookHandler::edit_buffer`.
    EditBuffer {
        text: String,
        cursor: i64,
    },
    Prompt {
        cwd: Option<String>,
    },
    PreExec,
    PostExec {
        command: Option<String>,
        exit_code: Option<i32>,
    },
    InterceptedKey {
        key: String,
        action: String,
    },
    /// A raw line, from the bare stubs.
    Line(String),
}

#[derive(Debug, Clone)]
struct Recorder(UnboundedSender<(Instant, HookEvent)>);

impl Recorder {
    fn record(&self, event: HookEvent) {
        // Stamped on arrival, the same point `Shell::keystroke` measures the pty binary's hook at.
        let _ = self.0.send((Instant::now(), event));
    }
}

#[async_trait::async_trait]
impl RemoteHookHandler for Recorder {
    type Error = anyhow::Error;

    async fn edit_buffer(&mut self, hook: &EditBufferHook, _: Uuid, _: &Arc<FigtermState>) -> Result<()> {
        self.record(HookEvent::EditBuffer {
            text: hook.text.clone(),
            cursor: hook.cursor,
        });
        Ok(())
    }

    async fn prompt(&mut self, hook: &PromptHook, _: Uuid, _: &Arc<FigtermState>) -> Result<()> {
        self.record(HookEvent::Prompt {
            cwd: hook.context.as_ref().and_then(|c| c.current_working_directory.clone()),
        });
        Ok(())
    }

    async fn pre_exec(&mut self, _: &PreExecHook, _: Uuid, _: &Arc<FigtermState>) -> Result<()> {
        self.record(HookEvent::PreExec);
        Ok(())
    }

    async fn post_exec(&mut self, hook: &PostExecHook, _: Uuid, _: &Arc<FigtermState>) -> Result<()> {
        self.record(HookEvent::PostExec {
            command: hook.command.clone(),
            exit_code: hook.exit_code,
        });
        Ok(())
    }

    async fn intercepted_key(&mut self, hook: InterceptedKeyHook, _: Uuid) -> Result<()> {
        self.record(HookEvent::InterceptedKey {
            key: hook.key,
            action: hook.action,
        });
        Ok(())
    }
}

/// Who is on the other end of `hook.sock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Daemon {
    /// `fig_remote_ipc::hook::start_hook_ipc`, the desktop's server, with a recording handler.
    Real,
    /// Nobody: the socket does not exist.
    Absent,
    /// Answers `HELLO` with `WELCOME 0` and hangs up, as a desktop of another protocol version.
    Refusing,
    /// Answers `WELCOME 1`, records raw lines, and can be sent lines or killed.
    Raw,
}

/// What goes in `.zshrc` between the integration's `pre` and `post` halves.
#[derive(Debug, Clone, Copy)]
pub enum Rc<'a> {
    /// A golden case's prelude and `zshrc`, exactly as it was recorded.
    Golden(&'a golden::Case),
    Text(&'a str),
}

pub struct HookShell {
    pty_pair: PtyPair,
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    pub tempdir: TempDir,
    pub zdotdir: PathBuf,
    socket: PathBuf,
    events: UnboundedReceiver<(Instant, HookEvent)>,
    /// Events a wait looked at, still owed to [`HookShell::events`].
    seen: Vec<HookEvent>,
    recorder: Recorder,
    output: Arc<std::sync::Mutex<Vec<u8>>>,
    output_at: UnboundedReceiver<Instant>,
    /// [`Daemon::Real`] only.
    pub figterm_state: Option<Arc<FigtermState>>,
    daemon: Vec<JoinHandle<()>>,
    /// [`Daemon::Raw`]: writers of the accepted connections.
    raw_writers: Arc<tokio::sync::Mutex<Vec<tokio::net::unix::OwnedWriteHalf>>>,
}

impl HookShell {
    pub async fn spawn(rc: Rc<'_>, daemon: Daemon) -> Result<HookShell> {
        Self::spawn_zsh(golden::ZSH, rc, daemon).await
    }

    /// With another zsh than `/bin/zsh`, e.g. the one on `PATH`.
    pub async fn spawn_zsh(zsh: &str, rc: Rc<'_>, daemon: Daemon) -> Result<HookShell> {
        let tempdir = TempDir::new()?;
        let home = tempdir.path().join("home");
        let zdotdir = tempdir.path().join("zdotdir");
        let runtime_dir = tempdir.path().join(RUNTIME_DIR_NAME);
        std::fs::create_dir_all(&home)?;
        let cli = crate::cli_binary()?;
        let (columns, rows) = match rc {
            Rc::Golden(case) => {
                case.write_zdotdir(&zdotdir, &cli, true, false)?;
                (case.columns, case.screen_lines)
            },
            Rc::Text(text) => {
                std::fs::create_dir_all(&zdotdir)?;
                std::fs::write(zdotdir.join(".zshenv"), "setopt no_global_rcs\n")?;
                let init = |when: &str| format!("eval \"$('{}' init zsh {when} --rcfile zshrc)\"\n", cli.display());
                std::fs::write(
                    zdotdir.join(".zshrc"),
                    format!("{}{text}\n{}", init("pre"), init("post")),
                )?;
                (80, 24)
            },
        };

        let socket = runtime_dir.join("hook.sock");
        let (events_tx, events) = unbounded_channel();
        let recorder = Recorder(events_tx);
        let raw_writers = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let mut figterm_state = None;
        let mut tasks = Vec::new();
        if daemon != Daemon::Absent {
            std::fs::create_dir_all(&runtime_dir)?;
        }
        match daemon {
            Daemon::Absent => {},
            Daemon::Real => {
                let state = Arc::new(FigtermState::new());
                figterm_state = Some(state.clone());
                let path = socket.clone();
                let recorder = recorder.clone();
                tasks.push(tokio::spawn(async move {
                    fig_remote_ipc::hook::start_hook_ipc(path, state, recorder)
                        .await
                        .unwrap();
                }));
                wait_for_socket(&socket).await?;
            },
            Daemon::Refusing | Daemon::Raw => {
                let listener = UnixListener::bind(&socket).context("binding hook.sock")?;
                tasks.push(tokio::spawn(raw_stub(
                    listener,
                    daemon == Daemon::Refusing,
                    recorder.clone(),
                    raw_writers.clone(),
                )));
            },
        }

        let pty_pair = native_pty_system().openpty(PtySize {
            rows,
            cols: columns,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new(zsh);
        cmd.arg("-i");
        golden::apply_env(&mut cmd, &home, &zdotdir, tempdir.path());
        cmd.cwd("/");
        let child = pty_pair.slave.spawn_command(cmd)?;
        let writer = pty_pair.master.take_writer()?;

        let mut reader = pty_pair.master.try_clone_reader()?;
        let (output_at_tx, output_at) = unbounded_channel();
        let output = Arc::new(std::sync::Mutex::new(Vec::new()));
        std::thread::spawn({
            let output = output.clone();
            move || {
                let mut buf = [0; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    let at = Instant::now();
                    if n == 0 {
                        break;
                    }
                    let _ = output_at_tx.send(at);
                    let mut output = output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    output.extend_from_slice(&buf[..n]);
                    let excess = output.len().saturating_sub(OUTPUT_TAIL);
                    output.drain(..excess);
                }
            }
        });

        let mut shell = HookShell {
            pty_pair,
            writer,
            child,
            tempdir,
            zdotdir,
            socket,
            events,
            seen: Vec::new(),
            recorder,
            output,
            output_at,
            figterm_state,
            daemon: tasks,
            raw_writers,
        };

        // Up means the first prompt is drawn and, with a daemon, its line-init report arrived.
        let deadline = Instant::now() + Duration::from_secs(20);
        match daemon {
            Daemon::Real | Daemon::Raw => {
                shell
                    .wait_for_event(deadline, |event| {
                        matches!(event, HookEvent::EditBuffer { .. })
                            || matches!(event, HookEvent::Line(line) if line.starts_with("E "))
                    })
                    .await
                    .context("the first prompt never reported its edit buffer")?;
            },
            // Nothing reports the prompt: wait for zsh to draw something and go quiet.
            _ => {
                tokio::time::timeout(
                    deadline.saturating_duration_since(Instant::now()),
                    shell.output_at.recv(),
                )
                .await
                .context("no output from zsh")?;
                shell
                    .settle(Duration::from_millis(500), Duration::from_secs(20))
                    .await?;
            },
        }
        shell
            .settle(Duration::from_millis(150), Duration::from_secs(10))
            .await?;
        Ok(shell)
    }

    /// Until an event `found` accepts has arrived. Nothing is consumed: [`HookShell::events`] still
    /// returns everything.
    pub async fn wait_for_event(&mut self, deadline: Instant, found: impl Fn(&HookEvent) -> bool) -> Result<()> {
        if self.seen.iter().any(&found) {
            return Ok(());
        }
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, self.events.recv()).await {
                Ok(Some((_, event))) => {
                    let done = found(&event);
                    self.seen.push(event);
                    if done {
                        return Ok(());
                    }
                },
                Ok(None) => bail!("the daemon stopped"),
                Err(_) => bail!("timed out; terminal output:\n{}", self.output()),
            }
        }
    }

    /// Waits for the next event `found` accepts and returns when it arrived. Events passed over
    /// stay owed to [`HookShell::events`].
    pub async fn arrival(&mut self, timeout: Duration, found: impl Fn(&HookEvent) -> bool) -> Result<Instant> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, self.events.recv()).await {
                Ok(Some((at, event))) if found(&event) => return Ok(at),
                Ok(Some((_, event))) => self.seen.push(event),
                Ok(None) => bail!("the daemon stopped"),
                Err(_) => bail!("timed out after {timeout:?}; events so far: {:?}", self.seen),
            }
        }
    }

    /// Writes `key` to the pty and returns.
    pub fn press(&mut self, key: &str) -> Result<()> {
        self.writer.write_all(key.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    /// One write per character, with a pause, the way a person types.
    pub async fn type_text(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            self.press(&c.to_string())?;
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        Ok(())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.pty_pair.master.resize(PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Until the terminal has been quiet for `quiet`.
    pub async fn settle(&mut self, quiet: Duration, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            match tokio::time::timeout(quiet, self.output_at.recv()).await {
                Err(_) => return Ok(()),
                Ok(Some(_)) if Instant::now() < deadline => continue,
                Ok(Some(_)) => bail!("output did not settle within {timeout:?}"),
                Ok(None) => bail!("the pty closed"),
            }
        }
    }

    /// Everything that reached the daemon since the last call, oldest first.
    pub fn events(&mut self) -> Vec<HookEvent> {
        let mut events = std::mem::take(&mut self.seen);
        events.extend(std::iter::from_fn(|| {
            self.events.try_recv().ok().map(|(_, event)| event)
        }));
        events
    }

    /// The edit buffers among [`HookShell::events`], as a golden recording spells them.
    pub fn edit_buffers(&mut self) -> Vec<golden::EditBuffer> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                HookEvent::EditBuffer { text, cursor } => Some(golden::EditBuffer { text, cursor }),
                _ => None,
            })
            .collect()
    }

    /// Writes `key` and waits for the edit buffer `text` to arrive at the daemon: how long that
    /// took, and how long until the first output came back (the echo the user sees). Both from
    /// the write to the pty master, as `Shell::keystroke` measures the pty binary.
    pub async fn keystroke(
        &mut self,
        key: &str,
        text: &str,
        timeout: Duration,
    ) -> Result<(Duration, Option<Duration>)> {
        self.events();
        while self.output_at.try_recv().is_ok() {}
        let start = Instant::now();
        self.press(key)?;
        let deadline = start + timeout;
        let hook = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, self.events.recv()).await {
                Ok(Some((at, HookEvent::EditBuffer { text: got, .. }))) if got == text => {
                    break at.saturating_duration_since(start);
                },
                Ok(Some(_)) => continue,
                Ok(None) => bail!("the daemon stopped"),
                Err(_) => bail!("no edit buffer {text:?} within {timeout:?} after {key:?}"),
            }
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        let echo = match tokio::time::timeout(remaining, self.output_at.recv()).await {
            Ok(Some(at)) => Some(at.saturating_duration_since(start)),
            _ => None,
        };
        Ok((hook, echo))
    }

    /// The last [`OUTPUT_TAIL`] bytes of terminal output.
    pub fn output(&self) -> String {
        let output = self.output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&output).into_owned()
    }

    pub async fn wait_for_output(&self, needle: &str, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while !self.output().contains(needle) {
            if Instant::now() > deadline {
                bail!(
                    "{needle:?} not in the terminal output within {timeout:?}:\n{}",
                    self.output()
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }

    /// Runs `command` and waits for `marker` in the output.
    pub async fn run(&mut self, command: &str, marker: &str) -> Result<()> {
        self.type_text(command).await?;
        self.press("\r")?;
        self.wait_for_output(marker, Duration::from_secs(10)).await
    }

    /// Sends `command` to the only hook session, the way the desktop's request handlers do.
    pub fn command(&self, command: FigtermCommand) -> Result<()> {
        let state = self.figterm_state.as_ref().context("not a Daemon::Real shell")?;
        let sender = state
            .inner
            .lock()
            .linked_sessions
            .values()
            .find(|session| session.kind == SessionKind::ZshHook)
            .map(|session| session.sender.clone())
            .context("no hook session")?;
        sender.send(command)?;
        Ok(())
    }

    /// Hook sessions the desktop currently has.
    pub fn sessions(&self) -> usize {
        self.figterm_state.as_ref().map_or(0, |state| {
            state
                .inner
                .lock()
                .linked_sessions
                .values()
                .filter(|session| session.kind == SessionKind::ZshHook)
                .count()
        })
    }

    /// [`Daemon::Raw`]: writes `line` to every connection.
    pub async fn send_line(&self, line: &str) -> Result<()> {
        for writer in self.raw_writers.lock().await.iter_mut() {
            writer.write_all(format!("{line}\n").as_bytes()).await?;
        }
        Ok(())
    }

    /// Stops the daemon: listener and connections go, and so does the socket file.
    pub async fn kill_daemon(&mut self) {
        for task in self.daemon.drain(..) {
            task.abort();
            let _ = task.await;
        }
        for mut writer in self.raw_writers.lock().await.drain(..) {
            let _ = writer.shutdown().await;
        }
        let _ = std::fs::remove_file(&self.socket);
    }

    /// Starts a [`Daemon::Raw`] stub on the socket after the shell is already up.
    pub fn start_raw_daemon(&mut self) -> Result<()> {
        std::fs::create_dir_all(self.socket.parent().context("socket has no parent")?)?;
        let listener = UnixListener::bind(&self.socket)?;
        self.daemon.push(tokio::spawn(raw_stub(
            listener,
            false,
            self.recorder.clone(),
            self.raw_writers.clone(),
        )));
        Ok(())
    }
}

impl Drop for HookShell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for task in &self.daemon {
            task.abort();
        }
    }
}

async fn wait_for_socket(socket: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        if Instant::now() > deadline {
            bail!("{} was never bound", socket.display());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}

/// A daemon that knows nothing but the handshake: `WELCOME 0` and hang up, or `WELCOME 1` and
/// record every line as [`HookEvent::Line`].
async fn raw_stub(
    listener: UnixListener,
    refuse: bool,
    recorder: Recorder,
    writers: Arc<tokio::sync::Mutex<Vec<tokio::net::unix::OwnedWriteHalf>>>,
) {
    // Aborting this task has to take the connections with it, or a killed daemon keeps listening.
    struct AbortOnDrop(Vec<JoinHandle<()>>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.iter().for_each(JoinHandle::abort);
        }
    }
    let mut connections = AbortOnDrop(Vec::new());
    while let Ok((stream, _)) = listener.accept().await {
        let (reader, mut writer) = stream.into_split();
        let recorder = recorder.clone();
        let mut lines = BufReader::new(reader).lines();
        if refuse {
            if let Ok(Some(line)) = lines.next_line().await {
                recorder.record(HookEvent::Line(line));
            }
            let _ = writer.write_all(b"WELCOME 0 test\n").await;
            continue;
        }
        let _ = writer
            .write_all(format!("WELCOME 1 {}\n", fig_util::HOOK_PROTOCOL_VERSION).as_bytes())
            .await;
        writers.lock().await.push(writer);
        connections.0.push(tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                recorder.record(HookEvent::Line(line));
            }
        }));
    }
}
