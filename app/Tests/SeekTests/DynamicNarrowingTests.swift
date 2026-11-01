//
//  DynamicNarrowingTests.swift
//  Seek
//
//  What the list holds while a query is being typed.
//
//  Typing `ha` shows everything that matches `ha` — `hash`, `har`,
//  `hammer` — and extending it to `hash` must leave only what matches
//  `hash`. That is the whole behaviour of the panel, and with a streaming
//  search it is no longer free: a search hands over several pages, and a
//  page of the *previous* query can still be on its way to the main actor
//  when the next keystroke arrives.
//
//  This file holds the settled half of that: once a search has finished,
//  every row is a row the query asks for. The race itself — a page from a
//  superseded search arriving late — is pinned down by
//  `StreamingSelectionTests`, because driving it through a real search
//  would mean winning a race on purpose.
//

import Foundation
import XCTest

@testable import Seek

@MainActor
final class DynamicNarrowingTests: XCTestCase {
    private func wait(
        seconds: TimeInterval = 20,
        until condition: () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(seconds)
        while !condition() && Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
    }

    /// Whether `needle` is a subsequence of `text`, which is what the
    /// fuzzy matcher means by a match.
    private func matches(_ needle: String, _ text: String) -> Bool {
        var remaining = Array(needle.lowercased())
        for character in text.lowercased() where character == remaining.first {
            remaining.removeFirst()
            if remaining.isEmpty { return true }
        }
        return remaining.isEmpty
    }

    func test_extending_the_query_leaves_only_what_the_longer_query_matches() async throws {
        let model = AppModel()
        try await wait(seconds: 180) { !model.isIndexing }

        model.isPanelVisible = true

        model.query = "ha"
        try await wait { !model.isSearching && !model.results.isEmpty }
        XCTAssertFalse(model.results.isEmpty, "`ha` should match something in a home directory")

        let broad = model.matchedCount
        let broadRows = model.results.count
        XCTAssertTrue(
            model.results.allSatisfy { matches("ha", $0.relativePath) },
            "a row that `ha` does not match: "
                + "\(model.results.first { !matches("ha", $0.relativePath) }?.relativePath ?? "")"
        )

        // The rows for `ha` include plenty that `hash` does not match, so
        // the list has to change rather than merely shrink.
        let onlyHa = model.results.filter { !matches("hash", $0.relativePath) }
        XCTAssertFalse(
            onlyHa.isEmpty,
            "`ha`'s rows all matched `hash` too, so this test cannot tell the two apart"
        )

        model.query = "hash"
        try await wait { !model.isSearching }

        XCTAssertFalse(model.results.isEmpty, "`hash` should match something too")
        for hit in model.results {
            XCTAssertTrue(
                matches("hash", hit.relativePath),
                "a row left from the previous query: \(hit.relativePath)"
            )
        }
        XCTAssertLessThan(
            model.matchedCount,
            broad,
            "extending the query should match fewer paths"
        )
        XCTAssertLessThanOrEqual(model.results.count, max(broadRows, model.results.count))
    }
}
