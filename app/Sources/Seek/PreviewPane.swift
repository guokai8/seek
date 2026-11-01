//
//  PreviewPane.swift
//  Seek
//
//  The preview column, which follows the selected row.
//
//  The content is QuickLook's, not the app's: the same generators the
//  Finder uses for the space bar, so an image, a PDF, a video, a
//  spreadsheet and a syntax-highlighted source file all look the way the
//  user already expects them to. Reimplementing even the text case would
//  be a second, worse answer to a question the system has answered.
//

import Quartz
import SwiftUI

/// The preview column.
struct PreviewPane: View {
    /// What the column is pointed at.
    @ObservedObject var controller: PreviewController

    /// The selected row.
    ///
    /// The header is drawn from the row rather than from the controller,
    /// because the row is known the moment the selection moves while the
    /// target waits for it to settle. Reading the name from the target
    /// would leave the header a step behind the highlight.
    let hit: SearchHit

    /// How tall the column is.
    let height: CGFloat

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider().overlay(Theme.separator)
            content
        }
        .frame(width: Theme.previewWidth, height: height)
    }

    // MARK: - Header

    /// The name of the file and what is known about it.
    private var header: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(hit.name)
                .font(.system(size: 12.5, weight: .medium))
                .foregroundStyle(.primary)
                .lineLimit(1)
                .truncationMode(.middle)

            Text(summary)
                .font(.caption)
                .foregroundStyle(Theme.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
    }

    /// The line under the name.
    ///
    /// The size and the date arrive with the content rather than before
    /// it, so the header never describes one file while the body still
    /// holds the last one. Until they do, the directory stands on its own.
    private var summary: String {
        var parts: [String] = []
        let metadata = controller.preview?.url.path == hit.path
            ? controller.preview?.metadata
            : nil

        if hit.isDirectory {
            parts.append("Folder")
        } else if let size = metadata?.sizeDescription {
            parts.append(size)
        }

        if let modified = metadata?.modified {
            parts.append(modified.formatted(date: .abbreviated, time: .shortened))
        }

        if !hit.directory.isEmpty {
            parts.append(hit.directory)
        }

        return parts.joined(separator: " · ")
    }

    // MARK: - Content

    /// The preview itself.
    @ViewBuilder
    private var content: some View {
        switch controller.preview?.body {
        case .text(let text):
            // Monospaced and soft-wrapped. Code is what most of this
            // corpus is, and a 360pt column is too narrow to scroll
            // sideways through a long line.
            ScrollView {
                Text(text)
                    .font(.system(size: 11, weight: .regular, design: .monospaced))
                    .foregroundStyle(.primary)
                    .textSelection(.enabled)
                    .lineSpacing(1.5)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

        case .quickLook:
            if let url = controller.preview?.url {
                QuickLook(url: url)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }

        case nil:
            // The row has not settled yet. A spinner here would flash on
            // every keystroke, so the space is simply left quiet.
            Color.clear
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

/// A QuickLook preview of a file.
///
/// The view is reused across selections rather than rebuilt: QuickLook
/// keeps its generator and its scroll position per view, and a new view
/// per row would start from nothing every time.
struct QuickLook: NSViewRepresentable {
    /// The file to preview.
    let url: URL

    func makeNSView(context: Context) -> QLPreviewView {
        // The compact style leaves out QuickLook's own title bar, which
        // would repeat the name the header above already gives. It draws
        // the same content as the normal style — both were tried against
        // a PDF, an image and a Markdown file, and the renditions were
        // identical.
        let view = QLPreviewView(frame: .zero, style: .compact) ?? QLPreviewView()
        view.autostarts = true
        view.shouldCloseWithWindow = false
        view.previewItem = url as NSURL

        return view
    }

    func updateNSView(_ view: QLPreviewView, context: Context) {
        // Handing QuickLook the item it already holds makes it reload,
        // and SwiftUI updates this view for reasons that have nothing to
        // do with the file.
        guard (view.previewItem as? NSURL) as URL? != url else { return }

        view.previewItem = url as NSURL
    }

    static func dismantleNSView(_ view: QLPreviewView, coordinator: ()) {
        // The view holds a generator and a file handle, and the panel is
        // opened and closed hundreds of times a day.
        view.close()
    }
}
