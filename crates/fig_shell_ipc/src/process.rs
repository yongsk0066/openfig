//! `RunProcess` for a hook session.
//!
//! Generator commands have to see the shell's environment and working directory. A shell hook has
//! no process of its own to run them in, so the desktop runs them — with the environment and
//! directory the shell last reported.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use fig_proto::fig::{
    EnvironmentVariable,
    RunProcessResponse,
};
use fig_util::env_var::PROCESS_LAUNCHED_BY_FIG;
use tokio::process::Command;
use tokio::sync::Semaphore;

static ADMISSION: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(32));
static RUNNING: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(4));
const MAX_OUTPUT: usize = 1024 * 1024;

struct ProcessGroup(i32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // Each child starts its own group; cancel shells and their descendants together.
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(self.0), nix::sys::signal::Signal::SIGKILL);
    }
}

async fn read_output(reader: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    reader
        .take((MAX_OUTPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|err| err.to_string())?;
    if bytes.len() > MAX_OUTPUT {
        return Err("Generator output exceeds 1 MiB per stream".into());
    }
    Ok(bytes)
}

/// The timeout when the request carries none.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Request {
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
    pub env: Vec<EnvironmentVariable>,
    pub timeout: Option<Duration>,
}

/// The shell's side of it, as last reported over the hook.
pub struct Shell {
    pub env: Vec<EnvironmentVariable>,
    pub cwd: Option<String>,
}

fn directory(path: Option<&str>) -> Option<PathBuf> {
    let path = PathBuf::from(path?).canonicalize().ok()?;
    path.is_dir().then_some(path)
}

/// The command's output, or why there is none.
pub async fn run(request: Request, shell: Shell) -> Result<RunProcessResponse, String> {
    let cwd = directory(request.working_directory.as_deref())
        .or_else(|| directory(shell.cwd.as_deref()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"));

    // A relative executable is looked up on the `PATH` set here, not the desktop's.
    let mut cmd = Command::new(&request.executable);
    cmd.current_dir(cwd).args(&request.arguments).kill_on_drop(true);

    let shell_env: Vec<_> = shell
        .env
        .into_iter()
        .filter_map(|EnvironmentVariable { key, value }| value.map(|value| (key, value)))
        .collect();
    if !shell_env.is_empty() {
        cmd.env_clear().envs(shell_env);
    }
    for key in ["LS_COLORS", "CLICOLOR_FORCE", "CLICOLOR", "COLORTERM"] {
        cmd.env_remove(key);
    }
    cmd.envs([
        (PROCESS_LAUNCHED_BY_FIG, "1"),
        ("HISTFILE", ""),
        ("HISTCONTROL", "ignoreboth"),
        ("TERM", "xterm-256color"),
        ("NO_COLOR", "1"),
    ]);
    for EnvironmentVariable { key, value } in request.env {
        match value {
            Some(value) => cmd.env(key, value),
            None => cmd.env_remove(key),
        };
    }

    // Admission bounds waiting work as well as running children. Waiting counts against the
    // deadline, and dropping this future cancels both queued and running work.
    let _admission = ADMISSION
        .try_acquire()
        .map_err(|_error| "Too many pending generator processes".to_owned())?;
    let timeout = request.timeout.unwrap_or(DEFAULT_TIMEOUT);
    tokio::time::timeout(timeout, async {
        let _slot = RUNNING.acquire().await.map_err(|err| err.to_string())?;
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut().process_group(0);
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|err| format!("failed running executable ({}): {err}", request.executable))?;
        let _group = ProcessGroup(child.id().expect("spawned child has a pid") as i32);
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (stdout, stderr, status) = tokio::try_join!(read_output(stdout), read_output(stderr), async {
            child.wait().await.map_err(|err| err.to_string())
        })?;
        Ok(RunProcessResponse {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_code: status.code().unwrap_or(1),
        })
    })
    .await
    .map_err(|_error| {
        format!(
            "timed out after {}ms running executable ({})",
            timeout.as_millis(),
            request.executable
        )
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(key: &str, value: &str) -> EnvironmentVariable {
        EnvironmentVariable {
            key: key.into(),
            value: Some(value.into()),
        }
    }

    fn stdout(response: Result<RunProcessResponse, String>) -> String {
        response.unwrap_or_else(|error| panic!("{error}")).stdout
    }

    #[tokio::test]
    async fn runs_in_the_shells_directory_with_the_shells_environment() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let response = run(
            Request {
                executable: "sh".into(),
                arguments: vec![
                    "-c".into(),
                    "pwd; echo \"$FROM_SHELL $FROM_REQUEST ${HOME-unset}\"".into(),
                ],
                working_directory: None,
                env: vec![env("FROM_REQUEST", "request")],
                timeout: None,
            },
            Shell {
                env: vec![env("PATH", "/usr/bin:/bin"), env("FROM_SHELL", "shell")],
                cwd: Some(cwd.to_string_lossy().into_owned()),
            },
        )
        .await;
        // Only the shell's environment: the desktop's `HOME` is not inherited.
        assert_eq!(stdout(response), format!("{}\nshell request unset\n", cwd.display()));
    }

    #[tokio::test]
    async fn the_request_directory_wins_and_a_missing_one_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let pwd = |working_directory: Option<String>, shell_cwd: Option<String>| {
            run(
                Request {
                    executable: "/bin/pwd".into(),
                    arguments: vec![],
                    working_directory,
                    env: vec![],
                    timeout: None,
                },
                Shell {
                    env: vec![],
                    cwd: shell_cwd,
                },
            )
        };
        assert_eq!(
            stdout(pwd(Some("/".into()), Some(cwd.to_string_lossy().into())).await),
            "/\n"
        );
        assert_eq!(
            stdout(pwd(Some("/does/not/exist".into()), Some(cwd.to_string_lossy().into())).await),
            format!("{}\n", cwd.display())
        );
    }

    #[tokio::test]
    async fn failures_are_errors() {
        let missing = run(
            Request {
                executable: "/does/not/exist".into(),
                arguments: vec![],
                working_directory: None,
                env: vec![],
                timeout: None,
            },
            Shell { env: vec![], cwd: None },
        )
        .await;
        assert!(missing.is_err(), "{missing:?}");

        let slow = run(
            Request {
                executable: "/bin/sleep".into(),
                arguments: vec!["5".into()],
                working_directory: None,
                env: vec![],
                timeout: Some(Duration::from_millis(50)),
            },
            Shell { env: vec![], cwd: None },
        )
        .await;
        assert!(
            matches!(&slow, Err(message) if message.contains("timed out")),
            "{slow:?}"
        );
    }
    #[tokio::test]
    async fn dropping_work_kills_shell_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("leaked");
        let ready = dir.path().join("ready");
        let work = tokio::spawn(run(
            Request {
                executable: "/bin/sh".into(),
                arguments: vec![
                    "-c".into(),
                    "(sleep 0.5; touch \"$1\") & touch \"$2\"; wait".into(),
                    "test".into(),
                    marker.to_string_lossy().into(),
                    ready.to_string_lossy().into(),
                ],
                working_directory: None,
                env: vec![],
                timeout: Some(Duration::from_secs(5)),
            },
            Shell { env: vec![], cwd: None },
        ));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        work.abort();
        assert!(work.await.unwrap_err().is_cancelled());
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(!marker.exists(), "a cancelled shell left its child running");
    }

    #[tokio::test]
    async fn oversized_output_is_rejected_without_waiting_for_the_deadline() {
        let result = run(
            Request {
                executable: "/usr/bin/yes".into(),
                arguments: vec![],
                working_directory: None,
                env: vec![],
                timeout: Some(Duration::from_secs(5)),
            },
            Shell { env: vec![], cwd: None },
        )
        .await;
        assert!(result.unwrap_err().contains("exceeds 1 MiB"));
    }
}
