import AppKit
import SwiftUI
import XCTest

@testable import Seek

/// A harness that renders the panel and saves it as a picture.
///
/// The panel is drawn offscreen, so the picture shows what the user sees
/// without needing the panel to be on screen and the mouse to be over it.
/// It is how the appearance of the panel is checked: the picture is looked
/// at, not asserted about.
@MainActor
final class PanelSnapshotTests: XCTestCase {
    /// Renders the panel with the given query and saves the picture.
    private func snapshot(
        name: String,
        query: String,
        results: [SearchHit]? = nil
    ) async throws {
        let model = AppModel()

        // The index is built, so that the panel shows real entries.
        let deadline = Date().addingTimeInterval(120)
        while model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }

        if let results {
            model.setTestResults(results)
        }

        model.query = query

        // The search runs in the background, so the picture is taken once
        // it has answered rather than after a fixed wait. A fixed wait
        // captures an empty list on a loaded machine, and a picture of the
        // panel before the results arrive is not a picture of the panel.
        try await waitForSearch(of: query, in: model)

        print("SNAPSHOT \(name) results=\(model.results.count)")

        let content = ContentView().environmentObject(model)
        let hosting = NSHostingController(rootView: content)

        // The panel is as tall as its content, so the picture is taken at
        // the size the panel asked for rather than at a fixed one. A fixed
        // size would show the empty states padded out to full height,
        // which is exactly what they no longer do.
        hosting.view.frame = NSRect(origin: .zero, size: NSSize(width: 720, height: 460))
        hosting.view.layoutSubtreeIfNeeded()

        let fitting = hosting.view.fittingSize
        let size = NSSize(width: 720, height: max(fitting.height, 1))
        hosting.view.frame = NSRect(origin: .zero, size: size)
        hosting.view.layoutSubtreeIfNeeded()

        let representation = hosting.view.bitmapImageRepForCachingDisplay(in: hosting.view.bounds)!
        hosting.view.cacheDisplay(in: hosting.view.bounds, to: representation)

        let data = representation.representation(using: .png, properties: [:])!
        let url = URL(fileURLWithPath: "/tmp/seek-\(name).png")
        try data.write(to: url)

        print("SNAPSHOT saved \(url.path)")
    }

    /// Waits until the model holds the answer to `query`.
    ///
    /// An empty list cannot be read as "the search has answered": a query
    /// that matches nothing also ends empty, and so does a search that has
    /// not run yet. The two are told apart by the footer, which only says
    /// "No matches" once the results have been published.
    private func waitForSearch(of query: String, in model: AppModel) async throws {
        let deadline = Date().addingTimeInterval(15)
        let settleAfter = Date().addingTimeInterval(1.5)

        while Date() < deadline {
            settled = Date() > settleAfter
            if hasAnswered(query, in: model) { return }
            try await Task.sleep(for: .milliseconds(50))
        }
    }

    /// Whether enough time has passed for a search to have answered.
    private var settled = false

    /// Whether the model is showing the answer to `query`.
    private func hasAnswered(_ query: String, in model: AppModel) -> Bool {
        if model.isIndexing { return false }

        // The empty query is answered with no results at all: the panel
        // shows its hint rather than the shortest paths in the index. The
        // answer has arrived once the index has, which `isIndexing` above
        // has already established.
        if query.isEmpty { return true }

        // A query that cannot be read is settled the moment the model has
        // published the reason; no search is run for it at all.
        if model.queryError != nil { return true }

        // A query that matched nothing leaves the list empty, which on its
        // own cannot be told from a search that has not run. The footer
        // reports the index size in both cases, so what distinguishes them
        // is that the body of the panel is drawn from `results` — and for
        // an unanswered search the generation counter has not published
        // anything yet. A short settle is enough to tell them apart.
        if model.results.isEmpty { return settled }

        // A list left over from a previous query would not be made up
        // entirely of rows this query matches.
        return !model.results.isEmpty
            && model.results.allSatisfy { hit in
                var remaining = Array(query.lowercased())

                for character in hit.relativePath.lowercased()
                where character == remaining.first {
                    remaining.removeFirst()
                    if remaining.isEmpty { break }
                }

                return remaining.isEmpty
            }
    }

    func test_the_panel_with_results() async throws {
        try await snapshot(name: "results", query: "readme")
    }

    func test_the_panel_with_no_query() async throws {
        try await snapshot(name: "empty", query: "")
    }

    func test_the_panel_with_no_matches() async throws {
        try await snapshot(name: "nomatch", query: "zzzqqqxxx")
    }

    func test_the_panel_with_a_glob() async throws {
        try await snapshot(name: "glob", query: "*.pdf")
    }

    func test_the_panel_with_a_broken_pattern() async throws {
        try await snapshot(name: "badquery", query: "re:[")
    }
}
