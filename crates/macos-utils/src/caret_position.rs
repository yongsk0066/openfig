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
    AXValueCreate,
    AXValueGetValue,
    AXValueRef,
    kAXBoundsForRangeParameterizedAttribute,
    kAXValueTypeCFRange,
    kAXValueTypeCGRect,
};
use core_foundation::base::{
    CFRange,
    CFType,
    CFTypeRef,
    TCFType,
    TCFTypeRef,
};
use core_foundation::string::CFString;
use core_graphics::geometry::CGRect;
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
/// `bool`. `extend_range == false` is always the cheaper and leak-free path.
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
    //   `size_of::<CFRange>()` bytes and correctly aligned (TYPE FACT), matching `kAXValueTypeCFRange`.
    // - `CFRange` is two `CFIndex` (`isize`) fields, so every bit pattern written is a valid `CFRange`
    //   — the `assume_init` cannot produce an invalid value even if the premise about "writes exactly
    //   on `true`" were wrong; only uninitialized-read UB would remain.
    // The comment below is the original author's: `ax_call` is needed because the out-parameter
    // must be a raw pointer into storage the FFI call can write.
    let selected_range_result: Result<CFRange, bool> = bool_ax_call(|x: *mut CFRange| {
        AXValueGetValue(
            selected_range_value.as_concrete_TypeRef() as AXValueRef,
            kAXValueTypeCFRange,
            x as *mut _ as *mut c_void,
        )
    });

    let selected_text_range: CFRange = match selected_range_result {
        Ok(selected_text_range) => selected_text_range,
        Err(err) => {
            error!("Couldn't get selected text range, did types match {:?}", err);
            return INVALID_CARET_POSITION;
        },
    };

    // SAFETY:
    // Operation (taken only when `extend_range`): the `AXValueCreate` FFI call.
    // Contract (HIServices `AXValueCreate(theType, valuePtr)`): `valuePtr` must point to a
    // readable value whose layout matches `theType`; the result follows the **Create rule**
    // (`+1`) and may be NULL.
    // Evidence:
    // - `updated_range` is a live `CFRange` local, so `&updated_range as *const _ as *const c_void` is
    //   non-null, aligned and initialized for the whole call. It stays alive until the end of the
    //   enclosing function, which covers the use of the resulting `AXValueRef` below.
    // - `kAXValueTypeCFRange` matches the `CFRange` layout, the same premise as above.
    // Postcondition / LEAK: the `+1` is dropped on the floor — `as_void_ptr()` erases it to a
    // raw `CFTypeRef` and nothing ever releases it. One `AXValue` leaks per caret update on the
    // xterm path. See docs/13-unsafe-review.md #6.
    // A NULL result would be passed to `AXUIElementCopyParameterizedAttributeValue` below,
    // which returns an error rather than crashing; that is unchecked but not UB.
    //
    // The `else` branch performs no unsafe operation: `as_concrete_TypeRef` follows the Get rule
    // and borrows from `selected_range_value`, which outlives the use below.
    let selected_range_value_2 = if extend_range {
        let updated_range = CFRange::init(selected_text_range.location, 1);
        AXValueCreate(kAXValueTypeCFRange, &updated_range as *const _ as *const c_void).as_void_ptr()
    } else {
        selected_range_value.as_concrete_TypeRef()
    };

    // https://linear.app/fig/issue/ENG-109/ - autocomplete-popup-shows-when-copying-and-pasting-in-terminal
    if selected_text_range.length > 1 {
        error!("selectedRange length > 1");
        return INVALID_CARET_POSITION;
    }

    // SAFETY:
    // Operations: `ax_call::<_, CFTypeRef>` (ending in `assume_init`) and the
    // `AXUIElementCopyParameterizedAttributeValue` FFI call.
    // Contract (HIServices): `element`, `parameterizedAttribute` and `parameter` must be live
    // CF objects; on `kAXErrorSuccess` a `+1` value is written through the out-parameter, and
    // nothing is written otherwise — which is exactly `ax_call`'s requirement.
    // Evidence:
    // - `focused_element` is a live `AXUIElement` owned by this frame.
    // - `CFString::new(...)` is a temporary whose lifetime extends to the end of the enclosing
    //   statement, i.e. past the FFI call.
    // - `selected_range_value_2` is live on both branches: either the leaked `AXValueCreate` result, or
    //   a borrow of `selected_range_value`, which is still owned by this frame.
    // - `CFTypeRef` is a raw pointer, so any written value is valid at that type.
    // LEAK: the `+1` written into `select_bounds` is never released — it is used as a raw
    // `AXValueRef` and dropped. One leak per caret update. See docs/13-unsafe-review.md #6.
    let select_bounds_result = ax_call(|x: *mut CFTypeRef| {
        AXUIElementCopyParameterizedAttributeValue(
            focused_element.as_concrete_TypeRef(),
            CFString::new(kAXBoundsForRangeParameterizedAttribute).as_concrete_TypeRef(),
            selected_range_value_2,
            x,
        )
    });

    let select_bounds: AXValueRef = match select_bounds_result {
        Ok(select_bounds) => select_bounds as AXValueRef,
        Err(err) => {
            error!("Selected bounds error, error code {:?}", err);
            return INVALID_CARET_POSITION;
        },
    };

    // SAFETY:
    // Operations: `bool_ax_call::<_, CGRect>` and the `AXValueGetValue` FFI call.
    // Contract: as for the `CFRange` call above — write iff `true`, `valuePtr` sized for
    // `theType`.
    // Evidence:
    // - `select_bounds` is the live `+1` value produced by the previous call (leaked, hence certainly
    //   still alive).
    // - `x` points at `MaybeUninit<CGRect>` storage in `bool_ax_call`'s frame, matching
    //   `kAXValueTypeCGRect`.
    // - `CGRect` is four `CGFloat` (`f64`) fields; every bit pattern is a valid `f64` (including NaN),
    //   so `assume_init` cannot produce an invalid value.
    let selected_rect_result =
        bool_ax_call(|x: *mut CGRect| AXValueGetValue(select_bounds, kAXValueTypeCGRect, x.cast()));

    let select_rect = match selected_rect_result {
        Ok(select_rect) => select_rect,
        Err(err) => {
            error!("Couldn't get selected range, did types match {:?}", err);
            return INVALID_CARET_POSITION;
        },
    };
    // Sanity check: prevents flashing autocomplete in bottom corner
    if select_rect.size.width == 0.0 && select_rect.size.height == 0.0 {
        error!("Prevents flashing autocomplete in bottom corner");
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
