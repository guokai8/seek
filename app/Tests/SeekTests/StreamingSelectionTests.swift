//
//  StreamingSelectionTests.swift
//  Seek
//
//  What happens to the selected row when a streaming search replaces the
//  page under it.
//
//  The ranking is global, so a later chunk can push the row the user is
//  reading from the top of the list to the middle of it. Following the
//  selection by position would move the highlight to a different file
//  while they look at it; following it by id keeps it on the row they
//  chose. Neither is right in every case, which is why these tests spell
//  out which case is which.
//

import Foundation
import XCTest

@testable import Seek

@MainActor
final class StreamingSelectionTests: XCTestCase {
    /// A row with the given id, named after it.
    private func hit(_ id: UInt32, score: Int32 = 100) -> SearchHit {
        SearchHit(
            id: id,
            score: score,
            path: "/tmp/f\(id).txt",
            relativePath: "f\(id).txt",
            name: "f\(id).txt",
            directory: "",
            isDirectory: false,
            positions: []
        )
    }

    func test_a_replaced_page_keeps_the_selection_on_the_row_the_user_chose() throws {
        let model = AppModel()
        model.setTestResults((0..<10).map { hit(UInt32($0)) })

        // The user arrows down to the fourth row.
        model.moveSelection(by: 3)
        let chosen = try XCTUnwrap(model.selectedHit).id
        XCTAssertEqual(chosen, 3)

        // A later chunk turns up better matches, and the chosen row slides
        // down the list.
        model.publishPage([hit(90), hit(91), hit(92)] + (0..<10).map { hit(UInt32($0)) },
                          matched: 13,
                          page: .partial(1), from: model.generation)

        XCTAssertEqual(model.selectedHit?.id, chosen, "the highlight moved to a different file")
        XCTAssertEqual(model.selection, 6)
    }

    func test_a_selection_still_at_the_top_stays_at_the_top() throws {
        // A selection the user has not moved means they are reading the
        // best match — which is whatever the latest page puts first, not
        // the row that used to be there. Following the id here would walk
        // the highlight away from the top as the ranking settled.
        let model = AppModel()
        model.setTestResults((0..<10).map { hit(UInt32($0)) })

        XCTAssertEqual(model.selection, 0)

        model.publishPage([hit(90), hit(91)] + (0..<10).map { hit(UInt32($0)) },
                          matched: 12,
                          page: .partial(1), from: model.generation)

        XCTAssertEqual(model.selection, 0)
        XCTAssertEqual(model.selectedHit?.id, 90, "the top row is not the best match")
    }

    func test_a_new_query_puts_the_selection_back_at_the_top() throws {
        // The reset happens when the query changes, not when its results
        // arrive. A streaming search hands over several pages and the user
        // may arrow down while they are still coming, so a reset that
        // waited for a page would either undo that or race with it.
        let model = AppModel()
        model.setTestResults((0..<10).map { hit(UInt32($0)) })
        model.moveSelection(by: 4)
        XCTAssertEqual(model.selection, 4)

        model.query = "something-else"

        XCTAssertEqual(
            model.selection,
            0,
            "the selection was still where the previous query had left it"
        )
    }

    func test_a_chosen_row_that_falls_off_the_page_clamps() throws {
        // The row the user was on need not survive: a page holds only the
        // best few hundred, and a later chunk can push it off the end.
        let model = AppModel()
        model.setTestResults((0..<10).map { hit(UInt32($0)) })
        model.moveSelection(by: 5)

        model.publishPage([hit(90), hit(91)], matched: 2, page: .partial(1), from: model.generation)

        XCTAssertEqual(model.selection, 1, "the selection ran off the end of the page")
        XCTAssertNotNil(model.selectedHit)
    }

    func test_a_page_from_a_superseded_search_is_dropped() throws {
        // What the user sees when they keep typing: `ha` matches `hash`
        // and `har`, and the moment the query becomes `hash` the `har`
        // rows must not come back. The search for `ha` is abandoned at
        // its next chunk boundary, but a page it had already handed to the
        // main actor can still arrive after the newer query was typed —
        // unstructured tasks of equal priority are not run in the order
        // they were created.
        let model = AppModel()
        model.finishIndexing(count: 100, complete: true)

        model.query = "ha"
        let superseded = model.generation
        model.publishPage([hit(1), hit(2)], matched: 2, page: .partial(0), from: superseded)
        XCTAssertEqual(model.results.map(\.id), [1, 2])

        model.query = "hash"
        model.publishPage(
            [hit(3), hit(4), hit(5)],
            matched: 5,
            page: .partial(1),
            from: superseded
        )

        XCTAssertEqual(
            model.results.map(\.id),
            [1, 2],
            "a page from the previous query put its rows back on screen"
        )

        // And the stale page must not have moved the page number along
        // with it: the new search's own first page would then look older
        // than something already shown, and be dropped.
        model.publishPage([hit(9)], matched: 1, page: .partial(0), from: model.generation)

        XCTAssertEqual(
            model.results.map(\.id),
            [9],
            "the stale page left the page number behind, so the new query's first page was dropped"
        )
    }

    func test_a_page_that_arrives_late_is_dropped() throws {
        // The pages reach the main actor as one unstructured task each,
        // and nothing promises they arrive in the order they were sent. A
        // page older than one already shown holds worse rows, and putting
        // it back on screen would look like the list going backwards.
        let model = AppModel()
        model.publishPage([hit(1)], matched: 1, page: .partial(0), from: model.generation)
        model.publishPage([hit(1), hit(2), hit(3)], matched: 3, page: .partial(4), from: model.generation)

        model.publishPage([hit(9)], matched: 1, page: .partial(2), from: model.generation)

        XCTAssertEqual(model.results.map(\.id), [1, 2, 3], "a stale page was shown")
    }
    func test_a_partial_page_does_not_move_the_match_count() throws {
        // A partial page knows how many candidates have matched *so far*,
        // which is not an answer to "how many matched". Publishing it
        // would make the counter climb during every search, and it would
        // break the one thing the count is relied on for: narrowing a
        // query can only ever match fewer files, so a count that rose as
        // the query grew would be read as a search that was never redone.
        // `loadMoreResults` branches on it too.
        let model = AppModel()

        model.publishPage((0..<3).map { hit(UInt32($0)) }, matched: 12, page: .partial(0), from: model.generation)

        XCTAssertEqual(model.results.count, 3, "the rows are what a partial page is for")
        XCTAssertEqual(model.matchedCount, 0, "a partial page moved the match count")

        model.publishPage((0..<5).map { hit(UInt32($0)) }, matched: 900, page: .complete, from: model.generation)

        XCTAssertEqual(model.matchedCount, 900, "the whole answer did not set the count")
    }

    func test_the_footer_does_not_claim_a_count_it_does_not_have() throws {
        // While a streaming search is in flight the rows on screen can
        // outnumber the last count the model was given — the count stays
        // the previous query's until this one finishes. Reporting it would
        // read as a total smaller than the list under it.
        let model = AppModel()

        // The scan is declared finished, or the footer says "Indexing…"
        // whatever the results are. This also stands in for a new query
        // starting, which is what reopens the model to partial pages.
        model.finishIndexing(count: 100, complete: true)

        model.publishPage((0..<20).map { hit(UInt32($0)) }, matched: 4, page: .partial(0), from: model.generation)

        XCTAssertEqual(model.results.count, 20)
        XCTAssertTrue(
            model.status.contains("20 so far"),
            "the footer claimed a total it does not have: \(model.status)"
        )
    }

    func test_the_footer_reports_the_total_once_the_search_finishes() throws {
        let model = AppModel()
        model.finishIndexing(count: 100, complete: true)

        model.publishPage((0..<20).map { hit(UInt32($0)) }, matched: 4, page: .partial(0), from: model.generation)
        model.publishPage((0..<20).map { hit(UInt32($0)) }, matched: 900, page: .complete, from: model.generation)

        XCTAssertTrue(
            model.status.contains("900 matches"),
            "the footer did not report the total: \(model.status)"
        )
        XCTAssertFalse(model.status.contains("so far"), "the footer is still provisional")
    }
    func test_a_search_says_while_it_is_still_running() async throws {
        // A streaming search hands over several pages before it is done,
        // so "there are rows on screen" no longer means "the search has
        // finished". Anything that waits for a settled list — the
        // watcher's refresh, a test — needs to be able to ask.
        let model = AppModel()
        let indexed = Date().addingTimeInterval(180)
        while model.isIndexing && Date() < indexed {
            try await Task.sleep(for: .milliseconds(100))
        }

        XCTAssertFalse(model.isSearching, "an idle model is not searching")

        model.query = "readme"
        XCTAssertTrue(model.isSearching, "a query was typed and nothing is searching")

        let deadline = Date().addingTimeInterval(60)
        while model.isSearching && Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }

        XCTAssertFalse(model.isSearching, "the search never reported that it had finished")
        XCTAssertFalse(model.results.isEmpty, "the search found nothing")
    }
}
