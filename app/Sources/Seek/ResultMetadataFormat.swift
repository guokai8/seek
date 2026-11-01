//
//  ResultMetadataFormat.swift
//  Seek
//
//  How a result row writes a file's size and the time it last changed.
//
//  Both are formatted per row, and a row is formatted on every keystroke.
//  The worst keystroke already costs about 130ms of ranking, so nothing
//  here is allowed to add to it: the formatters are built once and kept,
//  because constructing a `DateFormatter` costs more than every other
//  thing a row does put together. Only the visible rows are ever
//  formatted, because `LazyVStack` builds no others.
//

import Foundation

/// The formatters the result rows share.
///
/// Pinned to the main actor, which is where a row is drawn. A formatter
/// holds mutable state and is not `Sendable`, and one shared across
/// threads would be a data race rather than a saving.
@MainActor
enum ResultMetadataFormat {
    /// Sizes, in the units the Finder uses.
    private static let sizeFormatter: ByteCountFormatter = {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        formatter.allowsNonnumericFormatting = false
        return formatter
    }()

    /// Clock time, for something changed today.
    private static let today: DateFormatter = {
        let formatter = DateFormatter()
        formatter.setLocalizedDateFormatFromTemplate("j:mm")
        return formatter
    }()

    /// Day and month, for something changed this year.
    private static let thisYear: DateFormatter = {
        let formatter = DateFormatter()
        formatter.setLocalizedDateFormatFromTemplate("d MMM")
        return formatter
    }()

    /// Day, month and year, for anything older.
    ///
    /// Two digits of year, not four. The column is 68pt and a four-digit
    /// year does not fit in it: rendered, `Sep 27, 2025` came out as
    /// `Sep 27, 2...`, which is a date with the one part that made it
    /// worth showing cut off.
    private static let earlier: DateFormatter = {
        let formatter = DateFormatter()
        formatter.setLocalizedDateFormatFromTemplate("d MMM yy")
        return formatter
    }()

    /// A file size, written the way the Finder writes it.
    static func size(ofBytes bytes: UInt64) -> String {
        sizeFormatter.string(fromByteCount: Int64(clamping: bytes))
    }

    /// A timestamp, written as briefly as it can be read.
    ///
    /// The column is narrow and every row carries one, so the parts that
    /// the neighbouring rows would repeat are left out: today's files show
    /// the time, this year's the day and month, and only the older ones
    /// spend the width on a year.
    static func time(_ date: Date, now: Date = Date()) -> String {
        let calendar = Calendar.current

        if calendar.isDate(date, inSameDayAs: now) {
            return today.string(from: date)
        }
        if calendar.component(.year, from: date) == calendar.component(.year, from: now) {
            return thisYear.string(from: date)
        }
        return earlier.string(from: date)
    }
}

@MainActor
extension SearchHit {
    /// The size as the row shows it, or nil when there is none to show.
    ///
    /// A directory's `st_size` is the size of its own on-disk record — a
    /// few hundred bytes, the same for a folder of one file and a folder
    /// of ten thousand. Showing it would be showing a number that means
    /// nothing, so a directory row shows none.
    var sizeText: String? {
        guard !isDirectory else { return nil }
        return ResultMetadataFormat.size(ofBytes: byteSize)
    }

    /// The time the entry last changed, as the row shows it.
    var modifiedText: String? {
        guard let modified else { return nil }
        return ResultMetadataFormat.time(modified)
    }
}
