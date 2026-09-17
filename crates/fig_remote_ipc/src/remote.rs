use std::collections::{
    BTreeSet,
    HashMap,
};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{
    Context,
    Result,
};
use fig_ipc::{
    BufferedReader,
    RecvMessage,
    SendMessage,
};
use fig_proto::figterm::{
    InsertTextRequest,
    InterceptRequest,
    intercept_request,
};
use fig_proto::local::ShellContext;
use fig_proto::remote::clientbound::handshake_response::RejectReason;
use fig_proto::remote::clientbound::request::Request;
use fig_proto::remote::clientbound::{
    self,
    HandshakeResponse,
};
use fig_proto::remote::{
    Clientbound,
    Hostbound,
    PROTOCOL_VERSION,
    RunProcessRequest,
    hostbound,
};
use fig_util::PTY_BINARY_NAME;
use parking_lot::Mutex;
use tokio::net::{
    UnixListener,
    UnixStream,
};
use tokio::select;
use tokio::time::{
    Duration,
    MissedTickBehavior,
};
use tracing::{
    debug,
    error,
    info,
    trace,
    warn,
};
use uuid::Uuid;

use crate::RemoteHookHandler;
use crate::figterm::{
    EditBuffer,
    FigtermCommand,
    FigtermSession,
    FigtermState,
    InterceptMode,
};

/// Peer protocol versions a refusal has already been logged for.
///
/// Refusing is expected after a rebuild: every terminal tab opened before it still runs the old
/// `<shell> (figterm)` copy. A pty binary that predates the version check retries a refused
/// handshake every 5 s for as long as its tab stays open, so the warning is written once per
/// version and the repeats go to `debug`.
static REFUSED_VERSIONS: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());

fn handshake_response(success: bool, reject_reason: RejectReason) -> clientbound::Packet {
    clientbound::Packet::HandshakeResponse(HandshakeResponse {
        success,
        reject_reason: reject_reason.into(),
        protocol_version: PROTOCOL_VERSION,
    })
}

pub async fn start_remote_ipc(
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

    let listener = UnixListener::bind(socket_path)?;

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(handle_remote_ipc(stream, figterm_state.clone(), hook.clone()));
    }

    Ok(())
}

async fn handle_remote_ipc(
    stream: UnixStream,
    figterm_state: Arc<FigtermState>,
    mut hook: impl RemoteHookHandler + Send,
) {
    let (reader, writer) = tokio::io::split(stream);
    let (clientbound_tx, clientbound_rx) = flume::unbounded();

    let (on_close_tx, mut on_close_rx) = tokio::sync::broadcast::channel(1);

    let outgoing_task = tokio::spawn(handle_outgoing(
        writer,
        clientbound_rx,
        on_close_tx.clone(),
        on_close_tx.subscribe(),
    ));

    let ping_task = tokio::spawn(send_pings(clientbound_tx.clone(), on_close_tx.subscribe()));

    let mut initialized = false;
    let session_id = Uuid::new_v4();

    let mut reader = BufferedReader::new(reader);
    loop {
        tokio::select! {
            _ = on_close_rx.recv() => {
                debug!("Connection closed");
                break;
            }
            message = reader.recv_message::<Hostbound>() => match message {
                Ok(Some(message)) => {
                    trace!(?message, "Received remote message");
                    if let Some(response) = match message.packet {
                        Some(hostbound::Packet::Handshake(handshake)) => {
                            if handshake.protocol_version != PROTOCOL_VERSION {
                                // A pty binary from another build. Nothing it sends after this can
                                // be trusted to mean what this build reads, so the session is
                                // never created; the pty binary keeps the shell running without
                                // hooks.
                                if REFUSED_VERSIONS.lock().insert(handshake.protocol_version) {
                                    warn!(
                                        peer = handshake.protocol_version,
                                        ours = PROTOCOL_VERSION,
                                        "Refusing a {PTY_BINARY_NAME} built for another protocol version; its shell runs without autocomplete until it is restarted"
                                    );
                                } else {
                                    debug!(
                                        peer = handshake.protocol_version,
                                        ours = PROTOCOL_VERSION,
                                        "Refusing a {PTY_BINARY_NAME} built for another protocol version"
                                    );
                                }
                                Some(handshake_response(false, RejectReason::ProtocolVersionMismatch))
                            } else if initialized {
                                // maybe they missed our response, but they should've been listening harder
                                Some(handshake_response(false, RejectReason::Unspecified))
                            } else {
                                // The session id is minted here, per connection
                                // (`Uuid::new_v4()` above), so `with_update` could never find an
                                // existing session to compare a secret against: the secret branch
                                // was unreachable and the secret itself never verified. It is gone
                                // from the proto.
                                initialized = true;
                                let (command_tx, command_rx) = flume::unbounded();
                                tokio::spawn(handle_commands(command_rx, figterm_state.clone(), session_id));
                                debug!("Client auth for {} accepted", handshake.id);
                                figterm_state.insert(FigtermSession {
                                    id: session_id,
                                    sender: command_tx,
                                    writer: Some(clientbound_tx.clone()),
                                    edit_buffer: EditBuffer {
                                        text: "".to_string(),
                                        cursor: 0,
                                    },
                                    context: None,
                                    terminal_cursor_coordinates: None,
                                    response_map: HashMap::new(),
                                    nonce_counter: 0,
                                    intercept: InterceptMode::Unlocked,
                                    intercept_global: InterceptMode::Unlocked
                                });
                                Some(handshake_response(true, RejectReason::Unspecified))
                            }
                        },
                        Some(hostbound::Packet::Request(hostbound::Request { request: Some(request), .. })) => {
                            if matches!(
                                request,
                                hostbound::request::Request::EditBuffer(_)
                                | hostbound::request::Request::Prompt(_)
                                | hostbound::request::Request::PreExec(_)
                                | hostbound::request::Request::InterceptedKey(_)
                            ) && !initialized {
                                debug!("Client tried to send remote hook without auth");
                                Some(handshake_response(false, RejectReason::Unspecified))
                            } else {
                                /*
                                    WARNING, when adding new remote requests you must sanitize the context,
                                    otherwise the client can forge a message from another session
                                */
                                let res = match request {
                                    hostbound::request::Request::EditBuffer(mut edit_buffer) => {
                                        sanitize_fn(&mut edit_buffer.context, session_id);
                                        if let Some(shell_context) = &edit_buffer.context {
                                            hook.shell_context(shell_context, session_id).await;
                                        }
                                        hook.edit_buffer(
                                            &edit_buffer,
                                            session_id,
                                            &figterm_state,
                                        )
                                        .await
                                    },
                                    hostbound::request::Request::Prompt(mut prompt) => {
                                        sanitize_fn(&mut prompt.context, session_id);
                                        if let Some(shell_context) = &prompt.context {
                                            hook.shell_context(shell_context, session_id).await;
                                        }
                                        hook.prompt(&prompt, session_id, &figterm_state).await
                                    },
                                    hostbound::request::Request::PreExec(mut pre_exec) => {
                                        sanitize_fn(&mut pre_exec.context, session_id);
                                        if let Some(shell_context) = &pre_exec.context {
                                            hook.shell_context(shell_context, session_id).await;
                                        }
                                        hook.pre_exec(&pre_exec, session_id, &figterm_state).await
                                    },
                                    hostbound::request::Request::PostExec(mut post_exec) => {
                                        sanitize_fn(&mut post_exec.context, session_id);
                                        if let Some(shell_context) = &post_exec.context {
                                            hook.shell_context(shell_context, session_id).await;
                                        }
                                        hook.post_exec(&post_exec, session_id, &figterm_state).await
                                    },
                                    hostbound::request::Request::InterceptedKey(mut intercepted_key) => {
                                        sanitize_fn(&mut intercepted_key.context, session_id);
                                        if let Some(shell_context) = &intercepted_key.context {
                                            hook.shell_context(shell_context, session_id).await;
                                        }
                                        hook.intercepted_key(intercepted_key, session_id).await
                                    },
                                } ;

                                if let Err(err) = res {
                                    error!(%err, "Failed processing hook");
                                }
                                None
                            }
                        },
                        Some(hostbound::Packet::Response(hostbound::Response {
                            nonce,
                            response: Some(response),
                        })) => {
                            if initialized
                                && let Some(nonce) = nonce
                                && let Some(channel) = figterm_state
                                    .with(&session_id, |session| session.response_map.remove(&nonce))
                                    .flatten()
                            {
                                let _ = channel.send(response);
                            }
                            None
                        },
                        Some(hostbound::Packet::Pong(())) => {
                            trace!(?session_id, "Received pong");
                            None
                        },
                        Some(hostbound::Packet::Request(hostbound::Request { request: None, .. })
                            | hostbound::Packet::Response(hostbound::Response { response: None, .. }))
                            | None => {
                            warn!(?message.packet, "Received unknown remote packet");
                            None
                        }
                    } {
                        let _ = clientbound_tx.send(Clientbound { packet: Some(response) });
                    }
                }
                Ok(None) => {
                    debug!("{PTY_BINARY_NAME} connection closed");
                    break;
                }
                Err(err) => {
                    if !err.is_disconnect() {
                        warn!(%err, "Failed receiving remote message");
                    }
                    break;
                }
            }
        }
    }

    let _ = on_close_tx.send(());
    drop(clientbound_tx);

    figterm_state.remove_id(&session_id);

    if let Err(err) = ping_task.await {
        error!(%err, "remote ping task join error");
    }

    if let Err(err) = outgoing_task.await {
        error!(%err, "remote outgoing task join error");
    }

    info!("Disconnect from {session_id:?}");
}

async fn handle_outgoing(
    mut writer: tokio::io::WriteHalf<UnixStream>,
    outgoing: flume::Receiver<Clientbound>,
    on_close_tx: tokio::sync::broadcast::Sender<()>,
    mut on_close_rx: tokio::sync::broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = on_close_rx.recv() => {
                debug!("remote outgoing task exiting");
                break;
            },
            message = outgoing.recv_async() => {
                if let Ok(message) = message {
                    trace!(?message, "Sending remote message");
                    if let Err(err) = writer.send_message(message).await {
                        // The socket is unusable; tear the whole connection down rather than
                        // leaving the read loop waiting for a peer that can no longer be
                        // answered.
                        error!(%err, "remote outgoing task send error, closing connection");
                        let _ = on_close_tx.send(());
                        return;
                    }
                } else {
                    debug!("remote outgoing task exiting");
                    break;
                }
            }
        }
    }
}

async fn handle_commands(
    incoming: flume::Receiver<FigtermCommand>,
    figterm_state: Arc<FigtermState>,
    session_id: Uuid,
) -> Option<()> {
    while let Ok(command) = incoming.recv_async().await {
        let (request, nonce_channel) = match command {
            FigtermCommand::InterceptFigJs {
                intercept_keystrokes,
                intercept_global_keystrokes,
                actions,
                override_actions,
            } => (
                Request::Intercept(InterceptRequest {
                    intercept_command: Some(intercept_request::InterceptCommand::SetFigjsIntercepts(
                        intercept_request::SetFigjsIntercepts {
                            intercept_bound_keystrokes: intercept_keystrokes,
                            intercept_global_keystrokes,
                            actions,
                            override_actions,
                        },
                    )),
                }),
                None,
            ),
            FigtermCommand::InterceptFigJSVisible { visible } => (
                Request::Intercept(InterceptRequest {
                    intercept_command: Some(intercept_request::InterceptCommand::SetFigjsVisible(
                        intercept_request::SetFigjsVisible { visible },
                    )),
                }),
                None,
            ),
            FigtermCommand::InsertText {
                insertion,
                deletion,
                offset,
                immediate,
                insertion_buffer,
                insert_during_command,
            } => (
                Request::InsertText(InsertTextRequest {
                    insertion,
                    deletion: deletion.map(|x| x as u64),
                    offset,
                    immediate,
                    insertion_buffer,
                    insert_during_command,
                }),
                None,
            ),
            FigtermCommand::RunProcess {
                channel,
                executable,
                arguments,
                working_directory,
                env,
                timeout,
            } => (
                Request::RunProcess(RunProcessRequest {
                    executable,
                    arguments,
                    working_directory,
                    env,
                    timeout: timeout.map(Into::into),
                }),
                Some(channel),
            ),
        };

        let nonce = if let Some(channel) = nonce_channel {
            Some(figterm_state.with(&session_id, |session| {
                let nonce = session.nonce_counter;
                session.nonce_counter += 1;
                session.response_map.insert(nonce, channel);
                nonce
            })?)
        } else {
            None
        };

        figterm_state.with(&session_id, |session| {
            if let Some(writer) = &session.writer {
                let _ = writer.try_send(Clientbound {
                    packet: Some(clientbound::Packet::Request(clientbound::Request {
                        request: Some(request),
                        nonce,
                    })),
                });
            }
        })?;
    }

    None
}

async fn send_pings(outgoing: flume::Sender<Clientbound>, mut on_close_rx: tokio::sync::broadcast::Receiver<()>) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        select! {
            _ = interval.tick() => {
                let _ = outgoing.try_send(Clientbound {
                    packet: Some(clientbound::Packet::Ping(())),
                });
            }
            _ = on_close_rx.recv() => break
        }
    }
}

// This has to be used to sanitize as a hook can contain an invalid session_id and it must
// be sanitized before being sent to any consumers
fn sanitize_fn(context: &mut Option<ShellContext>, session_id: Uuid) {
    if let Some(context) = context {
        context.session_id = Some(session_id.to_string());
    }
}

#[cfg(test)]
mod tests {
    use fig_proto::local::{
        EditBufferHook,
        InterceptedKeyHook,
        PostExecHook,
        PreExecHook,
        PromptHook,
    };

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

    /// Sends one handshake claiming `protocol_version` to a fresh connection and returns the answer
    /// and whether the desktop side created a session for it.
    async fn handshake(protocol_version: u32) -> (HandshakeResponse, bool) {
        let (desktop, pty) = UnixStream::pair().unwrap();
        let state = Arc::new(FigtermState::new());
        tokio::spawn(handle_remote_ipc(desktop, state.clone(), NoHooks));

        let (reader, mut writer) = tokio::io::split(pty);
        writer
            .send_message(Hostbound {
                packet: Some(hostbound::Packet::Handshake(hostbound::Handshake {
                    id: "test".into(),
                    protocol_version,
                })),
            })
            .await
            .unwrap();

        let mut reader = BufferedReader::new(reader);
        let response = loop {
            let message = tokio::time::timeout(Duration::from_secs(5), reader.recv_message::<Clientbound>())
                .await
                .expect("no handshake response within 5 s")
                .unwrap()
                .expect("connection closed before the handshake response");
            if let Some(clientbound::Packet::HandshakeResponse(response)) = message.packet {
                break response;
            }
        };
        (response, state.most_recent().is_some())
    }

    #[tokio::test]
    async fn handshake_with_the_same_protocol_version_is_accepted() {
        let (response, session) = handshake(PROTOCOL_VERSION).await;
        assert!(response.success);
        assert_eq!(response.reject_reason(), RejectReason::Unspecified);
        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
        assert!(session);
    }

    #[tokio::test]
    async fn handshake_with_another_protocol_version_is_refused_without_a_session() {
        // 0 is what a pty binary that predates the field sends.
        for peer in [0, PROTOCOL_VERSION + 1] {
            let (response, session) = handshake(peer).await;
            assert!(!response.success, "peer version {peer} was accepted");
            assert_eq!(response.reject_reason(), RejectReason::ProtocolVersionMismatch);
            assert_eq!(response.protocol_version, PROTOCOL_VERSION);
            assert!(!session, "peer version {peer} got a session");
        }
    }
}
