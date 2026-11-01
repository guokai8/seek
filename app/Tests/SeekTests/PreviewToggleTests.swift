//
//  PreviewToggleTests.swift
//  Seek
//
//  The preview column: when it is there, and what remembers it.
//
//  The column is off by default and toggled with the tab key. A launcher
//  is opened hundreds of times a day to type three letters and press
//  return, and a panel that is half again as wide for all of them would be
//  paying for a preview nobody asked to see.
//

import AppKit
import XCTest

@testable import Seek

@MainActor
final class PreviewToggleTests: XCTestCase {
    /// What the preference held before the test changed it.
    private var saved: Bool = false

    override func setUp() async throws {
        saved = Preferences.shared.showPreview
        Preferences.shared.showPreview = false
    }

    override func tearDown() async throws {
        Preferences.shared.showPreview = saved
    }

    /// The tab key.
    private var tab: NSEvent {
        NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: 0,
            context: nil,
            characters: "\t",
            charactersIgnoringModifiers: "\t",
            isARepeat: false,
            keyCode: 48
        )!
    }

    /// A row that can be selected.
    private func hit(_ id: UInt32) -> SearchHit {
        SearchHit(
            id: id,
            score: 100,
            path: "/tmp/f\(id).txt",
            relativePath: "f\(id).txt",
            name: "f\(id).txt",
            directory: "",
            isDirectory: false,
            positions: []
        )
    }

    func test_the_tab_key_is_the_panel_s_own() {
        // The search field would otherwise take tab as a move of the
        // focus, and the panel has nowhere else for the focus to go.
        XCTAssertEqual(PanelKey.action(for: tab), .previewToggle)
    }

    func test_the_preview_is_off_until_it_is_asked_for() {
        let model = AppModel()

        XCTAssertFalse(model.isPreviewShown)
    }

    func test_the_tab_key_turns_the_preview_on_and_off() {
        let model = AppModel()

        XCTAssertTrue(model.handleKeyEvent(tab), "the tab key reached the search field")
        XCTAssertTrue(model.isPreviewShown)

        XCTAssertTrue(model.handleKeyEvent(tab))
        XCTAssertFalse(model.isPreviewShown)
    }

    func test_the_preview_is_remembered() {
        // The panel is summoned afresh hundreds of times a day, and a
        // setting that had to be turned on again each time would be a
        // setting nobody used twice.
        let model = AppModel()
        model.togglePreview()

        XCTAssertTrue(Preferences.shared.showPreview)
        XCTAssertTrue(AppModel().isPreviewShown, "a new panel forgot the setting")
    }

    func test_the_settings_reach_a_panel_that_is_already_open() async throws {
        // The settings window is opened from the panel's own gear button,
        // and the panel stays on screen behind it. A checkbox that only
        // took effect the next time the panel was summoned would read as
        // a checkbox that did nothing.
        let model = AppModel()
        XCTAssertFalse(model.isPreviewShown)

        Preferences.shared.showPreview = true

        let deadline = Date().addingTimeInterval(2)
        while !model.isPreviewShown, Date() < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }

        XCTAssertTrue(model.isPreviewShown, "the setting never reached the panel")
    }

    func test_there_is_nothing_to_preview_without_a_selection() {
        // The column is drawn only when there is a row for it to follow,
        // so an empty query leaves the panel exactly the shape it is
        // today rather than putting an empty box beside an empty list.
        let model = AppModel()
        model.togglePreview()

        XCTAssertTrue(model.isPreviewShown)
        XCTAssertNil(model.selectedHit)
        XCTAssertFalse(model.isPreviewVisible, "an empty list was given a preview column")

        model.setTestResults([hit(1), hit(2)])

        XCTAssertTrue(model.isPreviewVisible, "a selected row was given no preview column")
    }
}
