use std::path::Path;

use objc2::rc::Retained;
use objc2::{
    AllocAnyThread,
    MainThreadMarker,
};
use objc2_app_kit::{
    NSBitmapImageFileType,
    NSBitmapImageRep,
    NSGraphicsContext,
    NSImage,
    NSWorkspace,
};
use objc2_foundation::{
    NSDictionary,
    NSPoint,
    NSRect,
    NSSize,
    NSString,
};

fn resize_image(_mtm: MainThreadMarker, image: &NSImage, size: NSSize) -> Option<Retained<NSImage>> {
    let rect = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size,
    };
    let context = NSGraphicsContext::currentContext();
    // SAFETY: image/context remain live; no untyped hints are supplied to AppKit.
    let rep = unsafe { image.bestRepresentationForRect_context_hints(rect, context.as_deref(), None) }?;
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);
    Some(image)
}

/// Render a type icon on the main thread; only owned PNG bytes leave this boundary.
pub fn png_for_name(mtm: MainThreadMarker, name: &str) -> Option<Vec<u8>> {
    let file_type = NSString::from_str(name);
    #[allow(deprecated, reason = "iconForContentType is not available in objc2")]
    let image = NSWorkspace::sharedWorkspace().iconForFileType(&file_type);
    convert_image(mtm, &image)
}

/// Render an existing path's icon, or its extension's icon if the path is absent.
pub fn png_for_path(mtm: MainThreadMarker, path: &Path) -> Option<Vec<u8>> {
    if !path.exists() {
        return png_for_name(mtm, path.extension()?.to_str()?);
    }
    let file_path = NSString::from_str(path.to_str()?);
    let image = NSWorkspace::sharedWorkspace().iconForFile(&file_path);
    convert_image(mtm, &image)
}

fn convert_image(mtm: MainThreadMarker, image: &NSImage) -> Option<Vec<u8>> {
    let image = resize_image(mtm, image, NSSize {
        width: 32.0,
        height: 32.0,
    })?;
    let tiff_data = image.TIFFRepresentation()?;
    let image_rep = NSBitmapImageRep::imageRepWithData(&tiff_data)?;
    let properties = NSDictionary::new();
    // SAFETY: the retained image is live; an empty properties dictionary contains no
    // incorrectly typed values, so AppKit uses its PNG defaults.
    let png_data = unsafe { image_rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) }?;
    Some(png_data.to_vec())
}
