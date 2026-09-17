mod insert_text;
mod notifications;
mod onboarding;
mod process;
mod properties;
mod window;

use std::sync::Arc;

use fig_desktop_api::handler::DesktopRequest;
use fig_desktop_api::requests::{
    RequestResult,
    RequestResultImpl,
};

use crate::webview::{
    ApiState,
    WindowId,
};

/// Answers the requests only the desktop app can: `fig_desktop_api::handler::api_request` passes
/// them here.
pub async fn handle(
    request: DesktopRequest,
    message_id: i64,
    window_id: WindowId,
    state: &ApiState,
    ctx: &Arc<fig_os_shim::Context>,
) -> RequestResult {
    match request {
        DesktopRequest::InsertText(request) => insert_text::insert_text(request, &state.shell_sessions).await,
        DesktopRequest::Notification(request) => {
            notifications::handle_request(request, window_id, message_id, &state.notifications_state).await
        },
        DesktopRequest::RunProcess(request) => process::run(request, &state.shell_sessions).await,
        DesktopRequest::UpdateApplicationProperties(request) => {
            properties::update(request, &state.shell_sessions, &state.intercept_state)
        },
        DesktopRequest::PositionWindow(request) => {
            window::position_window(request, window_id, &state.shell_sessions, state.proxy.as_ref()).await
        },
        DesktopRequest::DragWindow(request) => window::drag(request, window_id, state.proxy.as_ref()).await,
        DesktopRequest::Onboarding(request) => onboarding::onboarding(request, state.proxy.as_ref(), ctx).await,
    }
}
