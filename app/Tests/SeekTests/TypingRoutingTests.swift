import AppKit
import SwiftUI
import XCTest

@testable import Seek

/// A check that a key press in the panel reaches the model.
///
/// The search engine works and the model works, but the two are joined by
/// the responder chain: the search field has to be the first responder, and
/// the key press has to travel from the field to the model's query.
///

@MainActor
final class TypingRoutingTests: XCTestCase {
    /// The text field inside the panel, found by walking the view tree.
    private func findTextField(in view: NSView) -> NSTextField? {
        if let field = view as? NSTextField {
            return field
        }

        for subview in view.subviews {
            if let found = findTextField(in: subview) {
                return found
            }
        }

        return nil
    }

    func test_a_key_press_reaches_the_query() async throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let model = AppModel()
        let content = ContentView().environmentObject(model)
        let hosting = NSHostingController(rootView: content)

        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 460),
            styleMask: [.fullSizeContentView, .titled, .closable],
            backing: .buffered,
            defer: false
        )
        panel.contentView = hosting.view
        panel.makeKeyAndOrderFront(nil)
        app.activate(ignoringOtherApps: true)

        // The layout has to have happened before the tree can be walked.
        try await Task.sleep(for: .milliseconds(300))

        guard let field = findTextField(in: panel.contentView!) else {
            XCTFail("the search field is not in the panel")
            return
        }

        print("ROUTING field=\(field) placeholder=\(field.placeholderString ?? "nil")")

        let accepted = panel.makeFirstResponder(field)
        print("ROUTING accepted=\(accepted) responder=\(String(describing: panel.firstResponder))")

        // The key press travels the way a real one does: through the window
        // and into the responder chain.
        for character in "readme" {
            let event = NSEvent.keyEvent(
                with: .keyDown,
                location: .zero,
                modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: panel.windowNumber,
                context: nil,
                characters: String(character),
                charactersIgnoringModifiers: String(character),
                isARepeat: false,
                keyCode: 0
            )!

            panel.sendEvent(event)
        }

        print("ROUTING field.stringValue=\(field.stringValue) model.query=\(model.query)")

        XCTAssertEqual(
            model.query,
            "readme",
            "the key presses did not reach the model"
        )
    }
}
