// A tiny helper that lists the windows of a running app and captures one.
// It only reads the window list, so it needs no accessibility permission.

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []

var target: [String: Any]?

for window in list {
    let owner = window[kCGWindowOwnerName as String] as? String ?? ""
    guard owner.lowercased().contains(CommandLine.arguments.count > 1 ? CommandLine.arguments[1].lowercased() : "seek") else {
        continue
    }

    let number = window[kCGWindowNumber as String] as? Int ?? 0
    let name = window[kCGWindowName as String] as? String ?? "(no title)"
    let bounds = window[kCGWindowBounds as String] as? [String: Any] ?? [:]
    let layer = window[kCGWindowLayer as String] as? Int ?? 0

    print("window \(number) layer=\(layer) name=\(name) bounds=\(bounds)")

    if target == nil, layer == 0 {
        target = window
    }
}

guard let target else {
    print("no window found")
    exit(1)
}

let number = target[kCGWindowNumber as String] as? Int ?? 0
let image = CGWindowListCreateImage(
    .null,
    .optionIncludingWindow,
    CGWindowID(number),
    [.boundsIgnoreFraming, .bestResolution]
)!

let url = URL(fileURLWithPath: CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "/tmp/window.png") as CFURL
guard let dest = CGImageDestinationCreateWithURL(url, UTType.png.identifier as CFString, 1, nil) else {
    exit(2)
}
CGImageDestinationAddImage(dest, image, nil)
CGImageDestinationFinalize(dest)

print("captured \(number) -> \(url.path)")
