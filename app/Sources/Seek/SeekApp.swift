//
//  SeekApp.swift
//  Seek
//
//  The application entry point. The app has no main window: it shows a
//  floating panel that is toggled with the global hotkey.
//

import AppKit
import Combine
import SwiftUI

@main
struct SeekApp: App {
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
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    /// The model shared by the panel.
    @MainActor lazy var model = AppModel()

    /// The floating panel.
    private var panel: NSPanel?

    /// The panel the delegate opened.
    ///
    /// The tests need it, because the application keeps every panel that
    /// was ever opened: a test that looked for *a* panel would find one
    /// from an earlier test and type into that one instead of its own.
    var testPanel: NSPanel? { panel }

    /// The global hotkey.
    private let hotKey = HotKeyManager.shared

    /// The menu bar item.
    private var statusItem: NSStatusItem?

    /// The menu bar item the delegate installed, for the tests.
    var testStatusItem: NSStatusItem? { statusItem }

    /// The menu line that reports the size of the index.
    private var indexStatusItem: NSMenuItem?

    /// Keeps the menu bar icon in step with the index.
    private var indexingSubscription: AnyCancellable?

    /// Keeps the panel's opacity in step with the setting.
    ///
    /// Watched rather than read when the panel opens: the settings window
    /// is opened from the panel and the panel stays on screen behind it,
    /// so a slider whose effect only appeared at the next summon would be
    /// a slider the user cannot aim.
    private var opacitySubscription: AnyCancellable?

    /// The event monitor that handles the panel's own keys.
    private var focusMonitor: Any?

    /// The timer that hides the panel when it is left alone.
    private var idleTimer: Timer?

    /// Whether the panel is being opened right now.
    ///
    /// The app's activation is itself a deactivation of whatever was in
    /// front, so the panel must not be hidden by that.
    private var isOpeningPanel = false

    /// The settings window.
    private var settingsWindow: NSWindow?

    /// The settings window the delegate opened, for the tests.
    var testSettingsWindow: NSWindow? { settingsWindow }

    /// The controller that hosts the panel's SwiftUI view.
    ///
    /// The controller has to be kept alive. A `NSHostingController` owns
    /// the subscription that connects the model to the view, so when it is
    /// released the panel stops updating: the query reaches the model, the
    /// model finds the files, and the results never appear on the screen.
    ///
    /// Holding only the view — which is what the window does — is not
    /// enough. The window keeps the pixels; the controller keeps the
    /// connection.
    private var panelHosting: NSHostingController<AnyView>?

    /// The controller that hosts the settings view.
    private var settingsHosting: NSHostingController<AnyView>?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // The app is a background agent: it has no Dock icon and no menu
        // bar, which is what a launcher should look like.
        NSApp.setActivationPolicy(.accessory)

        // The hotkey is registered with Carbon rather than with a global
        // event monitor, so it works without the accessibility permission.
        hotKey.onTrigger = { [weak self] in
            self?.togglePanel()
        }
        hotKey.start()

        installFocusMonitor()
        installStatusItem()

        // The settings window is opened from the keyboard shortcut.
        NotificationCenter.default.addObserver(
            forName: .seekShowSettings,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.showSettings()
            }
        }

        // The panel is as tall as its content, and the content says so
        // from inside SwiftUI.
        NotificationCenter.default.addObserver(
            forName: .seekResizePanel,
            object: nil,
            queue: .main
        ) { [weak self] note in
            guard let width = note.userInfo?["width"] as? CGFloat,
                  let height = note.userInfo?["height"] as? CGFloat
            else {
                return
            }

            MainActor.assumeIsolated {
                self?.resizePanel(to: CGSize(width: width, height: height))
            }
        }

        // The panel is created lazily so that the first launch is not slowed
        // down by building the UI before it is needed.
        DispatchQueue.main.async { [weak self] in
            self?.showPanel()

            // The icon follows the index from here rather than from the
            // line above, because reading the model is what builds it:
            // asking for it before the run loop has started would move
            // the index's restore and first scan into the launch itself.
            self?.followIndex()
        }

        // The command line can ask for a panel with a query in it. This is
        // how the app is checked without a person having to press the
        // hotkey and type: the command runs the whole path — panel, focus,
        // search — and the panel stays on screen to be looked at.
        if let query = CommandLine.arguments.dropFirst().first(where: { $0.hasPrefix("--query=") })
            .map({ String($0.dropFirst("--query=".count)) }) {
            // The query is set after the panel has been shown, because
            // showing the panel clears the query. Setting it first would be
            // undone a moment later.
            DispatchQueue.main.async { [weak self] in
                self?.showPanel()

                DispatchQueue.main.async { [weak self] in
                    self?.model.query = query
                }
            }
        }

        // The panel can also be written to a picture. The picture is taken
        // from the live view, so it shows exactly what the user sees —
        // including whether the results were drawn at all, which is the
        // one thing that a test of the model cannot tell.
        if let path = CommandLine.arguments.dropFirst().first(where: { $0.hasPrefix("--snapshot=") })
            .map({ String($0.dropFirst("--snapshot=".count)) }) {
            DispatchQueue.main.async { [weak self] in
                self?.writeSnapshot(to: path)
            }
        }
    }

    /// Writes the panel to a PNG file.
    ///
    /// The picture is taken from the panel's own view, after it has been
    /// laid out, so it shows the real result rows rather than a
    /// reconstruction of them.
    ///
    /// The picture is taken a moment later, so that the index has been
    /// built and the search has finished. A picture of an empty panel
    /// would say nothing about whether the results are drawn.
    private func writeSnapshot(to path: String) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
            guard let self, let panel = self.panel, let view = panel.contentView else {
                NSLog("seek: there is no panel to snapshot")
                return
            }

            // The view has to be laid out before it can be drawn.
            view.layoutSubtreeIfNeeded()

            guard let representation = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
                NSLog("seek: the panel could not be drawn")
                return
            }

            view.cacheDisplay(in: view.bounds, to: representation)

            guard let data = representation.representation(using: .png, properties: [:]) else {
                NSLog("seek: the picture could not be encoded")
                return
            }

            do {
                try data.write(to: URL(fileURLWithPath: path))
                NSLog(
                    "seek: the panel was written to \(path) "
                        + "results=\(self.model.results.count) "
                        + "query=\"\(self.model.query)\""
                )
            } catch {
                NSLog("seek: the picture could not be written: \(error)")
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        hotKey.stop()
        if let focusMonitor {
            NSEvent.removeMonitor(focusMonitor)
        }
        if let statusItem {
            NSStatusBar.system.removeStatusItem(statusItem)
        }
        statusItem = nil
        indexingSubscription = nil
    }

    /// Hides the panel when the user moves on to another application.
    ///
    /// The panel does not do this by itself any more — see the note on
    /// `hidesOnDeactivate` in `makePanel` — so it is done here.
    ///
    /// The panel is not hidden when the app is deactivated before it has
    /// ever been shown, which is what happens while the app is starting
    /// up: the activation that brings the panel up is itself a
    /// deactivation of whatever was in front.
    func applicationDidResignActive(_ notification: Notification) {
        // The panel that is being opened right now is kept.
        guard !isOpeningPanel else { return }

        hidePanel()
    }

    // MARK: - Menu bar

    /// Puts the app in the menu bar.
    ///
    /// The app has no Dock icon, so without this there is nowhere it can
    /// be reached from: the hotkey is the only door, and a user who has
    /// forgotten it cannot even tell that the app is running. The icon
    /// says it is, the menu says what the hotkey is, and the menu's first
    /// line says how far the index has got.
    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)

        // A template image is drawn black on a light bar and white on a
        // dark one, and inverts when the bar is highlighted. A coloured
        // one stays the colour it was and vanishes into one of the two.
        let icon = NSImage(
            systemSymbolName: "magnifyingglass",
            accessibilityDescription: "Seek"
        )
        icon?.isTemplate = true

        item.button?.image = icon
        item.button?.toolTip = "Seek"

        let menu = NSMenu()
        menu.delegate = self

        let show = NSMenuItem(
            title: showTitle,
            action: #selector(showPanelFromMenu),
            keyEquivalent: ""
        )
        show.target = self
        menu.addItem(show)

        menu.addItem(.separator())

        let index = NSMenuItem(title: "", action: nil, keyEquivalent: "")
        index.isEnabled = false
        indexStatusItem = index
        menu.addItem(index)

        menu.addItem(.separator())

        let settings = NSMenuItem(
            title: "Settings…",
            action: #selector(showSettingsFromMenu),
            keyEquivalent: ","
        )
        settings.target = self
        menu.addItem(settings)

        let quit = NSMenuItem(
            title: "Quit Seek",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
        quit.target = NSApp
        menu.addItem(quit)

        item.menu = menu
        statusItem = item
    }

    /// What the menu's first line says.
    ///
    /// The hotkey is part of the title rather than the item's key
    /// equivalent. A key equivalent would be a second handler for the
    /// combination the global hotkey already answers, and the two would
    /// fight over a panel that is already open.
    private var showTitle: String {
        "Show Seek (\(HotKeyManager.storedBinding().displayString))"
    }

    /// Keeps the menu bar icon in step with the index.
    private func followIndex() {
        indexingSubscription = model.$isIndexing
            .receive(on: RunLoop.main)
            .sink { [weak self] indexing in
                MainActor.assumeIsolated {
                    self?.statusItem?.button?.appearsDisabled = indexing
                }
            }
    }

    /// Brings the menu up to date just before it is shown.
    ///
    /// The index's size is written in here rather than once at launch,
    /// because the index grows: a number taken at launch would be the
    /// snapshot's count for the rest of the session. The hotkey is
    /// rewritten for the same reason — it can be changed in the settings.
    func menuNeedsUpdate(_ menu: NSMenu) {
        indexStatusItem?.title = model.indexStatus
        menu.items.first?.title = showTitle
    }

    @objc private func showPanelFromMenu() {
        showPanel()
    }

    @objc private func showSettingsFromMenu() {
        showSettings()
    }

    // MARK: - Hotkey

    /// Installs the monitor that handles the panel's own keys.
    ///
    /// A local monitor sees the key events before they reach the search
    /// field, which is what makes the navigation keys work: the field
    /// consumes the arrow keys as text cursor movement, so by the time they
    /// reach the responder chain they are already spoken for.
    private func installFocusMonitor() {
        focusMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown]) { [weak self] event in
            guard let self else { return event }

            // The keystrokes that reach the app are logged, because a panel
            // that shows nothing is indistinguishable from a panel whose
            // keystrokes never arrived.
            NSLog(
                "seek: key code=\(event.keyCode) "
                    + "chars=\(event.charactersIgnoringModifiers ?? "") "
                    + "panelKey=\(String(describing: PanelKey.action(for: event)))"
            )

            // A row being renamed owns the keyboard. The panel's own
            // keys would otherwise open the file on Return and hide the
            // whole panel on Escape, over a field the user is typing a
            // name into.
            switch self.model.keyWhileRenaming(event) {
            case .cancelled:
                self.resetIdleTimer()
                return nil
            case .passThrough:
                self.resetIdleTimer()
                return event
            case .notRenaming:
                break
            }

            if event.keyCode == 53 { // Escape
                self.hidePanel()
                return nil
            }

            // The editing commands act on the search field. They are
            // handled here because the app has no Edit menu of its own:
            // the key equivalents of a menu are dispatched to the key
            // window, and an accessory app's panel is not reliably the key
            // window, so the shortcut would be swallowed before it reached
            // the field.
            if self.performEditingAction(for: event) {
                self.resetIdleTimer()
                return nil
            }

            // The event is only consumed when it is one of the panel's own
            // keys, so that typing in the search field is left alone.
            if self.model.handleKeyEvent(event) {
                self.resetIdleTimer()
                return nil
            }

            // Any other key is a keystroke in the search field, which
            // counts as the user still using the panel.
            self.resetIdleTimer()

            return event
        }
    }

    /// Performs an editing command on the search field.
    ///
    /// Returns whether the event was one of the editing commands *and* the
    /// command was carried out.
    ///
    /// The command is sent to the field's editor, which is the view that
    /// actually holds the text and the selection. Sending it to the field
    /// would not select anything: the field is not the responder, the
    /// editor is.
    ///
    /// When there is no editor — because the panel is not on screen, or the
    /// focus is somewhere else — the event is *not* consumed. It travels on
    /// to the responder chain, so the shortcut still does whatever the
    /// system would have done with it.
    private func performEditingAction(for event: NSEvent) -> Bool {
        guard let action = PanelKey.action(for: event) else { return false }

        guard let editor = self.searchFieldEditor() else { return false }

        switch action {
        case .selectAll:
            editor.selectAll(nil)
        case .cut:
            editor.cut(nil)
        case .copy:
            editor.copy(nil)
        case .paste:
            editor.paste(nil)
        default:
            return false
        }

        return true
    }

    /// The editor of the panel's search field.
    ///
    /// The editor is the view that holds the text and the selection. The
    /// field is only the box it is drawn in.
    private func searchFieldEditor() -> NSText? {
        guard let panel = self.panel,
              let view = panel.contentView,
              let field = Self.findTextField(in: view)
        else {
            return nil
        }

        // `currentEditor()` already returns an `NSText?`, so there is
        // nothing to cast.
        return field.currentEditor()
    }

    /// Finds the search field in a view tree.
    private static func findTextField(in view: NSView) -> NSTextField? {
        if let field = view as? NSTextField { return field }

        for subview in view.subviews {
            if let found = findTextField(in: subview) { return found }
        }

        return nil
    }

    // MARK: - Panel

    /// Shows the panel, creating it if needed.
    private func showPanel() {
        let panel = self.panel ?? makePanel()
        self.panel = panel

        // The panel opens empty. A launcher is summoned hundreds of times a
        // day, and a leftover query would make the first keystroke append
        // to the last one instead of replacing it.
        model.prepareForOpen()

        // The panel is an activating panel, so it has to be made the key
        // window before the search field can take the focus.
        //
        // `orderFrontRegardless` on its own is not enough: a panel that is
        // not activating does not take the key window, so the text field
        // never receives the key presses and the panel looks dead.
        //
        // The app is activated *before* the panel is ordered in. The order
        // matters: the app's activation is itself a deactivation of
        // whatever was in front, and the panel would be hidden by that if
        // it were already on the screen.
        isOpeningPanel = true
        NSApp.activate(ignoringOtherApps: true)
        panel.makeKeyAndOrderFront(nil)
        isOpeningPanel = false
        model.isPanelVisible = true
        // The panel is moved to the screen the mouse is on every time it
        // opens. It was only positioned when it was created, so a panel
        // that was first opened on one display stayed there, off the
        // screen the user was looking at.
        centerPanelOnMouseScreen(panel)

        // The focus is requested after the panel is on screen and is the key
        // window, because a field in a window that is not key cannot become
        // the first responder.
        DispatchQueue.main.async { [weak self] in
            self?.model.requestSearchFocus()

            // The focus is checked a moment later, because SwiftUI's
            // `@FocusState` needs a turn of the run loop to act on the
            // notification. The state is logged, because a panel that is
            // on screen but has no focus looks exactly like a panel that
            // does not work.
            DispatchQueue.main.async { [weak self] in
                guard let self, let panel = self.panel else { return }

                NSLog(
                    "seek: panel isKey=\(panel.isKeyWindow) "
                        + "visible=\(panel.isVisible) "
                        + "appActive=\(NSApp.isActive) "
                        + "responder=\(String(describing: panel.firstResponder))"
                )
            }
        }

        resetIdleTimer()
    }

    /// Moves the panel to the middle of the screen the mouse is on.
    ///
    /// The arithmetic is in `PanelPlacement`, where it can be checked
    /// without a window and a display.
    private func centerPanelOnMouseScreen(_ panel: NSPanel) {
        guard let screen = screenUnderMouse() else { return }

        panel.setFrameOrigin(
            PanelPlacement.summoned(
                size: panel.frame.size,
                on: screen.visibleFrame
            ).origin
        )
    }

    /// Resizes the panel to the size its content reported.
    ///
    /// The top edge and the horizontal centre stay where they are, and the
    /// panel is pulled back onto the screen if the preview column has
    /// pushed it over an edge. See `PanelPlacement.resized`.
    private func resizePanel(to size: CGSize) {
        guard let panel, size.width > 0, size.height > 0 else { return }

        let screen = screenUnderMouse()?.visibleFrame
            ?? NSScreen.main?.visibleFrame
            ?? panel.frame

        let frame = PanelPlacement.resized(panel.frame, to: size, within: screen)

        // Layout reports the same size many times over. Anything under a
        // point is not a change worth a window redraw.
        guard abs(frame.width - panel.frame.width) >= 1
            || abs(frame.height - panel.frame.height) >= 1
            || abs(frame.minX - panel.frame.minX) >= 1
        else {
            return
        }

        panel.setFrame(frame, display: true)

        // The shadow is drawn from the opaque part of the window and is
        // cached, so a panel that changed size keeps the old shadow until
        // it is told otherwise.
        panel.invalidateShadow()
    }

    /// Creates the floating panel.
    private func makePanel() -> NSPanel {
        let content = ContentView()
            .environmentObject(model)

        let hosting = NSHostingController(rootView: AnyView(content))

        // The controller is kept in a property, so that it outlives this
        // method. See `panelHosting`.
        panelHosting = hosting

        let panel = NSPanel(
            // The height is a starting point only: the content reports its
            // own height as soon as it has laid out, and the panel follows
            // it. See `resizePanel`.
            contentRect: NSRect(
                x: 0,
                y: 0,
                width: Theme.panelWidth,
                height: Theme.panelMaxHeight
            ),
            // No window buttons. A launcher is dismissed with the escape
            // key or by clicking away from it, the way Spotlight is, so a
            // close button is a target the user has no reason to aim at
            // and a maximise button would be meaningless for a panel that
            // sizes itself to its results. `.closable` was here because
            // `performClose` needs it, but escape has not gone through
            // `performClose` for a while: it calls `hidePanel` directly.
            //
            // `.titled` stays. With `.fullSizeContentView`, a transparent
            // titlebar and a hidden title it draws nothing, and it is what
            // lets the panel become the key window — a `.borderless` panel
            // cannot without overriding `canBecomeKey`, and the search
            // field would never take the focus.
            //
            // The panel is also *not* a non-activating panel, for the same
            // reason.
            styleMask: [.fullSizeContentView, .titled],
            backing: .buffered,
            defer: false
        )

        panel.contentView = hosting.view
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        // The panel is not draggable by its background: the results fill
        // the window, and dragging them away mid-search loses the panel
        // off the edge of the screen.
        panel.isMovableByWindowBackground = false
        // The panel does *not* hide itself when the app loses the focus.
        //
        // `hidesOnDeactivate` fires at the wrong moment: it also fires for
        // the activation that brings the panel up, so the panel appears and
        // disappears in the same moment and is never on screen at all.
        //
        // The panel is hidden by hand instead — see
        // `applicationDidResignActive` — which is when the user really has
        // clicked away from it.
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = true
        panel.alphaValue = model.panelOpacity

        // The panel is movable, but only by `WindowDragArea` -- the strip
        // around the search well. `isMovableByWindowBackground` stays off
        // for the reason above it: it would make the results list a drag
        // handle too.
        opacitySubscription = model.$panelOpacity
            .receive(on: RunLoop.main)
            .sink { [weak panel] opacity in
                panel?.alphaValue = opacity
            }

        return panel
    }

    /// The screen that the mouse pointer is on.
    ///
    /// `NSScreen.main` is the screen that holds the key window, which is
    /// not the screen the user is looking at when the panel is summoned
    /// from another app.
    private func screenUnderMouse() -> NSScreen? {
        let location = NSEvent.mouseLocation

        return NSScreen.screens.first { screen in
            NSMouseInRect(location, screen.frame, false)
        } ?? NSScreen.main
    }

    /// Hides the panel.
    private func hidePanel() {
        panel?.orderOut(nil)
        idleTimer?.invalidate()
        idleTimer = nil

        // The watcher keeps reporting changes to a panel that is no longer
        // on screen, and searching for them is work nobody asked for.
        model.isPanelVisible = false
    }

    /// Shows or hides the panel.
    func togglePanel() {
        if panel?.isVisible == true {
            hidePanel()
        } else {
            showPanel()
        }
    }

    /// The time the panel stays open before it hides itself.
    ///
    /// A launcher panel that is left open sits on top of everything and
    /// hides the work behind it, so it gets out of the way on its own.
    private var idleTimeout: TimeInterval {
        max(0, Preferences.shared.autoHideSeconds)
    }

    /// Restarts the idle timer.
    private func resetIdleTimer() {
        idleTimer?.invalidate()
        idleTimer = nil

        // A timeout of zero means the panel stays open until it is closed
        // by hand, which is what a user who turned the option off expects.
        guard idleTimeout > 0 else { return }

        idleTimer = Timer.scheduledTimer(
            withTimeInterval: idleTimeout,
            repeats: false
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.hidePanel()
            }
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

        let hosting = NSHostingController(rootView: AnyView(view))

        // The controller is kept alive for the same reason the panel's one
        // is: it owns the connection between the preferences and the view.
        settingsHosting = hosting

        let window = NSWindow(contentViewController: hosting)
        window.title = "Seek Settings"
        // No window buttons, for the same reason the panel has none: the
        // window is dismissed with its own Done button, so close repeats
        // a control that is already there, minimise sends a window
        // belonging to an app with no Dock icon somewhere it cannot be
        // got back from, and zoom is meaningless for a form.
        //
        // `.titled` stays, so the window keeps its name and can be
        // dragged; dropping `.closable` is what takes the lights away,
        // and `close()` does not need it — only `performClose` does.
        window.styleMask = [.titled]
        window.isReleasedWhenClosed = false
        window.center()

        self.settingsWindow = window
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)    }
}
