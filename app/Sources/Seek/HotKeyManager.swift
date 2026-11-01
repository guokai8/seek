//
//  HotKeyManager.swift
//  Seek
//
//  The global shortcut that summons the panel.
//
//  The shortcut is registered with Carbon's `RegisterEventHotKey`, which is
//  the same mechanism macOS uses for its own shortcuts. It does not need the
//  accessibility permission that a global event monitor would, so the app
//  works the moment it is launched, and it works while another app is in
//  front — which is the whole point of a launcher.
//

import AppKit
import Carbon.HIToolbox
import SwiftUI

// MARK: - The binding

/// A key combination.
struct HotKeyBinding: Equatable {
    /// The virtual key code of the key.
    var keyCode: UInt32

    /// The modifier keys, as Carbon's modifier bits.
    var modifiers: UInt32

    /// The default shortcut: control and space.
    ///
    /// It is the same shortcut that Alfred and Raycast use, and it does not
    /// collide with Spotlight, which takes command and space.
    static let controlSpace = HotKeyBinding(keyCode: 49, modifiers: HotKeyModifier.control)

    /// Whether the binding is complete enough to be registered.
    ///
    /// A shortcut needs at least one modifier, otherwise it would swallow
    /// a bare key press everywhere in the system.
    var isValid: Bool {
        modifiers != 0 && keyCode != 0
    }

    /// The shortcut as it is written in the user interface.
    var displayString: String {
        var text = ""

        if modifiers & HotKeyModifier.control != 0 { text += "⌃" }
        if modifiers & HotKeyModifier.option != 0 { text += "⌥" }
        if modifiers & HotKeyModifier.shift != 0 { text += "⇧" }
        if modifiers & HotKeyModifier.command != 0 { text += "⌘" }

        return text + HotKeyBinding.keyName(for: keyCode)
    }

    /// The name of a key, the way it is shown in the user interface.
    static func keyName(for keyCode: UInt32) -> String {
        switch keyCode {
        case 49: return "Space"
        case 51: return "Delete"
        case 53: return "Esc"
        case 36: return "↩"
        case 48: return "Tab"
        case 37: return "←"
        case 38: return "↑"
        case 39: return "→"
        case 40: return "↓"
        case 33: return "Delete"
        case 115: return "Home"
        case 119: return "End"
        case 116: return "PgUp"
        case 121: return "PgDn"
        case 24: return "⇥"
        case 96: return "F5"
        case 97: return "F6"
        case 98: return "F7"
        case 99: return "F8"
        case 100: return "F9"
        case 101: return "F10"
        case 109: return "F11"
        case 103: return "F12"
        default: break
        }

        // Letters and digits are named by the character they produce.
        if let letter = UnicodeScalar(keyCode) {
            return String(letter).uppercased()
        }

        return "Key \(keyCode)"
    }
}

/// The modifier bits, as Carbon numbers them.
enum HotKeyModifier {
    static let command: UInt32 = 256
    static let shift: UInt32 = 512
    static let option: UInt32 = 2048
    static let control: UInt32 = 4096

    /// The Carbon bits of an event's modifier flags.
    static func carbonBits(for flags: NSEvent.ModifierFlags) -> UInt32 {
        var bits: UInt32 = 0

        if flags.contains(.command) { bits |= command }
        if flags.contains(.shift) { bits |= shift }
        if flags.contains(.option) { bits |= option }
        if flags.contains(.control) { bits |= control }

        return bits
    }

    /// The event flags of a set of Carbon bits.
    static func eventFlags(for bits: UInt32) -> NSEvent.ModifierFlags {
        var flags: NSEvent.ModifierFlags = []

        if bits & command != 0 { flags.insert(.command) }
        if bits & shift != 0 { flags.insert(.shift) }
        if bits & option != 0 { flags.insert(.option) }
        if bits & control != 0 { flags.insert(.control) }

        return flags
    }
}

// MARK: - The manager

/// The notification that asks the app to register a new shortcut.
extension Notification.Name {
    static let seekHotKeyChanged = Notification.Name("seek.hotKeyChanged")
}

/// Registers the global shortcut and reports when it is pressed.
///
/// The registration is a Carbon hot key rather than a global event
/// monitor, because a monitor only sees the key presses once the user has
/// granted the app accessibility access. A Carbon hot key works without
/// that permission, which removes a whole step from setting the app up.
@MainActor
final class HotKeyManager {
    /// The shared instance, so that the settings window and the app agree
    /// on the shortcut.
    static let shared = HotKeyManager()

    /// The shortcut that is currently registered.
    private(set) var binding: HotKeyBinding

    /// The Carbon registration, when one is active.
    private var hotKeyRef: EventHotKeyRef?

    /// The Carbon event handler, which stays registered for the life of the
    /// app.
    private var eventHandler: EventHandlerRef?

    /// The closure that is called when the shortcut is pressed.
    var onTrigger: (() -> Void)?

    /// Whether the last registration attempt failed.
    ///
    /// The system refuses a shortcut that another app already holds, and
    /// the user has to be told, rather than left with a shortcut that does
    /// nothing.
    private(set) var registrationFailed = false

    /// The signature that identifies this app's hot keys.
    private static let signature: OSType = 0x465A_4C54 // 'FZLT'

    /// The id of this app's hot key.
    private static let hotKeyID: UInt32 = 1

    init() {
        binding = HotKeyManager.storedBinding()
    }

    /// The shortcut the user has chosen, read from the settings.
    ///
    /// This is not tied to the main actor: it only reads and writes the
    /// user defaults, which are safe to use from anywhere.
    nonisolated static func storedBinding() -> HotKeyBinding {
        let keyCode = UserDefaults.standard.integer(forKey: "hotKeyCode")
        let modifiers = UserDefaults.standard.integer(forKey: "hotKeyModifiers")

        // The defaults are only applied when nothing has been stored yet.
        guard keyCode != 0 || modifiers != 0 else {
            return .controlSpace
        }

        return HotKeyBinding(
            keyCode: UInt32(truncatingIfNeeded: keyCode),
            modifiers: UInt32(truncatingIfNeeded: modifiers)
        )
    }

    /// Stores the shortcut in the settings.
    nonisolated static func store(_ binding: HotKeyBinding) {
        UserDefaults.standard.set(Int(binding.keyCode), forKey: "hotKeyCode")
        UserDefaults.standard.set(Int(binding.modifiers), forKey: "hotKeyModifiers")
    }

    /// Registers the shortcut.
    ///
    /// The shortcut is registered again when the user changes it, so that
    /// the new one takes effect at once rather than at the next launch.
    func start() {
        installEventHandler()
        register(binding)
    }

    /// Registers a new shortcut, replacing the previous one.
    func update(to binding: HotKeyBinding) {
        self.binding = binding

        unregister()
        register(binding)
    }

    /// Removes the shortcut.
    func stop() {
        unregister()
    }

    // MARK: - Registration

    /// Registers the Carbon hot key.
    private func register(_ binding: HotKeyBinding) {
        guard binding.isValid else { return }

        var identifier = EventHotKeyID()
        identifier.signature = HotKeyManager.signature
        identifier.id = HotKeyManager.hotKeyID

        var reference: EventHotKeyRef?
        let status = RegisterEventHotKey(
            binding.keyCode,
            binding.modifiers,
            identifier,
            GetApplicationEventTarget(),
            0,
            &reference
        )

        guard status == noErr else {
            registrationFailed = true
            NSLog("seek: could not register the hot key: status \(status)")
            return
        }

        registrationFailed = false
        NSLog("seek: hot key registered: \(binding.displayString)")
        hotKeyRef = reference
    }

    /// Removes the Carbon hot key.
    private func unregister() {
        guard let reference = hotKeyRef else { return }

        UnregisterEventHotKey(reference)
        hotKeyRef = nil
    }

    /// Installs the event handler that receives the key presses.
    private func installEventHandler() {
        guard eventHandler == nil else { return }

        // The event type is the hot key press, which is a keyboard event.
        var spec = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard),
            eventKind: UInt32(kEventHotKeyPressed)
        )

        var handler: EventHandlerRef?

        let status = InstallEventHandler(
            GetApplicationEventTarget(),
            HotKeyManager.eventHandler,
            1,
            &spec,
            UnsafeMutableRawPointer(Unmanaged.passUnretained(self).toOpaque()),
            &handler
        )

        guard status == noErr else {
            NSLog("seek: could not install the hot key handler: status \(status)")
            return
        }
        NSLog("seek: hot key handler installed")
        eventHandler = handler
    }

    /// The Carbon event handler.
    ///
    /// It is a plain C function pointer, so it cannot touch the main actor
    /// directly. The work is handed over to the main queue instead.
    private static let eventHandler: EventHandlerProcPtr = { _, event, userData in
        guard let event, let userData else { return OSStatus(eventNotHandledErr) }

        var identifier = EventHotKeyID()
        let status = GetEventParameter(
            event,
            EventParamName(kEventParamDirectObject),
            EventParamType(typeEventHotKeyID),
            nil,
            MemoryLayout<EventHotKeyID>.size,
            nil,
            &identifier
        )

        guard status == noErr, identifier.signature == HotKeyManager.signature else {
            return OSStatus(eventNotHandledErr)
        }

        let manager = Unmanaged<HotKeyManager>.fromOpaque(userData).takeUnretainedValue()

        Task { @MainActor in
            manager.onTrigger?()
        }

        return noErr
    }
}

// MARK: - The recorder

/// A field that captures the key combination the user presses.
///
/// The field is a real view rather than a SwiftUI control, because only a
/// view that is the first responder receives the key presses.
struct HotKeyRecorder: NSViewRepresentable {
    /// The shortcut that is being recorded, or `nil` while the field is
    /// waiting for a key press.
    @Binding var binding: HotKeyBinding?

    /// The shortcut to show when nothing is being recorded.
    let placeholder: HotKeyBinding?

    /// Whether the field is recording.
    @Binding var isRecording: Bool

    func makeNSView(context: Context) -> HotKeyField {
        let view = HotKeyField()
        view.onChange = { newBinding in
            isRecording = false
            binding = newBinding
        }
        view.onCancel = {
            isRecording = false
        }
        return view
    }

    func updateNSView(_ view: HotKeyField, context: Context) {
        view.binding = binding
        view.placeholder = placeholder
        view.isRecording = isRecording
        view.needsDisplay = true
    }

    /// The view that does the capturing.
    final class HotKeyField: NSView {
        /// The shortcut being recorded.
        var binding: HotKeyBinding?

        /// The shortcut shown when nothing is being recorded.
        var placeholder: HotKeyBinding?

        /// Whether the field is recording.
        var isRecording = false

        /// Called when a combination has been captured.
        var onChange: ((HotKeyBinding?) -> Void)?

        /// Called when the recording was cancelled.
        var onCancel: (() -> Void)?

        override var acceptsFirstResponder: Bool { true }

        override func becomeFirstResponder() -> Bool {
            needsDisplay = true
            return true
        }

        override func resignFirstResponder() -> Bool {
            needsDisplay = true
            return true
        }

        override func mouseDown(with event: NSEvent) {
            window?.makeFirstResponder(self)
        }

        override func keyDown(with event: NSEvent) {
            // Escape cancels, so the user is never trapped in the field.
            if event.keyCode == 53 {
                onCancel?()
                window?.makeFirstResponder(nil)
                return
            }

            // A shortcut needs a modifier, otherwise it would swallow the
            // key press everywhere in the system.
            let modifiers = HotKeyModifier.carbonBits(for: event.modifierFlags)
            guard modifiers != 0 else { return }

            onChange?(HotKeyBinding(keyCode: UInt32(event.keyCode), modifiers: modifiers))
            window?.makeFirstResponder(nil)
        }

        override func draw(_ dirtyRect: NSRect) {
            let text: String
            if isRecording {
                text = "Press keys…"
            } else if let binding {
                text = binding.displayString
            } else {
                text = placeholder?.displayString ?? "Not set"
            }

            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 13, weight: .medium),
                .foregroundColor: NSColor.labelColor,
            ]

            let size = text.size(withAttributes: attributes)
            let origin = NSPoint(
                x: (bounds.width - size.width) / 2,
                y: (bounds.height - size.height) / 2
            )

            text.draw(at: origin, withAttributes: attributes)
        }

        /// The text that the field shows.
        ///
        /// The text is exposed so that it can be checked without having to
        /// draw the view, which the snapshot renderer cannot do.
        var displayText: String {
            if isRecording {
                return "Press keys…"
            }
            if let binding {
                return binding.displayString
            }
            return placeholder?.displayString ?? "Not set"
        }
    }
}
