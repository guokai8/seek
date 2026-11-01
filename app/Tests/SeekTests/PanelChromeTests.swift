import XCTest

@testable import Seek

/// The panel carries no window buttons.
///
/// A launcher is dismissed with the escape key or by clicking away from
/// it, the way Spotlight is. A close button is a target the user has no
/// reason to aim at, and a maximise button is meaningless for a panel that
/// sizes itself to its results.
@MainActor
final class PanelChromeTests: XCTestCase {
    /// The delegate the last `openPanel` built.
    private var delegate: AppDelegate?

    private func openPanel() async throws -> NSPanel? {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let delegate = AppDelegate()
        self.delegate = delegate
        delegate.applicationDidFinishLaunching(
            Notification(name: NSApplication.didFinishLaunchingNotification)
        )

        try await Task.sleep(for: .milliseconds(600))
        return delegate.testPanel
    }

    func test_the_panel_has_no_close_minimise_or_zoom_button() async throws {
        guard let panel = try await openPanel() else {
            XCTFail("the app did not open a panel")
            return
        }

        XCTAssertFalse(
            panel.styleMask.contains(.closable),
            "a closable panel draws a close button"
        )
        XCTAssertFalse(panel.styleMask.contains(.miniaturizable))
        XCTAssertFalse(panel.styleMask.contains(.resizable))

        for button in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            let drawn = panel.standardWindowButton(button).map { !$0.isHidden } ?? false
            XCTAssertFalse(drawn, "\(button) is drawn on the panel")
        }
    }

    func test_the_settings_window_has_no_window_buttons_either() async throws {
        // The settings window is reached from the panel's gear button and
        // is dismissed with its own Done button, so the three lights in
        // its corner are targets with nothing behind them: close repeats
        // Done, minimise sends a window with no Dock icon somewhere it
        // cannot be got back from, and zoom is meaningless for a form.
        guard try await openPanel() != nil else {
            XCTFail("the app did not open a panel")
            return
        }

        let delegate = self.delegate
        delegate?.showSettings()

        try await Task.sleep(for: .milliseconds(400))

        let window = try XCTUnwrap(delegate?.testSettingsWindow, "the settings did not open")

        XCTAssertFalse(window.styleMask.contains(.miniaturizable))

        for button in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            let drawn = window.standardWindowButton(button).map { !$0.isHidden } ?? false
            XCTAssertFalse(drawn, "\(button) is drawn on the settings window")
        }

        // And it can still be put away, which is what the lights were
        // standing in for.
        window.close()
        XCTAssertFalse(window.isVisible)
    }

    /// The panel must still be typeable into.
    ///
    /// `.titled` is what lets it become the key window. A `.borderless`
    /// panel — the obvious way to get rid of the chrome — cannot, without
    /// overriding `canBecomeKey`, and the search field would never take
    /// the focus.
    func test_the_panel_can_still_become_the_key_window() async throws {
        guard let panel = try await openPanel() else {
            XCTFail("the app did not open a panel")
            return
        }

        XCTAssertTrue(panel.canBecomeKey, "the search field could not be typed into")
    }
}
