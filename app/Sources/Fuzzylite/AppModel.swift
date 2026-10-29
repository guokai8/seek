//
//  AppModel.swift
//  Fuzzylite
//
//  The state of the launcher: the query, the results, and the lifecycle of
//  the background index.
//
//  The searches and the scan run off the main actor so that the panel stays
//  responsive while a quarter of a million files are being matched.
//

import Foundation
import SwiftUI

/// The observable state of the launcher.
@MainActor
final class AppModel: ObservableObject {
    /// The text the user has typed.
    @Published var query: String = "" {
        didSet {
            if query != oldValue {
                scheduleSearch()
            }
        }
    }

    /// The current results, ranked.
    @Published private(set) var results: [SearchHit] = []

    /// The index of the selected result.
    @Published var selection: Int = 0

    /// Whether the index is still being built.
    @Published private(set) var isIndexing: Bool = false

    /// The number of entries in the index.
    @Published private(set) var indexedCount: Int = 0

    /// A short status message shown in the footer.
    @Published private(set) var status: String = ""

    /// The engine that owns the Rust-side index.
    ///
    /// The engine is thread-safe, so it is not isolated to the main actor.
    /// The scan and the searches run on background tasks.
    nonisolated let engine = SearchEngine()

    /// The task that runs the pending search, so a new keystroke cancels the
    /// previous one.
    private var pendingSearch: Task<Void, Never>?

    /// The task that builds the index.
    private var indexingTask: Task<Void, Never>?

    /// The task that applies the changes the filesystem watcher reports.
    private var watchingTask: Task<Void, Never>?

    /// The generation counter, used to discard results that arrive after a
    /// newer query has been issued.
    private var generation: UInt64 = 0

    init() {
        configureRoots()
        restoreIndex()
        startIndexing()
    }

    deinit {
        pendingSearch?.cancel()
        indexingTask?.cancel()
        watchingTask?.cancel()
    }

    // MARK: - Configuration

    /// Configures the engine from the stored preferences.
    private func configureRoots() {
        let preferences = Preferences.shared

        let roots = preferences.roots.isEmpty
            ? Preferences.defaultRoots
            : preferences.roots

        let excludes = preferences.excludes.isEmpty
            ? Preferences.defaultExcludes
            : preferences.excludes

        engine.setRoots(roots)
        engine.setExcludes(excludes)
        engine.caseSensitive = preferences.caseSensitive
        engine.scheme = preferences.scheme
    }

    // MARK: - Indexing

    /// Loads the index snapshot from the last run, if there is one.
    private func restoreIndex() {
        let path = SearchEngine.defaultSnapshotPath

        guard SearchEngine.snapshotExists(at: path) else { return }

        if engine.loadSnapshot(at: path) {
            indexedCount = engine.count
            status = "Restored \(indexedCount) files"
        }
    }

    /// Builds the index in the background.
    private func startIndexing() {
        isIndexing = true
        status = "Indexing…"

        let engine = self.engine

        indexingTask = Task.detached(priority: .utility) { [weak self] in
            _ = engine.startScan()

            // The scan runs on its own thread inside the Rust core, so this
            // only has to wait for it.
            while !engine.scanFinished {
                if Task.isCancelled { return }
                try? await Task.sleep(for: .milliseconds(50))
            }

            let count = engine.finishScan()

            // The snapshot is written in the background so that the next
            // launch starts with a warm index.
            let path = SearchEngine.defaultSnapshotPath
            Task.detached(priority: .utility) {
                engine.saveSnapshot(at: path)
            }

            await self?.finishIndexing(count: count)
        }

        startWatching()
    }

    /// Starts the filesystem watcher and keeps the index up to date.
    ///
    /// The watcher reports the changes to the indexed roots, which are
    /// applied to the index so that a file created after the launch shows
    /// up without a restart.
    private func startWatching() {
        let engine = self.engine

        guard engine.startWatching() else { return }

        watchingTask = Task.detached(priority: .background) { [weak self] in
            while !Task.isCancelled {
                let changes = await Task.detached(priority: .background) {
                    engine.pollChanges()
                }.value

                if !changes.isEmpty {
                    let count = engine.applyChanges(changes)
                    await self?.indexChanged(count: count)
                }

                // The watcher batches its events, so there is no point in
                // asking more often than this.
                try? await Task.sleep(for: .milliseconds(400))
            }
        }
    }

    /// Publishes an index update that came from the watcher.
    private func indexChanged(count: Int) {
        indexedCount = count
        updateStatus()

        // The results are recomputed, because the new entries may match the
        // current query.
        scheduleSearch()
    }

    /// Publishes the result of the scan.
    private func finishIndexing(count: Int) {
        indexedCount = count
        isIndexing = false
        status = "\(count) files indexed"

        // The results are recomputed now that the full index is available.
        scheduleSearch()
    }

    // MARK: - Searching

    /// Schedules a search for the current query.
    private func scheduleSearch() {
        pendingSearch?.cancel()

        let query = self.query
        let generation = self.generation &+ 1
        self.generation = generation

        let engine = self.engine

        // The search itself is fast, but it is scheduled with a tiny delay so
        // that fast typing does not run one search per keystroke.
        pendingSearch = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(12))
            if Task.isCancelled { return }

            let hits = await Task.detached(priority: .userInitiated) {
                engine.search(query)
            }.value

            if Task.isCancelled { return }

            self?.publish(hits: hits, generation: generation)
        }
    }

    /// Publishes the results of a search.
    private func publish(hits: [SearchHit], generation: UInt64) {
        // A newer query has been issued in the meantime.
        guard generation == self.generation else { return }

        results = hits
        selection = min(selection, max(0, hits.count - 1))
        updateStatus()
    }

    /// Updates the status line.
    private func updateStatus() {
        if isIndexing {
            status = "Indexing…"
        } else if query.isEmpty {
            status = "\(indexedCount) files"
        } else if results.isEmpty {
            status = "No matches"
        } else {
            status = "\(results.count) of \(indexedCount)"
        }
    }

    // MARK: - Actions

    /// The result that is currently selected.
    var selectedHit: SearchHit? {
        guard selection >= 0, selection < results.count else { return nil }
        return results[selection]
    }

    /// Moves the selection by `delta`, clamping it to the result list.
    func moveSelection(by delta: Int) {
        guard !results.isEmpty else {
            selection = 0
            return
        }
        selection = max(0, min(results.count - 1, selection + delta))
    }

    /// Moves the selection to the top.
    func selectFirst() {
        selection = 0
    }

    /// Moves the selection to the bottom.
    func selectLast() {
        selection = max(0, results.count - 1)
    }

    /// Opens the selected result.
    func openSelection() {
        guard let hit = selectedHit else { return }
        open(hit)
    }

    /// Opens a result in the default application.
    func open(_ hit: SearchHit) {
        NSWorkspace.shared.open(URL(fileURLWithPath: hit.path))
    }

    /// Reveals a result in Finder.
    func reveal(_ hit: SearchHit) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: hit.path)])
    }

    /// Copies the path of a result to the pasteboard.
    func copyPath(_ hit: SearchHit) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(hit.path, forType: .string)
    }
}
