//! Application-owned services assembled once and injected into protocol/window adapters.
use std::sync::Arc;

use fig_shell_ipc::session::ShellSessions;

use crate::event::EventSender;
use crate::webview::notification::WebviewNotificationsState;
use crate::webview::page_epoch::PageEpoch;

pub struct ApplicationServices {
    pub settings_bus: Arc<crate::notification_bus::NotificationBus>,
    pub icon_cache: crate::protocol::icons::Cache,
    pub page_epoch: Arc<PageEpoch>,
    pub native_icons: Option<fig_desktop::native::Icons>,
    pub io: fig_shell_ipc::process::Service,
    pub processes: crate::request::process::Processes,
    pub shell_sessions: Arc<ShellSessions>,
    pub notifications_state: Arc<WebviewNotificationsState>,
    pub proxy: Box<dyn EventSender>,
    /// Serializes geometry requests, including description-fit probes. Custom protocol
    /// handlers run concurrently; this lock preserves request order across their awaits.
    pub position_window: tokio::sync::Mutex<()>,
}
