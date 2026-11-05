//
//  WindowDragArea.swift
//  Seek
//
//  A region of the panel that drags the whole window.
//
//  Not `isMovableByWindowBackground`, which was tried and is what the
//  comment in `makePanel` argues against: it makes the results list a
//  drag handle, so a mouse-down that was meant to pick a row sometimes
//  throws the panel across the screen instead. A region of its own means
//  exactly one part of the panel drags it, and that part holds nothing
//  the user clicks for another reason.
//
//  `WindowDragGesture` would do this in SwiftUI, and it is macOS 15. The
//  app's minimum is 14.
//

import AppKit
import SwiftUI

/// A transparent region that drags the window it is placed in.
struct WindowDragArea: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        DragView()
    }

    func updateNSView(_ nsView: NSView, context: Context) {}

    /// The view that does the dragging.
    ///
    /// `performDrag` runs its own event loop until the mouse comes up, so
    /// there is no mouse-dragged or mouse-up handler to write: the window
    /// server moves the window and returns when the drag is over.
    final class DragView: NSView {
        override func mouseDown(with event: NSEvent) {
            window?.performDrag(with: event)
        }

        // A background region must not take the focus away from the
        // search field, which is the one thing the panel exists to be
        // typed into.
        override var acceptsFirstResponder: Bool { false }
    }
}
