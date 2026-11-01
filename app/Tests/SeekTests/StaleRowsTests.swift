//
//  StaleRowsTests.swift
//  Seek
//
//  The rows on screen belong to the query in the field.
//
//  Every other test here asks the model what it holds, and the model has
//  always been right: `results` holds the new query's rows, `matchedCount`
//  its count, `selectedHit` its best match. None of that is what the user
//  looks at. They look at a list of rows, and the list went on showing the
//  *previous* query's rows — `hello.sh`, `help.py`, with `hel` highlighted
//  — under a field that said `nature`, with the footer and the preview
//  column both already showing the new query's answer.
//
//  So this test looks at pixels. It renders the live view hierarchy, not a
//  fresh one: a bug where SwiftUI keeps a stale row cannot exist in a view
//  that was only just built, which is why no model test and no
//  `ImageRenderer` snapshot ever caught this.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class StaleRowsTests: XCTestCase {
    /// Windows the panels are hosted in.
    ///
    /// Never ordered front: that would take the key window away from the
    /// focus tests.
    private var windows: [NSWindow] = []

    /// The hosting controllers, which the panels do not own: the real
    /// panel keeps its own in a property for the same reason.
    private var hostings: [NSHostingController<AnyView>] = []

    /// The preview setting as the rest of the suite expects to find it.
    ///
    /// `togglePreview` writes the setting through to the preferences, and
    /// a preview left on makes every later panel wider and its list
    /// taller — which is five other tests failing for a reason that has
    /// nothing to do with them.
    private var showPreviewBefore = false

    override func setUp() async throws {
        showPreviewBefore = Preferences.shared.showPreview
    }

    override func tearDown() async throws {
        Preferences.shared.showPreview = showPreviewBefore
        windows = []
        hostings = []
    }

    /// A model with a real index behind it.
    private func indexed() async throws -> AppModel {
        let model = AppModel()
        let deadline = Date().addingTimeInterval(180)

        while model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }

        XCTAssertGreaterThan(model.indexedCount, 1000, "the index is too small to mean anything")

        return model
    }

    /// Puts `model` behind a live panel.
    ///
    /// Built the way `makePanel` builds the real one, down to the
    /// `AnyView` root and the content view being set directly rather than
    /// through a content view controller. A panel assembled any other way
    /// is not the panel the bug was seen in.
    private func host(_ model: AppModel) -> NSView {
        let content = ContentView().environmentObject(model)
        let hosting = NSHostingController(rootView: AnyView(content))
        hostings.append(hosting)

        let panel = NSPanel(
            contentRect: NSRect(
                x: 0,
                y: 0,
                width: Theme.panelWidth,
                height: Theme.panelMaxHeight
            ),
            styleMask: [.fullSizeContentView, .titled],
            backing: .buffered,
            defer: false
        )
        panel.contentView = hosting.view
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = false
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = true

        windows.append(panel)

        hosting.view.layoutSubtreeIfNeeded()

        return hosting.view
    }

    /// Types `query` and waits for the list to settle.
    private func type(_ query: String, into model: AppModel) async throws {
        // One character at a time, the way the query really arrives: each
        // keystroke is its own search, and all but the last are cancelled
        // part-way through by the next one.
        model.query = ""
        for character in query {
            model.query.append(character)
            try await Task.sleep(for: .milliseconds(40))
        }

        let deadline = Date().addingTimeInterval(20)
        while (model.isSearching || model.results.isEmpty) && Date() < deadline {
            try await Task.sleep(for: .milliseconds(50))
        }

        XCTAssertFalse(model.results.isEmpty, "\"\(query)\" found nothing")

        // SwiftUI lays out on its own schedule, so the view is given a
        // moment after the model has settled.
        try await Task.sleep(for: .milliseconds(1200))
    }

    /// The pixels of the results list.
    ///
    /// Only the list: the search field holds the query, which differs
    /// between the two panels by construction, and a caret or a hover
    /// would make a whole-panel comparison fail for no reason.
    private func rows(of view: NSView) throws -> NSBitmapImageRep {
        view.layoutSubtreeIfNeeded()

        let searchBar: CGFloat = 70
        let footer: CGFloat = 40
        let area = NSRect(
            x: 0,
            y: footer,
            width: Theme.panelWidth,
            height: max(1, view.bounds.height - searchBar - footer)
        )

        let rep = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: area))
        view.cacheDisplay(in: area, to: rep)

        return rep
    }

    /// The share of pixels that differ between two captures.
    ///
    /// Byte equality of the PNGs is no good: two captures of the same
    /// rows differ by a few dozen bytes over a scrollbar that fades on
    /// its own schedule, which reads as the bug and is not.
    private func difference(_ a: NSBitmapImageRep, _ b: NSBitmapImageRep) throws -> Double {
        guard a.pixelsWide == b.pixelsWide, a.pixelsHigh == b.pixelsHigh else { return 1 }

        let left = try XCTUnwrap(a.bitmapData)
        let right = try XCTUnwrap(b.bitmapData)
        let bytes = a.bytesPerRow * a.pixelsHigh

        var differing = 0
        for byte in 0..<bytes where left[byte] != right[byte] {
            differing += 1
        }

        return Double(differing) / Double(bytes)
    }

    func test_the_rows_on_screen_belong_to_the_query_in_the_field() async throws {
        // Two queries that share no prefix and no results, so a stale
        // list cannot be mistaken for a correct one.
        let model = try await indexed()

        // The panel in the report had the preview column open, and the
        // preview is the half that was right while the rows were wrong.
        if !model.isPreviewShown {
            model.togglePreview()
        }

        let panel = host(model)

        try await type("hel", into: model)
        let first = try rows(of: panel)

        try await type("nature", into: model)
        let second = try rows(of: panel)

        // What the list should look like. The same model — so the same
        // rows in the same order, which two models would not agree on:
        // each scans for itself, and rows of equal score are separated by
        // their corpus index. Only the view hierarchy is new, and a
        // hierarchy built against the current results cannot be showing
        // an earlier query's.
        let referencePanel = host(model)

        // A `LazyVStack` has nothing to be lazy about until it has been
        // through a layout pass with a viewport, so a view that was only
        // just hosted needs the run loop before it draws anything.
        try await Task.sleep(for: .milliseconds(1500))
        let reference = try rows(of: referencePanel)

        let changed = try difference(first, second)
        let wrong = try difference(second, reference)

        // Without this, a reference that drew nothing at all would fail
        // the comparison below and read as the bug it is meant to detect.
        XCTAssertGreaterThan(changed, 0.05, "the list did not change at all when the query did")

        XCTAssertLessThan(
            wrong,
            changed / 4,
            "the list is showing the previous query's rows: the model holds "
                + "\(model.results.prefix(3).map(\.name)) and the panel is drawing something else"
        )
    }

    /// The same thing after the list has been scrolled.
    ///
    /// A `LazyVStack` only has views to keep once it has built some and
    /// scrolled away from the top. A list still showing its first screen
    /// may well redraw correctly while a scrolled one does not, so the
    /// query is replaced from halfway down the results rather than from
    /// the top.
    func test_the_rows_belong_to_the_query_after_the_list_has_scrolled() async throws {
        let model = try await indexed()

        if !model.isPreviewShown {
            model.togglePreview()
        }

        let panel = host(model)

        try await type("hel", into: model)

        // Down the list, far enough that the rows on screen are ones the
        // lazy stack built on the way there.
        for _ in 0..<60 {
            model.moveSelection(by: 1)
        }
        try await Task.sleep(for: .milliseconds(800))

        let first = try rows(of: panel)

        try await type("nature", into: model)
        let second = try rows(of: panel)

        let referencePanel = host(model)
        try await Task.sleep(for: .milliseconds(1500))
        let reference = try rows(of: referencePanel)

        let changed = try difference(first, second)
        let wrong = try difference(second, reference)

        XCTAssertGreaterThan(changed, 0.05, "the list did not change at all when the query did")

        XCTAssertLessThan(
            wrong,
            changed / 4,
            "a scrolled list went on showing the previous query's rows: the model holds "
                + "\(model.results.prefix(3).map(\.name)) and the panel is drawing something else"
        )
    }
}
