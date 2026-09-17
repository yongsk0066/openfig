use std::fmt::Display;
use std::sync::atomic::AtomicBool;

use base64::prelude::*;
use bytes::BytesMut;
use fig_proto::fig::notification::Type as NotificationEnum;
use fig_proto::fig::server_originated_message::Submessage as ServerOriginatedSubMessage;
use fig_proto::fig::{
    EventNotification,
    Notification,
    NotificationType,
    ServerOriginatedMessage,
};
use fig_proto::prost::Message;
use fig_shell_ipc::session::{
    ShellCommand,
    ShellSessions,
};
use parking_lot::Mutex;
use tao::dpi::{
    LogicalPosition,
    LogicalSize,
    Position,
    Size,
};
use tao::window::Window;
use tracing::{
    debug,
    error,
    info,
    instrument,
    warn,
};
use url::Url;
use wry::{
    Theme,
    WebContext,
    WebView,
};

use super::notification::WebviewNotificationsState;
use super::to_tao_theme;
use super::window_id::WindowId;
use crate::event::{
    EmitEventName,
    WindowEvent,
    WindowGeometryResult,
    WindowPosition,
};
use crate::platform::{
    self,
    PlatformState,
};
use crate::utils::Rect;
#[allow(unused_imports)]
use crate::{
    AUTOCOMPLETE_ID,
    DASHBOARD_ID,
    EventLoopWindowTarget,
};

pub struct WindowGeometryState {
    /// The outer position of the window by positioning scheme
    pub position: WindowPosition,
    /// The inner size of the window
    pub size: LogicalSize<f64>,
    /// The window anchor, which is added onto the final position
    pub anchor: LogicalSize<f64>,
}

// TODO: Add state for the active terminal window
#[allow(dead_code)]
pub struct WindowState {
    pub window: Window,
    pub webview: WebView,
    pub context: WebContext,
    pub window_id: WindowId,
    pub window_geometry_state: Mutex<WindowGeometryState>,
    /// What this window's frame was last set to, or `None` when nothing is known.
    ///
    /// Compared against instead of reading the frame back with `outer_position()` /
    /// `inner_size()`: tao hands `setFrameTopLeftPoint:` and `setContentSize:` to the main dispatch
    /// queue, while it handles every user event already queued in one pass of the run loop. A
    /// read-back in the second of two geometry events handled together can therefore still see the
    /// frame from before the first, and a move back to that stale frame would be skipped, leaving
    /// the window where the first event put it.
    applied_geometry: Mutex<Option<AppliedGeometry>>,
    pub enabled: AtomicBool,
    pub url: Mutex<Url>,
}

/// A window frame as it was last requested from Cocoa.
#[derive(Debug, Clone, Copy, PartialEq)]
struct AppliedGeometry {
    position: Position,
    size: LogicalSize<f64>,
}

/// The tao calls needed to take a window from `applied` to `target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GeometryCalls {
    /// `set_inner_size`, issued first.
    resize: bool,
    /// `set_outer_position`, issued after the resize.
    reposition: bool,
}

fn geometry_calls(applied: Option<AppliedGeometry>, target: AppliedGeometry) -> GeometryCalls {
    let resize = applied.is_none_or(|applied| applied.size != target.size);
    GeometryCalls {
        resize,
        // Cocoa's origin is the bottom-left, so `setContentSize:` keeps the bottom edge and moves
        // the top edge — the edge this window is anchored by. A resize therefore always needs the
        // position applied after it, even when the position itself did not change. Positioning
        // *before* the resize as well, as this used to, set a frame that the resize and the second
        // positioning replaced within the same main-queue drain.
        reposition: resize || applied.is_none_or(|applied| applied.position != target.position),
    }
}

/// One monitor, as `solve_geometry` needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorFrame {
    /// Physical position, as tao reports it.
    pub position: Position,
    /// Physical size, as tao reports it.
    pub size: Size,
    pub scale_factor: f64,
}

impl From<tao::monitor::MonitorHandle> for MonitorFrame {
    fn from(monitor: tao::monitor::MonitorHandle) -> Self {
        Self {
            position: monitor.position().into(),
            size: monitor.size().into(),
            scale_factor: monitor.scale_factor(),
        }
    }
}

impl MonitorFrame {
    fn rect(&self) -> Rect {
        Rect {
            position: self.position,
            size: self.size,
        }
    }

    fn logical_position(&self) -> LogicalPosition<f64> {
        self.position.to_logical(self.scale_factor)
    }

    fn logical_size(&self) -> LogicalSize<f64> {
        self.size.to_logical(self.scale_factor)
    }
}

/// Everything `solve_geometry` needs to know about the display arrangement.
#[derive(Debug, Clone, Default)]
pub struct Screen {
    /// The monitor the window is currently on, used for `Centered`.
    pub current: Option<MonitorFrame>,
    /// The primary monitor's scale factor. The caret arrives in the primary monitor's
    /// coordinate space, so it is what caret positions are converted with.
    pub primary_scale_factor: Option<f64>,
    pub monitors: Vec<MonitorFrame>,
}

/// Where the autocomplete window goes, and what to tell the app about the result.
///
/// This is the calculation half of `update_window_geometry`, which was 193 lines of state
/// update, monitor search, coordinate conversion, clipping and window manipulation with no
/// tests at all. Everything about the screen and the focused window arrives as an argument, so
/// the arithmetic can be exercised directly.
///
/// `None` means there is nothing to position (no current monitor for a `Centered` request).
fn solve_geometry(
    position: WindowPosition,
    size: LogicalSize<f64>,
    anchor: LogicalSize<f64>,
    screen: &Screen,
    max_height: f64,
    active_window_bottom: impl Fn(f64) -> Option<f64>,
) -> Option<(Position, WindowGeometryResult)> {
    let unclipped = WindowGeometryResult {
        is_above: false,
        is_clipped: false,
    };

    match position {
        WindowPosition::Absolute(position) => Some((position, unclipped)),
        WindowPosition::Centered => {
            let monitor = screen.current?;
            let monitor_position = monitor.logical_position();
            let monitor_size = monitor.logical_size();

            Some((
                Position::Logical(LogicalPosition::new(
                    monitor_position.x + monitor_size.width * 0.5 - size.width * 0.5,
                    monitor_position.y + monitor_size.height * 0.5 - size.height * 0.5,
                )),
                unclipped,
            ))
        },
        WindowPosition::RelativeToCaret {
            caret_position,
            caret_size,
        } => {
            // The monitor the caret is on, plus whether the dropdown fits above or below it.
            let found = screen.monitors.iter().find_map(|monitor| {
                let scale = screen.primary_scale_factor.unwrap_or(monitor.scale_factor);
                let logical_caret_position: LogicalPosition<f64> = caret_position.to_logical(scale);
                let logical_caret_size: LogicalSize<f64> = caret_size.to_logical(scale);

                monitor
                    .rect()
                    .contains(logical_caret_position.into(), monitor.scale_factor)
                    .then(|| {
                        let monitor_position = monitor.logical_position();
                        let monitor_size = monitor.logical_size();

                        (
                            logical_caret_position,
                            logical_caret_size,
                            monitor_position.y >= logical_caret_position.y - max_height,
                            monitor_position.y + monitor_size.height
                                < logical_caret_position.y + logical_caret_size.height + max_height,
                            monitor.scale_factor,
                            Some((monitor_position, monitor_size)),
                        )
                    })
            });

            let (
                caret_position,
                caret_size,
                overflows_monitor_above,
                overflows_monitor_below,
                scale_factor,
                monitor_frame,
            ) = found.unwrap_or_else(|| {
                (
                    caret_position.to_logical(1.0),
                    caret_size.to_logical(1.0),
                    false,
                    false,
                    1.0,
                    None,
                )
            });

            let overflows_window_below = active_window_bottom(scale_factor)
                .is_some_and(|bottom| bottom < max_height + caret_position.y + caret_size.height);

            let is_above = !overflows_monitor_above && (overflows_monitor_below || overflows_window_below);

            let mut x: f64 = caret_position.x + anchor.width;
            let mut y: f64 = if is_above {
                caret_position.y - size.height - anchor.height
            } else {
                caret_position.y + caret_size.height + anchor.height
            };

            let is_clipped = if let Some((monitor_position, monitor_size)) = &monitor_frame {
                let is_clipped = caret_position.x + size.width > monitor_position.x + monitor_size.width;

                x = x
                    .min(monitor_position.x + monitor_size.width - size.width)
                    .max(monitor_position.x);
                y = y
                    .min(monitor_position.y + monitor_size.height - size.height)
                    .max(monitor_position.y);

                is_clipped
            } else {
                false
            };

            Some((Position::Logical(LogicalPosition::new(x, y)), WindowGeometryResult {
                is_above,
                is_clipped,
            }))
        },
    }
}

impl WindowState {
    pub fn new(
        window: Window,
        window_id: WindowId,
        webview: WebView,
        context: WebContext,
        enabled: bool,
        url: Url,
    ) -> Self {
        let scale_factor = window.scale_factor();

        let position = window
            .outer_position()
            .expect("Failed to acquire window position")
            .to_logical(scale_factor);

        let size = window.inner_size().to_logical(scale_factor);

        Self {
            window,
            webview,
            context,
            window_id,
            window_geometry_state: Mutex::new(WindowGeometryState {
                position: WindowPosition::Absolute(Position::Logical(position)),
                size,
                anchor: LogicalSize::<f64>::default(),
            }),
            applied_geometry: Mutex::new(None),
            enabled: enabled.into(),
            url: Mutex::new(url),
        }
    }

    fn set_position(&self, platform_state: &PlatformState, position: Position, which: &'static str) {
        match platform_state.position_window(&self.window, &self.window_id, position) {
            Ok(_) => tracing::trace!(window_id =% self.window_id, ?position, %which, "updated window geometry"),
            Err(err) => tracing::error!(%err, window_id =% self.window_id, "failed to position window"),
        }
    }

    #[instrument(skip(self))]
    fn update_window_geometry(
        &self,
        position: Option<WindowPosition>,
        size: Option<LogicalSize<f64>>,
        anchor: Option<LogicalSize<f64>>,
        platform_state: &PlatformState,
        dry_run: bool,
    ) -> (bool, bool) {
        // Lock our atomic state
        let mut state = self.window_geometry_state.lock();

        // Acquire our position, size, and anchor, and update them if dirty
        let position = match position {
            Some(position) if !dry_run => {
                state.position = position;
                position
            },
            Some(position) => position,
            None => state.position,
        };

        let size = match size {
            Some(size) if !dry_run => {
                state.size = size;
                size
            },
            _ => state.size,
        };

        let anchor = match anchor {
            Some(anchor) if !dry_run => {
                state.anchor = anchor;
                anchor
            },
            _ => state.anchor,
        };

        let window = &self.window;

        let screen = Screen {
            current: window.current_monitor().map(MonitorFrame::from),
            primary_scale_factor: window.primary_monitor().map(|monitor| monitor.scale_factor()),
            monitors: window.available_monitors().map(MonitorFrame::from).collect(),
        };

        let max_height = fig_settings::settings::get_int_or("autocomplete.height", 140) as f64;

        let Some((position, result)) = solve_geometry(position, size, anchor, &screen, max_height, |scale| {
            platform_state.active_window_bottom(scale)
        }) else {
            return (false, false);
        };

        if !dry_run {
            // A keystroke that moves neither the caret nor the suggestion list costs no
            // `setFrameTopLeftPoint:` / `setContentSize:` at all; see `geometry_calls` for the order.
            let target = AppliedGeometry { position, size };
            let mut applied = self.applied_geometry.lock();
            let calls = geometry_calls(*applied, target);

            if calls.resize {
                self.window.set_inner_size(size);
            }
            if calls.reposition {
                let which = if calls.resize { "after resize" } else { "move" };
                self.set_position(platform_state, position, which);
            }
            *applied = Some(target);
        }

        (result.is_above, result.is_clipped)
    }

    #[allow(clippy::only_used_in_recursion)]
    #[allow(clippy::too_many_arguments)]
    #[allow(unused_variables)]
    pub fn handle(
        &self,
        event: WindowEvent,
        shell_sessions: &ShellSessions,
        platform_state: &PlatformState,
        notifications_state: &WebviewNotificationsState,
        window_target: &EventLoopWindowTarget,
    ) {
        match event {
            WindowEvent::UpdateWindowGeometry {
                position,
                size,
                anchor,
                dry_run,
                tx,
            } => {
                let (is_above, is_clipped) =
                    self.update_window_geometry(position, size, anchor, platform_state, dry_run);
                if let Some(tx) = tx
                    && let Err(err) = tx.send(WindowGeometryResult { is_above, is_clipped })
                {
                    tracing::error!(%err, "failed to send window geometry update result");
                }
            },
            WindowEvent::Hide => {
                if !self.window.is_visible() {
                    return;
                }
                self.window.set_visible(false);

                if self.window_id == AUTOCOMPLETE_ID {
                    platform_state.set_autocomplete_visible(false);
                    for session in shell_sessions.inner.lock().linked_sessions.values_mut() {
                        let _ = session
                            .sender
                            .send(ShellCommand::InterceptFigJSVisible { visible: false });
                    }

                    // This used to toggle `set_resizable(true)` then `set_resizable(false)` on
                    // every hide, under a "why?" TODO. The window is built non-resizable and
                    // nothing makes it resizable, so the pair was two `setStyleMask:` calls that
                    // ended where they started. Only a window that did become resizable changes.
                    if self.window.is_resizable() {
                        self.window.set_resizable(false);
                    }

                    // Forget the frame while hidden: the next placement then applies in full, so
                    // anything that moved the window behind our back (a display going away) is
                    // corrected by the time it is shown again.
                    *self.applied_geometry.lock() = None;
                }

                if self.window_id == DASHBOARD_ID {
                    use tao::platform::macos::{
                        ActivationPolicy,
                        EventLoopWindowTargetExtMacOS,
                    };

                    let mut policy_lock = platform::ACTIVATION_POLICY.lock().unwrap();
                    if *policy_lock != ActivationPolicy::Accessory {
                        *policy_lock = ActivationPolicy::Accessory;
                        window_target.set_activation_policy_at_runtime(ActivationPolicy::Accessory);
                    }
                }
            },
            WindowEvent::Show => {
                if self.window_id == AUTOCOMPLETE_ID {
                    // `Hide` has always checked; `Show` did not, so every keystroke re-sent
                    // `InterceptFigJSVisible` to every linked shell session and re-called
                    // `set_visible(true)` on an already visible window.
                    if !self.window.is_visible() {
                        for session in shell_sessions.inner.lock().linked_sessions.values_mut() {
                            let _ = session
                                .sender
                                .send(ShellCommand::InterceptFigJSVisible { visible: true });
                        }

                        self.window.set_visible(true);
                        platform_state.set_autocomplete_visible(true);
                        // We handle setting window level on focus changed on MacOS
                        // TODO: pull this out into platform code.
                    }
                } else {
                    if self.window_id == DASHBOARD_ID {
                        use tao::platform::macos::{
                            ActivationPolicy,
                            EventLoopWindowTargetExtMacOS,
                        };

                        let mut policy_lock = platform::ACTIVATION_POLICY.lock().unwrap();
                        if *policy_lock != ActivationPolicy::Regular {
                            *policy_lock = ActivationPolicy::Regular;
                            window_target.set_activation_policy_at_runtime(ActivationPolicy::Regular);
                        }
                    }

                    self.window.set_visible(true);
                    self.window.set_focus();
                }
            },
            WindowEvent::NavigateAbsolute { url } => {
                self.eval(&format!("window.location.href = '{url}';"));
                *self.url.lock() = url;
            },
            WindowEvent::NavigateRelative { path } => {
                let event_name = "dashboard.navigate";
                let payload = serde_json::json!({ "path": path });

                self.notification(notifications_state, &NotificationType::NotifyOnEvent, Notification {
                    r#type: Some(NotificationEnum::EventNotification(EventNotification {
                        event_name: Some(event_name.to_string()),
                        payload: Some(payload.to_string()),
                    })),
                });
            },
            WindowEvent::NavigateForward => {
                self.eval("window.history.forward();");
            },
            WindowEvent::NavigateBack => {
                self.eval("window.history.back();");
            },
            WindowEvent::Event { event_name, payload } => {
                self.notification(notifications_state, &NotificationType::NotifyOnEvent, Notification {
                    r#type: Some(NotificationEnum::EventNotification(EventNotification {
                        event_name: Some(event_name.into_owned()),
                        payload: payload.map(|s| s.into_owned()),
                    })),
                });
            },
            WindowEvent::Reload => {
                info!(%self.window_id, "Reloading window");

                let url = serde_json::json!(self.url.lock().clone());

                self.eval(&format!(
                    "if (window.location.href === 'about:blank') {{\
                            console.log('Reloading window to', {url});\
                            window.location.href = {url};\
                        }} else {{\
                            console.log('Reloading window');\
                            window.location.reload();\
                        }}"
                ));
            },
            WindowEvent::Emit { event_name, payload } => {
                self.emit(event_name, payload);
            },
            WindowEvent::Devtools => {
                if self.webview.is_devtools_open() {
                    self.webview.close_devtools();
                } else {
                    self.webview.open_devtools();
                }
            },
            WindowEvent::DebugMode(debug_mode) => {
                // Macos does not support setting the webview background color so we have
                // to set the css background root color to see the window
                self.eval(if debug_mode {
                    "document.querySelector(':root').style.setProperty('background-color', 'red');"
                } else {
                    "document.querySelector(':root').style.removeProperty('background-color');"
                });
            },
            WindowEvent::SetEnabled(enabled) => self.set_enabled(enabled),
            WindowEvent::SetTheme(theme) => self.set_theme(theme),
            WindowEvent::Drag => {
                if let Err(err) = self.window.drag_window() {
                    error!(%err, "Failed to drag window");
                }
            },
            WindowEvent::Batch(events) => {
                for event in events {
                    self.handle(
                        event,
                        shell_sessions,
                        platform_state,
                        notifications_state,
                        window_target,
                    );
                }
            },
        }
    }

    /// Run `script` in this window's webview, logging instead of panicking on failure.
    ///
    /// `evaluate_script` returns `Err` once the webview is gone — during shutdown, or after a
    /// WebKit process crash. Notifications keep arriving until the event loop actually stops, so
    /// unwrapping here turned an ordinary shutdown race into a main-thread panic.
    fn eval(&self, script: &str) {
        if let Err(err) = self.webview.evaluate_script(script) {
            warn!(%err, %self.window_id, "Failed to evaluate script in webview");
        }
    }

    pub fn emit(&self, event_name: impl Display, payload: impl Into<serde_json::Value>) {
        let payload = payload.into();
        self.eval(&format!(
            "document.dispatchEvent(new CustomEvent('{event_name}', {{'detail': {payload}}}));"
        ));
    }

    pub fn notification(
        &self,
        notifications_state: &WebviewNotificationsState,
        notification_type: &NotificationType,
        notification: Notification,
    ) {
        let window_id = &self.window_id;
        if let Some(notifications) = notifications_state.subscriptions.get(window_id) {
            if let Some(message_id) = notifications.get(notification_type) {
                let message = ServerOriginatedMessage {
                    id: Some(*message_id),
                    submessage: Some(ServerOriginatedSubMessage::Notification(notification)),
                };

                let mut encoded = BytesMut::new();

                match message.encode(&mut encoded) {
                    Ok(_) => {
                        debug!(?notification_type, %window_id, "Sending notification");
                        self.emit(EmitEventName::ProtoMessageReceived, BASE64_STANDARD.encode(encoded));
                    },
                    Err(err) => error!(%err, "Failed to encode notification"),
                }
            } else {
                warn!(?notification_type, %window_id, "No subscription for notification type");
            }
        } else {
            warn!(?notification_type, %window_id, "No subscriptions for window");
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.eval(format!("window.fig.enabled = {enabled};").as_str());
        self.enabled.store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_theme(&self, theme: Option<Theme>) {
        self.window.set_theme(theme.and_then(to_tao_theme));
    }
}

#[cfg(test)]
mod tests {
    use tao::dpi::PhysicalPosition;

    use super::*;

    fn geometry(x: f64, y: f64, width: f64, height: f64) -> AppliedGeometry {
        AppliedGeometry {
            position: Position::Logical(LogicalPosition::new(x, y)),
            size: LogicalSize::new(width, height),
        }
    }

    #[test]
    fn geometry_calls_only_for_what_changed() {
        let applied = geometry(100.0, 200.0, 300.0, 140.0);
        let calls = |target| geometry_calls(Some(applied), target);

        // Nothing known yet: everything, the resize first.
        assert_eq!(geometry_calls(None, applied), GeometryCalls {
            resize: true,
            reposition: true,
        });
        // The same frame again: nothing.
        assert_eq!(calls(applied), GeometryCalls {
            resize: false,
            reposition: false,
        });
        // Moved only: one positioning, no resize.
        assert_eq!(calls(geometry(101.5, 200.0, 300.0, 140.0)), GeometryCalls {
            resize: false,
            reposition: true,
        });
        // Resized in place: the position still has to be re-applied, because the resize moved the
        // top edge.
        assert_eq!(calls(geometry(100.0, 200.0, 300.0, 80.0)), GeometryCalls {
            resize: true,
            reposition: true,
        });
        // Both: still one positioning.
        assert_eq!(calls(geometry(0.0, 0.0, 300.0, 80.0)), GeometryCalls {
            resize: true,
            reposition: true,
        });
    }

    /// A 1920x1080 monitor at the origin, scale 1, so logical == physical.
    fn monitor() -> MonitorFrame {
        MonitorFrame {
            position: Position::Physical(PhysicalPosition::new(0, 0)),
            size: Size::Physical(tao::dpi::PhysicalSize::new(1920, 1080)),
            scale_factor: 1.0,
        }
    }

    fn screen() -> Screen {
        Screen {
            current: Some(monitor()),
            primary_scale_factor: Some(1.0),
            monitors: vec![monitor()],
        }
    }

    fn caret(x: f64, y: f64) -> WindowPosition {
        WindowPosition::RelativeToCaret {
            caret_position: Position::Logical(LogicalPosition::new(x, y)),
            caret_size: Size::Logical(LogicalSize::new(10.0, 20.0)),
        }
    }

    fn logical(position: Position) -> LogicalPosition<f64> {
        position.to_logical(1.0)
    }

    const SIZE: LogicalSize<f64> = LogicalSize::new(300.0, 140.0);
    const ANCHOR: LogicalSize<f64> = LogicalSize::new(0.0, 5.0);
    const MAX_HEIGHT: f64 = 140.0;

    fn solve(position: WindowPosition, bottom: Option<f64>) -> (Position, WindowGeometryResult) {
        solve_geometry(position, SIZE, ANCHOR, &screen(), MAX_HEIGHT, |_| bottom).unwrap()
    }

    #[test]
    fn an_absolute_position_is_passed_through() {
        let want = Position::Logical(LogicalPosition::new(12.0, 34.0));
        let (position, result) = solve(WindowPosition::Absolute(want), None);

        assert_eq!(logical(position), logical(want));
        assert!(!result.is_above);
        assert!(!result.is_clipped);
    }

    #[test]
    fn centered_puts_the_window_in_the_middle_of_the_current_monitor() {
        let (position, _) = solve(WindowPosition::Centered, None);
        assert_eq!(logical(position), LogicalPosition::new(960.0 - 150.0, 540.0 - 70.0));
    }

    #[test]
    fn there_is_nothing_to_centre_without_a_current_monitor() {
        let screen = Screen {
            current: None,
            ..screen()
        };
        assert!(solve_geometry(WindowPosition::Centered, SIZE, ANCHOR, &screen, MAX_HEIGHT, |_| None).is_none());
    }

    /// Plenty of room below the caret: the window sits under it, offset by the anchor.
    #[test]
    fn the_dropdown_opens_below_the_caret_when_it_fits() {
        let (position, result) = solve(caret(400.0, 300.0), None);

        assert!(!result.is_above);
        assert!(!result.is_clipped);
        assert_eq!(logical(position), LogicalPosition::new(400.0, 300.0 + 20.0 + 5.0));
    }

    /// Near the bottom of the screen there is no room below, so it flips above the caret.
    #[test]
    fn the_dropdown_flips_above_the_caret_at_the_bottom_of_the_screen() {
        let (position, result) = solve(caret(400.0, 1000.0), None);

        assert!(result.is_above);
        assert_eq!(logical(position).y, 1000.0 - 140.0 - 5.0);
    }

    /// It also flips when the *terminal window* ends before the dropdown would, even though the
    /// monitor has room — this is what `active_window_bottom` is for.
    #[test]
    fn the_dropdown_flips_above_when_the_terminal_window_ends_first() {
        let (_, below) = solve(caret(400.0, 300.0), Some(900.0));
        assert!(
            !below.is_above,
            "a window ending at 900 leaves room for 140 below y=320"
        );

        let (_, above) = solve(caret(400.0, 300.0), Some(400.0));
        assert!(above.is_above, "a window ending at 400 does not");
    }

    /// A caret near the right edge reports clipping and is pulled back onto the monitor.
    #[test]
    fn a_caret_near_the_right_edge_clips() {
        let (position, result) = solve(caret(1800.0, 300.0), None);

        assert!(result.is_clipped);
        assert_eq!(logical(position).x, 1920.0 - 300.0, "pulled back inside the monitor");
    }

    /// A caret on no known monitor still yields a position rather than nothing.
    #[test]
    fn a_caret_off_screen_falls_back_to_the_raw_coordinates() {
        let screen = Screen {
            monitors: vec![],
            ..screen()
        };
        let (position, result) =
            solve_geometry(caret(400.0, 300.0), SIZE, ANCHOR, &screen, MAX_HEIGHT, |_| None).unwrap();

        assert!(!result.is_clipped, "nothing to clip against");
        assert_eq!(logical(position), LogicalPosition::new(400.0, 325.0));
    }
}
