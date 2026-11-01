//
//  ResultPagingTests.swift
//  Seek
//
//  The panel used to show the best two hundred matches and stop there, so
//  a query matching 166,000 paths let you look at 200 of them. Asking for
//  all of them is not an option: ranking and resolving every match of a
//  broad query takes seconds, against 33ms for the first page. So the list
//  grows when the user reaches the end of it.
//
//  Growing a ranked list is only safe because the ranking is a total order
//  — score, then pathname, then length, then input position — which makes
//  the best N unique. The longer list therefore *begins* with the shorter
//  one, and the rows the user is reading do not move under them. That is
//  the property these tests pin; without it, scrolling would reshuffle
//  the list.
//

import XCTest

@testable import Seek

@MainActor
final class ResultPagingTests: XCTestCase {
    private var root: URL!

    /// A directory with far more matches than one page holds.
    override func setUp() async throws {
        root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("seek-paging-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)

        for i in 0..<900 {
            FileManager.default.createFile(
                at: root.appendingPathComponent("report-\(i).md")
            )
        }
    }

    override func tearDown() async throws {
        if let root { try? FileManager.default.removeItem(at: root) }
    }

    private func indexedModel() async throws -> AppModel {
        let model = AppModel()
        model.engine.setRoots([root.path])
        XCTAssertTrue(model.engine.startScan())

        let deadline = Date().addingTimeInterval(60)
        while !model.engine.scanFinished, Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        _ = model.engine.finishScan()
        return model
    }

    /// Waits until the row count stops changing.
    private func settle(_ model: AppModel) async throws {
        var last = -1
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            try await Task.sleep(for: .milliseconds(60))
            if model.results.count == last, !model.results.isEmpty { return }
            last = model.results.count
        }
    }

    func test_a_longer_page_begins_with_the_shorter_one() throws {
        let engine = SearchEngine()
        engine.setRoots([root.path])
        XCTAssertTrue(engine.startScan())
        while !engine.scanFinished { usleep(20_000) }
        _ = engine.finishScan()

        let first = engine.search("report", limit: 200)
        let second = engine.search("report", limit: 400)
        let third = engine.search("report", limit: 900)

        XCTAssertEqual(first.hits.count, 200)
        XCTAssertEqual(second.hits.count, 400)
        XCTAssertEqual(third.hits.count, 900)

        XCTAssertEqual(
            second.hits.prefix(200).map(\.id),
            first.hits.map(\.id),
            "a longer request reordered the rows the user was already looking at"
        )
        XCTAssertEqual(
            third.hits.prefix(400).map(\.id),
            second.hits.map(\.id),
            "the list is not stable as it grows"
        )
    }

    func test_the_page_has_no_gaps_or_duplicates() throws {
        let engine = SearchEngine()
        engine.setRoots([root.path])
        XCTAssertTrue(engine.startScan())
        while !engine.scanFinished { usleep(20_000) }
        _ = engine.finishScan()

        let all = engine.search("report", limit: 900)

        XCTAssertEqual(all.matched, 900)
        XCTAssertEqual(
            Set(all.hits.map(\.id)).count,
            all.hits.count,
            "the same entry came back twice"
        )
    }

    /// The buffer the core writes the resolved paths into used to be a
    /// fixed 256KB. It does not fail when it fills — it stops and reports
    /// how many it managed — so asking for more rows than fitted returned
    /// fewer, silently. At two hundred rows nothing noticed; past about
    /// thirteen hundred everything after them vanished, and asking for all
    /// 47,000 matches of a query returned 1,279 of them and said nothing.
    ///
    /// These paths are long on purpose, so that the bytes run out well
    /// before the rows do.
    func test_long_paths_do_not_silently_vanish() throws {
        let deep = root.appendingPathComponent(String(repeating: "d", count: 120))
        try FileManager.default.createDirectory(at: deep, withIntermediateDirectories: true)

        for i in 0..<900 {
            FileManager.default.createFile(
                at: deep.appendingPathComponent(
                    "ledger-" + String(repeating: "x", count: 150) + "-\(i).md"
                )
            )
        }

        let engine = SearchEngine()
        engine.setRoots([root.path])
        XCTAssertTrue(engine.startScan())
        while !engine.scanFinished { usleep(20_000) }
        _ = engine.finishScan()

        // Well over the old 256KB: 900 rows of a full path and a relative
        // path, each a few hundred bytes. Measured from the fixture, not
        // from the results — if the results are truncated, counting their
        // bytes counts only the ones that survived and the check passes
        // for the very reason it should fail.
        let sample = deep.appendingPathComponent(
            "ledger-" + String(repeating: "x", count: 150) + "-0.md"
        ).path
        let expectedBytes = 900 * (sample.utf8.count + sample.utf8.count - root.path.utf8.count)
        XCTAssertGreaterThan(
            expectedBytes, 256 * 1024,
            "this fixture no longer tests what it is for"
        )

        let outcome = engine.search("ledger", limit: 900)

        XCTAssertEqual(
            outcome.hits.count,
            900,
            "\(900 - outcome.hits.count) results were dropped on the way out of the core"
        )
        XCTAssertEqual(outcome.matched, 900)
    }

    func test_reaching_the_end_fetches_more() async throws {
        let model = try await indexedModel()

        model.query = "report"
        try await settle(model)

        XCTAssertEqual(model.results.count, AppModel.firstPage, "the first page is a page")
        XCTAssertEqual(model.matchedCount, 900)
        XCTAssertEqual(model.status, "900 matches · first 200")

        model.loadMoreResults()
        try await settle(model)

        XCTAssertGreaterThan(
            model.results.count,
            AppModel.firstPage,
            "reaching the end of the list did not fetch any more of it"
        )
        let grown = model.results.count

        // And again, until the whole match set has been walked.
        for _ in 0..<6 {
            model.loadMoreResults()
            try await settle(model)
        }

        XCTAssertEqual(
            model.results.count,
            900,
            "the list stopped growing before reaching the last match "
                + "(got \(model.results.count) after \(grown))"
        )
        XCTAssertEqual(model.status, "900 matches", "the footer still claims a cap")
    }

    func test_a_new_query_starts_from_the_first_page_again() async throws {
        let model = try await indexedModel()

        model.query = "report"
        try await settle(model)
        model.loadMoreResults()
        try await settle(model)
        XCTAssertGreaterThan(model.results.count, AppModel.firstPage)

        // A different query must not inherit the grown page, or every
        // keystroke after a scroll would pay for rows nobody asked for.
        model.query = "report-1"
        try await settle(model)

        XCTAssertLessThanOrEqual(
            model.results.count,
            AppModel.firstPage,
            "a new query kept the page size the previous one had grown to"
        )
    }

    func test_the_selection_survives_a_page_arriving() async throws {
        let model = try await indexedModel()

        model.query = "report"
        try await settle(model)

        model.selection = 150
        model.loadMoreResults()
        try await settle(model)

        XCTAssertEqual(
            model.selection,
            150,
            "fetching more results moved the row the user had selected"
        )
    }
}

extension FileManager {
    fileprivate func createFile(at url: URL) {
        createFile(atPath: url.path, contents: Data())
    }
}
