//
//  KeyboardHandler.swift
//  Fuzzylite
//
//  The keyboard shortcuts of the results list. The panel is not a normal
//  window, so the key handling is done by an `NSView` that sits behind the
//  SwiftUI content and forwards the events to the model.
//

import AppKit
import SwiftUI

/// A view that captures the key presses the SwiftUI list does not handle.
struct KeyboardHandler: NSViewRepresentable {
    /// The model that owns the selection.
    let model: AppModel

    func makeNSView(context: Context) -> KeyCatcher {
        KeyCatcher(model: model)
    }

    func updateNSView(_ nsView: KeyCatcher, context: Context) {
        nsView.model = model
    }
}

/// The view that catches the key presses.
final class KeyCatcher: NSView {
    /// The model that owns the selection.
    var model: AppModel

    init(model: AppModel) {
        self.model = model
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override func keyDown(with event: NSEvent) {
        let command = event.modifierFlags.contains(.command)
        let option = event.modifierFlags.contains(.option)

        // The shortcuts that carry a modifier are handled first, because
        // their key codes are shared with the unmodified ones.
        if command && option && event.keyCode == 15 { // Cmd+Opt+R
            if let hit = model.selectedHit {
                model.reveal(hit)
            }
            return
        }

        if command && event.keyCode == 43 { // Cmd+,
            NotificationCenter.default.post(name: .fuzzyliteShowSettings, object: nil)
            return
        }

        switch event.keyCode {
        case 126: // Up arrow
            model.moveSelection(by: -1)
        case 125: // Down arrow
            model.moveSelection(by: 1)
        case 115: // Home
            model.selectFirst()
        case 119: // End
            model.selectLast()
        case 36: // Return
            model.openSelection()
        case 53: // Escape
            window?.performClose(nil)
        default:
            super.keyDown(with: event)
        }
    }
}

/// The notification that asks the app to show the settings window.
extension Notification.Name {
    /// Posted when the settings window is requested.
    static let fuzzyliteShowSettings = Notification.Name("fuzzylite.showSettings")
}
