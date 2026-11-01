//
//  PanelHeightTests.swift
//  Seek
//
//  The panel is as tall as what is in it. These tests pin both ends of
//  that: a full list must not outgrow the ceiling, and an empty state must
//  not be padded out to it.
//

import AppKit
import SwiftUI
import XCTest

@testable import Seek

@MainActor
final class PanelHeightTests: XCTestCase {
    /// A window the panel can lay out in.
    ///
    /// The rows are a `LazyVStack` inside a `ScrollView`, and a lazy
    /// container has nothing to be lazy about without a viewport, so it
    /// draws and measures nothing outside a window. The window is never put
    /// on screen: ordering it front would take the key window away from the
    /// focus tests.
    private var windows: [NSWindow] = []

    /// The height the panel asks for, with `model` behind it.
    private func height(of model: AppModel) -> CGFloat {
        let hosting = NSHostingController(rootView: ContentView().environmentObject(model))

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: Theme.panelWidth, height: Theme.panelMaxHeight),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.contentViewController = hosting
        windows.append(window)

        hosting.view.layoutSubtreeIfNeeded()

        return hosting.view.fittingSize.height
    }

    /// A model holding `count` results.
    private func model(withResults count: Int) -> AppModel {
        let model = AppModel()

        model.setTestResults(
            (0..<count).map { i in
                SearchHit(
                    id: UInt32(i),
                    score: Int32(100 - i),
                    path: "/Users/someone/projects/seek/core/src/file\(i).rs",
                    relativePath: "core/src/file\(i).rs",
                    name: "file\(i).rs",
                    directory: "core/src",
                    isDirectory: false,
                    positions: [0, 1, 2]
                )
            }
        )
        model.query = "file"

        return model
    }

    func test_the_panel_never_outgrows_its_ceiling() {
        // Far more results than could ever fit, so the list is the thing
        // that would overflow if the ceiling were wrong.
        let tall = height(of: model(withResults: 200))

        XCTAssertLessThanOrEqual(
            tall,
            Theme.panelMaxHeight,
            "the panel is \(tall)pt tall, over its \(Theme.panelMaxHeight)pt ceiling: "
                + "Theme.maxResultsHeight no longer leaves room for the search bar and the footer"
        )
    }

    func test_the_panel_fills_its_ceiling_when_there_are_enough_results() {
        let tall = height(of: model(withResults: 200))

        // The other half of the claim: the ceiling is reached, not merely
        // respected. A `maxResultsHeight` set far too low would pass the
        // test above while wasting most of the panel.
        XCTAssertGreaterThan(
            tall,
            Theme.panelMaxHeight - 24,
            "the panel only grows to \(tall)pt of its \(Theme.panelMaxHeight)pt ceiling"
        )
    }

    func test_the_empty_state_does_not_fill_the_panel() {
        let short = height(of: AppModel())

        // An empty state in a full-height box is mostly box. Half the
        // ceiling is a generous bound: the hint is three lines of text.
        XCTAssertLessThan(
            short,
            Theme.panelMaxHeight / 2,
            "the empty panel is \(short)pt tall, which is a tall empty box"
        )
    }

    func test_a_short_list_does_not_fill_the_panel() {
        let short = height(of: model(withResults: 3))
        let tall = height(of: model(withResults: 200))

        XCTAssertLessThan(
            short,
            tall,
            "three results take as much room as two hundred"
        )
    }
}
