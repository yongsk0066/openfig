use std::cell::{
    Cell,
    RefCell,
};
use std::sync::atomic::{
    AtomicBool,
    Ordering,
};
use std::sync::{
    Arc,
    LazyLock,
};

use accessibility_sys::pid_t;
use anyhow::Context;
use core_graphics::display::CGRect;
use core_graphics::window::CGWindowID;
use fig_proto::fig::{
    AccessibilityChangeNotification,
    Notification,
    NotificationType,
};
use fig_util::Terminal;
use fig_util::consts::APP_BUNDLE_ID;
use macos_utils::accessibility::accessibility_is_enabled;
use macos_utils::window_server::{
    CGWindowInfo,
    CGWindowLevelForKey,
    UIElement,
};
use macos_utils::{
    NotificationCenter,
    NotificationSubscription,
    WindowServer,
    WindowServerEvent,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSEvent,
    NSEventMask,
    NSWorkspace,
};
use objc2_foundation::{
    NSOperationQueue,
    ns_string,
};
use tao::dpi::{
    LogicalPosition,
    LogicalSize,
    Position,
};
use tao::platform::macos::{
    ActivationPolicy,
    EventLoopWindowTargetExtMacOS,
};
use tracing::{
    debug,
    error,
    warn,
};

use super::PlatformBoundEvent;
use crate::event::{
    Event,
    EventSender,
    WindowEvent,
    WindowPosition,
};
use crate::utils::Rect;
use crate::webview::notification::WebviewNotificationsState;
use crate::webview::{
    FigIdMap,
    WindowId,
};
use crate::{
    AUTOCOMPLETE_ID,
    DASHBOARD_ID,
    EventLoopProxy,
    EventLoopWindowTarget,
};

// See for other window level keys
// https://github.com/phracker/MacOSX-SDKs/blob/master/MacOSX10.8.sdk/System/Library/Frameworks/CoreGraphics.framework/Versions/A/Headers/CGWindowLevel.h
#[allow(non_upper_case_globals)]
const kCGFloatingWindowLevelKey: i32 = 5;

static ACCESSIBILITY_ENABLED: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(accessibility_is_enabled()));

#[derive(Debug)]
struct ScrollMonitor {
    token: objc2::rc::Retained<objc2::runtime::AnyObject>,
    _main_thread: MainThreadMarker,
}

impl ScrollMonitor {
    fn new(main_thread: MainThreadMarker, proxy: EventLoopProxy) -> Option<Self> {
        let handler = block2::RcBlock::new(move |_| {
            if let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() {
                proxy.send_event_or_log(Event::PlatformBoundEvent(PlatformBoundEvent::ExternalScroll {
                    pid: app.processIdentifier(),
                }));
            }
        });
        NSEvent::addGlobalMonitorForEventsMatchingMask_handler(NSEventMask::ScrollWheel, &handler).map(|token| Self {
            token,
            _main_thread: main_thread,
        })
    }
}

impl Drop for ScrollMonitor {
    fn drop(&mut self) {
        // SAFETY: token is the live value returned by addGlobalMonitor, removed once on main.
        unsafe { NSEvent::removeMonitor(&self.token) };
    }
}

#[derive(Debug)]
pub struct PlatformState {
    main_thread: MainThreadMarker,
    activation_policy: Cell<ActivationPolicy>,
    accessibility_subscription: RefCell<Option<NotificationSubscription>>,
    _scroll_monitor: Option<ScrollMonitor>,
    proxy: EventLoopProxy,
    focused_window: RefCell<Option<PlatformWindowImpl>>,
    // Native callbacks enqueue events; they never re-enter these main-loop RefCells.
    window_server: RefCell<Option<WindowServer>>,
    activation_queries: tokio::sync::watch::Sender<Option<macos_utils::window_server::ApplicationSpecifier>>,
    /// Visibility and outstanding show intent are separate: geometry misses keep a slow retry.
    autocomplete_visible: Arc<AtomicBool>,
    autocomplete_tracking: Arc<AtomicBool>,
    caret_tracking: RefCell<super::caret_tracking::CaretTracking>,
    frame_watch: RefCell<fig_desktop::native::frame_watch::FrameWatch>,
    frame_watch_queued: Arc<AtomicBool>,
    frame_watch_wake: Arc<tokio::sync::Notify>,
    /// One serial worker; only identity-checked owned geometry returns to the main loop.
    window_frames: tokio::sync::watch::Sender<Option<super::focus_validation::FocusTarget>>,
    focus_generation: std::sync::atomic::AtomicU64,
    caret_worker: super::caret_worker::CaretWorker,
    input_method: Option<super::input_method::InputMethod>,
    input_method_ready: Cell<bool>,
    input_method_recovery: RefCell<super::input_method_recovery::Coordinator>,
    input_method_scrolled: Cell<bool>,
    input_method_focus_validation: tokio::sync::watch::Sender<Option<super::focus_validation::FocusTarget>>,
    caret_sequence: Cell<u64>,
    input_method_refreshed_at: Cell<Option<std::time::Instant>>,
    focus_validation: tokio::sync::watch::Sender<Option<super::focus_validation::FocusTarget>>,
}

#[derive(Debug)]
struct PlatformWindowImpl {
    window_id: CGWindowID,
    pub bundle_id: String,
    pub pid: pid_t,
    frame: Option<(u64, CGWindowInfo)>,
}

impl From<CGRect> for Rect {
    fn from(cgr: CGRect) -> Rect {
        Rect {
            position: LogicalPosition::new(cgr.origin.x, cgr.origin.y).into(),
            size: LogicalSize::new(cgr.size.width, cgr.size.height).into(),
        }
    }
}

impl PlatformWindowImpl {
    fn new(bundle_id: String, pid: pid_t, window_id: CGWindowID) -> Self {
        Self {
            window_id,
            pid,
            bundle_id,
            frame: None,
        }
    }

    pub fn bundle_id(&self) -> &str {
        self.bundle_id.as_str()
    }

    pub fn get_bounds(&self, generation: u64) -> Option<CGRect> {
        let (cached_generation, frame) = self.frame.as_ref()?;
        (*cached_generation == generation).then_some(frame.bounds)
    }
}

impl PlatformState {
    pub fn new(
        main_thread: MainThreadMarker,
        activation_policy: ActivationPolicy,
        proxy: EventLoopProxy,
    ) -> anyhow::Result<Self> {
        let focused_window: Option<PlatformWindowImpl> = None;
        let activation_proxy = proxy.clone();
        let activation_queries = fig_desktop::native::start_latest(
            |app: macos_utils::window_server::ApplicationSpecifier| fig_desktop::native::Request::Activation(app.pid),
            move |app, reply| {
                if let fig_desktop::native::Reply::Activation { window, fullscreen } = reply {
                    activation_proxy.send_event_or_log(Event::PlatformBoundEvent(
                        PlatformBoundEvent::ApplicationResolved {
                            app,
                            window_id: window,
                            fullscreen,
                        },
                    ));
                }
            },
        )?;
        let result_proxy = proxy.clone();
        let focus_validation =
            fig_desktop::native::start_latest(fig_desktop::native::Request::Focus, move |target, reply| {
                if let fig_desktop::native::Reply::Focus(focused_window_id) = reply {
                    result_proxy.send_event_or_log(Event::PlatformBoundEvent(
                        PlatformBoundEvent::WindowFocusValidated {
                            target,
                            focused_window_id,
                        },
                    ));
                }
            })?;
        let frame_proxy = proxy.clone();
        let window_frames =
            fig_desktop::native::start_latest(fig_desktop::native::Request::Frame, move |target, reply| {
                if let fig_desktop::native::Reply::Frame(frame) = reply {
                    frame_proxy.send_event_or_log(Event::PlatformBoundEvent(PlatformBoundEvent::WindowFrameResolved {
                        target,
                        frame: frame.map(fig_desktop::native::Frame::into_window),
                    }));
                }
            })?;
        let caret_proxy = proxy.clone();
        let caret_worker = super::caret_worker::CaretWorker::start(move |request, rect| {
            caret_proxy.send_event_or_log(Event::PlatformBoundEvent(PlatformBoundEvent::CaretResolved {
                request,
                rect,
                frame: None,
                suppressed: false,
            }));
        })?;
        let ime_proxy = proxy.clone();
        let ime_focus_proxy = proxy.clone();
        let input_method = super::input_method::InputMethod::start(
            move |request, rect, frame, suppressed| {
                ime_proxy.send_event_or_log(Event::PlatformBoundEvent(PlatformBoundEvent::CaretResolved {
                    request,
                    rect,
                    frame,
                    suppressed,
                }));
            },
            move |event| {
                ime_focus_proxy.send_event_or_log(Event::PlatformBoundEvent(PlatformBoundEvent::InputMethodLifecycle(
                    event,
                )));
            },
        )
        .map_err(|error| warn!(%error, "Input method bridge unavailable; other caret providers remain active"))
        .ok();
        let ime_validation_proxy = proxy.clone();
        let input_method_focus_validation =
            fig_desktop::native::start_latest(fig_desktop::native::Request::TextFocus, move |target, reply| {
                if let fig_desktop::native::Reply::Focus(window) = reply {
                    ime_validation_proxy.send_event_or_log(Event::PlatformBoundEvent(
                        PlatformBoundEvent::InputMethodFocusValidated { target, window },
                    ));
                }
            })?;
        Ok(Self {
            _scroll_monitor: ScrollMonitor::new(main_thread, proxy.clone()),
            caret_worker,
            activation_queries,
            input_method,
            input_method_ready: Cell::new(false),
            input_method_recovery: RefCell::new(super::input_method_recovery::Coordinator::new(
                main_thread,
                proxy.clone(),
            )),
            input_method_scrolled: Cell::new(false),
            input_method_focus_validation,
            caret_sequence: Cell::new(0),
            input_method_refreshed_at: Cell::new(None),
            main_thread,
            activation_policy: Cell::new(activation_policy),
            accessibility_subscription: RefCell::new(None),
            proxy,
            focused_window: RefCell::new(focused_window),
            window_server: RefCell::new(None),
            autocomplete_visible: Arc::new(AtomicBool::new(false)),
            autocomplete_tracking: Arc::new(AtomicBool::new(false)),
            caret_tracking: RefCell::default(),
            frame_watch: RefCell::default(),
            frame_watch_queued: Arc::default(),
            frame_watch_wake: Arc::default(),
            window_frames,
            focus_generation: Default::default(),
            focus_validation,
        })
    }

    pub fn set_activation_policy(&self, target: &EventLoopWindowTarget, policy: ActivationPolicy) {
        if self.activation_policy.replace(policy) != policy {
            debug!(?policy, "Setting application policy");
            target.set_activation_policy_at_runtime(policy);
        }
    }

    /// Called by the autocomplete window's Show/Hide handling; gates the frame watch.
    pub fn set_autocomplete_tracking(&self, tracking: bool) {
        if self.autocomplete_tracking.swap(tracking, Ordering::AcqRel) != tracking {
            self.frame_watch_wake.notify_one();
        }
        if !tracking {
            self.frame_watch.borrow_mut().clear();
            self.caret_tracking.borrow_mut().clear();
        }
    }

    pub fn set_autocomplete_visible(&self, visible: bool) {
        crate::latency::record(
            if visible { "window-visible" } else { "window-hidden" },
            "",
            None,
            None,
            None,
        );
        if self.autocomplete_visible.swap(visible, Ordering::AcqRel) != visible {
            self.frame_watch_wake.notify_one();
        }
    }

    pub fn handle(
        &self,
        event: PlatformBoundEvent,
        window_target: &EventLoopWindowTarget,
        window_map: &FigIdMap,
        notifications_state: &Arc<WebviewNotificationsState>,
        shell_sessions: &fig_shell_ipc::session::ShellSessions,
    ) -> anyhow::Result<()> {
        debug!("Handling platform event: {:?}", event);
        match event {
            PlatformBoundEvent::RequestInputMethodActivation { response } => {
                if !response.is_closed() {
                    let _ = response.send(macos_utils::input_source::request_activation(
                        self.main_thread,
                        &format!("{APP_BUNDLE_ID}.inputmethod"),
                    ));
                }
                Ok(())
            },
            PlatformBoundEvent::ExternalScroll { pid } => {
                let is_current_ime = self.focused_window.borrow().as_ref().is_some_and(|window| {
                    window.pid == pid
                        && Terminal::from_bundle_id(window.bundle_id()).is_some_and(|terminal| {
                            terminal.completion_anchor() == fig_util::terminal::CompletionAnchor::InputMethod
                        })
                });
                if is_current_ime {
                    // Ghostty's IME geometry describes the active grid even in scrollback.
                    // Keep session eligibility, but invalidate coordinates until the next input.
                    self.invalidate_caret_requests();
                    self.input_method_scrolled.set(true);
                    self.caret_tracking.borrow_mut().clear();
                    self.proxy.send_event_or_log(Event::WindowEvent {
                        window_id: AUTOCOMPLETE_ID,
                        window_event: WindowEvent::Hide,
                    });
                }
                Ok(())
            },
            PlatformBoundEvent::FinishInputMethodRecovery => {
                self.input_method_recovery
                    .borrow_mut()
                    .finish(std::time::Instant::now());
                self.validate_input_method_focus();
                Ok(())
            },
            PlatformBoundEvent::InputMethodLifecycle(super::input_method::LifecycleEvent::Attachment {
                request,
                attached,
            }) => {
                // The main-loop focus may have changed after the socket reader validated this
                // reply.
                if self.accepts_caret(request) {
                    self.input_method_recovery
                        .borrow_mut()
                        .observe(attached, std::time::Instant::now());
                }
                Ok(())
            },
            PlatformBoundEvent::InputMethodLifecycle(event) => {
                if event == super::input_method::LifecycleEvent::Connected {
                    self.input_method_recovery.borrow_mut().connected();
                } else if event == super::input_method::LifecycleEvent::Retiring {
                    self.input_method_recovery
                        .borrow_mut()
                        .retiring(std::time::Instant::now());
                }
                crate::latency::record("ime-focus-change", "", None, None, None);
                let is_ime = self
                    .focused_window
                    .borrow()
                    .as_ref()
                    .and_then(|window| Terminal::from_bundle_id(window.bundle_id()))
                    .is_some_and(|terminal| {
                        terminal.completion_anchor() == fig_util::terminal::CompletionAnchor::InputMethod
                    });
                if is_ime {
                    self.cancel_caret();
                    let generation = self.focus_generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
                    if let Some(window) = self.focused_window.borrow().as_ref() {
                        self.input_method_focus_validation
                            .send_replace(Some(super::focus_validation::FocusTarget {
                                pid: window.pid,
                                window_id: window.window_id,
                                generation,
                            }));
                    }
                    self.proxy.send_event_or_log(Event::WindowEvent {
                        window_id: AUTOCOMPLETE_ID,
                        window_event: if self.input_method_recovery.borrow().reconnecting() {
                            WindowEvent::SuspendGeometry
                        } else {
                            WindowEvent::Hide
                        },
                    });
                }
                Ok(())
            },
            PlatformBoundEvent::InputMethodFocusValidated { target, window } => {
                let current =
                    self.focused_window
                        .borrow()
                        .as_ref()
                        .map(|window| super::focus_validation::FocusTarget {
                            pid: window.pid,
                            window_id: window.window_id,
                            generation: self.focus_generation.load(Ordering::Relaxed),
                        });
                crate::latency::record(
                    "ime-focus-validation",
                    "",
                    None,
                    None,
                    Some(usize::from(current == Some(target)) + 2 * usize::from(window == Some(target.window_id))),
                );
                if current == Some(target) && window == Some(target.window_id) {
                    self.input_method_ready.set(true);
                    self.refresh_window_position()?;
                }
                Ok(())
            },
            PlatformBoundEvent::CaretResolved {
                request,
                rect,
                frame,
                suppressed,
            } => {
                let accepted = self.accepts_caret(request);
                crate::latency::record(
                    "caret-result",
                    "",
                    None,
                    None,
                    Some(usize::from(accepted) + 2 * usize::from(rect.is_some())),
                );
                if accepted && suppressed {
                    self.caret_tracking.borrow_mut().clear();
                    if let Some(window) = window_map.get(&AUTOCOMPLETE_ID) {
                        window.handle(
                            WindowEvent::Hide,
                            shell_sessions,
                            self,
                            notifications_state,
                            window_target,
                        );
                    }
                    return Ok(());
                }
                let moving = request.anchor == fig_util::terminal::CompletionAnchor::InputMethod
                    && self.caret_tracking.borrow().moving(std::time::Instant::now());
                if accepted
                    && !moving
                    && let Some(rect) = rect
                    && let Some(window) = window_map.get(&AUTOCOMPLETE_ID)
                    && window.enabled()
                {
                    if let Some(frame) = frame {
                        let frame = CGRect::new(
                            &core_graphics::geometry::CGPoint::new(frame.x, frame.y),
                            &core_graphics::geometry::CGSize::new(frame.width, frame.height),
                        );
                        self.caret_tracking.borrow_mut().remember(
                            request.target,
                            rect,
                            frame,
                            std::time::Instant::now(),
                        );
                    }
                    let position = LogicalPosition::new(rect.x, rect.y).into();
                    let size = LogicalSize::new(rect.width, rect.height).into();
                    let anchor = WindowPosition::RelativeToCaret {
                        caret_position: position,
                        caret_size: size,
                    };
                    window.queue_caret_update(request, anchor);
                }
                if accepted
                    && !moving
                    && request.anchor == fig_util::terminal::CompletionAnchor::InputMethod
                    && rect.is_none()
                    && let Some(window) = window_map.get(&AUTOCOMPLETE_ID)
                {
                    // An IMK miss during a drag is not a user dismissal. Keep the show intent
                    // and frame watch; only a newly verified anchor can resume presentation.
                    window.handle(
                        WindowEvent::SuspendGeometry,
                        shell_sessions,
                        self,
                        notifications_state,
                        window_target,
                    );
                }
                Ok(())
            },
            PlatformBoundEvent::Initialize => {
                if accessibility_is_enabled() {
                    UIElement::set_global_messaging_timeout(0.25);
                }

                let (tx, rx) = flume::unbounded::<WindowServerEvent>();

                self.window_server.borrow_mut().replace(WindowServer::new(
                    self.main_thread,
                    tx,
                    APP_BUNDLE_ID,
                    fig_util::terminal::xterm_bundle_ids(),
                ));

                let accessibility_proxy = self.proxy.clone();
                let distributed = NotificationCenter::distributed_center();
                let ax_notification_name = ns_string!("com.apple.accessibility.api");
                let queue = NSOperationQueue::new();
                let subscription =
                    distributed.subscribe(self.main_thread, ax_notification_name, Some(&queue), move |_| {
                        accessibility_proxy
                            .clone()
                            .send_event(Event::PlatformBoundEvent(
                                PlatformBoundEvent::AccessibilityUpdateRequested,
                            ))
                            .ok();
                    });

                self.accessibility_subscription.replace(Some(subscription));

                let observer_proxy = self.proxy.clone();
                let activation_queries = self.activation_queries.clone();
                tokio::runtime::Handle::current().spawn(async move {
                    while let std::result::Result::Ok(result) = rx.recv_async().await {
                        let mut events: Vec<Event> = vec![];

                        match result {
                            WindowServerEvent::FocusRequested { app } => {
                                activation_queries.send_replace(Some(app));
                            },
                            WindowServerEvent::Workspace(event) => {
                                events.push(Event::PlatformBoundEvent(PlatformBoundEvent::WorkspaceNotification(
                                    event,
                                )));
                            },
                            WindowServerEvent::WindowDestroyed { app } => {
                                events.push(Event::PlatformBoundEvent(PlatformBoundEvent::WindowDestroyed { app }));
                            },
                            WindowServerEvent::ActiveSpaceChanged => {
                                events.push(Event::WindowEvent {
                                    window_id: AUTOCOMPLETE_ID,
                                    window_event: WindowEvent::Hide,
                                });
                            },
                            WindowServerEvent::RequestCaretPositionUpdate => {
                                events.push(Event::PlatformBoundEvent(
                                    PlatformBoundEvent::CaretPositionUpdateRequested,
                                ));
                            },
                        };

                        for event in events {
                            if let Err(e) = observer_proxy.send_event(event) {
                                warn!("Error sending event: {e:?}");
                            }
                        }
                    }
                });

                Ok(())
            },
            PlatformBoundEvent::InitializePostRun => {
                if let Some(window) = window_map.get(&AUTOCOMPLETE_ID) {
                    super::webview_host::install_focus_behavior(&window.webview)?;
                }

                // Observe drag state at 20 Hz; idle native frame queries run at 4 Hz.
                // Only one tick may wait in the main queue.
                let proxy = self.proxy.clone();
                let visible = self.autocomplete_visible.clone();
                let tracking = self.autocomplete_tracking.clone();
                let queued = self.frame_watch_queued.clone();
                let wake = self.frame_watch_wake.clone();
                tokio::runtime::Handle::current().spawn(async move {
                    loop {
                        if !tracking.load(Ordering::Acquire) {
                            wake.notified().await;
                            continue;
                        }
                        let period = if visible.load(Ordering::Acquire) { 50 } else { 250 };
                        tokio::select! {
                            () = wake.notified() => continue,
                            () = tokio::time::sleep(std::time::Duration::from_millis(period)) => {},
                        }
                        if tracking.load(Ordering::Acquire)
                            && !queued.swap(true, Ordering::AcqRel)
                            && proxy
                                .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::FrameWatchTick))
                                .is_err()
                        {
                            break;
                        }
                    }
                });

                // Accessibility trust is only re-evaluated on the `com.apple.accessibility.api`
                // distributed notification, which fires when the user flips the switch — never
                // at login. Launched by the LaunchAgent early in the session, `AXIsProcessTrusted`
                // can answer `false` before TCC is ready, and the app then tracked nothing until
                // the next restart (2026-09-17). Poll cheaply and reconcile on change.
                let proxy = self.proxy.clone();
                tokio::runtime::Handle::current().spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        interval.tick().await;
                        let enabled = accessibility_is_enabled();
                        if enabled != ACCESSIBILITY_ENABLED.load(Ordering::SeqCst)
                            && proxy
                                .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::AccessibilityUpdated {
                                    enabled,
                                }))
                                .is_err()
                        {
                            break;
                        }
                    }
                });
                Ok(())
            },
            PlatformBoundEvent::EditBufferChanged => {
                self.caret_tracking.borrow_mut().clear();
                self.request_window_frame();
                self.input_method_scrolled.set(false);
                // Tracking for the terminal in use can have failed at activation time
                // (`macos_utils::window_server::register`: the 0.25 s AX messaging timeout
                // while the terminal was busy) and nothing else retried it, so the autocomplete
                // window neither followed nor hid (the terminal focus recovery path). An
                // edit-buffer update proves the terminal is responsive right now; retry here. When
                // the app is already tracked this is one `frontmostApplication` call and a map
                // lookup.
                if let Some(window_server) = self.window_server.borrow_mut().as_mut() {
                    window_server.ensure_frontmost_tracked();
                }

                // `debug!`, to match `CaretPositionUpdateRequested` below, which calls the exact
                // same function. Without accessibility permission this fires once per keystroke,
                // and at `error!` it drowned out every real error in the log.
                if let Err(err) = self.refresh_window_position() {
                    debug!(%err, "Failed to refresh window position");
                }
                Ok(())
            },
            PlatformBoundEvent::ApplicationResolved {
                app,
                window_id,
                fullscreen,
            } => {
                let front = frontmost_application();
                debug!(?app, ?window_id, ?front, "Isolated activation resolved");
                if front.as_ref() == Some(&app) {
                    self.proxy.send_event_or_log(Event::PlatformBoundEvent(
                        PlatformBoundEvent::ExternalWindowFocusChanged { app, window_id },
                    ));
                    if let Some(fullscreen) = fullscreen {
                        self.proxy.send_event_or_log(Event::PlatformBoundEvent(
                            PlatformBoundEvent::FullscreenStateUpdated {
                                fullscreen,
                                dashboard_visible: None,
                            },
                        ));
                    }
                }
                Ok(())
            },
            PlatformBoundEvent::WorkspaceNotification(event) => {
                if matches!(&event, macos_utils::window_server::WorkspaceEvent::Activated(app)
                    if app.bundle_id != APP_BUNDLE_ID && frontmost_application().as_ref() == Some(app))
                {
                    self.cancel_caret();
                }
                if let Some(server) = self.window_server.borrow_mut().as_mut() {
                    server.handle_workspace(event);
                }
                Ok(())
            },
            PlatformBoundEvent::ExternalWindowFocusChanged { window_id, app } => {
                let front = frontmost_application();
                let decision = super::focus_validation::classify_focus_change(
                    front.as_ref(),
                    &app,
                    window_id,
                    self.focused_window
                        .borrow()
                        .as_ref()
                        .map(|window| (window.pid, window.bundle_id.as_str(), window.window_id)),
                );
                match decision {
                    super::focus_validation::FocusChange::Stale => return Ok(()),
                    super::focus_validation::FocusChange::Duplicate => {
                        self.validate_input_method_focus();
                        self.refresh_window_position()?;
                        return Ok(());
                    },
                    super::focus_validation::FocusChange::Changed => {},
                }
                // Only a verified change may hide suggestions. Delayed or duplicate AX
                // notifications must not dismiss a list produced by a newer shell edit.
                self.cancel_caret();
                self.focus_generation.fetch_add(1, Ordering::Relaxed);
                let had_focus = self.focused_window.borrow_mut().take().is_some();
                if (had_focus || window_id.is_none() || Terminal::from_bundle_id(app.bundle_id.clone()).is_none())
                    && let Some(autocomplete) = window_map.get(&AUTOCOMPLETE_ID)
                {
                    // Commit the hide in this transition; a queued Hide could overtake the
                    // next shell edit and erase its newly established show intent.
                    autocomplete.handle(
                        WindowEvent::Hide,
                        shell_sessions,
                        self,
                        notifications_state,
                        window_target,
                    );
                }
                let Some(window_id) = window_id else { return Ok(()) };
                let window = PlatformWindowImpl::new(app.bundle_id, app.pid, window_id);
                let current_terminal = Terminal::from_bundle_id(window.bundle_id.clone());

                let enabled_proxy = self.proxy.clone();
                tokio::spawn(async move {
                    let is_terminal_disabled = current_terminal.as_ref().is_some_and(|terminal| {
                        fig_settings::settings::get_bool_or(
                            format!("integrations.{}.disabled", terminal.internal_id()),
                            false,
                        )
                    });

                    let terminal_cursor_backing_installed = current_terminal.is_some();

                    let is_enabled = !is_terminal_disabled
                        && terminal_cursor_backing_installed
                        && !fig_settings::settings::get_bool_or("autocomplete.disable", false)
                        && accessibility_is_enabled();

                    enabled_proxy.send_event_or_log(Event::WindowEvent {
                        window_id: AUTOCOMPLETE_ID,
                        window_event: WindowEvent::SetEnabled(is_enabled),
                    });
                });

                self.focused_window.borrow_mut().replace(window);
                self.request_window_frame();
                self.validate_input_method_focus();
                self.refresh_window_position()?;

                Ok(())
            },
            PlatformBoundEvent::WindowFrameResolved { target, frame } => {
                let mut focused = self.focused_window.borrow_mut();
                let Some(window) = focused.as_mut() else { return Ok(()) };
                if target
                    != (super::focus_validation::FocusTarget {
                        pid: window.pid,
                        window_id: window.window_id,
                        generation: self.focus_generation.load(Ordering::Relaxed),
                    })
                {
                    return Ok(());
                }
                let changed = match (&window.frame, &frame) {
                    (Some((generation, before)), Some(now)) => {
                        *generation != target.generation
                            || before.bounds.origin.x != now.bounds.origin.x
                            || before.bounds.origin.y != now.bounds.origin.y
                            || before.bounds.size.width != now.bounds.size.width
                            || before.bounds.size.height != now.bounds.size.height
                            || before.level != now.level
                    },
                    (None, None) => false,
                    _ => true,
                };
                let translated = frame.as_ref().and_then(|frame| {
                    self.caret_tracking
                        .borrow_mut()
                        .translate(target, frame.bounds, std::time::Instant::now())
                });
                let available = frame.is_some();
                let level = frame.as_ref().map(|info| info.level);
                window.frame = frame.map(|frame| (target.generation, frame));
                drop(focused);
                if changed
                    && !available
                    && let Some(window) = window_map.get(&AUTOCOMPLETE_ID)
                {
                    window.handle(
                        WindowEvent::SuspendGeometry,
                        shell_sessions,
                        self,
                        notifications_state,
                        window_target,
                    );
                }
                if changed {
                    self.frame_watch.borrow_mut().changed(std::time::Instant::now());
                    if let Some(window) = window_map.get(&AUTOCOMPLETE_ID) {
                        // SAFETY: the documented numeric level key has no pointer/thread contract.
                        let above = level
                            .filter(|level| *level != 0)
                            .unwrap_or_else(|| unsafe { CGWindowLevelForKey(kCGFloatingWindowLevelKey) as i64 });
                        super::webview_host::set_level(self.main_thread, &window.window, above);
                    }
                    self.refresh_window_position()?;
                    if let Some(rect) = translated {
                        let request = super::caret_worker::Request {
                            target,
                            sequence: self.caret_sequence.get(),
                            anchor: fig_util::terminal::CompletionAnchor::InputMethod,
                        };
                        if self.accepts_caret(request)
                            && let Some(window) = window_map.get(&AUTOCOMPLETE_ID)
                        {
                            window.queue_caret_update(request, WindowPosition::RelativeToCaret {
                                caret_position: LogicalPosition::new(rect.x, rect.y).into(),
                                caret_size: LogicalSize::new(rect.width, rect.height).into(),
                            });
                            crate::latency::record("caret-window-translation", "", None, None, None);
                        }
                    }
                }
                Ok(())
            },
            PlatformBoundEvent::FrameWatchTick => {
                self.frame_watch_queued.store(false, Ordering::Release);
                if !self.autocomplete_tracking.load(Ordering::Acquire) {
                    return Ok(());
                }
                if self.frame_watch.borrow_mut().due(
                    std::time::Instant::now(),
                    true,
                    self.autocomplete_visible.load(Ordering::Acquire),
                    NSEvent::pressedMouseButtons() != 0,
                ) {
                    self.request_window_frame();
                }
                let input_method_due = self
                    .focused_window
                    .borrow()
                    .as_ref()
                    .and_then(|window| Terminal::from_bundle_id(window.bundle_id()))
                    .is_some_and(|terminal| {
                        terminal.completion_anchor() == fig_util::terminal::CompletionAnchor::InputMethod
                    })
                    && self
                        .input_method_refreshed_at
                        .get()
                        .is_none_or(|time| time.elapsed() >= std::time::Duration::from_millis(250));
                if input_method_due {
                    self.refresh_window_position()?;
                }
                Ok(())
            },
            PlatformBoundEvent::CaretPositionUpdateRequested => {
                self.frame_watch.borrow_mut().changed(std::time::Instant::now());
                self.request_window_frame();
                if let Err(e) = self.refresh_window_position() {
                    debug!(%e, "Failed to refresh window position");
                }
                Ok(())
            },
            PlatformBoundEvent::FullscreenStateUpdated {
                fullscreen,
                dashboard_visible,
            } => {
                let policy = if fullscreen {
                    ActivationPolicy::Accessory
                } else {
                    let dashboard_visible = dashboard_visible.unwrap_or_else(|| {
                        window_map
                            .get(&DASHBOARD_ID)
                            .is_some_and(|window| window.window.is_visible())
                    });

                    if dashboard_visible {
                        ActivationPolicy::Regular
                    } else {
                        ActivationPolicy::Accessory
                    }
                };

                self.set_activation_policy(window_target, policy);
                Ok(())
            },
            PlatformBoundEvent::AccessibilityUpdateRequested => {
                let proxy = self.proxy.clone();
                tokio::runtime::Handle::current().spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let enabled = accessibility_is_enabled();
                    proxy
                        .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::AccessibilityUpdated {
                            enabled,
                        }))
                        .ok();
                    if enabled {
                        // Bounds unresponsive applications; the temporary AX reference is owned.
                        UIElement::set_global_messaging_timeout(0.25);
                    }
                });

                Ok(())
            },
            PlatformBoundEvent::AccessibilityUpdated { enabled } => {
                if !enabled {
                    self.cancel_caret();
                }
                let was_enabled = ACCESSIBILITY_ENABLED.swap(enabled, Ordering::SeqCst);

                // `WindowServer::new` is the only thing that ever registered AX observers, and
                // it runs once during `Initialize`. Without accessibility permission every
                // `register` call returns early, so granting permission at runtime left the app
                // tracking zero applications — reporting autocomplete as enabled while nothing
                // followed the caret — until the next restart.
                if enabled
                    && !was_enabled
                    && let Some(window_server) = self.window_server.borrow_mut().as_mut()
                {
                    window_server.reregister_all();
                }

                let proxy = self.proxy.clone();
                let notifications_state = notifications_state.clone();
                tokio::spawn(async move {
                    if let Err(err) = notifications_state
                        .broadcast_notification_all(
                            &NotificationType::NotifyOnAccessibilityChange,
                            Notification {
                                r#type: Some(fig_proto::fig::notification::Type::AccessibilityChangeNotification(
                                    AccessibilityChangeNotification { enabled },
                                )),
                            },
                            &proxy,
                        )
                        .await
                    {
                        error!(%err, "Failed to broadcast notification");
                    }
                });

                self.proxy.send_event(Event::ReloadAccessibility).ok();

                Ok(())
            },
            PlatformBoundEvent::AppWindowFocusChanged {
                window_id,
                focused,
                fullscreen,
                visible,
            } => {
                // Update activation policy
                if window_id == DASHBOARD_ID && focused {
                    debug!("Sending FullscreenStateUpdated");
                    self.proxy
                        .send_event(Event::PlatformBoundEvent(PlatformBoundEvent::FullscreenStateUpdated {
                            fullscreen,
                            dashboard_visible: Some(visible),
                        }))
                        .ok();
                }
                Ok(())
            },
            PlatformBoundEvent::WindowDestroyed { app } => {
                let focused = self.focused_window.borrow();
                if let Some(window) = focused.as_ref()
                    && window.bundle_id() == app.bundle_id
                    && window.pid == app.pid
                {
                    self.focus_validation
                        .send_replace(Some(super::focus_validation::FocusTarget {
                            pid: window.pid,
                            window_id: window.window_id,
                            generation: self.focus_generation.load(Ordering::Relaxed),
                        }));
                }
                Ok(())
            },
            PlatformBoundEvent::WindowFocusValidated {
                target,
                focused_window_id,
            } => {
                let mut focused = self.focused_window.borrow_mut();
                if let Some(window) = focused.as_ref() {
                    let current = super::focus_validation::FocusTarget {
                        pid: window.pid,
                        window_id: window.window_id,
                        generation: self.focus_generation.load(Ordering::Relaxed),
                    };
                    if super::focus_validation::should_clear(current, target, focused_window_id) {
                        self.cancel_caret();
                        focused.take();
                        self.focus_generation.fetch_add(1, Ordering::Relaxed);
                        self.proxy.send_event_or_log(Event::WindowEvent {
                            window_id: AUTOCOMPLETE_ID,
                            window_event: WindowEvent::Hide,
                        });
                    }
                }
                Ok(())
            },
        }
    }

    fn request_window_frame(&self) {
        if let Some(window) = self.focused_window.borrow().as_ref() {
            self.window_frames
                .send_replace(Some(super::focus_validation::FocusTarget {
                    pid: window.pid,
                    window_id: window.window_id,
                    generation: self.focus_generation.load(Ordering::Relaxed),
                }));
        }
    }

    fn validate_input_method_focus(&self) {
        if let Some(window) = self.focused_window.borrow().as_ref()
            && Terminal::from_bundle_id(window.bundle_id()).is_some_and(|terminal| {
                terminal.completion_anchor() == fig_util::terminal::CompletionAnchor::InputMethod
            })
        {
            self.input_method_focus_validation
                .send_replace(Some(super::focus_validation::FocusTarget {
                    pid: window.pid,
                    window_id: window.window_id,
                    generation: self.focus_generation.load(Ordering::Relaxed),
                }));
        }
    }

    fn cancel_caret(&self) {
        self.caret_tracking.borrow_mut().clear();
        self.frame_watch.borrow_mut().clear();
        self.invalidate_caret_requests();
        self.input_method_ready.set(false);
        self.input_method_scrolled.set(false);
        self.input_method_focus_validation.send_replace(None);
        self.window_frames.send_replace(None);
    }

    fn invalidate_caret_requests(&self) {
        // Invalidate events already queued to main as well as the worker's pending retry.
        self.caret_sequence.set(self.caret_sequence.get().wrapping_add(1));
        self.caret_worker.cancel();
        if let Some(input_method) = &self.input_method {
            input_method.cancel();
        }
    }

    pub fn accepts_caret(&self, request: super::caret_worker::Request) -> bool {
        let focused = self.focused_window.borrow();
        let Some(window) = focused.as_ref() else { return false };
        let target = super::focus_validation::FocusTarget {
            pid: window.pid,
            window_id: window.window_id,
            generation: self.focus_generation.load(Ordering::Relaxed),
        };
        let reason = if window.get_bounds(target.generation).is_none() {
            Some("caret-wait-frame")
        } else if request.anchor == fig_util::terminal::CompletionAnchor::InputMethod
            && (!self.input_method_ready.get() || self.input_method_scrolled.get())
        {
            Some("caret-wait-ime")
        } else if !super::caret_worker::accepts(target, self.caret_sequence.get(), request) {
            Some("caret-stale")
        } else if NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier())
            != Some(target.pid)
        {
            Some("caret-not-foreground")
        } else {
            None
        };
        if let Some(reason) = reason {
            crate::latency::record(reason, "", None, None, None);
        }
        reason.is_none()
    }

    fn refresh_window_position(&self) -> anyhow::Result<()> {
        if self.focused_window.borrow().as_ref().is_some_and(|window| {
            window
                .get_bounds(self.focus_generation.load(Ordering::Relaxed))
                .is_none()
        }) {
            self.request_window_frame();
        }
        let sequence = self.caret_sequence.get().wrapping_add(1);
        self.caret_sequence.set(sequence);
        let guard = self.focused_window.borrow();
        let active = guard.as_ref().context("No active window")?;
        let terminal = Terminal::from_bundle_id(active.bundle_id());
        let frontmost = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier());
        if !should_query_caret(terminal.is_some(), active.pid, frontmost) {
            self.cancel_caret();
            return Ok(());
        }
        if terminal
            .as_ref()
            .is_some_and(|terminal| terminal.completion_anchor() == fig_util::terminal::CompletionAnchor::InputMethod)
        {
            // AppKit text clients may not service IMK requests inside a mouse-tracking loop.
            // Follow the verified window-relative anchor instead; no input-event monitor is needed.
            if NSEvent::pressedMouseButtons() != 0 || self.caret_tracking.borrow().moving(std::time::Instant::now()) {
                return Ok(());
            }
            self.input_method_refreshed_at.set(Some(std::time::Instant::now()));
        }
        let request = super::caret_worker::Request {
            target: super::focus_validation::FocusTarget {
                pid: active.pid,
                window_id: active.window_id,
                generation: self.focus_generation.load(Ordering::Relaxed),
            },
            sequence,
            anchor: terminal.map_or(fig_util::terminal::CompletionAnchor::TextRange, |terminal| {
                terminal.completion_anchor()
            }),
        };
        if request.anchor == fig_util::terminal::CompletionAnchor::InputMethod {
            self.caret_worker.cancel();
            if let Some(input_method) = &self.input_method
                && !self.input_method_scrolled.get()
            {
                input_method.request(request);
            }
        } else {
            if let Some(input_method) = &self.input_method {
                input_method.cancel();
            }
            self.caret_worker.request(request);
        }
        Ok(())
    }

    #[allow(clippy::unused_self)]
    pub fn position_window(
        &self,
        webview_window: &tao::window::Window,
        _window_id: &WindowId,
        position: Position,
    ) -> wry::Result<()> {
        webview_window.set_outer_position(position);
        std::result::Result::Ok(())
    }

    /// Latest worker-validated bounds for the current focused window. Never queries AppKit/CG.
    pub fn active_window_bottom(&self, scale: f64) -> Option<f64> {
        let guard = self.focused_window.borrow();
        let active_window = guard.as_ref()?;
        Some(Rect::from(active_window.get_bounds(self.focus_generation.load(Ordering::Relaxed))?).bottom(scale))
    }

    pub fn accessibility_is_enabled() -> Option<bool> {
        Some(ACCESSIBILITY_ENABLED.load(Ordering::SeqCst))
    }
}

fn frontmost_application() -> Option<macos_utils::window_server::ApplicationSpecifier> {
    let current = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(macos_utils::window_server::ApplicationSpecifier {
        pid: current.processIdentifier(),
        bundle_id: current.bundleIdentifier()?.to_string(),
    })
}

fn should_query_caret(is_terminal: bool, tracked_pid: pid_t, frontmost_pid: Option<pid_t>) -> bool {
    is_terminal && frontmost_pid == Some(tracked_pid)
}

#[cfg(test)]
mod caret_target_tests {
    use super::{
        CGRect,
        CGWindowInfo,
        PlatformWindowImpl,
        should_query_caret,
    };
    #[test]
    fn cached_window_bounds_cannot_cross_a_focus_generation() {
        let mut window = PlatformWindowImpl::new("terminal".into(), 12, 34);
        assert!(window.get_bounds(1).is_none());
        window.frame = Some((1, CGWindowInfo {
            owner_pid: 12,
            window_id: 34,
            bounds: CGRect::new(
                &core_graphics::geometry::CGPoint::new(0.0, 0.0),
                &core_graphics::geometry::CGSize::new(100.0, 100.0),
            ),
            level: 0,
        }));
        assert!(window.get_bounds(1).is_some());
        assert!(window.get_bounds(2).is_none());
    }

    #[test]
    fn queued_caret_events_do_not_query_an_unrelated_foreground_app() {
        assert!(should_query_caret(true, 10, Some(10)));
        assert!(!should_query_caret(true, 10, Some(11)));
        assert!(!should_query_caret(true, 10, None));
        assert!(!should_query_caret(false, 10, Some(10)));
    }
}
