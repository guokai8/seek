//
//  ContentView.swift
//  Seek
//
//  The floating panel: a search field at the top and a list of results below.
//

import SwiftUI

/// The content of the floating panel.
struct ContentView: View {
    @EnvironmentObject private var model: AppModel

    @FocusState private var isSearchFocused: Bool

    /// How tall the results would like to be.
    ///
    /// A `ScrollView` has no height of its own: it takes whatever it is
    /// offered, which in a panel that sizes itself to its content is
    /// nothing. So the list inside it is measured, and the scroll view is
    /// given that height up to [`Theme.maxResultsHeight`].
    @State private var resultsContentHeight: CGFloat = 0

    /// What the preview column is pointed at.
    ///
    /// The controller lives with the view rather than with the model,
    /// because it exists only while the column is on screen and because
    /// the model is already the largest file in the app.
    @StateObject private var preview = PreviewController()

    var body: some View {
        VStack(spacing: 0) {
            searchBar
            Divider().overlay(Theme.separator)

            HStack(spacing: 0) {
                resultsList

                if model.isPreviewShown, let hit = model.selectedHit {
                    Divider().overlay(Theme.separator)
                    PreviewPane(controller: preview, hit: hit, height: resultsHeight)
                }
            }

            Divider().overlay(Theme.separator)
            footer
        }
        .frame(width: model.isPreviewVisible
            ? Theme.panelWidth + Theme.previewWidth
            : Theme.panelWidth)
        .background(PanelBackground())
        .clipShape(PanelShape())
        .overlay(
            PanelShape()
                .stroke(Theme.panelBorder, lineWidth: 1)
        )
        // The window follows the panel's height rather than the other way
        // round. A fixed window leaves an empty state sitting in a tall
        // empty box, and the box is what the user sees: the panel's
        // background and border are drawn by this view, so a window taller
        // than its content is a panel with nothing under it.
        .onGeometryChange(for: CGSize.self) { geometry in
            geometry.size
        } action: { size in
            NotificationCenter.default.post(
                name: .seekResizePanel,
                object: nil,
                userInfo: ["width": size.width, "height": size.height]
            )
        }
        .onAppear {
            isSearchFocused = true
            preview.show(previewTarget)
        }
        // The selection is handed over by path rather than by index: a
        // streaming search renumbers the rows under the highlight between
        // pages, and an index that stayed at zero while the file under it
        // changed would leave the column showing the wrong file.
        .onChange(of: previewTarget?.path) { _, _ in
            preview.show(previewTarget)
        }
        .onReceive(NotificationCenter.default.publisher(for: .seekFocusSearch)) { _ in
            // The focus is requested after the panel is on screen and is
            // the key window, because a field cannot take the focus before
            // that.
            isSearchFocused = true
        }
    }

    // MARK: - Search bar

    /// The search field.
    ///
    /// The field sits in its own rounded well rather than directly on the
    /// panel's background. A flat field on a flat panel has no edge, so the
    /// eye has nothing to land on and the panel reads as one undifferentiated
    /// block. The well gives the query a place to live.
    private var searchBar: some View {
        HStack(spacing: 9) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(Theme.secondary)
                .font(.system(size: 14, weight: .medium))

            // The hint is drawn here rather than given to the field as its
            // placeholder. AppKit stops drawing a placeholder the moment
            // the field is being edited, and the panel opens with the
            // field already focused — which is the whole point of it — so
            // the field's own placeholder was never once on the screen.
            // The panel opened as an empty grey box with a caret in it.
            TextField("", text: $model.query)
                .textFieldStyle(.plain)
                .font(.query)
                .foregroundStyle(.primary)
                .focused($isSearchFocused)
                .onSubmit { model.openSelection() }
                .overlay(alignment: .leading) {
                    if model.query.isEmpty {
                        Text("Search files and folders")
                            .font(.query)
                            .foregroundStyle(Theme.tertiary)
                            .allowsHitTesting(false)
                    }
                }

            // The mode is shown beside the query because it changes what
            // the query means: the same five letters are five characters
            // in a row or five characters in order. A switch with no
            // visible state is a switch that makes the panel's answers
            // look arbitrary.
            Button {
                model.toggleExactMatching()
            } label: {
                // In the accent rather than in grey. Grey text on a grey
                // chip inside a grey well is what a disabled control looks
                // like, and this one is neither disabled nor decoration:
                // it is the switch that decides what the query means.
                Text(model.isExactMatching ? "exact" : "fuzzy")
                    .font(.key)
                    .foregroundStyle(Theme.accent)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2.5)
                    .background(
                        RoundedRectangle(cornerRadius: Theme.keyRadius, style: .continuous)
                            .fill(Theme.accent.opacity(0.13))
                    )
            }
            .buttonStyle(.plain)
            .help(model.isExactMatching
                ? "Exact matching — ⌘E for fuzzy"
                : "Fuzzy matching — ⌘E for exact")
            .accessibilityLabel(model.isExactMatching ? "Exact matching" : "Fuzzy matching")

            if model.isIndexing {
                ProgressView()
                    .controlSize(.small)
                    .scaleEffect(0.6)
                    .frame(width: 14)
            }

            if !model.query.isEmpty {
                Button {
                    model.query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(Theme.tertiary)
                        .font(.system(size: 13))
                }
                .buttonStyle(.plain)
                .help("Clear")
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(Theme.well)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .stroke(Theme.wellBorder, lineWidth: 1)
        )
        .padding(.horizontal, Theme.gutter)
        .padding(.vertical, 12)
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
                LazyVStack(spacing: 1) {
                    if model.results.isEmpty {
                        emptyState
                    } else {
                        ForEach(Array(model.results.enumerated()), id: \.element.id) { index, hit in
                            // No `.id(index)` here. An explicit `.id` on a
                            // row inside a `ForEach` replaces the identity
                            // the `ForEach` gave it, and the two then
                            // disagree: a new query is every row removed
                            // and every row inserted as far as the file
                            // ids go, while the indices 0...199 are
                            // unchanged, so the lazy stack keeps the views
                            // it already has — and the panel goes on
                            // drawing the previous query's rows under the
                            // new query's field, footer and preview. The
                            // scroll target is the file id instead; see
                            // `onChange` below.
                            ResultRow(
                                hit: hit,
                                isSelected: index == model.selection,
                                showsModified: model.showsModified,
                                showsSize: model.showsSize
                            )
                                .padding(.horizontal, 8)
                                .background(
                                    RowShape()
                                        .fill(
                                            index == model.selection
                                                ? Theme.selection
                                                : Color.clear
                                        )
                                )
                                .contentShape(Rectangle())
                                .onTapGesture {
                                    model.selection = index
                                }
                                // Reaching the end of the list asks for
                                // the next page. The trigger is a little
                                // before the last row so that the rows
                                // arrive while there is still something
                                // to scroll through, rather than after
                                // the scroll has already stopped.
                                .onAppear {
                                    if model.shouldLoadMore(forRow: index) {
                                        model.loadMoreResults()
                                    }
                                }
                        }
                    }
                }
                .padding(.vertical, 6)
                .onGeometryChange(for: CGFloat.self) { geometry in
                    geometry.size.height
                } action: { height in
                    resultsContentHeight = height
                }
            }
            .frame(width: Theme.panelWidth, height: resultsHeight)
            .onChange(of: model.selection) { _, newValue in
                // Scrolled to by the row's own identity, which is the file
                // id the `ForEach` keys on.
                guard newValue >= 0, newValue < model.results.count else { return }

                let row = model.results[newValue].id

                withAnimation(.easeOut(duration: 0.1)) {
                    proxy.scrollTo(row, anchor: .center)
                }
            }
        }
    }

    /// How tall the results list is.
    ///
    /// The panel is as tall as its content — except while the preview is
    /// open, when it takes its ceiling instead. Three rows beside a 60pt
    /// preview box is a preview nobody can read, and a box that changed
    /// height with every keystroke would be worse still.
    private var resultsHeight: CGFloat {
        model.isPreviewVisible
            ? Theme.maxResultsHeight
            : min(resultsContentHeight, Theme.maxResultsHeight)
    }

    /// The row the preview column should be showing, if any.
    private var previewTarget: SearchHit? {
        model.isPreviewShown ? model.selectedHit : nil
    }

    /// What the panel has to say when there are no rows, if anything.
    ///
    /// An empty query says nothing at all. The panel opens focused with
    /// its placeholder already in the field, so a block of prose beneath
    /// it only repeated what the field had just said, and it was the one
    /// empty state that showed up every single time the panel was
    /// summoned rather than when something needed explaining.
    ///
    /// The other three earn the room: each one answers "why is this list
    /// empty", which a blank rectangle leaves the user guessing at.
    private var emptyMessage: (icon: String, title: String, detail: String)? {
        if let reason = model.queryError {
            return ("exclamationmark.triangle", "That query cannot be read", reason)
        }

        if model.isIndexing {
            return (
                "arrow.triangle.2.circlepath", "Building the index",
                "The first search runs as soon as the index is ready."
            )
        }

        if model.query.isEmpty {
            return nil
        }

        return (
            "magnifyingglass", "No matches for \u{201C}\(model.query)\u{201D}",
            "The search covers the whole path, not just the name. "
                + "Try a shorter term, or quote it for an exact match."
        )
    }

    /// The message, laid out -- or nothing, taking no room at all.
    ///
    /// The padding belongs here rather than around the whole thing,
    /// because an empty container still obeys its padding: wrapped the
    /// other way round, an empty query left 52pt of blank panel behind
    /// where the prose used to be.
    @ViewBuilder
    private var emptyState: some View {
        if let message = emptyMessage {
            EmptyState(icon: message.icon, title: message.title, detail: message.detail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, Theme.gutter + 6)
                .padding(.vertical, 26)
        }
    }

    // MARK: - Footer

    private var footer: some View {
        HStack(spacing: 14) {
            Text(model.status)
                .font(.caption)
                .foregroundStyle(Theme.secondary)
                .lineLimit(1)

            Spacer(minLength: 8)

            if !model.results.isEmpty {
                HStack(spacing: 5) {
                    key("↑")
                    key("↓")
                    Text("Navigate")
                        .font(.caption)
                        .foregroundStyle(Theme.secondary)
                }

                HStack(spacing: 5) {
                    key("↩")
                    Text("Open")
                        .font(.caption)
                        .foregroundStyle(Theme.secondary)
                }

                HStack(spacing: 5) {
                    key("⇥")
                    Text("Preview")
                        .font(.caption)
                        .foregroundStyle(Theme.secondary)
                }

                HStack(spacing: 5) {
                    key("⌘⌥R")
                    Text("Reveal")
                        .font(.caption)
                        .foregroundStyle(Theme.secondary)
                }

                HStack(spacing: 5) {
                    key("⌘⇧C")
                    Text("Copy")
                        .font(.caption)
                        .foregroundStyle(Theme.secondary)
                }
            }

            // The settings are opened from here as well as from the menu
            // bar item: the panel is where the user already is, and the
            // menu bar is a trip away from it.
            Button {
                NotificationCenter.default.post(name: .seekShowSettings, object: nil)
            } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 11.5))
                    .foregroundStyle(Theme.secondary)
            }
            .buttonStyle(.plain)
            .help("Settings")
            .accessibilityLabel("Settings")
        }
        .padding(.horizontal, Theme.gutter + 2)
        .padding(.vertical, 8)
    }

    /// A small key cap shown in the footer.
    private func key(_ label: String) -> some View {
        Text(label)
            .font(.key)
            .foregroundStyle(Theme.secondary)
            .padding(.horizontal, 4.5)
            .padding(.vertical, 1.5)
            .background(
                RoundedRectangle(cornerRadius: Theme.keyRadius, style: .continuous)
                    .fill(Theme.keyFill)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.keyRadius, style: .continuous)
                    .stroke(Theme.wellBorder, lineWidth: 0.5)
            )
    }
}

/// The empty state of the panel.
private struct EmptyState: View {
    let icon: String
    let title: String
    let detail: String

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: icon)
                .font(.system(size: 17, weight: .regular))
                .foregroundStyle(Theme.tertiary)
                .frame(width: 20)
                .padding(.top, 1)

            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(.primary)

                Text(detail)
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .lineSpacing(1.5)
            }

            Spacer(minLength: 0)
        }
    }
}

/// The vibrancy background of the panel.
///
/// The material is a sidebar rather than a HUD, because a HUD is very
/// dark and the panel sits over whatever the user is working on. The
/// sidebar material keeps the text readable over both light and dark
/// backgrounds.
struct PanelBackground: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .sidebar
        view.blendingMode = .behindWindow
        view.state = .active
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {
        nsView.material = .sidebar
    }
}
