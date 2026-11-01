//
//  QueryErrorTests.swift
//  Seek
//
//  A query that cannot be parsed has no results, and no results looks
//  exactly like a query that matched nothing. These pin the difference,
//  because it is the whole point of showing the reason.
//

import XCTest

@testable import Seek

@MainActor
final class QueryErrorTests: XCTestCase {
    func test_a_good_query_has_no_error() {
        for query in ["readme", "*.pdf", "src/*.rs", "re:^src/.*\\.rs$", "'exact'", "^doc", "pdf$"] {
            XCTAssertNil(
                SearchEngine.queryError(query),
                "\(query) should parse"
            )
        }
    }

    func test_a_broken_pattern_says_what_is_wrong() {
        guard let regex = SearchEngine.queryError("re:[") else {
            XCTFail("an unclosed character class should be reported")
            return
        }

        XCTAssertTrue(
            regex.contains("unclosed character class"),
            "the message does not say what is wrong: \(regex)"
        )

        // One line, because the panel shows it on one line.
        XCTAssertFalse(regex.contains("\n"), "the message is multi-line: \(regex)")

        XCTAssertNotNil(SearchEngine.queryError("*[.pdf"), "a broken glob should be reported")
    }

    func test_content_search_is_refused_by_name() {
        guard let message = SearchEngine.queryError("content:hello") else {
            XCTFail("content: is not implemented and should say so")
            return
        }

        XCTAssertTrue(message.contains("content:"), "\(message)")
    }

    func test_a_half_typed_quote_is_not_an_error() {
        // The search treats it as the text typed so far, so the panel
        // should stay quiet rather than scold mid-word.
        XCTAssertNil(SearchEngine.queryError("'read"))
    }

    func test_the_model_publishes_the_reason_and_clears_it() async throws {
        let model = AppModel()

        // The index has to be built first: `Indexing…` outranks everything
        // else in the status line, and rightly so.
        let deadline = Date().addingTimeInterval(120)
        while model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertFalse(model.isIndexing, "the index was not built in time")

        model.query = "re:["
        try await Task.sleep(for: .milliseconds(300))

        XCTAssertNotNil(model.queryError, "the model did not publish the reason")
        XCTAssertTrue(model.results.isEmpty, "a query that cannot be read has no results")

        // The footer must not report an index size for a search that never
        // ran; the body is carrying the reason.
        XCTAssertEqual(model.status, "")

        model.query = "readme"
        try await Task.sleep(for: .milliseconds(300))

        XCTAssertNil(model.queryError, "the reason outlived the query that caused it")
    }
}
