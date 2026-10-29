//
//  PreferencesView.swift
//  Fuzzylite
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

    /// Whether the `content:` operator searches file contents.
    @AppStorage("searchContent") var searchContent: Bool = false

    /// The scoring scheme.
    @AppStorage("scheme") var scheme: Int = 1

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

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    rootsSection
                    excludesSection
                    matchingSection
                }
                .padding(20)
            }

            Divider()

            footer
        }
        .frame(width: 560, height: 520)
    }

    private var header: some View {
        HStack {
            Text("Fuzzylite Settings")
                .font(.system(size: 15, weight: .semibold))
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    private var rootsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Indexed folders", systemImage: "folder")
                .font(.system(size: 12, weight: .semibold))

            Text("One folder per line. `~` is expanded.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            TextEditor(text: $preferences.rootsText)
                .font(.system(size: 12, design: .monospaced))
                .frame(height: 80)
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(.quaternary, lineWidth: 1)
                )

            HStack {
                Button("Add Folder…") { addFolder() }
                Spacer()
            }
        }
    }

    private var excludesSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Excluded patterns", systemImage: "nosign")
                .font(.system(size: 12, weight: .semibold))

            Text("One glob per line, matched the way `fd --exclude` matches them.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            TextEditor(text: $preferences.excludesText)
                .font(.system(size: 12, design: .monospaced))
                .frame(height: 80)
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(.quaternary, lineWidth: 1)
                )
        }
    }

    private var matchingSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Matching", systemImage: "textformat")
                .font(.system(size: 12, weight: .semibold))

            Toggle("Case sensitive", isOn: $preferences.caseSensitive)
            Toggle("Ignore accents (é matches e)", isOn: $preferences.normalize)
            Toggle("Search file contents with `content:`", isOn: $preferences.searchContent)

            Picker("Scoring", selection: $preferences.scheme) {
                Text("Path").tag(1)
                Text("Default").tag(0)
            }
            .pickerStyle(.radioGroup)
        }
        .font(.system(size: 12))
    }

    private var footer: some View {
        HStack {
            Text("Changes take effect the next time the app launches.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            Spacer()
            Button("Done") { onClose() }
                .keyboardShortcut(.defaultAction)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
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
