//
//  PanelDrawsResultsTests.swift
//  Seek
//
//  The last link in the chain: the rows reach the screen.
//
//  The other tests prove that the engine finds the files and that the model
//  publishes them. Neither of those says the user can see anything. The
//  panel is rendered here and the pixels in the list are counted, because a
//  result that is in the model but not drawn is exactly the failure this
//  app was reported to have.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class PanelDrawsResultsTests: XCTestCase {
    /// The band of the panel that holds the results.
    ///
    /// The search bar is above it and the footer below, so counting only
    /// this band means the query text and the status line cannot be
    /// mistaken for a drawn row.
    private func listBand(of size: CGSize) -> NSRect {
        NSRect(
            x: 0,
            y: 100,
            width: size.width,
            height: size.height - 100 - 56
        )
    }

    /// The results band in the bitmap's own pixels.
    ///
    /// The band is described in points, and on a retina display a bitmap
    /// holds two pixels for each of them. Indexing a bitmap with point
    /// coordinates reads the top half of the panel and calls it the whole
    /// of it.
    private func listBandInPixels(of size: CGSize, in bitmap: NSBitmapImageRep) -> NSRect {
        let band = listBand(of: size)
        let scale = CGFloat(bitmap.pixelsHigh) / size.height

        return NSRect(
            x: band.minX * scale,
            y: band.minY * scale,
            width: band.width * scale,
            height: band.height * scale
        )
    }

    /// Renders the panel and counts the rows of text drawn in the results
    /// band.
    ///
    /// A row is a run of scan lines that carry ink, separated from the next
    /// by a run that does not.
    ///
    /// This counts rows rather than ink, because how much ink a row carries
    /// is a matter of what happens to be on the disk: a search whose
    /// matches all sit in short directories draws a third less of it than
    /// one whose matches are buried deep, and neither says anything about
    /// whether the list was drawn.
    private func drawnRows(of model: AppModel) throws -> Int {
        let size = CGSize(width: Theme.panelWidth, height: Theme.panelMaxHeight)
        let hosting = NSHostingController(rootView: ContentView().environmentObject(model))

        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.contentViewController = hosting

        hosting.view.frame = NSRect(origin: .zero, size: size)
        hosting.view.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date().addingTimeInterval(0.4))
        hosting.view.layoutSubtreeIfNeeded()

        guard let bitmap = hosting.view.bitmapImageRepForCachingDisplay(
            in: hosting.view.bounds
        ) else {
            XCTFail("the panel did not render")
            return 0
        }
        hosting.view.cacheDisplay(in: hosting.view.bounds, to: bitmap)

        defer { window.contentViewController = nil }

        let band = listBandInPixels(of: size, in: bitmap)

        guard let background = bitmap.colorAt(x: 2, y: Int(band.minY) + 2) else {
            XCTFail("the panel rendered without pixels")
            return 0
        }

        // The left edge is skipped: the selected row is drawn with a filled
        // background that runs the whole width, which would make the rows
        // above and below it one continuous band.
        let left = Int(band.minX) + 40
        let maxX = min(Int(band.maxX), bitmap.pixelsWide) - 1
        let maxY = min(Int(band.maxY), bitmap.pixelsHigh) - 1

        var rows = 0
        var inRow = false

        for y in stride(from: Int(band.minY), through: maxY, by: 1) {
            var inked = 0

            for x in stride(from: left, through: maxX, by: 2) {
                guard let colour = bitmap.colorAt(x: x, y: y) else { continue }

                let difference =
                    abs(colour.redComponent - background.redComponent)
                    + abs(colour.greenComponent - background.greenComponent)
                    + abs(colour.blueComponent - background.blueComponent)

                if difference > 0.25 {
                    inked += 1
                }
            }

            // A handful of pixels is a divider or an anti-aliased edge; a
            // line of text covers far more than that.
            let textLine = inked > 12

            if textLine && !inRow {
                rows += 1
            }

            inRow = textLine
        }

        return rows
    }

    /// Waits until the model holds the results of `query`.
    private func waitForResults(of query: String, in model: AppModel) async throws {
        let needle = query.lowercased()
        let deadline = Date().addingTimeInterval(15)

        while Date() < deadline {
            let ready = !model.results.isEmpty
                && model.results.allSatisfy { hit in
                    var remaining = Array(needle)
                    for character in hit.relativePath.lowercased()
                    where character == remaining.first {
                        remaining.removeFirst()
                        if remaining.isEmpty { break }
                    }
                    return remaining.isEmpty
                }

            if ready { return }
            try await Task.sleep(for: .milliseconds(50))
        }
    }

    func test_the_rows_of_a_real_search_are_drawn() async throws {
        let model = AppModel()

        let deadline = Date().addingTimeInterval(180)
        while model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertGreaterThan(model.indexedCount, 0, "the index is empty")

        // A query that matches nothing gives the baseline: the list shows
        // its empty state and nothing else.
        model.query = "zzqqxxjjvv"
        let emptyDeadline = Date().addingTimeInterval(15)
        while !model.results.isEmpty && Date() < emptyDeadline {
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertTrue(model.results.isEmpty, "a nonsense query matched something")

        let emptyRows = try drawnRows(of: model)

        // And now a query that matches.
        model.query = "readme"
        try await waitForResults(of: "readme", in: model)

        XCTAssertFalse(model.results.isEmpty, "the search found nothing to draw")

        let rows = try drawnRows(of: model)

        print("DRAWN emptyRows=\(emptyRows) rows=\(rows) hits=\(model.results.count)")

        // The empty state is a heading and a line of advice. A drawn list
        // is a screenful of rows, several times as many.
        XCTAssertGreaterThan(
            rows, emptyRows * 3,
            "the list drew \(rows) rows of text against \(emptyRows) for an empty panel"
        )

        XCTAssertGreaterThan(
            rows, 8,
            "the results are in the model but the list was not drawn: "
                + "\(rows) rows of text for \(model.results.count) results"
        )
    }
}
