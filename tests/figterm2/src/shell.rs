use std::io::{
    Write,
    stdout,
};
use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::{
    Duration,
    Instant,
};

use anyhow::{
    Context,
    Result,
};
use fig_ipc::{
    BufferedReader,
    RecvMessage as _,
    SendMessage as _,
};
use fig_proto::local::{
    EditBufferHook,
    InterceptedKeyHook,
    PostExecHook,
    PreExecHook,
    PromptHook,
    ShellContext,
};
use fig_proto::remote::clientbound::HandshakeResponse;
use fig_proto::remote::clientbound::handshake_response::RejectReason;
use fig_proto::remote::{
    Clientbound,
    Hostbound,
    PROTOCOL_VERSION,
    clientbound,
    hostbound,
};
use fig_remote_ipc::RemoteHookHandler;
use fig_remote_ipc::figterm::FigtermState;
use fig_util::RUNTIME_DIR_NAME;
use portable_pty::{
    Child,
    CommandBuilder,
    PtyPair,
    PtySize,
    native_pty_system,
};
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{
    UnboundedReceiver,
    UnboundedSender,
    unbounded_channel,
};
use uuid::Uuid;

/// Session id handed to the pty binary; it names the socket under `<runtime dir>/t/`.
const SESSION_ID: &str = "1234";

/// How much of the terminal output [`Shell::wait_for_output`] can search: the most recent bytes.
const OUTPUT_TAIL: usize = 64 * 1024;

/// Environment variables that make `q _ should-figterm-launch` refuse to wrap the shell
/// (`crates/fig_cli/src/cli/internal/should_figterm_launch.rs`), or that `pre.sh` reads as "already
/// wrapped". The spawned shell inherits the test runner's environment, so any of these in the
/// caller's shell turns every test here into "figterm never connected": `CI=1` did exactly that
/// (docs/15-architecture-study.md §2.6 #11), and pushing over ssh would do the same through
/// `SSH_*`.
const LAUNCH_BLOCKING_ENV: &[&str] = &[
    "CI",
    "FIG_CI",
    "FIG_TERM",
    "FIG_TERM_TMUX",
    "FIG_TERM_DISABLED",
    "FIGTERM_SESSION_ID",
    "PROCESS_LAUNCHED_BY_FIG",
    "INSIDE_EMACS",
    "TERM_PROGRAM",
    "__CFBundleIdentifier",
    "__PWSH_LOGIN_CHECKED",
    "SSH_CLIENT",
    "SSH_CONNECTION",
    "SSH_TTY",
    "CODESPACES",
    "FIG_CODESPACES",
    "SHOULD_FIGTERM_LAUNCH",
];

/// One `EditBufferHook` as the stub desktop saw it.
#[derive(Debug, Clone)]
pub struct EditBufferEvent {
    /// Taken on arrival, after the hook has been read off the socket and decoded, before anything
    /// else is done with it — the same point where the desktop's handler starts.
    pub at: Instant,
    pub text: String,
    pub cursor: i64,
}

#[derive(Debug, Clone)]
struct RemoteHook {
    buffer: Arc<Mutex<Option<String>>>,
    shell_context: Arc<Mutex<Option<ShellContext>>>,
    events: UnboundedSender<EditBufferEvent>,
}

#[async_trait::async_trait]
impl RemoteHookHandler for RemoteHook {
    type Error = anyhow::Error;

    async fn edit_buffer(
        &mut self,
        edit_buffer_hook: &EditBufferHook,
        _session_id: Uuid,
        _figterm_state: &Arc<FigtermState>,
    ) -> Result<(), Self::Error> {
        let at = Instant::now();
        // The receiver lives as long as the `Shell`; after that nobody is listening.
        let _ = self.events.send(EditBufferEvent {
            at,
            text: edit_buffer_hook.text.clone(),
            cursor: edit_buffer_hook.cursor,
        });
        *self.buffer.lock().await = Some(edit_buffer_hook.text.clone());
        Ok(())
    }

    async fn prompt(
        &mut self,
        _prompt_hook: &PromptHook,
        _session_id: Uuid,
        _figterm_state: &Arc<FigtermState>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn pre_exec(
        &mut self,
        _pre_exec_hook: &PreExecHook,
        _session_id: Uuid,
        _figterm_state: &Arc<FigtermState>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn post_exec(
        &mut self,
        _post_exec_hook: &PostExecHook,
        _session_id: Uuid,
        _figterm_state: &Arc<FigtermState>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn intercepted_key(
        &mut self,
        _intercepted_key: InterceptedKeyHook,
        _session_id: Uuid,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn shell_context(&mut self, context: &ShellContext, _session_id: Uuid) {
        *self.shell_context.lock().await = Some(context.clone());
    }
}

/// Absolute path of the freshly built pty binary (`target/<profile>/<PTY_BINARY_NAME>`).
///
/// The test binary lives in `target/<profile>/deps/`, so the sibling of its grandparent directory
/// is the profile directory. `cargo nextest run` builds the workspace's bin targets, so this is
/// always present when the tests are run through the normal gate.
pub fn pty_binary() -> Result<std::path::PathBuf> {
    let test_exe = std::env::current_exe().context("no current_exe")?;
    let profile_dir = test_exe
        .parent()
        .and_then(|deps| deps.parent())
        .context("test binary is not under target/<profile>/deps")?;
    let path = profile_dir.join(fig_util::consts::PTY_BINARY_NAME);
    if !path.exists() {
        anyhow::bail!(
            "{} was not built at {}; run the tests through `cargo nextest run --workspace`",
            fig_util::consts::PTY_BINARY_NAME,
            path.display()
        );
    }
    Ok(path)
}

/// Path to the workspace build of the CLI, for the same reason as [`pty_binary`].
///
/// The generated shell integration calls the CLI by bare name (`q _ should-figterm-launch`,
/// `q _ get-shell`), so this goes on the spawned shell's `PATH` under that name.
pub fn cli_binary() -> Result<std::path::PathBuf> {
    let test_exe = std::env::current_exe().context("no current_exe")?;
    let profile_dir = test_exe
        .parent()
        .and_then(|deps| deps.parent())
        .context("test binary is not under target/<profile>/deps")?;
    let path = profile_dir.join(fig_util::consts::CLI_CRATE_NAME);
    if !path.exists() {
        anyhow::bail!(
            "{} was not built at {}; run the tests through `cargo nextest run --workspace`",
            fig_util::consts::CLI_CRATE_NAME,
            path.display()
        );
    }
    Ok(path)
}

/// What a [`Desktop::Refusing`] stub saw.
#[derive(Debug, Default)]
pub struct Refusals {
    /// `protocol_version` of every handshake, in arrival order.
    pub handshakes: Vec<u32>,
    /// Packets other than handshakes. A refused pty binary has no business sending any.
    pub other_packets: usize,
}

/// Answers every handshake on `listener` the way a desktop built for another protocol version
/// does (`fig_remote_ipc::remote`), and records what arrived.
async fn refuse_handshakes(listener: UnixListener, refusals: Arc<Mutex<Refusals>>) {
    while let Ok((stream, _)) = listener.accept().await {
        let refusals = refusals.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(stream);
            let mut reader = BufferedReader::new(reader);
            while let Ok(Some(message)) = reader.recv_message::<Hostbound>().await {
                let Some(hostbound::Packet::Handshake(handshake)) = message.packet else {
                    refusals.lock().await.other_packets += 1;
                    continue;
                };
                refusals.lock().await.handshakes.push(handshake.protocol_version);
                let response = Clientbound {
                    packet: Some(clientbound::Packet::HandshakeResponse(HandshakeResponse {
                        success: false,
                        reject_reason: RejectReason::ProtocolVersionMismatch.into(),
                        protocol_version: PROTOCOL_VERSION + 1,
                    })),
                };
                if writer.send_message(response).await.is_err() {
                    break;
                }
            }
        });
    }
}

pub struct Shell {
    // pub remote_socket: UnixListener,
    /// Bound for [`Desktop::Stub`] only.
    pub desktop_socket: Option<UnixListener>,
    /// `<TMPDIR>/<RUNTIME_DIR_NAME>` as the pty binary sees it.
    runtime_dir: std::path::PathBuf,

    pty_pair: PtyPair,
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    pub tempdir: TempDir,

    buffer: Arc<Mutex<Option<String>>>,
    pub shell_context: Arc<Mutex<Option<ShellContext>>>,
    events: UnboundedReceiver<EditBufferEvent>,
    /// Arrival time of every chunk of terminal output, as the terminal emulator would see it.
    output_at: UnboundedReceiver<Instant>,
    /// The last [`OUTPUT_TAIL`] bytes of terminal output.
    output: Arc<std::sync::Mutex<Vec<u8>>>,
    /// Filled by [`Desktop::Refusing`] only.
    pub refusals: Arc<Mutex<Refusals>>,
}

/// What one keystroke cost, both measured from the `write` to the pty master.
#[derive(Debug, Clone, Copy)]
pub struct KeystrokeTiming {
    /// Until the first byte of output came back through the pty binary: what the user sees.
    pub echo: Option<Duration>,
    /// Until the edit buffer hook for the new text was decoded by the stub desktop.
    pub hook: Duration,
}

/// Who is on the other end of `remote.sock`.
#[derive(Debug, Clone)]
pub enum Desktop {
    /// An in-process stub (`fig_remote_ipc::remote::start_remote_ipc`, the desktop's own server
    /// code) in the test's private runtime dir. Hooks show up in [`Shell::buffer`] and
    /// [`Shell::keystroke`].
    Stub,
    /// An in-process stub that refuses every handshake with
    /// `REJECT_REASON_PROTOCOL_VERSION_MISMATCH`, as the desktop does for a pty binary from
    /// another build. What reached it is in [`Shell::refusals`]; no hooks are observed.
    Refusing,
    /// A desktop that is already running with `TMPDIR` set to this directory — an isolated
    /// `fig_desktop --allow-multiple`, never the installed app. The pty binary connects to it
    /// instead; nothing is observed on this side, so only [`Shell::press`] is useful.
    External(std::path::PathBuf),
}

/// Where the shell's terminal output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Echo {
    /// Copied to the test's stdout, which nextest captures and shows on failure.
    Stdout,
    /// Read and dropped. The pty still has to be drained or the pty binary blocks on write.
    Discard,
}

impl Shell {
    pub async fn init(shell: &str) -> Result<Shell> {
        Self::spawn(shell, Echo::Stdout).await
    }

    pub async fn spawn(shell: &str, echo: Echo) -> Result<Shell> {
        Self::spawn_with(shell, echo, Desktop::Stub).await
    }

    pub async fn spawn_with(shell: &str, echo: Echo, desktop: Desktop) -> Result<Shell> {
        let tempdir = TempDir::new()?;
        println!("{tempdir:?}");

        // `TMPDIR` of the pty binary, which puts its runtime dir (and so `remote.sock`) under it.
        let runtime_parent = match &desktop {
            Desktop::Stub | Desktop::Refusing => tempdir.path().to_path_buf(),
            Desktop::External(tmpdir) => tmpdir.clone(),
        };
        let runtime_dir = runtime_parent.join(RUNTIME_DIR_NAME);
        println!("{runtime_dir:?} {}", runtime_dir.to_str().unwrap().len());

        let buffer = Arc::new(Mutex::new(None));
        let shell_context = Arc::new(Mutex::new(None));
        let (events_tx, events) = unbounded_channel();
        let refusals = Arc::new(Mutex::new(Refusals::default()));

        let desktop_socket = match &desktop {
            Desktop::Stub => {
                let figterm_state = Arc::new(FigtermState::new());
                tokio::fs::create_dir_all(&runtime_dir).await?;
                tokio::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700)).await?;

                let path = runtime_dir.join("remote.sock");
                tokio::spawn({
                    let buffer = buffer.clone();
                    let shell_context = shell_context.clone();
                    async move {
                        fig_remote_ipc::remote::start_remote_ipc(path, figterm_state.clone(), RemoteHook {
                            buffer,
                            shell_context,
                            events: events_tx,
                        })
                        .await
                        .unwrap();
                    }
                });

                Some(UnixListener::bind(runtime_dir.join("desktop.sock")).context("Failed to make desktop.socket")?)
            },
            Desktop::Refusing => {
                tokio::fs::create_dir_all(&runtime_dir).await?;
                tokio::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700)).await?;

                let listener =
                    UnixListener::bind(runtime_dir.join("remote.sock")).context("Failed to bind remote.sock")?;
                tokio::spawn(refuse_handshakes(listener, refusals.clone()));

                Some(UnixListener::bind(runtime_dir.join("desktop.sock")).context("Failed to make desktop.socket")?)
            },
            Desktop::External(_) => {
                if !runtime_dir.join("remote.sock").exists() {
                    anyhow::bail!("no desktop listening at {}", runtime_dir.join("remote.sock").display());
                }
                None
            },
        };

        let pty_system = native_pty_system();

        // Create a new pty
        let pty_pair = pty_system.openpty(PtySize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Spawn a shell into the pty
        let mut cmd = CommandBuilder::new(shell);

        // A throwaway `HOME` whose rc file sources *this build's* shell integration.
        //
        // The shell used to inherit the real `HOME`, so it read the installed app's rc hook —
        // `eval "$(~/.local/bin/q init bash pre …)"` — and the test measured whatever version was
        // installed on the machine. That is how the `Q_*` -> `FIG_*` rename first showed up here:
        // the installed CLI still emitted `Q_TERM_PATH`, so the `FIG_TERM_PATH` below was ignored,
        // the installed pty binary ran instead of the workspace one, and it did not know
        // `MOCK_FIGTERM_SESSION_ID` — which surfaced as `path must be shorter than SUN_LEN`,
        // because a random session UUID makes the socket path 32 characters longer than `1234`.
        let home = tempdir.path().join("home");
        tokio::fs::create_dir_all(&home).await?;
        let cli = cli_binary()?;
        let (rc_name, rc_file) = match shell {
            "zsh" => ("zshrc", ".zshrc"),
            _ => ("bashrc", ".bashrc"),
        };
        tokio::fs::write(
            home.join(rc_file),
            format!(
                "eval \"$({cli} init {shell} pre --rcfile {rc_name})\"\n\
                 eval \"$({cli} init {shell} post --rcfile {rc_name})\"\n",
                cli = cli.display(),
            ),
        )
        .await?;

        // The integration calls the CLI and tests for the pty binary by bare name.
        let bin = tempdir.path().join("bin");
        tokio::fs::create_dir_all(&bin).await?;
        tokio::fs::symlink(&cli, bin.join(fig_util::consts::CLI_BINARY_NAME)).await?;
        tokio::fs::symlink(pty_binary()?, bin.join(fig_util::consts::PTY_BINARY_NAME)).await?;

        for key in LAUNCH_BLOCKING_ENV {
            cmd.env_remove(key);
        }
        cmd.env("HOME", &home);
        cmd.env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
            ),
        );
        // `pre.sh` honours a pre-set `FIG_TERM_PATH`; without it the shell integration would exec
        // whatever pty binary is first on `PATH`.
        cmd.env("FIG_TERM_PATH", pty_binary()?);
        cmd.env("FIG_NEW_SESSION", "1");
        cmd.env("MOCK_FIGTERM_SESSION_ID", SESSION_ID);
        cmd.env("TMPDIR", &runtime_parent);
        cmd.env("XDG_RUNTIME_DIR", &runtime_parent);

        let child = pty_pair.slave.spawn_command(cmd)?;
        let writer = pty_pair.master.take_writer()?;

        let mut res = pty_pair.master.try_clone_reader().unwrap();
        let (output_at_tx, output_at) = unbounded_channel();
        let output = Arc::new(std::sync::Mutex::new(Vec::new()));
        std::thread::spawn({
            let output = output.clone();
            move || {
                let mut buf = [0; 1024];
                while let Ok(a) = res.read(&mut buf) {
                    let at = Instant::now();
                    if a == 0 {
                        break;
                    }
                    let _ = output_at_tx.send(at);
                    {
                        let mut output = output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        output.extend_from_slice(&buf[..a]);
                        let excess = output.len().saturating_sub(OUTPUT_TAIL);
                        output.drain(..excess);
                    }
                    if echo == Echo::Stdout {
                        stdout().write_all(&buf[..a]).unwrap();
                        stdout().flush().unwrap();
                    }
                }
            }
        });

        // Wait for the pty binary to come up and hand us its shell context, rather than guessing
        // a fixed delay: the tests run right after a build, when the machine is still busy, and a
        // 500 ms guess made them flaky.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        match desktop {
            Desktop::Stub => {
                while shell_context.lock().await.is_none() {
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!("{} never connected within 15 s", fig_util::consts::PTY_BINARY_NAME);
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            },
            Desktop::Refusing => {
                while refusals.lock().await.handshakes.is_empty() {
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!(
                            "{} never attempted a handshake within 15 s",
                            fig_util::consts::PTY_BINARY_NAME
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            },
            // Nothing to observe the handshake with; the session socket is bound right after it.
            Desktop::External(_) => {
                let session_socket = runtime_dir.join("t").join(format!("{SESSION_ID}.sock"));
                while !session_socket.exists() {
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!(
                            "{} never bound {}",
                            fig_util::consts::PTY_BINARY_NAME,
                            session_socket.display()
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            },
        }

        Ok(Shell {
            desktop_socket,
            runtime_dir,

            pty_pair,
            writer,
            child,
            tempdir,

            buffer,
            shell_context,
            events,
            output_at,
            output,
            refusals,
        })
    }

    /// Waits until `needle` shows up in the terminal output, as the user would see it scroll by.
    pub async fn wait_for_output(&self, needle: &str, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let output = {
                let output = self.output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                String::from_utf8_lossy(&output).into_owned()
            };
            if output.contains(needle) {
                return Ok(());
            }
            if Instant::now() > deadline {
                anyhow::bail!("{needle:?} not in the terminal output within {timeout:?}; output so far:\n{output}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Writes `key` to the pty and returns without waiting for anything.
    pub fn press(&mut self, key: &str) -> Result<()> {
        self.writer.write_all(key.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    pub async fn typed(&mut self, text: &str) -> Result<()> {
        println!("tying: {text}");
        for c in text.chars() {
            self.writer.write_all(c.to_string().as_bytes())?;
            self.writer.flush()?;

            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;

        Ok(())
    }

    /// Writes `text` to the pty in a single `write` the way a terminal delivers a paste, instead
    /// of one keystroke every 10 ms. The point is to hand the parser a burst it cannot drain
    /// between events.
    pub async fn pasted(&mut self, text: &str) -> Result<()> {
        println!("pasting: {text:?}");
        self.writer.write_all(text.as_bytes())?;
        self.writer.flush()?;
        tokio::time::sleep(Duration::from_millis(2000)).await;
        Ok(())
    }

    /// Writes `key` to the pty and measures how long it took until an edit buffer hook with `text`
    /// reached the stub desktop, and until the first output came back.
    ///
    /// The clock starts right before the `write` to the pty master — the moment a terminal
    /// emulator hands the keystroke over — and the hook clock stops when the hook is decoded on
    /// the desktop side of `remote.sock`. In between: the pty binary reads the key and forwards it
    /// to the shell, the line editor echoes and redraws, the pty binary parses that output back
    /// into an edit buffer and sends the hook. Anything that arrived before the write (a late
    /// redraw of the previous key) is discarded first so it cannot be taken for the answer.
    pub async fn keystroke(&mut self, key: &str, text: &str, timeout: Duration) -> Result<KeystrokeTiming> {
        while self.events.try_recv().is_ok() {}
        while self.output_at.try_recv().is_ok() {}

        let start = Instant::now();
        self.writer.write_all(key.as_bytes())?;
        self.writer.flush()?;

        let deadline = start + timeout;
        let hook = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(remaining, self.events.recv()).await {
                Ok(Some(event)) if event.text == text => break event.at.saturating_duration_since(start),
                Ok(Some(_)) => continue,
                Ok(None) => anyhow::bail!("the stub desktop stopped"),
                Err(_) => anyhow::bail!("no edit buffer {text:?} within {timeout:?} after writing {key:?}"),
            }
        };

        let remaining = deadline.saturating_duration_since(Instant::now());
        let echo = match tokio::time::timeout(remaining, self.output_at.recv()).await {
            Ok(Some(at)) => Some(at.saturating_duration_since(start)),
            _ => None,
        };

        Ok(KeystrokeTiming { echo, hook })
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.pty_pair.master.resize(PtySize {
            cols,
            rows,
            ..Default::default()
        })?;
        Ok(())
    }

    pub async fn reset(&mut self) -> Result<()> {
        self.typed("\n").await?;
        self.resize(80, 24)?;
        Ok(())
    }

    /// Pid of the pty binary. The shell we spawned `exec`s into it, so the pid is unchanged.
    pub fn pty_pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// True when the pty is in cooked mode (`ECHO` and `ICANON` set), i.e. the state a shell is
    /// normally left in. The pty binary clears both while it runs and is supposed to put them back
    /// on the way out.
    pub fn is_cooked(&self) -> bool {
        let fd = self.pty_pair.master.as_raw_fd().expect("no master fd");
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(fd, &mut termios) }, 0, "tcgetattr failed");
        (termios.c_lflag & libc::ECHO) != 0 && (termios.c_lflag & libc::ICANON) != 0
    }

    /// Path of the session socket the pty binary binds inside this test's runtime dir.
    pub fn session_socket(&self) -> std::path::PathBuf {
        self.runtime_dir.join("t").join(format!("{SESSION_ID}.sock"))
    }

    pub async fn buffer(&mut self) -> Option<String> {
        self.buffer.lock().await.clone()
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}
