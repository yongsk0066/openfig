use accessibility_sys::AXIsProcessTrusted;
use objc2::AllocAnyThread;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{
    NSURL,
    ns_string,
};

static ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

pub fn open_accessibility() -> bool {
    let string = ns_string!(ACCESSIBILITY_SETTINGS_URL);
    let url = NSURL::initWithString(NSURL::alloc(), string);
    if let Some(url) = url {
        let workspace = NSWorkspace::sharedWorkspace();
        workspace.openURL(&url)
    } else {
        false
    }
}

pub fn accessibility_is_enabled() -> bool {
    // SAFETY:
    // Operation: the `AXIsProcessTrusted` FFI call.
    // Contract (HIServices): the function takes no arguments and returns a `Boolean`. It has no
    // preconditions of any kind — no pointers, no state, no thread affinity — so the `unsafe`
    // here is only the blanket rule that calling any `extern` function is unsafe (Rust
    // Reference, "calls to unsafe functions").
    // Evidence: nothing to discharge; the call site passes no values.
    // Postcondition: the returned `Boolean` is 0 or 1; `accessibility_sys` declares the return
    // as `bool`, and the C `Boolean` type is `unsigned char` with those two values in practice.
    // TCB premise: HIServices never returns a `Boolean` other than 0/1, which would be an
    // invalid `bool` under the Rust Reference's validity rules. This is inherited from the
    // `accessibility-sys` binding, which is vendored and out of scope for this review.
    unsafe { AXIsProcessTrusted() }
}
