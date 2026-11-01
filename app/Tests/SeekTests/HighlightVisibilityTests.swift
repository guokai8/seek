//
//  HighlightVisibilityTests.swift
//  Seek
//
//  The matched characters are visible on every row, selected or not.
//
//  The selected row used to drop the highlight colour and mark the match
//  with weight alone — on a name that was already drawn semibold, so bold
//  against semibold at 13.5pt is a difference nobody can see. A search for
//  `nihao` put `Hao Law NIW EB1 Documents List Please Read.pdf` at the top
//  with `Hao` matched, and the top row was the one row where the user
//  could not tell which characters had matched.
//
//  What this measures is colour, not change. The first version of this
//  test compared the row against the same row with nothing matched and
//  asked how many pixels moved — and the broken version scored *higher*
//  than the working one, because going from semibold to bold reflows
//  every character after the match. Plenty of pixels moved; none of them
//  told the user anything.
//
//  So the measurement is the share of the row's ink that is not grey.
//  The text is white or grey at every weight; a matched character is the
//  one thing on the row drawn in a colour. That cannot be faked by a
//  reflow, and it does not assume which colour the accent is — the user's
//  accent colour is whatever they chose in System Settings.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class HighlightVisibilityTests: XCTestCase {
    /// The row from the report.
    private func hit(matching positions: [Int]) -> SearchHit {
        SearchHit(
            id: 1,
            score: 127,
            path: "/Users/someone/Self/Green/24-383 Guo, Kai_NIW/Hao Law NIW EB1.pdf",
            relativePath: "Self/Green/24-383 Guo, Kai_NIW/Hao Law NIW EB1.pdf",
            name: "Hao Law NIW EB1.pdf",
            directory: "Self/Green/24-383 Guo, Kai_NIW",
            isDirectory: false,
            positions: positions
        )
    }

    /// The matched offsets of `nihao` in that path: `NI` in the directory
    /// and `Hao` in the name, which is what the matcher really returns.
    private let matched = [27, 28, 31, 32, 33]

    /// Draws one row on an opaque background.
    ///
    /// Opaque, because the measurement below tells ink from background and
    /// a transparent background has no colour to tell it from.
    private func pixels(of hit: SearchHit, selected: Bool) throws -> NSBitmapImageRep {
        let row = ZStack {
            Color(white: 0.16)

            ResultRow(hit: hit, isSelected: selected, showsModified: false, showsSize: false)
                .background(selected ? Theme.accent.opacity(0.22) : Color.clear)
        }
        .frame(width: Theme.panelWidth, height: 28)

        let renderer = ImageRenderer(content: row)
        renderer.scale = 2

        let image = try XCTUnwrap(renderer.nsImage)
        let data = try XCTUnwrap(image.tiffRepresentation)

        return try XCTUnwrap(NSBitmapImageRep(data: data))
    }

    /// The share of the row's pixels that carry a colour.
    ///
    /// A pixel counts when its channels are far enough apart to read as a
    /// colour rather than as grey. The row's own tint is excluded by the
    /// margin: at 22% the selection is a wash, while a glyph drawn in the
    /// accent is the colour at full strength.
    private func colouredShare(_ rep: NSBitmapImageRep) throws -> Double {
        let data = try XCTUnwrap(rep.bitmapData)
        let samples = rep.samplesPerPixel
        var coloured = 0

        for y in 0..<rep.pixelsHigh {
            for x in 0..<rep.pixelsWide {
                let pixel = y * rep.bytesPerRow + x * samples
                let r = Int(data[pixel])
                let g = Int(data[pixel + 1])
                let b = Int(data[pixel + 2])

                let spread = max(abs(r - g), max(abs(g - b), abs(r - b)))

                if spread > 60 {
                    coloured += 1
                }
            }
        }

        return Double(coloured) / Double(rep.pixelsWide * rep.pixelsHigh)
    }

    func test_the_highlight_is_visible_on_an_unselected_row() throws {
        let withMatch = try colouredShare(try pixels(of: hit(matching: matched), selected: false))
        let without = try colouredShare(try pixels(of: hit(matching: []), selected: false))

        XCTAssertGreaterThan(
            withMatch,
            without * 2,
            "an unselected row's matched characters are not drawn in a colour"
        )
    }

    func test_the_highlight_is_visible_on_the_selected_row() throws {
        let withMatch = try colouredShare(try pixels(of: hit(matching: matched), selected: true))
        let without = try colouredShare(try pixels(of: hit(matching: []), selected: true))

        // Measured against the row nobody ever doubted. The selected row
        // is allowed to be a little quieter — the tint eats some of the
        // contrast — but not an order quieter, and never silent.
        let litRow = try colouredShare(try pixels(of: hit(matching: matched), selected: false))
        let plainRow = try colouredShare(try pixels(of: hit(matching: []), selected: false))
        let reference = litRow - plainRow

        XCTAssertGreaterThan(
            withMatch - without,
            reference / 2,
            "the selected row draws almost nothing in a colour (\(withMatch) against "
                + "\(without) with nothing matched, where an unselected row manages "
                + "\(reference)), so on the one row the user is looking at there is "
                + "no telling which characters matched"
        )
    }
}
