//! The macOS platform layer.
use macos_utils::window_server::{
    ApplicationSpecifier,
    WorkspaceEvent,
};

use crate::webview::WindowId;

mod caret_tracking;
pub(crate) mod caret_worker;
pub(crate) mod focus_validation;
mod input_method;
mod input_method_recovery;
mod macos;
mod objc_overrides;
pub(crate) mod webview_host;

pub use self::macos::*;

#[derive(Debug)]
pub enum PlatformBoundEvent {
    ApplicationResolved {
        app: ApplicationSpecifier,
        window_id: Option<u32>,
        fullscreen: Option<bool>,
    },
    RequestInputMethodActivation {
        response: tokio::sync::oneshot::Sender<Result<(), i32>>,
    },
    WindowFrameResolved {
        target: focus_validation::FocusTarget,
        frame: Option<macos_utils::window_server::CGWindowInfo>,
    },
    ExternalScroll {
        pid: i32,
    },
    InputMethodLifecycle(input_method::LifecycleEvent),
    FinishInputMethodRecovery,
    InputMethodFocusValidated {
        target: focus_validation::FocusTarget,
        window: Option<u32>,
    },
    CaretResolved {
        request: caret_worker::Request,
        rect: Option<caret_worker::AnchorRect>,
        frame: Option<caret_worker::AnchorRect>,
        suppressed: bool,
    },
    /// Early initialization before the event loop has started
    Initialize,
    /// Late initialization after the event loop has started
    InitializePostRun,
    EditBufferChanged,
    FullscreenStateUpdated {
        fullscreen: bool,
        dashboard_visible: Option<bool>,
    },
    AccessibilityUpdated {
        enabled: bool,
    },
    AccessibilityUpdateRequested,
    AppWindowFocusChanged {
        window_id: WindowId,
        focused: bool,
        fullscreen: bool,
        visible: bool,
    },
    CaretPositionUpdateRequested,
    /// Periodic tick while autocomplete is visible: request the followed terminal
    /// window's frame on the serial worker and reposition if it moved. macOS posts
    /// `kAXWindowMovedNotification` only when a drag *ends*, so without this the dropdown sat still
    /// during the drag.
    FrameWatchTick,
    FocusEligibility(focus_validation::FocusEligibility),
    WindowFocusValidated {
        target: focus_validation::FocusTarget,
        focused_window_id: Option<u32>,
    },
    WindowDestroyed {
        app: ApplicationSpecifier,
    },
    WorkspaceNotification(WorkspaceEvent),
    ExternalWindowFocusChanged {
        window_id: Option<u32>,
        app: ApplicationSpecifier,
    },
}
