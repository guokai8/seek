import Foundation
import XCTest

@testable import Seek

/// A check that the watcher does not cancel the search the user asked for.
///
/// The watcher reports the changes to the indexed folders, and each report
/// reschedules the search so that the new files are matched too. That
/// rescheduling cancels the search that the user asked for, and if the
/// watcher reports often enough the user's search never runs at all.
@MainActor
final class WatcherSearchCancellationTests: XCTestCase {
    func test_the_watcher_does_not_cancel_the_users_search() async throws {
        let model = AppModel()

        let deadline = Date().addingTimeInterval(120)
        while model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertGreaterThan(model.indexedCount, 0)

        model.query = "readme"

        // The watcher polls every 400 ms and reschedules the search when it
        // finds something. The search has to survive that.
        try await Task.sleep(for: .milliseconds(2000))

        print("CANCEL results=\(model.results.count) status=\(model.status)")

        XCTAssertFalse(
            model.results.isEmpty,
            "the watcher cancelled the search the user asked for"
        )
    }
}
