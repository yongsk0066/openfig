//! Utiities for IPC with Tauri App

use std::sync::atomic::{
    AtomicBool,
    Ordering,
};
use std::time::Duration;

use anyhow::Result;
use fig_ipc::{
    BufferedReader,
    RecvMessage,
    SendMessage,
};
use fig_proto::FigProtobufEncodable;
use fig_proto::figterm::{
    FigtermRequestMessage,
    FigtermResponseMessage,
};
use fig_proto::remote::clientbound::HandshakeResponse;
use fig_proto::remote::clientbound::handshake_response::RejectReason;
use fig_proto::remote::hostbound::Handshake;
use fig_proto::remote::{
    Clientbound,
    Hostbound,
    PROTOCOL_VERSION,
    clientbound,
    hostbound,
};
use fig_util::{
    PTY_BINARY_NAME,
    directories,
};
use flume::{
    Receiver,
    Sender,
    unbounded,
};
use tokio::io::{
    AsyncWriteExt,
    ReadHalf,
    WriteHalf,
};
use tokio::join;
use tokio::sync::oneshot;
use tokio::time::{
    MissedTickBehavior,
    interval,
};
use tracing::{
    debug,
    error,
    info,
    trace,
    warn,
};

use crate::MainLoopEvent;

/// Sent to the main loop when a write to the desktop fails, so a pending insertion lock does not
/// keep the terminal frozen waiting for a reply that will never come.
const UNLOCK_INSERTION: MainLoopEvent = MainLoopEvent::Insert {
    insert: Vec::new(),
    unlock: true,
    bracketed: false,
    execute: false,
};

/// Whether a desktop app is currently attached to the outgoing channel.
///
/// The outgoing channel has to stay unbounded — it also carries request *responses*
/// (`message.rs`) and a blocking `send` from the main loop, and dropping either of those leaves the
/// desktop side waiting on a oneshot forever. But while nothing is attached, nothing drains it
/// either, so every keystroke used to pile an edit-buffer hook onto a queue that is discarded
/// wholesale by `outgoing_rx.drain()` at the next handshake. Gating the *hooks* on this flag keeps
/// the queue empty without touching the messages that must not be dropped.
static DESKTOP_CONNECTED: AtomicBool = AtomicBool::new(false);

/// True while a handshake has succeeded and the sending task is still alive.
pub fn desktop_connected() -> bool {
    DESKTOP_CONNECTED.load(Ordering::Acquire)
}

pub(crate) fn set_desktop_connected(connected: bool) {
    DESKTOP_CONNECTED.store(connected, Ordering::Release);
}

/// Connect to the desktop's remote socket.
///
/// The `MessageSource`/`MessageSink` enums this used to return had a second `ChildStdout`/
/// `ChildStdin` variant left over from the SSH multiplexer — 49 lines of hand-written
/// `AsyncRead`/`AsyncWrite` forwarding plus a `pin_project` dependency, for a variant nothing
/// ever constructed.
async fn get_forwarded_stream() -> Result<(ReadHalf<tokio::net::UnixStream>, WriteHalf<tokio::net::UnixStream>)> {
    let socket = directories::local_remote_socket_path()?;
    let stream = fig_ipc::socket_connect_timeout(&socket, Duration::from_secs(5)).await?;
    Ok(tokio::io::split(stream))
}

/// How long to wait for the desktop's `HandshakeResponse`. It answers on receipt, so running out
/// means something is wrong with the other end; the attempt is dropped and retried at the next
/// tick instead of waiting forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How a handshake with the desktop app ended.
#[derive(Debug)]
enum HandshakeOutcome {
    Accepted,
    /// The desktop was built for another `PROTOCOL_VERSION`. Retrying cannot help — this binary
    /// stays what it is until the shell is restarted — so the connection loop stops for good.
    VersionMismatch {
        desktop: u32,
    },
    /// Anything else: the socket broke, the desktop refused for another reason or did not answer
    /// in time. Retried at the next tick, as a desktop that is not running yet is.
    Failed(anyhow::Error),
}

async fn handshake<R, W>(reader: &mut BufferedReader<R>, writer: &mut W, session_id: &str) -> HandshakeOutcome
where
    R: tokio::io::AsyncRead + Unpin + Send,
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    info!("Attempting handshake...");
    let handshake = Hostbound {
        packet: Some(hostbound::Packet::Handshake(Handshake {
            id: session_id.to_owned(),
            protocol_version: PROTOCOL_VERSION,
        })),
    };
    if let Err(err) = writer.send_message(handshake).await {
        return HandshakeOutcome::Failed(anyhow::Error::new(err).context("error sending handshake"));
    }

    info!("Awaiting handshake response...");
    let response = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        loop {
            match reader.recv_message::<Clientbound>().await? {
                Some(Clientbound {
                    packet: Some(clientbound::Packet::HandshakeResponse(response)),
                }) => return Ok(response),
                // A ping can arrive before the answer.
                Some(_) => {},
                None => anyhow::bail!("the desktop closed the connection before answering"),
            }
        }
    })
    .await;

    match response {
        Ok(Ok(HandshakeResponse { success: true, .. })) => HandshakeOutcome::Accepted,
        Ok(Ok(response)) if response.reject_reason() == RejectReason::ProtocolVersionMismatch => {
            HandshakeOutcome::VersionMismatch {
                desktop: response.protocol_version,
            }
        },
        Ok(Ok(response)) => HandshakeOutcome::Failed(anyhow::anyhow!(
            "the desktop refused the handshake ({:?})",
            response.reject_reason()
        )),
        Ok(Err(err)) => HandshakeOutcome::Failed(err.context("failed receiving handshake response")),
        Err(_) => HandshakeOutcome::Failed(anyhow::anyhow!("no handshake response within {HANDSHAKE_TIMEOUT:?}")),
    }
}

/// Spawns a local unix socket for communicating with figterm on a local machine.
///
/// Returns the path the socket was bound to so the caller can unlink it on shutdown; the listener
/// task itself never terminates.
pub async fn spawn_figterm_ipc(
    session_id: impl std::fmt::Display,
) -> Result<(
    std::path::PathBuf,
    Receiver<(FigtermRequestMessage, Sender<FigtermResponseMessage>)>,
)> {
    trace!("Spawning incoming receiver");

    let (incoming_tx, incoming_rx) = unbounded();

    let socket_path = directories::figterm_socket_path(session_id)?;
    if let Some(parent) = socket_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            error!(%err, "Failed to create {PTY_BINARY_NAME} socket directory");
        }

        {
            use std::fs::Permissions;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, Permissions::from_mode(0o700))?;
        }
    }

    tokio::fs::remove_file(&socket_path).await.ok();
    let socket_listener = tokio::net::UnixListener::bind(&socket_path)?;

    tokio::spawn(async move {
        loop {
            let stream = match socket_listener.accept().await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    // `accept` fails transiently (EMFILE, ECONNABORTED). Retrying immediately
                    // spun this task at 100% CPU for as long as the condition lasted.
                    warn!(%err, "Failed to accept on the {PTY_BINARY_NAME} socket");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                },
            };

            let incoming_tx = incoming_tx.clone();

            let (read_half, mut write_half) = tokio::io::split(stream);
            let (response_tx, response_rx) = unbounded::<FigtermResponseMessage>();

            tokio::spawn(async move {
                let mut read_half = BufferedReader::new(read_half);
                let mut rx_thread = tokio::spawn(async move {
                    loop {
                        match read_half.recv_message::<FigtermRequestMessage>().await {
                            Ok(Some(message)) => {
                                // debug!("Received message: {message:?}");
                                if let Err(err) = incoming_tx.send_async((message, response_tx.clone())).await {
                                    error!(%err, "No more listeners for incoming messages");
                                    break;
                                }
                            },
                            Ok(None) => {
                                debug!("Received EOF");
                                break;
                            },
                            Err(err) => {
                                error!("Error receiving message: {err}");
                                break;
                            },
                        }
                    }
                });

                loop {
                    tokio::select! {
                        // Break once the rx_thread quits
                        _ = &mut rx_thread => break,
                        res = response_rx.recv_async() => {
                            match res {
                                Ok(response) => {
                                    match response.encode_fig_protobuf() {
                                        Ok(protobuf) => {
                                            if let Err(err) = write_half.write_all(&protobuf).await {
                                                error!(%err, "Failed to send response");
                                                break;
                                            }
                                        },
                                        Err(err) => error!(%err, "Failed to encode protobuf")
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    }
                }
            });
        }
    });

    Ok((socket_path, incoming_rx))
}

/// Connects to the desktop app and allows for a remote connection from remote hosts
///
/// Reconnects every 5 s while the desktop is unreachable. If the desktop refuses this binary's
/// `PROTOCOL_VERSION`, it stops trying and parks until `stop_ipc` fires: the returned channels
/// stay open (a closed `Clientbound` receiver would spin figterm's main loop), `desktop_connected`
/// stays false so no hook is queued, and the shell keeps working as a plain pty.
pub async fn spawn_remote_ipc(
    session_id: String,
    main_loop_sender: Sender<MainLoopEvent>,
) -> Result<(Sender<Hostbound>, Receiver<Clientbound>, oneshot::Sender<()>)> {
    let (stop_ipc_tx, mut stop_ipc_rx) = oneshot::channel::<()>();
    let (outgoing_tx, outgoing_rx) = unbounded::<Hostbound>();
    let (incoming_tx, incoming_rx) = unbounded::<Clientbound>();

    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            tokio::select! {
                _ = &mut stop_ipc_rx => {
                    return;
                }
                res = get_forwarded_stream() => {
                    let (reader, mut writer) = match res {
                        Ok((reader, writer)) => (reader, writer),
                        Err(err) => {
                            error!("failed to get forwarded stream: {err}");
                            continue;
                        },
                    };

                    let mut reader = BufferedReader::new(reader);
                    match handshake(&mut reader, &mut writer, &session_id).await {
                        HandshakeOutcome::Accepted => info!("Handshake succeeded"),
                        HandshakeOutcome::VersionMismatch { desktop } => {
                            warn!(
                                ours = PROTOCOL_VERSION,
                                desktop,
                                "The desktop app speaks another protocol version; running without autocomplete until this shell is restarted"
                            );
                            break;
                        },
                        HandshakeOutcome::Failed(err) => {
                            error!("failed performing handshake: {err:#}");
                            continue;
                        },
                    }

                    // send outgoing messages
                    outgoing_rx.drain();
                    set_desktop_connected(true);
                    // A fresh desktop knows nothing about this session, so the edit-buffer
                    // de-duplication must not suppress the first hook after a reconnect.
                    crate::forget_last_sent_edit_buffer();
                    let outgoing_rx = outgoing_rx.clone();
                    let main_loop_sender = main_loop_sender.clone();
                    let outgoing_task = tokio::spawn(async move {
                        while let Ok(message) = outgoing_rx.recv_async().await {
                            trace!(?message, "Sending remote message");
                            // `send_message` already flushes (fig_ipc/src/send_message.rs:39).
                            match writer.send_message(message).await {
                                Ok(()) => {},
                                Err(err) => {
                                    error!(%err, "Failed to send message");
                                    if let Err(err) = main_loop_sender.send(UNLOCK_INSERTION) {
                                        error!(%err, "Failed to unlock insertion");
                                    }
                                    let _ = writer.shutdown().await;
                                    break;
                                }
                            }
                        }
                        debug!("outgoing_task exited");
                    });

                    // receive incoming messages
                    let incoming_tx = incoming_tx.clone();
                    let incoming_task = tokio::spawn(async move {
                        while let Some(message) = reader.recv_message().await.unwrap_or_else(|err| {
                            error!("failed receiving message from host: {err}");
                            None
                        }) {
                            trace!(?message, "Received remote message");
                            if let Err(err) = incoming_tx.send(message) {
                                error!("no more listeners for incoming messages: {err}");
                                break;
                            }
                        }
                        debug!("incoming_task exited");
                    });

                    let _ = join!(outgoing_task, incoming_task);
                    set_desktop_connected(false);
                    crate::forget_last_sent_edit_buffer();
                }
            }
        }

        // Only reached through a version mismatch; a stop returns above. The task owns the far
        // ends of the returned channels, so parking here until figterm shuts down keeps them open.
        let _ = stop_ipc_rx.await;
    });

    Ok((outgoing_tx, incoming_rx, stop_ipc_tx))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs [`handshake`] against a peer that answers with `answers`, in order, and returns the
    /// outcome together with the protocol version the handshake carried.
    async fn handshake_against(answers: Vec<clientbound::Packet>) -> (HandshakeOutcome, u32) {
        let (pty, desktop) = tokio::net::UnixStream::pair().unwrap();

        let desktop = tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(desktop);
            let mut reader = BufferedReader::new(reader);
            let Some(Hostbound {
                packet: Some(hostbound::Packet::Handshake(handshake)),
            }) = reader.recv_message::<Hostbound>().await.unwrap()
            else {
                panic!("the first packet was not a handshake");
            };
            for packet in answers {
                writer.send_message(Clientbound { packet: Some(packet) }).await.unwrap();
            }
            handshake.protocol_version
        });

        let (reader, mut writer) = tokio::io::split(pty);
        let outcome = handshake(&mut BufferedReader::new(reader), &mut writer, "1234").await;
        (outcome, desktop.await.unwrap())
    }

    fn response(success: bool, reject_reason: RejectReason, protocol_version: u32) -> clientbound::Packet {
        clientbound::Packet::HandshakeResponse(HandshakeResponse {
            success,
            reject_reason: reject_reason.into(),
            protocol_version,
        })
    }

    #[tokio::test]
    async fn handshake_sends_the_protocol_version_and_skips_pings() {
        let (outcome, sent) = handshake_against(vec![
            clientbound::Packet::Ping(()),
            response(true, RejectReason::Unspecified, PROTOCOL_VERSION),
        ])
        .await;
        assert_eq!(sent, PROTOCOL_VERSION);
        assert!(matches!(outcome, HandshakeOutcome::Accepted), "{outcome:?}");
    }

    #[tokio::test]
    async fn version_refusal_is_told_apart_from_other_failures() {
        let (outcome, _) = handshake_against(vec![response(
            false,
            RejectReason::ProtocolVersionMismatch,
            PROTOCOL_VERSION + 1,
        )])
        .await;
        assert!(
            matches!(outcome, HandshakeOutcome::VersionMismatch { desktop } if desktop == PROTOCOL_VERSION + 1),
            "{outcome:?}"
        );

        // A refusal without the reason (a desktop that predates it, or a second handshake on the
        // same connection) stays retryable.
        let (outcome, _) = handshake_against(vec![response(false, RejectReason::Unspecified, 0)]).await;
        assert!(matches!(outcome, HandshakeOutcome::Failed(_)), "{outcome:?}");

        // So does a desktop that hangs up without answering.
        let (outcome, _) = handshake_against(vec![]).await;
        assert!(matches!(outcome, HandshakeOutcome::Failed(_)), "{outcome:?}");
    }
}
