// Placeholder app/tray icon generator.
//
// Draws a temporary mark — a filled black disc with a white lowercase "f" for
// the app icon, and a bare monochrome "f" glyph for the menu-bar tray icon —
// so the bundle has real artwork until a designed icon exists. No npm or cargo
// dependencies: CoreGraphics + CoreText only.
//
// Usage: swift icon.swift <output-dir>
//   writes app-<size>.png and tray-<style>-<size>.png into <output-dir>

import AppKit
import CoreGraphics
import CoreText
import Foundation
import ImageIO
import UniformTypeIdentifiers

let black = CGColor(red: 0, green: 0, blue: 0, alpha: 1)
let white = CGColor(red: 1, green: 1, blue: 1, alpha: 1)

func makeContext(_ size: Int) -> CGContext {
    guard
        let ctx = CGContext(
            data: nil,
            width: size,
            height: size,
            bitsPerComponent: 8,
            bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )
    else {
        fatalError("could not create a \(size)x\(size) bitmap context")
    }
    ctx.setAllowsAntialiasing(true)
    ctx.setShouldAntialias(true)
    ctx.setShouldSmoothFonts(true)
    ctx.interpolationQuality = .high
    return ctx
}

/// Draws a tightly centred lowercase "f" filling `fraction` of the canvas.
func drawGlyph(_ ctx: CGContext, size: Int, color: CGColor, fraction: CGFloat) {
    let side = CGFloat(size)
    let font = CTFontCreateWithName("Helvetica-Bold" as CFString, side * fraction, nil)
    let attributed = NSAttributedString(
        string: "f",
        attributes: [
            .font: font,
            .foregroundColor: color,
        ]
    )
    let line = CTLineCreateWithAttributedString(attributed)
    // Glyph-path bounds ignore ascender/descender padding, so the mark ends up
    // optically centred rather than baseline centred.
    let bounds = CTLineGetBoundsWithOptions(line, .useGlyphPathBounds)
    ctx.textPosition = CGPoint(
        x: (side - bounds.width) / 2 - bounds.origin.x,
        y: (side - bounds.height) / 2 - bounds.origin.y
    )
    CTLineDraw(line, ctx)
}

func appIcon(_ size: Int) -> CGImage {
    let ctx = makeContext(size)
    let side = CGFloat(size)
    let inset = side * 0.045
    ctx.setFillColor(black)
    ctx.fillEllipse(in: CGRect(x: inset, y: inset, width: side - 2 * inset, height: side - 2 * inset))
    drawGlyph(ctx, size: size, color: white, fraction: 0.66)
    guard let image = ctx.makeImage() else { fatalError("app icon render failed") }
    return image
}

func trayIcon(_ size: Int, color: CGColor) -> CGImage {
    let ctx = makeContext(size)
    drawGlyph(ctx, size: size, color: color, fraction: 0.92)
    guard let image = ctx.makeImage() else { fatalError("tray icon render failed") }
    return image
}

func writePNG(_ image: CGImage, to url: URL) {
    guard
        let dest = CGImageDestinationCreateWithURL(
            url as CFURL,
            UTType.png.identifier as CFString,
            1,
            nil
        )
    else {
        fatalError("could not open \(url.path) for writing")
    }
    CGImageDestinationAddImage(dest, image, nil)
    guard CGImageDestinationFinalize(dest) else { fatalError("could not encode \(url.path)") }
}

let args = CommandLine.arguments
guard args.count == 2 else {
    FileHandle.standardError.write("usage: swift icon.swift <output-dir>\n".data(using: .utf8)!)
    exit(2)
}
let outDir = URL(fileURLWithPath: args[1], isDirectory: true)
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

// Every app-icon size the repo or the .iconset needs.
for size in [16, 32, 36, 48, 64, 128, 256, 512, 1024] {
    writePNG(appIcon(size), to: outDir.appendingPathComponent("app-\(size).png"))
}
for size in [16, 18, 22, 32, 36, 44, 64, 72] {
    writePNG(trayIcon(size, color: black), to: outDir.appendingPathComponent("tray-dark-\(size).png"))
    writePNG(trayIcon(size, color: white), to: outDir.appendingPathComponent("tray-light-\(size).png"))
}
print("wrote placeholder icons to \(outDir.path)")
