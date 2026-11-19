use crate::application::ApplicationServices;
mod insert_text;
mod notifications;
mod onboarding;
pub(crate) mod process;
mod properties;
mod window;

use std::sync::Arc;

use fig_desktop_api::handler::DesktopRequest;
use fig_desktop_api::requests::{
    RequestResult,
    RequestResultImpl,
};

use crate::webview::WindowId;

/// Answers the requests only the desktop app can: `fig_desktop_api::handler::api_request` passes
/// them here.
pub async fn handle(
    request: DesktopRequest,
    message_id: i64,
    window_id: WindowId,
    state: &ApplicationServices,
    ctx: &Arc<fig_os_shim::Context>,
    proxy: &dyn crate::event::EventSender,
) -> RequestResult {
    match request {
        DesktopRequest::Directory { path, classify } => {
            let file_names = state.io.directory(path, classify).await?;
            Ok(Box::new(
                fig_proto::fig::server_originated_message::Submessage::ContentsOfDirectoryResponse(
                    fig_proto::fig::ContentsOfDirectoryResponse { file_names },
                ),
            ))
        },
        DesktopRequest::InsertText(request) => insert_text::insert_text(request, &state.shell_sessions, proxy).await,
        DesktopRequest::Notification(request) => {
            notifications::handle_request(request, window_id, message_id, &state.notifications_state).await
        },
        DesktopRequest::RunProcess(request) => {
            process::run(request, window_id, &state.processes, &state.shell_sessions, &state.io).await
        },
        DesktopRequest::CancelProcess(request) => {
            state.processes.cancel(window_id, &request.cancellation_token)?;
            RequestResult::success()
        },
        DesktopRequest::UpdateApplicationProperties(request) => properties::update(request, &state.shell_sessions),
        DesktopRequest::PositionWindow(request) => {
            window::position_window(request, window_id, &state.shell_sessions, proxy).await
        },
        DesktopRequest::DragWindow(request) => window::drag(request, window_id, proxy).await,
        DesktopRequest::LoginItem(request) => {
            let action = fig_proto::fig::InstallAction::try_from(request.action)
                .map_err(|err| format!("Unknown login item action: {err}"))?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            proxy.send_event_or_err(crate::event::Event::InspectLoginItem(action, tx))?;
            tokio::time::timeout(std::time::Duration::from_secs(5), rx)
                .await
                .map_err(|err| format!("Login item request timed out: {err}"))?
                .map_err(|err| format!("Login item request canceled: {err}"))?
        },
        DesktopRequest::Onboarding(request) => onboarding::onboarding(request, proxy, ctx).await,
    }
}
