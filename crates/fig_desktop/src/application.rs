//! Application-owned services assembled once and injected into protocol/window adapters.
use std::sync::{
    Arc,
    RwLock,
};

use fig_shell_ipc::session::ShellSessions;

use crate::event::EventSender;
use crate::webview::notification::WebviewNotificationsState;
use crate::webview::page_epoch::PageEpoch;

#[derive(Debug, Default)]
pub struct InterceptState {
    pub intercept_bound_keystrokes: RwLock<bool>,
    pub intercept_global_keystrokes: RwLock<bool>,
}

pub struct ApplicationServices {
    pub page_epoch: Arc<PageEpoch>,
    pub processes: crate::request::process::Processes,
    pub shell_sessions: Arc<ShellSessions>,
    pub intercept_state: Arc<InterceptState>,
    pub notifications_state: Arc<WebviewNotificationsState>,
    pub proxy: Box<dyn EventSender>,
    /// Serialises `positionWindowRequest`s.
    ///
    /// `wrap_custom_protocol` spawns a task per request, so without this every
    /// `positionWindow` call — including the `dryRun` probes the app fires while it decides
    /// whether the description pane fits — runs concurrently. The dead `window.ipc.postMessage`
    /// path used to funnel exactly these through a single task; nothing replaced that when
    /// `api://` took over, and `tokio`'s mutex hands the lock out in wait order, so this
    /// restores the same one-at-a-time behaviour.
    pub position_window: tokio::sync::Mutex<()>,
}
