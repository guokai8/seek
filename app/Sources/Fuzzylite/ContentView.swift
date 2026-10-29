//
//  ContentView.swift
//  Fuzzylite
//
//  The floating panel: a search field at the top and a list of results below.
//

import SwiftUI

/// The content of the floating panel.
struct ContentView: View {
    @EnvironmentObject private var model: AppModel

    @FocusState private var isSearchFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            searchBar
            Divider().opacity(0.5)
            resultsList
            Divider().opacity(0.5)
            footer
        }
        .frame(width: 720, height: 460)
        .background(VibrancyBackground())
        .background(KeyboardHandler(model: model))
        .onAppear {
            isSearchFocused = true
        }
    }

    // MARK: - Search bar

    private var searchBar: some View {
        HStack(spacing: 10) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)
                .font(.system(size: 15, weight: .medium))

            TextField("Search files and folders", text: $model.query)
                .textFieldStyle(.plain)
                .font(.system(size: 20, weight: .light))
                .focused($isSearchFocused)
                .onSubmit { model.openSelection() }

            if model.isIndexing {
                ProgressView()
                    .controlSize(.small)
                    .scaleEffect(0.7)
            }

            if !model.query.isEmpty {
                Button {
                    model.query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
                .help("Clear")
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 14)
    }

    // MARK: - Results

    /// The results list.
    ///
    /// A plain scroll view is used rather than a `List`, because the list's
    /// selection handling does not cope well with the results changing
    /// while the selection is being updated.
    private var resultsList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(model.results.enumerated()), id: \.element.id) { index, hit in
                        ResultRow(hit: hit)
                            .id(index)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 2)
                            .background(
                                index == model.selection
                                    ? Color.accentColor.opacity(0.22)
                                    : Color.clear
                            )
                            .contentShape(Rectangle())
                            .onTapGesture {
                                model.selection = index
                            }
                    }
                }
                .padding(.vertical, 4)
            }
            .onChange(of: model.selection) { _, newValue in
                withAnimation(.easeOut(duration: 0.1)) {
                    proxy.scrollTo(newValue, anchor: .center)
                }
            }
        }
    }

    // MARK: - Footer

    private var footer: some View {
        HStack(spacing: 12) {
            Text(model.status)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)

            Spacer()

            if !model.results.isEmpty {
                HStack(spacing: 4) {
                    key("↑")
                    key("↓")
                    Text("navigate")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }

                HStack(spacing: 4) {
                    key("↩")
                    Text("open")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }

                HStack(spacing: 4) {
                    key("⌘⌥")
                    Text("reveal")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 8)
    }

    /// A small key cap shown in the footer.
    private func key(_ label: String) -> some View {
        Text(label)
            .font(.system(size: 10, weight: .medium, design: .rounded))
            .padding(.horizontal, 4)
            .padding(.vertical, 1)
            .background(
                RoundedRectangle(cornerRadius: 3)
                    .fill(.quaternary)
            )
    }
}

/// The vibrancy background of the panel.
struct VibrancyBackground: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .hudWindow
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {
        nsView.material = .hudWindow
    }
}
