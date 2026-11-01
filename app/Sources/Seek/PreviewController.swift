//
//  PreviewController.swift
//  Seek
//
//  What the preview column shows.
//
//  Two jobs. The first is *when*: loading a preview is expensive, and
//  somebody holding the down arrow walks through twenty rows in well under
//  a second, so the target lags the selection by a moment and only the row
//  they stopped on is ever read.
//
//  The second is *which half draws it*. QuickLook is the right answer for
//  an image, a PDF, a video or a spreadsheet — it is the Finder's own
//  machinery and nothing written here would do it better. It is the wrong
//  answer for text: macOS ships no Markdown generator, so a `.md` file
//  previews as its type icon, and over a corpus of notes and source files
//  that is the most common file in the list showing nothing. Text is
//  therefore read and drawn here.
//

import Foundation
import UniformTypeIdentifiers

/// How a file is drawn in the preview column.
enum PreviewBody: Sendable, Equatable {
    /// The head of a text file, to be drawn by the panel.
    case text(String)

    /// Anything else, to be drawn by QuickLook.
    case quickLook
}

/// A file, ready to be shown.
///
/// One value rather than three published properties: the name, the size,
/// the date and the content are drawn together, and publishing them as
/// they landed would draw the header against one file while the body
/// still held the last one.
struct FilePreview: Sendable, Equatable {
    /// The file being shown.
    let url: URL

    /// What the header says about it.
    let metadata: PreviewMetadata

    /// What the column draws.
    let body: PreviewBody
}

/// What the header says about the file.
struct PreviewMetadata: Sendable, Equatable {
    /// How large the file is, when it is a file.
    let byteCount: Int64?

    /// When it was last changed.
    let modified: Date?

    /// Whether it is a directory.
    let isDirectory: Bool

    /// Reads the metadata of `url`.
    init(of url: URL) {
        let values = try? url.resourceValues(forKeys: [
            .fileSizeKey,
            .contentModificationDateKey,
            .isDirectoryKey,
        ])

        isDirectory = values?.isDirectory ?? false
        modified = values?.contentModificationDate
        byteCount = isDirectory ? nil : values?.fileSize.map(Int64.init)
    }

    /// The size, written the way the Finder writes it.
    var sizeDescription: String? {
        guard let byteCount else { return nil }

        return ByteCountFormatter.string(fromByteCount: byteCount, countStyle: .file)
    }
}

/// The preview column's target.
@MainActor
final class PreviewController: ObservableObject {
    /// The file the column is showing.
    ///
    /// Every value this publishes is a file that was read, which is why
    /// the rows the selection only passed over never appear here.
    @Published private(set) var preview: FilePreview?

    /// How long the target lags the selection.
    private let delay: Duration

    /// The load that is waiting for the selection to settle.
    private var pending: Task<Void, Never>?

    /// The path that is shown or is about to be.
    ///
    /// Kept apart from `preview`, because it is set as soon as the
    /// selection moves while `preview` waits for the delay and then for
    /// the disk. Comparing against it is what stops a streaming search —
    /// which hands the selection over again for every page — from
    /// reloading the row the user is already looking at.
    private var shown: String?

    init(delay: Duration = .milliseconds(120)) {
        self.delay = delay
    }

    /// Points the column at `hit`, or at nothing.
    ///
    /// Clearing is immediate: a column that is going away must not spend
    /// its last moment reading a file, and a list that has just been
    /// emptied must not keep showing a row that is no longer in it.
    func show(_ hit: SearchHit?) {
        guard let hit else {
            pending?.cancel()
            pending = nil
            shown = nil
            preview = nil
            return
        }

        guard hit.path != shown else { return }

        shown = hit.path
        pending?.cancel()

        let delay = delay
        let url = URL(fileURLWithPath: hit.path)

        pending = Task { [weak self] in
            try? await Task.sleep(for: delay)

            guard !Task.isCancelled else { return }

            // The disk is read off the main actor. A file on a network
            // share that has gone away would otherwise freeze the panel
            // with the selection half-moved.
            let loaded = await Task.detached(priority: .utility) {
                FilePreview(
                    url: url,
                    metadata: PreviewMetadata(of: url),
                    body: PreviewReader.body(of: url)
                )
            }.value

            guard !Task.isCancelled else { return }

            // The selection may have moved on while the disk was being
            // asked, and the column must show the file it is pointed at
            // rather than the one it was.
            guard self?.shown == url.path else { return }

            self?.preview = loaded
        }
    }
}

/// Decides how a file is drawn, and reads it when it is text.
enum PreviewReader {
    /// How much of a file is read.
    ///
    /// A log file is gigabytes and the column shows its first screenful
    /// either way. Reading the rest would stall the panel and then hand
    /// SwiftUI a string it cannot lay out.
    static let headBytes = 64 * 1024

    /// How many lines of it are kept.
    ///
    /// The column is 363pt tall at 11pt type, so this is a dozen
    /// screenfuls to scroll through — past the point where somebody
    /// deciding whether this is the right file keeps reading.
    static let headLines = 400

    /// What the column should draw for `url`.
    static func body(of url: URL) -> PreviewBody {
        guard let head = head(of: url) else { return .quickLook }

        return .text(head)
    }

    /// The head of `url`, when it is text.
    ///
    /// Two questions, in this order. The file's declared type decides
    /// first: a PDF's head can be plain ASCII for pages at a time, so the
    /// bytes alone would call it text and draw a screen of `%PDF-1.4` and
    /// object definitions. A file whose type is not known — `Makefile`,
    /// `LICENSE`, `.gitignore`, which a launcher lands on constantly — has
    /// nothing but its bytes to go on, and those are asked next.
    private static func head(of url: URL) -> String? {
        guard declaresText(url) else { return nil }

        guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
        defer { try? handle.close() }

        guard let data = try? handle.read(upToCount: headBytes), !data.isEmpty else {
            return nil
        }

        // A NUL byte is what every binary format has and no text file
        // does.
        guard !data.contains(0) else { return nil }

        guard let text = decode(data) else { return nil }

        return text.split(separator: "\n", omittingEmptySubsequences: false)
            .prefix(headLines)
            .joined(separator: "\n")
    }

    /// Whether the file's own type says it holds text.
    ///
    /// An unknown type is not a no: it is the answer for every file
    /// without an extension, and those are decided by their bytes.
    private static func declaresText(_ url: URL) -> Bool {
        guard let type = UTType(filenameExtension: url.pathExtension) else {
            return true
        }

        guard type.conforms(to: .text) else { return false }

        // The two kinds of text QuickLook draws better than a monospaced
        // dump of the source would: rich text has formatting that is the
        // point of it, and a web page is meant to be rendered.
        return !type.conforms(to: .rtf) && !type.conforms(to: .html)
    }

    /// Decodes `data` as UTF-8, allowing for a character split by the end
    /// of the window.
    private static func decode(_ data: Data) -> String? {
        for trimmed in 0...3 {
            let slice = data.prefix(data.count - trimmed)

            if let text = String(data: slice, encoding: .utf8) {
                return text
            }
        }

        return nil
    }
}
