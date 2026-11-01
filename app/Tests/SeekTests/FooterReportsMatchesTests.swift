//
//  FooterReportsMatchesTests.swift
//  Seek
//
//  The panel shows at most `limit` rows. Over a real home directory a
//  broad query matches thousands of paths, so the list saturates and then
//  holds still: the best 200 matches for `readme` and for `readm` are the
//  same 200 paths in the same order, and fzf's own ranking agrees. Taking
//  a character off the end therefore changed nothing on screen — not one
//  row, and not the footer, which was built from the row count and so read
//  "200 of 258,328 files" for both. A correct search looked like a frozen
//  panel.
//
//  The footer now reports the match count instead, which is what fzf's
//  counter shows and the only number that still moves. These tests drive
//  the engine over a fixture directory, so they pin the behaviour without
//  waiting for a scan of the user's home directory.
//

import XCTest

@testable import Seek

@MainActor
final class FooterReportsMatchesTests: XCTestCase {
    private var root: URL!

    /// A directory of files that all match `readm`, most of which also
    /// match `readme`, and more of them than the engine will show.
    override func setUp() async throws {
        root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("seek-footer-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)

        for i in 0..<300 {
            FileManager.default.createFile(at: root.appendingPathComponent("readme-\(i).md"))
        }
        // These match `readm` but not `readme`, so the two queries have
        // genuinely different match counts.
        for i in 0..<50 {
            FileManager.default.createFile(at: root.appendingPathComponent("readmx-\(i).md"))
        }

        // A name no other fixture file can fuzzy-match: the matcher works
        // on subsequences, so `readmx-7.md` also matches `readmx-17.md`,
        // and asking for a single match needs a name with a letter none of
        // the others carry.
        FileManager.default.createFile(at: root.appendingPathComponent("zqjvw.md"))
    }

    override func tearDown() async throws {
        if let root { try? FileManager.default.removeItem(at: root) }
    }

    /// An engine indexing the fixture directory.
    private func engine() throws -> SearchEngine {
        let engine = SearchEngine()
        engine.setRoots([root.path])
        XCTAssertTrue(engine.startScan(), "the scan did not start")

        let deadline = Date().addingTimeInterval(30)
        while !engine.scanFinished, Date() < deadline {
            usleep(20_000)
        }
        let count = engine.finishScan()
        XCTAssertGreaterThan(count, 300, "the fixture was not indexed")
        return engine
    }

    func test_the_count_is_of_matches_not_of_rows() throws {
        let engine = try engine()

        let outcome = engine.search("readme")

        XCTAssertEqual(outcome.hits.count, engine.limit, "the rows should be capped")
        XCTAssertEqual(outcome.matched, 300, "the count should ignore the cap")
        XCTAssertTrue(outcome.isTruncated)
    }

    func test_the_row_count_cannot_tell_the_two_queries_apart() throws {
        let engine = try engine()

        let long = engine.search("readme")
        let short = engine.search("readm")

        // Both saturate the limit, so the row count — what the footer used
        // to be built from — is the same number for a query matching 300
        // paths and one matching 350.
        XCTAssertEqual(long.hits.count, short.hits.count)
        XCTAssertEqual(long.matched, 300)
        XCTAssertEqual(short.matched, 350)
    }

    func test_the_footer_differs_between_the_two_queries() async throws {
        let model = AppModel()
        model.engine.setRoots([root.path])
        XCTAssertTrue(model.engine.startScan())

        let deadline = Date().addingTimeInterval(30)
        while !model.engine.scanFinished, Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        _ = model.engine.finishScan()

        let long = try await footer(for: "readme", in: model)
        let short = try await footer(for: "readm", in: model)

        XCTAssertEqual(long, "300 matches · first 200")
        XCTAssertEqual(short, "350 matches · first 200")
        XCTAssertNotEqual(
            long,
            short,
            "the footer is the only thing that can tell the user their edit ran"
        )
    }

    func test_an_untruncated_search_does_not_mention_a_cap() async throws {
        let model = AppModel()
        model.engine.setRoots([root.path])
        XCTAssertTrue(model.engine.startScan())

        let deadline = Date().addingTimeInterval(30)
        while !model.engine.scanFinished, Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        _ = model.engine.finishScan()

        let status = try await footer(for: "zqjvw", in: model)

        XCTAssertEqual(status, "1 match", "one match should read in the singular")
    }

    /// Sets the query and waits for the footer to settle.
    private func footer(for query: String, in model: AppModel) async throws -> String {
        model.query = query

        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
            if !model.results.isEmpty, model.matchedCount > 0, !model.isIndexing {
                // One more turn, so a refresh already in flight cannot
                // land between the read and the assertion.
                try await Task.sleep(for: .milliseconds(120))
                return model.status
            }
        }

        return model.status
    }
}

extension FileManager {
    /// Creates an empty file, for building fixture trees.
    fileprivate func createFile(at url: URL) {
        createFile(atPath: url.path, contents: Data())
    }
}
