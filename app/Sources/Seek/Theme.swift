//
//  Theme.swift
//  Seek
//
//  The visual language of the panel.
//
//  The panel is a small, dense piece of interface that is looked at many
//  times a day, so the rules here are about restraint: one accent, one
//  type scale, and spacing that groups rather than decorates.
//

import SwiftUI

/// The look of the panel.
enum Theme {
    // MARK: - Metrics

    /// The radius of the panel's corners.
    static let panelRadius: CGFloat = 14

    /// The radius of a selected result row.
    static let rowRadius: CGFloat = 7

    /// The radius of the small key caps in the footer.
    static let keyRadius: CGFloat = 4

    /// The width of the panel.
    static let panelWidth: CGFloat = 720

    /// The width of the time column in a result row.
    ///
    /// Fixed, not fitted. A column that sized itself to its content would
    /// be a different width on every row -- "14:32" on one and
    /// "3 Mar 2019" on the next -- and the eye would have nothing to run
    /// down. This is wide enough for the longest of the three forms
    /// `ResultMetadataFormat.time` produces.
    static let metadataTimeWidth: CGFloat = 68

    /// The width of the size column in a result row.
    static let metadataSizeWidth: CGFloat = 58

    /// How much of a row both metadata columns take together.
    static let metadataWidth: CGFloat = metadataTimeWidth + metadataSizeWidth + 9

    /// The width of the preview column.
    ///
    /// Wide enough that a page of code or a photograph is worth looking
    /// at, and no wider: the column is half again the panel's width as it
    /// is, and every point of it is a point the results column has to
    /// shift sideways when the preview opens.
    static let previewWidth: CGFloat = 360

    /// The tallest the panel is allowed to be.
    ///
    /// The panel is as tall as its content and no taller: an empty state
    /// inside a fixed 460pt box is mostly box. This is the ceiling, and it
    /// is also where the panel's top edge is placed from, so that a panel
    /// that grows as the results arrive grows downwards instead of moving
    /// the row under the cursor.
    static let panelMaxHeight: CGFloat = 460

    /// The tallest the results list is allowed to be.
    ///
    /// This is the panel's ceiling less the 97pt the search bar, the
    /// footer and the two dividers between them take up. All of those are
    /// fixed by their own type and padding, so the figure is written down
    /// rather than measured — and `PanelHeightTests` fails on both sides if
    /// it is ever wrong: too large and the panel outgrows its ceiling, too
    /// small and it never reaches it.
    static let maxResultsHeight: CGFloat = 363

    /// The padding on the left and the right of the panel's content.
    static let gutter: CGFloat = 14

    // MARK: - Colour

    /// The accent used for the selection and the matched characters.
    ///
    /// The accent comes from the system, so it follows the user's accent
    /// colour setting and switches between light and dark on its own.
    static let accent = Color.accentColor

    /// The fill behind the selected row.
    ///
    /// Light enough that the matched characters, which are drawn in the
    /// accent at full strength, still read against it. A heavier tint
    /// takes the contrast out of exactly the row the user is looking at:
    /// the top hit was the one row where it was hardest to see what had
    /// matched.
    static let selection = Color.accentColor.opacity(0.15)

    /// The fill of a well that something sits in — the search field.
    ///
    /// The panel's background is a vibrancy material, so a well on it is
    /// a darkening of whatever is behind the window rather than a grey of
    /// its own. It has to be light, or the panel reads as a stack of grey
    /// boxes instead of one surface with a field in it.
    static let well = Color.primary.opacity(0.05)

    /// The line around a well.
    static let wellBorder = Color.primary.opacity(0.08)

    /// The fill of a key cap in the footer.
    ///
    /// Heavier than the well, because a key cap is a 10pt glyph in a box
    /// the size of a fingernail: at the well's weight the box disappears
    /// and the glyph reads as another word in the footer's sentence.
    static let keyFill = Color.primary.opacity(0.08)

    /// The colour of the line around the panel.
    static let panelBorder = Color.primary.opacity(0.10)

    /// The colour of the separator between the parts of the panel.
    static let separator = Color.primary.opacity(0.08)

    /// The colour of the text that is not the point of the panel.
    static let secondary = Color.primary.opacity(0.55)

    /// The colour of the text that is barely there.
    static let tertiary = Color.primary.opacity(0.38)
}

// MARK: - Type

extension Font {
    /// The query in the search field.
    ///
    /// The query is the one thing the user is looking at, so it is the
    /// largest text in the panel and the only thing set above the body
    /// size.
    static let query = Font.system(size: 20, weight: .regular)

    /// The name of a result.
    static let resultName = Font.system(size: 13.5, weight: .regular)

    /// The name of the selected result.
    static let resultNameSelected = Font.system(size: 13.5, weight: .semibold)

    /// The directory of a result.
    static let resultPath = Font.system(size: 11.5, weight: .regular)

    /// The status line and the other small text.
    static let caption = Font.system(size: 11, weight: .regular)

    /// The text on a key cap.
    static let key = Font.system(size: 10.5, weight: .medium, design: .rounded)
}

// MARK: - Shapes

/// The shape of the panel.
struct PanelShape: Shape {
    func path(in rect: CGRect) -> Path {
        Path(
            roundedRect: rect,
            cornerRadius: Theme.panelRadius,
            style: .continuous
        )
    }
}

/// The shape of a selected result row.
struct RowShape: Shape {
    func path(in rect: CGRect) -> Path {
        Path(
            roundedRect: rect,
            cornerRadius: Theme.rowRadius,
            style: .continuous
        )
    }
}
