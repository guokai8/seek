//
//  SearchSchedulingTests.swift
//  Seek
//
//  A check that a burst of keystrokes still produces results.
//
//  The search is scheduled with a delay so that fast typing does not run
//  one search per keystroke. The delay means the task is cancelled and
//  rescheduled on every keystroke, and the last one has to survive: if it
//  does not, the panel shows nothing at all.
//

import Foundation
import XCTest

@testable import Seek

@MainActor
final class SearchSchedulingTests: XCTestCase {
    /// Waits for the results of a query to arrive.
    ///
    /// The wait is not a fixed number of milliseconds. The suite runs
    /// several index scans at once, and the search queues behind them, so a
    /// fixed delay is long enough when the machine is quiet and too short
    /// when it is busy. The wait is for the results themselves.
    private func waitForResults(
        in model: AppModel,
        timeout: TimeInterval = 30
    ) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)

        while Date() < deadline {
            try? await Task.sleep(for: .milliseconds(20))

            if !model.results.isEmpty { return true }
        }

        return false
    }

    func test_a_burst_of_keystrokes_still_produces_results() async throws {
        let model = AppModel()

        let deadline = Date().addingTimeInterval(180)
        while model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }
        print("BURST indexed=\(model.indexedCount)")
        XCTAssertGreaterThan(model.indexedCount, 0)

        // The characters are typed one after another, the way the
        // keyboard does it.
        for character in "readme" {
            model.query.append(character)
        }

        print("BURST query=\(model.query)")

        let arrived = await waitForResults(in: model)

        print("BURST results=\(model.results.count) status=\(model.status)")

        XCTAssertTrue(
            arrived,
            "a burst of keystrokes left the panel with no results"
        )
    }

    func test_a_single_keystroke_produces_results() async throws {
        let model = AppModel()

        let deadline = Date().addingTimeInterval(180)
        while model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }

        model.query = "r"

        let arrived = await waitForResults(in: model)

        print("BURST single results=\(model.results.count) status=\(model.status)")

        XCTAssertTrue(
            arrived,
            "a single keystroke left the panel with no results"
        )
    }
}
