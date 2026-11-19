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
    use accessibility_sys::{
        AXIsProcessTrustedWithOptions,
        kAXTrustedCheckOptionPrompt,
    };
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    // SAFETY: the framework owns this immutable key. The dictionary retains the
    // key and value and lives through the call, which only requests user consent.
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
    if unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) } {
        return true;
    }

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
    // SAFETY: AXIsProcessTrusted takes no arguments or caller-owned pointers.
    unsafe { AXIsProcessTrusted() }
}
