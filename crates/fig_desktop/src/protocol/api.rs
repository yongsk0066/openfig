use std::borrow::Cow;
use std::sync::Arc;

use bytes::Bytes;
use fig_desktop_api::handler::ClientOriginatedSubMessage;
use fig_proto::fig::ClientOriginatedMessage;
use fig_proto::prost::Message;
use wry::http::header::CONTENT_TYPE;
use wry::http::{
    HeaderValue,
    Method,
    Request,
    Response,
    StatusCode,
};

use crate::application::ApplicationServices;
use crate::webview::WindowId;

static APPLICATION_FIG_API: HeaderValue = HeaderValue::from_static("application/fig-api");

pub async fn handle(
    state: Arc<ApplicationServices>,
    request: Request<Vec<u8>>,
    window_id: WindowId,
) -> anyhow::Result<Response<Cow<'static, [u8]>>> {
    if request.method() != Method::POST {
        let body = format!("Unsupported method: {}", request.method());
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Cow::Owned(body.into()))?);
    }

    let unsupported_media_body = match request.headers().get(CONTENT_TYPE) {
        Some(val) if val == APPLICATION_FIG_API => None,
        Some(val) => Some(format!("Unsupported content type: {val:?}")),
        None => Some("Missing content type".into()),
    };

    if let Some(body) = unsupported_media_body {
        return Ok(Response::builder()
            .status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
            .body(Cow::Owned(body.into()))?);
    }

    let epoch = if window_id == crate::webview::AUTOCOMPLETE_ID {
        let Some(epoch) = request
            .headers()
            .get("x-fig-view-epoch")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
        else {
            return Ok(Response::builder()
                .status(StatusCode::CONFLICT)
                .body(Cow::Borrowed(b"Missing page epoch".as_slice()))?);
        };
        Some(epoch)
    } else {
        None
    };
    let client_message = ClientOriginatedMessage::decode(Bytes::from(request.into_body()))?;
    let operation = async {
        let _sequence = match client_message.submessage {
            Some(ClientOriginatedSubMessage::PositionWindowRequest(_)) => Some(state.position_window.lock().await),
            _ => None,
        };
        let ctx = fig_os_shim::Context::new();
        let scoped;
        let proxy: &dyn crate::event::EventSender = if let Some(epoch) = epoch {
            scoped = crate::event::PageEventSender {
                epoch,
                inner: state.proxy.as_ref(),
            };
            &scoped
        } else {
            state.proxy.as_ref()
        };
        fig_desktop_api::handler::api_request(client_message, ctx.env(), async |message_id, request| {
            crate::request::handle(request, message_id, window_id, &state, &ctx, proxy).await
        })
        .await
    };
    let server_message = if let Some(epoch) = epoch {
        match state.page_epoch.run(epoch, operation).await {
            Ok(result) => result?,
            Err(()) => {
                return Ok(Response::builder()
                    .status(StatusCode::CONFLICT)
                    .body(Cow::Borrowed(b"Expired page epoch".as_slice()))?);
            },
        }
    } else {
        operation.await?
    };

    let body = server_message.encode_to_vec().into();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, &APPLICATION_FIG_API)
        .body(body)?)
}

#[cfg(test)]
mod tests {
    use fig_desktop_api::handler::ClientOriginatedSubMessage;
    use fig_proto::fig::server_originated_message::Submessage;
    use fig_proto::fig::{
        ClientOriginatedMessage,
        PingRequest,
    };
    use fig_shell_ipc::session::ShellSessions;

    use super::*;
    use crate::AUTOCOMPLETE_ID;
    use crate::event::{
        Event,
        EventSender,
    };
    use crate::webview::notification::WebviewNotificationsState;

    /// tao will not create an `EventLoop` off the process main thread, and libtest runs every
    /// test on a spawned one. Nothing on the paths exercised here sends an event.
    struct NoEventLoop;

    impl EventSender for NoEventLoop {
        fn try_send_event(&self, event: Event) -> Option<Event> {
            Some(event)
        }
    }

    fn api_state() -> Arc<ApplicationServices> {
        Arc::new(ApplicationServices {
            page_epoch: Default::default(),
            processes: Default::default(),
            native_icons: fig_desktop::native::Icons::new().ok(),
            shell_sessions: Arc::new(ShellSessions::default()),
            notifications_state: Arc::new(WebviewNotificationsState::default()),
            proxy: Box::new(NoEventLoop),
            position_window: Default::default(),
        })
    }

    #[tokio::test]
    async fn stale_page_is_rejected_before_dispatch() {
        let state = api_state();
        state.page_epoch.renew();
        let request = Request::builder()
            .method(Method::POST)
            .header(CONTENT_TYPE, APPLICATION_FIG_API.clone())
            .header("x-fig-view-epoch", "1")
            .body(ping_body())
            .unwrap();
        assert_eq!(
            handle(state, request, AUTOCOMPLETE_ID).await.unwrap().status(),
            StatusCode::CONFLICT
        );
    }

    fn ping_body() -> Vec<u8> {
        ClientOriginatedMessage {
            id: Some(7),
            submessage: Some(ClientOriginatedSubMessage::PingRequest(PingRequest {})),
        }
        .encode_to_vec()
    }

    #[tokio::test]
    async fn test_handle_errors() {
        let state = api_state();
        let id = AUTOCOMPLETE_ID;
        let body = ping_body();

        let request = Request::builder().method(Method::GET).body(body.clone()).unwrap();
        let response = handle(state.clone(), request, id.clone()).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        let request = Request::builder()
            .method(Method::POST)
            .header(CONTENT_TYPE, "text/plain")
            .body(body)
            .unwrap();
        let response = handle(state, request, id).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    /// The success path, which used to be untestable: the handler read five process globals
    /// that only `WebviewManager::new` ever set.
    #[tokio::test]
    async fn a_well_formed_request_is_answered() {
        let request = Request::builder()
            .method(Method::POST)
            .header("x-fig-view-epoch", "1")
            .header(CONTENT_TYPE, APPLICATION_FIG_API.clone())
            .body(ping_body())
            .unwrap();

        let response = handle(api_state(), request, AUTOCOMPLETE_ID).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(CONTENT_TYPE), Some(&APPLICATION_FIG_API));

        let message = fig_proto::fig::ServerOriginatedMessage::decode(response.body().as_ref()).unwrap();
        assert_eq!(message.id, Some(7), "the response must carry the request id");
        assert!(
            matches!(message.submessage, Some(Submessage::PingResponse(_))),
            "expected a PingResponse, got {:?}",
            message.submessage
        );
    }

    /// Sends each request through [`handle`] against one fresh [`ApplicationServices`] and records
    /// every exchange: the base64 of the response body (the bytes the webview receives) and its
    /// decoded form for reading.
    async fn transcript(requests: Vec<ClientOriginatedMessage>) -> String {
        use std::fmt::Write as _;

        use base64::prelude::*;

        let state = api_state();
        let mut transcript = String::new();
        for request in requests {
            writeln!(transcript, "> {request:?}").unwrap();
            let http_request = Request::builder()
                .method(Method::POST)
                .header("x-fig-view-epoch", "1")
                .header(CONTENT_TYPE, APPLICATION_FIG_API.clone())
                .body(request.encode_to_vec())
                .unwrap();
            match handle(state.clone(), http_request, AUTOCOMPLETE_ID).await {
                Ok(response) => {
                    let body = response.body().as_ref();
                    let message = fig_proto::fig::ServerOriginatedMessage::decode(body).unwrap();
                    writeln!(transcript, "< {} {}", response.status(), BASE64_STANDARD.encode(body)).unwrap();
                    writeln!(transcript, "< {message:?}").unwrap();
                },
                Err(err) => writeln!(transcript, "! {err}").unwrap(),
            }
        }
        transcript
    }

    fn message(id: i64, submessage: ClientOriginatedSubMessage) -> ClientOriginatedMessage {
        ClientOriginatedMessage {
            id: Some(id),
            submessage: Some(submessage),
        }
    }

    // What the desktop answers for the requests only it can handle, recorded before the dispatch
    // was flattened; each must stay byte identical.
    // `tests/fig-api/fig-api-mock/tests/dispatch.rs` records every variant through the shared
    // handlers with a sandboxed `HOME`. Only requests that cannot change the machine are sent
    // here: no session exists, so nothing reaches a shell, and `NoEventLoop` refuses every
    // window event. `UpdateApplicationPropertiesRequest` is left out because the desktop's
    // handler loads keybindings from the user's real settings file and creates it if missing.

    #[tokio::test]
    async fn snapshot_envelope() {
        let requests = vec![
            ClientOriginatedMessage {
                id: None,
                submessage: Some(ClientOriginatedSubMessage::PingRequest(PingRequest {})),
            },
            ClientOriginatedMessage {
                id: Some(1),
                submessage: None,
            },
            message(2, ClientOriginatedSubMessage::PingRequest(PingRequest {})),
        ];
        insta::assert_snapshot!(transcript(requests).await);
    }

    #[tokio::test]
    async fn snapshot_insert_text() {
        use fig_proto::fig::insert_text_request::Type;
        use fig_proto::fig::{
            InsertTextRequest,
            TextUpdate,
        };

        let requests = vec![
            message(
                1,
                ClientOriginatedSubMessage::InsertTextRequest(InsertTextRequest {
                    r#type: Some(Type::Text("echo".into())),
                    terminal_session_id: None,
                }),
            ),
            message(
                2,
                ClientOriginatedSubMessage::InsertTextRequest(InsertTextRequest {
                    r#type: Some(Type::Update(TextUpdate::default())),
                    terminal_session_id: Some("not-a-uuid".into()),
                }),
            ),
            message(
                3,
                ClientOriginatedSubMessage::InsertTextRequest(InsertTextRequest {
                    r#type: Some(Type::Text("echo".into())),
                    terminal_session_id: Some("00000000-0000-0000-0000-000000000000".into()),
                }),
            ),
            message(
                4,
                ClientOriginatedSubMessage::InsertTextRequest(InsertTextRequest {
                    r#type: None,
                    terminal_session_id: None,
                }),
            ),
        ];
        insta::assert_snapshot!(transcript(requests).await);
    }

    #[tokio::test]
    async fn snapshot_notification() {
        use fig_proto::fig::{
            NotificationRequest,
            NotificationType,
        };

        let request = |id, subscribe, r#type: Option<NotificationType>| {
            message(
                id,
                ClientOriginatedSubMessage::NotificationRequest(NotificationRequest {
                    subscribe,
                    r#type: r#type.map(Into::into),
                }),
            )
        };

        let requests = vec![
            request(1, Some(true), Some(NotificationType::NotifyOnSettingsChange)),
            request(2, None, Some(NotificationType::NotifyOnSettingsChange)),
            request(3, Some(false), Some(NotificationType::NotifyOnSettingsChange)),
            request(4, Some(false), Some(NotificationType::NotifyOnSettingsChange)),
            request(5, Some(true), Some(NotificationType::All)),
            request(6, Some(true), None),
        ];
        insta::assert_snapshot!(transcript(requests).await);
    }

    #[tokio::test]
    async fn snapshot_run_process() {
        use fig_proto::fig::RunProcessRequest;

        // An unparseable session id is rejected before anything would run.
        let requests = vec![message(
            1,
            ClientOriginatedSubMessage::RunProcessRequest(RunProcessRequest {
                executable: "true".into(),
                terminal_session_id: Some("not-a-uuid".into()),
                ..Default::default()
            }),
        )];
        insta::assert_snapshot!(transcript(requests).await);
    }

    #[tokio::test]
    async fn snapshot_position_window() {
        use fig_proto::fig::{
            Point,
            PositionWindowRequest,
            Size,
        };

        let size = Some(Size {
            width: 300.0,
            height: 200.0,
        });
        let requests = vec![
            message(
                1,
                ClientOriginatedSubMessage::PositionWindowRequest(PositionWindowRequest::default()),
            ),
            message(
                2,
                ClientOriginatedSubMessage::PositionWindowRequest(PositionWindowRequest {
                    size,
                    ..Default::default()
                }),
            ),
            message(
                3,
                ClientOriginatedSubMessage::PositionWindowRequest(PositionWindowRequest {
                    anchor: Some(Point { x: 10.0, y: 20.0 }),
                    size,
                    dryrun: Some(true),
                }),
            ),
        ];
        insta::assert_snapshot!(transcript(requests).await);
    }

    #[tokio::test]
    async fn snapshot_drag_window() {
        use fig_proto::fig::DragWindowRequest;

        let requests = vec![message(
            1,
            ClientOriginatedSubMessage::DragWindowRequest(DragWindowRequest {}),
        )];
        insta::assert_snapshot!(transcript(requests).await);
    }

    #[tokio::test]
    async fn snapshot_onboarding() {
        use fig_proto::fig::{
            OnboardingAction,
            OnboardingRequest,
        };

        // The other actions install or remove dotfiles, write state, open Terminal or prompt for
        // accessibility.
        let request = |id, action: OnboardingAction| {
            message(
                id,
                ClientOriginatedSubMessage::OnboardingRequest(OnboardingRequest { action: action.into() }),
            )
        };
        let requests = vec![
            request(1, OnboardingAction::CloseAccessibilityPromptWindow),
            request(2, OnboardingAction::RequestRestart),
        ];
        insta::assert_snapshot!(transcript(requests).await);
    }
}
