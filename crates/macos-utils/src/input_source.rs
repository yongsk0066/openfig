//! Public Carbon TIS activation, requested by the desktop's AppKit event loop.
use core_foundation::array::CFArray;
use core_foundation::base::{
    CFType,
    CFTypeRef,
    TCFType,
};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{
    CFDictionary,
    CFDictionaryRef,
};
use core_foundation::string::{
    CFString,
    CFStringRef,
};
use objc2::MainThreadMarker;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    static kTISPropertyInputSourceID: CFStringRef;
    static kTISPropertyInputSourceCategory: CFStringRef;
    static kTISCategoryPaletteInputSource: CFStringRef;
    static kTISPropertyInputSourceIsEnabled: CFStringRef;
    static kTISPropertyInputSourceIsSelected: CFStringRef;
    fn TISCreateInputSourceList(properties: CFDictionaryRef, include_all: u8) -> core_foundation::array::CFArrayRef;
    fn TISEnableInputSource(source: CFTypeRef) -> i32;
    fn TISSelectInputSource(source: CFTypeRef) -> i32;
    fn TISDeselectInputSource(source: CFTypeRef) -> i32;
    fn TISGetInputSourceProperty(source: CFTypeRef, key: CFStringRef) -> CFTypeRef;
}

fn boolean_property(source: &CFType, key: CFStringRef) -> bool {
    // SAFETY: source is retained by the caller; Carbon's property is a borrowed CF object.
    let value = unsafe { TISGetInputSourceProperty(source.as_CFTypeRef(), key) };
    if value.is_null() {
        return false;
    }
    let value = unsafe { CFType::wrap_under_get_rule(value) };
    value.downcast::<CFBoolean>().is_some_and(bool::from)
}

/// A main-thread-only selection lease. Dropping it restores an approved palette even
/// if the owning recovery operation is canceled; keyboard layouts are never selected.
#[derive(Debug)]
pub struct PaletteReselection {
    source: Option<CFType>,
    _main_thread: MainThreadMarker,
}
impl PaletteReselection {
    pub fn begin(main_thread: MainThreadMarker, source_id: &str) -> Result<Option<Self>, i32> {
        // SAFETY: immutable Carbon constants; get-rule wrappers retain the strings.
        let (id_key, category_key, palette) = unsafe {
            (
                CFString::wrap_under_get_rule(kTISPropertyInputSourceID),
                CFString::wrap_under_get_rule(kTISPropertyInputSourceCategory),
                CFString::wrap_under_get_rule(kTISCategoryPaletteInputSource),
            )
        };
        let filter = CFDictionary::from_CFType_pairs(&[(id_key, CFString::new(source_id)), (category_key, palette)]);
        // SAFETY: valid documented filter; the Create-owned array is checked before wrapping.
        let raw = unsafe { TISCreateInputSourceList(filter.as_concrete_TypeRef(), 1) };
        if raw.is_null() {
            return Err(-50);
        }
        let sources = unsafe { CFArray::<CFType>::wrap_under_create_rule(raw) };
        if sources.len() != 1 {
            return Ok(None);
        }
        let Some(source) = sources.get(0) else { return Ok(None) };
        // SAFETY: Carbon owns these immutable property keys.
        if !boolean_property(&source, unsafe { kTISPropertyInputSourceIsEnabled }) {
            return Ok(None);
        }
        let selected = boolean_property(&source, unsafe { kTISPropertyInputSourceIsSelected });
        let source = (*source).clone();
        // Normal paletteWillTerminate also clears selection. Both graceful replacement
        // and a crash must restore our enabled palette; neither may grant new approval.
        if selected {
            // SAFETY: exact approved palette, retained across the call on its AppKit thread.
            let status = unsafe { TISDeselectInputSource(source.as_CFTypeRef()) };
            if status != 0 {
                return Err(status);
            }
        }
        Ok(Some(Self {
            source: Some(source),
            _main_thread: main_thread,
        }))
    }

    pub fn finish(mut self) -> Result<(), i32> {
        self.restore()
    }

    fn restore(&mut self) -> Result<(), i32> {
        let Some(source) = self.source.take() else {
            return Ok(());
        };
        // Do not undo an explicit user disable while the lease was outstanding.
        if !boolean_property(&source, unsafe { kTISPropertyInputSourceIsEnabled }) {
            return Ok(());
        }
        // SAFETY: the lease retains the palette and cannot leave its owning main thread.
        let status = unsafe { TISSelectInputSource(source.as_CFTypeRef()) };
        if status == 0 { Ok(()) } else { Err(status) }
    }
}
impl Drop for PaletteReselection {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Returns only the activation request's OSStatus, never a claim that approval persisted.
/// The caller must check enabled/selected from a fresh process after the user decides.
pub fn request_activation(_main_thread: MainThreadMarker, source_id: &str) -> Result<(), i32> {
    // SAFETY: Carbon owns the immutable property key. Get-rule wrappers retain it.
    let key = unsafe { CFString::wrap_under_get_rule(kTISPropertyInputSourceID) };
    let filter = CFDictionary::from_CFType_pairs(&[(key, CFString::new(source_id))]);
    // SAFETY: the live dictionary contains the documented string/string filter. Carbon
    // returns a Create-owned array of TIS references; null is checked before wrapping.
    let raw = unsafe { TISCreateInputSourceList(filter.as_concrete_TypeRef(), 1) };
    if raw.is_null() {
        return Err(-50);
    }
    let sources = unsafe { CFArray::<CFType>::wrap_under_create_rule(raw) };
    if sources.len() != 1 {
        return Err(-50);
    }
    let Some(source) = sources.get(0) else { return Err(-50) };
    // SAFETY: this is the sole TIS source returned for the exact identifier. The array
    // retains it through this synchronous call on the desktop's AppKit thread.
    let status = unsafe { TISEnableInputSource(source.as_CFTypeRef()) };
    if status == 0 { Ok(()) } else { Err(status) }
}
