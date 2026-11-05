//
//  PanelChromeExtrasTests.swift
//  Seek
//
//  Dragging the panel, and how see-through it is.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

/// The opacity setting.
///
/// The value is `alphaValue` on the window, so it fades the result text
/// along with the background. That is what the floor is for, and the
/// floor is the part worth pinning: a stored value below it -- from an
/// older build, from `defaults write`, from a settings file carried over
/// from another Mac -- must not be able to make the panel unreadable.
@MainActor
final class PanelOpacityTests: XCTestCase {
    private var saved: Double = 1

    override func setUp() async throws {
        saved = Preferences.shared.panelOpacity
    }

    override func tearDown() async throws {
        Preferences.shared.panelOpacity = saved
    }

    func test_a_stored_value_below_the_floor_is_clamped() {
        Preferences.shared.panelOpacity = 0.0

        XCTAssertEqual(
            Preferences.shared.effectiveOpacity,
            Preferences.minimumOpacity,
            accuracy: 0.0001,
            "a stored zero would make the panel invisible"
        )
    }

    func test_a_stored_value_above_one_is_clamped() {
        Preferences.shared.panelOpacity = 4.2

        XCTAssertEqual(Preferences.shared.effectiveOpacity, 1.0, accuracy: 0.0001)
    }

    func test_a_value_in_range_is_left_alone() {
        Preferences.shared.panelOpacity = 0.7

        XCTAssertEqual(Preferences.shared.effectiveOpacity, 0.7, accuracy: 0.0001)
    }

    func test_moving_the_slider_reaches_an_open_panel() async throws {
        Preferences.shared.panelOpacity = 1.0
        let model = AppModel()
        XCTAssertEqual(model.panelOpacity, 1.0, accuracy: 0.0001)

        Preferences.shared.panelOpacity = 0.6

        let deadline = Date().addingTimeInterval(3)
        while Date() < deadline, abs(model.panelOpacity - 0.6) > 0.0001 {
            try await Task.sleep(for: .milliseconds(30))
        }

        XCTAssertEqual(
            model.panelOpacity, 0.6, accuracy: 0.0001,
            "the slider moved and the open panel never heard"
        )
    }
}

/// The drag region.
///
/// There is no way to perform a real window drag in a test, so what is
/// pinned is the part that can silently disappear: that a drag view is
/// actually in the panel's view tree, and that the results list is not
/// one. A `.background(WindowDragArea())` deleted in a refactor leaves a
/// panel that simply cannot be moved, with nothing else to show for it.
@MainActor
final class PanelDragAreaTests: XCTestCase {
    /// Every descendant of a view, including itself.
    private func descendants(of view: NSView) -> [NSView] {
        [view] + view.subviews.flatMap(descendants(of:))
    }

    private func hostedPanelContent() -> NSView {
        let model = AppModel()
        let content = ContentView().environmentObject(model)
        let hosting = NSHostingController(rootView: AnyView(content))

        // Laid out, so that SwiftUI actually makes the backing views.
        hosting.view.frame = NSRect(
            x: 0, y: 0, width: Theme.panelWidth, height: Theme.panelMaxHeight
        )
        hosting.view.layoutSubtreeIfNeeded()

        return hosting.view
    }

    func test_the_panel_has_a_drag_region() {
        let root = hostedPanelContent()

        let dragViews = descendants(of: root)
            .filter { $0 is WindowDragArea.DragView }

        XCTAssertFalse(
            dragViews.isEmpty,
            "nothing in the panel drags the window, so the panel cannot be moved"
        )
    }

    func test_the_drag_region_does_not_take_the_focus() {
        // The panel exists to be typed into the moment it opens. A drag
        // region that could become the first responder would be a region
        // that sometimes swallows the first keystroke.
        XCTAssertFalse(WindowDragArea.DragView().acceptsFirstResponder)
    }
}
