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
    kAXWindowRole,
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
use dashmap::DashMap;
use flume::Sender;
use objc2::rc::{
    Allocated,
    Retained,
};
use objc2::runtime::NSObjectProtocol;
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

pub struct WindowServer {
    _inner: Pin<Box<WindowServerInner>>,
    observer: Retained<ObserverClass>,
}

// SAFETY: Requires creation, mutation and drop on the owning main run loop. PlatformState
// stores this behind a mutex, but only main-loop event handlers access it. The mutex alone
// does not enforce thread affinity: off-thread drop could race a callback using the inner
// state. Move ownership into main-loop-only state to encode this invariant in the type system.
// See docs/architecture.md (native thread ownership).
unsafe impl Send for WindowServer {}

impl std::fmt::Debug for WindowServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowServer").finish_non_exhaustive()
    }
}

pub struct WindowServerInner {
    observers: DashMap<ApplicationSpecifier, AXObserver<AccessibilityCallbackData>, fnv::FnvBuildHasher>,
    sender: Sender<WindowServerEvent>,
    /// Captured once at construction so the ObjC selectors never depend on a runtime
    /// thread-local; see the note in `register`.
    handle: tokio::runtime::Handle,
    /// The bundle id of the process we are running in, so the app never observes its own
    /// autocomplete window.
    own_bundle_id: &'static str,
    /// Bundle ids of terminals that render their UI in a webview and need
    /// `AXManualAccessibility` switched on before the caret can be read.
    xterm_bundle_ids: &'static [&'static str],
}

impl WindowServer {
    /// `own_bundle_id` and `xterm_bundle_ids` are injected rather than read from
    /// `fig_util::consts` / `fig_util::Terminal` so this crate keeps no dependency on `fig_util`
    /// and both values stay injectable in tests. (Not, as the previous comment claimed, because
    /// `fig_util` depends on this crate — it does not.)
    pub fn new(
        sender: Sender<WindowServerEvent>,
        own_bundle_id: &'static str,
        xterm_bundle_ids: &'static [&'static str],
    ) -> Self {
        let (mut inner, observer) = WindowServerInner::new_with_observer(sender, own_bundle_id, xterm_bundle_ids);

        let mut center = NotificationCenter::workspace_center();

        // Previously (in Swift) subscribed to the following as no-ops / log only:
        // - NSWorkspaceDidDeactivateApplicationNotification
        //
        // SAFETY:
        // Operations, four times over: (a) reading the extern statics
        // `NSWorkspace*Notification` (objc2-app-kit declares them in an `unsafe extern`
        // block), and (b) calling `NotificationCenter::subscribe_with_observer`, an unsafe fn.
        // Contract of `subscribe_with_observer` (see its `# Safety` section): `observer` must
        // stay alive and registered until `remove_observer`, and it must actually implement
        // the selector, with the ObjC signature `-(void)sel:(NSNotification *)notif`.
        // Evidence:
        // - The four statics are initialized by AppKit before `main` and are never written by this
        //   program, so reading them is not a data race and each is a live `NSNotificationName`.
        // - `observer` is a `Retained<ObserverClass>` that this function moves into the returned
        //   `WindowServer`, and `WindowServer::drop` calls `remove_observer` on the same workspace
        //   center, so the registration never outlives the object.
        // - `ObserverClass` defines exactly `activeSpaceChanged:`, `didLaunchApplication:`,
        //   `didTerminateApplication:` and `didActivateApplication:` in the `define_class!` block
        //   below, each `fn(&self, &NSNotification)` — the required IMP signature. The `sel!` macro
        //   produces those same names, checked here by inspection because ObjC selector dispatch is
        //   not type-checked by rustc.
        // - `NSNotificationCenter` is documented thread-safe, so registering here (main thread) is
        //   sound regardless of which thread posts.
        // Postcondition: AppKit holds an unretained reference to `observer`; the
        // `WindowServer` value returned below is what keeps it alive.
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

    /// Drop every AX observer and register them again from the current running-application list.
    ///
    /// Registration otherwise only happens inside [`WindowServer::new`], which runs once at
    /// startup — so granting accessibility permission while the app is running left it tracking
    /// zero applications until the next restart, even though the UI reported autocomplete as
    /// enabled.
    ///
    /// # Safety
    ///
    /// Same clause as [`WindowServerInner::register`]: must be called on the thread whose run
    /// loop the AX sources are attached to, i.e. the tao main thread.
    pub unsafe fn reregister_all(&mut self) {
        // SAFETY: the caller has discharged the run-loop affinity clause; the callback argument
        // is fixed inside `register` itself.
        unsafe { self._inner.as_mut().get_unchecked_mut() }.register_all();
    }

    /// Register the frontmost application if it is not tracked yet and, when that succeeds,
    /// publish its focused window. This is the retry path for a registration that failed at
    /// activation time (see `register`): the caller runs it on every edit-buffer update, i.e.
    /// at exactly the moments the terminal the user is typing in is known to be responsive.
    /// Costs one `frontmostApplication` call and a map lookup when the app is already tracked.
    ///
    /// # Safety
    ///
    /// Same as [`Self::reregister_all`]: must run on the thread owning the run loop the AX
    /// sources are attached to.
    pub unsafe fn ensure_frontmost_tracked(&mut self) {
        // SAFETY: the caller has discharged the run-loop affinity clause; the callback argument
        // is fixed inside `register` itself.
        unsafe { self._inner.as_mut().get_unchecked_mut() }.ensure_frontmost_tracked();
    }
}

impl Drop for WindowServer {
    fn drop(&mut self) {
        let center = NotificationCenter::workspace_center();
        // SAFETY:
        // Operation: `NotificationCenter::remove_observer`, an unsafe fn.
        // Contract (see its `# Safety` section): `observer` must be a live object that was
        // previously registered on this center, or not registered at all (AppKit documents
        // `-removeObserver:` as a no-op in that case).
        // Evidence:
        // - `self.observer` is alive: we are inside `WindowServer::drop`, so the `Retained` field
        //   has not been released yet, and `Retained` guarantees a `+1` on it.
        // - It was registered on `NSWorkspace.sharedWorkspace.notificationCenter` in
        //   `WindowServer::new`; `workspace_center()` returns that same singleton (Apple documents
        //   `sharedWorkspace` as a process-wide singleton).
        // Postcondition: after this call AppKit no longer holds the unretained observer
        // pointer, so releasing `self.observer` immediately afterwards cannot leave AppKit
        // with a dangling reference. This ordering is exactly why the removal lives in `Drop`.
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
    // Safety invariant: `handler` is a `*mut WindowServerInner` derived from a *mutable*
    // borrow of the `Pin<Box<WindowServerInner>>` that the owning `WindowServer` keeps in its
    // `_inner` field (see `WindowServerInner::new_with_observer`). It is therefore:
    //   - non-null, aligned for `WindowServerInner`, and pointing at an initialized value;
    //   - address-stable, because the box is pinned and `_inner` is never moved out of or
    //     reassigned;
    //   - live for at least as long as the `Retained<ObserverClass>` stored next to it in
    //     `WindowServer`, because `WindowServer::drop` unregisters the observer from the
    //     notification center *before* dropping either field, so no selector can run after the box
    //     is freed;
    //   - valid for writes, i.e. `&mut *handler` does not launder a shared-reference provenance
    //     into a mutable one.
    //
    // Aliasing obligation carried by every user: the selectors below each form
    // `&mut WindowServerInner` from it. They may do so only if no other live reference to the
    // same `WindowServerInner` exists at that moment. See `MODULE INVARIANT (single-borrow)`
    // documented on `WindowServerInner::new_with_observer`.
    handler: *mut c_void,
}

define_class!(
    // SAFETY:
    // Operation: `define_class!` with `#[unsafe(super(NSObject))]`, plus the four
    // `#[unsafe(method(..))]` / `#[unsafe(method_id(..))]` attributes below.
    // Contract (objc2 0.6 `define_class!` docs):
    // 1. The superclass must have no additional subclassing requirements. `NSObject` has none.
    // 2. The type must not implement `Drop` (objc2 cannot run it from `-dealloc`).
    //    `ObserverClass` is a unit struct with an `Ivars` of `Copy` fields and no `Drop` impl,
    //    so there is nothing to run.
    // 3. Each `#[unsafe(method(sel))]` asserts that the Rust fn signature matches the ObjC
    //    signature the runtime will dispatch with. The four notification selectors are
    //    `fn(&self, &NSNotification)`, matching AppKit's
    //    `-(void)name:(NSNotification *)notification` callback shape; `initWithHandler:` is
    //    `fn(Allocated<Self>, *mut c_void) -> Option<Retained<Self>>`, matching an `init`
    //    family method taking one pointer-sized argument.
    // 4. `#[unsafe(method_id(initWithHandler:))]` asserts the selector belongs to the `init`
    //    method family, i.e. it consumes the `Allocated` receiver and returns `+1`. The name
    //    starts with `init`, so the runtime applies exactly that ownership rule.
    // 5. The runtime class name must be unique in the process. See the `#[name]` note below.
    #[unsafe(super(NSObject))]
    // The name is referenced from the notification-center registrations below, so it is pinned
    // rather than left to objc2's module-path-plus-version default. Uniqueness premise for
    // obligation 5 above: the `Fig_` prefix is not used by AppKit, WebKit, tao or wry, and this
    // crate registers the class exactly once (objc2 registers on first use of `ObserverClass`).
    #[name = "Fig_WindowServerObserver"]
    #[ivars = Ivars]
    pub struct ObserverClass;

    impl ObserverClass {
        #[unsafe(method_id(initWithHandler:))]
        fn init_with_handler(this: Allocated<Self>, handler: *mut c_void) -> Option<Retained<Self>> {
            let this = this.set_ivars(Ivars {
                handler
            });
            // SAFETY: NSObject initialization consumes this fresh allocated receiver.
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(didActivateApplication:))]
        fn did_activate_application(&self, notif: &NSNotification) {
            let inner = self.ivars().handler.cast::<WindowServerInner>();
            // SAFETY: new_with_observer pins the live handler allocation with mutable
            // provenance. Main-run-loop selectors are serialized with owner access and drop,
            // so this borrow is exclusive and ends before returning to AppKit. The observer
            // is unregistered before its handler is dropped (Ivars::handler invariant).
            let inner = unsafe { &mut *inner };
            inner.did_activate_application(notif);
        }

        #[unsafe(method(activeSpaceChanged:))]
        fn active_space_changed(&self, notif: &NSNotification) {
            let inner = self.ivars().handler.cast::<WindowServerInner>();
            // SAFETY: new_with_observer pins the live handler allocation with mutable
            // provenance. Main-run-loop selectors are serialized with owner access and drop,
            // so this borrow is exclusive and ends before returning to AppKit. The observer
            // is unregistered before its handler is dropped (Ivars::handler invariant).
            let inner = unsafe { &mut *inner };
            inner.active_space_changed(notif);
        }

        #[unsafe(method(didTerminateApplication:))]
        fn did_terminate_application(&self, notif: &NSNotification) {
            let inner = self.ivars().handler.cast::<WindowServerInner>();
            // SAFETY: new_with_observer pins the live handler allocation with mutable
            // provenance. Main-run-loop selectors are serialized with owner access and drop,
            // so this borrow is exclusive and ends before returning to AppKit. The observer
            // is unregistered before its handler is dropped (Ivars::handler invariant).
            let inner = unsafe { &mut *inner };
            inner.did_terminate_application(notif);
        }

        #[unsafe(method(didLaunchApplication:))]
        fn did_launch_application(&self, notif: &NSNotification) {
            let inner = self.ivars().handler.cast::<WindowServerInner>();
            // SAFETY: new_with_observer pins the live handler allocation with mutable
            // provenance. Main-run-loop selectors are serialized with owner access and drop,
            // so this borrow is exclusive and ends before returning to AppKit. The observer
            // is unregistered before its handler is dropped (Ivars::handler invariant).
            let inner = unsafe { &mut *inner };
            inner.did_launch_application(notif);
        }
    }

    unsafe impl NSObjectProtocol for ObserverClass {}
);

impl ObserverClass {
    /// # Safety
    /// `handler` must satisfy the pinned lifetime and exclusive callback access invariants
    /// on `Ivars::handler`; keep it alive until the observer is unregistered and dropped.
    unsafe fn new(handler: *mut c_void) -> Retained<Self> {
        // SAFETY:
        // Operation: `msg_send![Self::alloc(), initWithHandler:handler]`.
        // Contract (objc2 0.6 `msg_send!`): the selector must exist on the receiver's class,
        // the argument types must match the ObjC signature, and — because `initWithHandler:`
        // is in the `init` family — the receiver must be a fresh `Allocated<Self>` that is
        // consumed by the call, with the result owned at `+1`.
        // Evidence:
        // - `Self::alloc()` (`AllocAnyThread`) returns exactly `Allocated<ObserverClass>`. objc2
        //   0.6 requires `AllocAnyThread` to be implemented, which `define_class!` does for a
        //   non-`MainThreadOnly` class; allocation itself is thread-agnostic.
        // - `initWithHandler:` is defined immediately above with
        //   `#[unsafe(method_id(initWithHandler:))]` and signature `(Allocated<Self>, *mut c_void)
        //   -> Option<Retained<Self>>`, so both the selector and the argument type match, and
        //   `Retained<Self>` is a valid return type because `-[NSObject init]` never returns nil
        //   for a successful allocation.
        // Postcondition: a `+1` `ObserverClass` whose `handler` ivar is `handler`.
        unsafe { msg_send![Self::alloc(), initWithHandler:handler] }
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
    /// Allocate the pinned inner state and the Objective-C observer that points at it.
    ///
    /// # MODULE INVARIANT (single-borrow)
    ///
    /// The returned `Pin<Box<Self>>` and the returned observer alias the same
    /// `WindowServerInner`. The alias is only sound because callers keep to this rule:
    ///
    /// - `WindowServer::new` is the only caller. It calls `inner.init()` once, *before* storing the
    ///   box in the private `WindowServer::_inner` field.
    /// - The one other borrow of `_inner` is `WindowServer::reregister_all`, which runs on the same
    ///   (main) thread, from inside the tao event loop's `PlatformBoundEvent` dispatch.
    /// - Every other `&mut WindowServerInner` is created inside an `ObserverClass` selector from
    ///   the raw ivar, and those selectors are delivered serially by the main run loop.
    /// - `register_all` performs no run-loop turn (it only calls AppKit accessors and
    ///   `Handle::spawn`), so no selector can fire while either of those `&mut`s is live.
    ///
    /// Consequently at most one `&mut WindowServerInner` exists at any program point.
    fn new_with_observer(
        sender: Sender<WindowServerEvent>,
        own_bundle_id: &'static str,
        xterm_bundle_ids: &'static [&'static str],
    ) -> (Pin<Box<Self>>, Retained<ObserverClass>) {
        let mut pin = Box::pin(Self {
            observers: Default::default(),
            sender,
            // Panics outside a runtime — loudly, at construction, instead of aborting later
            // inside an Objective-C callback.
            handle: tokio::runtime::Handle::current(),
            own_bundle_id,
            xterm_bundle_ids,
        });
        // SAFETY:
        // Operation: `Pin::get_unchecked_mut(pin.as_mut())`.
        // Contract (std `Pin::get_unchecked_mut`): the caller must never move the pointee out
        // of the returned `&mut Self`.
        // Evidence: the reference is immediately decayed to a raw pointer; no `Self` value is
        // read, written or moved through it here, and the box is thereafter only reachable
        // through `WindowServer::_inner`, which is private, never reassigned, and only dropped
        // in place.
        // Why not `&*pin as *const Self as *mut c_void` (the previous form): the ObjC
        // selectors reconstruct this pointer as `&mut WindowServerInner`. The Rust Reference's
        // aliasing rules forbid writing through a pointer whose provenance was derived from a
        // shared reference, so the mutable borrow has to happen here, at the derivation point.
        // See docs/architecture.md (native thread ownership).
        // Postcondition: `handler` satisfies the `Ivars::handler` safety invariant.
        let handler = std::ptr::from_mut(unsafe { Pin::get_unchecked_mut(pin.as_mut()) }).cast::<c_void>();
        // SAFETY: handler points into pin, whose owner unregisters the observer before drop.
        let r = unsafe { ObserverClass::new(handler) };
        (pin, r)
    }

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
        let owned_ax_ref = OwnedAxRef(AXUIElementCreateApplication(pid));
        let ax_ref = owned_ax_ref.0;

        if from_activation {
            // Preserve the activation grace period before querying the application window.
            let elem = UIElement::from_borrowed(ax_ref);
            let sender = self.sender.clone();
            let app = key.clone();
            // A stored `Handle`, not `tokio::spawn`: this runs inside an `objc2 define_class!`
            // selector, where a panic is an `abort`. `tokio::spawn` relies on a runtime
            // thread-local that happens to be set today only because `event_loop.run()` is
            // called from inside `#[tokio::main]`'s future.
            self.handle.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                if let Ok(window) = elem.focused_window()
                    && let Err(e) = sender.send(WindowServerEvent::FocusChanged { window, app })
                {
                    warn!("Error sending focus changed event: {e:?}");
                };
            });
        }

        if already_registered {
            debug!("app {} is already registered", key.bundle_id);
            return;
        }

        if self.xterm_bundle_ids.contains(&key.bundle_id.as_str()) {
            UIElement::from_borrowed(ax_ref)
                .enable_screen_reader_accessibility()
                .ok();
        }

        let bundle_id = key.bundle_id.as_str();
        // SAFETY: owned_ax_ref keeps the AX element live during create, which retains it.
        // Callback data has exactly the type application_ax_callback expects. Registration,
        // callbacks and teardown share the owning run loop; subscribe receives live elements.
        let mut observer = match AXObserver::create(
            key.pid,
            ax_ref,
            AccessibilityCallbackData {
                app: key.clone(),
                sender: self.sender.clone(),
            },
            application_ax_callback,
        ) {
            Ok(observer) => observer,
            Err(err) => {
                warn!(
                    bundle_id,
                    err = %ax_error_name(err),
                    "AXObserverCreate failed; tracking is retried on the next edit-buffer update"
                );
                return;
            },
        };

        // One failed subscription used to discard the whole observer, silently (the warning
        // carried no error code), and nothing tried again except the next
        // `didActivateApplication`. With the process-wide 0.25 s messaging timeout set in
        // `fig_desktop`'s `Initialize`, a terminal busy repainting for a quarter second at the
        // moment it was activated answered `kAXErrorCannotComplete` and then stayed untracked
        // for the rest of the session: no `WindowDestroyed`, no focused window for the frame
        // watch (2026-09-17, Terminal.app under heavy output, the terminal focus recovery path).
        // Now a notification the target does not support is skipped, any other failure is
        // logged with its code, and the app is left unregistered so `ensure_frontmost_tracked`
        // retries it on use.
        let mut failed: Vec<(&'static str, String)> = Vec::new();
        for notification in TRACKED_NOTIFICATIONS {
            match observer.subscribe(notification) {
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
            "Error setting up tracking; retried on the next edit-buffer update"
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

        // `register(_, false)` does not publish the focused window, and the activation-time
        // lookup failed for the same reason the registration did, so do it here: without it
        // `focused_window` stays `None` and the frame watch has nothing to follow.
        //
        // SAFETY:
        // Operation: the `AXUIElementCreateApplication` FFI call.
        // Contract (HIServices): takes a `pid_t`, returns a `+1` `AXUIElementRef` (Create rule),
        // no pointer preconditions.
        // Postcondition: `owned` releases that `+1` at the end of this scope;
        // `UIElement::from_borrowed` follows the Get rule and retains independently, so `elem`
        // outlives `owned` safely.
        let owned = OwnedAxRef(unsafe { AXUIElementCreateApplication(key.pid) });
        let elem = UIElement::from_borrowed(owned.0);
        match elem.focused_window() {
            Ok(window) => {
                if let Err(e) = self.sender.send(WindowServerEvent::FocusChanged { window, app: key }) {
                    warn!("Error sending focus changed event: {e:?}");
                }
            },
            Err(err) => debug!(err = %ax_error_name(err), "focused window still unavailable after recovery"),
        }
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

        // SAFETY:
        // Operations: two calls to `Self::register`, an unsafe fn.
        // Contract (see `register`'s `# Safety`): run-loop affinity, and
        // `application_ax_callback` as the registered callback.
        // Evidence: `register_all` is reached only from `init()`, which `WindowServer::new`
        // calls, and `WindowServer::new` is called from
        // `fig_desktop::platform::macos::PlatformStateImpl::handle` while it is servicing
        // `PlatformBoundEvent::Initialize` on the tao event loop, i.e. the main thread. The
        // callback argument is fixed in `register` itself, so clause 2 is discharged there.
        // `NSWorkspace::sharedWorkspace`, `frontmostApplication` and `runningApplications` are
        // safe fns in objc2-app-kit 0.3; they are inside this block only because `register` is.
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

    fn init(&mut self) {
        self.register_all();
    }

    fn deregister_all(&mut self) {
        self.observers.clear();
    }
}

impl WindowServerHandler for WindowServerInner {
    fn did_activate_application(&mut self, notif: &NSNotification) {
        // SAFETY: register runs on the main run-loop thread delivering NSWorkspace
        // notifications. get_app_from_notification performs a checked downcast.
        unsafe {
            if let Some(app) = get_app_from_notification(notif) {
                let bundle_id = app_bundle_id(&app);
                trace!("Activated application {bundle_id:?}");
                self.register(&app, true);
            }
        }
    }

    fn active_space_changed(&mut self, notif: &NSNotification) {
        // SAFETY: The notification object is checked with downcast. OwnedAxRef owns the
        // Create-rule AX application reference while from_borrowed takes a separate retain;
        // both references are released after the query.
        unsafe {
            let Some(object) = notif.object() else { return };
            let Ok(workspace) = object.downcast::<NSWorkspace>() else {
                return;
            };
            let Some(app) = workspace.frontmostApplication() else {
                return;
            };
            let pid = app.processIdentifier();
            let ax_app = OwnedAxRef(AXUIElementCreateApplication(pid));
            let app_elem = UIElement::from_borrowed(ax_app.0);
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
        if let Some(ns_app) = get_app_from_notification(notif)
            && let Some(bundle_id) = app_bundle_id(&ns_app)
        {
            trace!("Terminated application - {bundle_id:?}");

            // The notification's `NSRunningApplication` is already dead, and Apple
            // documents `processIdentifier` as -1 once that is the case, so the terminated
            // instance cannot be identified by pid. Ask instead which of the pids we track
            // for that bundle id are still alive — one lookup per tracked instance, rather
            // than re-enumerating all ~143 running applications and allocating a `String`
            // per app, which is what this used to do on every quit.
            let stale = keys_to_deregister(
                self.observers.iter().map(|entry| entry.key().clone()),
                &bundle_id,
                |pid| NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some(),
            );

            for key in stale {
                trace!("Deregistering {:?} (pid {})", key.bundle_id, key.pid);
                self.deregister_one(&key);
            }
        }
    }

    fn did_launch_application(&mut self, notif: &NSNotification) {
        // SAFETY: register runs on the main run-loop thread delivering NSWorkspace
        // notifications. get_app_from_notification performs a checked downcast.
        unsafe {
            if let Some(app) = get_app_from_notification(notif) {
                let bundle_id = app_bundle_id(&app);
                trace!("Launched application - {bundle_id:?}");
                self.register(&app, true);
            }
        }
    }
}

/// The `AXObserverCallback` registered by [`WindowServerInner::register`].
///
/// # Safety
///
/// HIServices, not Rust, calls this. The contract on *that* caller is:
///
/// 1. `refcon` is either null or a `*mut AccessibilityCallbackData` that is live, initialized,
///    aligned, and not concurrently referenced. This is guaranteed by `AXObserver::subscribe`,
///    which is the only registration site and derives the pointer from a mutable borrow of its own
///    pinned box.
/// 2. `element` is a live `AXUIElementRef` owned by the caller (Get rule: the callback must not
///    release it).
/// 3. `notification_name` is a live `CFStringRef` owned by the caller (Get rule).
/// 4. The call happens on the run loop the observer's source was attached to, so no other `&mut
///    AccessibilityCallbackData` derived from the same `refcon` is live.
/// 5. This function must not unwind into Objective-C. `extern "C"` functions abort on unwind since
///    Rust 1.81, so a panic here terminates the process rather than causing UB.
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

    // SAFETY:
    // Operations: the `as *mut AccessibilityCallbackData` cast and the creation of
    // `&mut AccessibilityCallbackData` from it.
    // Contract (Rust Reference, reference validity + aliasing): non-null, aligned,
    // initialized, in a live allocation, mutable-capable provenance, and exclusive for the
    // lifetime of the reference.
    // Evidence:
    // - Non-null: the `is_null` branch immediately above returned, and nothing between the check
    //   and here can change `refcon` (it is a by-value parameter).
    // - Alignment, initialization, liveness and provenance: `# Safety` clause 1 above, which
    //   `AXObserver::subscribe` discharges — it derives the pointer with `Pin::get_unchecked_mut`
    //   from a pinned `Box<AccessibilityCallbackData>` that stays alive until the run-loop source
    //   is removed in `AXObserver::drop`.
    // - Exclusivity: `# Safety` clause 4. Run-loop sources are serviced serially on one thread, and
    //   the only other path that touches the same allocation is `AXObserver::drop`, which removes
    //   the source before freeing the box.
    // Postcondition: the `&mut` lives to the end of this function; nothing in the body calls
    // back into `AXObserver` or the run loop, so it cannot be re-entered while it is live.
    let cb_data: &mut AccessibilityCallbackData = &mut *(refcon.cast::<AccessibilityCallbackData>());

    // SAFETY:
    // Operation: `UIElement::from_borrowed(AXUIElementRef)`, which performs
    // `AXUIElement::wrap_under_get_rule(element)`.
    // Contract (core-foundation 0.10 `TCFType::wrap_under_get_rule`): the reference must be
    // non-null (it is `assert!`ed, so a null would panic rather than be UB) and live; the call
    // performs `CFRetain` and the resulting wrapper `CFRelease`s on drop.
    // Evidence: `# Safety` clause 2 — HIServices passes a live element it still owns. Applying
    // the Get rule is therefore correct here: it adds our own reference for the duration of
    // this callback and gives it back on drop, without consuming the caller's.
    let element = UIElement::from_borrowed(element);

    // SAFETY:
    // Operation: `CFString::wrap_under_get_rule(notification_name)`.
    // Contract: identical to the previous call — non-null (asserted) and live, Get rule.
    // Evidence: `# Safety` clause 3. `name` is dropped at the end of this function, balancing
    // the `CFRetain` it performs.
    let name = CFString::wrap_under_get_rule(notification_name);
    let app = &cb_data.app;

    let event_name = name.to_string();
    trace!(app = %app.bundle_id, %event_name, "AX notification");

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
            // The element is already gone, so it usually cannot answer `role()` at all
            // (`kAXErrorInvalidUIElement`); requiring `Ok(window)` here meant a closed terminal
            // window never produced `WindowDestroyed` and the autocomplete window stayed on
            // screen (2026-09-17). Only a *known* non-window (menus, tooltips, text fields — the
            // bulk of these notifications) is skipped; everything else is reported and the
            // desktop decides by comparing window ids, which it can do cheaply.
            let known_non_window = element.role().is_ok_and(|role| role != kAXWindowRole);
            if known_non_window {
                None
            } else {
                Some(WindowServerEvent::WindowDestroyed { app: app.clone() })
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
