use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};
use tokio::process::{
    Child,
    ChildStdin,
    Command,
};
use tokio::sync::watch;
use tokio::time::{
    Instant,
    timeout,
};

use super::{
    Envelope,
    MAX_FRAME,
    QUERY_TIMEOUT,
    Reply,
    Request,
};

struct Connection {
    child: Child,
    input: ChildStdin,
    #[cfg(not(test))]
    output: tokio::process::ChildStdout,
    #[cfg(test)]
    output: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>,
    poisoned: bool,
}

/// Owns one child. Failed/canceled exchanges are never reused; replacement waits for reaping.
/// A kernel that cannot reap a killed process degrades this lane instead of leaking children.
pub struct Process {
    executable: PathBuf,
    #[cfg(test)]
    fixture: Option<(PathBuf, String)>,
    connection: Option<Connection>,
    sequence: u64,
    retry_at: Instant,
}

impl Process {
    pub fn installed() -> io::Result<Self> {
        let executable = std::env::current_exe()?.with_file_name("fig_native_worker");
        Ok(Self::new(executable))
    }

    /// Explicit path injection for development fixtures; never accepted over IPC.
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            #[cfg(test)]
            fixture: None,
            connection: None,
            sequence: 0,
            retry_at: Instant::now(),
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.connection.as_ref()?.child.id()
    }

    pub async fn shutdown(&mut self) {
        if let Some(connection) = &mut self.connection {
            connection.poisoned = true;
            let _ = connection.child.start_kill();
            if matches!(
                timeout(Duration::from_millis(500), connection.child.wait()).await,
                Ok(Ok(_))
            ) {
                self.connection = None;
            }
        }
    }

    pub async fn call(&mut self, request: Request, deadline: Duration) -> io::Result<Reply> {
        if self.connection.as_ref().is_some_and(|c| c.poisoned) {
            self.shutdown().await;
            if self.connection.is_some() {
                return Err(io::Error::other("native child has not exited; replacement deferred"));
            }
        }
        tokio::time::sleep_until(self.retry_at).await;
        let started = std::time::Instant::now();
        let outcome = timeout(deadline, self.exchange(&request)).await.unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "native query deadline exceeded",
            ))
        });
        trace_duration(
            &format!("{}-ipc", stage(&request)),
            started.elapsed().as_secs_f64() * 1000.0,
        );
        if outcome.is_err() {
            self.shutdown().await;
            self.retry_at = Instant::now() + Duration::from_millis(100);
        }
        outcome
    }

    async fn exchange(&mut self, request: &Request) -> io::Result<Reply> {
        if self.connection.is_none() {
            let mut command = Command::new(&self.executable);
            command.args(["--stdio", "--lane", stage(request)]);
            #[cfg(test)]
            if let Some((root, fault)) = &self.fixture {
                command = Command::new(&self.executable);
                command
                    .args([
                        "--exact",
                        "native::client::tests::worker_fixture",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("FIG_TEST_WORKER_ROOT", root)
                    .env("FIG_TEST_WORKER_FAULT", fault);
            }
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            // The Rust test harness owns stdout; only fixture children use stderr as their pipe.
            #[cfg(test)]
            if self.fixture.is_some() {
                command.stdout(Stdio::null()).stderr(Stdio::piped());
            }
            let mut child = command.spawn()?;
            let input = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("missing native stdin"))?;
            #[cfg(not(test))]
            let output = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("missing native stdout"))?;
            #[cfg(test)]
            let output: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>> = if self.fixture.is_some() {
                Box::pin(
                    child
                        .stderr
                        .take()
                        .ok_or_else(|| io::Error::other("missing fixture stderr"))?,
                )
            } else {
                Box::pin(
                    child
                        .stdout
                        .take()
                        .ok_or_else(|| io::Error::other("missing native stdout"))?,
                )
            };
            self.connection = Some(Connection {
                child,
                input,
                output,
                poisoned: false,
            });
        }
        self.sequence = self.sequence.wrapping_add(1);
        let payload = serde_json::to_vec(&Envelope {
            work_ms: None,
            id: self.sequence,
            value: request,
        })?;
        if payload.len() > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "native request too large"));
        }
        let connection = self
            .connection
            .as_mut()
            .ok_or_else(|| io::Error::other("missing native child"))?;
        // Set before the first await: dropping this future cannot leave a partial stream reusable.
        connection.poisoned = true;
        connection.input.write_u32(payload.len() as u32).await?;
        connection.input.write_all(&payload).await?;
        connection.input.flush().await?;
        let size = connection.output.read_u32().await? as usize;
        if size > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "native response too large"));
        }
        let mut bytes = vec![0; size];
        connection.output.read_exact(&mut bytes).await?;
        let response: Envelope<Reply> = serde_json::from_slice(&bytes)?;
        if response.id != self.sequence || !response.value.matches(request) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "native response mismatch"));
        }
        connection.poisoned = false;
        if let Some(ms) = response.work_ms.filter(|v| v.is_finite() && *v >= 0.0) {
            trace_duration(stage(request), ms);
        }
        Ok(response.value)
    }
}

/// One active request and one replaceable pending value. Failed queries retry without new input;
/// a new target supersedes the retry. Closing the owner also cancels and reaps active work.
pub fn start_latest<T: Clone + PartialEq + Send + Sync + 'static>(
    make_request: impl Fn(T) -> Request + Send + 'static,
    publish: impl Fn(T, Reply) + Send + 'static,
) -> io::Result<watch::Sender<Option<T>>> {
    let process = Process::installed()?;
    Ok(start_latest_with(process, make_request, publish))
}

fn start_latest_with<T: Clone + PartialEq + Send + Sync + 'static>(
    mut process: Process,
    make_request: impl Fn(T) -> Request + Send + 'static,
    publish: impl Fn(T, Reply) + Send + 'static,
) -> watch::Sender<Option<T>> {
    let (sender, mut receiver) = watch::channel(None::<T>);
    tokio::spawn(async move {
        let mut retry = None;
        let mut failures = 0_u32;
        loop {
            let target = if let Some(target) = retry.take() {
                let delay = Duration::from_millis(100 * (1_u64 << failures.min(5)));
                tokio::select! {
                    result = receiver.changed() => {
                        if result.is_err() { break; }
                        receiver.borrow_and_update().clone()
                    }
                    _ = tokio::time::sleep(delay) => Some(target),
                }
            } else {
                if receiver.changed().await.is_err() {
                    break;
                }
                receiver.borrow_and_update().clone()
            };
            let Some(target) = target else { continue };
            let outcome = tokio::select! {
                result = process.call(make_request(target.clone()), QUERY_TIMEOUT) => result,
                _ = receiver.wait_for(|_| false) => break,
            };
            if *receiver.borrow() != Some(target.clone()) {
                receiver.mark_changed();
                failures = 0;
                continue;
            }
            match outcome {
                Ok(reply) => {
                    failures = 0;
                    publish(target, reply);
                },
                Err(error) => {
                    failures = failures.saturating_add(1);
                    tracing::warn!(%error, failures, "Native query failed; retrying isolated process");
                    retry = Some(target);
                },
            }
        }
        process.shutdown().await;
    });
    sender
}

fn stage(request: &Request) -> &'static str {
    match request {
        Request::Frame(_) => "window-frame-worker",
        Request::Caret(_) => "caret-query-worker",
        Request::Focus(_) => "focus-validation-worker",
        Request::TextFocus(_) => "text-focus-worker",
        Request::Activation(_) => "activation-worker",
        Request::Icon(_) => "native-icon-render",
    }
}
fn trace_duration(stage: &str, ms: f64) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var("FIG_LATENCY_TRACE").is_ok_and(|value| value == "1")) {
        return;
    }
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0;
    let sample = serde_json::json!({"stage":stage,"sessionId":"","revision":null,"unixMs":unix_ms,"milliseconds":ms,"bytes":null});
    tracing::info!(target:"fig_latency", sample = %sample, "native");
}

#[cfg(test)]
pub(super) mod tests {
    use std::io::{
        Read,
        Write,
    };

    use super::*;
    use crate::native::FocusTarget;

    pub(crate) fn fixture(fault: &str) -> (tempfile::TempDir, Process) {
        let dir = tempfile::tempdir().unwrap();
        let mut process = Process::new(std::env::current_exe().unwrap());
        process.fixture = Some((dir.path().to_owned(), fault.to_owned()));
        (dir, process)
    }

    // A separate Rust test process exercises real pipes and reaping without interpreter startup.
    #[test]
    #[ignore = "child entry point; invoked by the native fault tests"]
    #[expect(
        clippy::exit,
        reason = "fault fixture must terminate its own child without unwinding"
    )]
    fn worker_fixture() {
        let root = PathBuf::from(std::env::var_os("FIG_TEST_WORKER_ROOT").expect("fixture root"));
        let fault = std::env::var("FIG_TEST_WORKER_FAULT").expect("fixture fault");
        let mut first = !root.join("started").exists();
        let mut started = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("started"))
            .unwrap();
        writeln!(started, "{}", std::process::id()).unwrap();
        let mut input = std::io::stdin().lock();
        let mut output = std::io::stderr().lock();
        loop {
            let mut header = [0; 4];
            if input.read_exact(&mut header).is_err() {
                break;
            }
            let size = u32::from_be_bytes(header) as usize;
            assert!(size <= MAX_FRAME);
            let mut bytes = vec![0; size];
            input.read_exact(&mut bytes).unwrap();
            let data: Envelope<Request> = serde_json::from_slice(&bytes).unwrap();
            let value = match data.value {
                Request::Focus(target) => target.pid,
                _ => 1,
            };
            let mut requests = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("requests"))
                .unwrap();
            writeln!(requests, "{value}").unwrap();
            if first {
                first = false;
                match fault.as_str() {
                    "hang" => std::thread::sleep(Duration::from_secs(3600)),
                    "crash" => std::process::exit(71),
                    "partial" | "oversize" => {
                        let length = if fault == "partial" { 100_u32 } else { u32::MAX };
                        output.write_all(&length.to_be_bytes()).unwrap();
                        if fault == "partial" {
                            output.write_all(b"{").unwrap();
                        }
                        output.flush().unwrap();
                        std::thread::sleep(Duration::from_secs(3600));
                    },
                    "slow" => {
                        while !root.join("release").exists() {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    },
                    _ => {},
                }
            }
            let reply = Envelope {
                id: data.id + u64::from(fault == "wrong-id"),
                work_ms: None,
                value: if fault == "wrong-kind" {
                    Reply::Caret(None)
                } else if matches!(data.value, Request::Icon(_)) {
                    Reply::Icon(None)
                } else {
                    Reply::Focus(Some(value.try_into().unwrap()))
                },
            };
            let bytes = serde_json::to_vec(&reply).unwrap();
            output.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
            output.write_all(&bytes).unwrap();
            output.flush().unwrap();
        }
    }
    fn request(pid: i32) -> Request {
        Request::Focus(FocusTarget {
            pid,
            window_id: 2,
            generation: 3,
        })
    }
    async fn started(dir: &std::path::Path) {
        timeout(Duration::from_secs(2), async {
            while !dir.join("requests").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn hang_crash_partial_and_oversized_response_are_reaped_and_next_query_recovers() {
        for fault in ["hang", "crash", "partial", "oversize"] {
            let (dir, mut process) = fixture(fault);
            let result = process.call(request(1), Duration::from_secs(3)).await;
            assert!(result.is_err(), "{fault}");
            let pid: i32 = std::fs::read_to_string(dir.path().join("started"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert_eq!(process.pid(), None);
            // SAFETY: signal zero checks the process that this supervisor has just reaped.
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "{fault}");
            assert!(matches!(
                process.call(request(2), Duration::from_secs(2)).await.unwrap(),
                Reply::Focus(Some(2))
            ));
            process.shutdown().await;
        }
    }
    #[tokio::test]
    async fn mismatched_correlation_and_reply_type_retire_the_connection() {
        for fault in ["wrong-id", "wrong-kind"] {
            let (_dir, mut process) = fixture(fault);
            assert!(process.call(request(1), Duration::from_secs(2)).await.is_err());
            assert_eq!(process.pid(), None);
        }
    }
    #[tokio::test]
    async fn cancellation_does_not_reuse_a_partial_exchange() {
        let (_dir, mut process) = fixture("partial");
        {
            let query = process.call(request(1), Duration::from_secs(10));
            tokio::pin!(query);
            tokio::select! {
                result = &mut query => panic!("partial response completed: {result:?}"),
                _ = started(_dir.path()) => {},
            }
            assert!(timeout(Duration::from_millis(100), &mut query).await.is_err());
        }
        let pid = process.pid().unwrap();
        assert!(matches!(
            process.call(request(2), Duration::from_secs(2)).await.unwrap(),
            Reply::Focus(Some(2))
        ));
        assert_ne!(process.pid(), Some(pid));
        process.shutdown().await;
    }
    #[tokio::test]
    async fn latest_lane_restarts_after_a_hang_without_another_input() {
        let (_dir, process) = fixture("hang");
        let (done, mut results) = tokio::sync::mpsc::unbounded_channel();
        let sender = start_latest_with(process, request, move |target, _| {
            done.send(target).unwrap();
        });
        sender.send_replace(Some(7));
        assert_eq!(timeout(Duration::from_secs(4), results.recv()).await.unwrap(), Some(7));
    }
    #[tokio::test]
    async fn latest_lane_coalesces_and_never_publishes_superseded_work() {
        let (dir, process) = fixture("slow");
        let (done, mut results) = tokio::sync::mpsc::unbounded_channel();
        let sender = start_latest_with(process, request, move |target, _| {
            done.send(target).unwrap();
        });
        sender.send_replace(Some(1));
        started(dir.path()).await;
        sender.send_replace(Some(2));
        sender.send_replace(Some(3));
        std::fs::write(dir.path().join("release"), []).unwrap();
        assert_eq!(timeout(Duration::from_secs(3), results.recv()).await.unwrap(), Some(3));
        assert!(results.try_recv().is_err());
        assert_eq!(std::fs::read_to_string(dir.path().join("requests")).unwrap(), "1\n3\n");
    }
    #[tokio::test]
    async fn closing_owner_cancels_an_active_child_without_waiting_for_its_query_deadline() {
        let (dir, process) = fixture("hang");
        let sender = start_latest_with(process, request, |_, _| panic!("closed owner must not publish"));
        sender.send_replace(Some(1));
        started(dir.path()).await;
        let pid: i32 = std::fs::read_to_string(dir.path().join("started"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(sender);
        timeout(Duration::from_millis(500), async {
            // SAFETY: signal zero is a process existence check.
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}
