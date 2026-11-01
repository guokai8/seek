//
//  ResultRowMetadataTests.swift
//  Seek
//
//  The row can show when a file last changed and how big it is, and the
//  user decides whether it does.
//
//  The drawing is checked by rendering the row and looking at the ink in
//  its right-hand column, rather than by reading the text back: what can
//  go wrong here is a column that is laid out but never drawn, or one that
//  is drawn when the preference says not to, and ink is what tells those
//  apart. The wording is checked separately, on the formatter, where it is
//  a pure function of a date.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class ResultRowMetadataTests: XCTestCase {
    /// A hit with a size and an mtime to show.
    private func hit() -> SearchHit {
        SearchHit(
            id: 1,
            score: 100,
            path: "/Users/someone/Documents/notes/quarterly.md",
            relativePath: "Documents/notes/quarterly.md",
            name: "quarterly.md",
            directory: "Documents/notes",
            isDirectory: false,
            byteSize: 48_210,
            modified: Date(timeIntervalSinceNow: -3600),
            positions: [0, 1, 2]
        )
    }

    /// The number of drawn pixels in the right-hand `width` points of a
    /// rendered row.
    ///
    /// The row is rendered on an opaque white canvas, so anything the row
    /// draws is a pixel that is no longer white. Counting rather than
    /// comparing, because the question is whether the column has content
    /// at all.
    private func inkOnTheRight(of row: some View, width: CGFloat) throws -> Int {
        let renderer = ImageRenderer(
            content: row.frame(width: Theme.panelWidth).background(.white)
        )
        renderer.scale = 1

        let image = try XCTUnwrap(renderer.nsImage, "the row did not render")
        let tiff = try XCTUnwrap(image.tiffRepresentation)
        let bitmap = try XCTUnwrap(NSBitmapImageRep(data: tiff))

        let from = Int(Theme.panelWidth - width)
        var ink = 0

        for x in from..<bitmap.pixelsWide {
            for y in 0..<bitmap.pixelsHigh {
                guard let colour = bitmap.colorAt(x: x, y: y) else { continue }
                if colour.brightnessComponent < 0.9 { ink += 1 }
            }
        }

        return ink
    }

    /// The width of the region the metadata is allowed to use.
    private var column: CGFloat { Theme.metadataWidth + 18 }

    func test_the_right_hand_column_is_empty_when_both_are_switched_off() throws {
        let ink = try inkOnTheRight(
            of: ResultRow(hit: hit(), isSelected: false, showsModified: false, showsSize: false),
            width: column
        )

        XCTAssertEqual(ink, 0, "nothing was asked for, so nothing may be drawn")
    }

    func test_the_time_is_drawn_when_it_is_switched_on() throws {
        let ink = try inkOnTheRight(
            of: ResultRow(hit: hit(), isSelected: false, showsModified: true, showsSize: false),
            width: column
        )

        XCTAssertGreaterThan(ink, 20, "the time was asked for but nothing was drawn")
    }

    func test_the_size_adds_to_what_is_drawn() throws {
        let timeOnly = try inkOnTheRight(
            of: ResultRow(hit: hit(), isSelected: false, showsModified: true, showsSize: false),
            width: column
        )
        let both = try inkOnTheRight(
            of: ResultRow(hit: hit(), isSelected: false, showsModified: true, showsSize: true),
            width: column
        )

        XCTAssertGreaterThan(
            both, timeOnly,
            "switching the size on drew no more than the time alone"
        )
    }

    // MARK: - Fitting the column

    /// The width the row draws `text` at.
    ///
    /// Measured with monospaced digits, because that is what the row uses
    /// and a monospaced digit is wider than a proportional one.
    private func drawnWidth(of text: String) -> CGFloat {
        NSAttributedString(
            string: text,
            attributes: [
                .font: NSFont.monospacedDigitSystemFont(ofSize: 11.5, weight: .regular)
            ]
        ).size().width
    }

    /// Every form `ResultMetadataFormat.time` can produce must fit.
    ///
    /// Found by looking at a rendered row: a four-digit year came out as
    /// `Sep 27, 2...`, a date with the one part that made it worth showing
    /// cut off. A fixed column only works if the widest thing that can go
    /// in it fits, and nothing but a measurement says whether it does.
    func test_every_form_of_the_time_fits_its_column() throws {
        let now = try XCTUnwrap(
            Calendar.current.date(from: DateComponents(year: 2026, month: 12, day: 30))
        )

        let dates = [
            now.addingTimeInterval(-60),
            try XCTUnwrap(
                Calendar.current.date(from: DateComponents(year: 2026, month: 9, day: 27))
            ),
            try XCTUnwrap(
                Calendar.current.date(from: DateComponents(year: 2019, month: 11, day: 28))
            ),
        ]

        for date in dates {
            let text = ResultMetadataFormat.time(date, now: now)

            XCTAssertLessThanOrEqual(
                drawnWidth(of: text), Theme.metadataTimeWidth,
                "\(text) is wider than the column, so the row truncates it"
            )
        }
    }

    /// The widest size the formatter produces must fit too.
    func test_the_widest_size_fits_its_column() {
        // Just under a kilobyte spells the unit out, which is the widest
        // the formatter gets; the rest are three digits and two letters.
        for bytes: UInt64 in [0, 999, 4_912_345_678, 999_999_999_999] {
            let text = ResultMetadataFormat.size(ofBytes: bytes)

            XCTAssertLessThanOrEqual(
                drawnWidth(of: text), Theme.metadataSizeWidth,
                "\(text) is wider than the column, so the row truncates it"
            )
        }
    }

    // MARK: - The wording

    func test_a_file_changed_today_shows_the_clock_time() {
        let now = Date()
        let text = ResultMetadataFormat.time(now.addingTimeInterval(-600), now: now)

        XCTAssertFalse(
            text.contains(String(Calendar.current.component(.year, from: now))),
            "today's file spent the column on a year: \(text)"
        )
        XCTAssertTrue(text.contains(":"), "expected a clock time, got \(text)")
    }

    func test_a_file_from_an_earlier_year_shows_the_year() throws {
        let now = try XCTUnwrap(
            Calendar.current.date(from: DateComponents(year: 2026, month: 6, day: 1))
        )
        let then = try XCTUnwrap(
            Calendar.current.date(from: DateComponents(year: 2023, month: 6, day: 1))
        )

        // Two digits, not four: a four-digit year does not fit the column,
        // which `test_every_form_of_the_time_fits_its_column` measures.
        let text = ResultMetadataFormat.time(then, now: now)
        XCTAssertTrue(text.contains("23"), "expected the year, got \(text)")
    }

    func test_a_file_from_this_year_leaves_the_year_off() throws {
        let now = try XCTUnwrap(
            Calendar.current.date(from: DateComponents(year: 2026, month: 6, day: 1))
        )
        let then = try XCTUnwrap(
            Calendar.current.date(from: DateComponents(year: 2026, month: 2, day: 3))
        )

        let text = ResultMetadataFormat.time(then, now: now)
        XCTAssertFalse(text.contains("2026"), "the year is the same, so it is noise: \(text)")
        XCTAssertFalse(text.isEmpty)
    }
}

/// The settings window opens over the panel, so a checkbox has to reach a
/// panel that is already on screen.
///
/// `@AppStorage` is a property wrapper rather than a published property,
/// so there is nothing on `Preferences` to subscribe to — the model
/// listens for `UserDefaults.didChangeNotification` instead. That
/// indirection is what this pins: the wiring between the checkbox and the
/// row, not the checkbox.
@MainActor
final class MetadataPreferenceTests: XCTestCase {
    private var savedModified = true
    private var savedSize = true

    override func setUp() async throws {
        savedModified = Preferences.shared.showModified
        savedSize = Preferences.shared.showSize
    }

    override func tearDown() async throws {
        Preferences.shared.showModified = savedModified
        Preferences.shared.showSize = savedSize
    }

    /// Waits for the model to catch up with the defaults.
    private func settle(
        _ reached: @escaping () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(3)
        while Date() < deadline {
            if reached() { return }
            try await Task.sleep(for: .milliseconds(30))
        }
    }

    func test_switching_the_time_off_reaches_an_open_panel() async throws {
        Preferences.shared.showModified = true
        let model = AppModel()
        XCTAssertTrue(model.showsModified, "the model did not read the stored setting")

        Preferences.shared.showModified = false
        try await settle { !model.showsModified }

        XCTAssertFalse(
            model.showsModified,
            "the checkbox was switched off and the open panel never heard"
        )
    }

    func test_switching_the_size_on_reaches_an_open_panel() async throws {
        Preferences.shared.showSize = false
        let model = AppModel()
        XCTAssertFalse(model.showsSize, "the model did not read the stored setting")

        Preferences.shared.showSize = true
        try await settle { model.showsSize }

        XCTAssertTrue(
            model.showsSize,
            "the checkbox was switched on and the open panel never heard"
        )
    }
}
