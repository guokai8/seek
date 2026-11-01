//
//  ResultMetadataTests.swift
//  Seek
//
//  The row shows a file's size and when it was last changed. Both come
//  from the index, which the scan filled from the same `stat` it had to
//  make anyway, and they travel across the C ABI with the result.
//
//  Travelling with the result is also what these tests really check. The
//  C struct in `core/include/seek.h` and the Rust struct in
//  `core/src/ffi.rs` are two declarations of one memory layout, and
//  nothing makes the compiler compare them. Get a field's order or width
//  wrong and the size does not come back missing — it comes back as a
//  plausible-looking number. So the file here is written to a size the
//  test picks, and the assertion is an equality.
//

import XCTest

@testable import Seek

@MainActor
final class ResultMetadataTests: XCTestCase {
    private var root: URL!

    /// The size the subject file is written to, in bytes.
    private let subjectSize = 4096

    override func setUp() async throws {
        root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("seek-metadata-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try Data(repeating: 0x78, count: subjectSize)
            .write(to: root.appendingPathComponent("subject.bin"))
    }

    override func tearDown() async throws {
        if let root { try? FileManager.default.removeItem(at: root) }
    }

    /// The hit for the one file in the root.
    private func subject() throws -> SearchHit {
        let engine = SearchEngine()
        engine.setRoots([root.path])
        XCTAssertTrue(engine.startScan())

        let deadline = Date().addingTimeInterval(60)
        while !engine.scanFinished, Date() < deadline {
            Thread.sleep(forTimeInterval: 0.02)
        }
        _ = engine.finishScan()

        let hits = engine.search("subject.bin").hits
        let hit = try XCTUnwrap(
            hits.first { $0.name == "subject.bin" },
            "the search did not find the file it had just indexed"
        )
        return hit
    }

    func test_a_hit_carries_the_file_size() throws {
        let hit = try subject()

        XCTAssertEqual(
            hit.byteSize, UInt64(subjectSize),
            "the hit reports the wrong size, which is what a disagreement "
                + "between the C and Rust declarations looks like"
        )
    }

    func test_a_hit_carries_the_modification_time() throws {
        let hit = try subject()

        let modified = try XCTUnwrap(hit.modified, "the hit has no modification time")
        XCTAssertEqual(
            modified.timeIntervalSinceNow, 0, accuracy: 120,
            "the file was written during this test, so its mtime is now"
        )
    }

    func test_a_directory_has_no_size_to_show() throws {
        // A directory's `st_size` is the size of its own on-disk record,
        // which is not a thing anyone wants to read in a result row.
        let engine = SearchEngine()
        engine.setRoots([root.path])
        XCTAssertTrue(engine.startScan())

        let deadline = Date().addingTimeInterval(60)
        while !engine.scanFinished, Date() < deadline {
            Thread.sleep(forTimeInterval: 0.02)
        }
        _ = engine.finishScan()

        let hits = engine.search(root.lastPathComponent).hits
        if let directory = hits.first(where: { $0.isDirectory }) {
            XCTAssertNil(
                directory.sizeText,
                "a directory row must not claim a file size"
            )
        }
    }
}
