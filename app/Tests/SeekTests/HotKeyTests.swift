//
//  HotKeyTests.swift
//  Seek
//
//  A check that the global shortcut is stored and shown correctly.
//
//  The shortcut is the one thing the user has to get right for the app to
//  be usable at all, and it was hardcoded to `Cmd+Space`, which Spotlight
//  already owns.
//

import XCTest

@testable import Seek

final class HotKeyTests: XCTestCase {
    func test_the_default_shortcut_is_control_and_space() {
        let binding = HotKeyBinding.controlSpace

        XCTAssertEqual(binding.keyCode, 49, "space is not the key")
        XCTAssertTrue(
            binding.modifiers & HotKeyModifier.control != 0,
            "the control key is not part of the default shortcut"
        )
        XCTAssertTrue(
            binding.modifiers & HotKeyModifier.command == 0,
            "the default shortcut uses the command key, which Spotlight owns"
        )
        XCTAssertTrue(binding.isValid)
    }

    func test_a_shortcut_without_a_modifier_is_rejected() {
        // Such a shortcut would swallow a bare key press everywhere in the
        // system, so it must not be accepted.
        let binding = HotKeyBinding(keyCode: 49, modifiers: 0)

        XCTAssertFalse(binding.isValid)
    }

    func test_the_shortcut_is_written_the_way_it_is_shown() {
        let binding = HotKeyBinding(
            keyCode: 49,
            modifiers: HotKeyModifier.control | HotKeyModifier.shift
        )

        XCTAssertEqual(binding.displayString, "⌃⇧Space")
    }

    func test_the_shortcut_is_remembered() {
        let binding = HotKeyBinding(
            keyCode: 36,
            modifiers: HotKeyModifier.command | HotKeyModifier.option
        )

        HotKeyManager.store(binding)

        let restored = HotKeyManager.storedBinding()

        XCTAssertEqual(restored, binding)

        // The stored value is put back, so that the test does not change
        // the user's own settings.
        HotKeyManager.store(HotKeyBinding.controlSpace)
    }

    func test_the_modifier_bits_round_trip() {
        let flags: NSEvent.ModifierFlags = [.command, .shift, .option, .control]

        let bits = HotKeyModifier.carbonBits(for: flags)
        let restored = HotKeyModifier.eventFlags(for: bits)

        XCTAssertEqual(restored, flags)
    }
}
