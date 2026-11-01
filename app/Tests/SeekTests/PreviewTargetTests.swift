//
//  PreviewTargetTests.swift
//  Seek
//
//  What the preview column is pointed at, and how it is drawn.
//
//  Loading a preview is expensive: QuickLook starts a generator, reads the
//  file and renders it. Somebody holding the down arrow walks through
//  twenty rows in well under a second, and pointing the preview at each
//  one in turn would start twenty loads to show the twenty-first. So the
//  target lags the selection by a moment, and these tests pin that it is
//  the row the user *stopped* on that gets loaded.
//
//  They also pin which half of the preview draws it. QuickLook has no
//  generator for Markdown on macOS, and a launcher over a corpus of
//  `.md` and source files that previews the most common file as its type
//  icon is a preview that does not work. Text is therefore read and drawn
//  here, and everything else is left to QuickLook, which does it better
//  than any of this could.
//

import Combine
import Foundation
import XCTest

@testable import Seek

@MainActor
final class PreviewTargetTests: XCTestCase {
    /// A short delay, so that the tests do not wait on a human timescale.
    private let delay: Duration = .milliseconds(40)

    /// The files a test wrote.
    private var scratch: [URL] = []

    override func tearDown() async throws {
        for url in scratch {
            try? FileManager.default.removeItem(at: url)
        }
        scratch = []
    }

    /// A row standing for a path.
    private func hit(_ path: String) -> SearchHit {
        SearchHit(
            id: UInt32(abs(path.hashValue % 100_000)),
            score: 100,
            path: path,
            relativePath: (path as NSString).lastPathComponent,
            name: (path as NSString).lastPathComponent,
            directory: (path as NSString).deletingLastPathComponent,
            isDirectory: false,
            positions: []
        )
    }

    /// Writes `contents` to a scratch file called `name`.
    private func file(_ name: String, _ contents: Data) throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("seek-preview-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        scratch.append(directory)

        let url = directory.appendingPathComponent(name)
        try contents.write(to: url)

        return url
    }

    /// Waits for the controller to settle.
    private func settle() async throws {
        try await Task.sleep(for: delay * 4)
    }

    /// Waits for the controller to publish a preview of `url`.
    private func preview(
        of url: URL,
        in controller: PreviewController
    ) async throws -> FilePreview {
        controller.show(hit(url.path))

        let deadline = Date().addingTimeInterval(5)
        while controller.preview?.url != url, Date() < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }

        return try XCTUnwrap(controller.preview, "no preview of \(url.lastPathComponent) arrived")
    }

    // MARK: - Which row

    func test_the_preview_follows_the_selected_row() async throws {
        let controller = PreviewController(delay: delay)

        controller.show(hit("/tmp/one.txt"))
        try await settle()

        XCTAssertEqual(controller.preview?.url.path, "/tmp/one.txt")
    }

    func test_a_row_that_was_only_passed_over_is_never_loaded() async throws {
        // The reason the delay exists. Nothing but the last row may ever
        // reach the view, because everything that reaches the view is a
        // preview that was read and rendered.
        let controller = PreviewController(delay: delay)

        var seen: [String] = []
        let subscription = controller.$preview.sink { preview in
            if let preview { seen.append(preview.url.path) }
        }
        defer { subscription.cancel() }

        controller.show(hit("/tmp/one.txt"))
        controller.show(hit("/tmp/two.txt"))
        controller.show(hit("/tmp/three.txt"))
        try await settle()

        XCTAssertEqual(seen, ["/tmp/three.txt"], "a row that was passed over was loaded")
    }

    func test_closing_the_preview_drops_the_target_at_once() async throws {
        // Clearing is not debounced: a column that is going away must not
        // spend its last moment reading a file, and a list that has just
        // been emptied must not keep showing a row that is no longer in
        // it.
        let controller = PreviewController(delay: delay)

        controller.show(hit("/tmp/one.txt"))
        try await settle()
        XCTAssertNotNil(controller.preview)

        controller.show(nil)

        XCTAssertNil(controller.preview, "the old file was still on screen")
    }

    func test_the_same_row_is_not_loaded_twice() async throws {
        // The view hands over the selection whenever anything about the
        // results changes, and a streaming search changes them several
        // times per keystroke. Reloading the row the user is already
        // looking at would make the preview flicker through every page.
        let controller = PreviewController(delay: delay)

        var loads = 0
        let subscription = controller.$preview.sink { preview in
            if preview != nil { loads += 1 }
        }
        defer { subscription.cancel() }

        controller.show(hit("/tmp/one.txt"))
        try await settle()
        controller.show(hit("/tmp/one.txt"))
        try await settle()

        XCTAssertEqual(loads, 1, "the same row was loaded again")
    }

    func test_the_preview_arrives_whole() async throws {
        // The name, the size, the date and the content are published
        // together. Publishing them as they landed would draw the header
        // against one file while the body still held the last one.
        let url = try file("notes.txt", Data("hello".utf8))

        let preview = try await preview(of: url, in: PreviewController(delay: delay))

        XCTAssertEqual(preview.metadata.byteCount, 5)
        XCTAssertNotNil(preview.metadata.modified)
        XCTAssertEqual(preview.body, .text("hello"))
    }

    // MARK: - Which half draws it

    func test_a_text_file_is_read_and_drawn_here() async throws {
        // The case QuickLook cannot do: there is no Markdown generator on
        // macOS, so the Finder's space bar shows the type icon too.
        let markdown = "# Title\n\nA paragraph.\n"
        let url = try file("README.md", Data(markdown.utf8))

        let preview = try await preview(of: url, in: PreviewController(delay: delay))

        XCTAssertEqual(preview.body, .text(markdown))
    }

    func test_a_file_with_no_extension_that_holds_text_is_drawn_here_too() async throws {
        // A launcher lands on these constantly, and nothing but the bytes
        // can say what they are.
        let url = try file("Makefile", Data("all:\n\tcargo build\n".utf8))

        let preview = try await preview(of: url, in: PreviewController(delay: delay))

        XCTAssertEqual(preview.body, .text("all:\n\tcargo build\n"))
    }

    func test_a_binary_file_is_left_to_quicklook() async throws {
        var png = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
        png.append(Data(repeating: 0, count: 64))
        let url = try file("picture.png", png)

        let preview = try await preview(of: url, in: PreviewController(delay: delay))

        XCTAssertEqual(preview.body, .quickLook)
    }

    func test_a_file_with_no_extension_that_holds_bytes_is_left_to_quicklook() async throws {
        // The case the declared type cannot settle, which is why the
        // bytes are looked at as well: an extensionless file is taken on
        // trust as far as its type goes, and a compiled binary would
        // otherwise be dumped into the column as mojibake.
        var binary = Data([0x7F, 0x45, 0x4C, 0x46])
        binary.append(Data(repeating: 0, count: 200))
        let url = try file("linker", binary)

        let preview = try await preview(of: url, in: PreviewController(delay: delay))

        XCTAssertEqual(preview.body, .quickLook)
    }

    func test_a_document_quicklook_renders_properly_is_left_to_it() async throws {
        // A PDF's head can be plain ASCII for pages at a time, so the
        // bytes alone would call it text and draw the user a screen of
        // `%PDF-1.4` and object definitions. The file's declared type
        // settles it before the bytes are ever looked at.
        let url = try file("paper.pdf", Data("%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\n".utf8))

        let preview = try await preview(of: url, in: PreviewController(delay: delay))

        XCTAssertEqual(preview.body, .quickLook)
    }

    func test_a_directory_is_left_to_quicklook() async throws {
        let url = try file("inner.txt", Data("x".utf8))
        let directory = url.deletingLastPathComponent()

        let controller = PreviewController(delay: delay)
        var row = hit(directory.path)
        row = SearchHit(
            id: row.id, score: row.score, path: row.path,
            relativePath: row.relativePath, name: row.name,
            directory: row.directory, isDirectory: true, positions: []
        )
        controller.show(row)

        let deadline = Date().addingTimeInterval(5)
        while controller.preview == nil, Date() < deadline {
            try await Task.sleep(for: .milliseconds(10))
        }

        let preview = try XCTUnwrap(controller.preview)
        XCTAssertEqual(preview.body, .quickLook)
        XCTAssertTrue(preview.metadata.isDirectory)
    }

    func test_only_the_head_of_a_long_file_is_read() async throws {
        // A log file is gigabytes. The column shows its first screenful
        // either way, and reading the rest would stall the panel.
        let line = String(repeating: "x", count: 120) + "\n"
        let big = String(repeating: line, count: 20_000)  // ~2.4MB
        let url = try file("huge.log", Data(big.utf8))

        let preview = try await preview(of: url, in: PreviewController(delay: delay))

        guard case .text(let shown) = preview.body else {
            return XCTFail("a long text file was not drawn as text")
        }

        XCTAssertLessThanOrEqual(
            shown.utf8.count,
            PreviewReader.headBytes,
            "more than the head was read"
        )
        XCTAssertTrue(shown.hasPrefix(line), "the head is not what was kept")
        XCTAssertEqual(preview.metadata.byteCount, Int64(big.utf8.count), "the size is the whole file's")
    }

    func test_a_file_of_many_short_lines_is_cut_to_a_line_count() async throws {
        // The byte ceiling is not enough on its own: 64KB of one-letter
        // lines is sixty thousand of them, and handing SwiftUI a `Text`
        // with sixty thousand lines in it stalls the panel on the row the
        // user only arrowed past.
        let many = String(repeating: "x\n", count: 30_000)  // 60KB, under the byte cap
        let url = try file("many.txt", Data(many.utf8))

        let preview = try await preview(of: url, in: PreviewController(delay: delay))

        guard case .text(let shown) = preview.body else {
            return XCTFail("a text file was not drawn as text")
        }

        XCTAssertLessThan(shown.utf8.count, PreviewReader.headBytes, "the byte cap did the cutting")
        XCTAssertLessThanOrEqual(
            shown.split(separator: "\n", omittingEmptySubsequences: false).count,
            PreviewReader.headLines,
            "a file of short lines was handed over whole"
        )
    }
}
