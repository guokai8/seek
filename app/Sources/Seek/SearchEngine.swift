//
//  SearchEngine.swift
//  Seek
//
//  A Swift wrapper around the Rust search core. The Rust side owns the index
//  and the ranking; this class owns the lifetime of the engine handle and
//  translates between Swift values and the C ABI.
//

import Foundation
import SeekCore

/// What a search found.
///
/// The hits are the best [`SearchEngine.limit`] of the matches, not all of
/// them. `matched` is the whole count, and it is the only part of this
/// that is guaranteed to change when the query changes: a broad query has
/// thousands of matches, and taking a character off the end usually leaves
/// the best two hundred of them exactly as they were.
struct SearchOutcome: Equatable {
    /// The ranked results, capped at the engine's limit.
    let hits: [SearchHit]
    /// How many entries matched in all, before the limit.
    let matched: Int

    /// Whether the matches outgrew the list.
    var isTruncated: Bool { matched > hits.count }
}

/// A single search result.
struct SearchHit: Identifiable, Equatable {
    /// The index entry id, used to resolve the path and the highlight.
    let id: UInt32

    /// The match score. Higher is better.
    let score: Int32

    /// The full path of the entry.
    let path: String

    /// The path of the entry relative to the root it is under.
    ///
    /// This is the path the matcher scored, the way `fd` prints it. It is
    /// what the row shows, because the root's prefix is the same for
    /// every entry under it and carries no information.
    let relativePath: String

    /// The file name of the entry.
    let name: String

    /// The directory containing the entry, relative to the root.
    let directory: String

    /// Whether the entry is a directory.
    let isDirectory: Bool

    /// The byte offsets of the matched characters within the relative path.
    ///
    /// The matcher scores the whole path, the way fzf does, so these
    /// offsets are relative to the path rather than to the file name.
    let positions: [Int]

    /// The id, exposed for `Identifiable`.
    var identity: UInt32 { id }

    /// The matched offsets that fall inside the file name.
    ///
    /// The name is the tail of the path, so the offsets are shifted by the
    /// length of the directory part.
    var namePositions: [Int] {
        let offset = relativePath.utf8.count - name.utf8.count

        return positions.compactMap { position in
            let shifted = position - offset
            return shifted >= 0 ? shifted : nil
        }
    }

    /// The matched offsets that fall inside the directory part.
    var directoryPositions: [Int] {
        let offset = relativePath.utf8.count - name.utf8.count

        return positions.filter { $0 < offset }
    }
}

/// Carries a streaming search's callback across the C boundary.
///
/// C has no way to hold a Swift closure, so the closure is put here and
/// the box travels as the search's context pointer. The box lives only
/// for the duration of the search, which is why it is handed over
/// unretained.
private final class ProgressBox {
    /// The engine, for resolving a page's paths and highlights.
    let engine: SearchEngine

    /// The query, which the highlight offsets are computed against.
    let query: String

    /// What to do with a page, and whether to carry on.
    let report: (SearchOutcome) -> Bool

    init(engine: SearchEngine, query: String, report: @escaping (SearchOutcome) -> Bool) {
        self.engine = engine
        self.query = query
        self.report = report
    }

    /// Hands one page over, and says whether the search should carry on.
    ///
    /// The rows are borrowed for the duration of the call, so they are
    /// copied out before anything else touches them.
    func hand(over rows: UnsafePointer<FlSearchResult>?, count: Int32, matched: Int32) -> Int32 {
        let raw: [FlSearchResult]
        if let rows, count > 0 {
            let wanted = min(Int(count), SearchEngine.partialRows)
            raw = Array(UnsafeBufferPointer(start: rows, count: wanted))
        } else {
            raw = []
        }

        let page = SearchOutcome(
            hits: raw.isEmpty ? [] : engine.hits(from: raw, query: query),
            matched: Int(matched)
        )

        return report(page) ? 1 : 0
    }
}

/// The progress callback, as C sees it.
///
/// It captures nothing, which is what lets it be passed as a plain C
/// function pointer; everything it needs arrives through `context`.
private func streamingProgress(
    rows: UnsafePointer<FlSearchResult>?,
    count: Int32,
    matched: Int32,
    context: UnsafeMutableRawPointer?
) -> Int32 {
    guard let context else { return 1 }

    let box = Unmanaged<ProgressBox>.fromOpaque(context).takeUnretainedValue()

    return box.hand(over: rows, count: count, matched: matched)
}

/// The Rust-backed search engine.
///
/// The engine is thread-safe: the Rust side guards its state with a mutex,
/// so searches can run on a background queue while the UI stays responsive.
/// This makes the class `Sendable`, which lets it be used from the scan
/// queue and the search queue.
final class SearchEngine: @unchecked Sendable {
    /// How many rows the panel can show at once.
    ///
    /// `Theme.maxResultsHeight` over the height of a row. It is a floor
    /// for `partialRows`, not a limit on anything: a page with fewer rows
    /// than this would make the list grow in height as a search went on.
    static let panelRows = 14

    /// How many rows of a partial page are resolved.
    ///
    /// Turning the core's rows into drawable results — the full path, the
    /// relative path, the highlight offsets — is the expensive half of
    /// answering a search, and a provisional page exists to be *looked
    /// at*: nobody reads row 180 of a list that will be replaced in four
    /// milliseconds. Over a home directory, resolving the whole limit on
    /// every page put 20ms on top of a keystroke, which is more than the
    /// search it was meant to hide.
    ///
    /// Comfortably more than `panelRows`, so the panel is full and
    /// scrollable from the first page; the complete answer carries the
    /// rest. The core still ranks the whole limit, so a page is always a
    /// true prefix of the final ranking rather than a different list.
    static let partialRows = 32

    /// The opaque handle owned by the Rust side.
    private let handle: OpaquePointer?

    /// How many results a search returns when it is not told otherwise.
    ///
    /// This is a default for `search(_:)`, not engine state: the count is
    /// passed with each search, so a scroll asking for more rows cannot
    /// change what a keystroke already in flight comes back with.
    var limit: Int = 200

    /// Whether matching is case sensitive.
    var caseSensitive: Bool = false {
        didSet { sk_engine_set_case_sensitive(handle, caseSensitive ? 1 : 0) }
    }

    /// Whether a bare term is an exact substring rather than a fuzzy
    /// subsequence. fzf's `--exact`.
    var exactMatch: Bool = true {
        didSet { sk_engine_set_exact_match(handle, exactMatch ? 1 : 0) }
    }

    /// Whether accented Latin letters are folded to their base letter.
    var normalize: Bool = true {
        didSet { sk_engine_set_normalize(handle, normalize ? 1 : 0) }
    }

    /// The scoring scheme: `0` for default, `1` for path-aware.
    var scheme: Int = 1 {
        didSet { sk_engine_set_scheme(handle, Int32(scheme)) }
    }

    init() {
        handle = sk_engine_new()

        // A property's `didSet` does not run for the value it is declared
        // with, so the initial values are applied here. Without this the
        // engine keeps its own defaults and the settings never take effect.
        sk_engine_set_case_sensitive(handle, caseSensitive ? 1 : 0)
        sk_engine_set_exact_match(handle, exactMatch ? 1 : 0)
        sk_engine_set_normalize(handle, normalize ? 1 : 0)
        sk_engine_set_scheme(handle, Int32(scheme))
    }

    deinit {
        if let handle {
            sk_engine_free(handle)
        }
    }

    // MARK: - Configuration

    /// Sets the directories to index.
    func setRoots(_ roots: [String]) {
        let joined = roots.joined(separator: ",")
        _ = joined.withCString { sk_engine_set_roots(handle, $0) }
    }

    /// Sets the glob patterns to exclude.
    func setExcludes(_ patterns: [String]) {
        let joined = patterns.joined(separator: ",")
        _ = joined.withCString { sk_engine_set_excludes(handle, $0) }
    }

    // MARK: - Scanning

    /// Starts a background scan of the configured roots.
    @discardableResult
    func startScan() -> Bool {
        sk_engine_start_scan(handle) == 0
    }

    /// Whether the background scan has finished.
    ///
    /// A scan is also finished once it has been given longer than its
    /// deadline. `opendir()` on an unresponsive directory blocks in the
    /// kernel and cannot be cancelled — measured at fifty-four minutes on
    /// a dead NFS mount — so the index stops waiting for it rather than
    /// never publishing at all.
    var scanFinished: Bool {
        sk_engine_scan_finished(handle) != 0
    }

    /// Whether the last scan walked everything it was asked to.
    ///
    /// False when it was cut short by its deadline, which makes the index
    /// partial. The panel says so: reporting a partial index as a complete
    /// one misleads in the same way a permanent "Indexing…" did.
    var scanComplete: Bool {
        sk_engine_scan_complete(handle) != 0
    }

    /// Collects the scanned entries into the index.
    @discardableResult
    func finishScan() -> Int {
        Int(sk_engine_finish_scan(handle))
    }

    /// The number of entries in the index.
    var count: Int {
        Int(sk_engine_count(handle))
    }

    // MARK: - Persistence

    /// The default path of the index snapshot.
    static var defaultSnapshotPath: String {
        guard let path = sk_engine_default_snapshot_path() else { return "" }
        defer { sk_string_free(path) }
        return String(cString: path)
    }

    /// Whether a snapshot exists at the given path.
    static func snapshotExists(at path: String) -> Bool {
        path.withCString { sk_snapshot_exists($0) != 0 }
    }

    /// Loads an index snapshot.
    @discardableResult
    func loadSnapshot(at path: String) -> Bool {
        path.withCString { sk_engine_load_snapshot(handle, $0) >= 0 }
    }

    /// Writes the index to a snapshot file.
    @discardableResult
    func saveSnapshot(at path: String) -> Bool {
        path.withCString { sk_engine_save_snapshot(handle, $0) == 0 }
    }

    // MARK: - Watching

    /// A change to the filesystem.
    struct FileChange {
        /// The path that changed.
        let path: String

        /// The kind of change.
        enum Kind: Int {
            case created = 0
            case modified = 1
            case removed = 2

            /// The change could not be described precisely, so the subtree
            /// has to be rescanned.
            case unknown = 3
        }

        let kind: Kind

        /// Whether the entry is a directory.
        let isDirectory: Bool
    }

    /// Starts watching the configured roots for changes.
    @discardableResult
    func startWatching() -> Bool {
        sk_engine_start_watch(handle) == 0
    }

    /// Collects the changes the watchers have reported.
    func pollChanges() -> [FileChange] {
        // The buffer holds the records, each of which is terminated by a
        // null byte. A kilobyte is enough for a batch of changes.
        let capacity = 16 * 1024
        var buffer = [CChar](repeating: 0, count: capacity)

        let found = buffer.withUnsafeMutableBufferPointer { pointer in
            sk_engine_poll_changes(handle, pointer.baseAddress, Int32(capacity))
        }

        guard found > 0 else { return [] }

        // The records are packed one after another, each ending with a null
        // byte, so they are read out of the buffer as C strings.
        //
        // The buffer is walked by finding the null bytes, because the
        // records are of different lengths and there is no length prefix.
        var changes: [FileChange] = []
        var offset = 0

        for _ in 0..<found {
            // The search for the terminator starts at the current offset,
            // so that the index that comes back is the absolute one.
            guard let terminator = buffer[offset...].firstIndex(of: 0) else { break }

            let record = String(
                decoding: buffer[offset..<terminator].map { UInt8(bitPattern: $0) },
                as: UTF8.self
            )
            offset = terminator + 1

            let fields = record.split(separator: "\t", omittingEmptySubsequences: false)
            guard fields.count == 3,
                  let kind = FileChange.Kind(rawValue: Int(fields[0]) ?? 3),
                  let isDirectory = Int(fields[1])
            else { continue }

            changes.append(
                FileChange(
                    path: String(fields[2]),
                    kind: kind,
                    isDirectory: isDirectory != 0
                )
            )
        }

        return changes
    }

    /// Applies a batch of changes to the index.
    @discardableResult
    func applyChanges(_ changes: [FileChange]) -> Int {
        guard !changes.isEmpty else { return count }

        // The path strings have to outlive the call, so they are copied into
        // heap buffers that are freed once the call has returned.
        var cStrings: [UnsafeMutablePointer<CChar>] = []
        cStrings.reserveCapacity(changes.count)

        defer {
            for pointer in cStrings {
                free(pointer)
            }
        }

        var raw = [FlFileChange](
            repeating: FlFileChange(path: nil, kind: 0, is_dir: 0),
            count: changes.count
        )

        return raw.withUnsafeMutableBufferPointer { buffer in
            for (index, change) in changes.enumerated() {
                guard let pointer = strdup(change.path) else { continue }
                cStrings.append(pointer)
                buffer[index].path = UnsafePointer(pointer)
                buffer[index].kind = Int32(change.kind.rawValue)
                buffer[index].is_dir = change.isDirectory ? 1 : 0
            }

            return Int(sk_engine_apply_changes(handle, buffer.baseAddress, Int32(buffer.count)))
        }
    }

    // MARK: - Searching

    /// Why `query` does not parse, or `nil` when it does.
    ///
    /// A query that cannot be parsed has no results, and a list of no
    /// results looks exactly like a query that matched nothing. The two
    /// are not the same thing to the person who typed it: one means
    /// "there is no such file" and the other means "that is not a valid
    /// regular expression". This is what tells them apart.
    ///
    /// It does not touch the engine, so it costs nothing to ask on every
    /// keystroke and never waits behind a search.
    nonisolated static func queryError(_ query: String) -> String? {
        var buffer = [CChar](repeating: 0, count: 512)

        let length = query.withCString { queryPointer in
            buffer.withUnsafeMutableBufferPointer { out in
                sk_query_error(queryPointer, out.baseAddress, Int32(out.count))
            }
        }

        guard length > 0 else { return nil }

        return String(cString: buffer)
    }

    /// How many bytes a path is assumed to take when sizing the buffer
    /// the core writes the resolved paths into.
    ///
    /// Generous on purpose: under-sizing costs a second pass over every
    /// result, and the buffer is transient.
    private let averagePathBytes = 160

    /// Runs a query and returns the ranked results.
    ///
    /// The heavy lifting happens in Rust, so this is safe to call from a
    /// background queue.
    func search(_ query: String) -> SearchOutcome {
        search(query, limit: limit)
    }

    /// Runs a query and returns at most `limit` ranked results.
    ///
    /// Asking for more costs more: the ranking can be cut down to the rows
    /// that will be shown, and a limit of a few hundred is answered in
    /// about 25ms over a quarter of a million paths, while asking for all
    /// of a broad query's matches means sorting and resolving every one of
    /// them — seconds, not milliseconds. The panel asks for a screenful
    /// and grows the request only when the user scrolls past it.
    func search(_ query: String, limit: Int) -> SearchOutcome {
        search(query, limit: limit, chunk: 0) { _ in true }
    }

    /// Runs a query, handing over the best rows it has found as it goes.
    ///
    /// `chunk` is how many candidates are scored between reports; zero
    /// means one chunk, which is a search with nothing to report until it
    /// has finished. `onProgress` is called with the rows so far and
    /// returns whether to carry on — returning false abandons the search,
    /// which is what makes a keystroke the user has already typed past
    /// cheap.
    ///
    /// The ranking is global, so the best match can be the very last
    /// candidate: an intermediate page is *provisional* and a later one
    /// may replace it. What never happens is a row that a plain ranking of
    /// the candidates seen so far would not have shown.
    ///
    /// Each report resolves the paths and the highlight offsets of its
    /// rows, because a page that cannot be drawn is not worth handing
    /// over early. That makes the report cost real, and it is why the
    /// caller picks `chunk` rather than this deciding for it: the number
    /// of reports is the number of times those paths are resolved.
    ///
    /// `onProgress` runs on the thread that called this, between chunks.
    func search(
        _ query: String,
        limit: Int,
        chunk: Int,
        onProgress: (SearchOutcome) -> Bool
    ) -> SearchOutcome {
        guard handle != nil, limit > 0 else {
            return SearchOutcome(hits: [], matched: 0)
        }

        var raw = [FlSearchResult](repeating: FlSearchResult(), count: limit)
        var matched: Int32 = 0

        // A C function pointer cannot carry a Swift closure, so the
        // closure travels as the context pointer and `streamingProgress`
        // unwraps it on the other side. It genuinely does not outlive this
        // call, which is what `withoutActuallyEscaping` is for: marking
        // the parameter `@escaping` instead would oblige every caller to
        // reason about a lifetime that does not exist.
        let found = withoutActuallyEscaping(onProgress) { report -> Int32 in
            let box = ProgressBox(engine: self, query: query, report: report)

            return withExtendedLifetime(box) {
                query.withCString { queryPointer in
                    raw.withUnsafeMutableBufferPointer { buffer in
                        sk_engine_search_streaming(
                            handle,
                            queryPointer,
                            buffer.baseAddress,
                            Int32(buffer.count),
                            &matched,
                            Int32(chunk),
                            streamingProgress,
                            Unmanaged.passUnretained(box).toOpaque()
                        )
                    }
                }
            }
        }

        guard found > 0 else { return SearchOutcome(hits: [], matched: 0) }
        raw.removeLast(raw.count - Int(found))

        return SearchOutcome(hits: hits(from: raw, query: query), matched: Int(matched))
    }

    /// Turns the core's rows into drawable results.
    ///
    /// This is the expensive half of a search's bookkeeping: the paths and
    /// the highlight offsets are not part of the ranking, so they are
    /// resolved here, for the rows that will be shown rather than for the
    /// ones that matched.
    fileprivate func hits(from raw: [FlSearchResult], query: String) -> [SearchHit] {
        let ids = raw.map(\.id)

        // The paths are resolved in one pass. Asking the core for the paths
        // of each result separately means one lock acquisition and one walk
        // of the parent chain per result, which costs far more than the
        // search itself.
        //
        // The paths come back as null-terminated strings in one buffer,
        // and the offsets say where each of them starts.
        var pathOffsets = [Int32](repeating: 0, count: raw.count * 2)

        // `sk_engine_resolve` stops when its buffer is full and reports how
        // many it managed, so a buffer that is too small does not fail —
        // it silently returns fewer results than were asked for. A fixed
        // 256KB was enough while only two hundred rows were ever asked
        // for, and quietly dropped everything past about thirteen hundred
        // once more were. So the buffer is sized from the request, and
        // grown and retried if it still fills.
        var capacity = max(256 * 1024, raw.count * 2 * averagePathBytes)
        var pathBuffer = [CChar]()
        var resolved: Int32 = 0

        for _ in 0..<4 {
            pathBuffer = [CChar](repeating: 0, count: capacity)

            resolved = ids.withUnsafeBufferPointer { idBuffer in
                pathBuffer.withUnsafeMutableBufferPointer { buffer in
                    pathOffsets.withUnsafeMutableBufferPointer { offsetBuffer in
                        sk_engine_resolve(
                            handle,
                            idBuffer.baseAddress,
                            Int32(idBuffer.count),
                            buffer.baseAddress,
                            Int32(buffer.count),
                            offsetBuffer.baseAddress
                        )
                    }
                }
            }

            if resolved >= Int32(raw.count) { break }
            capacity *= 4
        }

        // The offsets are resolved in one pass as well, for the same
        // reason.
        var offsets = [Int32](repeating: 0, count: max(raw.count * 32, 64))
        var counts = [Int32](repeating: 0, count: raw.count)

        _ = ids.withUnsafeBufferPointer { idBuffer in
            offsets.withUnsafeMutableBufferPointer { offsetBuffer in
                counts.withUnsafeMutableBufferPointer { countBuffer in
                    query.withCString { queryPointer in
                        sk_engine_positions_batch(
                            handle,
                            idBuffer.baseAddress,
                            Int32(idBuffer.count),
                            queryPointer,
                            offsetBuffer.baseAddress,
                            Int32(offsetBuffer.count),
                            countBuffer.baseAddress
                        )
                    }
                }
            }
        }

        var hits: [SearchHit] = []
        hits.reserveCapacity(Int(resolved))

        var offsetStart = 0

        for index in 0..<Int(resolved) {
            let fullStart = Int(pathOffsets[index * 2])
            let relativeStart = Int(pathOffsets[index * 2 + 1])

            guard fullStart < pathBuffer.count, relativeStart < pathBuffer.count else {
                continue
            }

            let path = String(
                decoding: pathBuffer[fullStart...].prefix { $0 != 0 }.map {
                    UInt8(bitPattern: $0)
                },
                as: UTF8.self
            )

            let relativePath = String(
                decoding: pathBuffer[relativeStart...].prefix { $0 != 0 }.map {
                    UInt8(bitPattern: $0)
                },
                as: UTF8.self
            )

            let name = (relativePath as NSString).lastPathComponent
            let directory = (relativePath as NSString).deletingLastPathComponent

            let count = Int(counts[index])
            let positions = offsets[offsetStart..<(offsetStart + count)].map { Int($0) }
            offsetStart += count

            hits.append(SearchHit(
                id: ids[index],
                score: raw[index].score,
                path: path,
                relativePath: relativePath,
                name: name,
                directory: directory,
                isDirectory: raw[index].is_dir != 0,
                positions: positions
            ))
        }

        return hits
    }

    /// The byte offsets of the matched characters for a result.
    private func matchedPositions(id: UInt32, query: String) -> [Int] {
        var buffer = [Int32](repeating: 0, count: 256)

        let total = query.withCString { queryPointer in
            buffer.withUnsafeMutableBufferPointer { pointer in
                sk_engine_positions(
                    handle,
                    id,
                    queryPointer,
                    pointer.baseAddress,
                    Int32(pointer.count)
                )
            }
        }

        guard total > 0 else { return [] }
        return buffer.prefix(Int(total)).map { Int($0) }
    }
}
