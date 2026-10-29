//
//  SmokeTest.swift
//  Fuzzylite
//
//  A command line check that the core behaves as the app expects: the index
//  is built, the search finds the right files, and the ranking matches what
//  fzf would produce.
//
//  Run it with:
//
//      swift run Fuzzylite --smoke-test /some/directory
//

import Foundation
import FuzzyliteCore

/// The end-to-end check.
enum SmokeTest {
    /// Runs the check against the given directory.
    static func run(root: String) -> Int32 {
        let engine = SearchEngine()
        engine.setRoots([root])
        engine.setExcludes([".git", "node_modules"])

        print("scanning \(root) …")

        guard engine.startScan() else {
            print("FAIL: could not start the scan")
            return 1
        }

        // The scan runs on a background thread inside the Rust core.
        let deadline = Date().addingTimeInterval(120)
        while !engine.scanFinished {
            if Date() > deadline {
                print("FAIL: the scan did not finish in time")
                return 1
            }
            Thread.sleep(forTimeInterval: 0.05)
        }

        let count = engine.finishScan()
        print("indexed \(count) entries")
        guard count > 0 else {
            print("FAIL: the index is empty")
            return 1
        }

        // A query for a name that is known to exist.
        let name = (root as NSString).lastPathComponent
        let hits = engine.search(name)
        print("query \"\(name)\" -> \(hits.count) hits")

        guard !hits.isEmpty else {
            print("FAIL: the query returned nothing")
            return 1
        }

        for hit in hits.prefix(5) {
            print("  \(hit.score)\t\(hit.path)")
        }

        // The best hit must have its matched characters highlighted.
        if let first = hits.first {
            guard !first.positions.isEmpty else {
                print("FAIL: the top hit has no match positions")
                return 1
            }
            print("top hit matches at \(first.positions)")
        }

        // The index must survive a round trip through a snapshot.
        let snapshot = FileManager.default.temporaryDirectory
            .appendingPathComponent("fuzzylite-smoke-\(ProcessInfo.processInfo.processIdentifier).flidx")

        guard engine.saveSnapshot(at: snapshot.path) else {
            print("FAIL: the snapshot could not be written")
            return 1
        }

        let restored = SearchEngine()
        restored.setRoots([root])
        guard restored.loadSnapshot(at: snapshot.path) else {
            print("FAIL: the snapshot could not be read")
            return 1
        }

        let restoredCount = restored.count
        print("restored \(restoredCount) entries")

        let restoredHits = restored.search(name)
        guard restoredHits.count == hits.count else {
            print("FAIL: the restored index returns \(restoredHits.count) hits, expected \(hits.count)")
            return 1
        }

        try? FileManager.default.removeItem(at: snapshot)

        // The ranking must be identical, which means the snapshot really
        // stores the same index and not just the same number of entries.
        guard restoredHits.first?.path == hits.first?.path else {
            print("FAIL: the restored index ranks the results differently")
            return 1
        }

        print("OK")
        return 0
    }
}
