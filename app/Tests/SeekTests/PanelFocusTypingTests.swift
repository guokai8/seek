//
//  PanelFocusTypingTests.swift
//  Seek
//
//  A check that the panel takes the focus by itself.
//
//  The other typing tests call `makeFirstResponder` by hand, which is what
//  the test needs but not what the app does. The app asks for the focus
//  through a notification, and SwiftUI's `@FocusState` turns that into a
//  real first responder.
//
//  If that path does not work, the panel is on screen and looks fine, and
//  the key presses go to whatever was in front of it instead of the search
//  field. The user types and nothing happens.
//
//  This test needs to be the only panel in the process, and says so by
//  closing everything else before it starts. `.seekFocusSearch` is a
//  process-wide broadcast, and every `ContentView` alive in the process
//  answers it by setting its own `isSearchFocused`. A panel that an earlier
//  test ordered in is retained by AppKit's window list long after that test
//  returned, so in a full run the broadcast reached several of them at once
//  and this panel's field *resigned*: the responder went from the field
//  editor to the panel itself, and the test read as "the focus never
//  arrived" when what really happened is that it arrived and was taken away
//  again. The app has one panel, so it never sees this.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class PanelFocusTypingTests: XCTestCase {
    /// The panels this class builds, so they can be disposed of.
    private var windows: [NSPanel] = []

    override func setUp() async throws {
        closeEveryWindow()
    }

    override func tearDown() async throws {
        // Symmetry: this class is not allowed to leave behind what it
        // refuses to inherit.
        closeEveryWindow()
        windows = []
    }

    /// Closes every window the process still has open, taking the content
    /// view with it so the SwiftUI view inside stops listening.
    private func closeEveryWindow() {
        for window in NSApplication.shared.windows {
            window.contentViewController = nil
            window.contentView = nil
            // Under ARC a window that releases itself on close is an
            // over-release waiting to happen.
            window.isReleasedWhenClosed = false
            window.orderOut(nil)
            window.close()
        }
    }

    /// Finds the search field in the panel's view tree.
    private func findTextField(in view: NSView) -> NSTextField? {
        if let field = view as? NSTextField { return field }

        for subview in view.subviews {
            if let found = findTextField(in: subview) { return found }
        }

        return nil
    }

    func test_the_panel_takes_the_focus_without_being_told_to() async throws {
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
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        windows.append(panel)

        // This is exactly what the app does: activate, then order in.
        app.activate(ignoringOtherApps: true)
        panel.makeKeyAndOrderFront(nil)

        try await Task.sleep(for: .milliseconds(400))

        guard let field = findTextField(in: panel.contentView!) else {
            XCTFail("the search field is not in the panel")
            return
        }

        print("FOCUS panel.isKey=\(panel.isKeyWindow) app.isActive=\(app.isActive)")
        print("FOCUS responder=\(String(describing: panel.firstResponder))")

        // The app asks for the focus through this notification, and nothing
        // else. There is no `makeFirstResponder` call.
        NotificationCenter.default.post(name: .seekFocusSearch, object: nil)
        try await Task.sleep(for: .milliseconds(500))

        print("FOCUS after-notify responder=\(String(describing: panel.firstResponder))")

        XCTAssertTrue(
            panel.firstResponder === field.currentEditor(),
            "the search field did not take the focus by itself"
        )
    }

    func test_typing_into_the_unfocused_panel_reaches_the_model() async throws {
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
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        windows.append(panel)

        app.activate(ignoringOtherApps: true)
        panel.makeKeyAndOrderFront(nil)
        try await Task.sleep(for: .milliseconds(400))

        guard let field = findTextField(in: panel.contentView!) else {
            XCTFail("the search field is not in the panel")
            return
        }

        NotificationCenter.default.post(name: .seekFocusSearch, object: nil)
        try await Task.sleep(for: .milliseconds(500))

        // The panel has to be the key window and the field the responder,
        // or the key presses have nowhere to go.
        panel.makeKey()
        panel.makeFirstResponder(field)

        // The editor only exists once the field has taken the focus, so
        // the focus is waited for rather than assumed.
        let deadline = Date().addingTimeInterval(3)
        while field.currentEditor() == nil, Date() < deadline {
            try await Task.sleep(for: .milliseconds(50))
        }

        XCTAssertNotNil(
            field.currentEditor(),
            "the search field never took the focus"
        )

        // The key presses go to the window, which routes them to whatever
        // is the first responder. If the field did not take the focus, they
        // go nowhere.
        for character in "abc" {
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

        print("TYPING field=\(field.stringValue) model.query=\(model.query)")

        XCTAssertEqual(
            model.query,
            "abc",
            "the key presses never reached the search field"
        )
    }
}
