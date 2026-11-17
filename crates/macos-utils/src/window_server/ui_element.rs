use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;

use accessibility::{
    AXAttribute,
    AXUIElement,
};
use accessibility_sys::{
    AXError,
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

/// An owned AX reference confined to the thread that queries it. Worker messages carry
/// numeric identities instead of sharing native objects.
///
/// ```compile_fail,E0277
/// fn require_send<T: Send>() {}
/// require_send::<macos_utils::window_server::UIElement>();
/// ```
///
/// ```compile_fail,E0277
/// fn require_sync<T: Sync>() {}
/// require_sync::<macos_utils::window_server::UIElement>();
/// ```
pub struct UIElement(AXUIElement);

impl Clone for UIElement {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl fmt::Debug for UIElement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Formatting must not perform synchronous AX IPC on a logging path.
        f.debug_struct("UIElement").finish_non_exhaustive()
    }
}

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
    /// Cache attribute names per thread: CF wrappers are not Sync. Return owned clones
    /// before AX IPC, which may reenter a callback on the same run loop.
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
    /// `_AXUIElementGetWindow` is a private HIServices SPI. The caller relies on its
    /// declaration matching the OS ABI and accepting this live AX reference. The output
    /// is initialized, so a success without a write is rejected rather than read as UB.
    pub unsafe fn get_window_id(&self) -> Result<CGWindowID> {
        // SAFETY: self owns the live reference; the optional SPI boundary validates the result.
        unsafe { super::window_id::get(self.get_ref()) }
    }

    fn get_attr_ref(&self, attr: &'static str) -> Result<CFType> {
        let attribute = cached_attribute(attr);
        self.0.attribute(&attribute).map_err(|err| match err {
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

    pub fn is_fullscreen(&self) -> Result<bool> {
        self.get_attr::<CFBoolean>(kAXFullScreenAttribute).map(|res| res.into())
    }

    pub fn frame(&self) -> Result<CGRect> {
        self.get_attr::<AXValue>(kAXFrameAttribute)?.as_rect().ok_or(-1)
    }

    pub fn focused_window(&self) -> Result<Self> {
        let window = self.get_attr::<AXUIElement>(kAXFocusedWindowAttribute)?;
        Ok(window.into())
    }

    /// Validate an IMK session's input target once on focus changes, not on each keystroke.
    /// A search field in a terminal app must not receive shell completions.
    pub fn focused_text_area_window_id(&self, budget: &mut crate::query_budget::QueryBudget) -> Option<u32> {
        let focused: AXUIElement = self.query(budget, || self.0.attribute(&AXAttribute::focused_ui()).ok())?;
        let focused = Self(focused);
        if focused.query(budget, || focused.role().ok())? != accessibility_sys::kAXTextAreaRole {
            return None;
        }
        let window = self.query(budget, || self.focused_window().ok())?;
        // SAFETY: the window owns a live AX reference; inherit the optional window-ID SPI contract.
        let id = window.query(budget, || unsafe { window.get_window_id().ok() })?;
        let current: AXUIElement = self.query(budget, || self.0.attribute(&AXAttribute::focused_ui()).ok())?;
        (focused.0 == current).then_some(id)
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
        let children = self.get_attr::<CFArray<*const c_void>>(kAXChildrenAttribute)?;
        children
            .iter()
            // The search starts at the last sibling. Bound local conversion/allocation too,
            // not only AX IPC; pathological remote arrays must not monopolize the worker.
            .skip((children.len() as usize).saturating_sub(512))
            .map(|child| {
                if child.is_null() {
                    return Err(kAXErrorNoValue);
                }
                // SAFETY: the AX CFArray owns live CF objects during iteration. Retain
                // generically, then check the concrete type before calling any AX API.
                let value = unsafe { CFType::wrap_under_get_rule(*child) };
                value
                    .downcast_into::<AXUIElement>()
                    .map(UIElement::from)
                    .ok_or(kAXErrorNoValue)
            })
            .collect()
    }

    pub fn query<T>(
        &self,
        budget: &mut crate::query_budget::QueryBudget,
        operation: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        budget.run(&self.0, operation)
    }

    pub fn find_x_term_caret_tree(&self, budget: &mut crate::query_budget::QueryBudget) -> Option<Vec<UIElement>> {
        self.find_x_term_caret_tree_at(budget, 0)
    }

    fn find_x_term_caret_tree_at(
        &self,
        budget: &mut crate::query_budget::QueryBudget,
        depth: usize,
    ) -> Option<Vec<UIElement>> {
        // Bound native work and Rust stack use even for a cyclic or unusually deep AX tree.
        if depth >= 64 || budget.exhausted() {
            return None;
        }
        let role = self.query(budget, || self.role().ok())?;
        if role == kAXTextFieldRole
            && self.query(budget, || self.is_focused().ok()) == Some(true)
            && self
                .query(budget, || self.dom_class_list().ok())
                .is_some_and(|classes| classes.iter().any(|class| class == "xterm-helper-textarea"))
        {
            return Some(vec![self.clone()]);
        }
        let children = self.query(budget, || self.children().ok())?;
        // Preserve the former last-match choice without scanning other matching subtrees.
        for child in children.into_iter().rev() {
            if budget.exhausted() {
                break;
            }
            let Some(role) = child.query(budget, || child.role().ok()) else {
                continue;
            };
            let role: std::borrow::Cow<'_, str> = (&role).into();
            if XTERM_ROLES.contains(&role.as_ref())
                && let Some(mut path) = child.find_x_term_caret_tree_at(budget, depth + 1)
            {
                path.push(self.clone());
                return Some(path);
            }
            if budget.exhausted() {
                break;
            }
        }
        None
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
