//
//  make-icon.swift
//  Seek
//
//  Draws the app icon and writes the .icns the bundle ships.
//
//  The icon is code rather than a picture checked into the repository, so
//  that changing a proportion or a colour is an edit with a diff, and so
//  that every size is drawn at its own size instead of being resampled
//  down from one big one — which is what turns a crisp 16pt icon into a
//  grey smudge in the Dock.
//
//  Run it with:
//
//      swift tools/make-icon.swift build/Seek.iconset
//
//  and then `iconutil -c icns` on the directory it writes. bundle.sh does
//  both.
//
//  The mark is the result list: rows narrowing as the query sharpens, with
//  the matched run of each row lit. It is what the panel actually shows,
//  which is a better thing for an icon to say than a magnifying glass that
//  every other search tool already uses.
//

import AppKit

/// The colours of the mark.
///
/// The blue is the one macOS uses for its own default accent, so the icon
/// sits with the system rather than against it. The tile is graphite
/// rather than black: a pure black tile loses its corners against a dark
/// wallpaper.
enum Palette {
    static let accent = NSColor(srgbRed: 0.24, green: 0.55, blue: 1.0, alpha: 1)
    static let tileTop = NSColor(srgbRed: 0.17, green: 0.19, blue: 0.23, alpha: 1)
    static let tileBottom = NSColor(srgbRed: 0.09, green: 0.10, blue: 0.13, alpha: 1)
    static let row = NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.34)
    static let rim = NSColor(white: 1, alpha: 0.16)
}

/// Draws the icon at `size` points into the current graphics context.
func drawIcon(size: CGFloat) {
    // Apple's macOS icon grid: the art is inset from the canvas so the
    // shadow and the rounded corners have room.
    let inset = size * 0.094
    let tile = CGRect(x: inset, y: inset, width: size - inset * 2, height: size - inset * 2)
    let radius = tile.width * 0.2237

    let shape = NSBezierPath(roundedRect: tile, xRadius: radius, yRadius: radius)
    NSGradient(starting: Palette.tileTop, ending: Palette.tileBottom)!.draw(in: shape, angle: -90)

    // A hairline rim, the way system icons catch the light at the top.
    shape.lineWidth = max(size / 256, 0.5)
    Palette.rim.setStroke()
    shape.stroke()

    // Rows, narrowing. The lit run shortens with them: the more the query
    // has narrowed the list, the less of each row the match covers.
    //
    // Below 32pt the three rows stop being three rows. At 16pt each one is
    // barely a pixel tall and the gaps between them close up, so the mark
    // reads as a grey smear. Two thicker rows say the same thing and
    // survive. This is the whole reason every size is drawn rather than
    // resampled from one large rendition.
    let small = size < 32
    let rows: [(width: CGFloat, lit: CGFloat)] = small
        ? [(0.62, 0.40), (0.42, 0.26)]
        : [(0.66, 0.42), (0.50, 0.30), (0.37, 0.22)]

    let height = tile.height * (small ? 0.150 : 0.085)
    let gap = height * (small ? 1.90 : 1.65)
    let left = tile.minX + tile.width * (small ? 0.14 : 0.17)

    // The stack is centred on the tile whatever its height, so the two
    // layouts sit in the same place rather than drifting up or down.
    let span = gap * CGFloat(rows.count - 1)
    var y = tile.midY + span / 2 - height / 2

    for row in rows {
        let width = tile.width * row.width

        bar(x: left, y: y, width: width, height: height, colour: Palette.row)
        bar(x: left, y: y, width: width * row.lit, height: height, colour: Palette.accent)

        y -= gap
    }
}

/// Fills one rounded bar.
func bar(x: CGFloat, y: CGFloat, width: CGFloat, height: CGFloat, colour: NSColor) {
    colour.setFill()
    NSBezierPath(
        roundedRect: CGRect(x: x, y: y, width: width, height: height),
        xRadius: height / 2,
        yRadius: height / 2
    ).fill()
}

/// Renders the icon at `pixels` square and returns the PNG data.
///
/// Each size is drawn rather than scaled, so the small ones keep their
/// edges. `NSImage` is given the point size and the bitmap the pixel size,
/// which is how a @2x rendition is produced.
func render(pixels: Int) -> Data {
    let side = CGFloat(pixels)

    guard let representation = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: pixels,
        pixelsHigh: pixels,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    ) else {
        fatalError("could not make a bitmap \(pixels) wide")
    }

    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: representation)
    NSGraphicsContext.current?.imageInterpolation = .high

    drawIcon(size: side)

    NSGraphicsContext.restoreGraphicsState()

    guard let png = representation.representation(using: .png, properties: [:]) else {
        fatalError("could not encode the \(pixels)px icon")
    }

    return png
}

// The sizes an .iconset holds, each at 1x and 2x.
let sizes = [16, 32, 128, 256, 512]

let directory = CommandLine.arguments.count > 1
    ? CommandLine.arguments[1]
    : "build/Seek.iconset"

try? FileManager.default.createDirectory(
    atPath: directory,
    withIntermediateDirectories: true
)

for size in sizes {
    for scale in [1, 2] {
        let name = scale == 1 ? "icon_\(size)x\(size).png" : "icon_\(size)x\(size)@2x.png"
        let data = render(pixels: size * scale)

        try! data.write(to: URL(fileURLWithPath: "\(directory)/\(name)"))
    }
}

print("wrote \(sizes.count * 2) renditions to \(directory)")
