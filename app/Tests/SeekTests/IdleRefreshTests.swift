import XCTest

@testable import Seek

/// The watcher reports changes for as long as the app runs, and each
/// report used to re-run the search and republish the whole list — even
/// with the panel put away and nobody looking at it. On a developer's
/// machine that is a search every few hundred milliseconds, all day.
@MainActor
final class IdleRefreshTests: XCTestCase {
    /// Waits until `condition` holds, or the deadline passes.
    private func wait(
        seconds: TimeInterval = 5,
        until condition: () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(seconds)
        while !condition() && Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    func test_the_watcher_refreshes_the_list_while_the_panel_is_open() async throws {
        let model = AppModel()
        try await wait(seconds: 180) { !model.isIndexing }

        model.isPanelVisible = true
        model.query = "readme"
        try await wait { !model.results.isEmpty }
        XCTAssertFalse(model.results.isEmpty, "the query should match something")

        // The list is emptied by hand. A refresh fills it again, because
        // the query still matches; no refresh leaves it as it is.
        model.setTestResults([])
        model.indexChanged(count: model.indexedCount + 1)

        try await wait { !model.results.isEmpty }
        XCTAssertFalse(
            model.results.isEmpty,
            "the watcher should refresh the list of an open panel"
        )
    }

    func test_the_watcher_does_not_refresh_a_panel_that_is_put_away() async throws {
        let model = AppModel()
        try await wait(seconds: 180) { !model.isIndexing }

        model.isPanelVisible = true
        model.query = "readme"

        // Waiting for rows is not enough: a streaming search hands over
        // several pages, and a page still in flight would refill the list
        // after it is emptied below, which looks exactly like the refresh
        // this test is meant to catch.
        try await wait { !model.isSearching && !model.results.isEmpty }
        XCTAssertFalse(model.results.isEmpty, "the query should match something")

        // The panel is put away with the query still in it, which is what
        // the escape key and the auto-hide timer both leave behind.
        model.isPanelVisible = false
        model.setTestResults([])
        model.indexChanged(count: model.indexedCount + 1)

        // Long enough for a refresh to have landed if one had been started.
        try await Task.sleep(for: .milliseconds(1500))

        XCTAssertTrue(
            model.results.isEmpty,
            "a hidden panel searched anyway: \(model.results.count) results arrived"
        )
    }

    func test_the_index_count_is_still_kept_up_to_date_while_hidden() async throws {
        let model = AppModel()
        try await wait(seconds: 180) { !model.isIndexing }

        model.isPanelVisible = false
        let reported = model.indexedCount + 77
        model.indexChanged(count: reported)

        // Not searching is not the same as not listening: the panel has to
        // show the right number of files the next time it is opened.
        XCTAssertEqual(model.indexedCount, reported)
    }
}
