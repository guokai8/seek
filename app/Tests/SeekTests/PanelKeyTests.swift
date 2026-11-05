//
//  PanelKeyTests.swift
//  Seek
//
//  A check that the panel's shortcuts map to the right actions.
//
//  The mapping is kept separate from the model, so that the chords can be
//  tested without quitting the test runner or opening the settings
//  window.
//

import AppKit
import XCTest

@testable import Seek

@MainActor
final class PanelKeyTests: XCTestCase {
    /// Builds a key event.
    private func event(
        _ keyCode: UInt16,
        _ flags: NSEvent.ModifierFlags = []
    ) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: flags,
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: 0,
            context: nil,
            characters: "",
            charactersIgnoringModifiers: "",
            isARepeat: false,
            keyCode: keyCode
        )!
    }

    func test_the_navigation_keys_are_mapped() {
        XCTAssertEqual(PanelKey.action(for: event(126)), .moveUp)
        XCTAssertEqual(PanelKey.action(for: event(125)), .moveDown)
        XCTAssertEqual(PanelKey.action(for: event(115)), .first)
        XCTAssertEqual(PanelKey.action(for: event(119)), .last)
        XCTAssertEqual(PanelKey.action(for: event(36)), .open)
    }

    func test_the_chords_are_mapped() {
        XCTAssertEqual(
            PanelKey.action(for: event(36, [.command])),
            .reveal,
            "Cmd+Return should reveal"
        )
        XCTAssertNil(
            PanelKey.action(for: event(15, [.command, .option])),
            "Cmd+Opt+R was the old reveal chord and is gone"
        )
        XCTAssertEqual(
            PanelKey.action(for: event(36)),
            .open,
            "Return on its own still opens -- the modifier is what tells them apart"
        )
        XCTAssertEqual(
            PanelKey.action(for: event(8, [.command, .shift])),
            .copyPath,
            "Cmd+Shift+C should copy the path"
        )
        XCTAssertEqual(
            PanelKey.action(for: event(43, [.command])),
            .settings,
            "Cmd+, should open the settings"
        )
        XCTAssertEqual(
            PanelKey.action(for: event(12, [.command])),
            .quit,
            "Cmd+Q should quit"
        )
        XCTAssertEqual(
            PanelKey.action(for: event(14, [.command])),
            .exactToggle,
            "Cmd+E should switch between exact and fuzzy matching"
        )
    }

    func test_the_matching_switch_needs_the_command_key_alone() {
        // `e` is a letter people type, and ⌘⌥E and ⌘⇧E are not the
        // panel's: a chord it does not claim has to reach the field.
        XCTAssertNil(PanelKey.action(for: event(14)))
        XCTAssertNil(PanelKey.action(for: event(14, [.command, .option])))
        XCTAssertNil(PanelKey.action(for: event(14, [.command, .shift])))
    }

    func test_a_typed_character_is_not_the_panel_s_key() {
        // The letter "a" is what a person types, so it must reach the
        // search field rather than being swallowed.
        XCTAssertNil(PanelKey.action(for: event(0)))
    }

    func test_the_navigation_keys_ignore_the_modifiers() {
        // Cmd+Down is a shortcut of something else, so the panel must not
        // steal it.
        XCTAssertNil(PanelKey.action(for: event(125, [.command])))
        XCTAssertNil(PanelKey.action(for: event(126, [.option])))
        XCTAssertNil(PanelKey.action(for: event(125, [.control])))
    }

    func test_the_chords_are_not_confused_with_the_navigation_keys() {
        // The arrow keys carry no characters, and the chords are matched
        // on their key code, so the two cannot be mixed up.
        XCTAssertNotEqual(PanelKey.action(for: event(15, [.command, .option])), .moveUp)
        XCTAssertNotEqual(PanelKey.action(for: event(8, [.command, .shift])), .moveDown)
    }

    func test_the_editing_shortcuts_are_mapped() {
        // The app has no Edit menu of its own, so the editing commands are
        // mapped here. Without them the query cannot be replaced: selecting
        // it and typing over it appends instead of replacing.
        XCTAssertEqual(
            PanelKey.action(for: event(0, [.command])),
            .selectAll,
            "Cmd+A should select the query"
        )
        XCTAssertEqual(
            PanelKey.action(for: event(8, [.command])),
            .copy,
            "Cmd+C should copy"
        )
        XCTAssertEqual(
            PanelKey.action(for: event(9, [.command])),
            .paste,
            "Cmd+V should paste"
        )
        XCTAssertEqual(
            PanelKey.action(for: event(7, [.command])),
            .cut,
            "Cmd+X should cut"
        )
    }

    func test_the_editing_shortcuts_need_the_command_key() {
        // Without the command key these are ordinary letters, and they
        // have to reach the search field.
        XCTAssertNil(PanelKey.action(for: event(0)))
        XCTAssertNil(PanelKey.action(for: event(8)))
        XCTAssertNil(PanelKey.action(for: event(9)))
        XCTAssertNil(PanelKey.action(for: event(7)))
    }

    func test_the_copy_path_shortcut_is_not_the_copy_shortcut() {
        // Cmd+C copies from the search field, and Cmd+Shift+C copies the
        // path of the result. The two must not be confused.
        XCTAssertEqual(PanelKey.action(for: event(8, [.command])), .copy)
        XCTAssertEqual(PanelKey.action(for: event(8, [.command, .shift])), .copyPath)
    }
}
