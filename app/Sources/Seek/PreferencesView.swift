//
//  PreferencesView.swift
//  Seek
//
//  The settings window: the directories that are indexed, the exclusion
//  patterns, and the matching options.
//

import SwiftUI
import AppKit

/// The preferences of the app.
@MainActor
final class Preferences: ObservableObject {
    /// The shared instance, so that the settings window and the panel read
    /// the same values.
    static let shared = Preferences()

    /// The directories that are indexed, one per line.
    @AppStorage("roots") var rootsText: String = ""

    /// The exclusion patterns, one per line.
    @AppStorage("excludes") var excludesText: String = ""

    /// Whether matching is case sensitive.
    @AppStorage("caseSensitive") var caseSensitive: Bool = false

    /// Whether accented Latin letters are folded to their base letter.
    @AppStorage("normalize") var normalize: Bool = true

    /// Whether a bare term is an exact substring rather than a fuzzy
    /// subsequence. This is fzf's `--exact`.
    ///
    /// On by default, where fzf's is off. fzf is a filter the user pipes a
    /// list into, and a loose first pass they then narrow is what they
    /// want from it; this is a launcher, where the top row is usually the
    /// answer. `nihao` finding 174,504 paths — `Hao Law NIW EB1 Documents
    /// List.pdf` among them, through five scattered letters — is not an
    /// answer. The panel's ⌘E switches it, and `'` asks for the other
    /// mode for one term.
    @AppStorage("exactMatch") var exactMatch: Bool = true

    /// Whether the `content:` operator searches file contents.
    @AppStorage("searchContent") var searchContent: Bool = false

    /// The scoring scheme.
    @AppStorage("scheme") var scheme: Int = 1

    /// The global shortcut that summons the panel.
    @AppStorage("hotKeyCode") var hotKeyCode: Int = 49

    /// The modifier keys of the global shortcut, as Carbon bits.
    @AppStorage("hotKeyModifiers") var hotKeyModifiers: Int = 4096

    /// Whether the preview column is shown beside the results.
    ///
    /// Off by default: a launcher is opened hundreds of times a day to
    /// type three letters and press return, and a panel half again as
    /// wide for all of them would be paying for a preview nobody asked to
    /// see.
    @AppStorage("showPreview") var showPreview: Bool = false

    /// How long the panel stays open before it hides itself, in seconds.
    ///
    /// Zero means the panel stays open until it is closed by hand.
    @AppStorage("autoHideSeconds") var autoHideSeconds: Double = 30

    /// The global shortcut that summons the panel.
    var hotKey: HotKeyBinding {
        get {
            HotKeyBinding(
                keyCode: UInt32(hotKeyCode),
                modifiers: UInt32(hotKeyModifiers)
            )
        }
        set {
            hotKeyCode = Int(newValue.keyCode)
            hotKeyModifiers = Int(newValue.modifiers)
        }
    }

    /// The directories that are indexed.
    var roots: [String] {
        rootsText
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// The exclusion patterns.
    var excludes: [String] {
        excludesText
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// The default roots, used the first time the app runs.
    static var defaultRoots: [String] {
        [FileManager.default.homeDirectoryForCurrentUser.path]
    }

    /// The default exclusions.
    static var defaultExcludes: [String] {
        [
            "Library",
            ".Trash",
            ".cache",
            ".npm",
            ".cargo",
            "node_modules",
            ".git",
            "Applications",
        ]
    }

    private init() {
        // The defaults are filled in the first time the app runs.
        if rootsText.isEmpty {
            rootsText = Self.defaultRoots.joined(separator: "\n")
        }
        if excludesText.isEmpty {
            excludesText = Self.defaultExcludes.joined(separator: "\n")
        }
    }
}

/// The settings window.
struct PreferencesView: View {
    @ObservedObject var preferences = Preferences.shared

    /// The closure that closes the window.
    var onClose: () -> Void = {}

    /// Whether the shortcut is being recorded.
    @State private var isRecording = false

    /// The shortcut that was captured, or `nil` when nothing was captured.
    @State private var recordingBinding: HotKeyBinding?

    /// Applies a shortcut that the user has captured.
    ///
    /// The shortcut is registered at once, so that the user can try it
    /// without leaving the settings window.
    private func applyShortcut(_ binding: HotKeyBinding) {
        preferences.hotKey = binding
        HotKeyManager.store(binding)
        HotKeyManager.shared.update(to: binding)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    shortcutSection
                    rootsSection
                    excludesSection
                    matchingSection
                    behaviourSection
                }
                .padding(22)
            }

            Divider()

            footer
        }
        .frame(width: 560, height: 540)
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Seek")
                    .font(.system(size: 15, weight: .semibold))

                Text("Search your files the way fzf searches them.")
                    .font(.system(size: 11.5))
                    .foregroundStyle(Theme.secondary)
            }

            Spacer()
        }
        .padding(.horizontal, 22)
        .padding(.vertical, 16)
    }

    /// A note that explains a setting in one line.
    private func hint(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 11.5))
            .foregroundStyle(Theme.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .lineSpacing(1.5)
    }

    /// The heading of a section.
    private func sectionTitle(_ title: String, icon: String) -> some View {
        Label {
            Text(title)
                .font(.system(size: 12.5, weight: .semibold))
        } icon: {
            Image(systemName: icon)
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(Theme.accent)
        }
        .foregroundStyle(.primary)
    }

    /// The section that sets the global shortcut.
    ///
    /// The shortcut is captured by pressing the keys, rather than by
    /// choosing them from a list of checkboxes, because that is how every
    /// other launcher does it and because it is the only way to set a
    /// combination that is not one of the presets.
    private var shortcutSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionTitle("Shortcut", icon: "keyboard")

            hint("Press the keys to summon the panel. Esc cancels.")

            HStack(spacing: 10) {
                // The current shortcut is shown as text, so that the user
                // can read it without having to focus the recorder.
                Text(preferences.hotKey.displayString)
                    .font(.system(size: 13, weight: .medium))
                    .frame(width: 110, alignment: .leading)

                HotKeyRecorder(
                    binding: $recordingBinding,
                    placeholder: preferences.hotKey,
                    isRecording: $isRecording
                )
                .frame(width: 110, height: 24)
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .stroke(
                            isRecording ? Theme.accent : Theme.secondary.opacity(0.4),
                            lineWidth: 1
                        )
                )
                .onChange(of: recordingBinding) { _, newValue in
                    // The shortcut is applied as soon as it is captured, so
                    // that the user can try it right away.
                    if let newValue {
                        applyShortcut(newValue)
                    }
                }

                if isRecording {
                    Button("Cancel") {
                        isRecording = false
                    }
                    .buttonStyle(.link)
                } else {
                    Button("Change…") {
                        isRecording = true
                    }
                }

                Spacer()
            }

            if let conflict = shortcutConflict {
                Label(conflict, systemImage: "exclamationmark.triangle")
                    .font(.system(size: 11.5))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// A known shortcut that the chosen one collides with.
    ///
    /// The user is told, rather than left to find out that the shortcut
    /// does nothing because another app took it.
    private var shortcutConflict: String? {
        let binding = preferences.hotKey

        // Spotlight takes command and space, which is the shortcut the app
        // used to use.
        if binding.modifiers & HotKeyModifier.command != 0,
           binding.keyCode == 49 {
            return "Spotlight already uses ⌘Space. Turn it off in System Settings › Keyboard › Keyboard Shortcuts."
        }

        return nil
    }

    /// The section that sets how the panel behaves.
    private var behaviourSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionTitle("Behaviour", icon: "gearshape")

            HStack(spacing: 8) {
                Text("Hide after")

                    .font(.system(size: 12))

                Picker("", selection: $preferences.autoHideSeconds) {
                    Text("Never").tag(0.0)
                    Text("15 seconds").tag(15.0)
                    Text("30 seconds").tag(30.0)
                    Text("1 minute").tag(60.0)
                    Text("5 minutes").tag(300.0)
                }
                .labelsHidden()
                .frame(width: 140)

                Spacer()
            }

            hint("The panel hides itself when it is left alone, so it never sits on top of your work.")

            Toggle("Show the preview column", isOn: $preferences.showPreview)
                .font(.system(size: 12))

            hint("A QuickLook preview of the selected file, beside the results. Press \u{21e5} to show or hide it without coming here.")
        }
    }

    private var rootsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionTitle("Indexed folders", icon: "folder")

            hint("One folder per line. `~` is expanded.")

            TextEditor(text: $preferences.rootsText)
                .font(.system(size: 12, design: .monospaced))
                .frame(height: 80)
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .stroke(Theme.secondary.opacity(0.3), lineWidth: 1)
                )

            HStack {
                Button("Add Folder…") { addFolder() }
                Spacer()
            }

            // A path that does not exist indexes nothing at all, and the
            // panel would only say that nothing matched. The mistake is
            // named here, where it was made.
            if let invalid = invalidRoot {
                Label(invalid, systemImage: "exclamationmark.triangle")
                    .font(.system(size: 11.5))
                    .foregroundStyle(.orange)
            }
        }
    }

    /// The first indexed folder that does not exist, if there is one.
    private var invalidRoot: String? {
        for root in preferences.roots {
            let path = (root as NSString).expandingTildeInPath
            var isDirectory: ObjCBool = false
            if !FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory) {
                return "“\(root)” does not exist."
            }
        }
        return nil
    }

    private var excludesSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionTitle("Excluded patterns", icon: "nosign")

            hint("One glob per line, matched the way `fd --exclude` matches them.")

            TextEditor(text: $preferences.excludesText)
                .font(.system(size: 12, design: .monospaced))
                .frame(height: 80)
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .stroke(Theme.secondary.opacity(0.3), lineWidth: 1)
                )
        }
    }

    private var matchingSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            sectionTitle("Matching", icon: "textformat")

            Toggle("Exact matching", isOn: $preferences.exactMatch)

            hint(
                "A term matches characters that sit together: `nihao` finds "
                    + "`nihao.txt` and not `Hao Law NIW.pdf`. Turn it off for "
                    + "fzf's fuzzy matching, where the characters only have to "
                    + "appear in order. \u{2318}E switches it from the panel, and a "
                    + "leading `'` asks for the other one for a single term."
            )

            Toggle("Case sensitive", isOn: $preferences.caseSensitive)
            Toggle("Ignore accents (é matches e)", isOn: $preferences.normalize)

            // The content search is not implemented yet. A toggle that does
            // nothing would be worse than no toggle, because a user who
            // turns it on would conclude that the matcher is broken. The
            // query parser refuses `content:` by name for the same reason.
            Toggle("Search file contents with `content:`", isOn: .constant(false))
                .disabled(true)
                .help("Not available yet — the query is refused rather than ignored")

            Picker("Scoring", selection: $preferences.scheme) {
                Text("Path").tag(1)
                Text("Default").tag(0)
            }
            .pickerStyle(.radioGroup)

            hint(
                "Path scoring rewards matches that land on a folder boundary, "
                    + "the way fzf's path scheme does. Default scoring ranks by "
                    + "the match alone."
            )
        }
        .font(.system(size: 12))
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Text("Changes take effect the next time the app launches.")
                .font(.system(size: 11))
                .foregroundStyle(Theme.secondary)
            Spacer()
            Button("Relaunch Now") { relaunch() }
            Button("Quit Seek") { NSApp.terminate(nil) }
            Button("Done") { onClose() }
                .keyboardShortcut(.defaultAction)
        }
        .padding(.horizontal, 22)
        .padding(.vertical, 12)
    }

    /// Relaunches the app, so that the new settings take effect at once.
    ///
    /// The app has no Dock icon and no menu bar, so there is no other way
    /// to restart it. Telling the user to relaunch without giving them a
    /// way to relaunch would be an instruction they cannot follow.
    private func relaunch() {
        let bundleURL = Bundle.main.bundleURL

        NSApp.terminate(nil)

        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
            process.arguments = ["-n", bundleURL.path]
            try? process.run()
        }
    }

    /// Opens a folder picker and adds the chosen folder to the roots.
    private func addFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Add"

        guard panel.runModal() == .OK, let url = panel.url else { return }

        let path = url.path
        var lines = preferences.rootsText
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }

        if !lines.contains(path) {
            lines.append(path)
            preferences.rootsText = lines.joined(separator: "\n")
        }
    }
}
