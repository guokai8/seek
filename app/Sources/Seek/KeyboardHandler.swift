//
//  KeyboardHandler.swift
//  Seek
//
//  The keyboard shortcuts of the panel.
//
//  The panel intercepts its own keys with a local event monitor, which
//  sees them before the search field does. That is what makes the
//  navigation keys work: the field consumes the arrow keys as text cursor
//  movement, so by the time they reach the responder chain they are
//  already spoken for.
//
//  A background `NSView` cannot do this. Such a view is not part of the
//  responder chain, so it never sees the key presses at all.
//

import AppKit
import SwiftUI

/// The keys that the panel handles itself.
///
/// The mapping is kept apart from the model, so that it can be tested
/// without the test runner being quit or the settings window opening.
enum PanelKey {
    /// What a key press does.
    enum Action {
        case moveUp
        case moveDown
        case first
        case last
        case open
        case reveal
        case copyPath
        case settings
        case quit

        /// Shows or hides the preview column.
        case previewToggle

        /// Switches between exact and fuzzy matching.
        case exactToggle

        /// Selects the whole query, so that it can be typed over.
        case selectAll

        /// Cuts the selected text.
        case cut

        /// Copies the selected text.
        case copy

        /// Pastes the clipboard into the query.
        case paste
    }

    /// The action a key press maps to, or `nil` when the key is not one of
    /// the panel's own.
    ///
    /// The chords are matched first, because they share their key codes
    /// with the unmodified keys.
    static func action(for event: NSEvent) -> Action? {
        let flags = event.modifierFlags
        let command = flags.contains(.command)
        let option = flags.contains(.option)
        let shift = flags.contains(.shift)

        // Cmd+Return reveals the result in Finder, next to the plain
        // Return that opens it. The two live on one key because they are
        // one thought -- "this row" -- and the modifier says where it
        // goes. The old chord was Cmd+Opt+R, three keys away from the one
        // the hand is already on.
        if command && event.keyCode == 36 {
            return .reveal
        }

        // Cmd+Shift+C copies the path. Cmd+C belongs to the search field,
        // so the path is copied with an extra modifier. fzf uses ctrl-y
        // for the same thing.
        if command && shift && event.keyCode == 8 {
            return .copyPath
        }

        // Cmd+E switches between exact and fuzzy matching. E for exact,
        // and nothing else in the panel wants it: the field's own chords
        // are the editing ones below.
        if command && !option && !shift && event.keyCode == 14 {
            return .exactToggle
        }

        // Cmd+, opens the settings, the way it does in every Mac app.
        if command && event.keyCode == 43 {
            return .settings
        }

        // Cmd+Q quits, the way it does in every Mac app. The app has no
        // Dock icon and no menu bar, so this is the only escape hatch.
        if command && event.keyCode == 12 {
            return .quit
        }

        // The editing shortcuts.
        //
        // These are handled here rather than by the Edit menu, because the
        // app is an accessory app with no menu bar of its own. The key
        // equivalents of a menu are dispatched to the *key window*, and an
        // accessory app's panel is not always the key window, so the
        // shortcut is swallowed before it reaches the search field.
        //
        // Without them the query cannot be edited: selecting it and typing
        // over it appends instead of replacing, because the selection never
        // happened.
        if command && !option && !shift {
            switch event.keyCode {
            case 0: return .selectAll // A
            case 8: return .copy // C
            case 9: return .paste // V
            case 7: return .cut // X
            default: break
            }
        }

        // The navigation keys only work without a modifier, because they
        // would otherwise be a shortcut of something else.
        guard !command, !option, !flags.contains(.control) else {
            return nil
        }

        switch event.keyCode {
        // Tab shows and hides the preview column. The search field would
        // otherwise take it as a move of the focus, and the panel has
        // nowhere else for the focus to go: there is one field, and losing
        // it leaves the panel unable to be typed into.
        case 48: return .previewToggle
        case 126: return .moveUp // Up arrow
        case 125: return .moveDown // Down arrow
        case 115: return .first // Home
        case 119: return .last // End
        case 36: return .open // Return
        default: return nil
        }
    }
}

/// The notification that asks the app to show the settings window.
extension Notification.Name {
    /// Posted when the settings window is requested.
    static let seekShowSettings = Notification.Name("seek.showSettings")

    /// Posted when the search field should take the focus.
    static let seekFocusSearch = Notification.Name("seek.focusSearch")

    /// Posted when the panel's content has changed size.
    ///
    /// The panel is as large as what is in it, and only the view knows
    /// how large that is — the height follows the results, and the width
    /// follows the preview column. Both travel in the `width` and
    /// `height` keys of the notification's `userInfo`.
    static let seekResizePanel = Notification.Name("seek.resizePanel")
}
