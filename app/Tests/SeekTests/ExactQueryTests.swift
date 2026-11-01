//
//  ExactQueryTests.swift
//  Seek
//
//  What a bare term means, and how the panel switches it.
//
//  A bare term is an exact substring: `nihao` is those five characters
//  with nothing between them. fzf's default is the other way round — a
//  bare term is a subsequence, so `fzf --filter=nihao` returns
//  `Hao Law NIW EB1 Documents List Please Read.pdf` and 174,503 other
//  paths — and this panel takes fzf's `--exact` as its default instead.
//  A launcher is opened to type three letters and press return, and a
//  list of a hundred and seventy thousand rows is not an answer.
//
//  Both are a keystroke away. fzf's `'` prefix asks for the other mode
//  for one term, and ⌘E switches the panel's own, which is what the
//  bottom test here is about: the text in the field means something else
//  afterwards, so the list cannot be left as it was.
//

import XCTest

@testable import Seek

@MainActor
final class ExactQueryTests: XCTestCase {
    /// The model these tests share.
    ///
    /// One per class, not one per test. Every `AppModel` scans the whole
    /// home directory and leaves a filesystem watcher running behind it,
    /// and three of them for three queries was enough to starve the
    /// tests that run after this class — `PanelRetypingTests` started
    /// finding nothing, in a full run only, for a reason that had
    /// nothing to do with it.
    private static var sharedModel: AppModel?

    override func tearDown() async throws {
        // The mode is a stored preference, so a test that switched it
        // would otherwise hand the next class a panel in fuzzy mode.
        Self.sharedModel?.setExactMatching(true)
        Preferences.shared.exactMatch = true
    }

    /// A model with a real index behind it.
    private func indexed() async throws -> AppModel {
        if let model = Self.sharedModel {
            return model
        }

        let model = AppModel()
        Self.sharedModel = model
        let deadline = Date().addingTimeInterval(180)

        while model.isIndexing && Date() < deadline {
            try await Task.sleep(for: .milliseconds(100))
        }

        XCTAssertGreaterThan(model.indexedCount, 1000, "the index is too small to mean anything")

        return model
    }

    /// Runs `query` and waits for the answer.
    private func search(_ query: String, in model: AppModel) async throws {
        model.query = query
        try await settle(model)
    }

    /// Waits for the search in flight to finish.
    private func settle(_ model: AppModel) async throws {
        let deadline = Date().addingTimeInterval(20)
        while model.isSearching && Date() < deadline {
            try await Task.sleep(for: .milliseconds(50))
        }
    }

    func test_a_bare_term_matches_the_characters_together() async throws {
        let model = try await indexed()

        // Not `nihao`: no path in this index spells it, so an empty list
        // would be the right answer and would prove nothing. `halffloat`
        // is a real file under numpy, and fuzzy matching finds plenty
        // besides it.
        try await search("halffloat", in: model)

        XCTAssertNil(model.queryError, "the term came back as an error")
        XCTAssertFalse(model.results.isEmpty, "the exact query found nothing at all")

        for hit in model.results {
            XCTAssertTrue(
                hit.relativePath.lowercased().contains("halffloat"),
                "\(hit.relativePath) has no \"halffloat\" in it, so the bare term is not exact"
            )
        }
    }

    func test_a_leading_quote_asks_for_the_other_mode() async throws {
        let model = try await indexed()

        try await search("'nihao", in: model)

        XCTAssertNil(model.queryError, "a leading quote came back as an error")
        XCTAssertFalse(model.results.isEmpty, "the fuzzy query found nothing")

        // Loose on purpose, and far looser than the bare term: this is
        // what `fzf --filter=nihao` does with the same pattern.
        let scattered = model.results.contains { !$0.relativePath.lowercased().contains("nihao") }

        XCTAssertTrue(
            scattered,
            "every result spells \"nihao\" outright, so the quote did not reach fuzzy matching"
        )
    }

    func test_the_bare_term_is_narrower_than_the_quoted_one() async throws {
        let model = try await indexed()

        try await search("nihao", in: model)
        let exact = model.matchedCount

        try await search("'nihao", in: model)
        let fuzzy = model.matchedCount

        XCTAssertLessThan(exact, fuzzy, "quoting the term matched as little as not quoting it")
    }

    func test_switching_the_mode_recomputes_the_query_on_screen() async throws {
        let model = try await indexed()

        // What the text means in each mode, asked for one at a time. The
        // quoted form is the control: it is the same matching the switch
        // is about to turn on, so the two have to agree afterwards.
        try await search("nihao", in: model)
        let exact = model.matchedCount

        try await search("'nihao", in: model)
        let fuzzy = model.matchedCount

        XCTAssertLessThan(exact, fuzzy, "the two modes answer this query the same way")

        // Back to the bare term, which is where the user would be.
        try await search("nihao", in: model)
        XCTAssertEqual(model.matchedCount, exact)

        model.toggleExactMatching()

        XCTAssertFalse(model.isExactMatching, "the switch did not take")
        XCTAssertTrue(
            model.isSearching,
            "nothing was recomputed: the switch left the panel showing the answer to the "
                + "question the old rule was asked"
        )

        try await settle(model)

        XCTAssertEqual(
            model.matchedCount,
            fuzzy,
            "the query on screen was not answered again under the new rule"
        )
    }

    func test_the_mode_is_remembered() async throws {
        let model = try await indexed()

        model.setExactMatching(false)
        XCTAssertFalse(Preferences.shared.exactMatch, "the switch was not written to the settings")

        model.setExactMatching(true)
        XCTAssertTrue(Preferences.shared.exactMatch)
    }
}
