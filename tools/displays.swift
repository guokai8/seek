import CoreGraphics
import Foundation

var count: UInt32 = 0
CGGetActiveDisplayList(0, nil, &count)
var displays = [CGDirectDisplayID](repeating: 0, count: Int(count))
CGGetActiveDisplayList(count, &displays, &count)

for display in displays {
    let id = display
    let bounds = CGDisplayBounds(id)
    print("display \(id) bounds=\(bounds.origin.x),\(bounds.origin.y) \(Int(bounds.width))x\(Int(bounds.height)) builtin=\(CGDisplayIsBuiltin(id))")
}
