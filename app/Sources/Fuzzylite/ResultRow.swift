//
//  ResultRow.swift
//  Fuzzylite
//
//  A single row in the results list, with the matched characters
//  highlighted the way fzf highlights them.
//

import SwiftUI

/// One row of the results list.
struct ResultRow: View {
    /// The result to display.
    let hit: SearchHit

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: hit.isDirectory ? "folder" : "doc")
                .font(.system(size: 12))
                .foregroundStyle(hit.isDirectory ? Color.accentColor : Color.secondary)
                .frame(width: 16)

            Text(highlightedName)
                .font(.system(size: 13))
                .lineLimit(1)
                .truncationMode(.middle)

            Text(hit.directory)
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .lineLimit(1)
                .truncationMode(.head)

            Spacer(minLength: 0)
        }
        .padding(.vertical, 1)
    }

    /// The file name with the matched characters emphasised.
    ///
    /// The offsets come from the matcher as byte offsets, so they are
    /// converted to character offsets before they are used to split the
    /// string.
    private var highlightedName: AttributedString {
        var attributed = AttributedString()

        let characters = Array(hit.name)
        let matched = Set(hit.positions)

        // The offsets are byte offsets, so they are walked byte by byte and
        // matched against the byte offset of each character.
        var byteOffset = 0

        for character in characters {
            var piece = AttributedString(String(character))

            if matched.contains(byteOffset) {
                piece.foregroundColor = .accentColor
                piece.font = .system(size: 13, weight: .semibold)
            }

            attributed.append(piece)
            byteOffset += String(character).utf8.count
        }

        return attributed
    }
}
