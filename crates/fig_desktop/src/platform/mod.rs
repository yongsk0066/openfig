//! The macOS platform layer.
use macos_utils::window_server::ApplicationSpecifier;

use crate::webview::WindowId;

mod focus_validation;
mod macos;
mod objc_overrides;
pub(crate) mod webview_host;

pub use self::macos::*;

#[derive(Debug)]
pub enum PlatformBoundEvent {
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
    /// Periodic tick while the autocomplete window is visible: re-read the followed terminal
    /// window's frame and reposition if it moved. macOS posts `kAXWindowMovedNotification`
    /// only when a drag *ends*, so without this the dropdown sat still during the drag.
    FrameWatchTick,
    WindowFocusValidated {
        target: focus_validation::FocusTarget,
        focused_window_id: Option<u32>,
    },
    WindowDestroyed {
        // TODO: dont use on other platforms than macos
        app: ApplicationSpecifier,
    },
    ExternalWindowFocusChanged {
        window: PlatformWindowImpl,
    },
}
