//
//  StreamingSearchTests.swift
//  Seek
//
//  The streaming search: rows handed to the panel while the search is
//  still running.
//
//  A search over a quarter of a million paths takes tens of milliseconds,
//  and the panel used to show nothing at all until it finished. The point
//  of streaming is that the first rows are drawable almost at once — so
//  these tests insist not only that a page arrives early, but that what
//  arrives is a row the panel could actually draw: a resolved path, a
//  name, and the matched offsets to highlight. A page of empty shells
//  would satisfy a weaker test and show the user nothing.
//

import Foundation
import XCTest

@testable import Seek

final class StreamingSearchTests: XCTestCase {
    /// How many files the fixture holds.
    ///
    /// Enough that the search takes more than one chunk, and that a
    /// stopped search provably stops short.
    private static let files = 3000

    /// How many candidates go by between reports.
    private static let chunk = 400

    /// Builds an engine over a fresh directory, scanned and ready.
    ///
    /// The names scatter `report`'s letters through most of the files and
    /// put it right after a separator in a few, so the ranking has
    /// something to change its mind about as the chunks go by.
    private func makeEngine() throws -> (SearchEngine, URL) {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("seek-streaming-\(UUID().uuidString)")
        let good = root.appendingPathComponent("report")
        try FileManager.default.createDirectory(at: good, withIntermediateDirectories: true)

        // The names hold the query as a run, not merely in order: the
        // panel matches exactly by default, so `r-e-p-o-r-t-0.log` is a
        // file these tests would not find and the pages they are about
        // would carry a handful of rows instead of thousands.
        for i in 0..<Self.files {
            try "x".write(
                to: root.appendingPathComponent("report-\(i).log"),
                atomically: false,
                encoding: .utf8
            )
        }
        for i in 0..<40 {
            try "x".write(
                to: good.appendingPathComponent("\(i).txt"),
                atomically: false,
                encoding: .utf8
            )
        }

        let engine = SearchEngine()
        engine.setRoots([root.path])
        engine.setExcludes([])

        XCTAssertTrue(engine.startScan())
        let deadline = Date().addingTimeInterval(60)
        while !engine.scanFinished {
            XCTAssertLessThan(Date(), deadline, "the scan did not finish")
            usleep(10_000)
        }
        let count = engine.finishScan()
        XCTAssertGreaterThanOrEqual(count, Self.files, "indexed \(count) entries")

        return (engine, root)
    }

    func test_a_broad_search_reports_rows_before_it_finishes() throws {
        let (engine, root) = try makeEngine()
        defer { try? FileManager.default.removeItem(at: root) }

        var pages: [SearchOutcome] = []
        let outcome = engine.search("report", limit: 20, chunk: Self.chunk) { page in
            pages.append(page)
            return true
        }

        XCTAssertGreaterThan(
            pages.count,
            1,
            "a search over \(Self.files) files in chunks of \(Self.chunk) reported \(pages.count) times"
        )
        XCTAssertFalse(outcome.hits.isEmpty, "the search found nothing")

        // The running count only grows, and ends at the whole of it.
        for pair in zip(pages, pages.dropFirst()) {
            XCTAssertGreaterThanOrEqual(pair.1.matched, pair.0.matched)
        }
        XCTAssertEqual(pages.last?.matched, outcome.matched)
    }

    func test_a_reported_page_is_drawable() throws {
        // The reason streaming exists. A page whose rows have no resolved
        // path, no name or no highlight offsets would arrive early and
        // show nothing, and every other test here would still pass.
        let (engine, root) = try makeEngine()
        defer { try? FileManager.default.removeItem(at: root) }

        var first: SearchOutcome?
        _ = engine.search("report", limit: 20, chunk: Self.chunk) { page in
            if first == nil, !page.hits.isEmpty { first = page }
            return true
        }

        let page = try XCTUnwrap(first, "no page with rows ever arrived")
        let hit = try XCTUnwrap(page.hits.first)

        // Not `hasPrefix(root.path)`: macOS symlinks /var to /private/var,
        // so the resolved path and the URL the fixture was built at do not
        // share a prefix even when both name the same file. What the row
        // needs is an absolute path that ends in the entry it stands for,
        // because that is what opening and revealing it use.
        XCTAssertTrue(hit.path.hasPrefix("/"), "the path is not absolute: \(hit.path)")
        XCTAssertTrue(
            hit.path.hasSuffix(hit.relativePath),
            "the full path \(hit.path) does not end in the relative path \(hit.relativePath)"
        )
        XCTAssertTrue(
            FileManager.default.fileExists(atPath: hit.path),
            "the resolved path does not exist: \(hit.path)"
        )
        XCTAssertFalse(hit.name.isEmpty, "the row has no name to draw")
        XCTAssertFalse(hit.relativePath.isEmpty, "the row has no relative path")
        XCTAssertFalse(hit.positions.isEmpty, "the row has nothing to highlight")
    }

    func test_the_final_outcome_is_what_a_plain_search_returns() throws {
        // Streaming changes when rows appear, never which rows they are.
        let (engine, root) = try makeEngine()
        defer { try? FileManager.default.removeItem(at: root) }

        let streamed = engine.search("report", limit: 20, chunk: Self.chunk) { _ in true }
        let plain = engine.search("report", limit: 20)

        XCTAssertEqual(streamed.matched, plain.matched)
        XCTAssertEqual(streamed.hits.map(\.id), plain.hits.map(\.id))
        XCTAssertEqual(streamed.hits.map(\.score), plain.hits.map(\.score))
        XCTAssertEqual(streamed.hits.map(\.positions), plain.hits.map(\.positions))
    }

    func test_the_last_reported_page_agrees_with_the_final_outcome() throws {
        // The panel draws the last page and then the final outcome. If the
        // two could differ, the list would flick to a different answer at
        // the very end of every search.
        let (engine, root) = try makeEngine()
        defer { try? FileManager.default.removeItem(at: root) }

        var last: SearchOutcome?
        let outcome = engine.search("report", limit: 20, chunk: Self.chunk) { page in
            last = page
            return true
        }

        XCTAssertEqual(last?.hits.map(\.id), outcome.hits.map(\.id))
    }

    func test_returning_false_abandons_the_search() throws {
        // What makes a superseded keystroke cheap. Without it, a query the
        // user has already typed past runs to completion.
        let (engine, root) = try makeEngine()
        defer { try? FileManager.default.removeItem(at: root) }

        var reports = 0
        let stopped = engine.search("report", limit: 20, chunk: Self.chunk) { _ in
            reports += 1
            return false
        }
        let whole = engine.search("report", limit: 20, chunk: Self.chunk) { _ in true }

        XCTAssertEqual(reports, 1, "the search kept going after it was told to stop")
        XCTAssertLessThan(
            stopped.matched,
            whole.matched,
            "a stopped search reported \(stopped.matched) of \(whole.matched) matches"
        )
    }

    func test_one_chunk_reports_once() throws {
        // A narrow query is answered in well under a millisecond, and
        // reporting it in pieces would cost the app a SwiftUI pass per
        // piece for nothing.
        let (engine, root) = try makeEngine()
        defer { try? FileManager.default.removeItem(at: root) }

        var reports = 0
        _ = engine.search("report", limit: 20, chunk: 0) { _ in
            reports += 1
            return true
        }

        XCTAssertEqual(reports, 1)
    }
    func test_a_partial_page_resolves_only_the_rows_that_can_be_looked_at() throws {
        // Resolving a page's paths and highlight offsets is the expensive
        // half of answering a search, and a provisional page exists to be
        // *looked at*: nobody reads row 180 of a list that will be
        // replaced in four milliseconds. So a partial page carries enough
        // rows to fill the panel and leave it scrollable, and the complete
        // answer carries the rest.
        //
        // Measured over a home directory, resolving the whole limit on
        // every page put 20ms on top of a keystroke — more than the search
        // it was meant to hide.
        let (engine, root) = try makeEngine()
        defer { try? FileManager.default.removeItem(at: root) }

        var partials: [Int] = []
        let outcome = engine.search("report", limit: 200, chunk: Self.chunk) { page in
            partials.append(page.hits.count)
            return true
        }

        let intermediate = partials.dropLast()
        XCTAssertFalse(intermediate.isEmpty, "the search did not report before it finished")

        for count in intermediate {
            XCTAssertLessThanOrEqual(
                count,
                SearchEngine.partialRows,
                "a partial page resolved \(count) rows"
            )
        }

        // And the panel can be filled from one: fewer rows than it shows
        // would make the list grow in height as the search went on.
        XCTAssertGreaterThan(
            partials.first ?? 0,
            SearchEngine.panelRows,
            "the first page cannot fill the panel"
        )

        // The complete answer is not capped.
        XCTAssertGreaterThan(
            outcome.hits.count,
            SearchEngine.partialRows,
            "the complete answer was capped like a partial page"
        )
    }
}
