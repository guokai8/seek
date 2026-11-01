//! The streaming search: rows published while the search is still running.
//!
//! A search over a quarter of a million paths takes tens of milliseconds,
//! and the panel used to show nothing at all until it finished. A
//! streaming search hands over the best rows it has seen so far as it
//! goes, so the first of them appear in a millisecond or two.
//!
//! The ranking is global, which is what makes this worth testing rather
//! than merely worth writing: the best match can be the very last
//! candidate, so an intermediate publish is *provisional* and a later one
//! has to be allowed to replace it. The property that has to hold is
//! therefore not "the rows never change" but:
//!
//! * every publish is exactly the top rows of the candidates seen so far,
//!   so nothing is shown that a plain ranking of that prefix would not
//!   have shown; and
//! * the last publish, and the outcome, are what a plain search returns —
//!   streaming changes when rows appear, never which rows they are.
//!
//! The fixture puts the best matches at the *end* on purpose. With the
//! good rows first, the first publish would already be the final answer
//! and a merge that simply ignored every later chunk would pass.

use seek_core::corpus::Corpus;
use seek_core::fuzzy::{rank_candidates, FuzzyMatcher};
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, Query, SearchOptions, SearchResult};
use seek_core::search::{search_in, search_streaming, Progress};

/// How many scattered matches come first.
const SCATTERED: usize = 4000;
/// How many ceiling scorers come last.
const BEST: usize = 300;
/// The chunk the tests publish on.
const CHUNK: usize = 1000;

/// A corpus whose best matches are deliberately last.
///
/// The scattered names hold `test` in order but never adjacent, so they
/// match and score poorly. The ones under `best/` put it right after a
/// separator, where it scores at the ceiling. Padding sits between them so
/// that several chunks go by with nothing worth publishing.
fn corpus() -> Corpus {
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    let scattered = index.add_named(Some(root), "scattered", EntryKind::Directory, 0, 0);
    for i in 0..SCATTERED {
        index.add_named(
            Some(scattered),
            &format!("t-e-s-{i}-t.md"),
            EntryKind::File,
            1,
            0,
        );
    }

    let padding = index.add_named(Some(root), "padding", EntryKind::Directory, 0, 0);
    for i in 0..2000 {
        index.add_named(Some(padding), &format!("{i}.bin"), EntryKind::File, 1, 0);
    }

    let best = index.add_named(Some(root), "best", EntryKind::Directory, 0, 0);
    for i in 0..BEST {
        let dir = index.add_named(Some(best), &format!("d{i}"), EntryKind::Directory, 0, 0);
        index.add_named(Some(dir), "test.rs", EntryKind::File, 1, 0);
    }

    index.touch();
    Corpus::build(&index)
}

/// The options the tests search with.
fn options(limit: usize) -> SearchOptions {
    SearchOptions {
        limit,
        ..SearchOptions::default()
    }
}

/// Every path of the corpus, in corpus order.
fn paths(corpus: &Corpus) -> Vec<&str> {
    (0..corpus.len())
        .filter_map(|i| corpus.relative_path(i))
        .collect()
}

/// Collects every publish of a streaming search.
fn publishes(
    corpus: &Corpus,
    query: &Query,
    options: &SearchOptions,
) -> (Vec<Vec<SearchResult>>, Vec<usize>, seek_core::query::SearchOutcome) {
    let mut rows: Vec<Vec<SearchResult>> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let (outcome, _) = search_streaming(corpus, query, options, None, CHUNK, &mut |page, matched| {
        rows.push(page.to_vec());
        counts.push(matched);
        Progress::Continue
    });
    (rows, counts, outcome)
}

/// What a plain ranking of the first `prefix` paths would show.
fn oracle(corpus: &Corpus, pattern: &str, prefix: usize, limit: usize) -> Vec<u32> {
    let all = paths(corpus);
    let slice = &all[..prefix.min(all.len())];
    let defaults = SearchOptions::default();
    let matcher = FuzzyMatcher::with_options(pattern, false, defaults.scheme, defaults.normalize);
    rank_candidates(matcher, slice, &defaults.criteria)
        .into_iter()
        .take(limit)
        .map(|m| corpus.entries[m.index].id)
        .collect()
}

#[test]
fn the_best_matches_are_last_in_the_corpus() {
    // The precondition every other test here rests on. If the corpus were
    // built in another order the fixture would stop exercising the thing
    // it was built to exercise, and it would do so silently.
    let corpus = corpus();
    let all = paths(&corpus);
    let first_best = all
        .iter()
        .position(|p| p.starts_with("best/"))
        .expect("the fixture has ceiling scorers");

    assert!(
        first_best > SCATTERED,
        "the ceiling scorers start at {first_best}, not after the {SCATTERED} scattered ones"
    );
}

#[test]
fn a_broad_query_publishes_more_than_once() {
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let (rows, _, _) = publishes(&corpus, &query, &options(10));

    assert!(
        rows.len() > 1,
        "a search over {} paths in chunks of {CHUNK} published {} times",
        corpus.len(),
        rows.len()
    );
}

#[test]
fn every_publish_is_the_top_of_what_has_been_seen() {
    // The load-bearing test. Each publish must be exactly what a plain
    // ranking of the candidates consumed so far would show — not merely
    // "something reasonable", because a merge that dropped a chunk, or
    // kept the wrong side of a tie, would still look reasonable.
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let limit = 10;
    let (rows, _, _) = publishes(&corpus, &query, &options(limit));

    for (publish, page) in rows.iter().enumerate() {
        let prefix = (publish + 1) * CHUNK;
        let expected = oracle(&corpus, "test", prefix, limit);
        let actual: Vec<u32> = page.iter().map(|r| r.id).collect();

        assert_eq!(
            actual, expected,
            "publish {publish} covers the first {prefix} paths"
        );
    }
}

#[test]
fn the_last_publish_is_what_a_plain_search_returns() {
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let defaults = options(10);
    let (rows, _, outcome) = publishes(&corpus, &query, &defaults);
    let (plain, _) = search_in(&corpus, &query, &defaults, None);

    let last: Vec<u32> = rows.last().expect("at least one publish")
        .iter().map(|r| r.id).collect();
    let expected: Vec<u32> = plain.results.iter().map(|r| r.id).collect();

    assert_eq!(last, expected, "the last publish");

    let streamed: Vec<u32> = outcome.results.iter().map(|r| r.id).collect();
    assert_eq!(streamed, expected, "the outcome");
    assert_eq!(outcome.matched, plain.matched);
    assert_eq!(outcome.scanned, plain.scanned);
}

#[test]
fn the_rows_do_get_replaced() {
    // The counterpart of the test above: if the fixture or the chunking
    // ever stopped making later chunks matter, the property that each
    // publish is the top of its prefix would still hold — vacuously. This
    // insists that at least one publish actually supersedes another, so
    // the merge is being exercised rather than merely being correct about
    // nothing.
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let (rows, _, _) = publishes(&corpus, &query, &options(10));

    let first = rows.first().expect("at least one publish");
    let last = rows.last().expect("at least one publish");

    assert!(
        first[0].score < last[0].score,
        "the best row started at {} and ended at {}, so no later chunk ever won",
        first[0].score,
        last[0].score
    );
}

#[test]
fn the_running_count_only_grows_and_ends_complete() {
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let defaults = options(10);
    let (_, counts, outcome) = publishes(&corpus, &query, &defaults);

    for pair in counts.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "the match count went backwards: {} then {}",
            pair[0],
            pair[1]
        );
    }
    assert_eq!(
        counts.last().copied(),
        Some(outcome.matched),
        "the last publish's count is the final one"
    );
}

#[test]
fn the_callback_can_stop_the_search() {
    // What makes a superseded keystroke cheap. Without this a search the
    // user has already typed past runs to completion, and over a home
    // directory that is tens of milliseconds spent on an answer nobody
    // will see.
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let defaults = options(10);

    let mut seen = 0usize;
    let (stopped, _) = search_streaming(&corpus, &query, &defaults, None, CHUNK, &mut |_, _| {
        seen += 1;
        Progress::Stop
    });

    let (whole, _) = search_in(&corpus, &query, &defaults, None);

    assert_eq!(seen, 1, "the search kept going after it was told to stop");
    assert!(
        stopped.scanned < whole.scanned,
        "a stopped search scanned {} of {}",
        stopped.scanned,
        whole.scanned
    );
    assert!(
        stopped.matched < whole.matched,
        "a stopped search is not a complete answer, and its count must not pretend to be"
    );
}

#[test]
fn a_search_that_fits_in_one_chunk_publishes_once() {
    // A narrow query is answered in under a millisecond, and publishing it
    // in pieces would cost the app a SwiftUI pass per piece for nothing.
    let corpus = corpus();
    let query = Query::parse("test", Matching::Fuzzy).unwrap();
    let (rows, _, _) = search_streaming_rows(&corpus, &query, &options(10), corpus.len());

    assert_eq!(rows.len(), 1);
}

#[test]
fn a_plain_search_is_a_streaming_search_that_publishes_once() {
    // `search_in` is not a second implementation of the search; it is this
    // one with the whole corpus as a single chunk. The test says so, so
    // that the two cannot drift.
    let corpus = corpus();
    let defaults = options(25);

    for pattern in ["test", "e", "s", "best", "nothing-matches-this"] {
        let query = Query::parse(pattern, Matching::Fuzzy).unwrap();
        let (plain, plain_pool) = search_in(&corpus, &query, &defaults, None);
        let (streamed, streamed_pool) =
            search_streaming(&corpus, &query, &defaults, None, corpus.len(), &mut |_, _| {
                Progress::Continue
            });

        assert_eq!(plain.matched, streamed.matched, "{pattern:?} matched");
        assert_eq!(plain_pool, streamed_pool, "{pattern:?} pool");

        // Not `scored`. How many candidates escape being scored depends on
        // when the prune arms, and the counter that arms it is shared
        // between the workers — so the number moves with the thread
        // interleaving even for the same query over the same corpus. The
        // rows do not, which is the thing worth pinning, and the assertion
        // below pins it.
        assert!(plain.scored <= plain.matched, "{pattern:?}");
        assert!(streamed.scored <= streamed.matched, "{pattern:?}");

        let a: Vec<(u32, i32, Vec<i32>)> = plain
            .results
            .iter()
            .map(|r| (r.id, r.score, r.positions.clone()))
            .collect();
        let b: Vec<(u32, i32, Vec<i32>)> = streamed
            .results
            .iter()
            .map(|r| (r.id, r.score, r.positions.clone()))
            .collect();
        assert_eq!(a, b, "{pattern:?} rows");
    }
}

#[test]
fn the_pool_is_complete_although_only_the_top_rows_were_published() {
    // The next keystroke searches this query's pool, so a pool missing the
    // matches that never made it into a publish would lose rows as the
    // query grew — the failure the narrowing tests exist to catch, arriving
    // by a new route.
    let corpus = corpus();
    // Not `test`: it matches more than half the corpus, and a pool that
    // large is deliberately not taken. `test.rs` matches only the ceiling
    // scorers, which is a pool, and still far more than the ten rows a
    // publish shows.
    let query = Query::parse("test.rs", Matching::Fuzzy).unwrap();
    let defaults = options(10);

    let (outcome, pool) = search_streaming(&corpus, &query, &defaults, None, CHUNK, &mut |_, _| {
        Progress::Continue
    });
    let (_, plain_pool) = search_in(&corpus, &query, &defaults, None);

    assert_eq!(pool, plain_pool);
    assert!(
        outcome.matched > defaults.limit,
        "the fixture has to match more than one page, or the test is vacuous"
    );
    assert_eq!(
        pool.as_ref().map(|p| p.len()),
        Some(outcome.matched),
        "the pool holds every match, not just the ones shown"
    );
}

#[test]
fn a_stopped_search_hands_back_no_pool() {
    // The pool is the query's *complete* match set, and the next keystroke
    // searches it instead of the corpus. A stopped search holds only part
    // of that set, so returning it would make the next keystroke lose every
    // row the search never reached — a row that silently stops appearing,
    // with nothing in the output to say why. The next keystroke goes back
    // to the corpus instead: slower, and right.
    let corpus = corpus();
    let query = Query::parse("test.rs", Matching::Fuzzy).unwrap();
    let defaults = options(10);

    let (complete, pool) = search_streaming(&corpus, &query, &defaults, None, CHUNK, &mut |_, _| {
        Progress::Continue
    });
    assert!(pool.is_some(), "a search that ran to the end has a pool");

    let (stopped, no_pool) =
        search_streaming(&corpus, &query, &defaults, None, CHUNK, &mut |_, _| Progress::Stop);

    assert_eq!(no_pool, None);
    assert!(
        stopped.matched < complete.matched,
        "the fixture has to stop short of the whole corpus, or the test is vacuous: \
         {} of {}",
        stopped.matched,
        complete.matched
    );
}

/// Runs a streaming search and returns its publishes, for a chunk size the
/// caller picks.
fn search_streaming_rows(
    corpus: &Corpus,
    query: &Query,
    options: &SearchOptions,
    chunk: usize,
) -> (Vec<Vec<SearchResult>>, Vec<usize>, seek_core::query::SearchOutcome) {
    let mut rows = Vec::new();
    let mut counts = Vec::new();
    let (outcome, _) = search_streaming(corpus, query, options, None, chunk, &mut |page, matched| {
        rows.push(page.to_vec());
        counts.push(matched);
        Progress::Continue
    });
    (rows, counts, outcome)
}
