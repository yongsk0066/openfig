//! A separate IMK palette owns text-input clients. Only correlated scalar geometry crosses IPC.
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{
    Context,
    bail,
};
use serde::{
    Deserialize,
    Serialize,
};
use tokio::io::{
    AsyncBufReadExt,
    AsyncWriteExt,
    BufReader,
};
use tokio::net::{
    UnixListener,
    UnixStream,
};
use tokio::sync::watch;

use super::caret_worker::{
    AnchorRect,
    Request,
};

const MAX_FRAME: u64 = 4096;
const REPLY_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    Connected,
    Retiring,
    FocusChanged,
    Attachment { request: Request, attached: Option<bool> },
}

#[derive(Debug)]
pub struct InputMethod {
    requests: watch::Sender<Option<Request>>,
    task: tokio::task::JoinHandle<()>,
    path: PathBuf,
}

impl InputMethod {
    pub fn start(
        publish: impl Fn(Request, Option<AnchorRect>, Option<AnchorRect>, bool) + Send + Sync + 'static,
        lifecycle: impl Fn(LifecycleEvent) + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        let path = fig_util::directories::desktop_socket_path()?.with_file_name("ime.sock");
        let parent = path.parent().context("Input method socket needs a parent")?;
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        match std::fs::remove_file(&path) {
            Ok(()) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&path)?;
        let (requests, mut receiver) = watch::channel(None);
        let task = tokio::spawn(async move {
            let mut last_launch = tokio::time::Instant::now() - Duration::from_secs(5);
            loop {
                tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            // The containing directory is user-private; check the peer as well.
                            if stream.peer_cred().is_ok_and(|peer| peer.uid() == nix::unistd::Uid::current().as_raw()) {
                                if let Err(error) = serve(stream, &mut receiver, &publish, &lifecycle).await {
                                    tracing::debug!(%error, "Input method connection ended");
                                }
                                if let Some(request) = *receiver.borrow() {
                                    publish(request, None, None, false);
                                }
                            }
                        },
                        Err(error) => {
                            tracing::warn!(%error, "Input method accept failed");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        },
                    },
                    changed = receiver.changed() => if changed.is_err() { break },
                    () = tokio::time::sleep(Duration::from_secs(1)) => {},
                }
                if receiver.borrow().is_some() && last_launch.elapsed() >= Duration::from_secs(5) {
                    last_launch = tokio::time::Instant::now();
                    let helper = fig_util::app_bundle_path().join("Contents/Helpers/FigInputMethod.app");
                    if helper.is_dir() {
                        // Open our background-only helper; this never launches or activates a
                        // terminal.
                        let _ = tokio::process::Command::new("/usr/bin/open")
                            .arg("-g")
                            .arg(helper)
                            .status()
                            .await;
                    }
                }
            }
        });
        Ok(Self { requests, task, path })
    }

    pub fn request(&self, request: Request) {
        self.requests.send_replace(Some(request));
    }

    pub fn cancel(&self) {
        self.requests.send_replace(None);
    }
}

impl Drop for InputMethod {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Serialize)]
struct Query<'a> {
    v: u8,
    r#type: &'a str,
    id: &'a str,
    pid: i32,
    window: u32,
    bundle: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Message {
    #[serde(rename = "ready")]
    Ready { v: u8 },
    #[serde(rename = "retiring")]
    Retiring { v: u8 },
    #[serde(rename = "focus")]
    Focus { v: u8 },
    #[serde(rename = "rect")]
    Rect {
        v: u8,
        id: String,
        rect: Option<[f64; 4]>,
        #[serde(default)]
        frame: Option<[f64; 4]>,
        #[serde(default)]
        suppressed: bool,
        #[serde(default)]
        attachment: Option<bool>,
    },
}

struct MessageReader {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    frame: Vec<u8>,
}
impl MessageReader {
    // The partial frame belongs to the connection, not this future. select! cancellation
    // on a newer request must not discard a prefix already read from the socket.
    async fn read(&mut self) -> anyhow::Result<Message> {
        loop {
            let buffer = self.reader.fill_buf().await?;
            if buffer.is_empty() {
                bail!("Input method disconnected")
            }
            let newline = buffer.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(buffer.len(), |index| index + 1);
            if (self.frame.len() + count) as u64 > MAX_FRAME {
                bail!("Oversized input method frame")
            }
            self.frame.extend_from_slice(&buffer[..count]);
            self.reader.consume(count);
            if newline.is_some() {
                let message = serde_json::from_slice(&self.frame)?;
                self.frame.clear();
                return Ok(message);
            }
        }
    }
}

fn rectangle(values: [f64; 4]) -> Option<AnchorRect> {
    let [x, y, width, height] = values;
    (values.iter().all(|value| value.is_finite()) && width >= 0.0 && height > 0.0).then_some(AnchorRect {
        x,
        y,
        width,
        height,
    })
}

async fn serve(
    stream: UnixStream,
    requests: &mut watch::Receiver<Option<Request>>,
    publish: &(impl Fn(Request, Option<AnchorRect>, Option<AnchorRect>, bool) + Sync),
    lifecycle: &(impl Fn(LifecycleEvent) + Sync),
) -> anyhow::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = MessageReader {
        reader: BufReader::new(read),
        frame: Vec::new(),
    };
    let mut last_sent = None;
    let mut pending: Option<(String, Request)> = None;
    let mut deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
    let mut retry_at = None;
    loop {
        let latest = *requests.borrow_and_update();
        if pending.is_none() && latest != last_sent {
            last_sent = latest;
            if let Some(request) = latest {
                let id = uuid::Uuid::new_v4().to_string();
                let query = Query {
                    v: 1,
                    r#type: "query",
                    id: &id,
                    pid: request.target.pid,
                    window: request.target.window_id,
                    bundle: "com.mitchellh.ghostty",
                };
                let mut frame = serde_json::to_vec(&query)?;
                frame.push(b'\n');
                tokio::time::timeout(REPLY_TIMEOUT, write.write_all(&frame)).await??;
                pending = Some((id, request));
                deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
            }
        }
        tokio::select! {
            changed = requests.changed() => if changed.is_err() { return Ok(()) },
            message = reader.read() => match message? {
                Message::Ready { v: 1 } => lifecycle(LifecycleEvent::Connected),
                Message::Retiring { v: 1 } => lifecycle(LifecycleEvent::Retiring),
                Message::Focus { v: 1 } => lifecycle(LifecycleEvent::FocusChanged),
                Message::Rect { v: 1, id, rect, frame, suppressed, attachment } => {
                    let Some((expected, request)) = pending.take() else { bail!("Unsolicited caret reply") };
                    if id != expected { bail!("Mismatched caret reply") }
                    crate::latency::record(
                        "input-method-reply", "", None,
                        Some((tokio::time::Instant::now() - (deadline - REPLY_TIMEOUT)).as_secs_f64() * 1000.0),
                        None,
                    );
                    if *requests.borrow() == Some(request) {
                        // A detached foreground client must recover even with no new key or
                        // visible popup. Background/attached replies stop this bounded polling.
                        retry_at = (attachment == Some(false)).then(|| tokio::time::Instant::now() + REPLY_TIMEOUT);
                        lifecycle(LifecycleEvent::Attachment { request, attached: attachment });
                        publish(request, rect.and_then(rectangle), frame.and_then(rectangle), suppressed);
                    }
                },
                _ => bail!("Unsupported input method protocol version"),
            },
            () = tokio::time::sleep_until(deadline), if pending.is_some() => {
                bail!("Input method request timed out");
            },
            () = tokio::time::sleep_until(retry_at.unwrap_or(deadline)), if pending.is_none() && latest.is_some() && retry_at.is_some() => {
                retry_at = None;
                last_sent = None;
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detached_client_is_retried_without_input_and_background_stops_retrying() {
        let (desktop, helper) = UnixStream::pair().unwrap();
        let (send, mut requests) = watch::channel(Some(request(1)));
        let task = tokio::spawn(async move { serve(desktop, &mut requests, &|_, _, _, _| {}, &|_| {}).await });
        let (read, mut write) = helper.into_split();
        let mut lines = BufReader::new(read).lines();
        for attachment in ["false", "null"] {
            let query = tokio::time::timeout(Duration::from_secs(1), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let query: serde_json::Value = serde_json::from_str(&query).unwrap();
            write
                .write_all(
                    format!(
                        "{{\"v\":1,\"type\":\"rect\",\"id\":{},\"attachment\":{attachment},\"rect\":null}}\n",
                        query["id"]
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(350), lines.next_line())
                .await
                .is_err()
        );
        drop(send);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn connection_readiness_is_not_focus_and_stale_attachment_cannot_trigger_recovery() {
        let (desktop, helper) = UnixStream::pair().unwrap();
        let (send, mut requests) = watch::channel(Some(request(1)));
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            serve(desktop, &mut requests, &|_, _, _, _| {}, &move |event| {
                events.send(event).unwrap();
            })
            .await
        });
        let (read, mut write) = helper.into_split();
        let mut lines = BufReader::new(read).lines();
        let first: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        write.write_all(b"{\"v\":1,\"type\":\"ready\"}\n").await.unwrap();
        assert_eq!(received.recv().await, Some(LifecycleEvent::Connected));
        write
            .write_all(
                format!(
                    "{{\"v\":1,\"type\":\"rect\",\"id\":{},\"attachment\":false,\"rect\":null}}\n",
                    first["id"]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(
            received.recv().await,
            Some(LifecycleEvent::Attachment {
                request: request(1),
                attached: Some(false)
            })
        );
        send.send_replace(Some(request(2)));
        let second: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        send.send_replace(None);
        write.write_all(format!("{{\"v\":1,\"type\":\"rect\",\"id\":{},\"attachment\":false,\"rect\":null}}\n{{\"v\":1,\"type\":\"focus\"}}\n", second["id"]).as_bytes()).await.unwrap();
        assert_eq!(received.recv().await, Some(LifecycleEvent::FocusChanged));
        assert!(received.try_recv().is_err());
        drop(send);
        task.await.unwrap().unwrap();
    }

    #[test]
    fn accepts_insertion_lines_and_negative_screen_origins_but_not_invalid_geometry() {
        assert!(rectangle([-1200.0, -350.0, 0.0, 18.0]).is_some());
        for value in [
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, 18.0],
            [f64::NAN, 0.0, 1.0, 18.0],
            [0.0, f64::INFINITY, 1.0, 18.0],
            [0.0, 0.0, 1.0, -18.0],
        ] {
            assert!(rectangle(value).is_none());
        }
    }
    fn request(sequence: u64) -> Request {
        Request {
            target: super::super::focus_validation::FocusTarget {
                pid: 42,
                window_id: 7,
                generation: 2,
            },
            sequence,
            anchor: fig_util::terminal::CompletionAnchor::InputMethod,
        }
    }

    #[tokio::test]
    async fn fragmented_reply_survives_cancellation_of_the_read_future() {
        let (a, mut b) = UnixStream::pair().unwrap();
        let (read, _) = a.into_split();
        let mut reader = MessageReader {
            reader: BufReader::new(read),
            frame: Vec::new(),
        };
        b.write_all(br#"{"v":1,"type":"rec"#).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), reader.read())
                .await
                .is_err()
        );
        b.write_all(b"t\",\"id\":\"ok\",\"rect\":[-10,20,0,18]}\n")
            .await
            .unwrap();
        assert!(matches!(reader.read().await.unwrap(), Message::Rect { id, .. } if id == "ok"));
    }

    #[tokio::test]
    async fn bounded_frames_reject_oversize_and_partial_eof() {
        for bytes in [vec![b'x'; MAX_FRAME as usize + 1], b"{\"v\":1".to_vec()] {
            let (a, mut b) = UnixStream::pair().unwrap();
            let (read, _) = a.into_split();
            let mut reader = MessageReader {
                reader: BufReader::new(read),
                frame: Vec::new(),
            };
            b.write_all(&bytes).await.unwrap();
            b.shutdown().await.unwrap();
            assert!(reader.read().await.is_err());
        }
    }

    #[tokio::test]
    async fn coalesces_pending_requests_and_never_publishes_an_outdated_reply() {
        let (a, b) = UnixStream::pair().unwrap();
        let (send, mut recv) = watch::channel(Some(request(1)));
        let (published, mut results) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            serve(
                a,
                &mut recv,
                &move |request, rect, _frame, _suppressed| {
                    published.send((request.sequence, rect.is_some())).unwrap();
                },
                &|_| {},
            )
            .await
        });
        let (read, mut write) = b.into_split();
        let mut lines = BufReader::new(read).lines();
        let first: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        send.send_replace(Some(request(2)));
        send.send_replace(Some(request(3)));
        write
            .write_all(
                format!(
                    "{{\"v\":1,\"type\":\"rect\",\"id\":{},\"rect\":[1,2,0,18]}}\n",
                    first["id"]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let next: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_ne!(first["id"], next["id"]);
        assert!(results.try_recv().is_err());
        write
            .write_all(
                format!(
                    "{{\"v\":1,\"type\":\"rect\",\"id\":{},\"rect\":[1,2,0,18]}}\n",
                    next["id"]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(results.recv().await, Some((3, true)));
        send.send_replace(None);
        drop(send);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn focus_change_and_cancellation_do_not_deliver_the_old_coordinates() {
        let (a, b) = UnixStream::pair().unwrap();
        let (send, mut recv) = watch::channel(Some(request(1)));
        let (focus, mut events) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            serve(
                a,
                &mut recv,
                &|_, _, _, _| panic!("cancelled reply must not publish"),
                &move |event| {
                    if event == LifecycleEvent::FocusChanged {
                        focus.send(()).unwrap();
                    }
                },
            )
            .await
        });
        let (read, mut write) = b.into_split();
        let mut lines = BufReader::new(read).lines();
        let first: serde_json::Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        write.write_all(b"{\"v\":1,\"type\":\"focus\"}\n").await.unwrap();
        assert_eq!(events.recv().await, Some(()));
        send.send_replace(None);
        write
            .write_all(
                format!(
                    "{{\"v\":1,\"type\":\"rect\",\"id\":{},\"rect\":[1,2,0,18]}}\n{{\"v\":1,\"type\":\"focus\"}}\n",
                    first["id"]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        // This second event proves the cancelled reply was consumed without publishing it,
        // even when the reply and focus frame were delivered in the same socket write.
        assert_eq!(events.recv().await, Some(()));
        drop(send);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn timeout_and_wrong_nonce_end_the_connection_without_publishing_coordinates() {
        for wrong_nonce in [false, true] {
            let (a, b) = UnixStream::pair().unwrap();
            let (_send, mut recv) = watch::channel(Some(request(1)));
            let task = tokio::spawn(async move {
                serve(
                    a,
                    &mut recv,
                    &|_, _, _, _| panic!("invalid response must not publish"),
                    &|_| {},
                )
                .await
            });
            let (read, mut write) = b.into_split();
            let mut lines = BufReader::new(read).lines();
            assert!(lines.next_line().await.unwrap().is_some());
            if wrong_nonce {
                write
                    .write_all(b"{\"v\":1,\"type\":\"rect\",\"id\":\"wrong\",\"rect\":[0,0,0,18]}\n")
                    .await
                    .unwrap();
            }
            assert!(task.await.unwrap().is_err());
        }
    }
}
