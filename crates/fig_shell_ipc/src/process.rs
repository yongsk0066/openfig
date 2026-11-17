//! Application-owned I/O budgets. Arbitrary paths and generator commands run only in disposable
//! children; a canceled caller cannot release a running slot before its process has been reaped.
mod worker;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fig_proto::fig::{
    EnvironmentVariable,
    RunProcessResponse,
};
use serde::{
    Deserialize,
    Serialize,
};
use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};
use tokio::process::Command;
use tokio::sync::{
    Semaphore,
    oneshot,
    watch,
};
use tokio::time::{
    Instant,
    timeout_at,
};
pub use worker::serve;

const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_FRAME: usize = 16 * 1024 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const FILE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Request {
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
    pub env: Vec<EnvironmentVariable>,
    pub timeout: Option<Duration>,
}
pub struct Shell {
    pub env: Vec<EnvironmentVariable>,
    pub cwd: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Run {
    executable: String,
    arguments: Vec<String>,
    working_directory: Option<String>,
    env: Vec<(String, Option<String>)>,
    shell_env: Vec<(String, String)>,
    shell_cwd: Option<String>,
}
#[derive(Serialize, Deserialize)]
enum Job {
    Run(Run),
    Directory { path: PathBuf, classify: bool },
}
#[derive(Serialize, Deserialize)]
enum Output {
    Run {
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    Directory(Vec<String>),
}

#[derive(Clone)]
struct Budget {
    admission: Arc<Semaphore>,
    running: Arc<Semaphore>,
}
impl Budget {
    fn new(admission: usize, running: usize) -> Self {
        Self {
            admission: Arc::new(Semaphore::new(admission)),
            running: Arc::new(Semaphore::new(running)),
        }
    }
}
#[derive(Clone)]
pub struct Service {
    executable: PathBuf,
    commands: Budget,
    files: Budget,
    lifetime: watch::Sender<()>,
}
impl Service {
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            commands: Budget::new(32, 4),
            files: Budget::new(8, 2),
            lifetime: watch::channel(()).0,
        }
    }

    pub fn installed() -> std::io::Result<Self> {
        Ok(Self::new(std::env::current_exe()?.with_file_name("fig_io_worker")))
    }

    pub async fn run(&self, request: Request, shell: Shell) -> Result<RunProcessResponse, String> {
        let duration = request.timeout.unwrap_or(DEFAULT_TIMEOUT).min(DEFAULT_TIMEOUT);
        let job = Job::Run(Run {
            executable: request.executable,
            arguments: request.arguments,
            working_directory: request.working_directory,
            env: request.env.into_iter().map(|e| (e.key, e.value)).collect(),
            shell_env: shell
                .env
                .into_iter()
                .filter_map(|e| e.value.map(|value| (e.key, value)))
                .collect(),
            shell_cwd: shell.cwd,
        });
        match self.call(job, &self.commands, duration).await? {
            Output::Run {
                stdout,
                stderr,
                exit_code,
            } => Ok(RunProcessResponse {
                stdout,
                stderr,
                exit_code,
            }),
            Output::Directory(_) => Err("I/O worker returned the wrong response type".into()),
        }
    }

    pub async fn directory(&self, path: PathBuf, classify: bool) -> Result<Vec<String>, String> {
        match self
            .call(Job::Directory { path, classify }, &self.files, FILE_TIMEOUT)
            .await?
        {
            Output::Directory(names) => Ok(names),
            Output::Run { .. } => Err("I/O worker returned the wrong response type".into()),
        }
    }

    async fn call(&self, job: Job, budget: &Budget, duration: Duration) -> Result<Output, String> {
        let deadline = Instant::now() + duration;
        let admission = budget
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|error| format!("Too many pending I/O requests: {error}"))?;
        let payload = serde_json::to_vec(&job).map_err(|e| e.to_string())?;
        if payload.len() > MAX_OUTPUT {
            return Err("I/O request exceeds 1 MiB".into());
        }
        let running = budget.running.clone();
        let executable = self.executable.clone();
        let mut lifetime = self.lifetime.subscribe();
        let (mut response, receiver) = oneshot::channel();
        tokio::spawn(async move {
            let _admission = admission;
            let _running = tokio::select! {
                biased;
                _ = response.closed() => return,
                _ = lifetime.changed() => return,
                result = timeout_at(deadline, running.acquire_owned()) => match result {
                    Ok(Ok(permit)) => permit,
                    _ => { let _ = response.send(Err("I/O request timed out while queued".into())); return; }
                }
            };
            if Instant::now() >= deadline {
                let _ = response.send(Err("I/O request timed out before launch".into()));
                return;
            }
            use std::os::unix::process::CommandExt;
            use std::process::Stdio;
            let mut command = Command::new(executable);
            command
                .arg("--stdio")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            command.as_std_mut().process_group(0);
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    let _ = response.send(Err(format!("I/O worker launch failed: {error}")));
                    return;
                },
            };
            let mut group = ProcessGroup(Some(nix::unistd::Pid::from_raw(
                child.id().expect("spawned child has a pid") as i32,
            )));
            let mut input = child.stdin.take().expect("piped stdin");
            let mut output = child.stdout.take().expect("piped stdout");
            let exchange = async {
                input.write_u32(payload.len() as u32).await?;
                input.write_all(&payload).await?;
                input.shutdown().await?;
                let size = output.read_u32().await? as usize;
                if size > MAX_FRAME {
                    return Err(std::io::Error::other("I/O response exceeds 16 MiB"));
                }
                let mut bytes = vec![0; size];
                output.read_exact(&mut bytes).await?;
                serde_json::from_slice::<Result<Output, String>>(&bytes).map_err(std::io::Error::other)
            };
            let result = tokio::select! {
                biased;
                _ = response.closed() => Err("I/O request cancelled".into()),
                _ = lifetime.changed() => Err("I/O owner stopped".into()),
                result = timeout_at(deadline, exchange) => match result {
                    Ok(Ok(result)) => result,
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Err("I/O request timed out".into()),
                }
            };
            // Helpers and their commands share this group. Kill descendants even after success.
            group.terminate();
            let _ = child.start_kill();
            // An unreapable process retains these permits, degrading this pool instead of leaking
            // replacement children. Caller cancellation/deadline remains independently bounded.
            loop {
                match child.wait().await {
                    Ok(_) => break,
                    Err(error) => {
                        tracing::warn!(%error, "I/O child not reaped; retaining execution slot");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    },
                }
            }
            let _ = response.send(result);
        });
        timeout_at(deadline, receiver)
            .await
            .map_err(|error| format!("I/O request timed out: {error}"))?
            .map_err(|error| format!("I/O worker stopped: {error}"))?
    }
}

struct ProcessGroup(Option<nix::unistd::Pid>);
impl ProcessGroup {
    fn terminate(&mut self) {
        if let Some(pid) = self.0.take() {
            let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    async fn until(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn cancellation_reaps_a_stalled_worker_and_queued_work_then_the_same_owner_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("worker");
        let marker = dir.path().join("started");
        let quoted = marker.to_string_lossy().replace('\'', "'\\''");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\necho $$ >> '{quoted}'\nexec /bin/sleep 30\n"),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut service = Service::new(executable.clone());
        service.files = Budget::new(2, 1);
        let service = Arc::new(service);
        let first = tokio::spawn({
            let service = service.clone();
            async move { service.directory("/".into(), true).await }
        });
        until(|| marker.exists()).await;
        let second = tokio::spawn({
            let service = service.clone();
            async move { service.directory("/".into(), true).await }
        });
        until(|| service.files.admission.available_permits() == 0).await;
        assert!(
            service
                .directory("/".into(), true)
                .await
                .unwrap_err()
                .contains("Too many")
        );
        // Abort the queued request first: it must never start merely because another slot frees.
        second.abort();
        let _ = second.await;
        first.abort();
        let _ = first.await;
        until(|| service.files.admission.available_permits() == 2).await;
        assert_eq!(service.files.running.available_permits(), 1);
        let pids = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(pids.lines().count(), 1);
        for pid in pids.lines() {
            assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid.parse().unwrap()), None).is_err());
        }
        let body = br#"{"Ok":{"Directory":["recovered"]}}"#;
        let header = (body.len() as u32)
            .to_be_bytes()
            .iter()
            .map(|byte| format!("\\{byte:03o}"))
            .collect::<String>();
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '{header}{}'\nexec /bin/sleep 30\n",
                std::str::from_utf8(body).unwrap()
            ),
        )
        .unwrap();
        assert_eq!(service.directory("/".into(), true).await.unwrap(), ["recovered"]);
        assert_eq!(service.files.running.available_permits(), 1);
    }

    #[tokio::test]
    async fn a_queued_request_spends_the_same_deadline_and_does_not_spawn() {
        let service = Service::new("must-not-be-spawned".into());
        let _occupied = service.files.running.acquire_many(2).await.unwrap();
        let start = Instant::now();
        let result = service
            .call(
                Job::Directory {
                    path: "/".into(),
                    classify: false,
                },
                &service.files,
                Duration::from_millis(20),
            )
            .await;
        assert!(result.err().unwrap().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(1));
        until(|| service.files.admission.available_permits() == 8).await;
    }
}
