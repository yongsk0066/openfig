use std::path::Path;

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBitmapImageFileType,
    NSBitmapImageRep,
    NSGraphicsContext,
    NSImage,
    NSWorkspace,
};
use objc2_foundation::{
    NSDictionary,
    NSFileTypeDirectory,
    NSPoint,
    NSRect,
    NSSize,
    NSString,
};

/// # Safety
///
/// The caller must ensure that AppKit may be used from the calling thread for these operations.
/// `NSImage`, `NSImageRep` and `NSBitmapImageRep` are documented as safe to use from secondary
/// threads *for drawing*, but `NSGraphicsContext::currentContext` is thread-local and
/// `+[NSImage bestRepresentationForRect:context:hints:]` consults it; on a thread with no
/// current context it is `None`, which changes representation selection but is not UB.
///
/// The operation that actually makes this `unsafe fn` necessary is
/// `-bestRepresentationForRect:context:hints:`, which objc2-app-kit 0.3 declares `unsafe`
/// because the `hints` dictionary is untyped: an arbitrary `NSDictionary` may hold keys AppKit
/// will interpret and objects it will message. This function always passes `None`, so that
/// obligation is discharged internally — see the proof below.
///
/// See docs/13-unsafe-review.md #16 for the thread question, which is *not* discharged here:
/// `fig_desktop`'s `icon_lookup` calls into this from a tokio worker thread.
unsafe fn resize_image(image: &NSImage, size: NSSize) -> Option<Retained<NSImage>> {
    let rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size,
    };

    let context = NSGraphicsContext::currentContext();
    // SAFETY:
    // Operation: `objc_msgSend` for `-[NSImage bestRepresentationForRect:context:hints:]`.
    // Contract (objc2-app-kit 0.3): live receiver; `context` a live `NSGraphicsContext` or nil;
    // `hints` a dictionary whose keys/values AppKit is prepared to interpret, or nil.
    // Evidence:
    // - `image: &NSImage` is a Rust reference, hence a live receiver.
    // - `context.as_deref()` is `Option<&NSGraphicsContext>` derived from a `Retained`, so it is
    //   either `None` or live for this call.
    // - `hints` is the literal `None`, so the untyped-dictionary obligation — the sole reason objc2
    //   marks this method `unsafe` — is vacuous.
    // Postcondition: `rep` is a `+0`-derived `Retained<NSImageRep>` owned by objc2; the
    // representation is still owned by `image`, so it must not outlive its use here. It does
    // not: it is handed to `addRepresentation` on the next line, which retains.
    let rep = image.bestRepresentationForRect_context_hints(rect, context.as_deref(), None)?;

    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);

    Some(image)
}

/// # Safety
///
/// The caller must ensure that AppKit may be used from the calling thread — see the `# Safety`
/// section of [`resize_image`] and docs/13-unsafe-review.md #16. There is no pointer, aliasing
/// or ownership obligation: every other operation in this function is a safe objc2 binding.
pub unsafe fn png_for_name(name: &str) -> Option<Vec<u8>> {
    let shared = NSWorkspace::sharedWorkspace();

    let file_type = NSString::from_str(name);

    #[allow(deprecated, reason = "iconForContentType is not available in objc2")]
    let image = shared.iconForFileType(&file_type);

    convert_image(&image)
}

/// # Safety
///
/// The caller must ensure that AppKit may be used from the calling thread — see the `# Safety`
/// section of [`resize_image`] and docs/13-unsafe-review.md #16.
///
/// Note the inverted branch below: the `path.exists()` arm uses `iconForFile`, and the *else*
/// arm (path does not exist) calls `std::fs::metadata`, which will also fail. That is a logic
/// smell, not a safety one.
pub unsafe fn png_for_path(path: &Path) -> Option<Vec<u8>> {
    let shared = NSWorkspace::sharedWorkspace();
    let image = if path.exists() {
        let file_path = NSString::from_str(path.to_str()?);
        shared.iconForFile(&file_path)
    } else {
        let is_dir = std::fs::metadata(path).ok().is_some_and(|meta| meta.is_dir());
        let file_type = if is_dir {
            NSFileTypeDirectory
        } else {
            &NSString::from_str(path.extension()?.to_str()?)
        };

        #[allow(deprecated, reason = "iconForContentType is not available in objc2")]
        shared.iconForFileType(file_type)
    };

    convert_image(&image)
}

fn convert_image(image: &NSImage) -> Option<Vec<u8>> {
    // SAFETY:
    // Operation: `resize_image`, an unsafe fn.
    // Contract (see its `# Safety`): AppKit usable from this thread.
    // Evidence: UNPROVEN — see docs/13-unsafe-review.md #16. `convert_image` is a *safe* fn, so
    // it cannot place the thread obligation on its own callers; it silently inherits whatever
    // thread `png_for_name` / `png_for_path` were called on. Those are `unsafe fn` and do carry
    // the obligation, and they are the only callers, so the contract is not actually lost — but
    // this frame is where it stops being expressible in the type system.
    let image = unsafe {
        resize_image(image, NSSize {
            width: 32.0,
            height: 32.0,
        })
    }?;
    let tiff_data = image.TIFFRepresentation()?;

    let image_rep = NSBitmapImageRep::imageRepWithData(&tiff_data)?;

    let properties = NSDictionary::new();
    // SAFETY:
    // Operation: `objc_msgSend` for
    // `-[NSBitmapImageRep representationUsingType:properties:]`.
    // Contract (objc2-app-kit 0.3): live receiver, and `properties` must be a dictionary whose
    // keys are `NSBitmapImageRep` property keys with values of the types AppKit expects —
    // the untyped-dictionary obligation again.
    // Evidence:
    // - `image_rep` is a `Retained<NSBitmapImageRep>` from `imageRepWithData`, live here.
    // - `properties` is `NSDictionary::new()`, i.e. empty, so no key can be misinterpreted and
    //   AppKit falls back to its documented defaults for PNG.
    // - `NSBitmapImageFileType::PNG` is a valid discriminant of the generated enum (TYPE FACT).
    let png_data = unsafe { image_rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) }?;

    Some(png_data.to_vec())
}
