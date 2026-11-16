use std::borrow::Cow;

use tao::dpi::{
    LogicalSize,
    Position,
    Size,
};
use tao::event_loop::ControlFlow;
use tokio::sync::mpsc::UnboundedSender;
use wry::Theme;

use crate::platform::PlatformBoundEvent;
use crate::webview::WindowId;

#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum Event {
    PageEvent {
        epoch: u64,
        event: Box<Event>,
    },
    EditBuffer(crate::event_queue::PendingEdit),
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
    RuntimeReset,
    RuntimeFatal(u64),
    RuntimeReload(u64),
    RuntimeHello {
        epoch: u64,
        nonce: String,
    },
    RuntimeReady(u64),
    RuntimeAck {
        epoch: u64,
        id: u64,
        success: bool,
    },
    RuntimeTick,
    RuntimeRecover,
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

    Devtools,
    DebugMode(bool),

    Drag,
    Batch(Vec<WindowEvent>),
}

impl WindowEvent {
    pub fn is_allowed_while_disabled(&self) -> bool {
        matches!(
            self,
            WindowEvent::RuntimeFatal(_) | WindowEvent::RuntimeReload(_) | WindowEvent::RuntimeReset | WindowEvent::RuntimeHello { .. } | WindowEvent::RuntimeReady(_) | WindowEvent::RuntimeAck { .. } | WindowEvent::RuntimeTick | WindowEvent::RuntimeRecover | WindowEvent::Hide
                | WindowEvent::SetEnabled(_)
                // Keep shell/UI state in sync during focus and permission transitions. A later
                // edit buffer may omit the environment that was only sent while disabled.
                | WindowEvent::Emit {
                    // Both proto variants dispatch FigProtoMessageReceived in the webview.
                    event_name:
                        EmitEventName::GlobalErrorOccurred | EmitEventName::ProtoMessageReceived | EmitEventName::Notification,
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
    /// Order a synchronous shell command between edit-buffer merge groups.
    fn with_edit_buffer_barrier(&self, action: Box<dyn FnOnce() + '_>) {
        action();
    }

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
    fn with_edit_buffer_barrier(&self, action: Box<dyn FnOnce() + '_>) {
        self.with_edit_barrier(action);
    }

    fn try_send_event(&self, event: Event) -> Option<Event> {
        crate::EventLoopProxy::send_event(self, event)
            .err()
            .map(|closed| closed.0)
    }
}

/// Tag queued effects as well as the API future which produced them.
pub struct PageEventSender<'a> {
    pub epoch: u64,
    pub inner: &'a dyn EventSender,
}
impl EventSender for PageEventSender<'_> {
    fn with_edit_buffer_barrier(&self, action: Box<dyn FnOnce() + '_>) {
        self.inner.with_edit_buffer_barrier(action);
    }

    fn try_send_event(&self, event: Event) -> Option<Event> {
        self.inner.try_send_event(Event::PageEvent {
            epoch: self.epoch,
            event: Box::new(event),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_proto_notification_names_keep_delivering_while_the_window_is_disabled() {
        // A focus/permission transition may disable the window while the shell sends its
        // full context. Dropping that notification leaves subsequent lean updates without
        // PATH and aliases, even after the window is enabled again.
        for event_name in [EmitEventName::Notification, EmitEventName::ProtoMessageReceived] {
            let event = WindowEvent::Emit {
                event_name,
                payload: "payload".into(),
            };
            assert!(event.is_allowed_while_disabled());
        }
        assert!(!WindowEvent::Show.is_allowed_while_disabled());
    }
}
