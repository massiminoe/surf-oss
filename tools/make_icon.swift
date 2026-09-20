// Rebuild the official stacked wordmark as a macOS iconset:
// swift tools/make_icon.swift /tmp/surf-oss.iconset
// Also writes assets/icon/surf-oss.icns and assets/icon/surf-oss.png.
import AppKit
import CoreText

let output = CommandLine.arguments[1]
try FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
func color(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat) -> NSColor {
    NSColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: 1)
}
// Outline the type once so every icon size uses identical letter shapes.
let font = CTFontCreateWithName("HelveticaNeue-Bold" as CFString, 400, nil)
func wordmark(_ text: String) -> CGPath {
    let line = CTLineCreateWithAttributedString(NSAttributedString(
        string: text, attributes: [.font: font, .kern: -10]))
    let path = CGMutablePath()
    for run in CTLineGetGlyphRuns(line) as! [CTRun] {
        let count = CTRunGetGlyphCount(run)
        var glyphs = [CGGlyph](repeating: 0, count: count)
        var positions = [CGPoint](repeating: .zero, count: count)
        CTRunGetGlyphs(run, CFRange(location: 0, length: 0), &glyphs)
        CTRunGetPositions(run, CFRange(location: 0, length: 0), &positions)
        for i in 0..<count {
            if let glyph = CTFontCreatePathForGlyph(font, glyphs[i], nil) {
                path.addPath(glyph, transform: CGAffineTransform(
                    translationX: positions[i].x, y: positions[i].y))
            }
        }
    }
    return path
}
let surf = wordmark("surf")
let oss = wordmark("oss")
let typeScale = 752 / surf.boundingBoxOfPath.width
func drawWord(_ path: CGPath, right: CGFloat, bottom: CGFloat, fill: NSColor) {
    let bounds = path.boundingBoxOfPath
    let context = NSGraphicsContext.current!.cgContext
    context.saveGState()
    context.translateBy(x: right - bounds.maxX * typeScale,
                        y: bottom - bounds.minY * typeScale)
    context.scaleBy(x: typeScale, y: typeScale)
    context.addPath(path)
    context.setFillColor(fill.cgColor)
    context.fillPath()
    context.restoreGState()
}
for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = size * scale
        let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels,
            pixelsHigh: pixels, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
            isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        let transform = AffineTransform(scale: CGFloat(pixels) / 1024)
        (transform as NSAffineTransform).concat()
        let tile = NSBezierPath(roundedRect: NSRect(x: 64, y: 64, width: 896, height: 896), xRadius: 204, yRadius: 204)
        color(7, 17, 48).setFill()
        tile.fill()
        drawWord(surf, right: 880, bottom: 486, fill: color(249, 247, 239))
        drawWord(oss, right: 880, bottom: 232, fill: color(12, 215, 237))
        NSGraphicsContext.restoreGraphicsState()
        let suffix = scale == 2 ? "@2x" : ""
        let url = URL(fileURLWithPath: "\(output)/icon_\(size)x\(size)\(suffix).png")
        try bitmap.representation(using: .png, properties: [:])!.write(to: url)
    }
}

// ICNS stores PNG representations in typed, big-endian length-prefixed chunks.
func bigEndian(_ value: Int) -> Data {
    var number = UInt32(value).bigEndian
    return withUnsafeBytes(of: &number) { Data($0) }
}
var chunks = Data()
for (type, name) in [
    ("icp4", "16x16"), ("icp5", "32x32"), ("icp6", "32x32@2x"),
    ("ic07", "128x128"), ("ic08", "256x256"), ("ic09", "512x512"),
    ("ic10", "512x512@2x"), ("ic11", "16x16@2x"),
    ("ic12", "32x32@2x"), ("ic13", "128x128@2x"), ("ic14", "256x256@2x")
] {
    let png = try Data(contentsOf: URL(fileURLWithPath: "\(output)/icon_\(name).png"))
    chunks.append(Data(type.utf8))
    chunks.append(bigEndian(png.count + 8))
    chunks.append(png)
}
var icns = Data("icns".utf8)
icns.append(bigEndian(chunks.count + 8))
icns.append(chunks)
try icns.write(to: URL(fileURLWithPath: "assets/icon/surf-oss.icns"))
try Data(contentsOf: URL(fileURLWithPath: "\(output)/icon_512x512@2x.png"))
    .write(to: URL(fileURLWithPath: "assets/icon/surf-oss.png"))
