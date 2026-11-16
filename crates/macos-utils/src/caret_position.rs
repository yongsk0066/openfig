use std::ffi::c_void;

use accessibility::util::{
    ax_call,
    bool_ax_call,
};
use accessibility::{
    AXAttribute,
    AXUIElement,
};
use accessibility_sys::{
    AXUIElementCopyParameterizedAttributeValue,
    AXValue,
    AXValueCreate,
    AXValueGetValue,
    AXValueRef,
    kAXBoundsForRangeParameterizedAttribute,
    kAXValueTypeCFRange,
};
use core_foundation::base::{
    CFRange,
    CFType,
    CFTypeRef,
    TCFType,
};
use core_foundation::string::CFString;
use tracing::{
    debug,
    error,
};

#[derive(Debug)]
pub struct CaretPosition {
    pub valid: bool,
    pub x: f64,
    pub y: f64,
    pub height: f64,
}

const INVALID_CARET_POSITION: CaretPosition = CaretPosition {
    valid: false,
    x: 0.0,
    y: 0.0,
    height: 0.0,
};

/// Read the caret rectangle of the system-wide focused UI element.
///
/// # Safety
///
/// The caller must ensure that:
///
/// 1. the AX API may be used from the calling thread (see the `Sync` discussion on
///    [`crate::window_server::UIElement`] and docs/13-unsafe-review.md #8); and
/// 2. the private-SPI and documentation premises listed below are acceptable, namely that
///    `AXValueGetValue` writes `*value` exactly when it returns `true`, and that
///    `kAXValueTypeCFRange` / `kAXValueTypeCGRect` really select `CFRange` / `CGRect` out-parameter
///    layouts.
///
/// There is no pointer, aliasing or ownership obligation on the caller: every argument is a
/// `bool`. Both paths own and release every Create/Copy result.
///
/// Returns [`INVALID_CARET_POSITION`] on every failure, including "no focused element".
pub unsafe fn get_caret_position(extend_range: bool) -> CaretPosition {
    let system_wide_element: AXUIElement = AXUIElement::system_wide();

    // Get the focused element
    let focused_element: AXUIElement = match system_wide_element.attribute(&AXAttribute::focused_ui()) {
        Ok(focused_element) => focused_element,
        Err(err) => {
            match err {
                accessibility::Error::Ax(-25212) => debug!(%err, "Selected range value did not exist"),
                _ => error!(%err, "Couldn't get selected range value, error code"),
            }

            return INVALID_CARET_POSITION;
        },
    };

    // Get the selected range value
    let selected_range_value: CFType = match focused_element.attribute(&AXAttribute::selected_range()) {
        Ok(selected_range_value) => selected_range_value,
        Err(err) => {
            match err {
                accessibility::Error::Ax(-25212) => debug!(%err, "Selected range value did not exist"),
                _ => error!(%err, "Couldn't get selected range value, error code"),
            }

            return INVALID_CARET_POSITION;
        },
    };

    // SAFETY:
    // Operations: `bool_ax_call::<_, CFRange>` (which ends in
    // `MaybeUninit::<CFRange>::assume_init()`), the `AXValueGetValue` FFI call, and the read of
    // the extern-ish constant `kAXValueTypeCFRange`.
    // Contract of `bool_ax_call`: `assume_init` is only reached when the closure returns `true`,
    // so the closure must have initialized `*x` on that path.
    // Contract of `AXValueGetValue(value, theType, valuePtr)` (HIServices): `value` must be a
    // live `AXValueRef`; `theType` must match the value's actual type; `valuePtr` must point to
    // writable storage of the size that `theType` implies. It returns `true` and writes exactly
    // when the type matches, `false` and writes nothing otherwise.
    // Evidence:
    // - `selected_range_value` is a live `CFType` returned by
    //   `AXUIElement::attribute(&AXAttribute::selected_range())` and owned by this frame; the `as
    //   AXValueRef` cast is the toll-free reinterpretation of the same CF object.
    // - `x` points at `MaybeUninit<CFRange>` storage owned by `bool_ax_call`'s frame, which is
    //   `size_of::<CFRange>()` bytes and correctly aligned (TYPE FACT), matching
    //   `kAXValueTypeCFRange`.
    // - `CFRange` is two `CFIndex` (`isize`) fields, so every bit pattern written is a valid
    //   `CFRange` — the `assume_init` cannot produce an invalid value even if the premise about
    //   "writes exactly on `true`" were wrong; only uninitialized-read UB would remain.
    // The comment below is the original author's: `ax_call` is needed because the out-parameter
    // must be a raw pointer into storage the FFI call can write.
    let selected_range_result: Result<CFRange, bool> = bool_ax_call(|x: *mut CFRange| {
        AXValueGetValue(
            selected_range_value.as_concrete_TypeRef() as AXValueRef,
            kAXValueTypeCFRange,
            x.cast::<c_void>(),
        )
    });

    let selected_text_range: CFRange = match selected_range_result {
        Ok(selected_text_range) => selected_text_range,
        Err(err) => {
            error!("Couldn't get selected text range, did types match {:?}", err);
            return INVALID_CARET_POSITION;
        },
    };

    // A selection is not a caret. Reject before allocating its extended range.
    if selected_text_range.length > 1 {
        debug!("selectedRange length > 1");
        return INVALID_CARET_POSITION;
    }

    // SAFETY: AXValueCreate copies the live CFRange. Its non-null Create result is
    // immediately owned, so errors below release it too. The original remains borrowed.
    let extended_range = if extend_range {
        let updated_range = CFRange::init(selected_text_range.location, 1);
        let raw = AXValueCreate(kAXValueTypeCFRange, std::ptr::from_ref(&updated_range).cast());
        if raw.is_null() {
            return INVALID_CARET_POSITION;
        }
        own_ax_value(raw)
    } else {
        None
    };
    let range = extended_range.as_ref().map_or_else(
        || selected_range_value.as_concrete_TypeRef(),
        |value| value.as_CFTypeRef(),
    );

    // SAFETY: The element, attribute name and range live throughout this call.
    // On success the out-parameter follows the Copy rule; take ownership immediately.
    let select_bounds_result = ax_call(|x: *mut CFTypeRef| {
        AXUIElementCopyParameterizedAttributeValue(
            focused_element.as_concrete_TypeRef(),
            CFString::new(kAXBoundsForRangeParameterizedAttribute).as_concrete_TypeRef(),
            range,
            x,
        )
    });

    let select_bounds = match select_bounds_result {
        Ok(raw) if !raw.is_null() => own_ax_value(raw.cast_mut().cast()).unwrap(),
        Ok(_) => return INVALID_CARET_POSITION,
        Err(err) => {
            debug!("Selected bounds error: {err:?}");
            return INVALID_CARET_POSITION;
        },
    };
    let Some(select_rect) = select_bounds.as_rect() else {
        return INVALID_CARET_POSITION;
    };
    // Sanity check: prevents flashing autocomplete in bottom corner
    if select_rect.size.width == 0.0 && select_rect.size.height == 0.0 {
        debug!("Prevents flashing autocomplete in bottom corner");
        return INVALID_CARET_POSITION;
    }

    // Tauri uses Quartz coordinates (don't need to convert coordinates to Cocoa like macos)
    let result = CaretPosition {
        valid: true,
        x: select_rect.origin.x,
        y: select_rect.origin.y,
        height: select_rect.size.height,
    };
    debug!("Got position {result:?}");
    result
}

// SAFETY: `raw` must be null or an AXValue Create/Copy result whose +1 ownership is transferred
// here.
unsafe fn own_ax_value(raw: AXValueRef) -> Option<AXValue> {
    if raw.is_null() {
        None
    } else {
        Some(unsafe { AXValue::wrap_under_create_rule(raw) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_range_is_released_on_normal_and_early_return_paths() {
        // SAFETY: Both Create calls use the matching CFRange layout. The cloned witness retains
        // each value independently so its retain count can be inspected after the owner drops.
        unsafe {
            assert!(own_ax_value(std::ptr::null_mut()).is_none());
            for early_return in [false, true] {
                let range = CFRange::init(17, 1);
                let raw = AXValueCreate(kAXValueTypeCFRange, std::ptr::from_ref(&range).cast());
                assert!(!raw.is_null());
                let witness = AXValue::wrap_under_get_rule(raw);
                let retained = witness.retain_count();
                let read = || {
                    let value = own_ax_value(raw).unwrap();
                    if early_return {
                        return None;
                    }
                    value.as_range()
                };
                assert_eq!(read(), if early_return { None } else { Some(range) });
                assert_eq!(witness.retain_count(), retained - 1);
            }
        }
    }
}
