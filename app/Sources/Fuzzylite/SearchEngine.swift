//
//  SearchEngine.swift
//  Fuzzylite
//
//  A Swift wrapper around the Rust search core. The Rust side owns the index
//  and the ranking; this class owns the lifetime of the engine handle and
//  translates between Swift values and the C ABI.
//

import Foundation
import FuzzyliteCore

/// A single search result.
struct SearchHit: Identifiable, Equatable {
    /// The index entry id, used to resolve the path and the highlight.
    let id: UInt32

    /// The match score. Higher is better.
    let score: Int32

    /// The full path of the entry.
    let path: String

    /// The file name of the entry, which is what the matcher scored.
    let name: String

    /// The directory containing the entry.
    let directory: String

    /// Whether the entry is a directory.
    let isDirectory: Bool

    /// The byte offsets of the matched characters within the file name.
    let positions: [Int]

    /// The id, exposed for `Identifiable`.
    var identity: UInt32 { id }
}

/// The Rust-backed search engine.
///
/// The engine is thread-safe: the Rust side guards its state with a mutex,
/// so searches can run on a background queue while the UI stays responsive.
/// This makes the class `Sendable`, which lets it be used from the scan
/// queue and the search queue.
final class SearchEngine: @unchecked Sendable {
    /// The opaque handle owned by the Rust side.
    private let handle: OpaquePointer?

    /// The number of results a search returns.
    var limit: Int = 200 {
        didSet { fl_engine_set_limit(handle, Int32(limit)) }
    }

    /// Whether matching is case sensitive.
    var caseSensitive: Bool = false {
        didSet { fl_engine_set_case_sensitive(handle, caseSensitive ? 1 : 0) }
    }

    /// The scoring scheme: `0` for default, `1` for path-aware.
    var scheme: Int = 1 {
        didSet { fl_engine_set_scheme(handle, Int32(scheme)) }
    }

    init() {
        handle = fl_engine_new()
    }

    deinit {
        if let handle {
            fl_engine_free(handle)
        }
    }

    // MARK: - Configuration

    /// Sets the directories to index.
    func setRoots(_ roots: [String]) {
        let joined = roots.joined(separator: ",")
        _ = joined.withCString { fl_engine_set_roots(handle, $0) }
    }

    /// Sets the glob patterns to exclude.
    func setExcludes(_ patterns: [String]) {
        let joined = patterns.joined(separator: ",")
        _ = joined.withCString { fl_engine_set_excludes(handle, $0) }
    }

    // MARK: - Scanning

    /// Starts a background scan of the configured roots.
    @discardableResult
    func startScan() -> Bool {
        fl_engine_start_scan(handle) == 0
    }

    /// Whether the background scan has finished.
    var scanFinished: Bool {
        fl_engine_scan_finished(handle) != 0
    }

    /// Collects the scanned entries into the index.
    @discardableResult
    func finishScan() -> Int {
        Int(fl_engine_finish_scan(handle))
    }

    /// The number of entries in the index.
    var count: Int {
        Int(fl_engine_count(handle))
    }

    // MARK: - Persistence

    /// The default path of the index snapshot.
    static var defaultSnapshotPath: String {
        guard let path = fl_engine_default_snapshot_path() else { return "" }
        defer { fl_string_free(path) }
        return String(cString: path)
    }

    /// Whether a snapshot exists at the given path.
    static func snapshotExists(at path: String) -> Bool {
        path.withCString { fl_snapshot_exists($0) != 0 }
    }

    /// Loads an index snapshot.
    @discardableResult
    func loadSnapshot(at path: String) -> Bool {
        path.withCString { fl_engine_load_snapshot(handle, $0) >= 0 }
    }

    /// Writes the index to a snapshot file.
    @discardableResult
    func saveSnapshot(at path: String) -> Bool {
        path.withCString { fl_engine_save_snapshot(handle, $0) == 0 }
    }

    // MARK: - Searching

    /// Runs a query and returns the ranked results.
    ///
    /// The heavy lifting happens in Rust, so this is safe to call from a
    /// background queue.
    func search(_ query: String) -> [SearchHit] {
        guard handle != nil else { return [] }

        var raw = [FlSearchResult](repeating: FlSearchResult(), count: limit)

        let found = query.withCString { queryPointer in
            raw.withUnsafeMutableBufferPointer { buffer in
                fl_engine_search(
                    handle,
                    queryPointer,
                    buffer.baseAddress,
                    Int32(buffer.count)
                )
            }
        }

        guard found > 0 else { return [] }
        raw.removeLast(raw.count - Int(found))

        // The paths are resolved in one pass, so the Rust side is only asked
        // for the paths that are actually on screen.
        var hits: [SearchHit] = []
        hits.reserveCapacity(raw.count)

        for entry in raw {
            guard let pathPtr = fl_engine_path(handle, entry.id) else { continue }
            let path = String(cString: pathPtr)
            fl_string_free(pathPtr)

            let name = (path as NSString).lastPathComponent
            let directory = (path as NSString).deletingLastPathComponent

            var isDirectory: ObjCBool = false
            FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory)

            let positions = matchedPositions(id: entry.id, query: query)

            hits.append(SearchHit(
                id: entry.id,
                score: entry.score,
                path: path,
                name: name,
                directory: directory,
                isDirectory: isDirectory.boolValue,
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
                fl_engine_positions(
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
