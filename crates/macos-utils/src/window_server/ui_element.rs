use std::ffi::c_void;
use std::fmt;

use accessibility::util::ax_call;
use accessibility_sys::{
    _AXUIElementGetWindow,
    AXError,
    AXUIElement,
    AXUIElementCopyAttributeNames,
    AXUIElementCopyAttributeValue,
    AXUIElementRef,
    AXUIElementSetAttributeValue,
    AXValue,
    kAXApplicationRole,
    kAXBrowserRole,
    kAXChildrenAttribute,
    kAXDOMClassListAttribute,
    kAXEnhancedUserInterfaceAttribute,
    kAXErrorAttributeUnsupported,
    kAXFocusedAttribute,
    kAXFocusedWindowAttribute,
    kAXFrameAttribute,
    kAXFullScreenAttribute,
    kAXGroupRole,
    kAXManualAccessibilityAttribute,
    kAXRoleAttribute,
    kAXScrollAreaRole,
    kAXTextFieldRole,
    kAXWebAreaRole,
};
use core_foundation::ConcreteCFType;
use core_foundation::array::{
    CFArray,
    CFArrayRef,
};
use core_foundation::base::{
    CFType,
    CFTypeRef,
    TCFType,
    TCFTypeRef,
};
use core_foundation::boolean::{
    CFBoolean,
    kCFBooleanTrue,
};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{
    CFString,
    CFStringRef,
};
use core_graphics::display::{
    self,
    CGRect,
};
use core_graphics::window::{
    self,
    CGWindowID,
    kCGWindowBounds,
    kCGWindowLayer,
    kCGWindowNumber,
    kCGWindowOwnerPID,
};
use tracing::warn;

use crate::util::{
    NSArrayRef,
    NSStringRef,
};

pub struct UIElement(AXUIElement);

impl Clone for UIElement {
    fn clone(&self) -> Self {
        UIElement::from(self.get_ref())
    }
}

impl fmt::Debug for UIElement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let position = self.frame().map(|CGRect { origin, size }| {
            format!(
                "({:.3}, {:.3}) - ({:.3}, {:.3})",
                origin.x,
                origin.y,
                origin.x + size.width,
                origin.y + size.height
            )
        });
        f.debug_struct("UIElement")
            .field("role", &self.role())
            .field("position", &position)
            .finish()
    }
}

// SAFETY: partially proven — see docs/13-unsafe-review.md #8.
//
// Proposition asserted: a `UIElement` may be moved between threads (`Send`) and shared by `&`
// across threads (`Sync`). Both are load-bearing: `WindowServerEvent::FocusChanged` carries a
// `UIElement` over a `flume` channel into a tokio task, and `PlatformWindowImpl` (which owns
// several) is moved through the tao `EventLoopProxy`.
//
// `Send` is proved: the only field is an `AXUIElement`, a newtype over a `CFTypeRef`, and its
// `Drop` is `CFRelease`. Apple's "Memory Management Programming Guide for Core Foundation"
// documents `CFRetain`/`CFRelease` as thread-safe, so moving ownership to another thread and
// releasing there cannot race with the retain that created it.
//
// `Sync` is UNPROVEN. `&UIElement` gives shared access to `get_attr_ref`, `frame`, `role`,
// `focused_window`, `window_info`, … which call `AXUIElementCopyAttributeValue` and friends.
// Apple's HIServices documentation makes no thread-safety statement about those functions, and
// the API is a synchronous IPC to the observed application; the only guarantee we can cite is
// that the AX API is *not* documented as main-thread-only. Missing premise: an authoritative
// statement that concurrent AX calls on one element are permitted.
//
// Not part of the proof, but worth recording: the `CFRelease` on drop and the AX calls happen
// on whichever thread holds the value, so soundness here does not depend on the run-loop
// affinity that `AXObserver` needs — a `UIElement` holds no run-loop source.
unsafe impl Send for UIElement {}
// SAFETY: UNPROVEN — see the comment on the `Send` impl above and docs/13-unsafe-review.md #8.
unsafe impl Sync for UIElement {}

impl From<AXUIElement> for UIElement {
    fn from(ax_ref: AXUIElement) -> Self {
        UIElement(ax_ref)
    }
}

/// # Invariants
///
/// This conversion applies Core Foundation's **Get rule**: it retains `ax_ref` and releases the
/// retain when the `UIElement` drops. It therefore does *not* consume a `+1` produced by a
/// Create-rule function such as `AXUIElementCreateApplication`; passing one leaks that `+1`.
///
/// It is a *safe* `From` impl that dereferences a raw pointer, so it silently places an
/// unchecked memory-safety obligation on safe callers — `ax_ref` must be a live
/// `AXUIElementRef`. The review skill rejects that shape; it is kept here only because the
/// blast radius of changing the trait impl exceeds this review's remit.
/// See docs/13-unsafe-review.md #10.
impl From<AXUIElementRef> for UIElement {
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn from(ax_ref: AXUIElementRef) -> Self {
        // SAFETY:
        // Operation: `AXUIElement::wrap_under_get_rule(ax_ref)`.
        // Contract (core-foundation 0.10 `TCFType::wrap_under_get_rule`): `reference` must be
        // non-null and point at a live CF object of the concrete type; the call performs
        // `CFRetain` and hands the resulting `+1` to a wrapper that `CFRelease`s on drop.
        // Evidence:
        // - Non-nullness is not proved here — it is checked: the generated `wrap_under_get_rule` starts
        //   with `assert!(!reference.is_null())`, so a null input panics rather than causing UB.
        // - Liveness and concrete type are the caller obligation described in the `# Invariants` section
        //   above. Every in-crate caller passes either an element that HIServices just handed to
        //   `application_ax_callback` (owned by HIServices for the duration of the call), the result of
        //   `AXUIElementCreateApplication` (live by the Create rule), or an element read out of a
        //   `CFArray`/attribute that is still alive.
        // Postcondition: the returned `UIElement` owns one reference, independent of the
        // caller's.
        UIElement(unsafe { AXUIElement::wrap_under_get_rule(ax_ref) })
    }
}

static XTERM_ROLES: &[&str] = &[
    kAXScrollAreaRole,
    kAXGroupRole,
    kAXWebAreaRole,
    kAXTextFieldRole,
    kAXApplicationRole,
    kAXBrowserRole,
];

type Result<T> = std::result::Result<T, AXError>;

#[derive(Debug)]
pub struct CGWindowInfo {
    pub window_id: CGWindowID,
    pub bounds: CGRect,
    pub owner_pid: u64,
    pub level: i64,
}

impl UIElement {
    fn get_ref(&self) -> AXUIElementRef {
        self.0.as_concrete_TypeRef()
    }

    /// The `CGWindowID` of this element, if it is a window.
    ///
    /// # Safety
    ///
    /// The caller must ensure that this element's `AXUIElementRef` is still live — that is,
    /// that the type invariant of `UIElement` (its `AXUIElement` field owns a live reference)
    /// has not been broken by unsafe code elsewhere. `_AXUIElementGetWindow` is a private
    /// HIServices SPI with no published contract, so this function additionally rests on the
    /// TCB premise that it writes a `CGWindowID` through the out-parameter exactly when it
    /// returns `kAXErrorSuccess`. See docs/13-unsafe-review.md #11.
    ///
    /// Given a safe `&self`, no caller can actually violate the first clause, so the `unsafe`
    /// marker really only fences the SPI premise.
    pub unsafe fn get_window_id(&self) -> Result<CGWindowID> {
        // TODO: cache this value on struct?
        // SAFETY:
        // Operations: `ax_call::<_, CGWindowID>` (whose tail is
        // `MaybeUninit::<CGWindowID>::assume_init()`) and the `_AXUIElementGetWindow` FFI call.
        // Contract of `ax_call`: the closure must initialize `*out` whenever it returns
        // `kAXErrorSuccess`; `assume_init` is only reached on that path.
        // Evidence: the SPI premise in the `# Safety` section above. `CGWindowID` is `u32`, so
        // any bit pattern the SPI writes is a valid value; the only residual risk is reading
        // uninitialized memory on a success return that did not write, which is precisely what
        // the TCB premise excludes.
        ax_call(|window_id: *mut CGWindowID| _AXUIElementGetWindow(self.get_ref(), window_id))
    }

    fn get_attr_ref(&self, attr: &str) -> Result<CFType> {
        // SAFETY:
        // Operations: (1) `ax_call::<_, CFTypeRef>`, ending in
        // `MaybeUninit::<CFTypeRef>::assume_init()`; (2) the `AXUIElementCopyAttributeValue`
        // FFI call; (3) `CFType::wrap_under_get_rule(cf_ref)`.
        //
        // (1)+(2) Contract of `ax_call`: the closure must initialize `*out` on
        // `kAXErrorSuccess`. Contract of `AXUIElementCopyAttributeValue(element, attribute,
        // *value)` (HIServices): `element` and `attribute` must be live CF objects; on
        // `kAXErrorSuccess` it writes a `+1` value through `value` ("the caller is responsible
        // for releasing"), otherwise it writes nothing.
        // Evidence:
        // - `self.get_ref()` returns the `AXUIElementRef` owned by this `UIElement`, live for the duration
        //   of `&self` (TYPE FACT: the `AXUIElement` field holds a `+1`).
        // - `attr` is a `CFString` local; the CFString is dropped only at the end of the closure body, i.e.
        //   after the FFI call returns, and `as_concrete_TypeRef` follows the Get rule so it does not
        //   transfer ownership.
        // - `CFTypeRef` is a raw pointer, so any value written is a valid `CFTypeRef`.
        //
        // (3) Contract of `wrap_under_get_rule`: non-null (asserted) and live; it `CFRetain`s.
        // Evidence: liveness is the postcondition of (2).
        //
        // BUG (retain-count, not UB): (2) follows the **Copy rule** and yields `+1`, but (3)
        // applies the **Get rule** and retains again, giving `+2` while the returned `CFType`
        // will only release once. Every attribute read through this crate therefore leaks one
        // reference to the returned value. The fix is `wrap_under_create_rule`, which consumes
        // the `+1`; it is deliberately not applied in this review because an over-release would
        // convert a leak into a use-after-free and there is no way to exercise it here.
        // See docs/13-unsafe-review.md #12.
        unsafe {
            let cf_ref = ax_call(|value_ref: *mut CFTypeRef| {
                let attr = CFString::new(attr);
                let attr_ref = attr.as_concrete_TypeRef();
                AXUIElementCopyAttributeValue(self.get_ref(), attr_ref, value_ref)
            })?;
            Ok(CFType::wrap_under_get_rule(cf_ref))
        }
    }

    fn get_attr<T: ConcreteCFType>(&self, attr: &str) -> Result<T> {
        self.get_attr_ref(attr)?.downcast::<T>().ok_or(-1)
    }

    fn set_attr<T: ConcreteCFType>(&self, attr: &str, value: T::Ref) -> Result<()> {
        // SAFETY:
        // Operations: the `AXUIElementSetAttributeValue` FFI call and `value.as_void_ptr()`
        // (`TCFTypeRef::as_void_ptr`, an unsafe-free but pointer-punning conversion).
        // Contract (HIServices `AXUIElementSetAttributeValue(element, attribute, value)`): all
        // three must be live CF objects; the function does not take ownership of any of them.
        // Evidence:
        // - `self.get_ref()` is live for `&self` (TYPE FACT, as in `get_attr_ref`).
        // - `attr` is a `CFString` local, alive until the end of this block, i.e. past the call.
        // - `value: T::Ref` is a `TCFTypeRef` supplied by the caller. Callers in this file pass
        //   `kCFBooleanTrue`, an immortal CoreFoundation singleton, so liveness holds. This is the one
        //   obligation this private helper places on its callers; it is discharged by inspection because
        //   both call sites are in `enable_screen_reader_accessibility`.
        // Postcondition: no ownership changes; `err == 0` means the attribute was set.
        unsafe {
            let attr = CFString::new(attr);
            let err = AXUIElementSetAttributeValue(self.get_ref(), attr.as_concrete_TypeRef(), value.as_void_ptr());

            if err == 0 { Ok(()) } else { Err(err) }
        }
    }

    pub(crate) fn enable_screen_reader_accessibility(&self) -> Result<()> {
        // SAFETY:
        // Operation: reading the extern static `kCFBooleanTrue` (twice). `set_attr` itself is
        // safe; the block exists solely because CoreFoundation's boolean singletons are
        // declared in an `unsafe extern` block by core-foundation 0.10.
        // Contract (Rust Reference, "accesses to mutable statics or extern statics"): the
        // static must be initialized and not concurrently mutated.
        // Evidence: `kCFBooleanTrue` is initialized by CoreFoundation before `main` and is
        // documented as an immortal singleton; nothing in this process writes to it.
        unsafe {
            let res = self.set_attr::<CFBoolean>(kAXManualAccessibilityAttribute, kCFBooleanTrue);

            if matches!(res, Err(kAXErrorAttributeUnsupported)) {
                self.set_attr::<CFBoolean>(kAXEnhancedUserInterfaceAttribute, kCFBooleanTrue)
            } else {
                res
            }
        }
    }

    fn is_focused(&self) -> Result<bool> {
        let focused = self.get_attr::<CFBoolean>(kAXFocusedAttribute)?;
        Ok(focused.into())
    }

    fn role(&self) -> Result<CFString> {
        self.get_attr::<CFString>(kAXRoleAttribute)
    }

    pub(crate) fn is_fullscreen(&self) -> Result<bool> {
        self.get_attr::<CFBoolean>(kAXFullScreenAttribute).map(|res| res.into())
    }

    pub fn frame(&self) -> Result<CGRect> {
        self.get_attr::<AXValue>(kAXFrameAttribute)?.as_rect().ok_or(-1)
    }

    pub(crate) fn focused_window(&self) -> Result<Self> {
        let window = self.get_attr::<AXUIElement>(kAXFocusedWindowAttribute)?;
        Ok(window.into())
    }

    fn dom_class_list(&self) -> Result<Vec<String>> {
        let class_list = self.get_attr::<CFArray>(kAXDOMClassListAttribute)?;
        let filtered: Vec<_> = class_list
            .iter()
            .filter_map(|attr| unsafe {
                // SAFETY:
                // Operations: the `*const c_void -> *mut objc::runtime::Object` cast and
                // `NSStringRef::new`, an unsafe fn.
                // Contract of `NSStringRef::new` (see its `# Safety`): the pointer must be null
                // or a live `NSString` that stays live for the returned value's lifetime.
                // Evidence:
                // - `class_list` is a `CFArray` returned by `get_attr_ref`, alive for this whole expression, and
                //   `CFArray::iter` yields references into it, so each element is live while the `NSStringRef`
                //   exists.
                // - `kAXDOMClassListAttribute` is documented by WebKit's AX implementation to be a `CFArray` of
                //   `CFString`. `CFString` and `NSString` are toll-free bridged (Apple, "Toll-Free Bridged Types"),
                //   which is the premise that makes the pointer cast to an ObjC object legitimate.
                //   UNPROVEN-BUT-LIKELY: there is no runtime check that the array really holds strings; a
                //   non-string element would send `-UTF8String` to an object that may not respond.
                // - The borrow ends inside this closure: `to_owned()` copies before the `NSStringRef` and the array
                //   element go out of scope.
                let x = NSStringRef::new(*attr as *mut objc::runtime::Object);
                x.as_str().map(|x| x.to_owned())
            })
            .collect();

        Ok(filtered)
    }

    fn attribute_list(&self) -> Result<Vec<String>> {
        // SAFETY:
        // Operations: `ax_call::<_, CFArrayRef>` (ending in `assume_init`) and the
        // `AXUIElementCopyAttributeNames` FFI call.
        // Contract (HIServices): on `kAXErrorSuccess` it writes a `+1` `CFArrayRef` of
        // `CFString` through the out-parameter and writes nothing otherwise; `ax_call` only
        // `assume_init`s on success, and `CFArrayRef` is a raw pointer so any written value is
        // valid at that type.
        // Evidence: `self.get_ref()` is live for `&self` (TYPE FACT).
        // Leak note: `NSArrayRef::from(CFArrayRef)` is *non-owning*, so the `+1` from the Copy
        // rule is never released — one leaked array per call. `print_all_attribute_values`, the
        // only caller, is `#[allow(dead_code)]` debug tooling.
        // See docs/13-unsafe-review.md #6.
        let attrs: NSArrayRef<NSStringRef> =
            unsafe { ax_call(|names: *mut CFArrayRef| AXUIElementCopyAttributeNames(self.get_ref(), names))?.into() };
        let filtered: Vec<_> = attrs
            .iter()
            // SAFETY:
            // Operations: `NSStringRef::new` and, through `Deref`, the read of the element
            // pointer.
            // Contract of `NSStringRef::new`: null, or a live `NSString` for the returned
            // value's lifetime.
            // Evidence: the array is leaked (see above) and therefore outlives every element
            // reference derived from it; HIServices documents the array as containing
            // `CFString`s, which are toll-free bridged to `NSString`. `to_owned()` copies
            // before the borrow ends.
            .filter_map(|attr| unsafe { NSStringRef::new(*attr).as_str().map(|s| s.to_owned()) })
            .collect();

        Ok(filtered)
    }

    #[allow(dead_code)]
    pub fn print_all_attribute_values(&self) {
        if let Ok(attrs) = self.attribute_list() {
            for key in attrs {
                if let Ok(value) = self.get_attr_ref(key.as_str()) {
                    let value_str = if let Some(s) = value.downcast::<CFString>() {
                        format!("{s:?}")
                    } else if let Some(cf_b) = value.downcast::<CFBoolean>() {
                        let b: bool = cf_b.into();
                        format!("{b:?}")
                    } else if let Some(ax) = value.downcast::<AXValue>() {
                        format!("{ax:?}")
                    } else {
                        format!("Unknown {{ type_id: {:?} }}", value.type_of())
                    };

                    warn!("{key}: {value_str}");
                }
            }
        }
    }

    fn children(&self) -> Result<Vec<UIElement>> {
        let children: Vec<_> = self
            .get_attr::<CFArray<*const c_void>>(kAXChildrenAttribute)?
            .iter()
            .map(|child| {
                // SAFETY:
                // Operations: `AXUIElementRef::from_void_ptr(*child)` (core-foundation 0.10
                // `TCFTypeRef::from_void_ptr`, an unsafe pointer reinterpretation) and
                // `UIElement::from`.
                // Contract of `from_void_ptr`: the pointer must be a valid reference of the
                // target CF type. Contract of `UIElement::from`: live, non-null element.
                // Evidence: `kAXChildrenAttribute` is documented by HIServices to be a
                // `CFArray` of `AXUIElementRef`, and the `CFArray<*const c_void>` returned by
                // `get_attr` is alive for the whole `map`, so each element pointer is live at
                // the moment `UIElement::from` retains it (Get rule) — which is what keeps the
                // child alive after the array is dropped.
                // UNPROVEN-BUT-LIKELY: the element type is an Apple documentation lemma, not a
                // checked fact; a non-`AXUIElement` entry would be retained as one and later
                // passed to AX functions. `CFArray::iter` yields raw `*const c_void` here
                // precisely because the element type is erased.
                UIElement::from(unsafe { AXUIElementRef::from_void_ptr(*child) })
            })
            .collect();

        Ok(children)
    }

    pub fn find_x_term_caret_tree(&self) -> Result<Vec<UIElement>> {
        if self.role().map(|role| role == kAXTextFieldRole).unwrap_or(false)
            && self.is_focused().unwrap_or(false)
            && let Ok(classes) = self.dom_class_list()
            && classes.iter().any(|cls| cls == "xterm-helper-textarea")
        {
            // self.print_all_attribute_values();
            return Ok(vec![self.clone()]);
        }

        let mut children_with_cursor: Vec<_> = self
            .children()?
            .into_iter()
            .filter_map(|elem| {
                elem.role().ok().and_then(|role| {
                    let role: std::borrow::Cow<'_, str> = (&role).into();
                    if XTERM_ROLES.contains(&role.as_ref()) {
                        elem.find_x_term_caret_tree().ok()
                    } else {
                        None
                    }
                })
            })
            .collect();

        if children_with_cursor.len() > 1 {
            warn!("Found multiple candidate cursors");
        }

        let mut tree = children_with_cursor.pop().ok_or(-1)?;
        tree.push(self.clone());
        Ok(tree)
    }

    pub fn window_info(&self, all_windows: bool) -> Option<CGWindowInfo> {
        // SAFETY:
        // Operations in this block: (a) `self.get_window_id()`, an unsafe fn; (b) reads of the
        // extern statics `kCGWindowListOption*`, `kCGNullWindowID`, `kCGWindowNumber`,
        // `kCGWindowOwnerPID`, `kCGWindowBounds`, `kCGWindowLayer`; (c) the
        // `CGWindowListCopyWindowInfo` FFI call; (d)
        // `CFArray::<CFDictionary>::wrap_under_create_rule`.
        //
        // (a) Contract: see `get_window_id`'s `# Safety`. Evidence: `self` is a live
        // `UIElement`, and the SPI premise is inherited.
        // (b) Contract (Rust Reference, extern statics): initialized and not concurrently
        // mutated. Evidence: CoreGraphics initializes these before `main` and they are
        // immortal constants.
        // (c) Contract (CoreGraphics `CGWindowListCopyWindowInfo(option, relativeToWindow)`):
        // returns a `+1` `CFArrayRef` of `CFDictionaryRef`, or NULL on failure. No pointer
        // preconditions.
        // (d) Contract of `wrap_under_create_rule`: non-null (asserted) and a live `+1` CF
        // object whose ownership transfers to the wrapper. This is the correct rule here —
        // unlike `get_attr_ref` above — because `CGWindowListCopyWindowInfo` follows the Copy
        // rule.
        // Residual risk: a NULL return from (c) trips the `assert!` inside
        // `wrap_under_create_rule` and panics. That is a robustness defect (an FFI failure
        // becomes a crash in an `Option`-returning function), not UB.
        // See docs/13-unsafe-review.md, adjacent smells.
        //
        // The element type of the array is an Apple documentation lemma: the dictionaries are
        // read only through `CFDictionary::find` + `downcast`, which are dynamically checked,
        // so a surprise element type yields `None` rather than UB.
        unsafe {
            let window_id = self.get_window_id().ok()?;
            let windows = if all_windows {
                CFArray::<CFDictionary>::wrap_under_create_rule(display::CGWindowListCopyWindowInfo(
                    window::kCGWindowListOptionAll,
                    window::kCGNullWindowID,
                ))
            } else {
                CFArray::<CFDictionary>::wrap_under_create_rule(display::CGWindowListCopyWindowInfo(
                    window::kCGWindowListOptionOnScreenOnly
                        | window::kCGWindowListExcludeDesktopElements
                        | window::kCGWindowListOptionIncludingWindow,
                    window_id,
                ))
            };

            let window = windows.iter().find(|window| {
                get_num(window, kCGWindowNumber)
                    .map(|id| id == (window_id as i64))
                    .unwrap_or(false)
            })?;

            let owner_pid = get_num(&window, kCGWindowOwnerPID)?;
            let bounds = get_value::<CFDictionary>(&window, kCGWindowBounds)?;
            let bounds_rect = CGRect::from_dict_representation(&bounds)?;
            let level = get_num(&window, kCGWindowLayer)?;

            Some(CGWindowInfo {
                owner_pid: owner_pid as u64,
                window_id,
                bounds: bounds_rect,
                level,
            })
        }
    }
}

fn get_value<T: ConcreteCFType>(dict: &CFDictionary, key: CFStringRef) -> Option<T> {
    let val_ref = dict.find(key as CFTypeRef)?;
    // SAFETY:
    // Operation: `CFType::wrap_under_get_rule(*val_ref)`.
    // Contract: non-null (asserted inside) and live; performs `CFRetain`.
    // Evidence:
    // - `dict.find` returns `Some(&CFTypeRef)` only for a key present in the dictionary
    //   (core-foundation 0.10), and CFDictionary values are owned by the dictionary, so the value is
    //   live for as long as `dict` is borrowed here — which covers this call.
    // - The Get rule is the right one: `find` does not transfer ownership, so retaining and later
    //   releasing is balanced. Unlike `get_attr_ref`, this site does not leak.
    // Postcondition: `cf_type` owns one reference, independent of the dictionary's; the
    // subsequent `downcast` is a dynamically checked `CFGetTypeID` comparison, so the concrete
    // type is not assumed.
    let cf_type = unsafe { CFType::wrap_under_get_rule(*val_ref) };
    cf_type.downcast::<T>()
}

fn get_num(dict: &CFDictionary, key: CFStringRef) -> Option<i64> {
    let num = get_value::<CFNumber>(dict, key)?;
    match num.to_i32() {
        Some(num) => Some(num as i64),
        None => num.to_i64(),
    }
}

// SAFETY:
// Operation: an `unsafe extern "C"` block, i.e. the assertion that these declarations match the
// symbols the linker will bind.
// Contract (Rust Reference, "external blocks"): the declared signature, ABI and linkage must
// match the actual definition, and the foreign function must uphold the declared behaviour.
// Evidence:
// - `CGWindowLevelForKey` is declared in CoreGraphics' `CGWindowLevel.h` as `int32_t
//   CGWindowLevelForKey(CGWindowLevelKey key)`, where `CGWindowLevelKey` is an `int32_t`-backed
//   enum. `i32 -> i32` with the C ABI is therefore the matching Rust signature.
// - The `#[link]` attribute names the framework that defines it, so no other object can supply an
//   incompatible symbol.
// - It is a pure lookup over an enum: passing an out-of-range key returns a level, not UB, which is
//   why the declaration is not marked `unsafe fn`… except that every `extern` fn is unsafe to call.
//   The one in-crate caller passes `kCGFloatingWindowLevelKey` (5).
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGWindowLevelForKey(key: i32) -> i32;
}
