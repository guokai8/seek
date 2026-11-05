//
//  SelectAllTests.swift
//  Seek
//
//  The query has to be editable, not just typeable.
//
//  A launcher that can only be typed into once is not a launcher. The user
//  searches for "nature", decides they meant "review", selects the query and
//  types over it — and the results change with it, the way they do in fzf.
//
//  This went wrong because the app is an accessory app with no menu bar of
//  its own, so there was no Edit menu to own the Select All shortcut. The
//  key was swallowed by nothing at all, the selection never happened, and
//  the new text was appended instead of replacing the old: "nature" became
//  "naturereview".
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class SelectAllTests: XCTestCase {
    /// A small folder with a file for each of the two queries.
    private func makeFixture() throws -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("seek-select-\(UUID().uuidString)")

        let docs = root.appendingPathComponent("docs")
        try FileManager.default.createDirectory(at: docs, withIntermediateDirectories: true)

        for name in ["nature-of-the-day.md", "nature-notes.txt"] {
            try "hello".write(
                to: docs.appendingPathComponent(name),
                atomically: true,
                encoding: .utf8
            )
        }

        for name in ["review.md", "review-notes.txt"] {
            try "hello".write(
                to: docs.appendingPathComponent(name),
                atomically: true,
                encoding: .utf8
            )
        }

        return root
    }

    private func findTextField(in view: NSView) -> NSTextField? {
        if let field = view as? NSTextField { return field }

        for subview in view.subviews {
            if let found = findTextField(in: subview) { return found }
        }

        return nil
    }

    /// Sends a key press the way the system sends it.
    ///
    /// A command chord carries the *control* character in `characters`, but
    /// `charactersIgnoringModifiers` still holds the plain key. Both have to
    /// be right for the chord to be recognised.
    private func chord(
        _ control: UnicodeScalar,
        plain: String,
        code: UInt16,
        in panel: NSPanel
    ) {
        let event = NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [.command],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: panel.windowNumber,
            context: nil,
            characters: String(control),
            charactersIgnoringModifiers: plain,
            isARepeat: false,
            keyCode: code
        )!

        NSApp.sendEvent(event)
    }

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

    /// Waits until the results are the ones the query asks for.
    ///
    /// The check is not just that the results are non-empty: the results of
    /// the *previous* query are still on screen until the new search
    /// finishes, and a test that accepts them would pass on a panel that
    /// never updated at all.
    private func waitForResults(
        matching query: String,
        in model: AppModel,
        timeout: TimeInterval
    ) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            try? await Task.sleep(for: .milliseconds(20))

            guard model.query == query, !model.results.isEmpty else { continue }

            // Every result has to be one the query asks for. The names are
            // checked rather than the paths, because the paths carry the
            // directory as well.
            if model.results.allSatisfy({ $0.name.contains(query) }) {
                return true
            }
        }

        return false
    }

    func test_the_query_can_be_selected_and_typed_over() async throws {
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

        guard let panel = delegate.testPanel else {
            XCTFail("the app did not open a panel")
            return
        }

        let deadline = Date().addingTimeInterval(60)
        while delegate.model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }

        guard let field = findTextField(in: panel.contentView!) else {
            XCTFail("the search field is not in the panel")
            return
        }

        NotificationCenter.default.post(name: .seekFocusSearch, object: nil)
        try await Task.sleep(for: .milliseconds(300))

        // The panel has to be the key window and the field the responder,
        // or the shortcut has nowhere to go.
        panel.makeKey()
        panel.makeFirstResponder(field)

        // The focus is waited for rather than assumed: the editor only
        // exists once the field has taken the focus.
        let focusDeadline = Date().addingTimeInterval(3)
        while field.currentEditor() == nil, Date() < focusDeadline {
            try await Task.sleep(for: .milliseconds(50))
        }

        XCTAssertNotNil(
            field.currentEditor(),
            "the search field never took the focus"
        )

        let model = delegate.model

        // ---- the first query -------------------------------------------
        type("nature", into: panel)

        let firstArrived = await waitForResults(matching: "nature", in: model, timeout: 10)
        XCTAssertTrue(firstArrived, "the first query found nothing")

        let firstNames = model.results.map(\.name)
        print("SELECT first='\(model.query)' names=\(firstNames)")

        XCTAssertEqual(model.query, "nature")

        // ---- the query is selected --------------------------------------
        //
        // Command-A, the way the system reports it.
        chord("\u{1}", plain: "a", code: 0, in: panel)
        try await Task.sleep(for: .milliseconds(200))

        // `currentEditor()` already hands back an `NSText?`, so there is
        // nothing to cast. The cast that used to be here was a no-op the
        // compiler warned about.
        let editor = field.currentEditor()
        print("SELECT after-cmd-a range=\(String(describing: editor?.selectedRange))")

        XCTAssertEqual(
            editor?.selectedRange.length,
            6,
            "the query was not selected, so typing over it would append"
        )

        // ---- and typed over ---------------------------------------------
        type("review", into: panel)

        let secondArrived = await waitForResults(matching: "review", in: model, timeout: 10)

        print("SELECT second='\(model.query)' names=\(model.results.map(\.name))")

        XCTAssertEqual(
            model.query,
            "review",
            "the query was not replaced"
        )

        XCTAssertTrue(secondArrived, "the changed query found nothing")

        XCTAssertNotEqual(
            model.results.map(\.name),
            firstNames,
            "the results did not change with the query"
        )

        XCTAssertTrue(
            model.results.allSatisfy { $0.name.contains("review") },
            "the results are not the ones the new query asks for"
        )

        panel.orderOut(nil)
    }
}
