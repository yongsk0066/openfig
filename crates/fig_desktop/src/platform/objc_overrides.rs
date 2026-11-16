//! Keep Wry's runtime class intact: AppKit/WebKit cache class-dependent view properties.
use std::ffi::CString;
use std::sync::OnceLock;

use anyhow::{
    Result,
    ensure,
};
use objc2::encode::{
    EncodeArguments,
    EncodeReturn,
};
use objc2::runtime::{
    AnyClass,
    AnyObject,
    Bool,
    MethodImplementation,
    Sel,
};
use objc2::{
    MainThreadMarker,
    msg_send,
    sel,
};
use objc2_app_kit::{
    NSApplication,
    NSEvent,
    NSView,
};

struct Overrides {
    superclass: &'static AnyClass,
    // Identity only; never dereferenced. WindowState owns this view for its whole lifetime.
    autocomplete: usize,
}
static OVERRIDES: OnceLock<Overrides> = OnceLock::new();

extern "C" fn should_delay_window_ordering(this: *mut NSView, _cmd: Sel, event: *mut NSEvent) -> Bool {
    let state = OVERRIDES
        .get()
        .expect("overrides initialized before method registration");
    if this as usize == state.autocomplete {
        return Bool::YES;
    }
    // SAFETY: AppKit supplies a live NSView and NSEvent, and the saved superclass is
    // the superclass of the class owning this IMP, even if the instance is later subclassed.
    unsafe { msg_send![super(this, state.superclass), shouldDelayWindowOrderingForEvent: event] }
}

extern "C" fn mouse_down(this: *mut NSView, _cmd: Sel, event: *mut NSEvent) {
    let state = OVERRIDES
        .get()
        .expect("overrides initialized before method registration");
    if this as usize == state.autocomplete {
        let mtm = MainThreadMarker::new().expect("AppKit mouseDown must run on the main thread");
        NSApplication::sharedApplication(mtm).preventWindowOrdering();
    }
    // SAFETY: the original Wry superclass implements the NSView mouseDown: ABI.
    // Never use this.class().superclass(): a dynamic descendant could recurse into us.
    unsafe {
        let _: () = msg_send![super(this, state.superclass), mouseDown: event];
    }
}

/// Add an inherited method override without changing any object's class or layout.
///
/// # Safety
/// The class must be a subclass of F::Callee and the function must implement this
/// selector's documented ABI/semantics. Call before instances can dispatch the method.
unsafe fn add_method<F: MethodImplementation>(class: &AnyClass, selector: Sel, implementation: F) -> Result<()> {
    let args = F::Arguments::ENCODINGS;
    ensure!(
        selector.name().to_bytes().iter().filter(|byte| **byte == b':').count() == args.len(),
        "selector argument mismatch"
    );
    let mut encoding = format!("{}@:", F::Return::ENCODING_RETURN);
    for argument in args {
        encoding.push_str(&argument.to_string());
    }
    let encoding = CString::new(encoding)?;
    // SAFETY: class is registered and immortal. The selector is interned; IMP and
    // encoding come from the same Rust function type. Runtime copies the encoding.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            (class as *const AnyClass).cast_mut(),
            selector,
            implementation.__imp(),
            encoding.as_ptr(),
        )
    };
    ensure!(
        added.as_bool(),
        "Wry already owns {selector}; refusing to replace its implementation"
    );
    Ok(())
}

pub(super) fn install(object: &AnyObject) -> Result<()> {
    let _mtm = MainThreadMarker::new().expect("WebView setup must run on the main thread");
    if OVERRIDES.get().is_some() {
        return Ok(());
    }
    let class = object.class();
    let superclass = class.superclass().expect("Wry inherits WKWebView");
    let _ = OVERRIDES.set(Overrides {
        superclass,
        autocomplete: object as *const AnyObject as usize,
    });
    // SAFETY: the live Wry view is an NSView subclass. The two callbacks match its
    // inherited AppKit methods. Only autocomplete receives special behavior; other
    // instances forward unchanged. Crucially, object_setClass/set_class is never used.
    unsafe {
        add_method(
            class,
            sel!(shouldDelayWindowOrderingForEvent:),
            should_delay_window_ordering as extern "C" fn(*mut NSView, Sel, *mut NSEvent) -> Bool,
        )?;
        add_method(
            class,
            sel!(mouseDown:),
            mouse_down as extern "C" fn(*mut NSView, Sel, *mut NSEvent),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use objc2::ClassType;
    use objc2::rc::Retained;
    use objc2::runtime::{
        ClassBuilder,
        NSObject,
    };

    use super::*;
    extern "C" fn answer(_this: *mut NSObject, _cmd: Sel) -> usize {
        42
    }
    #[test]
    fn adding_a_method_preserves_instance_class_layout_and_superclass() {
        let class = ClassBuilder::new(c"FigMethodInjectionTest", NSObject::class())
            .unwrap()
            .register();
        // SAFETY: registered NSObject subclass, standard alloc/init ownership convention.
        let object: Retained<NSObject> = unsafe { msg_send![class, new] };
        let size = class.instance_size();
        // SAFETY: class is an NSObject subclass; this new selector takes no arguments
        // and returns NSUInteger, matching answer's Rust encoding.
        unsafe {
            add_method(
                class,
                sel!(figTestAnswer),
                answer as extern "C" fn(*mut NSObject, Sel) -> usize,
            )
            .unwrap();
        }
        assert_eq!(object.class(), class);
        assert_eq!(class.instance_size(), size);
        assert_eq!(class.superclass(), Some(NSObject::class()));
        // SAFETY: installed the typed selector immediately above.
        let value: usize = unsafe { msg_send![&*object, figTestAnswer] };
        assert_eq!(value, 42);
        // SAFETY: same signature; must fail rather than replace an existing method.
        assert!(
            unsafe {
                add_method(
                    class,
                    sel!(figTestAnswer),
                    answer as extern "C" fn(*mut NSObject, Sel) -> usize,
                )
            }
            .is_err()
        );
    }
}
