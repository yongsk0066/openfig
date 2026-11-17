#![allow(non_upper_case_globals)]

mod ax_observer;
mod ui_element;
mod window_id;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::Hash;

use accessibility_sys::{
    AXError,
    AXIsProcessTrusted,
    AXObserverRef,
    AXUIElementCreateApplication,
    AXUIElementRef,
    kAXApplicationActivatedNotification,
    kAXApplicationShownNotification,
    kAXErrorAPIDisabled,
    kAXErrorCannotComplete,
    kAXErrorFailure,
    kAXErrorIllegalArgument,
    kAXErrorInvalidUIElement,
    kAXErrorInvalidUIElementObserver,
    kAXErrorNotImplemented,
    kAXErrorNotificationAlreadyRegistered,
    kAXErrorNotificationUnsupported,
    kAXFocusedWindowChangedNotification,
    kAXMainWindowChangedNotification,
    kAXUIElementDestroyedNotification,
    kAXWindowCreatedNotification,
    kAXWindowMovedNotification,
    kAXWindowResizedNotification,
    pid_t,
};
use ax_observer::AXObserver;
use core_foundation::base::{
    CFRelease,
    TCFType,
};
use core_foundation::string::{
    CFString,
    CFStringRef,
};
use flume::Sender;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplicationActivationPolicy,
    NSRunningApplication,
    NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification,
    NSWorkspaceDidActivateApplicationNotification,
    NSWorkspaceDidLaunchApplicationNotification,
    NSWorkspaceDidTerminateApplicationNotification,
};
use objc2_foundation::NSOperationQueue;
use tracing::{
    debug,
    error,
    info,
    trace,
    warn,
};
pub use ui_element::{
    CGWindowInfo,
    CGWindowLevelForKey,
    UIElement,
};

use crate::util::notification_center::get_app_from_notification;
use crate::util::{
    NotificationCenter,
    NotificationSubscription,
};

/// System processes that never host a shell. Our own bundle id is *not* here: it is
/// injected through [`WindowServer::new`] so this crate stays independent of `fig_util` and the
/// list stays testable.
const BLOCKED_BUNDLE_IDS: &[&str] = &[
    "com.apple.ViewBridgeAuxiliary",
    "com.apple.notificationcenterui",
    "com.apple.WebKit.WebContent",
    "com.apple.WebKit.Networking",
    "com.apple.controlcenter",
];

const TRACKED_NOTIFICATIONS: &[&str] = &[
    kAXWindowCreatedNotification,
    kAXFocusedWindowChangedNotification,
    kAXMainWindowChangedNotification,
    kAXApplicationShownNotification,
    kAXApplicationActivatedNotification,
    kAXWindowResizedNotification,
    kAXWindowMovedNotification,
    kAXUIElementDestroyedNotification,
];

#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct ApplicationSpecifier {
    pub pid: pid_t,
    pub bundle_id: String,
}

#[derive(Debug)]
pub enum WorkspaceEvent {
    Activated(ApplicationSpecifier),
    Launched(ApplicationSpecifier),
    Terminated { bundle_id: String },
    SpaceChanged,
}

pub enum WindowServerEvent {
    Workspace(WorkspaceEvent),
    FocusRequested { app: ApplicationSpecifier },
    WindowDestroyed { app: ApplicationSpecifier },
    ActiveSpaceChanged,
    RequestCaretPositionUpdate,
}

pub struct AccessibilityCallbackData {
    app: ApplicationSpecifier,
    sender: Sender<WindowServerEvent>,
}

/// Name known HIServices errors while retaining the numeric value for unknown errors.
fn ax_error_name(err: AXError) -> String {
    let name = match err {
        e if e == kAXErrorFailure => "kAXErrorFailure",
        e if e == kAXErrorIllegalArgument => "kAXErrorIllegalArgument",
        e if e == kAXErrorInvalidUIElement => "kAXErrorInvalidUIElement",
        e if e == kAXErrorInvalidUIElementObserver => "kAXErrorInvalidUIElementObserver",
        e if e == kAXErrorCannotComplete => "kAXErrorCannotComplete",
        e if e == kAXErrorNotificationUnsupported => "kAXErrorNotificationUnsupported",
        e if e == kAXErrorNotImplemented => "kAXErrorNotImplemented",
        e if e == kAXErrorNotificationAlreadyRegistered => "kAXErrorNotificationAlreadyRegistered",
        e if e == kAXErrorAPIDisabled => "kAXErrorAPIDisabled",
        _ => "AXError",
    };
    format!("{name}({err})")
}

/// The bundle identifier, or `None` when the application has no Info.plist.
fn app_bundle_id(app: &NSRunningApplication) -> Option<String> {
    app.bundleIdentifier().map(|s| s.to_string())
}

/// Main-loop owner. Native callbacks send owned events and never borrow this registry.
pub struct WindowServer {
    // Unregister workspace callbacks before dropping AX observers.
    _subscriptions: Vec<NotificationSubscription>,
    inner: WindowServerInner,
}

impl std::fmt::Debug for WindowServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowServer").finish_non_exhaustive()
    }
}

struct WindowServerInner {
    main_thread: MainThreadMarker,
    last_tracking_retry: Option<(ApplicationSpecifier, std::time::Instant)>,
    observers: HashMap<ApplicationSpecifier, AXObserver<AccessibilityCallbackData>, fnv::FnvBuildHasher>,
    sender: Sender<WindowServerEvent>,
    own_bundle_id: &'static str,
    xterm_bundle_ids: &'static [&'static str],
}

impl WindowServer {
    pub fn new(
        main_thread: MainThreadMarker,
        sender: Sender<WindowServerEvent>,
        own_bundle_id: &'static str,
        xterm_bundle_ids: &'static [&'static str],
    ) -> Self {
        let mut inner = WindowServerInner {
            main_thread,
            last_tracking_retry: None,
            observers: Default::default(),
            sender: sender.clone(),
            own_bundle_id,
            xterm_bundle_ids,
        };
        let center = NotificationCenter::workspace_center();
        let queue = NSOperationQueue::mainQueue();
        #[derive(Clone, Copy)]
        enum Kind {
            Activated,
            Launched,
            Terminated,
            SpaceChanged,
        }
        // SAFETY: these immutable AppKit notification names are initialized before main.
        let names = unsafe {
            [
                (NSWorkspaceDidActivateApplicationNotification, Kind::Activated),
                (NSWorkspaceDidLaunchApplicationNotification, Kind::Launched),
                (NSWorkspaceDidTerminateApplicationNotification, Kind::Terminated),
                (NSWorkspaceActiveSpaceDidChangeNotification, Kind::SpaceChanged),
            ]
        };
        let subscriptions = names
            .into_iter()
            .map(|(name, kind)| {
                let sender = sender.clone();
                center.subscribe(main_thread, name, Some(&queue), move |notification| {
                    let application = || {
                        let app = get_app_from_notification(notification)?;
                        Some(ApplicationSpecifier {
                            pid: app.processIdentifier(),
                            bundle_id: app_bundle_id(&app)?,
                        })
                    };
                    let event = match kind {
                        Kind::Activated => application().map(WorkspaceEvent::Activated),
                        Kind::Launched => application().map(WorkspaceEvent::Launched),
                        Kind::Terminated => application().map(|app| WorkspaceEvent::Terminated {
                            bundle_id: app.bundle_id,
                        }),
                        Kind::SpaceChanged => Some(WorkspaceEvent::SpaceChanged),
                    };
                    if let Some(event) = event
                        && let Err(err) = sender.send(WindowServerEvent::Workspace(event))
                    {
                        debug!(%err, "Workspace listener closed");
                    }
                })
            })
            .collect();
        inner.register_all();
        Self {
            _subscriptions: subscriptions,
            inner,
        }
    }

    pub fn reregister_all(&mut self) {
        self.inner.register_all();
    }

    pub fn ensure_frontmost_tracked(&mut self) {
        // SAFETY: MainThreadMarker makes this owner neither Send nor Sync.
        unsafe {
            self.inner.ensure_frontmost_tracked();
        }
    }

    pub fn handle_workspace(&mut self, event: WorkspaceEvent) {
        match event {
            WorkspaceEvent::Activated(app) | WorkspaceEvent::Launched(app) => {
                // The event may have waited in the queue: reject a terminated/reused pid.
                if let Some(running) = NSRunningApplication::runningApplicationWithProcessIdentifier(app.pid)
                    && app_bundle_id(&running).as_deref() == Some(app.bundle_id.as_str())
                {
                    // SAFETY: registry access and observer lifetimes remain on the main thread.
                    unsafe {
                        self.inner.register(&running, true);
                    }
                }
            },
            WorkspaceEvent::Terminated { bundle_id } => {
                let stale = keys_to_deregister(self.inner.observers.keys().cloned(), &bundle_id, |pid| {
                    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                        .is_some_and(|running| app_bundle_id(&running).as_deref() == Some(bundle_id.as_str()))
                });
                for key in stale {
                    self.inner.deregister_one(&key);
                }
            },
            WorkspaceEvent::SpaceChanged => {
                let _ = self.inner.sender.send(WindowServerEvent::ActiveSpaceChanged);
                if let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication()
                    && let Some(bundle_id) = app_bundle_id(&app)
                {
                    let _ = self.inner.sender.send(WindowServerEvent::FocusRequested {
                        app: ApplicationSpecifier {
                            pid: app.processIdentifier(),
                            bundle_id,
                        },
                    });
                }
            },
        }
    }
}

/// Owns the `+1` that `AXUIElementCreateApplication` returns, so every exit path from
/// [`WindowServerInner::register`] balances it.
///
/// `AXObserver::create` retains the element itself, so dropping this after a successful
/// registration leaves the observer holding the only reference — which is exactly the lifetime
/// the observer needs.
struct OwnedAxRef(AXUIElementRef);

impl Drop for OwnedAxRef {
    fn drop(&mut self) {
        // SAFETY:
        // Operation: the `CFRelease` FFI call.
        // Contract (Apple, "Memory Management Programming Guide for Core Foundation"):
        // requires a live, non-null CFType on which the caller owns a reference.
        // Evidence: `self.0` is the `+1` returned by `AXUIElementCreateApplication`, which this
        // value took ownership of at construction and has not released; nothing else releases
        // it. `AXUIElementCreateApplication` does not return NULL for a well-formed pid.
        unsafe { CFRelease(self.0.cast()) };
    }
}

/// The observer keys to drop when `bundle_id` reports a termination: every tracked instance of
/// that bundle id whose pid no longer resolves to a running application.
///
/// Split out of `did_terminate_application` so it can be tested — `observers` holds live
/// `AXObserver`s, which cannot be fabricated without a real accessibility session.
fn keys_to_deregister(
    keys: impl IntoIterator<Item = ApplicationSpecifier>,
    bundle_id: &str,
    is_running: impl Fn(pid_t) -> bool,
) -> Vec<ApplicationSpecifier> {
    keys.into_iter()
        .filter(|key| key.bundle_id == bundle_id && !is_running(key.pid))
        .collect()
}

impl WindowServerInner {
    /// Start observing accessibility events for `ns_app`, unless it is filtered out.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    ///
    /// 1. this call happens on the thread whose `CFRunLoop` the resulting `AXObserver`s are to be
    ///    attached to, and on which they will later be dropped — in this crate, the tao main
    ///    thread. `AXObserver::create` attaches the source to `CFRunLoopGetCurrent()` and
    ///    `AXObserver::drop` removes it from `CFRunLoopGetCurrent()`; a mismatch is a
    ///    use-after-free (see docs/architecture.md (native thread ownership));
    /// 2. `application_ax_callback` remains the callback registered for the observers created here,
    ///    since it reinterprets the `refcon` as `*mut AccessibilityCallbackData`.
    ///
    /// It is *not* a precondition that the process be AX-trusted: that is checked below and
    /// the function returns early if it is not.
    unsafe fn register(&mut self, ns_app: &NSRunningApplication, from_activation: bool) {
        // SAFETY:
        // Operation: the `AXIsProcessTrusted` FFI call.
        // Contract (HIServices): takes no arguments, returns a `Boolean`, has no
        // preconditions, and is safe to call from any thread. The only reason it is `unsafe`
        // in Rust is that it is an `extern` fn.
        // Postcondition: `false` means every later HIServices call would fail with
        // `kAXErrorAPIDisabled`, which is why the early return below is a correctness (not
        // soundness) guard.
        if !unsafe { AXIsProcessTrusted() } {
            info!("Cannot register to observer window events without accessibility perms");
            return;
        }

        let bundle_id = match app_bundle_id(ns_app) {
            Some(bundle_id) => bundle_id,
            None => {
                debug!("Ignoring empty bundle id");
                return;
            },
        };

        let pid = ns_app.processIdentifier();
        let key = ApplicationSpecifier {
            pid,
            bundle_id: bundle_id.clone(),
        };

        if bundle_id == self.own_bundle_id || BLOCKED_BUNDLE_IDS.contains(&bundle_id.as_str()) {
            debug!("Ignoring bundle id {:?}", bundle_id);
            return;
        }

        if ns_app.activationPolicy() == NSApplicationActivationPolicy::Prohibited {
            debug!("Ignoring application by activation policy");
            return;
        }

        // Reuse existing subscriptions, but still refresh focus for activation events.
        let already_registered = self.observers.contains_key(&key);
        if already_registered && !from_activation {
            debug!("app {} is already registered", key.bundle_id);
            return;
        }

        // SAFETY:
        // Operation: the `AXUIElementCreateApplication` FFI call.
        // Contract (HIServices): takes a `pid_t` and returns a `+1` `AXUIElementRef` (Create
        // rule). It has no pointer preconditions; an unknown pid yields an element that later
        // AX calls reject with `kAXErrorInvalidUIElement`.
        // Postcondition: `ax_ref` is a live element that this frame owns one reference to.
        //
        // Keep the Create-rule reference alive through registration. AXObserver::create and
        // UIElement::from_borrowed retain independently; this guard releases the original.
        let owned_ax_ref = OwnedAxRef(unsafe { AXUIElementCreateApplication(pid) });
        let ax_ref = owned_ax_ref.0;

        if from_activation {
            let _ = self.sender.send(WindowServerEvent::FocusRequested { app: key.clone() });
        }

        if already_registered {
            debug!("app {} is already registered", key.bundle_id);
            return;
        }

        if self.xterm_bundle_ids.contains(&key.bundle_id.as_str()) {
            unsafe { UIElement::from_borrowed(ax_ref) }
                .enable_screen_reader_accessibility()
                .ok();
        }

        let bundle_id = key.bundle_id.as_str();
        // SAFETY: owned_ax_ref keeps the AX element live during create, which retains it.
        // Callback data has exactly the type application_ax_callback expects. Registration,
        // callbacks and teardown share the owning run loop; subscribe receives live elements.
        let mut observer = match unsafe {
            AXObserver::create(
                self.main_thread,
                key.pid,
                ax_ref,
                AccessibilityCallbackData {
                    app: key.clone(),
                    sender: self.sender.clone(),
                },
                application_ax_callback,
            )
        } {
            Ok(observer) => observer,
            Err(err) => {
                warn!(
                    bundle_id,
                    err = %ax_error_name(err),
                    "AXObserverCreate failed; tracking is retried on a later edit-buffer update"
                );
                return;
            },
        };

        // Unsupported notifications are optional. Other subscription failures leave the
        // app untracked so a later edit can retry (rate-limited by ensure_frontmost_tracked).
        // Keeping a partially registered observer would suppress those retries permanently.
        let mut failed: Vec<(&'static str, String)> = Vec::new();
        for notification in TRACKED_NOTIFICATIONS {
            match unsafe { observer.subscribe(notification) } {
                Ok(()) => {},
                Err(err) if err == kAXErrorNotificationUnsupported => {
                    debug!(
                        bundle_id,
                        notification = *notification,
                        "notification unsupported by the target; skipped"
                    );
                },
                Err(err) => failed.push((*notification, ax_error_name(err))),
            }
        }

        if failed.is_empty() {
            debug!("Began tracking {bundle_id:?}");
            self.observers.insert(key, observer);
            return;
        }

        warn!(
            bundle_id,
            ?failed,
            "Error setting up tracking; retried on a later edit-buffer update"
        );
    }

    /// See [`WindowServer::ensure_frontmost_tracked`].
    ///
    /// # Safety
    ///
    /// As for `register`.
    unsafe fn ensure_frontmost_tracked(&mut self) {
        let workspace = NSWorkspace::sharedWorkspace();
        let Some(app) = workspace.frontmostApplication() else {
            return;
        };
        let Some(bundle_id) = app_bundle_id(&app) else {
            return;
        };
        let key = ApplicationSpecifier {
            pid: app.processIdentifier(),
            bundle_id,
        };
        if key.bundle_id == self.own_bundle_id || self.observers.contains_key(&key) {
            return;
        }

        // Registration must attach to the main run loop, but a busy terminal must not
        // cause a full AX subscription retry for every keystroke.
        if self
            .last_tracking_retry
            .as_ref()
            .is_some_and(|(previous, time)| previous == &key && time.elapsed() < std::time::Duration::from_secs(1))
        {
            return;
        }
        self.last_tracking_retry = Some((key.clone(), std::time::Instant::now()));

        // SAFETY:
        // Operation: `Self::register`, an unsafe fn.
        // Contract (see its `# Safety`): run-loop affinity, and `application_ax_callback` as the
        // registered callback.
        // Evidence: affinity is this function's own `# Safety` clause, discharged by the caller;
        // the callback argument is fixed inside `register` itself.
        unsafe { self.register(&app, false) };
        if !self.observers.contains_key(&key) {
            // `register` filtered the app out or failed again; it said why at its own level.
            return;
        }
        info!(bundle_id = key.bundle_id, "tracking recovered on an edit-buffer update");

        // Recovery asks the supervised query lane; no target AX read runs on this callback thread.
        let _ = self.sender.send(WindowServerEvent::FocusRequested { app: key });
    }

    /// Drop the observer for exactly one `(pid, bundle_id)`.
    ///
    /// The old `deregister(bundle_id)` matched on the bundle id alone even though the map is
    /// keyed by `(pid, bundle_id)`, so terminating one of two instances of the same app — two
    /// VS Code profiles, say — silently took the other one's observer with it.
    fn deregister_one(&mut self, key: &ApplicationSpecifier) {
        self.observers.remove(key);
    }

    fn register_all(&mut self) {
        self.deregister_all();

        // SAFETY: the registry's MainThreadMarker enforces main-run-loop ownership;
        // register fixes the callback/data pairing for every observer it creates.
        unsafe {
            let workspace = NSWorkspace::sharedWorkspace();
            if let Some(app) = workspace.frontmostApplication() {
                self.register(&app, true);
            }

            // objc2-foundation 0.3 yields Retained<T> from NSArray::iter, not &T.
            for app in workspace.runningApplications().iter() {
                self.register(&app, false);
            }
        }

        info!("Tracking {:?} applications", self.observers.len());
    }

    fn deregister_all(&mut self) {
        self.observers.clear();
    }
}

/// The `AXObserverCallback` registered by [`WindowServerInner::register`].
///
/// # Safety
///
/// HIServices supplies live element/notification references and a nullable refcon. A non-null
/// refcon points to the observer's pinned AccessibilityCallbackData until teardown removes the
/// source. The callback only borrows it immutably and never unwinds into C.
unsafe extern "C" fn application_ax_callback(
    _observer: AXObserverRef,
    _element: AXUIElementRef,
    notification_name: CFStringRef,
    refcon: *mut c_void,
) {
    if refcon.is_null() {
        error!("refcon must not be null");
        return;
    }

    // SAFETY: AXObserver pins this data until the source is unregistered on this run loop.
    // Callbacks only read it; reentrant delivery does not create exclusive aliases.
    let cb_data = unsafe { &*refcon.cast::<AccessibilityCallbackData>() };

    // SAFETY: HIServices supplies a live, non-null notification name. Get-rule wrapping
    // retains it for this callback; dropping `name` balances that retain.
    let name = unsafe { CFString::wrap_under_get_rule(notification_name) };
    let app = &cb_data.app;

    let event_name = name.to_string();
    trace!(app = %app.bundle_id, %event_name, "AX notification");

    let event = match &*event_name {
        kAXFocusedWindowChangedNotification
        | kAXMainWindowChangedNotification
        | kAXApplicationActivatedNotification
        | kAXApplicationShownNotification
        | kAXWindowCreatedNotification => Some(WindowServerEvent::FocusRequested { app: app.clone() }),
        kAXWindowResizedNotification | kAXWindowMovedNotification => {
            Some(WindowServerEvent::RequestCaretPositionUpdate)
        },
        // A destroyed element cannot reliably answer AX queries. Validate the current window
        // asynchronously; latest-only coalescing handles bursts of non-window destruction.
        kAXUIElementDestroyedNotification => Some(WindowServerEvent::WindowDestroyed { app: app.clone() }),

        unknown => {
            info!("Unhandled AX event: {unknown}");
            None
        },
    };

    if let Some(event) = event
        && let Err(e) = cb_data.sender.send(event)
    {
        warn!("Error sending focus changed event: {e:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(pid: pid_t, bundle_id: &str) -> ApplicationSpecifier {
        ApplicationSpecifier {
            pid,
            bundle_id: bundle_id.to_owned(),
        }
    }

    /// Two instances of the same app: quitting one must not deregister the other. The old
    /// `deregister(bundle_id)` matched on the bundle id alone and took both.
    #[test]
    fn only_the_terminated_instance_is_deregistered() {
        let tracked = vec![key(1, "com.microsoft.VSCode"), key(2, "com.microsoft.VSCode")];

        let stale = keys_to_deregister(tracked.clone(), "com.microsoft.VSCode", |pid| pid == 2);

        assert_eq!(stale, vec![key(1, "com.microsoft.VSCode")]);

        let mut remaining = tracked;
        remaining.retain(|k| !stale.contains(k));
        assert_eq!(remaining, vec![key(2, "com.microsoft.VSCode")]);
    }

    /// The last instance going away takes its observer with it.
    #[test]
    fn the_last_instance_is_deregistered() {
        let tracked = vec![key(1, "com.apple.Terminal")];
        assert_eq!(keys_to_deregister(tracked, "com.apple.Terminal", |_| false), vec![key(
            1,
            "com.apple.Terminal"
        )]);
    }

    /// Other applications are never touched, however they are keyed.
    #[test]
    fn other_bundle_ids_are_untouched() {
        let tracked = vec![key(1, "com.apple.Terminal"), key(2, "com.googlecode.iterm2")];
        assert_eq!(keys_to_deregister(tracked, "com.apple.Terminal", |_| false), vec![key(
            1,
            "com.apple.Terminal"
        )]);
    }
}
