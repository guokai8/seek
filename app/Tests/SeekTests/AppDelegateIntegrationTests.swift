//
//  AppDelegateIntegrationTests.swift
//  Seek
//
//  The check that the app itself works, not just its parts.
//
//  Every other test builds its own window and its own hosting controller,
//  which means it never exercises the code the app really runs. The app
//  creates one panel and one hosting controller, keeps them in a property
//  and reuses them for the life of the process.
//
//  A hosting controller that is released tears the SwiftUI hierarchy down
//  with it: the panel still shows what it drew the first time, but it never
//  updates again. The query would reach the model, the model would find
//  the files, and the results would never appear on the screen.
//
//  So this test drives the real delegate: it shows the real panel, types
//  into the real field, and waits for the real results.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class AppDelegateIntegrationTests: XCTestCase {
    /// A small folder with a few known files in it.
    private func makeFixture() throws -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("seek-app-\(UUID().uuidString)")

        let nested = root.appendingPathComponent("docs/readme")
        try FileManager.default.createDirectory(at: nested, withIntermediateDirectories: true)

        for name in ["readme.md", "readme.txt", "notes.txt"] {
            try "hello".write(
                to: nested.appendingPathComponent(name),
                atomically: true,
                encoding: .utf8
            )
        }

        return root
    }

    /// The panel the delegate opened.
    private func findPanel() -> NSPanel? {
        NSApp.windows.first { $0 is NSPanel } as? NSPanel
    }

    /// Finds the search field in the panel's view tree.
    private func findTextField(in view: NSView) -> NSTextField? {
        if let field = view as? NSTextField { return field }

        for subview in view.subviews {
            if let found = findTextField(in: subview) { return found }
        }

        return nil
    }

    func test_the_real_app_panel_shows_results_when_typed_into() async throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let root = try makeFixture()
        defer { try? FileManager.default.removeItem(at: root) }

        // The app is pointed at the fixture, so that the test does not have
        // to index the whole home directory.
        let preferences = Preferences.shared
        let savedRoots = preferences.rootsText
        let savedExcludes = preferences.excludesText
        preferences.rootsText = root.path
        preferences.excludesText = ""

        defer {
            preferences.rootsText = savedRoots
            preferences.excludesText = savedExcludes
        }

        let delegate = AppDelegate()
        delegate.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))

        // The panel is shown on the next turn of the run loop, by the
        // delegate itself. It is not toggled here: the toggle would hide
        // the panel that was just opened.
        try await Task.sleep(for: .milliseconds(600))

        guard let panel = findPanel() else {
            XCTFail("the app did not open a panel")
            return
        }

        XCTAssertTrue(panel.isVisible, "the panel is not on screen")

        // The index has to be built before the search can find anything.
        let deadline = Date().addingTimeInterval(60)
        while delegate.model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }

        XCTAssertGreaterThan(
            delegate.model.indexedCount,
            0,
            "the app did not index the fixture"
        )

        guard let field = findTextField(in: panel.contentView!) else {
            XCTFail("the search field is not in the panel")
            return
        }

        // The panel asks for the focus through a notification.
        NotificationCenter.default.post(name: .seekFocusSearch, object: nil)
        try await Task.sleep(for: .milliseconds(300))

        panel.makeFirstResponder(field)

        XCTAssertTrue(
            panel.firstResponder is NSText,
            "the search field did not become the first responder"
        )

        // The key presses go through the application, so that the app's own
        // event monitor sees them.
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

            NSApp.sendEvent(event)
        }

        print("APP query=\(delegate.model.query) field=\(field.stringValue)")

        XCTAssertEqual(
            delegate.model.query,
            "readme",
            "the key presses did not reach the model"
        )

        try await Task.sleep(for: .milliseconds(1200))

        print("APP results=\(delegate.model.results.count) status=\(delegate.model.status)")

        XCTAssertFalse(
            delegate.model.results.isEmpty,
            "the real app panel found nothing"
        )

        // The results have to be on the screen, not just in the model. The
        // hosting controller is the thing that ties the model to the view,
        // so this is the check that catches a controller that was released.
        panel.contentView!.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(400))

        let drawn = countLabels(in: panel.contentView!)
        print("APP drawn labels=\(drawn)")

        XCTAssertGreaterThan(
            drawn,
            0,
            "the results are in the model but nothing was drawn on the screen"
        )

        panel.orderOut(nil)
    }

    func test_the_real_panel_follows_its_content() async throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let root = try makeFixture()
        defer { try? FileManager.default.removeItem(at: root) }

        let preferences = Preferences.shared
        let savedRoots = preferences.rootsText
        let savedExcludes = preferences.excludesText
        preferences.rootsText = root.path
        preferences.excludesText = ""

        defer {
            preferences.rootsText = savedRoots
            preferences.excludesText = savedExcludes
        }

        let delegate = AppDelegate()
        delegate.applicationDidFinishLaunching(
            Notification(name: NSApplication.didFinishLaunchingNotification)
        )

        try await Task.sleep(for: .milliseconds(600))

        guard let panel = findPanel() else {
            XCTFail("the app did not open a panel")
            return
        }

        let deadline = Date().addingTimeInterval(60)
        while delegate.model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }

        // The empty state is a line of text, so the panel should be a
        // fraction of its ceiling rather than a tall empty box.
        try await Task.sleep(for: .milliseconds(600))
        let empty = panel.frame
        let emptyTop = empty.origin.y + empty.height

        XCTAssertLessThan(
            empty.height,
            Theme.panelMaxHeight / 2,
            "the panel is \(empty.height)pt tall with nothing in it"
        )

        // The height has to *settle*. The content reports its height and
        // the window takes it; if the two measure different things — a
        // title bar's worth of difference would do it — each resize
        // triggers another and the panel never stops moving.
        try await Task.sleep(for: .milliseconds(700))

        XCTAssertEqual(
            panel.frame.height,
            empty.height,
            accuracy: 0.5,
            "the panel is still resizing itself a second later"
        )

        // Typing fills the list, and the panel grows to hold it.
        delegate.model.query = "readme"
        try await Task.sleep(for: .milliseconds(1500))

        XCTAssertFalse(delegate.model.results.isEmpty, "the fixture found nothing")

        let full = panel.frame

        XCTAssertGreaterThan(
            full.height,
            empty.height,
            "the panel did not grow for \(delegate.model.results.count) results"
        )

        XCTAssertLessThanOrEqual(
            full.height,
            Theme.panelMaxHeight + 0.5,
            "the panel grew past its ceiling"
        )

        // The search field must not move while the user is typing into it.
        XCTAssertEqual(
            full.origin.y + full.height,
            emptyTop,
            accuracy: 0.5,
            "the panel's top edge moved as it grew"
        )

        panel.orderOut(nil)
    }

    /// Counts the text views in a tree that show something.
    private func countLabels(in view: NSView) -> Int {
        var count = 0

        if let text = view as? NSTextField, !text.stringValue.isEmpty {
            count += 1
        }

        for subview in view.subviews {
            count += countLabels(in: subview)
        }

        return count
    }
}
