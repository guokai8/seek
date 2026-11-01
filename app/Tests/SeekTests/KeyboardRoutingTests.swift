//
//  KeyboardRoutingTests.swift
//  Seek
//
//  A check that the arrow keys actually move the selection.
//
//  The panel is not a normal window: the search field sits at the front and
//  consumes the arrow keys as text cursor movement. The panel therefore
//  intercepts its own keys with a local event monitor, and this test
//  exercises that exact path.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class KeyboardRoutingTests: XCTestCase {
    /// The key codes of the keys the panel handles.
    private enum Key {
        static let upArrow: UInt16 = 126
        static let downArrow: UInt16 = 125
        static let home: UInt16 = 115
        static let end: UInt16 = 119
    }

    /// Builds a model with a few results in the list.
    private func makeModel() -> AppModel {
        let model = AppModel()

        model.setTestResults([
            SearchHit(
                id: 0,
                score: 100,
                path: "/tmp/alpha.txt",
                relativePath: "/tmp/alpha.txt",
                name: "alpha.txt",
                directory: "/tmp",
                isDirectory: false,
                positions: []
            ),
            SearchHit(
                id: 1,
                score: 90,
                path: "/tmp/beta.txt",
                relativePath: "/tmp/beta.txt",
                name: "beta.txt",
                directory: "/tmp",
                isDirectory: false,
                positions: []
            ),
            SearchHit(
                id: 2,
                score: 80,
                path: "/tmp/gamma.txt",
                relativePath: "/tmp/gamma.txt",
                name: "gamma.txt",
                directory: "/tmp",
                isDirectory: false,
                positions: []
            ),
        ])

        return model
    }

    /// Builds a key event.
    private func keyEvent(
        _ keyCode: UInt16,
        modifiers: NSEvent.ModifierFlags = []
    ) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: modifiers,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: 0,
            context: nil,
            characters: "",
            charactersIgnoringModifiers: "",
            isARepeat: false,
            keyCode: keyCode
        )!
    }

    func test_the_arrow_keys_move_the_selection() {
        let model = makeModel()

        XCTAssertEqual(model.selection, 0)

        // The down arrow moves the selection down.
        model.handleKeyEvent(keyEvent(Key.downArrow))
        XCTAssertEqual(
            model.selection,
            1,
            "the down arrow did not move the selection"
        )

        model.handleKeyEvent(keyEvent(Key.downArrow))
        XCTAssertEqual(model.selection, 2)

        // The up arrow moves it back.
        model.handleKeyEvent(keyEvent(Key.upArrow))
        XCTAssertEqual(
            model.selection,
            1,
            "the up arrow did not move the selection"
        )
    }

    func test_the_selection_stays_inside_the_results() {
        let model = makeModel()

        // Up at the top stays at the top.
        model.handleKeyEvent(keyEvent(Key.upArrow))
        XCTAssertEqual(model.selection, 0)

        // Down at the bottom stays at the bottom.
        for _ in 0..<10 {
            model.handleKeyEvent(keyEvent(Key.downArrow))
        }
        XCTAssertEqual(model.selection, 2)
    }

    func test_home_and_end_jump_to_the_ends() {
        let model = makeModel()

        model.handleKeyEvent(keyEvent(Key.end))
        XCTAssertEqual(model.selection, 2, "End did not go to the last result")

        model.handleKeyEvent(keyEvent(Key.home))
        XCTAssertEqual(model.selection, 0, "Home did not go to the first result")
    }

    func test_a_typed_character_is_not_consumed() {
        let model = makeModel()

        // The letter "a" is not one of the panel's keys, so the monitor
        // leaves it alone and it reaches the search field.
        let consumed = model.handleKeyEvent(keyEvent(0))

        XCTAssertFalse(
            consumed,
            "a typed character was swallowed by the panel's key handling"
        )
        XCTAssertEqual(model.selection, 0)
    }

    func test_the_panel_opens_with_an_empty_query() {
        let model = makeModel()
        model.query = "leftover"
        model.selection = 2

        model.prepareForOpen()

        XCTAssertEqual(model.query, "", "the query was not cleared")
        XCTAssertEqual(model.selection, 0, "the selection was not reset")
        XCTAssertTrue(model.results.isEmpty, "the results were not cleared")
    }
}
