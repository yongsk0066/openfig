use std::borrow::Cow;

use tao::dpi::{
    LogicalSize,
    Position,
    Size,
};
use tao::event_loop::{
    ControlFlow,
    EventLoopClosed,
};
use tokio::sync::mpsc::UnboundedSender;
use wry::Theme;

use crate::platform::PlatformBoundEvent;
use crate::webview::WindowId;

#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum Event {
    WindowEvent {
        window_id: WindowId,
        window_event: WindowEvent,
    },
    WindowEventAll {
        window_event: WindowEvent,
    },

    PlatformBoundEvent(PlatformBoundEvent),
    ControlFlow(ControlFlow),
    SetTrayVisible(bool),

    ReloadAccessibility,

    ShowMessageNotification(ShowMessageNotification),
}

impl From<PlatformBoundEvent> for Event {
    fn from(event: PlatformBoundEvent) -> Self {
        Self::PlatformBoundEvent(event)
    }
}

impl From<ShowMessageNotification> for Event {
    fn from(event: ShowMessageNotification) -> Self {
        Self::ShowMessageNotification(event)
    }
}

#[derive(Debug, Default)]
pub struct ShowMessageNotification {
    pub title: Cow<'static, str>,
    pub body: Cow<'static, str>,
    pub parent: Option<WindowId>,
    pub buttons: Option<rfd::MessageButtons>,
    pub buttons_result: Option<tokio::sync::mpsc::Sender<rfd::MessageDialogResult>>,
}

#[derive(Debug, Clone)]
pub enum EmitEventName {
    Notification,
    ProtoMessageReceived,
    GlobalErrorOccurred,
}

impl std::fmt::Display for EmitEventName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Notification | Self::ProtoMessageReceived => "FigProtoMessageReceived",
            Self::GlobalErrorOccurred => "FigGlobalErrorOccurred",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowPosition {
    Absolute(Position),
    Centered,
    RelativeToCaret { caret_position: Position, caret_size: Size },
}

pub struct WindowGeometryResult {
    pub is_above: bool,
    pub is_clipped: bool,
}

#[derive(Debug, Clone)]
pub enum WindowEvent {
    /// Sets the window to be enabled or disabled
    ///
    /// This will cause events to be ignored other than [`WindowEvent::Hide`] and
    /// [`WindowEvent::SetEnabled(true)`]
    SetEnabled(bool),
    /// Sets the theme of the window (light, dark, or system if None)
    SetTheme(Option<Theme>),
    UpdateWindowGeometry {
        position: Option<WindowPosition>,
        size: Option<LogicalSize<f64>>,
        anchor: Option<LogicalSize<f64>>,
        dry_run: bool,
        tx: Option<UnboundedSender<WindowGeometryResult>>,
    },
    /// Hides the window
    Hide,
    Show,
    Emit {
        event_name: EmitEventName,
        payload: Cow<'static, str>,
    },
    NavigateRelative {
        path: Cow<'static, str>,
    },
    NavigateAbsolute {
        url: url::Url,
    },
    NavigateForward,
    NavigateBack,

    Event {
        event_name: Cow<'static, str>,
        payload: Option<Cow<'static, str>>,
    },

    Reload,

    Api {
        /// A base64 encoded protobuf
        payload: String,
    },
    Devtools,
    DebugMode(bool),

    Drag,
    Batch(Vec<WindowEvent>),
}

impl WindowEvent {
    pub fn is_allowed_while_disabled(&self) -> bool {
        matches!(
            self,
            WindowEvent::Hide
                | WindowEvent::SetEnabled(_)
                // TODO: we really shouldnt need to allow these to be called when disabled, 
                // however we allow them at the moment because notification listeners are
                // initialized early on and we dont have a way to delay them until the window
                // is enabled
                | WindowEvent::Api { .. }
                | WindowEvent::Emit {
                    event_name: EmitEventName::GlobalErrorOccurred | EmitEventName::ProtoMessageReceived,
                    ..
                }
        )
    }
}

/// The event-loop handle the request handlers and IPC tasks hold.
///
/// A trait rather than `EventLoopProxy` directly, so the `api://` handler and the request
/// handlers underneath it can be exercised in tests: tao refuses to create an `EventLoop`
/// anywhere but the process main thread, and libtest runs every test on a spawned one.
pub trait EventSender: Send + Sync {
    /// Send `event`, handing it back if the event loop has already exited.
    #[must_use = "an undelivered event is handed back"]
    fn try_send_event(&self, event: Event) -> Option<Event>;

    /// Send `event`, turning a closed event loop into an error for the caller to report.
    fn send_event_or_err(&self, event: Event) -> Result<(), &'static str> {
        match self.try_send_event(event) {
            None => Ok(()),
            Some(_) => Err("the event loop has exited"),
        }
    }

    /// Send `event` from a place that cannot meaningfully handle the loop having exited.
    ///
    /// The event loop only ever ends via `process::exit` (`local_ipc/commands.rs`, `tray.rs`),
    /// so an IPC message, tray click, menu accelerator or settings-watcher wake-up that arrives
    /// during shutdown races it by design. Unwrapping the send result turned that ordinary race
    /// into a panicking task; the event is simply dropped instead.
    fn send_event_or_log(&self, event: Event) {
        if let Some(event) = self.try_send_event(event) {
            tracing::debug!(?event, "Dropped event: the event loop has exited");
        }
    }
}

impl EventSender for crate::EventLoopProxy {
    fn try_send_event(&self, event: Event) -> Option<Event> {
        crate::EventLoopProxy::send_event(self, event)
            .err()
            .map(|EventLoopClosed(event)| event)
    }
}
