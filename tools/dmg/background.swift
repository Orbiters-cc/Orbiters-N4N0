// Draws the picture behind the disk image's window (`cargo xtask dist` on a Mac copies the result, it does not run this):
//   swift tools/dmg/background.swift crates/orbiters-ui/assets/OrbitersSans-VF.ttf <folder>
// writes background.png (660 x 420) and background@2x.png there; tools/dmg/README.md says how they become the
// committed crates/orbiters-desktop/assets/dmg/background.tiff. The look is the app's: the page's near-black, the
// icon's violet and blue glows, the app's own typeface, Orbiters green for the one thing to do.
import AppKit
import CoreText

let arguments = CommandLine.arguments
guard arguments.count == 3 else { fatalError("usage: background.swift <OrbitersSans-VF.ttf> <folder>") }
CTFontManagerRegisterFontsForURL(URL(fileURLWithPath: arguments[1]) as CFURL, .process, nil)
let folder = URL(fileURLWithPath: arguments[2], isDirectory: true)

// The window, in points; the two icons' centres are the layout's (tools/dmg/layout.py), y from the top.
let size = CGSize(width: 660, height: 420)
let app = CGPoint(x: 180, y: 200), applications = CGPoint(x: 480, y: 200)

func colour(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
}

func glow(_ c: CGContext, at centre: CGPoint, radius: CGFloat, _ hex: UInt32, _ alpha: CGFloat) {
    let gradient = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: [colour(hex, alpha), colour(hex, alpha * 0.45), colour(hex, 0)] as CFArray, locations: [0, 0.45, 1])!
    c.drawRadialGradient(gradient, startCenter: centre, startRadius: 0, endCenter: centre, endRadius: radius, options: [])
}

func text(_ c: CGContext, _ words: String, size points: CGFloat, weight: CGFloat, _ hex: UInt32, _ alpha: CGFloat, centre: CGPoint, tracking: CGFloat = 0) {
    let base = CTFontCreateWithName("Orbiters Sans" as CFString, points, nil)
    let variation = [NSNumber(value: 0x77676874): NSNumber(value: Double(weight))] as CFDictionary // 'wght'
    let font = CTFontCreateCopyWithAttributes(base, points, nil, CTFontDescriptorCreateWithAttributes([kCTFontVariationAttribute: variation] as CFDictionary))
    let line = CTLineCreateWithAttributedString(NSAttributedString(string: words, attributes: [.font: font, .foregroundColor: NSColor(cgColor: colour(hex, alpha))!, .kern: tracking]))
    let bounds = CTLineGetBoundsWithOptions(line, [])
    c.textPosition = CGPoint(x: centre.x - bounds.width / 2 - bounds.minX, y: centre.y - bounds.height / 2 - bounds.minY)
    CTLineDraw(line, c)
}

func draw(scale: CGFloat, to name: String) {
    let (w, h) = (Int(size.width * scale), Int(size.height * scale))
    let c = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    c.scaleBy(x: scale, y: scale)
    // From here on y grows upwards; `top` turns a distance from the top into it.
    func top(_ y: CGFloat) -> CGFloat { size.height - y }
    c.setFillColor(colour(0x09090b))
    c.fill(CGRect(origin: .zero, size: size))
    glow(c, at: CGPoint(x: 120, y: top(40)), radius: 430, 0x3a2270, 0.95)
    glow(c, at: CGPoint(x: 600, y: top(400)), radius: 380, 0x4f89ff, 0.30)
    glow(c, at: CGPoint(x: 660, y: top(0)), radius: 300, 0xbd65ff, 0.14)

    // A soft plate under each icon, as the app's cards sit on its page.
    for centre in [app, applications] {
        let plate = CGRect(x: centre.x - 98, y: top(centre.y) - 112, width: 196, height: 212)
        c.addPath(CGPath(roundedRect: plate, cornerWidth: 28, cornerHeight: 28, transform: nil))
        c.setFillColor(colour(0xffffff, 0.045))
        c.fillPath()
        c.addPath(CGPath(roundedRect: plate.insetBy(dx: 0.5, dy: 0.5), cornerWidth: 27.5, cornerHeight: 27.5, transform: nil))
        c.setStrokeColor(colour(0xffffff, 0.09))
        c.setLineWidth(1)
        c.strokePath()
    }

    // Finder writes the names under the icons in black whatever is behind them: a light pill under each, wide enough
    // for the name with its ".app" (shown on Macs set to show extensions).
    for (centre, width) in [(app, CGFloat(164)), (applications, CGFloat(112))] {
        let pill = CGRect(x: centre.x - width / 2, y: top(centre.y + 83) - 12, width: width, height: 24)
        c.addPath(CGPath(roundedRect: pill, cornerWidth: 12, cornerHeight: 12, transform: nil))
        c.setFillColor(colour(0xecedee, 0.92))
        c.fillPath()
    }

    // The arrow from the one to the other, in Orbiters green.
    let (from, to, y) = (app.x + 112, applications.x - 112, top(app.y - 8))
    c.setStrokeColor(colour(0x00da6d))
    c.setLineWidth(3)
    c.setLineCap(.round)
    c.setLineJoin(.round)
    c.move(to: CGPoint(x: from, y: y))
    c.addLine(to: CGPoint(x: to, y: y))
    c.move(to: CGPoint(x: to - 11, y: y + 11))
    c.addLine(to: CGPoint(x: to, y: y))
    c.addLine(to: CGPoint(x: to - 11, y: y - 11))
    c.strokePath()

    text(c, "Orbiters - N4N0", size: 22, weight: 650, 0xecedee, 1, centre: CGPoint(x: size.width / 2, y: top(44)))
    text(c, "Drag N4N0 onto Applications to install it", size: 13, weight: 450, 0xb4b4bb, 1, centre: CGPoint(x: size.width / 2, y: top(72)))
    text(c, "ORBITERS.CC", size: 10.5, weight: 600, 0x8b8b94, 0.9, centre: CGPoint(x: size.width / 2, y: top(356)), tracking: 2.2) // above a Finder path bar, when the person shows it

    let image = NSBitmapImageRep(cgImage: c.makeImage()!)
    image.size = size
    try! image.representation(using: .png, properties: [:])!.write(to: folder.appendingPathComponent(name))
}

draw(scale: 1, to: "background.png")
draw(scale: 2, to: "background@2x.png")
