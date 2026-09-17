//! The macOS platform layer.
//!
//! `PlatformState` used to be an `Arc<PlatformStateImpl>` newtype whose six methods forwarded to
//! `self.0` verbatim, with a `Native`/`Platform` naming split that only `PlatformBoundEvent`
//! still used. There is one platform; the impl *is* the state.

use macos_utils::window_server::ApplicationSpecifier;

use crate::webview::WindowId;

mod macos;
mod objc_overrides;

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
    WindowDestroyed {
        // TODO: dont use on other platforms than macos
        app: ApplicationSpecifier,
    },
    ExternalWindowFocusChanged {
        window: PlatformWindowImpl,
    },
}
