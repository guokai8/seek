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

    /// The message comes back from the core as bytes, and the bytes are
    /// the user's own query quoted back at them — so a query with
    /// multi-byte characters in it is the case where the conversion from
    /// the core's buffer to a Swift string has to be right. An ASCII
    /// message survives almost any mistake in that conversion; a Chinese
    /// one does not.
    func test_a_broken_pattern_with_multibyte_characters_comes_back_whole() {
        guard let message = SearchEngine.queryError("re:[文档") else {
            XCTFail("an unclosed character class should be reported")
            return
        }

        XCTAssertTrue(
            message.contains("文档"),
            "the query did not survive the trip through the core: \(message)"
        )
        XCTAssertFalse(
            message.contains("\u{FFFD}"),
            "the message came back with replacement characters in it: \(message)"
        )

        // Both ends of the buffer are pinned, because the message is read
        // by the length the core reports and the two ways of getting that
        // length wrong fail differently: reading one byte too far brings
        // the terminator back with it, and reading one byte too few eats
        // the last character. Neither shows up in the Chinese part, which
        // sits in the middle.
        // Escaped, because the thing this assertion catches is a NUL in
        // the message — and a NUL printed raw ends the line the test
        // failure is written on, so the report comes back empty.
        XCTAssertFalse(
            message.contains("\0"),
            "the message was read past its end: \(String(reflecting: message))"
        )
        XCTAssertTrue(
            message.hasSuffix("unclosed character class"),
            "the message was cut short: \(message)"
        )
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
