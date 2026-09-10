#![allow(non_upper_case_globals)]

mod ax_observer;
mod ui_element;
use std::boxed::Box;
use std::ffi::c_void;
use std::hash::Hash;
use std::pin::Pin;

use accessibility_sys::{
    AXError,
    AXIsProcessTrusted,
    AXObserverRef,
    AXUIElementCreateApplication,
    AXUIElementRef,
    kAXApplicationActivatedNotification,
    kAXApplicationShownNotification,
    kAXFocusedWindowChangedNotification,
    kAXMainWindowChangedNotification,
    kAXUIElementDestroyedNotification,
    kAXWindowCreatedNotification,
    kAXWindowMovedNotification,
    kAXWindowResizedNotification,
    pid_t,
};
use ax_observer::AXObserver;
use core_foundation::base::TCFType;
use core_foundation::string::{
    CFString,
    CFStringRef,
};
use dashmap::DashMap;
use flume::Sender;
use objc2::rc::{
    Allocated,
    Retained,
};
use objc2::runtime::{
    AnyObject,
    NSObjectProtocol,
};
use objc2::{
    AllocAnyThread,
    DefinedClass,
    define_class,
    msg_send,
    sel,
};
use objc2_app_kit::{
    NSApplicationActivationPolicy,
    NSRunningApplication,
    NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification,
    NSWorkspaceDidActivateApplicationNotification,
    NSWorkspaceDidLaunchApplicationNotification,
    NSWorkspaceDidTerminateApplicationNotification,
};
use objc2_foundation::{
    NSNotification,
    NSObject,
};
use tracing::{
    debug,
    error,
    info,
    trace,
    warn,
};
pub use ui_element::{
    CGWindowLevelForKey,
    UIElement,
};

use crate::util::NotificationCenter;
use crate::util::notification_center::get_app_from_notification;

/// System processes that never host a shell. Our own bundle id is *not* here: it is
/// injected through [`WindowServer::new`] because this crate cannot depend on
/// `fig_util` (`fig_util` already depends on this crate on macOS).
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

pub enum WindowServerEvent {
    FocusChanged {
        window: UIElement,
        app: ApplicationSpecifier,
    },
    WindowDestroyed {
        app: ApplicationSpecifier,
    },
    ActiveSpaceChanged {
        is_fullscreen: bool,
    },
    RequestCaretPositionUpdate,
}

pub struct AccessibilityCallbackData {
    pub app: ApplicationSpecifier,
    pub sender: Sender<WindowServerEvent>,
}

unsafe fn app_bundle_id(app: &NSRunningApplication) -> Option<String> {
    app.bundleIdentifier().map(|s| s.to_string())
}

pub struct WindowServer {
    _inner: Pin<Box<WindowServerInner>>,
    observer: Retained<ObserverClass>,
}

// SAFETY: observer id pointer is send + sync safe
unsafe impl Send for WindowServer {}
unsafe impl Sync for WindowServer {}

pub struct WindowServerInner {
    observers: DashMap<ApplicationSpecifier, AXObserver<AccessibilityCallbackData>, fnv::FnvBuildHasher>,
    sender: Sender<WindowServerEvent>,
    /// The bundle id of the process we are running in, so the app never observes its own
    /// autocomplete window.
    own_bundle_id: &'static str,
    /// Bundle ids of terminals that render their UI in a webview and need
    /// `AXManualAccessibility` switched on before the caret can be read.
    xterm_bundle_ids: &'static [&'static str],
}

impl WindowServer {
    /// `own_bundle_id` and `xterm_bundle_ids` are injected rather than read from
    /// `fig_util::consts` / `fig_util::Terminal`: `fig_util` depends on this crate on
    /// macOS, so depending back on it would be a cycle.
    pub fn new(
        sender: Sender<WindowServerEvent>,
        own_bundle_id: &'static str,
        xterm_bundle_ids: &'static [&'static str],
    ) -> Self {
        let (mut inner, observer) = WindowServerInner::new_with_observer(sender, own_bundle_id, xterm_bundle_ids);

        let mut center = NotificationCenter::workspace_center();

        // Previously (in Swift) subscribed to the following as no-ops / log only:
        // - NSWorkspaceDidDeactivateApplicationNotification
        unsafe {
            center.subscribe_with_observer(
                NSWorkspaceActiveSpaceDidChangeNotification,
                &observer,
                sel!(activeSpaceChanged:),
            );

            center.subscribe_with_observer(
                NSWorkspaceDidLaunchApplicationNotification,
                &observer,
                sel!(didLaunchApplication:),
            );

            center.subscribe_with_observer(
                NSWorkspaceDidTerminateApplicationNotification,
                &observer,
                sel!(didTerminateApplication:),
            );

            center.subscribe_with_observer(
                NSWorkspaceDidActivateApplicationNotification,
                &observer,
                sel!(didActivateApplication:),
            );
        }

        inner.init();
        Self {
            _inner: inner,
            observer,
        }
    }
}

impl Drop for WindowServer {
    fn drop(&mut self) {
        let center = NotificationCenter::workspace_center();
        unsafe {
            center.remove_observer(&self.observer);
        }
    }
}

trait WindowServerHandler {
    fn did_activate_application(&mut self, notif: &NSNotification);
    fn active_space_changed(&mut self, notif: &NSNotification);
    fn did_terminate_application(&mut self, notif: &NSNotification);
    fn did_launch_application(&mut self, notif: &NSNotification);
}

pub struct Ivars {
    handler: *mut c_void,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `ObserverClass` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    // The name is referenced from the notification-center registrations below, so it is pinned
    // rather than left to objc2's module-path-plus-version default.
    #[name = "Fig_WindowServerObserver"]
    #[ivars = Ivars]
    pub struct ObserverClass;

    impl ObserverClass {
        #[unsafe(method_id(initWithHandler:))]
        fn init_with_handler(this: Allocated<Self>, handler: *mut c_void) -> Option<Retained<Self>> {
            let this = this.set_ivars(Ivars {
                handler
            });
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(didActivateApplication:))]
        fn did_activate_application(&self, notif: &NSNotification) {
            let inner = self.ivars().handler as *mut WindowServerInner;
            let inner = unsafe { &mut *inner };
            inner.did_activate_application(notif);
        }

        #[unsafe(method(activeSpaceChanged:))]
        fn active_space_changed(&self, notif: &NSNotification) {
            let inner = self.ivars().handler as *mut WindowServerInner;
            let inner = unsafe { &mut *inner };
            inner.active_space_changed(notif);
        }

        #[unsafe(method(didTerminateApplication:))]
        fn did_terminate_application(&self, notif: &NSNotification) {
            let inner = self.ivars().handler as *mut WindowServerInner;
            let inner = unsafe { &mut *inner };
            inner.did_terminate_application(notif);
        }

        #[unsafe(method(didLaunchApplication:))]
        fn did_launch_application(&self, notif: &NSNotification) {
            let inner = self.ivars().handler as *mut WindowServerInner;
            let inner = unsafe { &mut *inner };
            inner.did_launch_application(notif);
        }
    }

    unsafe impl NSObjectProtocol for ObserverClass {}
);

impl ObserverClass {
    pub fn new(handler: *mut c_void) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(), initWithHandler:handler] }
    }
}

impl WindowServerInner {
    pub fn new_with_observer(
        sender: Sender<WindowServerEvent>,
        own_bundle_id: &'static str,
        xterm_bundle_ids: &'static [&'static str],
    ) -> (Pin<Box<Self>>, Retained<ObserverClass>) {
        let pin = Box::pin(Self {
            observers: Default::default(),
            sender,
            own_bundle_id,
            xterm_bundle_ids,
        });
        let handler = &*pin as *const Self as *mut c_void;
        let r = ObserverClass::new(handler);
        (pin, r)
    }

    #[allow(clippy::missing_safety_doc)]
    unsafe fn register(&mut self, ns_app: &NSRunningApplication, from_activation: bool) {
        if !AXIsProcessTrusted() {
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

        let ax_ref = AXUIElementCreateApplication(pid);

        if bundle_id == self.own_bundle_id || BLOCKED_BUNDLE_IDS.contains(&bundle_id.as_str()) {
            debug!("Ignoring bundle id {:?}", bundle_id);
            return;
        }

        if ns_app.activationPolicy() == NSApplicationActivationPolicy::Prohibited {
            debug!("Ignoring application by activation policy");
            return;
        }

        if self.observers.contains_key(&key) {
            debug!("app {} is already registered", key.bundle_id);
            self.deregister(&key.bundle_id)
        }

        if from_activation {
            // In Swift had 0.25s delay before this...?
            let elem = UIElement::from(ax_ref);
            let sender = self.sender.clone();
            let app = key.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                if let Ok(window) = elem.focused_window()
                    && let Err(e) = sender.send(WindowServerEvent::FocusChanged { window, app })
                {
                    warn!("Error sending focus changed event: {e:?}");
                };
            });
        }

        if self.xterm_bundle_ids.contains(&key.bundle_id.as_str()) {
            UIElement::from(ax_ref).enable_screen_reader_accessibility().ok();
        }

        let bundle_id = key.bundle_id.as_str();
        if let Ok(mut observer) = AXObserver::create(
            key.pid,
            ax_ref,
            AccessibilityCallbackData {
                app: key.clone(),
                sender: self.sender.clone(),
            },
            application_ax_callback,
        ) {
            let result: Result<Vec<_>, AXError> = TRACKED_NOTIFICATIONS
                .iter()
                .map(|notification| observer.subscribe(notification))
                .collect();

            if result.is_ok() {
                debug!("Began tracking {bundle_id:?}");

                self.observers.insert(key, observer);
                return;
            }
        }

        warn!("Error setting up tracking for '{bundle_id:?}'");
    }

    fn deregister(&mut self, bundle_id: &str) {
        self.observers.retain(|key, _| bundle_id != key.bundle_id);
    }

    fn register_all(&mut self) {
        self.deregister_all();

        unsafe {
            let workspace = NSWorkspace::sharedWorkspace();
            if let Some(app) = workspace.frontmostApplication() {
                self.register(&app, true);
            }

            // objc2-foundation 0.3 yields Retained<T> from NSArray::iter, not &T.
            for app in workspace.runningApplications().iter() {
                self.register(&app, false)
            }
        }

        info!("Tracking {:?} applications", self.observers.len());
    }

    pub fn init(&mut self) {
        self.register_all();
    }

    fn deregister_all(&mut self) {
        self.observers.clear();
    }
}

impl WindowServerHandler for WindowServerInner {
    fn did_activate_application(&mut self, notif: &NSNotification) {
        unsafe {
            if let Some(app) = get_app_from_notification(notif) {
                let bundle_id = app_bundle_id(&app);
                trace!("Activated application {bundle_id:?}");
                self.register(&app, true);
            }
        }
    }

    fn active_space_changed(&mut self, notif: &NSNotification) {
        unsafe {
            let Some(object) = notif.object() else { return };
            let workspace: Retained<NSWorkspace> = Retained::<AnyObject>::cast(object);
            let Some(app) = workspace.frontmostApplication() else {
                return;
            };
            let pid = app.processIdentifier();
            let ax_app = AXUIElementCreateApplication(pid);
            let app_elem: UIElement = ax_app.into();
            if let Ok(window) = app_elem.focused_window() {
                let fullscreen = window.is_fullscreen();
                if let Ok(is_fullscreen) = fullscreen
                    && let Err(e) = self
                        .sender
                        .send(WindowServerEvent::ActiveSpaceChanged { is_fullscreen })
                {
                    warn!("Error sending active space changed notif: {e:?}");
                }
            }
        }
    }

    fn did_terminate_application(&mut self, notif: &NSNotification) {
        unsafe {
            if let Some(ns_app) = get_app_from_notification(notif)
                && let Some(bundle_id) = app_bundle_id(&ns_app)
            {
                trace!("Terminated application - {bundle_id:?}");

                let apps = NSWorkspace::sharedWorkspace().runningApplications();

                let has_running = apps
                    .iter()
                    .any(|running| app_bundle_id(&running).map(|id| id == bundle_id).unwrap_or(false));

                if !has_running {
                    trace!("Deregistering app {bundle_id:?} since no other instances are running");
                    self.deregister(bundle_id.as_str());
                }
            }
        }
    }

    fn did_launch_application(&mut self, notif: &NSNotification) {
        unsafe {
            if let Some(app) = get_app_from_notification(notif) {
                let bundle_id = app_bundle_id(&app);
                trace!("Launched application - {bundle_id:?}");
                self.register(&app, true)
            }
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn application_ax_callback(
    _observer: AXObserverRef,
    element: AXUIElementRef,
    notification_name: CFStringRef,
    refcon: *mut c_void,
) {
    if refcon.is_null() {
        error!("refcon must not be null");
        return;
    }

    let cb_data: &mut AccessibilityCallbackData = &mut *(refcon as *mut AccessibilityCallbackData);
    // get_rule will call CFRetain to increment the RC in objc to make sure element is not freed
    // before we are done with it. CFRelease is called automatically on drop.
    let element = UIElement::from(element);

    let name = CFString::wrap_under_get_rule(notification_name);
    let app = &cb_data.app;

    let event_name = name.to_string();

    let event = match &*event_name {
        kAXFocusedWindowChangedNotification | kAXMainWindowChangedNotification => {
            Some(WindowServerEvent::FocusChanged {
                window: element,
                app: app.clone(),
            })
        },
        kAXApplicationActivatedNotification | kAXApplicationShownNotification => {
            element
                .focused_window()
                .ok()
                .map(|window| WindowServerEvent::FocusChanged {
                    window,
                    app: app.clone(),
                })
        },
        kAXWindowResizedNotification | kAXWindowMovedNotification => {
            Some(WindowServerEvent::RequestCaretPositionUpdate)
        },
        kAXUIElementDestroyedNotification => {
            // We check to see if there is a valid window for the app, if there is not then we know the final
            // window has been destroyed. This is done via getting an error when trying to get the focused
            // window.
            let ax_app_ref = UIElement::from(AXUIElementCreateApplication(app.pid));
            if ax_app_ref.focused_window().is_err() {
                Some(WindowServerEvent::WindowDestroyed { app: app.clone() })
            } else {
                None
            }
        },

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
