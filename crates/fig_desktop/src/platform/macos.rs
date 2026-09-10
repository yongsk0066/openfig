// This is needed for objc
#![allow(unexpected_cfgs)]
#![allow(deprecated)]

use std::ffi::CString;
use std::path::Path;
use std::slice;
use std::sync::atomic::{
    AtomicBool,
    Ordering,
};
use std::sync::{
    Arc,
    LazyLock,
    Mutex,
};

use accessibility_sys::{
    AXError,
    AXIsProcessTrusted,
    AXUIElementCreateSystemWide,
    AXUIElementSetMessagingTimeout,
    pid_t,
};
use anyhow::Context;
use cocoa::base::{
    NO,
    YES,
    id,
};
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
use macos_utils::caret_position::{
    CaretPosition,
    get_caret_position,
};
use macos_utils::window_server::{
    CGWindowLevelForKey,
    UIElement,
};
use macos_utils::{
    NotificationCenter,
    WindowServer,
    WindowServerEvent,
};
use objc::declare::MethodImplementation;
use objc::runtime::{
    BOOL,
    Class,
    Object,
    Sel,
    class_addMethod,
    objc_getClass,
    object_getClass,
};
use objc::{
    Encode,
    EncodeArguments,
    Encoding,
    msg_send,
    sel,
    sel_impl,
};
use objc2_foundation::{
    NSOperationQueue,
    ns_string,
};
use serde::Serialize;
use tao::dpi::{
    LogicalPosition,
    LogicalSize,
    Position,
};
use tao::platform::macos::{
    ActivationPolicy,
    EventLoopWindowTargetExtMacOS,
    WindowExtMacOS as _,
};
use tracing::{
    debug,
    error,
    trace,
    warn,
};

use super::PlatformBoundEvent;
use crate::event::{
    Event,
    EventSender,
    WindowEvent,
    WindowPosition,
};
use crate::protocol::icons::{
    AssetKind,
    AssetSpecifier,
    ProcessedAsset,
};
use crate::utils::Rect;
use crate::webview::notification::WebviewNotificationsState;
use crate::webview::{
    FigIdMap,
    GLOBAL_PROXY,
    WindowId,
};
use crate::{
    AUTOCOMPLETE_ID,
    AUTOCOMPLETE_WINDOW_TITLE,
    DASHBOARD_ID,
    EventLoopProxy,
    EventLoopWindowTarget,
};

pub const DEFAULT_CARET_WIDTH: f64 = 10.0;

// See for other window level keys
// https://github.com/phracker/MacOSX-SDKs/blob/master/MacOSX10.8.sdk/System/Library/Frameworks/CoreGraphics.framework/Versions/A/Headers/CGWindowLevel.h
#[allow(non_upper_case_globals)]
const kCGFloatingWindowLevelKey: i32 = 5;

static ACCESSIBILITY_ENABLED: LazyLock<AtomicBool> = LazyLock::new(|| AtomicBool::new(accessibility_is_enabled()));

static MACOS_VERSION: LazyLock<semver::Version> = LazyLock::new(|| {
    let version = macos_utils::os::OperatingSystemVersion::get();
    semver::Version::new(version.major() as u64, version.minor() as u64, version.patch() as u64)
});

pub static ACTIVATION_POLICY: Mutex<ActivationPolicy> = Mutex::new(ActivationPolicy::Regular);

#[allow(dead_code)]
pub fn is_ventura() -> bool {
    MACOS_VERSION.major >= 13
}

#[derive(Debug, Serialize)]
pub struct PlatformStateImpl {
    #[serde(skip)]
    proxy: EventLoopProxy,
    #[serde(skip)]
    focused_window: Mutex<Option<PlatformWindowImpl>>,
    /// The AX observer registry.
    ///
    /// This is the only owner: dropping it drops every `AXObserver`. It used to live in a
    /// `static`, which is what forced the (unproven) `unsafe impl Sync for WindowServer`;
    /// hanging it off the state that already reaches every user removes that requirement and
    /// gives `AccessibilityUpdated` a handle to re-register with.
    #[serde(skip)]
    window_server: Mutex<Option<WindowServer>>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PlatformWindowImpl {
    window_id: CGWindowID,
    ui_element: UIElement,
    x_term_tree_cache: Option<Vec<UIElement>>,
    pub bundle_id: String,
    pub pid: pid_t,
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
    pub fn new(bundle_id: String, pid: pid_t, ui_element: UIElement) -> Result<Self, AXError> {
        // SAFETY:
        // Operation: `UIElement::get_window_id`, an unsafe fn.
        // Contract (see macos-utils `UIElement::get_window_id`): the element must still be
        // live, and the caller accepts the `_AXUIElementGetWindow` SPI premise (it writes a
        // `CGWindowID` exactly on `kAXErrorSuccess`).
        // Evidence: `ui_element` is an owned `UIElement`, whose type invariant is that it holds
        // a live `+1` on its `AXUIElementRef`; it was just moved in from a
        // `WindowServerEvent::FocusChanged`. The SPI premise is inherited and recorded in
        // docs/13-unsafe-review.md #11.
        let window_id = unsafe { ui_element.get_window_id()? };
        Ok(Self {
            window_id,
            ui_element,
            pid,
            x_term_tree_cache: None,
            bundle_id,
        })
    }

    pub fn get_window_id(&self) -> CGWindowID {
        self.window_id
    }

    pub fn bundle_id(&self) -> &str {
        self.bundle_id.as_str()
    }

    pub fn get_bounds(&self) -> Option<CGRect> {
        let info = self.ui_element.window_info(false)?;
        Some(info.bounds)
    }

    pub fn get_level(&self) -> Option<i64> {
        // We grab all the windows since we don't want this to fail and are more fine if it's slow
        let info = self.ui_element.window_info(true)?;
        Some(info.level)
    }

    pub fn get_x_term_cursor_elem(&mut self) -> Option<UIElement> {
        let tree = self
            .x_term_tree_cache
            .as_ref()
            .and_then(|cached| {
                debug!("About to walk through {:?}", cached.len());
                let result: Option<Vec<UIElement>> =
                    cached.iter().fold(None::<Vec<UIElement>>, |accum, item| match accum {
                        Some(mut x) => {
                            x.push(item.clone());
                            Some(x)
                        },
                        None => item.find_x_term_caret_tree().ok(),
                    });
                result
            })
            .or_else(|| self.ui_element.find_x_term_caret_tree().ok());

        self.x_term_tree_cache = tree;
        self.x_term_tree_cache.as_ref()?.first().cloned()
    }
}

impl PlatformStateImpl {
    pub(super) fn new(proxy: EventLoopProxy) -> Self {
        let focused_window: Option<PlatformWindowImpl> = None;
        Self {
            proxy,
            focused_window: Mutex::new(focused_window),
            window_server: Mutex::new(None),
        }
    }

    //
    fn count_args(sel: Sel) -> usize {
        sel.name().chars().filter(|&c| c == ':').count()
    }

    fn method_type_encoding(ret: &Encoding, args: &[Encoding]) -> CString {
        let mut types = ret.as_str().to_owned();
        // First two arguments are always self and the selector
        types.push_str(<*mut Object>::encode().as_str());
        types.push_str(Sel::encode().as_str());
        types.extend(args.iter().map(|e| e.as_str()));
        CString::new(types).unwrap()
    }

    /// The Objective-C class behind wry's `WryWebView`, taken off a live webview.
    ///
    /// It cannot be looked up by name: since wry 0.57 the class is registered through objc2's
    /// `define_class!` with no explicit `#[name = ...]`, and objc2 0.6 then derives the runtime
    /// name from the module path plus the crate version --
    /// `wry::wkwebview::class::wry_web_view::WryWebView0.57.0`, not `WryWebView`. Reading the
    /// class off an instance survives every wry version bump.
    fn wry_web_view_class(window_map: &FigIdMap) -> Option<*mut Class> {
        use wry::WebViewExtMacOS;

        let window = window_map.values().next()?;
        let webview = window.webview.webview();
        let obj: *const Object = (&*webview as *const wry::WryWebView).cast();
        // SAFETY:
        // Operations: (1) the `*const wry::WryWebView -> *const objc::runtime::Object` cast on
        // the line above, and (2) the `object_getClass` FFI call.
        // Contract of `object_getClass(obj)` (Objective-C runtime): `obj` must be nil or a
        // pointer to a live Objective-C object; it returns the object's class (or nil for nil).
        // Evidence:
        // - `webview` is the `Retained<WryWebView>` bound on the line above, so it holds a strong reference
        //   for this whole borrow and `obj` is non-null and live.
        // - The cast in (1) reinterprets an objc2 object pointer as an objc-0.2 object pointer. Both are
        //   opaque `#[repr(C)]` placeholders for the same ObjC object; neither crate attaches Rust-level
        //   validity requirements beyond "points at a live object". This crate deliberately mixes the two
        //   runtimes because tao/wry straddle them.
        // Postcondition: `class` is nil or an immortal `Class`. ObjC classes are never
        // deallocated, so the returned pointer stays valid after `webview` is dropped — which
        // is why returning it out of this function is sound.
        let class = unsafe { object_getClass(obj) };
        (!class.is_null()).then_some(class.cast_mut())
    }

    // Add an implementation for an ObjC selector, which will override default WKWebView & WryWebView
    // behavior
    fn override_webview_method<F>(webview_class: Option<*mut Class>, sel: Sel, func: F)
    where
        F: MethodImplementation<Callee = Object>,
    {
        Self::override_objc_class_method(webview_class, "WryWebView", sel, func);
    }

    fn override_app_delegate_method<F>(sel: Sel, func: F)
    where
        F: MethodImplementation<Callee = Object>,
    {
        // tao still registers this one with the old `objc` ClassDecl under a literal name:
        // https://github.com/tauri-apps/tao/blob/dev/src/platform_impl/macos/app_delegate.rs
        const CLASS: &str = "TaoAppDelegateParent";
        let name = CString::new(CLASS).unwrap();
        // SAFETY:
        // Operation: the `objc_getClass` FFI call, plus the `*const Class -> *mut Class` cast.
        // Contract (Objective-C runtime): the argument must be a valid NUL-terminated C string;
        // the function returns the registered class of that name, or nil.
        // Evidence:
        // - `name` is a `CString` built from a `&'static str` literal with no interior NUL, so
        //   `CString::new` succeeded (the `unwrap` is infallible for this literal) and `as_ptr()` yields a
        //   valid NUL-terminated pointer. `name` is a live local for the whole call.
        // - The nil case is handled by the `is_null` test below, so a missing class degrades to "do not
        //   install the override" rather than to UB.
        // Postcondition: `class` is nil or an immortal `Class`; the `*const -> *mut` cast is
        // sound because `class_addMethod` mutates the class through the runtime's own locks,
        // not through Rust aliasing rules.
        // Fragility note (not a soundness defect): the class name is tao's, matched by string.
        // A tao upgrade that renames or objc2-ifies `TaoAppDelegateParent` silently disables
        // the override — the `trace!` in `override_objc_class_method` is the only signal.
        let class = unsafe { objc_getClass(name.as_ptr()) as *mut Class };
        Self::override_objc_class_method((!class.is_null()).then_some(class), CLASS, sel, func);
    }

    fn override_objc_class_method<F>(class: Option<*mut Class>, class_name: &str, sel: Sel, func: F)
    where
        F: MethodImplementation<Callee = Object>,
    {
        let encs = F::Args::encodings();
        let encs = encs.as_ref();
        let sel_args = Self::count_args(sel);
        assert!(
            sel_args == encs.len(),
            "Selector accepts {} arguments, but function accepts {}",
            sel_args,
            encs.len(),
        );

        let types = Self::method_type_encoding(&F::Ret::encode(), encs);

        let res = match class {
            // SAFETY:
            // Operation: the `class_addMethod` FFI call.
            // Contract (Objective-C runtime `class_addMethod(cls, name, imp, types)`): `cls`
            // must be a registered class, `name` a registered selector, `imp` a function
            // pointer whose ABI matches `types`, and `types` a valid NUL-terminated ObjC type
            // encoding string describing that same signature. Returns NO (without installing)
            // if the class already implements `name` directly.
            // Evidence:
            // - `cls`: the `Some` arm means the pointer came from `object_getClass` or `objc_getClass` and is non-null,
            //   i.e. a registered, immortal class.
            // - `name`: `sel` is produced by `sel!`, which interns the selector.
            // - `imp`/`types` agree: `types` is built by `method_type_encoding` from `F::Ret::encode()` and
            //   `F::Args::encodings()`, i.e. from the *same* `F` whose `imp()` is passed here, so the encoding is
            //   derived from the function type rather than written by hand. The `assert!` above additionally checks
            //   that the number of colons in the selector equals the number of declared arguments, which is the one
            //   thing the type system cannot cross-check.
            // - `types.as_ptr()` is valid for the call: `types` is a live `CString` local, and the runtime copies the
            //   encoding string.
            // Postcondition: on `true`, ObjC dispatch for `sel` on `class` now enters `func`.
            // `func` must therefore satisfy every obligation of an ObjC IMP for as long as the
            // class exists — which is forever, so `F` must be a plain `extern "C" fn`, as the
            // `MethodImplementation` bound enforces.
            Some(class) => unsafe { class_addMethod(class, sel, func.imp(), types.as_ptr()) },
            None => false,
        };
        trace!(class = %class_name, sel =% sel.name(), %res, "class_addMethod");
    }

    pub(super) fn handle(
        self: &Arc<Self>,
        event: PlatformBoundEvent,
        window_target: &EventLoopWindowTarget,
        window_map: &FigIdMap,
        notifications_state: &Arc<WebviewNotificationsState>,
    ) -> anyhow::Result<()> {
        debug!("Handling platform event: {:?}", event);
        match event {
            PlatformBoundEvent::Initialize => {
                // SAFETY:
                // Operations: `AXIsProcessTrusted`, `AXUIElementCreateSystemWide`, and
                // `AXUIElementSetMessagingTimeout`.
                // Contracts (HIServices):
                // - `AXIsProcessTrusted()` takes no arguments and has no preconditions.
                // - `AXUIElementCreateSystemWide()` takes no arguments and returns a `+1` `AXUIElementRef` (Create
                //   rule); it never returns NULL.
                // - `AXUIElementSetMessagingTimeout(element, seconds)` requires a live element and a non-negative
                //   timeout; it borrows the element.
                // Evidence: the element argument is the POSTCONDITION of the call nested
                // directly inside it, so it is live for the whole call; `0.25` is a positive
                // `f32`.
                // Leak note: the `+1` is never released. It is one system-wide element per
                // process, created at most twice (here and in `AccessibilityUpdateRequested`),
                // so the leak is bounded and harmless. See docs/13-unsafe-review.md #6.
                unsafe {
                    if AXIsProcessTrusted() {
                        // This prevents Fig from becoming unresponsive if one of the applications
                        // we are tracking becomes unresponsive.
                        AXUIElementSetMessagingTimeout(AXUIElementCreateSystemWide(), 0.25);
                    }
                }

                let (tx, rx) = flume::unbounded::<WindowServerEvent>();

                self.window_server.lock().unwrap().replace(WindowServer::new(
                    tx,
                    APP_BUNDLE_ID,
                    fig_util::terminal::xterm_bundle_ids(),
                ));

                let accessibility_proxy = self.proxy.clone();
                let mut distributed = NotificationCenter::distributed_center();
                let ax_notification_name = ns_string!("com.apple.accessibility.api");
                let queue = NSOperationQueue::new();
                distributed.subscribe(ax_notification_name, Some(&queue), move |_| {
                    accessibility_proxy
                        .clone()
                        .send_event(Event::PlatformBoundEvent(
                            PlatformBoundEvent::AccessibilityUpdateRequested,
                        ))
                        .ok();
                });

                let observer_proxy = self.proxy.clone();
                tokio::runtime::Handle::current().spawn(async move {
                    while let std::result::Result::Ok(result) = rx.recv_async().await {
                        let mut events: Vec<Event> = vec![];

                        match result {
                            WindowServerEvent::FocusChanged { window, app } => {
                                events.push(Event::WindowEvent {
                                    window_id: AUTOCOMPLETE_ID,
                                    window_event: WindowEvent::Hide,
                                });

                                if let Ok(window) = PlatformWindowImpl::new(app.bundle_id, app.pid, window) {
                                    events.push(Event::PlatformBoundEvent(
                                        PlatformBoundEvent::ExternalWindowFocusChanged { window },
                                    ));
                                }
                            },
                            WindowServerEvent::WindowDestroyed { app } => {
                                events.push(Event::PlatformBoundEvent(PlatformBoundEvent::WindowDestroyed { app }));
                            },
                            WindowServerEvent::ActiveSpaceChanged { is_fullscreen } => {
                                events.extend([
                                    Event::WindowEvent {
                                        window_id: AUTOCOMPLETE_ID.clone(),
                                        window_event: WindowEvent::Hide,
                                    },
                                    Event::PlatformBoundEvent(PlatformBoundEvent::FullscreenStateUpdated {
                                        fullscreen: is_fullscreen,
                                        dashboard_visible: None,
                                    }),
                                ]);
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
                /// Borrow an `NSString`'s UTF-8 bytes as a `&str`.
                ///
                /// # Safety
                ///
                /// The caller must ensure that:
                ///
                /// 1. `nsstring_obj` is `nil` or a pointer to a live `NSString`;
                /// 2. that object, and the autorelease pool that owns the buffer returned by
                ///    `-UTF8String`, both outlive the chosen lifetime `'a`. `'a` is *unbounded* —
                ///    it is chosen by the caller, not tied to any input — so nothing in the type
                ///    system enforces this. See docs/13-unsafe-review.md #17.
                ///
                /// Both call sites below satisfy (2) by comparing the `&str` and dropping it
                /// within the same ObjC callback frame.
                ///
                /// This was a *safe* `fn` returning an unbounded lifetime from a raw pointer,
                /// which let safe code produce a dangling `&str`; it is now `unsafe fn`, which
                /// costs nothing because both callers already sit inside `unsafe` blocks.
                unsafe fn to_s<'a>(nsstring_obj: *mut Object) -> Option<&'a str> {
                    const UTF8_ENCODING: libc::c_uint = 4;

                    // SAFETY:
                    // Operations: (1) `objc_msgSend` for
                    // `-lengthOfBytesUsingEncoding:NSUTF8StringEncoding`; (2) `objc_msgSend`
                    // for `-UTF8String`; (3) `slice::from_raw_parts(utf8_str, length)`.
                    //
                    // (1)+(2) Contract (ObjC ABI): the receiver must be nil or a live object
                    // responding to the selector. Evidence: `# Safety` clause 1. A nil
                    // receiver returns 0 / NULL under the documented nil-messaging rule, which
                    // the null check below turns into `None`.
                    //
                    // (3) Contract (std `slice::from_raw_parts`): `data` must be non-null and
                    // aligned; `len * size_of::<u8>()` bytes must be initialized and lie in one
                    // live allocation; the memory must not be mutated for `'a`.
                    // Evidence:
                    // - Non-null: the explicit `is_null` check below. This is load-bearing, not defensive — std
                    //   requires a non-null pointer *even for `len == 0`*, and `[window title]` legitimately returns
                    //   nil for an untitled `NSWindow`, which would have made `nsstring_obj` nil and `utf8_str` NULL.
                    //   The previous version had no check and constructed a slice from NULL. See
                    //   docs/13-unsafe-review.md #18.
                    // - Alignment: `u8` has alignment 1.
                    // - Length/initialization: Apple documents `-lengthOfBytesUsingEncoding:` as the byte count of the
                    //   receiver in the given encoding excluding the NUL, and `-UTF8String` as a NUL-terminated UTF-8
                    //   representation, so the first `length` bytes of the buffer are initialized. DEPENDENCY LEMMA
                    //   (Apple Foundation).
                    // - No mutation: the buffer is an immutable copy owned by the string or the autorelease pool.
                    // Note `std::str::from_utf8` (checked) is used below, so an ill-formed
                    // representation yields `None` rather than an invalid `&str`.
                    let bytes = unsafe {
                        let length = msg_send![nsstring_obj, lengthOfBytesUsingEncoding: UTF8_ENCODING];
                        let utf8_str: *const u8 = msg_send![nsstring_obj, UTF8String];
                        if utf8_str.is_null() {
                            return None;
                        }
                        slice::from_raw_parts(utf8_str, length)
                    };
                    std::str::from_utf8(bytes).ok()
                }

                /// ObjC IMP installed on wry's `WryWebView` for
                /// `-shouldDelayWindowOrderingForEvent:`.
                ///
                /// # Safety
                ///
                /// Called by AppKit, not by Rust. The obligations on that caller are the
                /// standard IMP ones: `this` is a live receiver of the class this method was
                /// added to, `_cmd` is the selector, and the call happens on the main thread
                /// (AppKit event dispatch). It must not unwind into Objective-C; `extern "C"`
                /// functions abort on unwind since Rust 1.81, so a panic terminates the
                /// process rather than causing UB.
                extern "C" fn should_delay_window_ordering(this: &Object, _cmd: Sel, _event: id) -> BOOL {
                    debug!("should_delay_window_ordering");

                    // SAFETY:
                    // Operations: `objc_msgSend` for `-window` and `-title`, then `to_s`.
                    // Contract (ObjC ABI): live receiver responding to the selector, with the
                    // declared return type.
                    // Evidence:
                    // - `this: &Object` is a Rust reference, hence a live receiver. It is an `NSView` subclass
                    //   (`WryWebView`), which responds to `-window`.
                    // - `window` may legitimately be nil (a view not in a window). Sending `-title` to nil returns nil
                    //   under the documented nil-messaging rule, and `to_s` turns nil into `None`. So the missing nil
                    //   check is covered by that rule plus `to_s`'s own null handling — not by luck.
                    // Contract of `to_s` (see its `# Safety`): nil-or-live `NSString`, and the
                    // string plus its autorelease pool must outlive `'a`.
                    // Evidence: `title` is autoreleased into the pool AppKit wraps around event
                    // dispatch; the resulting `&str` is compared and dropped inside this same
                    // frame, well before that pool drains.
                    unsafe {
                        let window: id = msg_send![this, window];
                        let title: id = msg_send![window, title];

                        // TODO: implement better method for determining if WebView belongs to autocomplete
                        if let Some(title) = to_s(title)
                            && title == AUTOCOMPLETE_WINDOW_TITLE
                        {
                            return YES;
                        }
                    }

                    NO
                }

                /// ObjC IMP installed on wry's `WryWebView` for `-mouseDown:`.
                ///
                /// # Safety
                ///
                /// Same IMP contract as [`should_delay_window_ordering`]: AppKit supplies a
                /// live receiver of the overridden class on the main thread, and this function
                /// must not unwind into Objective-C.
                ///
                /// Note the `Class::get("NSApplication").unwrap()` below: a panic there aborts
                /// (see the unwind rule above) rather than corrupting AppKit state, but it is
                /// still an `unwrap` on an FFI lookup. See docs/13-unsafe-review.md, adjacent
                /// smells.
                extern "C" fn mouse_down(this: &Object, _cmd: Sel, event: id) {
                    let application = Class::get("NSApplication").unwrap();

                    // SAFETY:
                    // Operations: `objc_msgSend` for `-window`, `-title`, `+sharedApplication`,
                    // `-preventWindowOrdering`, `-superclass`, and `objc_msgSendSuper` for
                    // `-mouseDown:`; plus `to_s`.
                    // Contract (ObjC ABI): live receiver, existing selector, matching signature.
                    // Evidence:
                    // - `this: &Object` is live (Rust reference) and is a `WryWebView`, an `NSView` subclass, so
                    //   `-window` and `-superclass` exist. Nil `window` and nil `title` are handled by the
                    //   nil-messaging rule and `to_s`.
                    // - `application` is the registered `NSApplication` class, which responds to `+sharedApplication`;
                    //   the result responds to `-preventWindowOrdering` (documented `NSApplication` method).
                    // - `to_s`'s obligations are discharged exactly as in `should_delay_window_ordering`.
                    // - The `super` send: `objc_msgSendSuper` requires an `objc_super { receiver, super_class }` whose
                    //   `super_class` actually defines (or inherits) the selector.
                    //
                    // UNPROVEN-BUT-LIKELY — see docs/13-unsafe-review.md #19. `supercls` is
                    // `[this superclass]`, i.e. the superclass of the *object's* class, not of
                    // the class this IMP was installed on. Those coincide only while
                    // `[this class] == WryWebView`. If AppKit or WebKit ever isa-swizzles the
                    // instance into a subclass whose `-class`/`-superclass` are not overridden
                    // the way KVO overrides them, `supercls` becomes `WryWebView` itself and
                    // this send re-enters this very IMP — unbounded recursion, then stack
                    // overflow. The robust form hardcodes the class the method was added to.
                    unsafe {
                        let window: id = msg_send![this, window];
                        let title: id = msg_send![window, title];

                        // TODO: implement better method for determining if WebView belongs to autocomplete
                        if let Some(title) = to_s(title)
                            && title == AUTOCOMPLETE_WINDOW_TITLE
                        {
                            // Prevent clicked window from taking focus
                            let app: id = msg_send![application, sharedApplication];
                            let _: () = msg_send![app, preventWindowOrdering];
                        }

                        // Invoke superclass implementation
                        let supercls = msg_send![this, superclass];
                        let _: () = msg_send![super(this, supercls), mouseDown: event];
                    }
                }

                // Use objc runtime to override WryWebview methods
                let webview_class = Self::wry_web_view_class(window_map);
                Self::override_webview_method(
                    webview_class,
                    sel!(shouldDelayWindowOrderingForEvent:),
                    should_delay_window_ordering as extern "C" fn(&Object, Sel, id) -> BOOL,
                );
                Self::override_webview_method(
                    webview_class,
                    sel!(mouseDown:),
                    mouse_down as extern "C" fn(&Object, Sel, id),
                );

                extern "C" fn application_should_handle_reopen(
                    _this: &Object,
                    _cmd: Sel,
                    _sender: id,
                    _visible_windows: BOOL,
                ) -> BOOL {
                    trace!("application_should_handle_reopen");

                    let proxy = GLOBAL_PROXY.get().unwrap();

                    if let Err(err) = proxy.send_event(Event::WindowEvent {
                        window_id: DASHBOARD_ID,
                        window_event: WindowEvent::Show,
                    }) {
                        warn!(%err, "Error sending event");
                    }

                    YES
                }

                Self::override_app_delegate_method(
                    sel!(applicationShouldHandleReopen:hasVisibleWindows:),
                    application_should_handle_reopen as extern "C" fn(&Object, Sel, id, BOOL) -> BOOL,
                );

                Ok(())
            },
            PlatformBoundEvent::EditBufferChanged => {
                // `debug!`, to match `CaretPositionUpdateRequested` below, which calls the exact
                // same function. Without accessibility permission this fires once per keystroke,
                // and at `error!` it drowned out every real error in the log.
                if let Err(err) = self.refresh_window_position() {
                    debug!(%err, "Failed to refresh window position");
                }
                Ok(())
            },
            PlatformBoundEvent::ExternalWindowFocusChanged { window } => {
                let current_terminal = Terminal::from_bundle_id(window.bundle_id.clone());
                let level = window.get_level();

                // Neither of these depends on the window level. `get_level()` returns `None`
                // whenever `CGWindowListCopyWindowInfo` has nothing for the window — an iTerm2
                // hotkey window (level != 0), a fullscreen transition, a failed
                // `_AXUIElementGetWindow` — and gating them on `Some(0)` left `focused_window`
                // permanently `None` there, so the autocomplete window never moved again and
                // every keystroke failed in `refresh_window_position`. Only the level *sync*
                // below cares about the level.
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

                self.focused_window.lock().unwrap().replace(window);

                if let Some(window) = window_map.get(&AUTOCOMPLETE_ID) {
                    let ns_window = window.window.ns_window().cast::<Object>();
                    // Handle iTerm Quake mode by explicitly setting window level. See
                    // https://github.com/gnachman/iTerm2/blob/1a5a09f02c62afcc70a647603245e98862e51911/sources/iTermProfileHotKey.m#L276-L310
                    // for more on window levels.
                    let above = match level {
                        // SAFETY:
                        // Operation: the `CGWindowLevelForKey` FFI call.
                        // Contract (see the `unsafe extern` block in macos-utils'
                        // `ui_element.rs`): takes a `CGWindowLevelKey` (`i32`) and returns an
                        // `i32`; no pointers, no state, no thread affinity.
                        // Evidence: `kCGFloatingWindowLevelKey` is the literal 5, a documented
                        // member of `CGWindowLevelKey`.
                        None | Some(0) => unsafe { CGWindowLevelForKey(kCGFloatingWindowLevelKey) as i64 },
                        Some(level) => level,
                    };
                    debug!("Setting window level to {level:?}");
                    // SAFETY:
                    // Operation: `objc_msgSend` for `-[NSWindow setLevel:]`.
                    // Contract (ObjC ABI): live receiver responding to the selector, and the
                    // argument type must match `NSWindowLevel`.
                    // Evidence:
                    // - `ns_window` is `window.window.ns_window().cast::<Object>()`, i.e. tao's backing `NSWindow` for
                    //   a `tao::window::Window` that `window_map` still owns, so it is live for this statement. A nil
                    //   pointer would make this a documented no-op rather than UB.
                    // - `NSWindowLevel` is a typedef for `NSInteger`, which on aarch64-apple and x86_64-apple is
                    //   64-bit; `above: i64` therefore matches the ObjC parameter type exactly (TYPE FACT for the only
                    //   supported targets).
                    // - The `()` return type matches the method's `void`.
                    let _: () = unsafe { msg_send![ns_window, setLevel: above] };
                }

                Ok(())
            },
            PlatformBoundEvent::CaretPositionUpdateRequested => {
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

                let mut policy_lock = ACTIVATION_POLICY.lock().unwrap();
                if *policy_lock != policy {
                    debug!(?policy, "Setting application policy");
                    *policy_lock = policy;
                    window_target.set_activation_policy_at_runtime(policy);
                }
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
                        // SAFETY:
                        // Operations and contracts: identical to the `Initialize` arm above —
                        // `AXUIElementCreateSystemWide()` returns a live `+1` element with no
                        // preconditions, and `AXUIElementSetMessagingTimeout` borrows it.
                        // Evidence: the argument is the postcondition of the nested call.
                        // Thread note: unlike the `Initialize` arm, this runs on a tokio worker
                        // thread, not the main thread. That is fine for these two calls (they
                        // touch no run loop and no per-thread AX state) but it is the same
                        // untested "AX from a worker thread" premise recorded in
                        // docs/13-unsafe-review.md #8.
                        // Leak note: one more system-wide element per accessibility toggle.
                        unsafe {
                            // This prevents Fig from becoming unresponsive if one of the applications
                            // we are tracking becomes unresponsive.
                            AXUIElementSetMessagingTimeout(AXUIElementCreateSystemWide(), 0.25);
                        }
                    }
                });

                Ok(())
            },
            PlatformBoundEvent::AccessibilityUpdated { enabled } => {
                let was_enabled = ACCESSIBILITY_ENABLED.swap(enabled, Ordering::SeqCst);

                // `WindowServer::new` is the only thing that ever registered AX observers, and
                // it runs once during `Initialize`. Without accessibility permission every
                // `register` call returns early, so granting permission at runtime left the app
                // tracking zero applications — reporting autocomplete as enabled while nothing
                // followed the caret — until the next restart.
                if enabled
                    && !was_enabled
                    && let Some(window_server) = self.window_server.lock().unwrap().as_mut()
                {
                    // SAFETY:
                    // Operation: `WindowServer::reregister_all`, an unsafe fn.
                    // Contract (see its `# Safety`): must run on the thread owning the run loop
                    // the AX sources are attached to.
                    // Evidence: `PlatformBoundEvent`s are dispatched from `webview/mod.rs:503`,
                    // inside `event_loop.run`, i.e. the tao main thread — the same thread
                    // `WindowServer::new` was called on during `Initialize`.
                    unsafe { window_server.reregister_all() };
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
                let mut focused = self.focused_window.lock().unwrap();
                if let Some(focused_window) = focused.as_ref()
                    && focused_window.bundle_id() == app.bundle_id
                {
                    focused.take();
                    self.proxy
                        .send_event(Event::WindowEvent {
                            window_id: AUTOCOMPLETE_ID,
                            window_event: WindowEvent::Hide,
                        })
                        .ok();
                }
                Ok(())
            },
        }
    }

    fn refresh_window_position(&self) -> anyhow::Result<()> {
        let mut guard = self.focused_window.lock().unwrap();
        let active_window = guard.as_mut().context("No active window")?;
        let current_terminal = Terminal::from_bundle_id(active_window.bundle_id());

        let is_xterm = current_terminal.is_some_and(|t| t.is_xterm());

        let caret = if is_xterm {
            active_window
                .get_x_term_cursor_elem()
                .and_then(|c| c.frame().ok())
                .map(Rect::from)
        } else {
            self.get_cursor_position()
        };

        let caret = caret.context("Failed to get cursor position")?;
        debug!("Sending caret update {:?}", caret);

        self.proxy.send_event_or_log(Event::WindowEvent {
            window_id: AUTOCOMPLETE_ID,
            window_event: WindowEvent::UpdateWindowGeometry {
                position: Some(WindowPosition::RelativeToCaret {
                    caret_position: caret.position,
                    caret_size: caret.size,
                }),
                size: None,
                anchor: None,
                tx: None,
                dry_run: false,
            },
        });

        Ok(())
    }

    #[allow(clippy::unused_self)]
    pub(super) fn position_window(
        &self,
        webview_window: &tao::window::Window,
        _window_id: &WindowId,
        position: Position,
    ) -> wry::Result<()> {
        webview_window.set_outer_position(position);
        std::result::Result::Ok(())
    }

    #[allow(clippy::unused_self)]
    pub(super) fn get_cursor_position(&self) -> Option<Rect> {
        // SAFETY:
        // Operation: `macos_utils::caret_position::get_caret_position`, an unsafe fn.
        // Contract (see its `# Safety`): the AX API must be usable from this thread, and the
        // caller accepts the `AXValueGetValue` / `kAXValueType*` layout premises.
        // Evidence: `get_cursor_position` is reached from `refresh_window_position`, which runs
        // on the tao event loop (main thread) for `EditBufferChanged` and
        // `CaretPositionUpdateRequested`. The layout premises are recorded in
        // docs/13-unsafe-review.md #11 and are inherited from the vendored
        // `accessibility-sys` bindings.
        let caret: CaretPosition = unsafe { get_caret_position(true) };

        if caret.valid {
            Some(Rect {
                position: LogicalPosition::new(caret.x, caret.y).into(),
                size: LogicalSize::new(DEFAULT_CARET_WIDTH, caret.height).into(),
            })
        } else {
            None
        }
    }

    /// The bottom edge of the focused window, in logical pixels.
    ///
    /// This used to return a whole `PlatformWindow`, which meant cloning the `PlatformWindowImpl`
    /// — a `UIElement` `CFRetain` plus the entire `x_term_tree_cache` `Vec`, each element
    /// retained — so that the caller could read `rect.bottom()` and drop the rest.
    /// `PlatformWindow::inner` had no reader at all.
    pub(super) fn active_window_bottom(&self, scale: f64) -> Option<f64> {
        let guard = self.focused_window.lock().unwrap();
        let active_window = guard.as_ref()?;
        Some(Rect::from(active_window.get_bounds()?).bottom(scale))
    }

    pub(super) async fn icon_lookup(asset: &AssetSpecifier<'_>) -> Option<ProcessedAsset> {
        // SAFETY: UNPROVEN — see docs/13-unsafe-review.md #16.
        // Operations: the five calls to `macos_utils::image::png_for_name` /
        // `png_for_path`, all unsafe fns.
        // Contract (see their `# Safety`): AppKit must be usable from the calling thread.
        // Evidence: this is an `async fn` driven by the tokio runtime, so it runs on a worker
        // thread, not the main thread. `NSImage`, `NSBitmapImageRep` and `NSWorkspace`'s icon
        // accessors are documented as usable from secondary threads, and
        // `NSGraphicsContext::currentContext()` simply returns `None` off the main thread — so
        // the missing premise is narrow (no documented main-thread-only call is reached) rather
        // than obviously violated. It is nevertheless not discharged by anything authoritative,
        // and it is the reason `png_for_*` are `unsafe fn` at all.
        let data = match asset {
            AssetSpecifier::Named(name) => unsafe { macos_utils::image::png_for_name(name)? },
            AssetSpecifier::PathBased(path) => (unsafe { macos_utils::image::png_for_path(path) })
                .or_else(|| {
                    if path.to_str()?.ends_with('/') {
                        // /bin will always exist and looks like the default folder
                        // TODO: replace with `iconForContentType`
                        unsafe { macos_utils::image::png_for_path(Path::new("/bin")) }
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    if let Some(ext) = path.extension() {
                        unsafe { macos_utils::image::png_for_name(ext.to_str()?) }
                    } else {
                        None
                    }
                })
                .or_else(|| unsafe { macos_utils::image::png_for_name("file") })?,
        };

        Some((Arc::new(data.into()), AssetKind::Png))
    }

    pub(super) fn accessibility_is_enabled() -> Option<bool> {
        Some(ACCESSIBILITY_ENABLED.load(Ordering::SeqCst))
    }
}

pub const fn autocomplete_active() -> bool {
    true
}
