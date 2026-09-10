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
use fig_proto::remote::hostbound::Handshake;
use fig_proto::remote::{
    Clientbound,
    Hostbound,
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
};

use crate::MainLoopEvent;

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

/// Spawns a local unix socket for communicating with figterm on a local machine
pub async fn spawn_figterm_ipc(
    session_id: impl std::fmt::Display,
) -> Result<Receiver<(FigtermRequestMessage, Sender<FigtermResponseMessage>)>> {
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
            if let Ok((stream, _)) = socket_listener.accept().await {
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
                                    incoming_tx
                                        .clone()
                                        .send_async((message, response_tx.clone()))
                                        .await
                                        .unwrap();
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
        }
    });

    Ok(incoming_rx)
}

/// Connects to the desktop app and allows for a remote connection from remote hosts
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
                    break;
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
                    info!("Attempting handshake...");
                    if let Err(err) = writer.send_message(Hostbound {
                        packet: Some(hostbound::Packet::Handshake(Handshake {
                            id: session_id.clone(),
                        })),
                    })
                    .await
                    {
                        error!(%err, "error sending handshake");
                        continue;
                    }
                    let mut handshake_success = false;
                    info!("Awaiting handshake response...");
                    while let Some(message) = reader.recv_message::<Clientbound>().await.unwrap_or_else(|err| {
                        error!(%err, "failed receiving handshake response");
                        None
                    }) {
                        if let Some(clientbound::Packet::HandshakeResponse(response)) = message.packet {
                            handshake_success = response.success;
                            break;
                        }
                    }
                    if !handshake_success {
                        error!("failed performing handshake");
                        continue;
                    }
                    info!("Handshake succeeded");

                    // send outgoing messages
                    outgoing_rx.drain();
                    set_desktop_connected(true);
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
                                    main_loop_sender
                                        .send(MainLoopEvent::Insert {
                                            insert: Vec::new(),
                                            unlock: true,
                                            bracketed: false,
                                            execute: false,
                                        })
                                        .unwrap();
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
                }
            }
        }
    });

    Ok((outgoing_tx, incoming_rx, stop_ipc_tx))
}
