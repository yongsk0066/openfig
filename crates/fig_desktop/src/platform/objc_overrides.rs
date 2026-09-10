//! Objective-C method injection into wry's `WryWebView` class.
//!
//! Split out of the `PlatformBoundEvent::InitializePostRun` match arm, which nested two `extern
//! "C"` IMPs and their SAFETY commentary — about 200 lines — inside a `match`, making
//! `PlatformState::handle` unreadable.

// `cocoa` 0.27 is deprecated in favour of objc2, but the `objc` 0.2
// `Encode`/`MethodImplementation` machinery these overrides are written against still takes
// `cocoa::base::id`.
#![allow(unexpected_cfgs)]
#![allow(deprecated)]

use std::ffi::CString;
use std::slice;

use cocoa::base::{
    NO,
    YES,
    id,
};
use objc::declare::MethodImplementation;
use objc::runtime::{
    BOOL,
    Class,
    Object,
    Sel,
    class_addMethod,
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
use tracing::{
    debug,
    trace,
};

use crate::AUTOCOMPLETE_WINDOW_TITLE;
use crate::webview::FigIdMap;

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
    override_objc_class_method(webview_class, "WryWebView", sel, func);
}

fn override_objc_class_method<F>(class: Option<*mut Class>, class_name: &str, sel: Sel, func: F)
where
    F: MethodImplementation<Callee = Object>,
{
    let encs = F::Args::encodings();
    let encs = encs.as_ref();
    let sel_args = count_args(sel);
    assert!(
        sel_args == encs.len(),
        "Selector accepts {} arguments, but function accepts {}",
        sel_args,
        encs.len(),
    );

    let types = method_type_encoding(&F::Ret::encode(), encs);

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
        //   `F::Args::encodings()`, i.e. from the *same* `F` whose `imp()` is passed here, so the encoding is derived
        //   from the function type rather than written by hand. The `assert!` above additionally checks that the number
        //   of colons in the selector equals the number of declared arguments, which is the one thing the type system
        //   cannot cross-check.
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

/// Install the `WryWebView` overrides. Runs once, from `PlatformBoundEvent::InitializePostRun`.
pub(super) fn install(window_map: &FigIdMap) {
    /// Borrow an `NSString`'s UTF-8 bytes as a `&str`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    ///
    /// 1. `nsstring_obj` is `nil` or a pointer to a live `NSString`;
    /// 2. that object, and the autorelease pool that owns the buffer returned by `-UTF8String`,
    ///    both outlive the chosen lifetime `'a`. `'a` is *unbounded* — it is chosen by the caller,
    ///    not tied to any input — so nothing in the type system enforces this. See
    ///    docs/13-unsafe-review.md #17.
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
    let webview_class = wry_web_view_class(window_map);
    override_webview_method(
        webview_class,
        sel!(shouldDelayWindowOrderingForEvent:),
        should_delay_window_ordering as extern "C" fn(&Object, Sel, id) -> BOOL,
    );
    override_webview_method(
        webview_class,
        sel!(mouseDown:),
        mouse_down as extern "C" fn(&Object, Sel, id),
    );
}
