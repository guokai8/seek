import XCTest

@testable import Seek

/// The app lives in the menu bar.
///
/// A launcher with no Dock icon and no menu bar has nowhere to be reached
/// from: the hotkey is the only way in, and a user who has forgotten it
/// has no way to find out that the app is even running. The menu bar item
/// is that place — it says the app is there, what the hotkey is, and how
/// far the index has got.
@MainActor
final class StatusItemTests: XCTestCase {
    /// The delegate the last `launch` built.
    private var delegate: AppDelegate?

    private func launch() async throws -> AppDelegate {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let delegate = AppDelegate()
        self.delegate = delegate
        delegate.applicationDidFinishLaunching(
            Notification(name: NSApplication.didFinishLaunchingNotification)
        )

        try await Task.sleep(for: .milliseconds(600))
        return delegate
    }

    override func tearDown() async throws {
        delegate?.applicationWillTerminate(
            Notification(name: NSApplication.willTerminateNotification)
        )
        delegate = nil
    }

    func test_the_app_puts_an_icon_in_the_menu_bar() async throws {
        let delegate = try await launch()

        let item = try XCTUnwrap(delegate.testStatusItem, "there is no menu bar item")
        let button = try XCTUnwrap(item.button, "the menu bar item has no button")
        let image = try XCTUnwrap(button.image, "the menu bar item has no icon")

        // A template image is the only kind that follows the menu bar:
        // it is drawn black on a light bar and white on a dark one, and
        // it inverts when the bar is highlighted. A coloured image stays
        // the colour it was and disappears into one of the two.
        XCTAssertTrue(image.isTemplate, "the icon does not follow the menu bar")
    }

    func test_the_menu_shows_the_panel_the_hotkey_the_settings_and_the_way_out() async throws {
        let delegate = try await launch()

        let menu = try XCTUnwrap(delegate.testStatusItem?.menu, "the item has no menu")
        let titles = menu.items.map(\.title)

        let show = try XCTUnwrap(
            menu.items.first { $0.title.hasPrefix("Show") },
            "the menu does not offer to show the panel, titles: \(titles)"
        )
        XCTAssertNotNil(show.action, "the show item does nothing")

        // The hotkey is on the item rather than as its key equivalent. A
        // key equivalent would be a second handler for the combination the
        // global hotkey already answers, and the two would fight over a
        // panel that is already open.
        XCTAssertTrue(
            show.title.contains(HotKeyManager.storedBinding().displayString),
            "the menu does not say what the hotkey is: \(show.title)"
        )
        XCTAssertTrue(show.keyEquivalent.isEmpty, "the show item claims a key of its own")

        XCTAssertTrue(
            titles.contains { $0.hasPrefix("Settings") },
            "the menu does not reach the settings, titles: \(titles)"
        )
        XCTAssertTrue(
            titles.contains { $0.hasPrefix("Quit") },
            "the menu has no way out, titles: \(titles)"
        )
    }

    func test_the_menu_reports_the_index_when_it_is_opened() async throws {
        let delegate = try await launch()

        let menu = try XCTUnwrap(delegate.testStatusItem?.menu)

        // The line is filled in when the menu opens rather than once at
        // launch, because the index grows: a number written down at launch
        // would be the snapshot's count for the rest of the session.
        menu.delegate?.menuNeedsUpdate?(menu)

        let line = try XCTUnwrap(
            menu.items.first { $0.title == delegate.model.indexStatus },
            "the menu does not report the index, titles: \(menu.items.map(\.title))"
        )
        XCTAssertFalse(line.isEnabled, "the index line is a target with nothing behind it")
        XCTAssertFalse(line.title.isEmpty, "the index line says nothing")
    }

    func test_the_icon_is_dimmed_while_the_index_is_being_built() async throws {
        let delegate = try await launch()

        let button = try XCTUnwrap(delegate.testStatusItem?.button)

        // The icon carries the state as well as the menu does, because the
        // bar is what the user can see without clicking. A dimmed glass
        // means the answers are not complete yet.
        XCTAssertEqual(
            button.appearsDisabled,
            delegate.model.isIndexing,
            "the icon does not follow the index"
        )
    }

    func test_the_app_keeps_no_dock_icon() async throws {
        _ = try await launch()

        XCTAssertEqual(
            NSApp.activationPolicy(),
            .accessory,
            "the app would show up in the Dock and in the ⌘⇥ switcher"
        )
    }
}
