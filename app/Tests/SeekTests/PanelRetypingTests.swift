import AppKit
import SwiftUI
import XCTest

@testable import Seek

/// Typing into the panel the way a person does.
///
/// Most of the tests here set `model.query` directly, which skips the
/// search field and its binding entirely. This one sends real key events
/// to the panel, so each character goes through the field, its field
/// editor and the binding before it reaches the model.
///
/// The sequence is the one that was reported as broken: type a word, see
/// results, delete the whole thing, then type something else. It passed
/// the first time it was run — which is how the search for the fault moved
/// off the typing path and onto the work the app was doing in the
/// background — and it is kept so that it cannot break unnoticed.
@MainActor
final class PanelRetypingTests: XCTestCase {
    /// A panel with the real view in it, focused and ready to be typed into.
    private func makePanel(_ model: AppModel) async throws -> NSPanel {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let hosting = NSHostingController(rootView: ContentView().environmentObject(model))
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 460),
            styleMask: [.titled, .fullSizeContentView, .closable],
            backing: .buffered,
            defer: false
        )
        panel.contentView = hosting.view
        panel.level = .floating
        panel.makeKeyAndOrderFront(nil)
        app.activate(ignoringOtherApps: true)
        model.isPanelVisible = true
        model.requestSearchFocus()
        try await Task.sleep(for: .milliseconds(600))

        return panel
    }

    /// Presses a key, and lets the app see it.
    private func press(_ chars: String, _ keyCode: UInt16, on panel: NSPanel) async throws {
        let event = NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: panel.windowNumber,
            context: nil,
            characters: chars,
            charactersIgnoringModifiers: chars,
            isARepeat: false,
            keyCode: keyCode
        )!

        // Handed straight to the panel, the way the other typing tests do
        // it, rather than posted to `NSApp` and drained off its queue.
        //
        // That queue belongs to the process and not to this test: the other
        // typing tests leave their own key events on it, and draining it
        // delivered those into this panel too, which is how a typed "human"
        // came back as "Shuemuarn" with a stray "s" and "e" threaded
        // through it. Clearing it first and then filtering what came back
        // out both failed, for the same underlying reason -- whether
        // `NSApp.sendEvent` reaches a panel that the window server has not
        // made key is not something this test can know, so a key was
        // either dropped or delivered twice ("hhuummaann").
        //
        // Nothing is lost by not going through `NSApp`. The app's own key
        // monitor is installed by the delegate, and this test builds its
        // panel itself, so the monitor was never in this path to begin
        // with -- it is covered by `AppDelegateIntegrationTests`, which
        // runs the real delegate. What is particular to this test is the
        // order the keys come in: a word, the whole word deleted, then a
        // fresh one, which is the sequence that was reported as broken.
        panel.sendEvent(event)

        try await Task.sleep(for: .milliseconds(120))
    }

    /// Waits for `condition`, or gives up.
    private func wait(seconds: TimeInterval = 5, until condition: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(seconds)
        while !condition() && Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    func test_a_fresh_word_after_deleting_the_last_one_searches_again() async throws {
        let model = AppModel()
        try await wait(seconds: 180) { !model.isIndexing }
        let panel = try await makePanel(model)

        let human: [(String, UInt16)] = [("h", 4), ("u", 32), ("m", 46), ("a", 0), ("n", 45)]
        for (character, code) in human {
            try await press(character, code, on: panel)
        }

        try await wait { model.matchedCount > 0 }
        XCTAssertEqual(model.query, "human", "the keys should reach the model")
        XCTAssertGreaterThan(model.matchedCount, 0, "typing a word should find something")

        // Backspace over the whole word, one key at a time.
        for _ in 0..<human.count {
            try await press("\u{8}", 51, on: panel)
        }

        try await wait { model.query.isEmpty }
        XCTAssertEqual(model.query, "", "backspace should empty the query")
        try await wait { model.results.isEmpty }
        XCTAssertTrue(model.results.isEmpty, "an empty query has no results")

        // The report: after this, nothing changed however much was typed.
        let fresh: [(String, UInt16)] = [("r", 15), ("e", 14), ("a", 0)]
        var counts: [Int] = []

        for (character, code) in fresh {
            let before = model.matchedCount
            try await press(character, code, on: panel)
            try await wait { model.matchedCount != before }
            counts.append(model.matchedCount)
        }

        XCTAssertEqual(model.query, "rea", "the fresh keys should reach the model too")
        XCTAssertGreaterThan(model.results.count, 0, "the fresh query found nothing")
        XCTAssertEqual(
            Set(counts).count, counts.count,
            "the match count did not move for every key: \(counts)"
        )

        // Narrowing a query can only ever match fewer files, so a count
        // that does not fall is a count that was never recomputed.
        for (earlier, later) in zip(counts, counts.dropFirst()) {
            XCTAssertLessThan(later, earlier, "narrowing the query did not narrow the matches")
        }

        panel.orderOut(nil)
    }
}
