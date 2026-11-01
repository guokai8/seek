//
//  ResultRow.swift
//  Seek
//
//  A single row in the results list, with the matched characters
//  highlighted the way fzf highlights them.
//

import SwiftUI

/// One row of the results list.
struct ResultRow: View {
    /// The result to display.
    let hit: SearchHit

    /// Whether this row is the selected one.
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 9) {
            icon
                .frame(width: 16)

            Text(highlightedName)
                .font(isSelected ? .resultNameSelected : .resultName)
                .foregroundStyle(.primary)
                .lineLimit(1)
                .truncationMode(.middle)
                .layoutPriority(1)

            if !hit.directory.isEmpty {
                // The directory is separated from the name by a hairline
                // rather than by space alone. Space alone leaves the two
                // texts looking like one run-on string, and the eye cannot
                // tell where the name ends.
                Rectangle()
                    .fill(Theme.separator)
                    .frame(width: 1, height: 9)

                Text(highlightedDirectory)
                    .font(.resultPath)
                    .foregroundStyle(Theme.secondary)
                    .lineLimit(1)
                    .truncationMode(.head)
            }

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 4)
    }

    /// The icon that tells a folder from a file.
    ///
    /// The icon is tinted rather than coloured, because the row is about
    /// the name: a folder in blue next to a page in grey is noise when
    /// there are two hundred rows of them.
    private var icon: some View {
        Image(systemName: hit.isDirectory ? "folder.fill" : "doc")
            .font(.system(size: 12, weight: .regular))
            .foregroundStyle(
                hit.isDirectory ? Theme.accent.opacity(0.85) : Theme.tertiary
            )
    }

    /// The file name with the matched characters emphasised.
    ///
    /// The matched characters are distinguished by weight as well as by
    /// colour, so the highlight does not depend on colour perception alone.
    private var highlightedName: AttributedString {
        highlighted(
            hit.name,
            at: hit.namePositions,
            size: 13.5,
            isSelected: isSelected
        )
    }

    /// The directory with the matched characters emphasised.
    ///
    /// The directory is matched too, because the matcher scores the full
    /// path. A match that lands in the directory is shown there, so the
    /// row shows exactly where the query was found.
    private var highlightedDirectory: AttributedString {
        highlighted(
            hit.directory,
            at: hit.directoryPositions,
            size: 11.5,
            isSelected: isSelected
        )
    }

    /// Builds the attributed text with the matched characters emphasised.
    ///
    /// The selected row is highlighted the same way as every other row.
    /// It used to drop the colour and mark the match with weight alone,
    /// on the theory that the row's tint already carried the emphasis —
    /// but the selected row's name is drawn semibold, so the highlight was
    /// bold against semibold at 13.5pt, which is nothing. The top hit was
    /// the one row where the user could not see what had matched. The tint
    /// is only 22% of the accent, and the accent at full strength reads
    /// cleanly on it in both appearances.
    ///
    /// The offsets come from the matcher as byte offsets, so they are
    /// converted to character offsets before they are used to split the
    /// string.
    private func highlighted(
        _ text: String,
        at offsets: [Int],
        size: CGFloat,
        isSelected: Bool
    ) -> AttributedString {
        var attributed = AttributedString()

        let characters = Array(text)
        let matched = Set(offsets)

        // The offsets are byte offsets, so they are walked byte by byte and
        // matched against the byte offset of each character.
        var byteOffset = 0

        for character in characters {
            var piece = AttributedString(String(character))

            if matched.contains(byteOffset) {
                piece.foregroundColor = Theme.accent
                piece.font = .system(size: size, weight: isSelected ? .bold : .semibold)
            }

            attributed.append(piece)
            byteOffset += String(character).utf8.count
        }

        return attributed
    }
}
