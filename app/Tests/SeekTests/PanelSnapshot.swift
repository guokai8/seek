//
//  PanelSnapshot.swift
//  Seek
//
//  Renders the panel to a PNG, so that the visual result can be looked at
//  without having to drive the app by hand.
//
//  Run it with:
//
//      swift test --filter PanelSnapshot
//
//  and the images land in the build directory.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class PanelSnapshot: XCTestCase {
    /// Renders a view into an image.
    private func render<V: View>(_ view: V, size: CGSize) -> NSImage? {
        let renderer = ImageRenderer(content: view)
        renderer.scale = 2

        guard let image = renderer.nsImage else { return nil }

        // The renderer uses the view's own size, so the image is put on a
        // canvas of the size that was asked for.
        let canvas = NSImage(size: size)
        canvas.lockFocus()
        NSColor.windowBackgroundColor.setFill()
        NSRect(origin: .zero, size: size).fill()
        image.draw(
            in: NSRect(origin: .zero, size: size),
            from: NSRect(origin: .zero, size: image.size),
            operation: .copy,
            fraction: 1
        )
        canvas.unlockFocus()

        return canvas
    }

    /// Writes an image to a file.
    private func write(_ image: NSImage?, to name: String) {
        guard let image,
              let tiff = image.tiffRepresentation,
              let representation = NSBitmapImageRep(data: tiff),
              let png = representation.representation(using: .png, properties: [:])
        else {
            XCTFail("could not render \(name)")
            return
        }

        // The snapshots are written next to the built product, so that they
        // can be looked at after the test run. The temporary directory is
        // private to the test process and is gone by then.
        let directory = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("build", isDirectory: true)
            .appendingPathComponent("snapshots", isDirectory: true)

        try? FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )

        let path = directory.appendingPathComponent("\(name).png")
        try? png.write(to: path)

        print("snapshot: \(path.path)")
    }

    /// A model with a few results in the list.
    private func makeModel() -> AppModel {
        let model = AppModel()

        model.setTestResults([
            SearchHit(
                id: 0,
                score: 100,
                path: "/Users/bioguo/projects/seek/core/src/search.rs",
                relativePath: "/Users/bioguo/projects/seek/core/src/search.rs",
                name: "search.rs",
                directory: "/Users/bioguo/projects/seek/core/src",
                isDirectory: false,
                positions: [0, 1, 2, 3]
            ),
            SearchHit(
                id: 1,
                score: 90,
                path: "/Users/bioguo/projects/seek/app/Sources/Seek/SearchEngine.swift",
                relativePath: "/Users/bioguo/projects/seek/app/Sources/Seek/SearchEngine.swift",
                name: "SearchEngine.swift",
                directory: "/Users/bioguo/projects/seek/app/Sources/Seek",
                isDirectory: false,
                positions: [0, 1, 2]
            ),
            SearchHit(
                id: 2,
                score: 80,
                path: "/Users/bioguo/projects/seek/core/src/fuzzy",
                relativePath: "/Users/bioguo/projects/seek/core/src/fuzzy",
                name: "fuzzy",
                directory: "/Users/bioguo/projects/seek/core/src",
                isDirectory: true,
                positions: []
            ),
        ])

        model.query = "sea"

        return model
    }

    func test_the_panel_is_rendered() {
        let model = makeModel()
        let view = ContentView().environmentObject(model)

        let image = render(view, size: CGSize(width: 720, height: 460))

        XCTAssertNotNil(image, "the panel did not render")
        write(image, to: "panel-with-results")
    }

    func test_the_empty_panel_is_rendered() {
        let model = AppModel()
        let view = ContentView().environmentObject(model)

        let image = render(view, size: CGSize(width: 720, height: 460))

        XCTAssertNotNil(image, "the panel did not render")
        write(image, to: "panel-empty")
    }

    func test_the_settings_window_is_rendered() {
        let view = PreferencesView()

        let image = render(view, size: CGSize(width: 560, height: 520))

        XCTAssertNotNil(image, "the settings did not render")
        write(image, to: "settings")
    }
}
