//
//  PanelDeletingTests.swift
//  Seek
//
//  Typing forwards is only half of what a person does to a search box. The
//  other half is taking characters back out — and a panel that answers the
//  first query and then stops listening is worse than one that never
//  answered, because it shows a stale list and calls it a result.
//
//  This drives the real panel the way PanelTypingSearchTests does: a real
//  NSPanel, the real focus handling, the real event monitor, real key
//  events sent through the application.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class PanelDeletingTests: XCTestCase {
    /// The panel the app builds, with the same style mask.
    private func makePanel(rootView: some View) -> NSPanel {
        let hosting = NSHostingController(rootView: rootView)

        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 460),
            styleMask: [.fullSizeContentView, .titled, .closable],
            backing: .buffered,
            defer: false
        )

        panel.contentView = hosting.view
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = false
        panel.isReleasedWhenClosed = false
        panel.level = .floating
        panel.backgroundColor = .clear
        panel.isOpaque = false

        return panel
    }

    /// Finds the search field in the panel's view tree.
    private func findTextField(in view: NSView) -> NSTextField? {
        if let field = view as? NSTextField { return field }

        for subview in view.subviews {
            if let found = findTextField(in: subview) { return found }
        }

        return nil
    }

    /// Sends one key press through the application.
    private func press(
        _ characters: String,
        keyCode: UInt16,
        to panel: NSPanel
    ) {
        let event = NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: panel.windowNumber,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: characters,
            isARepeat: false,
            keyCode: keyCode
        )!

        NSApp.sendEvent(event)
    }

    /// Types a string, one key press at a time.
    private func type(_ text: String, into panel: NSPanel) {
        for character in text {
            press(String(character), keyCode: 0, to: panel)
        }
    }

    /// Presses the delete key `count` times.
    ///
    /// Backspace is key code 51 and carries U+0008 as its character, which
    /// is what a real keyboard sends.
    private func delete(_ count: Int, from panel: NSPanel) {
        for _ in 0..<count {
            press("\u{8}", keyCode: 51, to: panel)
        }
    }

    /// Waits until `model.query` is `text`, or the deadline passes.
    private func waitForQuery(
        _ text: String,
        in model: AppModel,
        timeout: TimeInterval = 5
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            if model.query == text { return }
            try await Task.sleep(for: .milliseconds(50))
        }
    }

    /// Waits until every row is something `query` could have produced.
    private func waitForResults(
        of query: String,
        in model: AppModel,
        timeout: TimeInterval = 10
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            if !model.results.isEmpty, rowsAllMatch(query, in: model) { return }
            try await Task.sleep(for: .milliseconds(50))
        }
    }

    /// Whether every row is something the query could have produced.
    private func rowsAllMatch(_ query: String, in model: AppModel) -> Bool {
        model.results.allSatisfy { hit in
            isSubsequence(query.lowercased(), of: hit.relativePath.lowercased())
        }
    }

    /// Whether `needle` is a subsequence of `haystack`.
    private func isSubsequence(_ needle: String, of haystack: String) -> Bool {
        var remaining = Array(needle)

        for character in haystack where character == remaining.first {
            remaining.removeFirst()
            if remaining.isEmpty { return true }
        }

        return remaining.isEmpty
    }

    func test_deleting_a_character_runs_the_shorter_search() async throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let model = AppModel()

        let indexed = Date().addingTimeInterval(180)
        while model.isIndexing && Date() < indexed {
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertGreaterThan(model.indexedCount, 0, "the index is empty")

        let panel = makePanel(rootView: ContentView().environmentObject(model))
        panel.makeKeyAndOrderFront(nil)
        app.activate(ignoringOtherApps: true)

        try await Task.sleep(for: .milliseconds(300))

        guard let field = findTextField(in: panel.contentView!) else {
            XCTFail("the search field is not in the panel")
            return
        }

        NotificationCenter.default.post(name: .seekFocusSearch, object: nil)
        try await Task.sleep(for: .milliseconds(200))
        panel.makeFirstResponder(field)

        // A query narrow enough that widening it has to change the list.
        type("readme", into: panel)
        try await waitForQuery("readme", in: model)
        XCTAssertEqual(model.query, "readme", "the typing did not reach the model")

        try await waitForResults(of: "readme", in: model)
        let narrow = model.results
        XCTAssertFalse(narrow.isEmpty, "the first search found nothing")

        // Now take three characters back out. This is the half of the
        // interaction that was never tested.
        delete(3, from: panel)
        try await waitForQuery("rea", in: model)

        XCTAssertEqual(
            model.query,
            "rea",
            "the delete key never reached the model: the field and the model disagree"
        )

        // The stale list also satisfies "every row could have come from
        // `rea`" — `readme` contains `rea` — so waiting on that condition
        // proves nothing here. What has to be waited for is the list
        // *changing*, and generously, so that "never updates" is told
        // apart from "updates late".
        let settle = Date().addingTimeInterval(8)
        var changedAfter: TimeInterval?
        let started = Date()

        while Date() < settle {
            if model.results.map(\.id) != narrow.map(\.id) {
                changedAfter = Date().timeIntervalSince(started)
                break
            }
            try await Task.sleep(for: .milliseconds(50))
        }

        XCTAssertNotNil(
            changedAfter,
            "the list never changed when the query got shorter, even after 8s"
        )
        if let changedAfter {
            print("LIST CHANGED after \(Int(changedAfter * 1000))ms")
        }
        XCTAssertTrue(
            rowsAllMatch("rea", in: model),
            "the panel is still showing the results of a query the user has edited"
        )
    }

    func test_deleting_everything_returns_to_the_hint() async throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let model = AppModel()

        let indexed = Date().addingTimeInterval(180)
        while model.isIndexing && Date() < indexed {
            try await Task.sleep(for: .milliseconds(100))
        }

        let panel = makePanel(rootView: ContentView().environmentObject(model))
        panel.makeKeyAndOrderFront(nil)
        app.activate(ignoringOtherApps: true)
        try await Task.sleep(for: .milliseconds(300))

        guard let field = findTextField(in: panel.contentView!) else {
            XCTFail("the search field is not in the panel")
            return
        }

        NotificationCenter.default.post(name: .seekFocusSearch, object: nil)
        try await Task.sleep(for: .milliseconds(200))
        panel.makeFirstResponder(field)

        type("abc", into: panel)
        try await waitForQuery("abc", in: model)

        delete(3, from: panel)
        try await waitForQuery("", in: model)

        XCTAssertEqual(model.query, "", "the field did not empty")
        XCTAssertTrue(
            model.results.isEmpty,
            "an emptied query should leave the hint, not the last list"
        )
    }
}
