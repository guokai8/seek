//
//  AppModel.swift
//  Seek
//
//  The state of the launcher: the query, the results, and the lifecycle of
//  the background index.
//
//  The searches and the scan run off the main actor so that the panel stays
//  responsive while a quarter of a million files are being matched.
//

import Combine
import Foundation
import SwiftUI

/// The observable state of the launcher.
@MainActor
final class AppModel: ObservableObject {
    /// The text the user has typed.
    @Published var query: String = "" {
        didSet {
            if query != oldValue {
                // The row being renamed is about to be replaced by the
                // next query's rows, and a field left open over a row
                // that no longer means the same file would rename the
                // wrong one.
                cancelRename()
                scheduleSearch()
            }
        }
    }

    /// The current results, ranked.
    @Published private(set) var results: [SearchHit] = []

    /// How many rows the current query has been asked for.
    ///
    /// A keystroke asks for one page. Reaching the end of the list asks
    /// for the next one, so that the whole of a broad query's matches can
    /// be walked without every keystroke paying for them: ranking and
    /// resolving all 166,000 matches of `src` takes over five seconds,
    /// while the first page takes 33ms.
    private var pageLimit = AppModel.firstPage

    /// Whether a page is being fetched, so that scrolling cannot pile up
    /// requests for the same rows.
    private var loadingMore = false

    /// The rows a keystroke asks for.
    ///
    /// Far more than the dozen the panel can show, so that scrolling is
    /// smooth before any of them has to be fetched, and small enough that
    /// the ranking can be cut down to it.
    static let firstPage = 200

    /// How many candidates a search scores between pages.
    ///
    /// This is the only knob on streaming, and it is a trade: the pages
    /// are what make the first rows appear early, and each one costs a
    /// resolution of its rows plus a pass over the main actor.
    ///
    /// The chunk counts candidates, not matches, so the number of pages
    /// follows the size of the corpus rather than the breadth of the
    /// query: over a quarter of a million entries every query gets four,
    /// the first of them a couple of milliseconds in. For a narrow query
    /// that is a few pages saying the same thing, which costs a fraction
    /// of a millisecond — less than the bookkeeping to notice and skip
    /// them would.
    ///
    /// It is measured against the keystroke budget rather than guessed: at
    /// 40,000 the six pages of a broad query spent 7ms of the 8ms the
    /// shell is allowed on top of the engine, and at 64,000 they spend 6.
    /// In release the core's own cost for the chunking is under 1.5ms.
    static let searchChunk = 64_000

    /// How many entries matched the current query, before the limit.
    ///
    /// The results are capped, so `results.count` saturates at the limit
    /// and stops moving while the query keeps changing. This is the number
    /// that tells the user their edit took effect.
    @Published private(set) var matchedCount: Int = 0

    /// Whether the last scan walked everything it was asked to.
    ///
    /// False when the scan hit its deadline, which leaves the index
    /// partial. The footer says so rather than reporting a partial index
    /// as a complete one.
    @Published private(set) var scanComplete: Bool = true

    /// The index of the selected result.
    @Published var selection: Int = 0

    /// Whether the index is still being built.
    @Published private(set) var isIndexing: Bool = false

    /// The number of entries in the index.
    @Published private(set) var indexedCount: Int = 0

    /// Why the current query does not parse, when it does not.
    ///
    /// It is held beside the results rather than derived from them,
    /// because an empty list cannot say why it is empty.
    @Published private(set) var queryError: String?

    /// A short status message shown in the footer.
    @Published private(set) var status: String = ""

    /// The entry id of the row whose name is being edited, if any.
    ///
    /// By id rather than by index: the list is rebuilt under the user on
    /// every keystroke and after every watcher batch, and an index would
    /// quietly come to mean a different file.
    @Published private(set) var renamingID: UInt32?

    /// Whether a row is being renamed.
    var isRenaming: Bool { renamingID != nil }

    /// The engine that owns the Rust-side index.
    ///
    /// The engine is thread-safe, so it is not isolated to the main actor.
    /// The scan and the searches run on background tasks.
    nonisolated let engine = SearchEngine()

    /// The task that runs the pending search.
    ///
    /// The search that is already running is not cancelled. A task that is
    /// released while it is running is cancelled by the runtime, and a
    /// cancelled task that is waiting for a search to finish throws that
    /// cancellation away — so the results never arrive and the panel stays
    /// empty. The task is therefore kept until it has finished.
    private var pendingSearch: Task<Void, Never>?

    /// The searches that are running.
    ///
    /// A search that is running is kept here until it has finished, so
    /// that it is not cancelled half way through. The results of a search
    /// that has become stale are dropped by the generation counter.
    private var runningSearches: [Task<Void, Never>] = []

    /// The task that builds the index.
    private var indexingTask: Task<Void, Never>?

    /// The task that applies the changes the filesystem watcher reports.
    private var watchingTask: Task<Void, Never>?

    /// The generation counter, used to discard results that arrive after a
    /// newer query has been issued.
    private(set) var generation: UInt64 = 0

    /// The newest streaming page of the current query that has been shown.
    private var lastPage = -1

    /// Whether the current query's whole answer has been shown.
    private var isComplete = false

    /// The newest query's generation, readable off the main actor.
    ///
    /// A streaming search's callback runs on a background thread and has
    /// to decide, between chunks, whether the query it is answering is
    /// still the one in the field. It is synchronous, so it cannot hop to
    /// the main actor to ask — the counter is mirrored here instead.
    private let liveGeneration = GenerationBox()

    /// Whether a search is still running.
    ///
    /// A streaming search hands over several pages before it is done, so
    /// "there are rows on screen" no longer means "the search has
    /// finished". Anything that waits for a settled list has to be able to
    /// ask, and the panel can say so.
    @Published private(set) var isSearching = false

    /// Whether the preview column is shown beside the results.
    ///
    /// The setting outlives the panel — see `Preferences.showPreview` —
    /// so it is read once here and written back whenever it is toggled.
    @Published private(set) var isPreviewShown: Bool = Preferences.shared.showPreview

    /// Whether a bare term is an exact substring rather than a fuzzy
    /// subsequence.
    ///
    /// The setting outlives the panel — see `Preferences.exactMatch` — so
    /// it is read once here and written back whenever it is switched.
    @Published private(set) var isExactMatching: Bool = Preferences.shared.exactMatch

    /// Whether a result row says when the entry last changed.
    ///
    /// Mirrored here for the same reason `isPreviewShown` is: the settings
    /// window opens over the panel, so a checkbox that only took effect
    /// the next time the panel was summoned would read as one that did
    /// nothing.
    @Published private(set) var showsModified: Bool = Preferences.shared.showModified

    /// Whether a result row says how big the entry is.
    @Published private(set) var showsSize: Bool = Preferences.shared.showSize

    /// How opaque the panel is drawn, already clamped.
    ///
    /// The app delegate owns the window and watches this, so that moving
    /// the slider shows its effect on the panel standing behind the
    /// settings window rather than the next time the panel is summoned.
    @Published private(set) var panelOpacity: Double = Preferences.shared.effectiveOpacity

    /// Whether the preview column is drawn.
    ///
    /// Asking for the preview is not the same as having something to
    /// preview. With no selected row the column is left out altogether,
    /// so an empty query leaves the panel exactly the shape it has
    /// without the preview rather than putting an empty box beside an
    /// empty list.
    var isPreviewVisible: Bool { isPreviewShown && selectedHit != nil }

    /// Whether the panel is on screen.
    ///
    /// The app delegate owns the panel and says when it comes and goes.
    /// The model needs to know because the watcher's refreshes are only
    /// worth doing for a panel that somebody is looking at.
    var isPanelVisible = false

    /// The subscriptions the model holds.
    ///
    /// Combine rather than a notification observer, because an observer
    /// would have to be taken off the centre in `deinit`, which cannot
    /// reach the model's own properties. A cancellable cancels itself
    /// when the set is released.
    private var subscriptions: Set<AnyCancellable> = []

    init() {
        configureRoots()
        restoreIndex()
        startIndexing()
        observeSettings()
    }

    /// Follows the settings that the panel shows while it is open.
    ///
    /// The settings window is opened from the panel's own gear button and
    /// the panel stays on screen behind it, so a checkbox that only took
    /// effect the next time the panel was summoned would read as a
    /// checkbox that did nothing.
    ///
    /// `@AppStorage` is a property wrapper rather than a published
    /// property, so there is nothing on `Preferences` to subscribe to; the
    /// defaults themselves say when they change.
    private func observeSettings() {
        NotificationCenter.default
            .publisher(for: UserDefaults.didChangeNotification)
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self else { return }

                    let shown = Preferences.shared.showPreview

                    if shown != self.isPreviewShown {
                        self.isPreviewShown = shown
                    }

                    let modified = Preferences.shared.showModified

                    if modified != self.showsModified {
                        self.showsModified = modified
                    }

                    let size = Preferences.shared.showSize

                    if size != self.showsSize {
                        self.showsSize = size
                    }

                    let opacity = Preferences.shared.effectiveOpacity

                    if opacity != self.panelOpacity {
                        self.panelOpacity = opacity
                    }

                    // Both ways in lead here: the checkbox writes the
                    // default, ⌘E writes it too, and whichever it was the
                    // engine and the list are brought up to date in one
                    // place. The guard inside makes the second arrival a
                    // no-op, so the query is not searched twice.
                    self.setExactMatching(Preferences.shared.exactMatch)
                }
            }
            .store(in: &subscriptions)
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
        engine.exactMatch = preferences.exactMatch
        engine.normalize = preferences.normalize
        engine.scheme = preferences.scheme
    }

    // MARK: - Indexing

    /// Loads the index snapshot from the last run, if there is one.
    private func restoreIndex() {
        let path = SearchEngine.defaultSnapshotPath

        guard SearchEngine.snapshotExists(at: path) else { return }

        if engine.loadSnapshot(at: path) {
            indexedCount = engine.count

            // The snapshot records whether the scan behind it finished, and
            // the default for this is the optimistic one. Without reading it
            // back, an index restored from a scan that ran out of its
            // deadline would lose its "scan cut short" the moment it came
            // off disk, and the footer would report half a tree as a whole
            // one until the next scan happened to get all the way through.
            scanComplete = engine.scanComplete

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
            let complete = engine.scanComplete
            NSLog("seek: indexed \(count) entries, complete=\(complete)")

            // The snapshot is written in the background so that the next
            // launch starts with a warm index.
            let path = SearchEngine.defaultSnapshotPath
            Task.detached(priority: .utility) {
                engine.saveSnapshot(at: path)
            }

            await self?.finishIndexing(count: count, complete: complete)
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
    ///
    /// Not private: the tests drive it, because waiting for the watcher to
    /// report a real change is not something a test can rely on.
    func indexChanged(count: Int) {
        indexedCount = count
        updateStatus()

        // The results are refreshed, because the new entries may match the
        // current query. The refresh does not cancel a search that the
        // user has asked for.
        refreshResults()
    }

    /// Publishes the result of the scan.
    ///
    /// Not private: the tests drive it, because a scan that is cut short by
    /// its deadline is not something a test can bring about by waiting.
    func finishIndexing(count: Int, complete: Bool) {
        indexedCount = count
        isIndexing = false
        scanComplete = complete
        status = indexedSummary

        // The results are recomputed now that the full index is available.
        // The index has only just been built, so there is no search of the
        // user's to preserve, and the search is scheduled the ordinary
        // way.
        scheduleSearch()
    }

    // MARK: - Searching

    /// Schedules a search for the current query.
    ///
    /// The search is scheduled with a tiny delay so that fast typing does
    /// not run one search per keystroke.
    ///
    /// The search that is already running is *not* cancelled. Cancelling a
    /// task that is waiting for a search to finish makes the wait throw,
    /// and the results are then thrown away with it — which leaves the
    /// panel showing nothing. The results of a search that has become
    /// stale are dropped by the generation counter instead, which is
    /// cheaper than running the search again.
    private func scheduleSearch() {
        let query = self.query
        let generation = self.generation &+ 1
        self.generation = generation
        liveGeneration.set(generation)

        // A new query is a new list, so it starts at the first page again.
        pageLimit = Self.firstPage
        lastPage = -1
        isComplete = false
        loadingMore = false

        // The selection goes back to the top here, when the query changes,
        // rather than when its first results arrive. A streaming search
        // hands over several pages and the user may arrow down while they
        // are still coming, so a reset that waited for a page to land
        // would either undo that or race with it, depending on which task
        // the main actor happened to run first.
        selection = 0

        // An empty query is not a search. The engine answers it the way fzf
        // does — an empty pattern matches everything — so asking it would
        // rank a quarter of a million paths by length and hand back the
        // shortest ones, which are top-level directories the user never
        // asked about. The panel shows its hint instead, and the scan does
        // not run at all.
        guard !query.isEmpty else {
            queryError = nil
            isSearching = false
            publish(SearchOutcome(hits: [], matched: 0), generation: generation, resetSelection: true)
            return
        }

        // A malformed pattern is settled before the search runs, so the
        // panel can say what is wrong rather than show an empty list and
        // leave the user to guess.
        // Assigning a `@Published` property publishes whether or not the
        // value changed, so setting this to nil on every keystroke of a
        // perfectly good query cost a SwiftUI pass of its own, before the
        // search had even started.
        let error = SearchEngine.queryError(query)
        if error != queryError {
            queryError = error
        }
        if queryError != nil {
            isSearching = false
            publish(SearchOutcome(hits: [], matched: 0), generation: generation, resetSelection: true)
            return
        }

        let engine = self.engine

        isSearching = true

        let chunk = Self.searchChunk
        let live = liveGeneration

        pendingSearch = Task { [weak self] in
            // This task used to open by sleeping 12ms, to let a faster
            // typist supersede a search before it started. But the hop
            // into this task is already such a point, and the generation
            // says whether it happened — so the check below buys the same
            // coalescing, while the sleep was charging every keystroke
            // 12ms whether or not anything superseded it. Against a search
            // that costs ~25ms over a quarter of a million paths, that was
            // a third of the latency spent waiting on purpose.
            guard let self, self.generation == generation else { return }

            let page = Self.firstPage
            let outcome = await Task.detached(priority: .userInitiated) {
                var index = 0

                return engine.search(query, limit: page, chunk: chunk) { partial in
                    // A newer keystroke has been typed. The rest of this
                    // search would be spent on an answer that is already
                    // stale, so it is abandoned here instead — which is
                    // most of what streaming buys a fast typist.
                    guard live.current == generation else { return false }

                    let number = index
                    index += 1

                    Task { @MainActor [weak self] in
                        self?.publishPage(
                            partial.hits,
                            matched: partial.matched,
                            page: .partial(number),
                            from: generation
                        )
                    }

                    return true
                }
            }.value

            // The results are only shown when no newer query has been
            // issued in the meantime.
            guard self.generation == generation else { return }

            self.publishPage(outcome.hits, matched: outcome.matched, page: .complete, from: generation)
        }

        // The task is kept until it has finished. A task that is released
        // while it is running is cancelled by the runtime, and a cancelled
        // task throws away the results it was waiting for.
        keep(pendingSearch!)
    }

    /// Fetches the next page of results.
    ///
    /// Called when the user reaches the end of the list, by scrolling or
    /// by holding the down arrow. The ranking is a total order, so the
    /// longer list begins with exactly the rows the shorter one had: the
    /// rows already on screen do not move, they are only followed by more.
    /// Whether drawing row `index` should ask for the next page.
    ///
    /// The margin is a screenful or so of rows, so the next page is on its
    /// way before the user reaches the end rather than after.
    ///
    /// Only once the query's own answer is in. A streaming search hands
    /// over its chunks one at a time, and an early chunk is short — so a
    /// row a dozen down a 32-row chunk sits "within twenty of the end" of
    /// a list that is still being filled. The number it would be weighed
    /// against makes it worse: `matchedCount` is written only when a
    /// search completes, so mid-stream it is the *previous* query's total,
    /// which all but guarantees it says there is more to fetch.
    ///
    /// Together they doubled the result set on a panel nobody had
    /// touched: `nihao` sat on its first row reporting
    /// `174,504 matches · first 400`.
    func shouldLoadMore(forRow index: Int) -> Bool {
        isComplete && index >= results.count - Self.pageMargin
    }

    /// How close to the end of the list counts as the end.
    static let pageMargin = 20

    func loadMoreResults() {
        // `isComplete` is here as well as in `shouldLoadMore`, because
        // `selectLast` asks for the next page without going through it.
        // Until the search finishes, `matchedCount` belongs to the
        // previous query and the comparison below is meaningless.
        guard isComplete,
              !loadingMore,
              !query.isEmpty,
              queryError == nil,
              results.count < matchedCount
        else { return }

        loadingMore = true

        // The page doubles, so walking a long list takes a handful of
        // fetches rather than hundreds. Each one costs roughly what it
        // returns — a thousand rows in 50ms, five thousand in 190ms — and
        // it is paid when the user scrolls, never on a keystroke.
        let next = min(max(pageLimit * 2, Self.firstPage), matchedCount)
        let query = self.query
        let generation = self.generation
        let engine = self.engine

        let task = Task { [weak self] in
            let outcome = await Task.detached(priority: .userInitiated) {
                engine.search(query, limit: next)
            }.value

            guard let self, self.generation == generation else {
                self?.loadingMore = false
                return
            }

            self.pageLimit = next
            self.loadingMore = false

            // The selection stays where the user left it: they are reading
            // the list, not starting a new search.
            self.publishPage(outcome.hits, matched: outcome.matched, page: .complete, from: generation)
        }

        keep(task)
    }

    /// Keeps a task alive until it has finished.
    ///
    /// A task that nothing holds on to is cancelled when it is released,
    /// and a task that is cancelled while it is waiting for a search throws
    /// the search away. The finished task removes itself.
    private func keep(_ task: Task<Void, Never>) {
        runningSearches.append(task)

        Task { [weak self] in
            await task.value

            // The task is dropped once it has finished, so that the list
            // does not grow for the lifetime of the app.
            guard let self else { return }
            self.runningSearches.removeAll { $0 == task }
        }
    }

    /// Refreshes the results after the index has changed.
    ///
    /// The watcher reports the changes to the indexed folders every few
    /// hundred milliseconds, and each report refreshes the results so that
    /// a file created a moment ago shows up.
    ///
    /// The refresh does *not* cancel a search that the user asked for. The
    /// watcher runs all the time — it reports the app's own snapshot
    /// writes, the temporary files of every other program — so cancelling
    /// on every report means the user's search is cancelled before it
    /// ever finishes, and the panel never shows anything.
    private func refreshResults() {
        let query = self.query
        let generation = self.generation

        // There is nothing to refresh while the query is empty: an empty
        // query has no results by design. See `scheduleSearch`.
        guard !query.isEmpty else { return }

        // Nor is there anything to refresh into a panel that is put away.
        // The watcher reports changes for as long as the app runs, and
        // every report used to search the whole corpus and republish the
        // list with nobody looking at it. The panel clears its query when
        // it opens, so there is no stale list to catch up on either.
        guard isPanelVisible else { return }

        let engine = self.engine
        let limit = pageLimit

        let task = Task { [weak self] in
            let outcome = await Task.detached(priority: .utility) {
                engine.search(query, limit: limit)
            }.value

            // The results are only shown when no newer search has been
            // started in the meantime.
            guard let self, self.generation == generation else { return }

            // The watcher found a file; the user did not move. Keeping the
            // selection is what the comment above has always said this
            // should do.
            //
            // This is a complete answer, so it closes the query to any
            // streaming page still in flight: one landing after it would
            // be a list without the file the watcher just found.
            self.publishPage(outcome.hits, matched: outcome.matched, page: .complete, from: generation)
        }

        keep(task)
    }

    /// Publishes the results of a search.
    ///
    /// The selection goes back to the top when the query has changed, so
    /// that the best match is always the one that is under the cursor. The
    /// selection is kept when the results change for another reason, such
    /// as the watcher adding a file.
    private func publish(
        _ outcome: SearchOutcome,
        generation: UInt64,
        resetSelection: Bool,
        countIsFinal: Bool = true
    ) {
        // A newer query has been issued in the meantime.
        guard generation == self.generation else { return }

        let hits = outcome.hits

        // Whether the selection goes back to the top is something the
        // caller knows and this function cannot work out cheaply: it used
        // to compare the new results against the old, which is a deep
        // comparison of two hundred hits holding four strings each, run on
        // the main actor on every keystroke. It also disagreed with its
        // own documentation, resetting the selection whenever the watcher
        // happened to change a row.
        if resetSelection {
            selection = 0
        }

        // A streaming search replaces the page as better matches turn up,
        // so the row the user is reading can slide down the list. It is
        // followed by id rather than by position, or the highlight would
        // move to a different file while they look at it.
        //
        // Only when they have moved off the top, though. A selection still
        // on the first row means they are reading the best match, which is
        // whatever the latest page puts first — not the row that happened
        // to be there when the search was a tenth of the way through.
        let anchor = (selection > 0 && results.indices.contains(selection))
            ? results[selection].id
            : nil

        results = hits

        // A partial page's count is how many candidates have matched *so
        // far*, which is not an answer to "how many matched". Publishing
        // it would make the counter climb through every search, and it
        // would break what the count is relied on for: narrowing a query
        // can only ever match fewer files, so a count that rose as the
        // query grew reads as a search that was never redone.
        // `loadMoreResults` branches on it too.
        if countIsFinal {
            matchedCount = outcome.matched
        }

        if let anchor, let moved = hits.firstIndex(where: { $0.id == anchor }) {
            selection = moved
        } else {
            selection = min(selection, max(0, hits.count - 1))
        }

        updateStatus()

        // The results are logged, because a launcher that finds nothing is
        // indistinguishable from one that is broken. The log is what tells
        // the two apart.
        //
        // Only the answer, though, not the pages on the way to it: a
        // streaming search publishes several times per keystroke, and
        // NSLog goes to the unified log, which is not something to do six
        // times while the user is typing.
        guard countIsFinal else { return }

        NSLog(
            "seek: query=\"\(query)\" results=\(hits.count) indexed=\(indexedCount) status=\"\(status)\""
        )
    }

    /// Which page of a query's answer a publish carries.
    enum Page {
        /// Page `n` of a streaming search, counting from zero.
        ///
        /// The pages reach the main actor from a background thread as one
        /// unstructured task each, and nothing promises they arrive in the
        /// order they were sent. A page older than one already shown holds
        /// worse rows — the ranking only improves as the chunks go by — so
        /// it is dropped rather than put back on screen.
        case partial(Int)

        /// The whole answer to the query.
        ///
        /// A finished search, a longer page the user scrolled to, or a
        /// refresh after the index changed. It always wins, and nothing
        /// partial may follow it: a page still in flight when the search
        /// finished would otherwise shrink the list back to a prefix of
        /// itself.
        case complete
    }

    /// Installs one page of an answer.
    ///
    /// Not private: the streaming search calls it, and the tests drive it,
    /// because reaching it through a real search would mean waiting on a
    /// scan of the whole home directory to decide where a highlight goes.
    func publishPage(
        _ hits: [SearchHit],
        matched: Int,
        page: Page,
        from generation: UInt64
    ) {
        // The page has to belong to the query that is on screen. A search
        // is abandoned at its next chunk boundary, so a page it had
        // already handed to the main actor can arrive after a newer query
        // was typed — and putting it up would show the rows of a query
        // the user has already replaced. Dropping it here rather than
        // inside `publish` is deliberate: the bookkeeping below must not
        // move either, or the newer search's own first page would look
        // older than something already shown and be dropped in its turn.
        guard generation == self.generation else { return }

        switch page {
        case .partial(let number):
            guard !isComplete, number > lastPage else { return }
            lastPage = number
        case .complete:
            isComplete = true
            isSearching = false
        }

        publish(
            SearchOutcome(hits: hits, matched: matched),
            generation: generation,
            resetSelection: false,
            countIsFinal: isComplete
        )
    }

    /// Installs a fixed set of results.
    ///
    /// This exists for the tests, which need results in the list without
    /// waiting for a scan.
    func setTestResults(_ hits: [SearchHit]) {
        results = hits
        selection = 0
        indexedCount = hits.count
        updateStatus()
    }

    /// Resets the panel for a new search.
    ///
    /// The query is cleared and the selection goes back to the top, so that
    /// the panel always opens in the same state and the first keystroke
    /// starts a fresh search.
    func prepareForOpen() {
        query = ""
        selection = 0
        results = []
        updateStatus()
    }

    /// Asks the panel to put the focus in the search field.
    ///
    /// The focus cannot be set when the panel is off screen: a field in a
    /// window that is not the key window cannot become the first
    /// responder. The request is therefore deferred until the panel is on
    /// screen and is the key window.
    func requestSearchFocus() {
        NotificationCenter.default.post(name: .seekFocusSearch, object: nil)
    }

    /// Switches between exact and fuzzy matching, and answers the query
    /// on screen again under the new rule.
    ///
    /// The recompute is the point. The text in the field means something
    /// else after the switch — `nihao` is five letters in a row or five
    /// letters in order — so a list left as it was is the answer to a
    /// question nobody is asking any more.
    func setExactMatching(_ exact: Bool) {
        guard exact != isExactMatching else { return }

        isExactMatching = exact
        Preferences.shared.exactMatch = exact
        engine.exactMatch = exact

        scheduleSearch()
    }

    /// Switches to the other matching mode.
    func toggleExactMatching() {
        setExactMatching(!isExactMatching)
    }

    /// Shows or hides the preview column.
    func togglePreview() {
        isPreviewShown.toggle()
        Preferences.shared.showPreview = isPreviewShown
    }

    /// Handles a key press that the panel intercepts.
    ///
    /// Returns whether the key was consumed. A key that is not one of the
    /// panel's own is left alone, so that it reaches the search field.
    @discardableResult
    func handleKeyEvent(_ event: NSEvent) -> Bool {
        guard let action = PanelKey.action(for: event) else { return false }

        switch action {
        case .moveUp:
            moveSelection(by: -1)
        case .moveDown:
            moveSelection(by: 1)
        case .first:
            selectFirst()
        case .last:
            selectLast()
        case .open:
            openSelection()
        case .reveal:
            if let hit = selectedHit {
                reveal(hit)
            }
        case .copyPath:
            if let hit = selectedHit {
                copyPath(hit)
            }
        case .previewToggle:
            togglePreview()
        case .exactToggle:
            toggleExactMatching()
        case .settings:
            NotificationCenter.default.post(name: .seekShowSettings, object: nil)
        case .quit:
            NSApp.terminate(nil)
        case .selectAll, .cut, .copy, .paste:
            // The editing commands act on the search field rather than on
            // the model, so they are left to the panel. See
            // `AppDelegate.performEditingAction`.
            return false
        }

        return true
    }

    /// Updates the status line.
    /// How the footer reports the size of the index.
    ///
    /// A scan that was cut short by its deadline says so, because the
    /// number alone would read as the whole of the disk.
    private var indexedSummary: String {
        let count = "\(indexedCount.formatted()) files indexed"
        return scanComplete ? count : "\(count) · scan cut short"
    }

    /// How the index is doing, in one line.
    ///
    /// The footer's status line is about the query — how many files
    /// matched — and says nothing about the index once there is a query to
    /// report on. The menu bar has no query, so it reports the index
    /// itself: how many files are in it, or that it is still being built.
    var indexStatus: String {
        isIndexing ? "Indexing…" : indexedSummary
    }

    private func updateStatus() {
        if isIndexing {
            status = "Indexing…"
        } else if queryError != nil {
            // The body carries the reason. The footer says nothing, rather
            // than reporting how much was searched when nothing was.
            status = ""
        } else if results.isEmpty {
            // The body of the panel already says that nothing matched, and
            // in the same words. The footer reports what it alone knows
            // instead: how much was searched to find nothing.
            status = indexedSummary
        } else if matchedCount < results.count {
            // A streaming search has put rows on screen before the match
            // count is known: until the search finishes, the count is the
            // previous query's. Reporting it would read as a total smaller
            // than the list under it. The rows are what the user is
            // looking at, so the footer reports those and does not claim a
            // total it does not have.
            //
            // The test is `matchedCount < results.count` rather than a
            // flag because a real count can never be smaller than the rows
            // it was cut down to: being behind is the only way to get here.
            status = "\(results.count.formatted()) so far"
        } else {
            // The match count, not the row count. The rows stop at the
            // limit, so a footer built from them reads "200 of 258,328"
            // for every broad query and never changes again — which is
            // what made an edited query look like a frozen panel, and it
            // was not even true: those 200 rows were the best of 1,281
            // matches, not the whole of them. This is the number fzf's
            // counter shows, and it moves on every keystroke.
            let matches = matchedCount == 1
                ? "1 match"
                : "\(matchedCount.formatted()) matches"

            // "showing N" is now how far down the list has been fetched,
            // not a ceiling: scrolling to the end asks for more. Saying
            // "first N" rather than "showing N" is the difference between
            // a limit and a position.
            status = matchedCount > results.count
                ? "\(matches) · first \(results.count.formatted())"
                : matches
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

        // Someone holding the down arrow is walking towards the end of
        // the page, and the next one has to be on its way before they get
        // there or the selection stops dead at a row that is not the last
        // match. The margin is a screenful or so of rows.
        if shouldLoadMore(forRow: selection) {
            loadMoreResults()
        }
    }


    /// Moves the selection to the top.
    func selectFirst() {
        selection = 0
    }

    /// Moves the selection to the bottom of what has been fetched.
    ///
    /// Not to the last match: a broad query has hundreds of thousands of
    /// them, and ranking and resolving all of them to answer one key
    /// press would take seconds. The next page is requested instead, so
    /// the key can be pressed again to keep going down.
    func selectLast() {
        selection = max(0, results.count - 1)
        loadMoreResults()
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

    // MARK: - Renaming a row

    /// What the panel should do with a key while a row is being renamed.
    enum RenameKey {
        /// No rename is in progress; the panel handles the key as usual.
        case notRenaming
        /// The key abandoned the rename, and nothing else should happen.
        case cancelled
        /// The key belongs to the rename field; the panel must not take it.
        case passThrough
    }

    /// Decides a key press while a row is being renamed.
    ///
    /// The panel's local monitor sees every key before the views do, and
    /// it maps Return to "open the selected file" and Escape to "hide the
    /// panel". Over a rename field both are wrong, so the monitor asks
    /// here first and stands aside for everything but Escape.
    func keyWhileRenaming(_ event: NSEvent) -> RenameKey {
        guard isRenaming else { return .notRenaming }

        if event.keyCode == 53 { // Escape
            cancelRename()
            return .cancelled
        }

        return .passThrough
    }

    /// Puts a row into its editing state.
    func beginRename(_ hit: SearchHit) {
        renamingID = hit.id
    }

    /// Leaves the editing state without renaming anything.
    func cancelRename() {
        renamingID = nil
    }

    /// Renames the row and leaves the editing state either way.
    ///
    /// The field closes on a refusal as well as on a success. The reason
    /// goes to the footer, and a field that stayed open would be a second
    /// place saying the same thing.
    func commitRename(_ hit: SearchHit, to name: String) {
        renamingID = nil
        rename(hit, to: name)
    }

    // MARK: - Writing to the filesystem

    /// Moves a result to the Trash.
    ///
    /// The Trash, not `removeItem`. This is a launcher that is opened a
    /// hundred times a day and the row under the pointer is whichever one
    /// the last keystroke ranked first, so the destructive action has to
    /// be the one the user can take back.
    ///
    /// The row goes away on its own: the watcher sees the file leave and
    /// the index drops it. Taking it out of `results` here as well would
    /// mean two sources deciding what the list holds.
    @discardableResult
    func trash(_ hit: SearchHit) -> Bool {
        do {
            try FileManager.default.trashItem(
                at: URL(fileURLWithPath: hit.path),
                resultingItemURL: nil
            )
            status = "Moved \(hit.name) to the Trash"
            return true
        } catch {
            status = "Could not move \(hit.name) to the Trash"
            NSLog("seek: could not trash \(hit.path): \(error)")
            return false
        }
    }

    /// Renames a result, in the folder it is already in.
    ///
    /// `name` is a name and never a path. `moveItem` would just as
    /// happily move the file somewhere else, so a name carrying a
    /// separator -- or `..` -- would turn renaming into moving, and the
    /// panel does not offer moving. Those are refused rather than
    /// sanitised: silently renaming `../x` to `x` is a different
    /// operation from the one that was asked for.
    ///
    /// Returns false and says why in the status when the name cannot be
    /// used. The file is left alone in every one of those cases.
    @discardableResult
    func rename(_ hit: SearchHit, to name: String) -> Bool {
        let wanted = name.trimmingCharacters(in: .whitespacesAndNewlines)

        guard !wanted.isEmpty else {
            status = "A file needs a name"
            return false
        }
        guard !wanted.contains("/"), wanted != ".", wanted != ".." else {
            status = "A name cannot contain a slash"
            return false
        }
        guard wanted != hit.name else {
            // Not a failure the user needs telling about -- they typed
            // the name it already had -- but not a rename either.
            return false
        }

        let source = URL(fileURLWithPath: hit.path)
        let target = source.deletingLastPathComponent().appendingPathComponent(wanted)

        // Checked rather than left to `moveItem`, which on a
        // case-insensitive volume would also refuse a rename that only
        // changes case -- the one rename most worth allowing.
        if FileManager.default.fileExists(atPath: target.path),
            target.path.compare(source.path, options: .caseInsensitive) != .orderedSame
        {
            status = "There is already something called \(wanted)"
            return false
        }

        do {
            try FileManager.default.moveItem(at: source, to: target)
            status = "Renamed to \(wanted)"
            return true
        } catch {
            status = "Could not rename \(hit.name)"
            NSLog("seek: could not rename \(hit.path): \(error)")
            return false
        }
    }
}

/// A counter the main actor writes and a background thread reads.
///
/// A streaming search's callback is synchronous and runs off the main
/// actor, so it cannot await the model to find out whether the query it is
/// answering is still the current one. The lock is uncontended in practice
/// — one write per keystroke, one read per chunk — and an `NSLock` around
/// a `UInt64` is cheaper to read than it is to reason about an atomic.
final class GenerationBox: @unchecked Sendable {
    private let lock = NSLock()
    private var value: UInt64 = 0

    /// The newest generation.
    var current: UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    /// Records a new generation.
    func set(_ generation: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        value = generation
    }
}
