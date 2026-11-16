#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]
use std::ffi::c_void;
use std::fmt;

use core_foundation::base::{
    CFRange,
    TCFType,
};
use core_foundation::{
    declare_TCFType,
    impl_TCFType,
};
use core_foundation_sys::base::CFTypeID;
use core_graphics::display::{
    CGPoint,
    CGRect,
    CGSize,
};

pub type AXValueType = u32;
pub const kAXValueTypeCGPoint: u32 = 1;
pub const kAXValueTypeCGSize: u32 = 2;
pub const kAXValueTypeCGRect: u32 = 3;
pub const kAXValueTypeCFRange: u32 = 4;
pub const kAXValueTypeAXError: u32 = 5;
pub const kAXValueTypeIllegal: u32 = 0;

// Opaque macOS object: only pointers cross the FFI boundary.
#[repr(C)]
pub struct __AXValue {
    _private: (),
    _marker: std::marker::PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}
pub type AXValueRef = *mut __AXValue;

declare_TCFType! {
    AXValue, AXValueRef
}
impl_TCFType!(AXValue, AXValueRef, AXValueGetTypeID);

impl AXValue {
    pub fn value_type(&self) -> AXValueType {
        unsafe { AXValueGetType(self.as_concrete_TypeRef()) }
    }

    pub fn as_rect(&self) -> Option<CGRect> {
        if self.value_type() != kAXValueTypeCGRect {
            return None;
        }

        let mut rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 0.0,
                height: 0.0,
            },
        };

        if unsafe {
            AXValueGetValue(
                self.as_concrete_TypeRef(),
                kAXValueTypeCGRect,
                std::ptr::from_mut(&mut rect).cast(),
            )
        } {
            Some(rect)
        } else {
            None
        }
    }

    pub fn as_size(&self) -> Option<CGSize> {
        if self.value_type() != kAXValueTypeCGSize {
            return None;
        }

        let mut size = CGSize {
            width: 0.0,
            height: 0.0,
        };

        if unsafe {
            AXValueGetValue(
                self.as_concrete_TypeRef(),
                kAXValueTypeCGSize,
                std::ptr::from_mut(&mut size).cast(),
            )
        } {
            Some(size)
        } else {
            None
        }
    }

    pub fn as_point(&self) -> Option<CGPoint> {
        if self.value_type() != kAXValueTypeCGPoint {
            return None;
        }

        let mut point = CGPoint { x: 0.0, y: 0.0 };

        if unsafe {
            AXValueGetValue(
                self.as_concrete_TypeRef(),
                kAXValueTypeCGPoint,
                std::ptr::from_mut(&mut point).cast(),
            )
        } {
            Some(point)
        } else {
            None
        }
    }

    pub fn as_range(&self) -> Option<CFRange> {
        if self.value_type() != kAXValueTypeCFRange {
            return None;
        }

        let mut range = CFRange { location: 0, length: 0 };

        if unsafe {
            AXValueGetValue(
                self.as_concrete_TypeRef(),
                kAXValueTypeCFRange,
                std::ptr::from_mut(&mut range).cast(),
            )
        } {
            Some(range)
        } else {
            None
        }
    }
}

impl fmt::Debug for AXValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let type_id = self.value_type();
        let maybe_res = if type_id == kAXValueTypeCGRect {
            self.as_rect().map(|CGRect { origin, size }| {
                let origin = (origin.x, origin.y);
                let size = (size.width, size.height);
                f.debug_struct("Rect")
                    .field("origin", &origin)
                    .field("size", &size)
                    .finish()
            })
        } else if type_id == kAXValueTypeCGSize {
            self.as_size().map(|CGSize { width, height }| {
                f.debug_struct("Size")
                    .field("width", &width)
                    .field("height", &height)
                    .finish()
            })
        } else if type_id == kAXValueTypeCGPoint {
            self.as_point()
                .map(|CGPoint { x, y }| f.debug_struct("Point").field("x", &x).field("y", &y).finish())
        } else if type_id == kAXValueTypeCFRange {
            self.as_range().map(|CFRange { location, length }| {
                f.debug_struct("Range")
                    .field("location", &location)
                    .field("length", &length)
                    .finish()
            })
        } else {
            None
        };

        maybe_res.unwrap_or_else(|| {
            f.debug_struct("Unknown AXValue Type")
                .field("type_id", &type_id)
                .finish()
        })
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXValueGetTypeID() -> CFTypeID;
    pub fn AXValueCreate(theType: AXValueType, valuePtr: *const c_void) -> AXValueRef;
    pub fn AXValueGetType(value: AXValueRef) -> AXValueType;
    pub fn AXValueGetValue(value: AXValueRef, theType: AXValueType, valuePtr: *mut c_void) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    // The caller pairs the concrete value with the corresponding AXValueType.
    unsafe fn native_value<T>(kind: AXValueType, value: &T) -> AXValue {
        let raw = unsafe { AXValueCreate(kind, std::ptr::from_ref(value).cast()) };
        assert!(!raw.is_null());
        unsafe { AXValue::wrap_under_create_rule(raw) }
    }

    #[test]
    fn geometry_round_trips_through_macos_ax_values() {
        let point = CGPoint { x: 12.0, y: -3.0 };
        let size = CGSize {
            width: 640.0,
            height: 480.0,
        };
        let rect = CGRect { origin: point, size };
        // SAFETY: Each tag matches the layout of its input value; AXValueCreate copies it.
        let point_value = unsafe { native_value(kAXValueTypeCGPoint, &point) };
        let size_value = unsafe { native_value(kAXValueTypeCGSize, &size) };
        let rect_value = unsafe { native_value(kAXValueTypeCGRect, &rect) };
        assert_eq!(point_value.as_point().map(|v| (v.x, v.y)), Some((point.x, point.y)));
        assert_eq!(
            size_value.as_size().map(|v| (v.width, v.height)),
            Some((size.width, size.height))
        );
        assert_eq!(
            rect_value
                .as_rect()
                .map(|v| (v.origin.x, v.origin.y, v.size.width, v.size.height)),
            Some((point.x, point.y, size.width, size.height))
        );
        assert!(point_value.as_rect().is_none());
        assert!(size_value.as_range().is_none());
        assert!(rect_value.as_point().is_none());
    }

    #[test]
    fn selected_text_range_round_trips_through_macos_ax_values() {
        let range = CFRange {
            location: 17,
            length: 4,
        };
        // SAFETY: CFRange matches kAXValueTypeCFRange and lives until the copy completes.
        let value = unsafe { native_value(kAXValueTypeCFRange, &range) };
        assert_eq!(value.as_range(), Some(range));
        assert!(value.as_size().is_none());
    }
}
