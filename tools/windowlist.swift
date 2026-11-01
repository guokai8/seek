// Lists the windows of an app, including the ones that are off screen.
//
//      ./windowlist Seek

import CoreGraphics
import Foundation

let needle = CommandLine.arguments.count > 1 ? CommandLine.arguments[1].lowercased() : ""

let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []

for window in list {
    let owner = window[kCGWindowOwnerName as String] as? String ?? ""
    guard needle.isEmpty || owner.lowercased().contains(needle) else { continue }

    let number = window[kCGWindowNumber as String] as? Int ?? 0
    let name = window[kCGWindowName as String] as? String ?? "(no title)"
    let layer = window[kCGWindowLayer as String] as? Int ?? 0
    let alpha = window[kCGWindowAlpha as String] as? Double ?? 0
    let onscreen = window[kCGWindowIsOnscreen as String] as? Bool ?? false
    let bounds = window[kCGWindowBounds as String] as? [String: Any] ?? [:]

    let x = bounds["X"] as? Double ?? 0
    let y = bounds["Y"] as? Double ?? 0
    let w = bounds["Width"] as? Double ?? 0
    let h = bounds["Height"] as? Double ?? 0

    print("id=\(number) layer=\(layer) alpha=\(alpha) onscreen=\(onscreen) owner=\(owner) name=\(name) rect=\(Int(x)),\(Int(y)) \(Int(w))x\(Int(h))")
}
