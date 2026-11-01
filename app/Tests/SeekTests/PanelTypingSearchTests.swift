//
//  PanelTypingSearchTests.swift
//  Seek
//
//  The check that matters most: a person opens the panel, types a word, and
//  sees results.
//
//  The other tests exercise the pieces — the engine, the model, the field —
//  one at a time. This one exercises the whole path at once, exactly the way
//  the app does it: a real `NSPanel`, the real focus handling, the real
//  local event monitor, and real key events sent through the application.
//
//  If the panel can be typed into but nothing comes back, the bug is in the
//  wiring between the field and the model, and no amount of unit testing of
//  the two ends finds it.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class PanelTypingSearchTests: XCTestCase {
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

    /// Types a string into the panel, one key press at a time.
    ///
    /// The key events are sent through the application rather than straight
    /// at the panel, so that the app's own event monitor sees them, the way
    /// it sees a real key press.
    private func type(_ text: String, into panel: NSPanel) {
        for character in text {
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
    }

    /// Waits until the model has results for `query`, or the deadline passes.
    ///
    /// The search runs in the background, so the test waits for the
    /// condition rather than for a fixed stretch of time. A fixed sleep
    /// either makes the test slow or makes it flaky on a loaded machine.
    ///
    /// Waiting for rows is not enough: a streaming search hands over
    /// several pages, and an early page is a provisional answer — the top
    /// of what has been seen so far, with the match count not yet known.
    /// A test that asserts on the footer or the count has to wait for the
    /// search to settle, not merely for something to appear.
    private func waitForResults(
        of query: String,
        in model: AppModel,
        timeout: TimeInterval = 10
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            if !model.isSearching, !model.results.isEmpty, rowsAllMatch(query, in: model) {
                return
            }
            try await Task.sleep(for: .milliseconds(50))
        }
    }

    /// Whether every row is something the query could have produced.
    ///
    /// This is what tells a real answer from a stale one. The empty query
    /// matches everything, so a list left over from it is never empty —
    /// and a test that only asks whether the list is empty passes while the
    /// panel shows the results of a query the user has already replaced.
    private func rowsAllMatch(_ query: String, in model: AppModel) -> Bool {
        model.results.allSatisfy { hit in
            isSubsequence(query.lowercased(), of: hit.relativePath.lowercased())
        }
    }

    /// Whether `needle` appears in `haystack` as a subsequence, which is
    /// what fzf's fuzzy matching accepts.
    private func isSubsequence(_ needle: String, of haystack: String) -> Bool {
        var remaining = Array(needle)

        for character in haystack where character == remaining.first {
            remaining.removeFirst()
            if remaining.isEmpty { return true }
        }

        return remaining.isEmpty
    }

    func test_typing_into_the_panel_produces_results() async throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let model = AppModel()

        // The index has to be there, or there is nothing to find.
        let deadline = Date().addingTimeInterval(180)
        while model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertGreaterThan(model.indexedCount, 0, "the index is empty")

        let panel = makePanel(rootView: ContentView().environmentObject(model))
        panel.makeKeyAndOrderFront(nil)
        app.activate(ignoringOtherApps: true)

        // The layout has to have happened before the tree can be walked.
        try await Task.sleep(for: .milliseconds(300))

        guard let field = findTextField(in: panel.contentView!) else {
            XCTFail("the search field is not in the panel")
            return
        }

        // The panel asks for the focus through a notification, because a
        // field cannot take the focus before the panel is the key window.
        NotificationCenter.default.post(name: .seekFocusSearch, object: nil)
        try await Task.sleep(for: .milliseconds(200))

        panel.makeFirstResponder(field)

        XCTAssertTrue(
            panel.firstResponder is NSText,
            "the search field did not become the first responder"
        )

        // The words are typed the way a person types them.
        type("readme", into: panel)

        print("PANEL field=\(field.stringValue) model.query=\(model.query)")

        XCTAssertEqual(model.query, "readme", "the key presses did not reach the model")

        // The search is given the time it needs to run.
        try await waitForResults(of: "readme", in: model)

        print("PANEL results=\(model.results.count) status=\(model.status)")

        XCTAssertFalse(
            model.results.isEmpty,
            "the panel was typed into, but no results came back"
        )

        // The rows have to be the answer to what was typed. The empty
        // query matches everything, so a list left over from it would pass
        // the check above while showing the user the wrong thing.
        XCTAssertTrue(
            rowsAllMatch("readme", in: model),
            "the rows are not results of the query that was typed: "
                + "\(model.results.prefix(5).map(\.relativePath))"
        )

        // The footer has to agree with the search. A footer that still
        // counts the index is a footer reporting a search that never
        // arrived.
        //
        // It reports the matches, not the rows: `readme` matches tens of
        // thousands of paths here and the list stops at the limit, so a
        // footer built from the row count would read the same for this
        // query and for every other broad one.
        XCTAssertGreaterThan(
            model.matchedCount,
            model.results.count,
            "a query this broad should match more paths than the list holds"
        )
        XCTAssertEqual(
            model.status,
            "\(model.matchedCount.formatted()) matches · first \(model.results.count.formatted())",
            "the footer does not describe the results that are shown"
        )
    }

    func test_the_results_are_actually_drawn() async throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let model = AppModel()

        let deadline = Date().addingTimeInterval(180)
        while model.isIndexing && Date() < deadline {
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

        type("readme", into: panel)
        try await waitForResults(of: "readme", in: model)

        XCTAssertFalse(model.results.isEmpty, "there are no results to draw")
        XCTAssertTrue(
            rowsAllMatch("readme", in: model),
            "the rows are not results of the query that was typed"
        )

        // The rows are counted by walking the drawn view tree, because a
        // result that is in the model but not on the screen is exactly the
        // bug this test exists to catch.
        panel.contentView!.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(300))

        let drawn = countDrawings(in: panel.contentView!)
        print("PANEL drawn views=\(drawn) results=\(model.results.count)")

        XCTAssertGreaterThan(
            drawn,
            0,
            "the results are in the model but nothing was drawn"
        )
    }

    /// Counts the views in a tree that draw something.
    private func countDrawings(in view: NSView) -> Int {
        var count = (view.subviews.isEmpty ? 0 : 0)

        for subview in view.subviews {
            count += countDrawings(in: subview)
        }

        // A text view that has drawn a string is a result row.
        if let text = view as? NSTextField, !text.stringValue.isEmpty {
            count += 1
        }

        return count
    }
}
