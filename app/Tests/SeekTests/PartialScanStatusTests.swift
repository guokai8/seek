import XCTest

@testable import Seek

/// A scan can be cut short.
///
/// `opendir()` on an unresponsive directory blocks in the kernel and cannot
/// be cancelled — measured at fifty-four minutes on a dead NFS mount. For
/// that whole time the index was never published and the footer said
/// "Indexing…". The core now gives the walk a deadline and publishes what
/// it has, which leaves a partial index. The footer has to say so:
/// reporting a partial index as the whole disk is the same kind of lie in
/// a quieter voice.
@MainActor
final class PartialScanStatusTests: XCTestCase {
    func test_a_complete_scan_reports_only_the_count() {
        let model = AppModel()

        model.finishIndexing(count: 1234, complete: true)

        XCTAssertFalse(model.isIndexing)
        XCTAssertTrue(model.scanComplete)
        XCTAssertEqual(model.status, "1,234 files indexed")
    }

    func test_a_scan_cut_short_says_so() {
        let model = AppModel()

        model.finishIndexing(count: 1234, complete: false)

        XCTAssertFalse(model.isIndexing, "the spinner stops either way")
        XCTAssertFalse(model.scanComplete)
        XCTAssertTrue(
            model.status.contains("cut short"),
            "the footer should not present a partial index as the whole disk: \(model.status)"
        )
    }

    func test_the_footer_keeps_saying_so_when_a_query_matches_nothing() {
        let model = AppModel()

        model.finishIndexing(count: 1234, complete: false)
        // The empty-result footer is built separately, and used to hard-code
        // the plain count.
        model.setTestResults([])

        XCTAssertTrue(
            model.status.contains("cut short"),
            "the warning should survive a query: \(model.status)"
        )
    }
}
