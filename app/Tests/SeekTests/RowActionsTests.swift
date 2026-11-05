//
//  RowActionsTests.swift
//  Seek
//
//  Renaming and trashing, which are the two things the panel does that
//  write to the filesystem.
//
//  Everything else the panel does is a read: a search, a preview, a path
//  on the pasteboard. These two are not, so the refusals matter as much
//  as the successes -- most of what is pinned here is the cases where the
//  file must come out untouched.
//

import XCTest

@testable import Seek

@MainActor
final class RowActionsTests: XCTestCase {
    private var root: URL!

    override func setUp() async throws {
        root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("seek-actions-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDown() async throws {
        if let root { try? FileManager.default.removeItem(at: root) }
    }

    /// Writes a file and returns the hit that stands for it.
    private func file(named name: String) throws -> SearchHit {
        let url = root.appendingPathComponent(name)
        try Data("x".utf8).write(to: url)

        return SearchHit(
            id: 1,
            score: 0,
            path: url.path,
            relativePath: name,
            name: name,
            directory: "",
            isDirectory: false,
            positions: []
        )
    }

    private func exists(_ name: String) -> Bool {
        FileManager.default.fileExists(atPath: root.appendingPathComponent(name).path)
    }

    // MARK: - Trash

    func test_trashing_takes_the_file_off_disk() throws {
        let model = AppModel()
        let hit = try file(named: "doomed.txt")

        XCTAssertTrue(model.trash(hit), "the trash move was refused")
        XCTAssertFalse(exists("doomed.txt"), "the file is still where it was")
    }

    func test_trashing_something_that_is_already_gone_is_refused_not_crashed() throws {
        let model = AppModel()
        let hit = try file(named: "fleeting.txt")
        try FileManager.default.removeItem(atPath: hit.path)

        XCTAssertFalse(model.trash(hit), "a file that is not there cannot be trashed")
        XCTAssertFalse(model.status.isEmpty, "the panel said nothing about the failure")
    }

    // MARK: - Rename

    func test_a_rename_moves_the_file_to_the_new_name() throws {
        let model = AppModel()
        let hit = try file(named: "before.txt")

        XCTAssertTrue(model.rename(hit, to: "after.txt"), "the rename was refused")
        XCTAssertFalse(exists("before.txt"))
        XCTAssertTrue(exists("after.txt"))
    }

    func test_a_rename_onto_a_name_that_is_taken_is_refused() throws {
        let model = AppModel()
        let hit = try file(named: "keep.txt")
        _ = try file(named: "taken.txt")

        XCTAssertFalse(model.rename(hit, to: "taken.txt"), "the rename overwrote a file")
        XCTAssertTrue(exists("keep.txt"), "the original is gone")
        XCTAssertTrue(exists("taken.txt"), "the other file is gone")
        XCTAssertFalse(model.status.isEmpty, "the panel said nothing about the refusal")
    }

    /// A rename takes a name, never a path.
    ///
    /// `moveItem` is happy to move a file anywhere, so a name holding a
    /// separator would turn a rename into a move -- and `../` would turn
    /// it into a move out of the folder the user was looking at. The
    /// panel offers renaming, so that is all it does.
    func test_a_name_holding_a_path_is_refused() throws {
        let model = AppModel()
        let hit = try file(named: "stay.txt")

        for attempt in ["sub/stay.txt", "../stay.txt", "/tmp/stay.txt"] {
            XCTAssertFalse(
                model.rename(hit, to: attempt),
                "\(attempt) was accepted, so renaming can move a file"
            )
            XCTAssertTrue(exists("stay.txt"), "the file moved for \(attempt)")
        }
    }

    /// Changing only the letter case is a rename, not a collision.
    ///
    /// On a case-insensitive volume -- which is what a Mac ships with --
    /// `Notes.txt` and `notes.txt` are the same path, so a check that
    /// only asked whether the target exists would refuse the one rename
    /// people most often want. That is what the case-insensitive
    /// comparison in `rename` is for; nothing else in this file covers
    /// it, and `moveItem` alone would not allow it.
    func test_changing_only_the_case_of_a_name_is_allowed() throws {
        let model = AppModel()
        let hit = try file(named: "notes.txt")

        XCTAssertTrue(
            model.rename(hit, to: "Notes.txt"),
            "a case-only rename was refused: \(model.status)"
        )

        let listed = try FileManager.default.contentsOfDirectory(atPath: root.path)
        XCTAssertEqual(listed, ["Notes.txt"], "the name did not change case")
    }

    func test_an_empty_name_is_refused() throws {
        let model = AppModel()
        let hit = try file(named: "named.txt")

        XCTAssertFalse(model.rename(hit, to: "   "), "a blank name was accepted")
        XCTAssertTrue(exists("named.txt"))
    }

    func test_renaming_to_the_same_name_does_nothing_and_says_so() throws {
        let model = AppModel()
        let hit = try file(named: "same.txt")

        XCTAssertFalse(model.rename(hit, to: "same.txt"), "a no-op rename reported success")
        XCTAssertTrue(exists("same.txt"), "a no-op rename deleted the file")
    }
}

/// While a row is being renamed, the panel's own keys have to stand down.
///
/// The panel reads every key through a local monitor before the views
/// see them, and it maps Return to "open the selected file" and Escape to
/// "hide the panel". Both are exactly wrong over a rename field: Return
/// has to commit the new name and Escape has to abandon it. The monitor
/// asks the model what to do, so the decision can be checked here without
/// a panel on screen.
@MainActor
final class RenameSessionTests: XCTestCase {
    private func event(_ keyCode: UInt16) -> NSEvent {
        NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: 0, context: nil,
            characters: "", charactersIgnoringModifiers: "",
            isARepeat: false, keyCode: keyCode
        )!
    }

    private func hit() -> SearchHit {
        SearchHit(
            id: 7, score: 0, path: "/tmp/x/note.txt", relativePath: "x/note.txt",
            name: "note.txt", directory: "x", isDirectory: false, positions: []
        )
    }

    func test_no_rename_in_progress_leaves_every_key_to_the_panel() {
        let model = AppModel()

        XCTAssertEqual(model.keyWhileRenaming(event(36)), .notRenaming)
        XCTAssertEqual(model.keyWhileRenaming(event(53)), .notRenaming)
    }

    func test_escape_abandons_the_rename_rather_than_hiding_the_panel() {
        let model = AppModel()
        model.beginRename(hit())
        XCTAssertTrue(model.isRenaming)

        XCTAssertEqual(model.keyWhileRenaming(event(53)), .cancelled)
        XCTAssertFalse(model.isRenaming, "escape left the row in its editing state")
    }

    func test_return_reaches_the_field_rather_than_opening_the_file() {
        let model = AppModel()
        model.beginRename(hit())

        XCTAssertEqual(
            model.keyWhileRenaming(event(36)), .passThrough,
            "Return was eaten by the panel, so the rename could never be committed"
        )
        XCTAssertTrue(model.isRenaming, "Return ended the session before the field saw it")
    }

    func test_the_arrow_keys_do_not_move_the_selection_mid_rename() {
        let model = AppModel()
        model.beginRename(hit())

        XCTAssertEqual(model.keyWhileRenaming(event(126)), .passThrough)
        XCTAssertEqual(model.keyWhileRenaming(event(125)), .passThrough)
    }

    func test_a_new_search_abandons_a_rename_in_progress() {
        let model = AppModel()
        model.beginRename(hit())

        model.query = "something else"

        XCTAssertFalse(
            model.isRenaming,
            "the row being renamed is gone from the list, but the field stayed open"
        )
    }
}
