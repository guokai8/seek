//
//  SearchHintTests.swift
//  Seek
//
//  The panel opens with the field already focused, which is the whole
//  point of a launcher: the hotkey and then straight into typing. AppKit
//  stops drawing a text field's placeholder the moment the field is being
//  edited, so the panel's own placeholder — "Search files and folders" —
//  was never once on the screen. The field opened as an empty grey box
//  with a caret in it.
//
//  This is checked in ink rather than in properties, because the property
//  was right all along: the field had its placeholder, and the placeholder
//  was not drawn.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class SearchHintTests: XCTestCase {
    /// The strip of the search field where the hint is written.
    ///
    /// It starts well to the right of the caret, so that the caret — which
    /// is drawn whether or not there is a hint — cannot be mistaken for
    /// one, and it stops short of the mode pill on the other side.
    private func hintBand(of size: CGSize) -> NSRect {
        NSRect(x: 100, y: 20, width: 180, height: 24)
    }

    /// Finds the search field in the panel's view tree.
    private func findTextField(in view: NSView) -> NSTextField? {
        if let field = view as? NSTextField { return field }

        for subview in view.subviews {
            if let found = findTextField(in: subview) { return found }
        }

        return nil
    }

    /// Renders the panel and counts the ink in the hint's strip.
    private func hintInk(of model: AppModel) throws -> Int {
        let size = CGSize(width: Theme.panelWidth, height: Theme.panelMaxHeight)
        let hosting = NSHostingController(rootView: ContentView().environmentObject(model))

        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.contentViewController = hosting
        window.makeKeyAndOrderFront(nil)

        hosting.view.frame = NSRect(origin: .zero, size: size)
        hosting.view.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date().addingTimeInterval(0.5))
        hosting.view.layoutSubtreeIfNeeded()

        // The field is focused, because that is the state the panel is
        // always in: it opens with the caret already in the field. A field
        // that is merely on screen still draws AppKit's placeholder, so a
        // render taken without the focus would be a render of a panel that
        // the user never sees.
        let field = try XCTUnwrap(
            findTextField(in: hosting.view),
            "the panel has no search field"
        )
        window.makeFirstResponder(field)
        RunLoop.current.run(until: Date().addingTimeInterval(0.2))

        XCTAssertNotNil(field.currentEditor(), "the search field is not being edited")

        defer {
            window.orderOut(nil)
            window.contentViewController = nil
        }

        let bitmap = try XCTUnwrap(
            hosting.view.bitmapImageRepForCachingDisplay(in: hosting.view.bounds),
            "the panel did not render"
        )
        hosting.view.cacheDisplay(in: hosting.view.bounds, to: bitmap)

        // From the width, not the height. The panel is as tall as its
        // content, so the view the window ends up with is nothing like the
        // 460pt it was offered — and a scale taken from the height would
        // be the ratio of two unrelated numbers.
        let scale = CGFloat(bitmap.pixelsWide) / Theme.panelWidth
        let band = hintBand(of: size)

        // The well's own fill, read from the quiet stretch between the end
        // of the hint and the mode pill. Reading it at the band's own edge
        // would land on the well's border and make every other pixel in
        // the strip look like ink.
        let background = try XCTUnwrap(
            bitmap.colorAt(x: Int(560 * scale), y: Int(32 * scale)),
            "the panel rendered without pixels"
        )

        var ink = 0

        for y in stride(from: Int(band.minY * scale), to: Int(band.maxY * scale), by: 1) {
            for x in stride(from: Int(band.minX * scale), to: Int(band.maxX * scale), by: 1) {
                guard let colour = bitmap.colorAt(x: x, y: y) else { continue }

                let difference =
                    abs(colour.redComponent - background.redComponent)
                    + abs(colour.greenComponent - background.greenComponent)
                    + abs(colour.blueComponent - background.blueComponent)

                if difference > 0.15 {
                    ink += 1
                }
            }
        }

        return ink
    }

    func test_the_empty_field_says_what_it_is_for() async throws {
        let model = AppModel()
        model.setTestResults([])

        let ink = try hintInk(of: model)

        // The hint covers about 3,400 pixels of that strip; an empty field
        // covers none of it. The threshold sits between the two with room
        // to spare on either side.
        XCTAssertGreaterThan(
            ink,
            1000,
            "the empty search field says nothing about what to type"
        )
    }

    func test_the_hint_goes_away_once_there_is_a_query() async throws {
        let model = AppModel()
        model.setTestResults([])
        model.query = "zzz"

        let ink = try hintInk(of: model)

        // The strip begins past the end of a three-character query, so a
        // field with a query in it leaves it completely empty. What must
        // not happen is the hint being drawn under or beside the query.
        XCTAssertLessThan(
            ink,
            100,
            "the hint is still drawn over a field that has a query in it"
        )
    }
}
