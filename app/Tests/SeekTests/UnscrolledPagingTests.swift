//
//  UnscrolledPagingTests.swift
//  Seek
//
//  The second page is fetched when the user scrolls to it, not before.
//
//  `loadMoreResults` says so in its own documentation — "it is paid when
//  the user scrolls, never on a keystroke" — and the panel was not doing
//  it. A search for `nihao`, with the list sitting on its first row and
//  nothing touched, reported `174,504 matches · first 400`: the second
//  page had already been fetched and the whole result set doubled for
//  rows nobody had asked to see. (That query was typed in what is now
//  the panel's fuzzy mode; the probe below is one that is broad in
//  either.)
//
//  The trigger is a row's `onAppear`, so this needs a real hosted panel.
//  A model on its own never calls it, which is why every paging test so
//  far has been green.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class UnscrolledPagingTests: XCTestCase {
    private var windows: [NSWindow] = []

    /// The preview setting as the rest of the suite expects it.
    private var showPreviewBefore = false
    private var hostings: [NSHostingController<AnyView>] = []

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

    /// Puts `model` behind a panel built the way `makePanel` builds it.
    private func host(_ model: AppModel) {
        let hosting = NSHostingController(rootView: AnyView(ContentView().environmentObject(model)))
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
        panel.isReleasedWhenClosed = false
        panel.level = .floating
        panel.backgroundColor = .clear
        panel.isOpaque = false
        windows.append(panel)

        hosting.view.layoutSubtreeIfNeeded()
    }

    func test_a_list_nobody_scrolled_holds_one_page() async throws {
        let model = try await indexed()

        // The panel in the report had the preview column open, which pins
        // the list to its full height instead of letting it shrink to the
        // rows it holds.
        if !model.isPreviewShown {
            model.togglePreview()
        }

        host(model)

        // A broad query in either mode. `nihao` was the one in the
        // report, and it is a fuzzy-only one: the panel matches exactly
        // now, so those five letters match nothing and there would be no
        // second page to fetch. Every name with an extension holds a dot.
        for character in "." {
            model.query.append(character)
            try await Task.sleep(for: .milliseconds(700))
        }

        let deadline = Date().addingTimeInterval(20)
        while (model.isSearching || model.results.isEmpty) && Date() < deadline {
            try await Task.sleep(for: .milliseconds(50))
        }

        XCTAssertGreaterThan(
            model.matchedCount,
            AppModel.firstPage,
            "the query matched one page or less, so there is no second page to fetch"
        )

        // Long enough for a row's `onAppear` to have fired and for the
        // fetch it starts to have landed.
        try await Task.sleep(for: .milliseconds(2000))

        XCTAssertEqual(
            model.results.count,
            AppModel.firstPage,
            "the panel fetched \(model.results.count) rows for a list that was never "
                + "scrolled — the whole result set doubled for rows nobody asked to see"
        )
    }

    /// A row, named after its id.
    private func row(_ id: UInt32) -> SearchHit {
        SearchHit(
            id: id,
            score: 100,
            path: "/tmp/f\(id).txt",
            relativePath: "f\(id).txt",
            name: "f\(id).txt",
            directory: "",
            isDirectory: false,
            positions: []
        )
    }

    /// `count` rows.
    private func rows(_ count: Int) -> [SearchHit] {
        (0..<count).map { row(UInt32($0)) }
    }

    /// The panel asked for the second page before the first had finished
    /// arriving.
    ///
    /// Nothing here is scrolled and nothing is selected past the top. The
    /// two halves that make it happen are both ordinary:
    ///
    /// * `matchedCount` is written only when a search *completes*, so
    ///   while the next query streams in, the count on hand is the
    ///   previous query's — by design, so the footer does not report a
    ///   total smaller than the list under it.
    /// * a streaming search publishes its answer in chunks, and an early
    ///   chunk is short.
    ///
    /// Put together, a row a dozen down a 32-row chunk satisfies "within
    /// twenty of the end", and the stale count says there is plenty more
    /// to fetch. `seek` then doubled the whole result set for rows
    /// nobody had asked to see: a search for `nihao` sat on its first row
    /// reporting `174,504 matches · first 400`.
    ///
    /// This is a test and not the hosted panel because a debug build is
    /// too slow to show it — the intermediate queries never finish
    /// between keystrokes, so `matchedCount` stays at zero and the guard
    /// below is never reached. In the release build the user runs, they
    /// finish every time.
    func test_a_chunk_that_is_still_streaming_is_not_the_end_of_the_list() {
        let model = AppModel()

        // A finished query leaves its count behind.
        model.query = "zzz"
        model.publishPage(rows(200), matched: 200_000, page: .complete, from: model.generation)

        XCTAssertEqual(model.matchedCount, 200_000, "the completed search did not set a count")

        // The next keystroke, and the first chunk of its answer.
        model.query = "zzzz"
        model.publishPage(rows(32), matched: 0, page: .partial(0), from: model.generation)

        XCTAssertEqual(model.results.count, 32, "the chunk did not land")
        XCTAssertEqual(
            model.matchedCount,
            200_000,
            "the count is the previous query's until this one finishes, which is the "
                + "whole point of this test"
        )

        XCTAssertFalse(
            model.shouldLoadMore(forRow: 14),
            "row 14 of a 32-row chunk that is still streaming counted as the end of "
                + "the list, so the panel fetched the second page before the first "
                + "had finished arriving"
        )
    }
}
