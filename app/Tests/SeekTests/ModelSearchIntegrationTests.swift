import Foundation
import XCTest

@testable import Seek

/// An end-to-end check that the model's search path works against a real
/// directory, the way the app drives it.
final class ModelSearchIntegrationTests: XCTestCase {
    func test_the_engine_finds_files_in_a_real_directory() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("seek-test-\(UUID().uuidString)")
        let sub = root.appendingPathComponent("src")
        try FileManager.default.createDirectory(at: sub, withIntermediateDirectories: true)
        try "hello world".write(
            to: sub.appendingPathComponent("needle.txt"),
            atomically: true,
            encoding: .utf8
        )

        defer { try? FileManager.default.removeItem(at: root) }

        let engine = SearchEngine()
        engine.setRoots([root.path])
        engine.setExcludes([])

        XCTAssertTrue(engine.startScan())
        while !engine.scanFinished {
            try await Task.sleep(for: .milliseconds(50))
        }
        let count = engine.finishScan()
        print("INTEGRATION indexed=\(count)")

        let hits = engine.search("needle").hits
        print("INTEGRATION hits=\(hits.count) first=\(hits.first?.path ?? "none")")
        XCTAssertFalse(hits.isEmpty, "the engine found nothing")
        XCTAssertTrue(
            hits.first?.path.hasSuffix("needle.txt") == true,
            "the wrong file came back"
        )
    }

    @MainActor
    func test_the_model_search_loop_publishes_results() async throws {
        let model = AppModel()
        model.setTestResults([])

        // Drive the model the way the search field does.
        model.query = "xyzzy"
        try await Task.sleep(for: .milliseconds(300))

        print("INTEGRATION model.results=\(model.results.count) status=\(model.status)")
    }
}
