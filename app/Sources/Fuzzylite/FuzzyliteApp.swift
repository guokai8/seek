//
//  FuzzyliteApp.swift
//  Fuzzylite
//
//  The application entry point. The app has no main window: it shows a
//  floating panel that is toggled with the global hotkey.
//

import AppKit
import ApplicationServices
import SwiftUI

@main
struct FuzzyliteApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    init() {
        // The smoke test is a command line check that bypasses the UI, so
        // the app is turned into a plain command line tool when it is asked
        // for.
        let arguments = CommandLine.arguments

        if arguments.count >= 3, arguments[1] == "--smoke-test" {
            let status = SmokeTest.run(root: arguments[2])
            exit(status)
        }
    }

    var body: some Scene {
        Settings {
            EmptyView()
        }
    }
}

/// The application delegate, which owns the hotkey and the panel.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// The model shared by the panel.
    @MainActor lazy var model = AppModel()

    /// The floating panel.
    private var panel: NSPanel?

    /// The global hotkey monitor.
    private var hotkeyMonitor: Any?

    /// The event monitor that closes the panel when it loses focus.
    private var focusMonitor: Any?

    /// The settings window.
    private var settingsWindow: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // The app is a background agent: it has no Dock icon and no menu
        // bar, which is what a launcher should look like.
        NSApp.setActivationPolicy(.accessory)

        requestAccessibilityIfNeeded()
        installHotkey()
        installFocusMonitor()

        // The settings window is opened from the keyboard shortcut.
        NotificationCenter.default.addObserver(
            forName: .fuzzyliteShowSettings,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.showSettings()
            }
        }

        // The panel is created lazily so that the first launch is not slowed
        // down by building the UI before it is needed.
        DispatchQueue.main.async { [weak self] in
            self?.showPanel()
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        if let hotkeyMonitor {
            NSEvent.removeMonitor(hotkeyMonitor)
        }
        if let focusMonitor {
            NSEvent.removeMonitor(focusMonitor)
        }
    }

    // MARK: - Hotkey

    /// Asks for the accessibility permission that the hotkey needs.
    ///
    /// macOS only lets an app watch the keyboard globally once the user has
    /// granted it accessibility access. Without the permission the hotkey
    /// silently does nothing, so the prompt is shown once at launch rather
    /// than leaving the user with a key that does not work.
    private func requestAccessibilityIfNeeded() {
        // The key is a documented constant of the accessibility API. It is
        // spelled out here because the global it comes from is not safe to
        // touch from Swift's concurrency checks.
        let options = ["AXTrustedCheckOptionPrompt": true] as CFDictionary
        let trusted = AXIsProcessTrustedWithOptions(options)

        guard !trusted else { return }

        // The prompt is a system dialog; the app waits for the user to
        // answer it before installing the monitor.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [weak self] in
            self?.installHotkey()
        }
    }

    /// Installs the global `Cmd+Space` hotkey.
    private func installHotkey() {
        let mask = NSEvent.ModifierFlags.command

        hotkeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard event.modifierFlags.contains(mask),
                  event.charactersIgnoringModifiers == " " else {
                return
            }
            DispatchQueue.main.async {
                self?.togglePanel()
            }
        }
    }

    /// Installs the monitor that hides the panel when it loses focus.
    private func installFocusMonitor() {
        focusMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown]) { [weak self] event in
            guard event.keyCode == 53 else { return event } // Escape
            self?.hidePanel()
            return nil
        }
    }

    // MARK: - Panel

    /// Shows the panel, creating it if needed.
    private func showPanel() {
        let panel = self.panel ?? makePanel()
        self.panel = panel

        panel.orderFrontRegardless()
        NSApp.activate(ignoringOtherApps: true)

        // The search field takes the focus so that typing works right away.
        panel.makeFirstResponder(panel.firstResponder)
    }

    /// Creates the floating panel.
    private func makePanel() -> NSPanel {
        let content = ContentView()
            .environmentObject(model)

        let hosting = NSHostingController(rootView: content)

        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 460),
            styleMask: [.nonactivatingPanel, .fullSizeContentView, .titled],
            backing: .buffered,
            defer: false
        )

        panel.contentView = hosting.view
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = true
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = true

        // The panel is centred on the screen that the mouse is on, which is
        // where the user is looking.
        if let screen = NSScreen.main {
            let frame = panel.frame
            let origin = NSPoint(
                x: screen.visibleFrame.midX - frame.width / 2,
                y: screen.visibleFrame.midY - frame.height / 3
            )
            panel.setFrameOrigin(origin)
        }

        return panel
    }

    /// Hides the panel.
    private func hidePanel() {
        panel?.orderOut(nil)
        NSApp.hide(nil)
    }

    /// Shows or hides the panel.
    func togglePanel() {
        if panel?.isVisible == true {
            hidePanel()
        } else {
            showPanel()
        }
    }

    /// Shows the settings window.
    func showSettings() {
        if let settingsWindow {
            settingsWindow.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        let view = PreferencesView {
            self.settingsWindow?.close()
        }

        let hosting = NSHostingController(rootView: view)
        let window = NSWindow(contentViewController: hosting)
        window.title = "Fuzzylite Settings"
        window.styleMask = [.titled, .closable, .miniaturizable]
        window.isReleasedWhenClosed = false
        window.center()

        self.settingsWindow = window
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}
