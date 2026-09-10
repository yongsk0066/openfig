use std::borrow::Cow;
use std::sync::Arc;

use bytes::Bytes;
use fig_desktop_api::handler::ClientOriginatedSubMessage;
use fig_proto::fig::ClientOriginatedMessage;
use fig_proto::prost::Message;
use fig_settings::{
    Settings,
    State,
};
use wry::http::header::CONTENT_TYPE;
use wry::http::{
    HeaderValue,
    Method,
    Request,
    Response,
    StatusCode,
};

use crate::request::{
    Context,
    EventHandler,
};
use crate::webview::{
    ApiState,
    WindowId,
};

static APPLICATION_FIG_API: HeaderValue = HeaderValue::from_static("application/fig-api");

pub async fn handle(
    state: Arc<ApiState>,
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

    let client_message = ClientOriginatedMessage::decode(Bytes::from(request.into_body()))?;

    // See `ApiState::position_window`.
    let _sequence = match client_message.submessage {
        Some(ClientOriginatedSubMessage::PositionWindowRequest(_)) => Some(state.position_window.lock().await),
        _ => None,
    };

    let server_message = fig_desktop_api::handler::api_request(
        EventHandler::default(),
        Context {
            window_id: &window_id,
            figterm_state: &state.figterm_state,
            intercept_state: &state.intercept_state,
            notifications_state: &state.notifications_state,
            proxy: state.proxy.as_ref(),
            settings: &Settings::new(),
            state: &State::new(),
            ctx: fig_os_shim::Context::new(),
        },
        client_message,
    )
    .await?;

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
    use fig_remote_ipc::figterm::FigtermState;

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

    fn api_state() -> Arc<ApiState> {
        Arc::new(ApiState {
            ctx: fig_os_shim::Context::new_fake(),
            figterm_state: Arc::new(FigtermState::default()),
            intercept_state: Default::default(),
            notifications_state: Arc::new(WebviewNotificationsState::default()),
            proxy: Box::new(NoEventLoop),
            position_window: Default::default(),
        })
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
}
