use std::io::{
    Write,
    stdout,
};
use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{
    Context,
    Result,
};
use fig_proto::local::{
    EditBufferHook,
    InterceptedKeyHook,
    PostExecHook,
    PreExecHook,
    PromptHook,
    ShellContext,
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
use uuid::Uuid;

#[derive(Debug, Clone)]
struct RemoteHook {
    buffer: Arc<Mutex<Option<String>>>,
    shell_context: Arc<Mutex<Option<ShellContext>>>,
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
fn pty_binary() -> Result<std::path::PathBuf> {
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

pub struct Shell {
    // pub remote_socket: UnixListener,
    pub desktop_socket: UnixListener,

    pty_pair: PtyPair,
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    pub tempdir: TempDir,

    buffer: Arc<Mutex<Option<String>>>,
    pub shell_context: Arc<Mutex<Option<ShellContext>>>,
}

impl Shell {
    pub async fn init(shell: &str) -> Result<Shell> {
        let tempdir = TempDir::new()?;
        println!("{tempdir:?}");

        let figterm_state = Arc::new(FigtermState::new());

        let runtime_dir = tempdir.path().join(RUNTIME_DIR_NAME);
        tokio::fs::create_dir_all(&runtime_dir).await?;
        tokio::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700)).await?;

        println!("{runtime_dir:?} {}", runtime_dir.to_str().unwrap().len());

        let path = runtime_dir.join("remote.sock");
        let buffer = Arc::new(Mutex::new(None));
        let shell_context = Arc::new(Mutex::new(None));
        tokio::spawn({
            let buffer = buffer.clone();
            let shell_context = shell_context.clone();
            async move {
                fig_remote_ipc::remote::start_remote_ipc(path, figterm_state.clone(), RemoteHook {
                    buffer,
                    shell_context,
                })
                .await
                .unwrap();
            }
        });

        let desktop_socket =
            UnixListener::bind(runtime_dir.join("desktop.sock")).context("Failed to make desktop.socket")?;

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

        let session_id = "1234";
        // `pre.sh` honours a pre-set `Q_TERM_PATH`; without it the shell integration would exec
        // whatever pty binary is *installed* on this machine, so the test would silently validate
        // the installed app instead of the workspace build.
        cmd.env("Q_TERM_PATH", pty_binary()?);
        cmd.env("Q_NEW_SESSION", "1");
        cmd.env("MOCK_QTERM_SESSION_ID", session_id);
        cmd.env("TMPDIR", tempdir.path());
        cmd.env("XDG_RUNTIME_DIR", tempdir.path());

        let child = pty_pair.slave.spawn_command(cmd)?;
        let writer = pty_pair.master.take_writer()?;

        let mut res = pty_pair.master.try_clone_reader().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0; 1024];
            while let Ok(a) = res.read(&mut buf) {
                if a == 0 {
                    break;
                }
                stdout().write_all(&buf[..a]).unwrap();
                stdout().flush().unwrap();
            }
        });

        // give time for shell to spawn
        tokio::time::sleep(Duration::from_millis(500)).await;

        Ok(Shell {
            desktop_socket,

            pty_pair,
            writer,
            child,
            tempdir,

            buffer,
            shell_context,
        })
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
