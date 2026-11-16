use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;

use accessibility::AXAttribute;
use accessibility::util::ax_call;
use accessibility_sys::{
    _AXUIElementGetWindow,
    AXError,
    AXUIElement,
    AXUIElementCreateSystemWide,
    AXUIElementRef,
    AXUIElementSetAttributeValue,
    AXUIElementSetMessagingTimeout,
    AXValue,
    kAXApplicationRole,
    kAXBrowserRole,
    kAXChildrenAttribute,
    kAXDOMClassListAttribute,
    kAXEnhancedUserInterfaceAttribute,
    kAXErrorAttributeUnsupported,
    kAXErrorNoValue,
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
use core_foundation::array::CFArray;
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
use core_graphics::display::CGRect;
use core_graphics::window::{
    self,
    CGWindowID,
    kCGWindowBounds,
    kCGWindowLayer,
    kCGWindowNumber,
    kCGWindowOwnerPID,
};
use tracing::warn;

pub struct UIElement(AXUIElement);

impl Clone for UIElement {
    fn clone(&self) -> Self {
        // SAFETY: self owns a live AX element for the duration of this retain.
        unsafe { UIElement::from_borrowed(self.get_ref()) }
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

// SAFETY: Owns a retained AXUIElement; CF retain/release permit ownership transfer.
// Focus events carry it to the desktop event loop. It owns no observer/run-loop source.
unsafe impl Send for UIElement {}
// SAFETY: The current shared PlatformWindow requires Sync, but concurrent AX attribute calls
// are not established safe merely by CF reference-counting guarantees. This remains an
// unresolved contract; prefer identity-based messages and worker-owned AX handles. See
// docs/architecture.md (native thread ownership).
unsafe impl Sync for UIElement {}

impl From<AXUIElement> for UIElement {
    fn from(ax_ref: AXUIElement) -> Self {
        UIElement(ax_ref)
    }
}

impl UIElement {
    /// Set the process-wide AX timeout without leaking the temporary Create-rule reference.
    pub fn set_global_messaging_timeout(seconds: f32) {
        // SAFETY: Create returns an owned AX element. The wrapper keeps it live during the
        // borrowing timeout call and releases it on return. AX rejects invalid timeout values.
        unsafe {
            let element = AXUIElement::wrap_under_create_rule(AXUIElementCreateSystemWide());
            AXUIElementSetMessagingTimeout(element.as_concrete_TypeRef(), seconds);
        }
    }

    /// Retain a borrowed AX element using the Get rule.
    ///
    /// # Safety
    /// `ax_ref` must point to a live, non-null AXUIElement for the duration of this call.
    /// This takes its own reference; it does not consume a caller's Create-rule reference.
    ///
    /// ```compile_fail,E0133
    /// macos_utils::window_server::UIElement::from_borrowed(std::ptr::null());
    /// ```
    pub unsafe fn from_borrowed(ax_ref: AXUIElementRef) -> Self {
        // SAFETY: The caller guarantees the concrete type and liveness; Get-rule wrapping retains
        // it.
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

thread_local! {
    /// The `AXAttribute`s this file asks for, built once per thread.
    ///
    /// `get_attr`/`set_attr` used to call `CFString::new(attr)` on every attribute access, which
    /// allocates a CoreFoundation string and a UTF-16 copy of the name. `caret_position` reads
    /// three attributes per update, i.e. three allocations per keystroke, and
    /// `find_window_by_bounds` walks a subtree reading `kAXChildrenAttribute` at every level
    /// (F-65). The names are `&'static str` constants, so the strings can be built once.
    ///
    /// A `thread_local!` rather than a `LazyLock`: `CFString` and `AXAttribute` are `!Sync`
    /// (core-foundation makes no thread-safety claim for its wrappers, and `AXAttribute` holds a
    /// `PhantomData<*const T>`), so a `static` would need an `unsafe impl Sync`. Per-thread costs
    /// one map at most — the AX calls all happen on the window-server thread.
    static ATTRIBUTES: RefCell<HashMap<&'static str, AXAttribute<CFType>>> =
        RefCell::new(HashMap::new());
}

/// The cached `AXAttribute` for `attr`, building it on first use on this thread.
///
/// Returns a clone (one `CFRetain`, no allocation) rather than a borrow so that the `RefCell`
/// borrow ends before any accessibility call: `AXUIElementCopyAttributeValue` blocks on the
/// target application and can pump the run loop, which could deliver an `AXObserver` callback
/// that re-enters this function on the same thread — with a live `borrow_mut` that would panic.
fn cached_attribute(attr: &'static str) -> AXAttribute<CFType> {
    ATTRIBUTES.with_borrow_mut(|attributes| {
        attributes
            .entry(attr)
            .or_insert_with(|| AXAttribute::new(&CFString::new(attr)))
            .clone()
    })
}

impl UIElement {
    /// Own the Create-rule application element without retaining an extra leaked reference.
    pub fn application(pid: accessibility_sys::pid_t) -> Self {
        // SAFETY: CreateApplication returns a live +1 application element for a process ID;
        // the Create-rule wrapper consumes precisely that reference and releases it on drop.
        Self(unsafe { AXUIElement::wrap_under_create_rule(accessibility_sys::AXUIElementCreateApplication(pid)) })
    }

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
    /// returns `kAXErrorSuccess`. See docs/architecture.md (native thread ownership).
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

    /// Read an accessibility attribute, delegating to the vendored `AXUIElement::attribute`.
    ///
    /// The vendored attribute accessor owns Copy-rule results; keep that ownership rather
    /// than retaining the same result again. The typed getter below checks the CF type ID.
    fn get_attr_ref(&self, attr: &'static str) -> Result<CFType> {
        let attribute = cached_attribute(attr);

        // SAFETY:
        // Operation: `accessibility::AXUIElement::wrap_under_get_rule`.
        // Contract (core-foundation `TCFType`): the reference must be non-null (asserted
        // inside) and live; the call performs `CFRetain` and the wrapper `CFRelease`s on drop.
        // Evidence: `self.get_ref()` is the `AXUIElementRef` this `UIElement` owns a reference
        // on (TYPE FACT), live for the duration of `&self`.
        // Postcondition: `element` borrows the same element with its own balanced reference;
        // `self`'s reference count is unchanged once it drops at the end of this function.
        let element = unsafe { accessibility::AXUIElement::wrap_under_get_rule(self.get_ref()) };

        element.attribute(&attribute).map_err(|err| match err {
            accessibility::Error::Ax(code) => code,
            accessibility::Error::NotFound => kAXErrorNoValue,
        })
    }

    fn get_attr<T: ConcreteCFType>(&self, attr: &'static str) -> Result<T> {
        self.get_attr_ref(attr)?.downcast::<T>().ok_or(-1)
    }

    fn set_attr<T: ConcreteCFType>(&self, attr: &'static str, value: T::Ref) -> Result<()> {
        // SAFETY:
        // Operations: the `AXUIElementSetAttributeValue` FFI call and `value.as_void_ptr()`
        // (`TCFTypeRef::as_void_ptr`, an unsafe-free but pointer-punning conversion).
        // Contract (HIServices `AXUIElementSetAttributeValue(element, attribute, value)`): all
        // three must be live CF objects; the function does not take ownership of any of them.
        // Evidence:
        // - `self.get_ref()` is live for `&self` (TYPE FACT, as in `get_attr_ref`).
        // - `attribute` owns the `CFString`, and it is a local alive until the end of this
        //   function, i.e. past the call.
        // - `value: T::Ref` is a `TCFTypeRef` supplied by the caller. Callers in this file pass
        //   `kCFBooleanTrue`, an immortal CoreFoundation singleton, so liveness holds. This is the
        //   one obligation this private helper places on its callers; it is discharged by
        //   inspection because both call sites are in `enable_screen_reader_accessibility`.
        // Postcondition: no ownership changes; `err == 0` means the attribute was set.
        let attribute = cached_attribute(attr);
        unsafe {
            let err = AXUIElementSetAttributeValue(
                self.get_ref(),
                attribute.as_CFString().as_concrete_TypeRef(),
                value.as_void_ptr(),
            );

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

    pub(crate) fn role(&self) -> Result<CFString> {
        self.get_attr::<CFString>(kAXRoleAttribute)
    }

    pub(crate) fn is_fullscreen(&self) -> Result<bool> {
        self.get_attr::<CFBoolean>(kAXFullScreenAttribute).map(|res| res.into())
    }

    pub fn frame(&self) -> Result<CGRect> {
        self.get_attr::<AXValue>(kAXFrameAttribute)?.as_rect().ok_or(-1)
    }

    pub fn focused_window(&self) -> Result<Self> {
        let window = self.get_attr::<AXUIElement>(kAXFocusedWindowAttribute)?;
        Ok(window.into())
    }

    fn dom_class_list(&self) -> Result<Vec<String>> {
        let class_list = self.get_attr::<CFArray>(kAXDOMClassListAttribute)?;
        let filtered: Vec<_> = class_list
            .iter()
            .filter_map(|attr| {
                if attr.is_null() {
                    return None;
                }
                // SAFETY: AX owns the live CF objects in class_list for this iteration.
                // Retain as CFType first, then check the runtime type before treating it as
                // a string. Conversion returns owned Rust text; no autorelease borrow escapes.
                let value = unsafe { CFType::wrap_under_get_rule(*attr) };
                value.downcast::<CFString>().map(|value| value.to_string())
            })
            .collect();

        Ok(filtered)
    }

    fn children(&self) -> Result<Vec<UIElement>> {
        let children: Vec<_> = self
            .get_attr::<CFArray<*const c_void>>(kAXChildrenAttribute)?
            .iter()
            .map(|child| {
                // SAFETY:
                // Operations: `AXUIElementRef::from_void_ptr(*child)` (core-foundation 0.10
                // `TCFTypeRef::from_void_ptr`, an unsafe pointer reinterpretation) and
                // `UIElement::from_borrowed`.
                // Contract of `from_void_ptr`: the pointer must be a valid reference of the
                // target CF type. Contract of `UIElement::from_borrowed`: live, non-null element.
                // Evidence: `kAXChildrenAttribute` is documented by HIServices to be a
                // `CFArray` of `AXUIElementRef`, and the `CFArray<*const c_void>` returned by
                // `get_attr` is alive for the whole `map`, so each element pointer is live at
                // the moment `UIElement::from_borrowed` retains it (Get rule) — which is what keeps
                // the child alive after the array is dropped.
                // FFI contract: the element type is an Apple documentation lemma, not a
                // checked fact; a non-`AXUIElement` entry would be retained as one and later
                // passed to AX functions. `CFArray::iter` yields raw `*const c_void` here
                // precisely because the element type is erased.
                unsafe { UIElement::from_borrowed(AXUIElementRef::from_void_ptr(*child)) }
            })
            .collect();

        Ok(children)
    }

    pub fn find_x_term_caret_tree(&self) -> Result<Vec<UIElement>> {
        if self.role().is_ok_and(|role| role == kAXTextFieldRole)
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

    /// Query the known window directly; never enumerate all windows or make an AX round trip.
    pub fn window_info_for_id(window_id: CGWindowID) -> Option<CGWindowInfo> {
        // CGWindowID array entries are pointer-sized integer values, NOT CFNumbers.
        // Use usize storage: CFArray::from_copyable reads pointer-sized slots, so passing
        // a &[u32] on a 64-bit host would read beyond the supplied element.
        let ids = CFArray::from_copyable(&[window_id as usize]);
        // SAFETY: ids is a live CFArray with one correctly encoded window ID and no object
        // retain/release callbacks. CoreGraphics returns a Copy-owned array of dictionaries
        // or NULL; check NULL before taking ownership. The array owns every dictionary while
        // dynamically typed field reads below retain/check each CF value.
        unsafe {
            let raw = window::CGWindowListCreateDescriptionFromArray(ids.as_concrete_TypeRef());
            if raw.is_null() {
                return None;
            }
            let windows = CFArray::<CFDictionary>::wrap_under_create_rule(raw);
            let window = windows
                .iter()
                .find(|item| get_num(item, kCGWindowNumber) == Some(i64::from(window_id)))?;
            Some(CGWindowInfo {
                owner_pid: get_num(&window, kCGWindowOwnerPID)? as u64,
                window_id,
                bounds: CGRect::from_dict_representation(&get_value::<CFDictionary>(&window, kCGWindowBounds)?)?,
                level: get_num(&window, kCGWindowLayer)?,
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
    //   (core-foundation 0.10), and CFDictionary values are owned by the dictionary, so the value
    //   is live for as long as `dict` is borrowed here — which covers this call.
    // - The Get rule is the right one: `find` does not transfer ownership, so retaining and later
    //   releasing is balanced.
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
