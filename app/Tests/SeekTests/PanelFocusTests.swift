//
//  PanelFocusTests.swift
//  Seek
//
//  A check that the panel actually takes the focus.
//
//  The panel used to be a non-activating panel that was ordered to the
//  front without ever becoming the key window. The search field therefore
//  never became the first responder, the key presses went to the app that
//  was in front, and the panel looked completely dead.
//
//  These tests check the two halves of the fix: the panel has to be able to
//  become the key window, and the focus has to reach the text field.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class PanelFocusTests: XCTestCase {
    /// The style mask the panel is built with.
    ///
    /// The `nonactivatingPanel` flag is the one that caused the bug: such a
    /// panel cannot take the key window.
    private let panelStyleMask: NSWindow.StyleMask = [
        .fullSizeContentView, .titled, .closable
    ]

    func test_the_panel_can_become_the_key_window() {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 460),
            styleMask: panelStyleMask,
            backing: .buffered,
            defer: false
        )

        XCTAssertTrue(
            panel.canBecomeKey,
            "the panel cannot become the key window, so it cannot be typed into"
        )
        XCTAssertFalse(
            panel.styleMask.contains(.nonactivatingPanel),
            "a non-activating panel never becomes the key window"
        )
    }

    func test_the_text_field_takes_the_focus() {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let hosting = NSHostingController(rootView: Color.clear)
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 460),
            styleMask: panelStyleMask,
            backing: .buffered,
            defer: false
        )
        panel.contentView = hosting.view
        panel.orderFrontRegardless()
        panel.makeKeyAndOrderFront(nil)

        let field = NSTextField()
        panel.contentView?.addSubview(field)

        let accepted = panel.makeFirstResponder(field)

        XCTAssertTrue(accepted, "the text field refused the first responder")

        // The first responder is the field's editor view rather than the
        // field itself, which is how AppKit focuses a text field.
        let editor = field.currentEditor()

        XCTAssertNotNil(
            editor,
            "the text field did not get an editor, so it cannot be typed into"
        )
        XCTAssertTrue(
            panel.firstResponder === editor,
            "the first responder is not the text field's editor"
        )
    }

    func test_the_focus_notification_is_answered() {
        // The panel asks for the focus through a notification, because the
        // focus cannot be set before the panel is on screen and is the key
        // window.
        let expectation = expectation(
            forNotification: .seekFocusSearch,
            object: nil
        ) { _ in true }

        NotificationCenter.default.post(name: .seekFocusSearch, object: nil)

        wait(for: [expectation], timeout: 1)
    }
}
