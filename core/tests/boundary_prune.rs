//! Dismissing a match that holds the query in the middle of a word.
//!
//! `top_rows_pruning.rs` covers the bound that dismisses a path with no
//! consecutive run of the query. That one leaves the hardest case
//! untouched: a common two-letter query like `te` is held as a consecutive
//! run by almost everything it matches, so almost nothing can be
//! dismissed, and a home directory's 196,680 matches get scored to show
//! two hundred.
//!
//! The sharper bound — a run away from a separator cannot reach the
//! ceiling either, see `score_bound.rs` — is what reaches those. These
//! tests are over a corpus where that is the whole population: every
//! match holds the query, and only a handful hold it where it scores.
//!
//! The risk is the same risk, so the test is the same kind: the rows are
//! compared field by field with a plain sequential ranking of the same
//! paths, which prunes nothing and so makes an honest oracle.

use seek_core::corpus::Corpus;
use seek_core::fuzzy::{literal, rank_candidates, FuzzyMatcher, Scratch};
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, Query, SearchOptions};
use seek_core::search::search_in;

/// How many matching paths the fixture holds.
const MATCHES: usize = 4040;
/// One in this many puts the query right after a separator, where it
/// scores at the ceiling. The rest hold it in the middle of a word.
const EVERY: usize = 101;
/// Paths that do not match at all, so the match set stays under half the
/// corpus and the narrowing pool is still built.
const FILLER: usize = 5000;

/// A corpus where every match holds the query as a consecutive run.
///
/// `latest-N.md` holds `test` at the fourth character of `latest`, after
/// an `a` — a run, so the older bound has to score it, and one bonus per
/// character short of the ceiling, so this one does not. The ceiling
/// scorers are spread through the corpus rather than grouped, because
/// where they sit decides how early the prune can arm.
fn corpus() -> Corpus {
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    let src = index.add_named(Some(root), "src", EntryKind::Directory, 0, 0);
    let test = index.add_named(Some(src), "test", EntryKind::Directory, 0, 0);
    let vendor = index.add_named(Some(root), "vendor", EntryKind::Directory, 0, 0);
    let docs = index.add_named(Some(root), "docs", EntryKind::Directory, 0, 0);

    for i in 0..MATCHES {
        if i % EVERY == 0 {
            index.add_named(Some(test), &format!("f{i}.rs"), EntryKind::File, 1, 0);
        } else {
            index.add_named(
                Some(vendor),
                &format!("latest-{i}.md"),
                EntryKind::File,
                1,
                0,
            );
        }
    }

    for i in 0..FILLER {
        // No `t`, so `test` cannot match these at all.
        index.add_named(Some(docs), &format!("readme-{i}.md"), EntryKind::File, 1, 0);
    }

    index.touch();
    Corpus::build(&index)
}

fn options(limit: usize) -> SearchOptions {
    SearchOptions {
        limit,
        ..Default::default()
    }
}

/// Every path of the corpus, in corpus order.
fn paths(corpus: &Corpus) -> Vec<&str> {
    (0..corpus.len())
        .filter_map(|i| corpus.relative_path(i))
        .collect()
}

#[test]
fn the_fixture_really_does_hold_the_query_everywhere() {
    // The precondition. If the matches stopped containing `test`, this
    // whole file would be re-testing `top_rows_pruning.rs` instead, and
    // the tests below would pass without the sharper bound existing.
    let corpus = corpus();
    let options = options(10);
    let matcher = FuzzyMatcher::with_options("test", false, options.scheme, options.normalize);
    let mut scratch = Scratch::new();
    let mut matched = 0usize;
    let mut containing = 0usize;

    for path in paths(&corpus) {
        if !matcher.match_text_fast(path, &mut scratch).is_match() {
            continue;
        }
        matched += 1;
        if literal::contains(path, "test", false) {
            containing += 1;
        }
    }

    assert_eq!(
        containing, matched,
        "every match has to hold the query as a run, or the older bound would do this work"
    );
    assert!(matched > 4000, "only {matched} matches");
}

#[test]
fn a_query_held_in_the_middle_of_a_word_is_left_unscored() {
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let options = options(10);

    let (outcome, _) = search_in(&corpus, &query, &options, None);

    assert!(
        outcome.matched > 4000,
        "{} matches, so the fixture is not what these tests assume",
        outcome.matched
    );
    assert!(
        outcome.scored > 0,
        "something has to be scored, or there would be no rows to show"
    );
    // Loose on purpose: how early the prune arms depends on how rayon
    // splits the work, so the exact count is not fixed. What has to hold
    // is that the majority went unscored — with only the consecutive-run
    // bound, every single match here is scored.
    assert!(
        outcome.scored * 2 < outcome.matched,
        "{} of {} matches were scored, so the boundary bound did next to nothing",
        outcome.scored,
        outcome.matched
    );
}

#[test]
fn the_rows_shown_are_the_rows_a_plain_ranking_gives() {
    let corpus = corpus();
    let paths = paths(&corpus);
    let query = Query::parse("test", Matching::Fuzzy).unwrap();

    for limit in [1, 10, 200] {
        let options = options(limit);
        let (outcome, _) = search_in(&corpus, &query, &options, None);

        // The oracle scores every candidate and sorts all of them, so it
        // has nothing to prune. Its tiebreak chain and scan direction are
        // the search's own.
        let matcher = FuzzyMatcher::with_options(
            "test",
            options.case_sensitive,
            options.scheme,
            options.normalize,
        );
        let ranked = rank_candidates(matcher, &paths, &options.criteria);

        assert_eq!(outcome.results.len(), limit.min(ranked.len()));

        for (row, (got, want)) in outcome.results.iter().zip(&ranked).enumerate() {
            let path = corpus.relative_path(want.index).unwrap();
            assert_eq!(
                got.id, corpus.entries[want.index].id,
                "row {row} of limit {limit} is a different entry; expected {path:?}"
            );
            assert_eq!(got.score, want.score, "row {row} of limit {limit}: score");
            assert_eq!(got.start, want.start, "row {row} of limit {limit}: start");
            assert_eq!(got.end, want.end, "row {row} of limit {limit}: end");
            assert_eq!(
                got.positions, want.positions,
                "row {row} of limit {limit}: highlighted positions"
            );
        }
    }
}

#[test]
fn the_match_count_is_not_touched_by_the_prune() {
    // A dismissed candidate still matched, and the footer counts matches.
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();

    let (pruning, _) = search_in(&corpus, &query, &options(10), None);
    // A limit above the number of ceiling scorers never arms the prune, so
    // this one scores everything and its count is the honest one.
    let (plain, _) = search_in(&corpus, &query, &options(100_000), None);

    assert_eq!(pruning.matched, plain.matched);
    assert_eq!(
        plain.scored, plain.matched,
        "with a limit that large nothing should be pruned"
    );
}

#[test]
fn every_match_stays_in_the_pool() {
    // The pool is the next keystroke's corpus. A candidate dismissed
    // without being scored still has to be in it, or extending the query
    // would silently lose rows.
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();

    let (outcome, pool) = search_in(&corpus, &query, &options(10), None);
    let pool = pool.expect("the match set is well under half the corpus");

    assert_eq!(pool.len(), outcome.matched);
}
