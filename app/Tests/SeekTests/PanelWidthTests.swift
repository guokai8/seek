//
//  PanelWidthTests.swift
//  Seek
//
//  How wide the panel is, with and without the preview column.
//
//  The width is not decoration: the panel is re-centred on the screen
//  whenever it changes, so a width that does not match what the theme says
//  moves the whole panel sideways. These tests pin both states, and the
//  one state that looks like a third: the preview is asked for but there
//  is nothing to show in it.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class PanelWidthTests: XCTestCase {
    /// Windows the panel can lay out in.
    ///
    /// Kept for the same reason `PanelHeightTests` keeps them: a lazy
    /// container has nothing to be lazy about without a viewport. They are
    /// never ordered front, so the focus tests keep the key window.
    private var windows: [NSWindow] = []

    /// What the preference held before the test changed it.
    private var saved: Bool = false

    override func setUp() async throws {
        saved = Preferences.shared.showPreview
        Preferences.shared.showPreview = false
    }

    override func tearDown() async throws {
        Preferences.shared.showPreview = saved
        windows = []
    }

    /// The size the panel asks for, with `model` behind it.
    private func size(of model: AppModel) -> CGSize {
        let hosting = NSHostingController(rootView: ContentView().environmentObject(model))

        let window = NSWindow(
            contentRect: NSRect(
                x: 0,
                y: 0,
                width: Theme.panelWidth + Theme.previewWidth,
                height: Theme.panelMaxHeight
            ),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.contentViewController = hosting
        windows.append(window)

        hosting.view.layoutSubtreeIfNeeded()

        return hosting.view.fittingSize
    }

    /// A model holding `count` results.
    ///
    /// The paths need not exist: nothing here is measuring what QuickLook
    /// draws, only how much room the column takes.
    private func model(withResults count: Int) -> AppModel {
        let model = AppModel()

        model.setTestResults(
            (0..<count).map { i in
                SearchHit(
                    id: UInt32(i),
                    score: Int32(100 - i),
                    path: "/Users/someone/projects/seek/core/src/file\(i).rs",
                    relativePath: "core/src/file\(i).rs",
                    name: "file\(i).rs",
                    directory: "core/src",
                    isDirectory: false,
                    positions: [0, 1, 2]
                )
            }
        )

        return model
    }

    func test_a_panel_without_a_preview_is_the_width_the_theme_says() {
        XCTAssertEqual(size(of: model(withResults: 30)).width, Theme.panelWidth, accuracy: 0.5)
    }

    func test_the_preview_makes_the_panel_wider_by_its_own_width() {
        let model = model(withResults: 30)
        model.togglePreview()

        XCTAssertEqual(
            size(of: model).width,
            Theme.panelWidth + Theme.previewWidth,
            accuracy: 0.5
        )
    }

    func test_a_panel_with_nothing_to_preview_stays_narrow() {
        // The setting is remembered between launches, so the panel opens
        // with the preview asked for and an empty query. It must look
        // exactly like a panel without the preview until there is a row.
        let model = model(withResults: 0)
        model.togglePreview()

        XCTAssertTrue(model.isPreviewShown)
        XCTAssertEqual(size(of: model).width, Theme.panelWidth, accuracy: 0.5)
    }

    func test_a_previewing_panel_is_at_its_full_height() {
        // With the preview open the panel stops sizing itself to its
        // results: three rows beside a 60pt preview box is a preview
        // nobody can read. The panel takes its ceiling instead, so the
        // column has a stable, usable height.
        let model = model(withResults: 3)
        model.togglePreview()

        XCTAssertEqual(size(of: model).height, Theme.panelMaxHeight, accuracy: 0.5)
    }
}
