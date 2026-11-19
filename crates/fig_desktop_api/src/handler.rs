use std::time::Duration;

use base64::prelude::*;
use fig_os_shim::Env;
pub use fig_proto::fig::client_originated_message::Submessage as ClientOriginatedSubMessage;
pub use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    CancelProcessRequest,
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

/// A request this crate cannot answer by itself, because it needs a window, a shell session or
/// the event loop. [`api_request`] hands these to the caller.
#[derive(Debug)]
pub enum DesktopRequest {
    Directory { path: std::path::PathBuf, classify: bool },
    InsertText(InsertTextRequest),
    Notification(NotificationRequest),
    RunProcess(RunProcessRequest),
    CancelProcess(CancelProcessRequest),
    UpdateApplicationProperties(UpdateApplicationPropertiesRequest),
    PositionWindow(PositionWindowRequest),
    DragWindow(DragWindowRequest),
    Onboarding(OnboardingRequest),
    LoginItem(fig_proto::fig::InstallRequest),
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

    let mut message = ServerOriginatedMessage {
        id: Some(request_id),
        submessage: Some(match response {
            Ok(msg) => *msg,
            Err(msg) => ServerOriginatedSubMessage::Error(msg.to_string()),
        }),
    };
    if message.encoded_len() > fig_proto::MAX_MESSAGE_SIZE {
        message.submessage = Some(ServerOriginatedSubMessage::Error("API response exceeds 16 MiB".into()));
    }
    Ok(message)
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
                CancelProcessRequest,
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
                // shell sessions
                InsertTextRequest(request) => desktop(message_id, DesktopRequest::InsertText(request)).await,
                // fs
                ReadFileRequest(request) => fs::read_file(request, env).await,
                WriteFileRequest(request) => fs::write_file(request, env).await,
                AppendToFileRequest(request) => fs::append_to_file(request, env).await,
                DestinationOfSymbolicLinkRequest(request) => fs::destination_of_symbolic_link(request, env).await,
                ContentsOfDirectoryRequest(request) => {
                    let path = request.directory.as_ref().ok_or("No path provided")?;
                    desktop(message_id, DesktopRequest::Directory {
                        path: crate::util::resolve_filepath(path, env).as_std_path().to_owned(),
                        classify: request.append_directory_separator(),
                    })
                    .await
                },
                CreateDirectoryRequest(request) => fs::create_directory_request(request, env).await,
                // notifications
                NotificationRequest(request) => desktop(message_id, DesktopRequest::Notification(request)).await,
                // process
                RunProcessRequest(request) => desktop(message_id, DesktopRequest::RunProcess(request)).await,
                CancelProcessRequest(request) => desktop(message_id, DesktopRequest::CancelProcess(request)).await,
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
                InstallRequest(request) if request.component == fig_proto::fig::InstallComponent::LoginItem as i32 => {
                    desktop(message_id, DesktopRequest::LoginItem(request)).await
                },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn oversized_desktop_reply_returns_a_correlated_error() {
        let request = ClientOriginatedMessage {
            id: Some(42),
            submessage: Some(ClientOriginatedSubMessage::RunProcessRequest(Default::default())),
        };
        let response = api_request(request, &Env::new(), async |_, _| {
            Ok(Box::new(ServerOriginatedSubMessage::RunProcessResponse(
                fig_proto::fig::RunProcessResponse {
                    stdout: "x".repeat(fig_proto::MAX_MESSAGE_SIZE),
                    ..Default::default()
                },
            )))
        })
        .await
        .unwrap();
        assert_eq!(response.id, Some(42));
        assert!(matches!(
            response.submessage,
            Some(ServerOriginatedSubMessage::Error(_))
        ));
    }
}
