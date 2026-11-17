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
    EventSender,
    WindowEvent,
    WindowGeometryResult,
    WindowPosition,
};
use crate::platform::PlatformState;
use crate::utils::Rect;
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

impl WindowGeometryState {
    fn requested(
        &mut self,
        position: Option<WindowPosition>,
        size: Option<LogicalSize<f64>>,
        anchor: Option<LogicalSize<f64>>,
        dry_run: bool,
    ) -> (WindowPosition, LogicalSize<f64>, LogicalSize<f64>) {
        let result = (
            position.unwrap_or(self.position),
            size.unwrap_or(self.size),
            anchor.unwrap_or(self.anchor),
        );
        if !dry_run {
            (self.position, self.size, self.anchor) = result;
        }
        result
    }
}

pub struct WindowState {
    pub(super) page_epoch: std::sync::Arc<super::page_epoch::PageEpoch>,
    pub window: Window,
    pub webview: WebView,
    // Retain the context until after its webview is dropped.
    _context: WebContext,
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
    geometry_turn: Mutex<GeometryTurn>,
    presentation: Mutex<Presentation>,
    pub enabled: AtomicBool,
    pub url: Mutex<Url>,
    pub(super) delivery: Mutex<super::delivery::Delivery>,
    pub(super) recovery_queued: AtomicBool,
    pub(super) watch_delivery: std::sync::Arc<AtomicBool>,
    pub(super) proxy: crate::EventLoopProxy,
}

/// Show is an intent, not permission to expose an unpositioned native window.
#[derive(Default)]
struct Presentation {
    requested: bool,
    anchor: Option<crate::platform::caret_worker::Request>,
}

impl Presentation {
    fn suspend_geometry(&mut self) {
        self.anchor = None;
    }

    fn can_show(&self, ready: bool, accepts: impl FnOnce(crate::platform::caret_worker::Request) -> bool) -> bool {
        self.requested && ready && self.anchor.is_some_and(accepts)
    }
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
        // position applied after it, even when the position itself did not change.
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

/// Coalesces caret-only updates in one event-loop turn. Requests that need a response
/// consume the latest caret immediately; dry runs and hide/show remain ordering barriers.
#[derive(Default)]
struct GeometryTurn {
    caret: Option<(WindowPosition, Option<crate::platform::caret_worker::Request>)>,
    screen: Option<Screen>,
}

impl GeometryTurn {
    fn screen(&mut self, read: impl FnOnce() -> Screen) -> Screen {
        self.screen.get_or_insert_with(read).clone()
    }
}

/// Where the autocomplete window goes, and what to tell the app about the result.
///
/// Pure geometry calculation: screen and focused-window inputs are supplied by the caller;
/// native window mutations happen only after a solution is accepted.
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
    pub(super) fn invalidate_geometry(&self) {
        *self.applied_geometry.lock() = None;
    }

    pub fn new(
        window: Window,
        window_id: WindowId,
        webview: WebView,
        context: WebContext,
        enabled: bool,
        url: Url,
        proxy: crate::EventLoopProxy,
    ) -> anyhow::Result<Self> {
        let scale_factor = window.scale_factor();

        let position = window.outer_position()?.to_logical(scale_factor);

        let size = window.inner_size().to_logical(scale_factor);

        let watch_delivery = std::sync::Arc::new(AtomicBool::new(window_id == AUTOCOMPLETE_ID));
        if window_id == AUTOCOMPLETE_ID {
            let watch = watch_delivery.clone();
            let proxy = proxy.clone();
            let id = window_id.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if watch.load(std::sync::atomic::Ordering::Relaxed)
                        && proxy
                            .send_event(crate::event::Event::WindowEvent {
                                window_id: id.clone(),
                                window_event: WindowEvent::RuntimeTick,
                            })
                            .is_err()
                    {
                        break;
                    }
                }
            });
        }
        Ok(Self {
            page_epoch: Default::default(),
            delivery: Mutex::default(),
            recovery_queued: AtomicBool::new(false),
            watch_delivery,
            proxy,
            window,
            webview,
            _context: context,
            window_id,
            window_geometry_state: Mutex::new(WindowGeometryState {
                position: WindowPosition::Absolute(Position::Logical(position)),
                size,
                anchor: LogicalSize::<f64>::default(),
            }),
            applied_geometry: Mutex::new(None),
            geometry_turn: Mutex::default(),
            presentation: Mutex::default(),
            enabled: enabled.into(),
            url: Mutex::new(url),
        })
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
        let _measure = crate::latency::Measure::new("geometry");
        // Lock our atomic state
        let mut state = self.window_geometry_state.lock();

        // A dry run solves the requested geometry without committing it.
        let (position, size, anchor) = state.requested(position, size, anchor, dry_run);

        let window = &self.window;

        let screen = self.geometry_turn.lock().screen(|| {
            let _measure = crate::latency::Measure::new("geometry-screen");
            Screen {
                current: window.current_monitor().map(MonitorFrame::from),
                primary_scale_factor: window.primary_monitor().map(|monitor| monitor.scale_factor()),
                monitors: window.available_monitors().map(MonitorFrame::from).collect(),
            }
        });

        let max_height = fig_settings::settings::get_int_or("autocomplete.height", 140) as f64;

        let Some((position, result)) = solve_geometry(position, size, anchor, &screen, max_height, |scale| {
            let _measure = crate::latency::Measure::new("geometry-window-bottom");
            platform_state.active_window_bottom(scale)
        }) else {
            return (false, false);
        };

        if !dry_run {
            // A keystroke that moves neither the caret nor the suggestion list costs no
            // `setFrameTopLeftPoint:` / `setContentSize:` at all; see `geometry_calls` for the
            // order.
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

    pub fn invalidate_geometry_screen(&self) {
        self.geometry_turn.lock().screen = None;
    }

    pub fn queue_caret_update(&self, request: crate::platform::caret_worker::Request, position: WindowPosition) {
        self.geometry_turn.lock().caret = Some((position, Some(request)));
        let mut presentation = self.presentation.lock();
        presentation.anchor = Some(request);
        if presentation.requested && !self.window.is_visible() {
            self.proxy.send_event_or_log(crate::event::Event::WindowEvent {
                window_id: self.window_id.clone(),
                window_event: WindowEvent::ReconcileVisibility,
            });
        }
    }

    fn take_pending_caret(&self, platform: &PlatformState) -> Option<WindowPosition> {
        let (position, request) = self.geometry_turn.lock().caret.take()?;
        // Focus/input may change between the worker event and the end-of-turn geometry flush.
        request
            .is_none_or(|request| platform.accepts_caret(request))
            .then_some(position)
    }

    pub fn flush_geometry(&self, platform: &PlatformState) {
        let caret = self.take_pending_caret(platform);
        if let Some(position) = caret {
            self.update_window_geometry(Some(position), None, None, platform, false);
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    #[allow(clippy::too_many_arguments)]
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
                if !dry_run && tx.is_none() && size.is_none() && anchor.is_none() {
                    if position.is_some() {
                        self.geometry_turn.lock().caret = position.map(|position| (position, None));
                    }
                    return;
                }
                if dry_run {
                    self.flush_geometry(platform_state);
                }
                let pending = self.take_pending_caret(platform_state);
                let (is_above, is_clipped) =
                    self.update_window_geometry(position.or(pending), size, anchor, platform_state, dry_run);
                if let Some(tx) = tx
                    && let Err(err) = tx.send(WindowGeometryResult { is_above, is_clipped })
                {
                    tracing::error!(%err, "failed to send window geometry update result");
                }
            },
            WindowEvent::RuntimeReset
            | WindowEvent::RuntimeRecover
            | WindowEvent::RuntimeHello { .. }
            | WindowEvent::RuntimeReady(_)
            | WindowEvent::RuntimeFatal(_)
            | WindowEvent::RuntimeReload(_)
            | WindowEvent::RuntimeAck { .. }
            | WindowEvent::RuntimeTick => {
                self.handle_runtime(event, shell_sessions, platform_state, notifications_state);
            },
            WindowEvent::Hide | WindowEvent::SuspendGeometry => {
                let suspended = matches!(event, WindowEvent::SuspendGeometry);
                crate::latency::record(
                    if suspended { "geometry-suspended" } else { "hide-intent" },
                    "",
                    None,
                    None,
                    None,
                );
                let tracking = {
                    let mut presentation = self.presentation.lock();
                    if suspended {
                        presentation.suspend_geometry();
                    } else {
                        *presentation = Presentation::default();
                    }
                    presentation.requested
                };
                if self.window_id == AUTOCOMPLETE_ID {
                    platform_state.set_autocomplete_tracking(tracking);
                }
                // Commit caret state, but do not perform an invisible placement.
                if let Some(caret) = self.take_pending_caret(platform_state) {
                    self.window_geometry_state.lock().position = caret;
                }
                self.invalidate_geometry_screen();
                if !self.window.is_visible() {
                    return;
                }
                self.window.set_visible(false);

                if self.window_id == AUTOCOMPLETE_ID {
                    platform_state.set_autocomplete_visible(false);
                    shell_sessions.broadcast(|| ShellCommand::InterceptFigJSVisible { visible: false });

                    // Avoid redundant AppKit style-mask updates on an already fixed-size popup.
                    if self.window.is_resizable() {
                        self.window.set_resizable(false);
                    }

                    // Forget the frame while hidden: the next placement then applies in full, so
                    // anything that moved the window behind our back (a display going away) is
                    // corrected by the time it is shown again.
                    *self.applied_geometry.lock() = None;
                }

                if self.window_id == DASHBOARD_ID {
                    use tao::platform::macos::ActivationPolicy;

                    platform_state.set_activation_policy(window_target, ActivationPolicy::Accessory);
                }
            },
            WindowEvent::Show | WindowEvent::ReconcileVisibility => {
                crate::latency::record(
                    "show-intent",
                    "",
                    None,
                    None,
                    Some(usize::from(self.presentation.lock().anchor.is_some())),
                );
                if matches!(event, WindowEvent::Show) {
                    self.presentation.lock().requested = true;
                    if self.window_id == AUTOCOMPLETE_ID {
                        platform_state.set_autocomplete_tracking(true);
                    }
                } else if !self.presentation.lock().requested {
                    return;
                }
                if self.window_id == AUTOCOMPLETE_ID
                    && !self.window.is_visible()
                    && !self
                        .presentation
                        .lock()
                        .can_show(self.delivery.lock().phase == super::delivery::Phase::Ready, |request| {
                            platform_state.accepts_caret(request)
                        })
                {
                    return;
                }
                if !self.window.is_visible() {
                    self.flush_geometry(platform_state);
                }
                if self.window_id == AUTOCOMPLETE_ID && self.delivery.lock().phase != super::delivery::Phase::Ready {
                    return;
                }
                if self.window_id == AUTOCOMPLETE_ID {
                    // `Hide` has always checked; `Show` did not, so every keystroke re-sent
                    // `InterceptFigJSVisible` to every linked shell session and re-called
                    // `set_visible(true)` on an already visible window.
                    if !self.window.is_visible() {
                        if let Err(error) = crate::platform::webview_host::show_without_activation(&self.window) {
                            tracing::error!(%error, "Unable to show completion window");
                            return;
                        }
                        shell_sessions.broadcast(|| ShellCommand::InterceptFigJSVisible { visible: true });
                        platform_state.set_autocomplete_visible(true);
                        // PlatformState synchronizes the native window level on focus changes.
                    }
                } else {
                    if self.window_id == DASHBOARD_ID {
                        use tao::platform::macos::ActivationPolicy;

                        platform_state.set_activation_policy(window_target, ActivationPolicy::Regular);
                    }

                    self.window.set_visible(true);
                    self.window.set_focus();
                    if let Err(err) = self.webview.focus() {
                        warn!(%err, "Unable to focus dashboard content");
                    }
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
    pub(super) fn eval(&self, script: &str) {
        if let Err(err) = self.webview.evaluate_script(script) {
            warn!(%err, %self.window_id, "Failed to evaluate script in webview");
        }
    }

    pub fn emit_edit_buffer(&self, channel: i64, notification: fig_proto::fig::EditBufferChangedNotification) {
        if self.window_id == AUTOCOMPLETE_ID {
            self.enqueue(super::delivery::Frame::Edit {
                channel,
                value: Box::new(notification),
            });
        } else {
            self.eval(&Self::edit_script(channel, notification));
        }
    }

    pub(super) fn edit_script(channel: i64, notification: fig_proto::fig::EditBufferChangedNotification) -> String {
        use base64::prelude::*;
        use fig_proto::fig::{
            Notification,
            ServerOriginatedMessage,
            notification,
            server_originated_message,
        };
        use fig_proto::prost::Message;
        let message = ServerOriginatedMessage {
            id: Some(channel),
            submessage: Some(server_originated_message::Submessage::Notification(Notification {
                r#type: Some(notification::Type::EditBufferNotification(notification)),
            })),
        };
        Self::event_script(
            crate::event::EmitEventName::Notification,
            BASE64_STANDARD.encode(message.encode_to_vec()).into(),
        )
    }

    pub fn emit(&self, event_name: impl Display, payload: impl Into<serde_json::Value>) {
        let script = Self::event_script(event_name, payload.into());
        if self.window_id == AUTOCOMPLETE_ID {
            self.enqueue(super::delivery::Frame::Script(script));
        } else {
            self.eval(&script);
        }
    }

    fn event_script(event_name: impl Display, payload: serde_json::Value) -> String {
        if event_name.to_string() == "FigProtoMessageReceived" {
            format!("window.figReceiveMessage({payload});")
        } else {
            format!("document.dispatchEvent(new CustomEvent('{event_name}', {{'detail': {payload}}}));")
        }
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
    fn edit_transport_encoding_round_trip_and_stage_cost() {
        use base64::prelude::*;
        use fig_proto::fig::{
            EditBufferChangedNotification,
            ServerOriginatedMessage,
        };
        use fig_proto::prost::Message;
        for length in [100, 16_384] {
            let value = EditBufferChangedNotification {
                buffer: Some("x".repeat(length)),
                ..Default::default()
            };
            let mut samples = Vec::new();
            let mut bytes = 0;
            for iteration in 0..1050 {
                let input = value.clone();
                let start = std::time::Instant::now();
                let script = WindowState::edit_script(7, input);
                let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
                bytes = std::hint::black_box(script.len());
                if iteration >= 50 {
                    samples.push(elapsed);
                }
                if iteration == 0 {
                    let payload = script
                        .strip_prefix("window.figReceiveMessage(")
                        .unwrap()
                        .strip_suffix(");")
                        .unwrap();
                    let encoded: String = serde_json::from_str(payload).unwrap();
                    let decoded =
                        ServerOriginatedMessage::decode(BASE64_STANDARD.decode(encoded).unwrap().as_slice()).unwrap();
                    assert_eq!(decoded.id, Some(7));
                    let Some(fig_proto::fig::server_originated_message::Submessage::Notification(notification)) =
                        decoded.submessage
                    else {
                        panic!("expected notification");
                    };
                    assert_eq!(
                        notification.r#type,
                        Some(fig_proto::fig::notification::Type::EditBufferNotification(
                            value.clone()
                        ))
                    );
                }
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "edit-encode input_bytes={length} script_bytes={bytes} p50_us={} p95_us={} (no WebKit/IPC time)",
                samples[500], samples[950]
            );
        }
    }

    #[test]
    fn dry_run_uses_requested_size_and_anchor_without_mutating_live_geometry() {
        let mut state = WindowGeometryState {
            position: WindowPosition::Centered,
            size: LogicalSize::new(300.0, 140.0),
            anchor: LogicalSize::new(0.0, 2.0),
        };
        let size = LogicalSize::new(600.0, 200.0);
        let anchor = LogicalSize::new(-300.0, 5.0);
        let (_, requested_size, requested_anchor) = state.requested(None, Some(size), Some(anchor), true);
        assert_eq!((requested_size, requested_anchor), (size, anchor));
        assert_eq!(state.size, LogicalSize::new(300.0, 140.0));
        assert_eq!(state.anchor, LogicalSize::new(0.0, 2.0));
        state.requested(None, Some(size), Some(anchor), false);
        assert_eq!((state.size, state.anchor), (size, anchor));
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

    #[test]
    fn delayed_caret_and_runtime_must_not_flash_or_resurrect_a_hidden_popup() {
        use crate::platform::caret_worker::{
            Request,
            accepts,
        };
        use crate::platform::focus_validation::FocusTarget;
        let target = FocusTarget {
            pid: 10,
            window_id: 20,
            generation: 1,
        };
        let request = Request {
            target,
            sequence: 3,
            anchor: fig_util::terminal::CompletionAnchor::InputMethod,
        };
        let mut presentation = Presentation {
            requested: true,
            anchor: None,
        };
        assert!(!presentation.can_show(true, |_| true), "show before caret must wait");
        presentation.anchor = Some(request);
        assert!(
            !presentation.can_show(false, |_| true),
            "caret before runtime must wait"
        );
        assert!(presentation.can_show(true, |r| accepts(target, 3, r)));
        assert!(
            !presentation.can_show(true, |r| accepts(target, 4, r)),
            "new input rejects old coordinates"
        );
        assert!(
            !presentation.can_show(true, |r| accepts(
                FocusTarget {
                    generation: 2,
                    ..target
                },
                3,
                r
            )),
            "focus reuse rejects old coordinates"
        );
        presentation.suspend_geometry();
        assert!(
            presentation.requested,
            "temporary geometry loss preserves the list intent"
        );
        assert!(
            !presentation.can_show(true, |_| true),
            "suspension rejects the old anchor"
        );
        presentation.anchor = Some(request);
        assert!(
            presentation.can_show(true, |r| accepts(target, 3, r)),
            "fresh geometry restores the same list"
        );
        presentation = Presentation::default();
        presentation.anchor = Some(request);
        assert!(
            !presentation.can_show(true, |_| true),
            "late geometry must not undo Hide"
        );
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
    #[test]
    fn geometry_screen_snapshot_is_shared_only_within_one_turn() {
        let mut turn = GeometryTurn::default();
        let calls = std::cell::Cell::new(0);
        let read = || {
            calls.set(calls.get() + 1);
            screen()
        };
        let a = turn.screen(read);
        let b = turn.screen(read);
        assert_eq!(a.monitors, b.monitors);
        assert_eq!(calls.get(), 1);
        turn.screen = None;
        turn.screen(read);
        assert_eq!(calls.get(), 2);
    }
}
