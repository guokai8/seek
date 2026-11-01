//! Leaving a match unscored when it cannot reach the rows that are shown.
//!
//! A query like `test` matches most of a home directory, and the panel
//! shows a few hundred rows. Nearly every match is therefore scored,
//! ranked and thrown away. Once enough candidates have been found at the
//! top of the scoring range, a candidate whose own ceiling is below them
//! can be dismissed without being scored at all — the bound that permits
//! that is pinned down in `score_bound.rs`.
//!
//! The risk is exactly the risk narrowing had: a candidate wrongly
//! dismissed is a row that silently never appears, and nothing in the
//! output says so. So these tests are differential. The rows the search
//! returns are compared, field by field, with the rows a plain sequential
//! ranking of the same paths gives — [`rank_candidates`] scores every
//! candidate and sorts the lot, so it cannot prune anything and makes an
//! honest oracle.
//!
//! `scored` is asserted alongside, because a test that only compared rows
//! would pass just as well if nothing were ever pruned.

use seek_core::corpus::Corpus;
use seek_core::fuzzy::{literal, rank_candidates, FuzzyMatcher, Scratch};
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, Query, SearchOptions};
use seek_core::search::search_in;

/// How many paths of each kind the fixture holds.
const MATCHES: usize = 4000;
/// One in this many matching paths puts the query right after a separator,
/// where it scores at the ceiling. The rest only hold its characters in
/// order, spread out, where they do not.
const EVERY: usize = 100;
/// Paths that do not match at all, so that the match set stays under half
/// the corpus and the narrowing pool is still built.
const FILLER: usize = 5000;
/// How many candidates reach the ceiling: one file in every `EVERY`, and
/// the `src/test` directory itself, whose own path is `src/test` and so
/// also holds the query right after a separator.
const AT_CEILING: usize = MATCHES / EVERY + 1;

/// A corpus built so that a query has far more matches than rows shown.
///
/// The ceiling-scoring paths are spread through the corpus rather than
/// grouped at the end. Where they sit decides how early the prune can
/// arm, and a fixture that put them last would describe a search that
/// never pruned anything.
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
            // `src/test/f…` — the run is consecutive and sits right after a
            // separator, which is the best `test` can do.
            index.add_named(Some(test), &format!("f{i}.rs"), EntryKind::File, 1, 0);
        } else {
            // The characters of `test` in order but never adjacent, so
            // every alignment pays at least one gap.
            index.add_named(
                Some(vendor),
                &format!("t-e-s-{i}t.md"),
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
fn a_broad_query_leaves_most_of_its_matches_unscored() {
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let options = options(10);

    let (outcome, _) = search_in(&corpus, &query, &options, None);

    // One more than the files: the `src/test` directory is an entry of its
    // own, and `test` matches it too.
    assert_eq!(
        outcome.matched,
        MATCHES + 1,
        "every matching path should count"
    );
    assert!(
        outcome.scored > 0,
        "something has to be scored, or there would be no rows to show"
    );
    // A loose bound on purpose. How early the prune arms depends on how
    // rayon splits the candidates across cores, which is not fixed, so the
    // exact number of scored candidates is not either. What has to hold is
    // that the great majority of the matches never got scored.
    assert!(
        outcome.scored * 2 < outcome.matched,
        "{} of {} matches were scored, so the prune did next to nothing",
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
    // A pruned candidate is still a match. The panel's counter is the one
    // number that moves when a query changes but its best rows do not, so
    // a prune that quietly stopped counting would be visible to the user
    // and invisible to the rows.
    let corpus = corpus();
    let paths = paths(&corpus);
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let options = options(10);

    let (outcome, _) = search_in(&corpus, &query, &options, None);

    let matcher = FuzzyMatcher::with_options(
        "test",
        options.case_sensitive,
        options.scheme,
        options.normalize,
    );
    let ranked = rank_candidates(matcher, &paths, &options.criteria);

    assert_eq!(outcome.matched, ranked.len());
}

#[test]
fn every_match_stays_in_the_pool() {
    // The narrowing pool is the next keystroke's candidate list. A pruned
    // candidate must still be in it: it matched, so the next query may
    // well want it, and by then it could be one of the rows shown.
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let options = options(10);

    let (outcome, pool) = search_in(&corpus, &query, &options, None);
    let pool = pool.expect("the match set is under half the corpus, so a pool is kept");

    assert_eq!(pool.len(), outcome.matched);
    assert!(
        outcome.scored < pool.len(),
        "the pool should hold more than was scored, or nothing was pruned"
    );
    assert!(
        pool.windows(2).all(|w| w[0] < w[1]),
        "the pool has to stay in ascending corpus order"
    );
}

#[test]
fn a_limit_above_the_ceiling_scorers_never_arms_the_prune() {
    // The prune arms only once `limit` candidates have been found scoring
    // higher than a gapped match can reach. A limit past the number of
    // such candidates can never arm it, and then every match has to be
    // scored the long way.
    //
    // That number is counted here rather than worked out by hand. The
    // fixture has `AT_CEILING` paths at the ceiling itself, but the
    // threshold is a gap below it, and getting that off by one is exactly
    // the kind of mistake that makes the assertion mean nothing.
    let corpus = corpus();
    let paths = paths(&corpus);
    let defaults = SearchOptions::default();
    let matcher = FuzzyMatcher::with_options(
        "test",
        defaults.case_sensitive,
        defaults.scheme,
        defaults.normalize,
    );
    let mut scratch = Scratch::new();
    let threshold = matcher.score_ceiling() - 2;
    let at_top = paths
        .iter()
        .filter(|p| matcher.match_text_fast(p, &mut scratch).score >= threshold)
        .count();

    assert_eq!(
        at_top, AT_CEILING,
        "the fixture should have no candidate between a gap below the ceiling and the ceiling"
    );

    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let (outcome, _) = search_in(&corpus, &query, &options(at_top + 1), None);

    assert_eq!(outcome.scored, outcome.matched);
}

#[test]
fn occurs_in_agrees_with_the_scoring_matcher() {
    // A pruned candidate is counted on this answer alone, so it has to be
    // the same answer the full matcher would have given.
    let corpus = corpus();
    let paths = paths(&corpus);
    let mut scratch = Scratch::new();
    let options = SearchOptions::default();

    for pattern in ["t", "test", "tst", "readme", "zq"] {
        let matcher =
            FuzzyMatcher::with_options(pattern, false, options.scheme, options.normalize);
        for path in &paths {
            assert_eq!(
                matcher.occurs_in(path),
                matcher.match_text_fast(path, &mut scratch).is_match(),
                "{pattern:?} against {path:?}"
            );
        }
    }
}

#[test]
fn an_accented_path_is_not_pruned() {
    // The substring test the prune uses works on bytes, while the matcher
    // folds accented letters the way fzf does. So `tést` holds a
    // consecutive run of `test` and scores at the ceiling — it belongs at
    // the very top — while the substring test says the path does not
    // contain the query at all. Pruning on that answer would drop the best
    // row there is, which is why the prune only ever looks at plain ASCII
    // paths.
    //
    // The search runs on one thread so that the order is the corpus order.
    // The accented path is added last, after the candidates that arm the
    // prune, which is the only arrangement that puts it on the pruning
    // path at all; left to several threads, whether it gets there is a
    // matter of how the work happened to be split.
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    let src = index.add_named(Some(root), "src", EntryKind::Directory, 0, 0);
    let test = index.add_named(Some(src), "test", EntryKind::Directory, 0, 0);
    for i in 0..200 {
        index.add_named(Some(test), &format!("f{i}.rs"), EntryKind::File, 1, 0);
    }

    // Then paths the prune will drop, so that `scored` can show it armed.
    let vendor = index.add_named(Some(root), "vendor", EntryKind::Directory, 0, 0);
    for i in 0..200 {
        index.add_named(
            Some(vendor),
            &format!("t-e-s-{i}t.md"),
            EntryKind::File,
            1,
            0,
        );
    }

    // Last, and short: it scores at the ceiling like the others, and the
    // `length` tiebreak then puts it first of all of them.
    let accented = index.add_named(Some(root), "tést", EntryKind::Directory, 0, 0);

    index.touch();
    let corpus = Corpus::build(&index);

    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let options = options(5);

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let (outcome, _) = pool.install(|| search_in(&corpus, &query, &options, None));

    assert!(
        outcome.scored < outcome.matched,
        "the prune has to have armed, or this test proves nothing"
    );
    assert_eq!(
        outcome.results[0].id, accented,
        "the accented path scores at the ceiling and is the shortest of those that do, \
         so it is the first row"
    );
}

#[test]
fn a_path_exactly_a_gap_below_the_ceiling_is_still_scored() {
    // The margin the prune arms on is two below the ceiling, not three,
    // and the difference is a row. A path without a consecutive run can
    // score exactly a gap below the ceiling — `a1/tes/t` does, because the
    // final `t` follows a separator and so keeps the full boundary bonus
    // despite the gap before it. If the margin were three, such a path
    // would both arm the prune and be prunable by it, and the ranking
    // among candidates on that score would come out wrong.
    //
    // Here nothing reaches the ceiling at all, so with the margin as it is
    // the prune never arms and all four candidates are scored. The
    // shortest of them is the first row.
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    // Longest first, so that with too wide a margin the three that arm the
    // prune are the ones that should rank last, and the row that goes
    // missing is the one that should rank first.
    let mut ids = Vec::new();
    for prefix in ["abcd1", "abc1", "ab1", "a1"] {
        let top = index.add_named(Some(root), prefix, EntryKind::Directory, 0, 0);
        let tes = index.add_named(Some(top), "tes", EntryKind::Directory, 0, 0);
        ids.push(index.add_named(Some(tes), "t", EntryKind::File, 1, 0));
    }

    for i in 0..50 {
        index.add_named(Some(root), &format!("readme-{i}.md"), EntryKind::File, 1, 0);
    }

    index.touch();
    let corpus = Corpus::build(&index);

    // The score this test turns on, asserted rather than assumed.
    let defaults = SearchOptions::default();
    let matcher = FuzzyMatcher::with_options(
        "test",
        defaults.case_sensitive,
        defaults.scheme,
        defaults.normalize,
    );
    let mut scratch = Scratch::new();
    assert_eq!(
        matcher.match_text_fast("a1/tes/t", &mut scratch).score,
        matcher.score_ceiling() - 3,
        "the fixture only tests the margin if this path sits exactly a gap below the ceiling"
    );

    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let options = options(3);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let (outcome, _) = pool.install(|| search_in(&corpus, &query, &options, None));

    assert_eq!(
        outcome.scored, outcome.matched,
        "nothing here reaches the ceiling, so the prune must not arm"
    );
    assert_eq!(
        outcome.results[0].id,
        *ids.last().unwrap(),
        "the shortest of the equally scoring paths is the first row"
    );
}

#[test]
fn a_query_of_several_terms_is_not_pruned() {
    // With several fuzzy terms the score is their sum, so it is no longer
    // bounded by any one term's ceiling, and the claim that a path without
    // a consecutive run of a term scores a gap below that ceiling says
    // nothing at all. The prune therefore leaves such a query alone.
    let corpus = corpus();
    let query = Query::parse("test rs", Matching::Fuzzy).unwrap();
    let options = options(5);

    let (outcome, _) = search_in(&corpus, &query, &options, None);

    assert!(outcome.matched > options.limit, "the query has to be broad");
    assert_eq!(outcome.scored, outcome.matched);
}

#[test]
fn for_one_character_containing_and_matching_are_the_same_question() {
    // Which is why the prune is left off for a one-character term: every
    // candidate it could dismiss is one that does not match at all, so it
    // would only ever pay a substring search per path to confirm a
    // non-match. Measured over a home directory, arming it for `s` cost
    // 9% and saved nothing.
    //
    // That makes the one-character guard a guard on cost, not on
    // correctness — removing it gives the same rows, only more slowly, and
    // no test of the results can catch its absence. This one states the
    // equivalence the guard rests on instead.
    let corpus = corpus();
    let paths = paths(&corpus);
    let defaults = SearchOptions::default();

    for pattern in ["t", "e", "z"] {
        let matcher =
            FuzzyMatcher::with_options(pattern, false, defaults.scheme, defaults.normalize);
        for path in &paths {
            assert_eq!(
                matcher.occurs_in(path),
                literal::contains(path, pattern, false),
                "{pattern:?} against {path:?}"
            );
        }
    }
}
