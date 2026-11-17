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

/// Query the focused element of one application. Every AX/CF handle is local to this call;
/// no shared UIElement or system-wide focus reference crosses a thread boundary.
/// Returns an invalid rectangle on missing focus, unsupported attributes or mismatched types.
pub fn get_caret_position(
    pid: accessibility_sys::pid_t,
    extend_range: bool,
    budget: &mut crate::query_budget::QueryBudget,
) -> CaretPosition {
    let application = AXUIElement::application(pid);
    let Some(focused_element): Option<AXUIElement> =
        budget.run(&application, || application.attribute(&AXAttribute::focused_ui()).ok())
    else {
        return INVALID_CARET_POSITION;
    };
    let Some(selected_range_value): Option<CFType> = budget.run(&focused_element, || {
        focused_element.attribute(&AXAttribute::selected_range()).ok()
    }) else {
        return INVALID_CARET_POSITION;
    };

    let Some(selected_range_value) = selected_range_value.downcast::<AXValue>() else {
        return INVALID_CARET_POSITION;
    };
    // SAFETY: the checked downcast guarantees a live AXValue. bool_ax_call provides
    // correctly sized CFRange storage; AXValueGetValue writes it only on success.
    let selected_range_result: Result<CFRange, bool> = unsafe {
        bool_ax_call(|x: *mut CFRange| {
            AXValueGetValue(
                selected_range_value.as_concrete_TypeRef() as AXValueRef,
                kAXValueTypeCFRange,
                x.cast::<c_void>(),
            )
        })
    };

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
        let raw = unsafe { AXValueCreate(kAXValueTypeCFRange, std::ptr::from_ref(&updated_range).cast()) };
        if raw.is_null() {
            return INVALID_CARET_POSITION;
        }
        unsafe { own_ax_value(raw) }
    } else {
        None
    };
    let range = extended_range
        .as_ref()
        .map_or_else(|| selected_range_value.as_CFTypeRef(), |value| value.as_CFTypeRef());

    // SAFETY: The element, attribute name and range live throughout this call.
    // On success the out-parameter follows the Copy rule; take ownership immediately.
    let select_bounds_result = budget.run(&focused_element, || unsafe {
        ax_call(|x: *mut CFTypeRef| {
            AXUIElementCopyParameterizedAttributeValue(
                focused_element.as_concrete_TypeRef(),
                CFString::new(kAXBoundsForRangeParameterizedAttribute).as_concrete_TypeRef(),
                range,
                x,
            )
        })
        .ok()
        .and_then(|raw| own_ax_value(raw.cast_mut().cast()))
    });

    // Own the Copy result inside the budget operation so an expired deadline also releases it.
    let Some(select_bounds) = select_bounds_result else {
        return INVALID_CARET_POSITION;
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
        // SAFETY: caller transfers a live Create/Copy CF object. Check the concrete type
        // before invoking AXValue methods; a mismatched object is still released normally.
        unsafe { CFType::wrap_under_create_rule(raw.cast()) }.downcast_into::<AXValue>()
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
