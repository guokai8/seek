//
//  PanelPlacementTests.swift
//  Seek
//
//  Where the panel sits, and where it goes when it changes size.
//
//  This arithmetic used to live inside the app delegate, where the only
//  way to check it was to open a real window on a real display. It is a
//  pure function now, because it has two invariants that matter and are
//  easy to break: the search field must not move under the user's eyes
//  while they type, and the panel must not walk off the side of the screen
//  when the preview column opens.
//

import AppKit
import XCTest

@testable import Seek

final class PanelPlacementTests: XCTestCase {
    /// A plausible screen: 1512 by 900, with the menu bar taken off.
    private let screen = NSRect(x: 0, y: 0, width: 1512, height: 862)

    /// A screen too narrow for the panel with its preview open.
    private let narrow = NSRect(x: 0, y: 0, width: 1024, height: 700)

    /// The panel with its preview column closed.
    private var closed: CGSize { CGSize(width: Theme.panelWidth, height: 300) }

    /// The panel with its preview column open.
    private var open: CGSize {
        CGSize(width: Theme.panelWidth + Theme.previewWidth, height: Theme.panelMaxHeight)
    }

    // MARK: - Summoning

    func test_a_summoned_panel_is_centred_on_the_screen() {
        let frame = PanelPlacement.summoned(size: closed, on: screen)

        XCTAssertEqual(frame.midX, screen.midX, accuracy: 0.5)
        XCTAssertEqual(frame.size, closed)
    }

    func test_a_summoned_panel_hangs_from_above_the_middle() {
        // A third of the way up rather than in the middle, so that the
        // window underneath stays partly visible — and placed by its *top*
        // edge, so that a panel holding two hundred results has its search
        // field in the same place as an empty one.
        let frame = PanelPlacement.summoned(size: closed, on: screen)

        XCTAssertEqual(
            frame.maxY,
            screen.midY + Theme.panelMaxHeight * 2 / 3,
            accuracy: 0.5
        )
    }

    // MARK: - Resizing

    func test_a_taller_panel_keeps_its_top_edge() {
        // A window's origin is its *bottom* left, so a panel that grew
        // while keeping its origin would drag the search field up the
        // screen as the results arrived.
        let before = PanelPlacement.summoned(size: closed, on: screen)

        let after = PanelPlacement.resized(
            before,
            to: CGSize(width: closed.width, height: closed.height + 160),
            within: screen
        )

        XCTAssertEqual(after.maxY, before.maxY, accuracy: 0.5)
        XCTAssertEqual(after.height, closed.height + 160, accuracy: 0.5)
    }

    func test_a_wider_panel_keeps_its_horizontal_centre() {
        // What opening the preview column does. Growing from one edge
        // would either push the panel off the right of the screen or leave
        // it visibly off-centre; growing from the middle costs the results
        // column a one-off sideways shift instead, at a moment the user
        // asked for.
        let before = PanelPlacement.summoned(size: closed, on: screen)

        let after = PanelPlacement.resized(before, to: open, within: screen)

        XCTAssertEqual(after.midX, before.midX, accuracy: 0.5)
        XCTAssertEqual(after.size, open)
    }

    func test_a_panel_pushed_past_the_edge_is_pulled_back() {
        // The panel is centred when it is summoned, but it need not stay
        // there: it is placed on the screen the mouse is on, and that
        // screen can be narrower than the one before it.
        let before = NSRect(
            x: screen.maxX - Theme.panelWidth - 20,
            y: 300,
            width: Theme.panelWidth,
            height: closed.height
        )

        let after = PanelPlacement.resized(before, to: open, within: screen)

        XCTAssertEqual(after.maxX, screen.maxX, accuracy: 0.5)
        XCTAssertGreaterThanOrEqual(after.minX, screen.minX - 0.5)
    }

    func test_a_panel_too_wide_for_the_screen_is_centred_on_it() {
        // Nothing fits, so the least bad answer is to lose the same amount
        // off each side rather than all of it off one.
        let before = PanelPlacement.summoned(size: closed, on: narrow)

        let after = PanelPlacement.resized(before, to: open, within: narrow)

        XCTAssertGreaterThan(after.width, narrow.width, "the fixture no longer overflows")
        XCTAssertEqual(after.midX, narrow.midX, accuracy: 0.5)
    }
}
