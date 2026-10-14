// Rasterizes the app icon's SVG into an .iconset with AppKit's own SVG
// support, so the gradients render as drawn at every size. The artwork fills
// the canvas; macOS masks it into its icon shape and shadows it itself.
// Usage: swift scripts/render-app-icon.swift <svg> <iconset dir>
import AppKit

let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    FileHandle.standardError.write("usage: render-app-icon.swift <svg> <iconset dir>\n".data(using: .utf8)!)
    exit(2)
}
let svgPath = arguments[1]
let outDir = arguments[2]
guard let svg = NSImage(contentsOfFile: svgPath) else {
    FileHandle.standardError.write("cannot read \(svgPath)\n".data(using: .utf8)!)
    exit(1)
}
try FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)

let sizes: [(String, Int)] = [
    ("icon_16x16", 16), ("icon_16x16@2x", 32), ("icon_32x32", 32), ("icon_32x32@2x", 64),
    ("icon_128x128", 128), ("icon_128x128@2x", 256), ("icon_256x256", 256),
    ("icon_256x256@2x", 512), ("icon_512x512", 512), ("icon_512x512@2x", 1024),
]
for (name, pixels) in sizes {
    guard let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0
    ) else { exit(1) }
    rep.size = NSSize(width: pixels, height: pixels)
    NSGraphicsContext.saveGraphicsState()
    guard let context = NSGraphicsContext(bitmapImageRep: rep) else { exit(1) }
    NSGraphicsContext.current = context
    let cg = context.cgContext
    cg.interpolationQuality = .high
    svg.draw(in: NSRect(x: 0, y: 0, width: pixels, height: pixels))
    NSGraphicsContext.restoreGraphicsState()
    guard let png = rep.representation(using: .png, properties: [:]) else { exit(1) }
    try png.write(to: URL(fileURLWithPath: "\(outDir)/\(name).png"))
}
