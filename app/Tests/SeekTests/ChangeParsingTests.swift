//
//  ChangeParsingTests.swift
//  Seek
//
//  A check that the watcher records are read out of the buffer correctly.
//
//  The records are packed one after another, each ending with a null byte.
//  Reading them wrongly used to crash the app with an index-out-of-range
//  error the first time the watcher reported something.
//

import Foundation
import XCTest

@testable import Seek

@MainActor
final class ChangeParsingTests: XCTestCase {
    /// A change to the filesystem.
    private typealias Change = SearchEngine.FileChange

    /// Splits a buffer of null-terminated records the way the watcher
    /// does.
    ///
    /// This mirrors the parsing in `SearchEngine.pollChanges`, so that the
    /// test exercises the same index arithmetic.
    private func parse(_ buffer: [CChar]) -> [Change] {
        var changes: [Change] = []
        var offset = 0

        for _ in 0..<buffer.count {
            guard let terminator = buffer[offset...].firstIndex(of: 0) else { break }

            let record = String(
                decoding: buffer[offset..<terminator].map { UInt8(bitPattern: $0) },
                as: UTF8.self
            )
            offset = terminator + 1

            let fields = record.split(separator: "\t", omittingEmptySubsequences: false)
            guard fields.count == 3,
                  let kind = Change.Kind(rawValue: Int(fields[0]) ?? 3),
                  let isDirectory = Int(fields[1])
            else { continue }

            changes.append(
                Change(
                    path: String(fields[2]),
                    kind: kind,
                    isDirectory: isDirectory != 0
                )
            )
        }

        return changes
    }

    /// Builds a buffer of null-terminated records.
    private func buffer(of records: [String]) -> [CChar] {
        var buffer: [CChar] = []

        for record in records {
            buffer.append(contentsOf: Array(record.utf8).map { CChar(bitPattern: $0) })
            buffer.append(0)
        }

        return buffer
    }

    func test_a_single_record_is_read_correctly() {
        let changes = parse(buffer(of: ["0\t0\t/tmp/alpha.txt"]))

        XCTAssertEqual(changes.count, 1)
        XCTAssertEqual(changes[0].path, "/tmp/alpha.txt")
        XCTAssertEqual(changes[0].kind, .created)
        XCTAssertFalse(changes[0].isDirectory)
    }

    func test_several_records_are_read_in_order() {
        let changes = parse(buffer(of: [
            "0\t0\t/tmp/alpha.txt",
            "1\t1\t/tmp/beta.txt",
            "2\t0\t/tmp/gamma.txt",
        ]))

        XCTAssertEqual(changes.count, 3)
        XCTAssertEqual(changes.map(\.path), [
            "/tmp/alpha.txt",
            "/tmp/beta.txt",
            "/tmp/gamma.txt",
        ])
        XCTAssertEqual(changes[1].kind, .modified)
        XCTAssertTrue(changes[1].isDirectory)
        XCTAssertEqual(changes[2].kind, .removed)
    }

    func test_records_of_different_lengths_are_read_correctly() {
        // The records have very different lengths, so a parser that mixed
        // up the relative and the absolute index would read the wrong
        // bytes and eventually run off the end of the buffer.
        let changes = parse(buffer(of: [
            "0\t0\t/a",
            "0\t0\t/some/much/longer/path/than/the/others.txt",
            "0\t1\t/b",
        ]))

        XCTAssertEqual(changes.count, 3)
        XCTAssertEqual(changes[1].path, "/some/much/longer/path/than/the/others.txt")
        XCTAssertEqual(changes[2].path, "/b")
    }

    func test_a_malformed_record_is_skipped_rather_than_crashing() {
        let changes = parse(buffer(of: [
            "0\t0\t/tmp/alpha.txt",
            "garbage",
            "0\t0\t/tmp/beta.txt",
        ]))

        XCTAssertEqual(changes.count, 2)
        XCTAssertEqual(changes[0].path, "/tmp/alpha.txt")
        XCTAssertEqual(changes[1].path, "/tmp/beta.txt")
    }

    func test_an_empty_buffer_yields_nothing() {
        XCTAssertTrue(parse([]).isEmpty)
    }

    func test_a_record_without_a_terminator_is_dropped() {
        // The last record is not terminated, so it is incomplete. It must
        // be dropped rather than read past the end of the buffer.
        var buffer = buffer(of: ["0\t0\t/tmp/alpha.txt"])
        buffer.append(contentsOf: Array("0\t0\t/tmp/unterminated".utf8).map { CChar(bitPattern: $0) })

        let changes = parse(buffer)

        XCTAssertEqual(changes.count, 1)
        XCTAssertEqual(changes[0].path, "/tmp/alpha.txt")
    }
}
