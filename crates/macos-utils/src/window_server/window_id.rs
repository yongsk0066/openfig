//! Optional private SPI boundary. Missing symbols disable window matching instead of dyld abort.
use std::sync::OnceLock;

use accessibility_sys::{
    AXError,
    AXUIElementRef,
    kAXErrorNoValue,
    kAXErrorNotImplemented,
};
use core_graphics::window::CGWindowID;

type GetWindow = unsafe extern "C" fn(AXUIElementRef, *mut CGWindowID) -> AXError;

/// # Safety
/// `element` must be a live AXUIElement. The private symbol's ABI remains an OS assumption.
pub(super) unsafe fn get(element: AXUIElementRef) -> Result<CGWindowID, AXError> {
    static FUNCTION: OnceLock<Option<GetWindow>> = OnceLock::new();
    let function = FUNCTION
        .get_or_init(|| {
            // SAFETY: the NUL-terminated symbol is looked up in loaded system frameworks.
            // Do not unload its owning image. Null is explicitly treated as unavailable.
            let address = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"_AXUIElementGetWindow".as_ptr()) };
            if address.is_null() {
                None
            } else {
                // SAFETY: this is the private ABI from the vendored HIServices declaration.
                // Runtime lookup checks availability, not signature compatibility.
                Some(unsafe { std::mem::transmute::<*mut libc::c_void, GetWindow>(address) })
            }
        })
        .ok_or(kAXErrorNotImplemented)?;
    let mut id = 0;
    // SAFETY: caller owns a live element; output is initialized and correctly aligned.
    let status = unsafe { function(element, &mut id) };
    checked_id(status, id)
}
fn checked_id(status: AXError, id: CGWindowID) -> Result<CGWindowID, AXError> {
    if status != 0 {
        Err(status)
    } else if id == 0 {
        Err(kAXErrorNoValue)
    } else {
        Ok(id)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_without_a_valid_window_is_rejected() {
        assert_eq!(checked_id(0, 0), Err(kAXErrorNoValue));
        assert_eq!(checked_id(0, 19), Ok(19));
        assert_eq!(checked_id(-1, 19), Err(-1));
    }
}
