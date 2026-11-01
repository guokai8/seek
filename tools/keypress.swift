// Posts real key events to the frontmost application, the way a physical
// keyboard does.
//
//      ./keypress readme
//
// The virtual key codes are the ANSI ones, not the ASCII values: 'a' is 0x00
// on macOS, not 0x61. Sending the ASCII value produces the wrong key — or
// no key at all, which looks exactly like a broken app.

import Carbon.HIToolbox
import Foundation

/// The ANSI virtual key code of an ASCII character.
let ansiKeyCodes: [Character: UInt16] = [
    "a": 0x00, "s": 0x01, "d": 0x02, "f": 0x03, "h": 0x04,
    "g": 0x05, "z": 0x06, "x": 0x07, "c": 0x08, "v": 0x09,
    "b": 0x0B, "q": 0x0C, "w": 0x0D, "e": 0x0E, "r": 0x0F,
    "y": 0x10, "t": 0x11, "1": 0x12, "2": 0x13, "3": 0x14,
    "4": 0x15, "6": 0x16, "5": 0x17, "=": 0x18, "9": 0x19,
    "7": 0x1A, "-": 0x1B, "8": 0x1C, "0": 0x1D, "]": 0x1E,
    "o": 0x1F, "u": 0x20, "[": 0x21, "i": 0x22, "p": 0x23,
    "l": 0x25, "j": 0x26, "'": 0x27, "k": 0x28, ";": 0x29,
    "\\": 0x2A, ",": 0x2B, "/": 0x2C, "n": 0x2D, "m": 0x2E,
    ".": 0x2F, "`": 0x32, " ": 0x31,
]

/// The shifted character of a key, where the two differ.
let shiftedCharacters: [UInt16: String] = [
    0x00: "A", 0x01: "S", 0x02: "D", 0x03: "F", 0x04: "H",
    0x05: "G", 0x06: "Z", 0x07: "X", 0x08: "C", 0x09: "V",
    0x0B: "B", 0x0C: "Q", 0x0D: "W", 0x0E: "E", 0x0F: "R",
    0x10: "Y", 0x11: "T", 0x12: "!", 0x13: "@", 0x14: "#",
    0x15: "$", 0x16: "^", 0x17: "%", 0x18: "+", 0x19: "(",
    0x1A: "&", 0x1B: "_", 0x1C: "*", 0x1D: ")", 0x1E: "{",
    0x1F: "O", 0x20: "U", 0x21: "[", 0x22: "I", 0x23: "P",
    0x25: "L", 0x26: "J", 0x27: "\"", 0x28: "K", 0x29: ":",
    0x2A: "|", 0x2B: "<", 0x2C: "?", 0x2D: "N", 0x2E: "M",
    0x2F: ">", 0x32: "~",
]

let text = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ""

for character in text {
    guard let keyCode = ansiKeyCodes[character] else {
        FileHandle.standardError.write(
            Data("no key code for \(character)\n".utf8)
        )
        continue
    }

    let shifted = shiftedCharacters[keyCode]

    let down = CGEvent(
        keyboardEventSource: nil,
        virtualKey: keyCode,
        keyDown: true
    )

    // The unicode string is set as well as the key code. The key code
    // alone makes the system resolve the character through the current
    // keyboard layout, which produces the wrong letter on a layout that
    // is not the one the key code was taken from.
    var utf16 = Array(String(character).utf16)
    down?.keyboardSetUnicodeString(
        stringLength: utf16.count,
        unicodeString: &utf16
    )
    down?.post(tap: .cghidEventTap)

    usleep(60_000)

    let up = CGEvent(
        keyboardEventSource: nil,
        virtualKey: keyCode,
        keyDown: false
    )
    up?.keyboardSetUnicodeString(
        stringLength: utf16.count,
        unicodeString: &utf16
    )
    up?.post(tap: .cghidEventTap)

    usleep(60_000)

    print("posted \(character) -> keyCode \(keyCode)\(shifted.map { " (shifted \($0))" } ?? "")")
}

print("posted \(text.count) keys")
