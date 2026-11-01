//
//  PanelPlacement.swift
//  Seek
//
//  Where the panel sits on the screen.
//
//  This is arithmetic with two invariants behind it, so it is kept apart
//  from the window that it moves: the search field must not move under the
//  user's eyes while they type, and the panel must not walk off the side
//  of the screen when the preview column opens. Both are pinned by
//  `PanelPlacementTests`, which needs no window and no display.
//

import Foundation

/// Where the panel goes.
enum PanelPlacement {
    /// The frame a panel of `size` takes when it is summoned on a screen.
    ///
    /// `screen` is the screen's *visible* frame — the part that is not the
    /// menu bar or the Dock.
    ///
    /// The panel is a third of the way up rather than in the middle, so
    /// that the window underneath it stays partly visible. It is the *top*
    /// edge that is placed, not the middle: the panel is as tall as its
    /// content, so a panel positioned by its centre would sit somewhere
    /// different for an empty query than for a query with two hundred
    /// results. The top is placed where a full-height panel's top would
    /// have been, and the panel grows and shrinks downwards from there.
    static func summoned(size: CGSize, on screen: CGRect) -> CGRect {
        let top = screen.midY + Theme.panelMaxHeight * 2 / 3

        return CGRect(
            x: screen.midX - size.width / 2,
            y: top - size.height,
            width: size.width,
            height: size.height
        )
    }

    /// The frame a panel takes when its content changes size.
    ///
    /// The top edge is kept, for the reason above, and so is the
    /// horizontal centre: the preview column makes the panel half again as
    /// wide, and growing from one edge would either push it off the right
    /// of the screen or leave it visibly off-centre. Growing from the
    /// middle costs the results column a one-off sideways shift instead,
    /// at a moment the user asked for.
    ///
    /// The result is then pulled back onto the screen horizontally. The
    /// panel is placed on the screen the mouse is on, and that screen can
    /// be narrower than the one before it; a panel wider than the screen
    /// is centred on it, so that the same amount is lost off each side
    /// rather than all of it off one.
    static func resized(_ current: CGRect, to size: CGSize, within screen: CGRect) -> CGRect {
        let centre = current.midX
        let top = current.maxY

        var x = centre - size.width / 2

        if size.width >= screen.width {
            x = screen.midX - size.width / 2
        } else {
            x = min(max(x, screen.minX), screen.maxX - size.width)
        }

        return CGRect(x: x, y: top - size.height, width: size.width, height: size.height)
    }
}
