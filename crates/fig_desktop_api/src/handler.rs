use std::time::Duration;

use base64::prelude::*;
use fig_os_shim::Env;
pub use fig_proto::fig::client_originated_message::Submessage as ClientOriginatedSubMessage;
pub use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    ClientOriginatedMessage,
    DragWindowRequest,
    InsertTextRequest,
    NotificationRequest,
    OnboardingRequest,
    PositionWindowRequest,
    RunProcessRequest,
    ServerOriginatedMessage,
    UpdateApplicationPropertiesRequest,
};
use fig_proto::prost::Message;
use tracing::warn;

use crate::error::Result;
use crate::requests::{
    self,
    RequestResult,
    RequestResultImpl,
};

/// A request this crate cannot answer by itself, because it needs a window, a figterm session or
/// the event loop. [`api_request`] hands these to the caller.
#[derive(Debug)]
pub enum DesktopRequest {
    InsertText(InsertTextRequest),
    Notification(NotificationRequest),
    RunProcess(RunProcessRequest),
    UpdateApplicationProperties(UpdateApplicationPropertiesRequest),
    PositionWindow(PositionWindowRequest),
    DragWindow(DragWindowRequest),
    Onboarding(OnboardingRequest),
}

pub fn request_from_b64(request_b64: &str) -> Result<ClientOriginatedMessage> {
    let data = BASE64_STANDARD.decode(request_b64)?;
    Ok(ClientOriginatedMessage::decode(data.as_slice())?)
}

pub fn response_to_b64(response_message: ServerOriginatedMessage) -> String {
    BASE64_STANDARD.encode(response_message.encode_to_vec())
}

/// Answers one API request.
///
/// Every request that does not need the desktop app is answered here; the rest go to `desktop`
/// together with the message id. This is the whole dispatch: the desktop app and `fig-api-mock`
/// each pass in a `match` over [`DesktopRequest`].
pub async fn api_request(
    request: ClientOriginatedMessage,
    env: &Env,
    desktop: impl AsyncFnOnce(i64, DesktopRequest) -> RequestResult,
) -> Result<ServerOriginatedMessage> {
    let request_id = match request.id {
        Some(request_id) => request_id,
        None => return Err(crate::error::Error::NoMessageId),
    };

    let response = match tokio::time::timeout(
        Duration::from_secs(60),
        handle_request(request_id, request, env, desktop),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => return Err(crate::error::Error::Timeout),
    };

    Ok(ServerOriginatedMessage {
        id: Some(request_id),
        submessage: Some(match response {
            Ok(msg) => *msg,
            Err(msg) => ServerOriginatedSubMessage::Error(msg.to_string()),
        }),
    })
}

async fn handle_request(
    message_id: i64,
    message: ClientOriginatedMessage,
    env: &Env,
    desktop: impl AsyncFnOnce(i64, DesktopRequest) -> RequestResult,
) -> RequestResult {
    match message.submessage {
        Some(submessage) => {
            use ClientOriginatedSubMessage::{
                AppendToFileRequest,
                ContentsOfDirectoryRequest,
                CreateDirectoryRequest,
                DestinationOfSymbolicLinkRequest,
                DragWindowRequest,
                GetLocalStateRequest,
                GetPlatformInfoRequest,
                GetSettingsPropertyRequest,
                HistoryQueryRequest,
                InsertTextRequest,
                InstallRequest,
                NotificationRequest,
                OnboardingRequest,
                OpenInExternalApplicationRequest,
                PingRequest,
                PositionWindowRequest,
                ReadFileRequest,
                RunProcessRequest,
                UpdateApplicationPropertiesRequest,
                UpdateLocalStateRequest,
                UpdateSettingsPropertyRequest,
                WriteFileRequest,
            };
            use requests::*;

            match submessage {
                // figterm
                InsertTextRequest(request) => desktop(message_id, DesktopRequest::InsertText(request)).await,
                // fs
                ReadFileRequest(request) => fs::read_file(request, env).await,
                WriteFileRequest(request) => fs::write_file(request, env).await,
                AppendToFileRequest(request) => fs::append_to_file(request, env).await,
                DestinationOfSymbolicLinkRequest(request) => fs::destination_of_symbolic_link(request, env).await,
                ContentsOfDirectoryRequest(request) => fs::contents_of_directory(request, env).await,
                CreateDirectoryRequest(request) => fs::create_directory_request(request, env).await,
                // notifications
                NotificationRequest(request) => desktop(message_id, DesktopRequest::Notification(request)).await,
                // process
                RunProcessRequest(request) => desktop(message_id, DesktopRequest::RunProcess(request)).await,
                // properties
                UpdateApplicationPropertiesRequest(request) => {
                    desktop(message_id, DesktopRequest::UpdateApplicationProperties(request)).await
                },
                // state
                GetLocalStateRequest(request) => state::get(request).await,
                UpdateLocalStateRequest(request) => state::update(request).await,
                // settings
                GetSettingsPropertyRequest(request) => settings::get(request).await,
                UpdateSettingsPropertyRequest(request) => settings::update(request).await,
                // window
                PositionWindowRequest(request) => desktop(message_id, DesktopRequest::PositionWindow(request)).await,
                DragWindowRequest(request) => desktop(message_id, DesktopRequest::DragWindow(request)).await,
                // onboarding
                OnboardingRequest(request) => desktop(message_id, DesktopRequest::Onboarding(request)).await,
                // install
                InstallRequest(request) => install::install(request, env).await,
                // history
                HistoryQueryRequest(request) => history::query(request).await,
                // other
                OpenInExternalApplicationRequest(request) => other::open_in_external_application(request).await,
                PingRequest(request) => other::ping(request),
                GetPlatformInfoRequest(request) => platform::get_platform_info(request).await,
            }
        },
        None => {
            warn!("Missing submessage: {message:?}");
            RequestResult::error("Missing submessage")
        },
    }
}
