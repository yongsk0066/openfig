use std::sync::Arc;

use macos_utils::window_server::ApplicationSpecifier;
use serde::Serialize;
use tao::dpi::Position;

use crate::protocol::icons::{
    AssetSpecifier,
    ProcessedAsset,
};
use crate::utils::Rect;
use crate::webview::notification::WebviewNotificationsState;
use crate::webview::{
    FigIdMap,
    WindowId,
};
use crate::{
    EventLoopProxy,
    EventLoopWindowTarget,
};

mod macos;
pub use self::macos::*;

#[derive(Debug, Serialize)]
pub struct PlatformState(Arc<PlatformStateImpl>);

impl PlatformState {
    /// Create a new PlatformState instance
    pub fn new(proxy: EventLoopProxy) -> Self {
        Self(Arc::new(PlatformStateImpl::new(proxy)))
    }

    /// Handle a [`PlatformBoundEvent`]
    pub fn handle(
        self: &Arc<Self>,
        event: PlatformBoundEvent,
        window_target: &EventLoopWindowTarget,
        window_map: &FigIdMap,
        notifications_state: &Arc<WebviewNotificationsState>,
    ) -> anyhow::Result<()> {
        self.clone()
            .0
            .handle(event, window_target, window_map, notifications_state)
    }

    /// Position the window at the given coordinates
    pub fn position_window(
        &self,
        webview_window: &tao::window::Window,
        window_id: &WindowId,
        position: Position,
    ) -> wry::Result<()> {
        self.0.position_window(webview_window, window_id, position)
    }

    /// Gets the current cursor position on the screen
    #[allow(dead_code)]
    pub fn get_cursor_position(&self) -> Option<Rect> {
        self.0.get_cursor_position()
    }

    /// The bottom edge of the currently focused window, in logical pixels.
    pub fn active_window_bottom(&self, scale: f64) -> Option<f64> {
        self.0.active_window_bottom(scale)
    }

    /// Looks up icons by name on the platform
    pub async fn icon_lookup(name: &AssetSpecifier<'_>) -> Option<ProcessedAsset> {
        PlatformStateImpl::icon_lookup(name).await
    }

    /// Whether or not accessibility is enabled
    pub fn accessibility_is_enabled() -> Option<bool> {
        PlatformStateImpl::accessibility_is_enabled()
    }

    /// Returns the platform specific implementation.
    #[allow(dead_code)]
    pub fn inner(&self) -> Arc<PlatformStateImpl> {
        Arc::clone(&self.0)
    }
}

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
    WindowDestroyed {
        // TODO: dont use on other platforms than macos
        app: ApplicationSpecifier,
    },
    ExternalWindowFocusChanged {
        window: PlatformWindowImpl,
    },
}
