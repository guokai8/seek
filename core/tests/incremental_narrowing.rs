//! Narrowing a search to what the previous query matched.
//!
//! When a query extends the one before it, every path it can match is
//! already in the previous query's result set, so the search can run over
//! that set instead of the whole corpus. This is where fzf's "typing gets
//! faster" comes from, and it is also the single easiest way to break
//! ranking: a candidate missing from the pool is a result that silently
//! never appears, with nothing in the output to say so.
//!
//! So these tests are differential. Every narrowed search is compared with
//! the same search run cold over the whole corpus — the ids, the scores,
//! the match spans, the highlighted positions, the order, and the match
//! count — and the pool is carried from one keystroke to the next, the way
//! the engine carries it, so that a pool computed *from* a pool is checked
//! too.
//!
//! `scanned` is asserted alongside, because a test that only compared the
//! results would still pass if the pool were quietly ignored.

use seek_core::corpus::Corpus;
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, narrows, Query, SearchOptions, SearchOutcome};
use seek_core::search::search_in;

/// A corpus of a few thousand nested paths, built deterministically.
///
/// The names have to be as varied as a real tree's. An earlier version of
/// this fixture used eight directory names that were all built from `s`,
/// `r` and `c`, which made `src` match 57% of it — and a pool that size is
/// deliberately not kept, so the tests measured a path that never narrowed.
/// Here `src` matches about a quarter, which is what a two or three
/// character query matches over a real home directory.
fn corpus() -> Corpus {
    const DIRS: [&str; 8] = [
        "src", "tests", "docs", "build", "vendor", "examples", "node_modules", "target",
    ];
    const NAMES: [&str; 10] = [
        "main.rs", "index.html", "package.json", "LICENSE", "notes.txt",
        "bundle.min.js", "app.py", "data.csv", "image.png", "Makefile",
    ];

    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    // A linear congruential generator, so the tree is the same on every
    // run but not uniform: a uniform tree hides ordering bugs, because
    // every candidate ties with every other one.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };

    for a in DIRS {
        let top = index.add_named(Some(root), a, EntryKind::Directory, 0, 0);
        for b in DIRS {
            let mid = index.add_named(Some(top), b, EntryKind::Directory, 0, 0);
            for i in 0..NAMES.len() {
                let name = NAMES[next() % NAMES.len()];
                index.add_named(Some(mid), &format!("{i}_{name}"), EntryKind::File, 1, 0);
            }
        }
    }

    index.touch();
    Corpus::build(&index)
}

fn options() -> SearchOptions {
    SearchOptions {
        limit: 200,
        ..Default::default()
    }
}

/// Asserts that two outcomes are the same answer, field by field.
fn assert_same_answer(narrowed: &SearchOutcome, cold: &SearchOutcome, what: &str) {
    assert_eq!(narrowed.matched, cold.matched, "{what}: match count");
    assert_eq!(
        narrowed.results.len(),
        cold.results.len(),
        "{what}: row count"
    );

    for (i, (got, want)) in narrowed.results.iter().zip(&cold.results).enumerate() {
        assert_eq!(got.id, want.id, "{what}: row {i} is a different entry");
        assert_eq!(got.score, want.score, "{what}: row {i} score");
        assert_eq!(got.start, want.start, "{what}: row {i} match start");
        assert_eq!(got.end, want.end, "{what}: row {i} match end");
        assert_eq!(
            got.positions, want.positions,
            "{what}: row {i} highlighted positions"
        );
    }
}

#[test]
fn a_typed_sequence_narrows_to_the_same_answer_as_a_cold_search() {
    let corpus = corpus();
    let options = options();

    let typed = ["s", "sr", "src", "src ", "src t", "src te", "src tes"];

    let mut previous: Option<(Query, Vec<u32>)> = None;
    let mut narrowed_keystrokes = 0;

    for text in typed {
        let query = Query::parse(text, Matching::Fuzzy).unwrap();

        let pool = previous
            .as_ref()
            .filter(|(before, _)| narrows(before, &query))
            .map(|(_, pool)| pool.as_slice());

        if pool.is_some() {
            narrowed_keystrokes += 1;
        }

        let (narrowed, carried) = search_in(&corpus, &query, &options, pool);
        let (cold, _) = search_in(&corpus, &query, &options, None);

        assert_same_answer(&narrowed, &cold, text);

        // The pool is the whole match set, not the page that was shown.
        if let Some(carried) = carried.as_ref() {
            assert_eq!(
                carried.len(),
                cold.matched,
                "{text}: the pool should hold every match"
            );
        }

        previous = carried.map(|carried| (query, carried));
    }

    // Without this the test would pass just as happily if every keystroke
    // had been answered from the corpus.
    assert!(
        narrowed_keystrokes >= 4,
        "only {narrowed_keystrokes} of the keystrokes narrowed"
    );
}

/// Types `keys` through, carrying the pool, and checks every keystroke
/// against the same search run cold.
fn differential_sequence(matching: Matching, keys: &[&str], least_narrowed: usize) {
    let corpus = corpus();
    let options = options();

    let mut previous: Option<(Query, Vec<u32>)> = None;
    let mut narrowed_keystrokes = 0;

    for text in keys {
        let query = Query::parse(text, matching).unwrap();

        let pool = previous
            .as_ref()
            .filter(|(before, _)| narrows(before, &query))
            .map(|(_, pool)| pool.as_slice());

        if pool.is_some() {
            narrowed_keystrokes += 1;
        }

        let (narrowed, carried) = search_in(&corpus, &query, &options, pool);
        let (cold, _) = search_in(&corpus, &query, &options, None);

        assert_same_answer(&narrowed, &cold, text);

        if let Some(carried) = carried.as_ref() {
            assert_eq!(
                carried.len(),
                cold.matched,
                "{text}: the pool should hold every match"
            );
        }

        previous = carried.map(|carried| (query, carried));
    }

    assert!(
        narrowed_keystrokes >= least_narrowed,
        "only {narrowed_keystrokes} of the keystrokes narrowed, wanted {least_narrowed}"
    );
}

#[test]
fn a_typed_sequence_in_exact_mode_narrows_to_the_same_answer() {
    // The panel's own mode, where a bare term is a substring. It reaches
    // the pool for a different reason than the fuzzy one does — dropping
    // the last character of a substring leaves a substring — and it
    // reaches it far more often, because a substring matches less.
    differential_sequence(
        Matching::Exact,
        &["s", "sr", "src", "src ", "src m", "src ma", "src mai"],
        4,
    );
}

#[test]
fn a_typed_prefix_term_narrows_to_the_same_answer() {
    // `^src` → `^src/` → `^src/t`: the anchored term grows at the end
    // that is free, so each keystroke can only match fewer paths.
    differential_sequence(
        Matching::Exact,
        &["^s", "^sr", "^src", "^src/", "^src/t", "^src/te"],
        4,
    );
}

#[test]
fn a_typed_sequence_that_switches_mode_is_not_narrowed() {
    // `'` asks for the other matching, so the pool built for the bare
    // term holds the wrong set. The guard is `narrows`, and this is the
    // differential check that it really refuses.
    let corpus = corpus();
    let options = options();

    let bare = Query::parse("src", Matching::Exact).unwrap();
    let (_, pool) = search_in(&corpus, &bare, &options, None);
    let pool = pool.expect("`src` is selective enough to leave a pool");

    let quoted = Query::parse("'src", Matching::Exact).unwrap();
    assert!(!narrows(&bare, &quoted), "the kinds differ, so the pool is stale");

    // And it is not merely a conservative refusal: the answers differ.
    let (from_pool, _) = search_in(&corpus, &quoted, &options, Some(&pool));
    let (cold, _) = search_in(&corpus, &quoted, &options, None);

    assert!(
        from_pool.matched < cold.matched,
        "the pool should be missing the paths only a subsequence reaches"
    );
}

#[test]
fn the_narrowed_search_really_looks_at_less() {
    // Without this the test above would pass just as happily if the pool
    // were accepted and then ignored.
    let corpus = corpus();
    let options = options();

    let first = Query::parse("src", Matching::Fuzzy).unwrap();
    let (_, pool) = search_in(&corpus, &first, &options, None);
    let pool = pool.expect("`src` is selective enough to leave a pool");

    let second = Query::parse("src test", Matching::Fuzzy).unwrap();
    assert!(narrows(&first, &second));

    let (narrowed, _) = search_in(&corpus, &second, &options, Some(&pool));
    let (cold, _) = search_in(&corpus, &second, &options, None);

    assert_eq!(cold.scanned, corpus.len(), "a cold search sees everything");
    assert_eq!(
        narrowed.scanned,
        pool.len(),
        "a narrowed search sees the pool"
    );
    assert!(
        narrowed.scanned < cold.scanned,
        "the pool ({}) should be smaller than the corpus ({})",
        narrowed.scanned,
        cold.scanned
    );
}

#[test]
fn the_pool_is_in_corpus_order() {
    // The last of fzf's tiebreaks is the position in the input, which here
    // is the position in the corpus. The pool is iterated in the order it
    // is stored, so an unsorted pool would reorder candidates that tie on
    // every other criterion — a difference the comparison above would
    // catch only when such a tie happened to exist.
    let corpus = corpus();
    let query = Query::parse("src", Matching::Fuzzy).unwrap();

    let (_, pool) = search_in(&corpus, &query, &options(), None);
    let pool = pool.expect("`src` is selective enough to leave a pool");

    assert!(pool.len() > 1, "the query should match something");
    assert!(
        pool.windows(2).all(|w| w[0] < w[1]),
        "the pool should be strictly ascending"
    );

    // And a pool derived from a pool keeps the order.
    let narrower = Query::parse("src t", Matching::Fuzzy).unwrap();
    let (_, carried) = search_in(&corpus, &narrower, &options(), Some(&pool));
    let carried = carried.expect("`src t` is selective too");

    assert!(
        carried.windows(2).all(|w| w[0] < w[1]),
        "a pool built from a pool should be ascending too"
    );
}

#[test]
fn an_empty_pool_stays_empty() {
    // A query that matched nothing leaves an empty pool, and a longer
    // query cannot match more than nothing. The search must answer with
    // nothing rather than fall back to the whole corpus.
    let corpus = corpus();
    let options = options();

    let first = Query::parse("zzqq", Matching::Fuzzy).unwrap();
    let (outcome, pool) = search_in(&corpus, &first, &options, None);
    let pool = pool.expect("a query that matched nothing still leaves a pool");
    assert_eq!(outcome.matched, 0);
    assert!(pool.is_empty());

    let second = Query::parse("zzqqx", Matching::Fuzzy).unwrap();
    let (narrowed, _) = search_in(&corpus, &second, &options, Some(&pool));

    assert_eq!(narrowed.matched, 0);
    assert_eq!(narrowed.scanned, 0, "nothing should have been looked at");
}

#[test]
fn a_pool_that_covers_most_of_the_corpus_is_not_worth_keeping() {
    // A fuzzy term of one or two characters matches almost everything: `c`
    // over a home directory matches 208,000 of 208,156 paths. A pool that
    // size saves nothing on the next keystroke — it was measured at two
    // tenths of a millisecond on a ten millisecond search — while building
    // it allocates four bytes per match and copies them. So it is not
    // built, and the next keystroke goes back to the corpus, which is what
    // it would have done anyway.
    let corpus = corpus();
    let options = options();

    // `e` is a subsequence of nearly every path in the fixture.
    let broad = Query::parse("e", Matching::Fuzzy).unwrap();
    let (outcome, pool) = search_in(&corpus, &broad, &options, None);

    assert!(
        outcome.matched > corpus.len() / 2,
        "the fixture should make this query a broad one: {} of {}",
        outcome.matched,
        corpus.len()
    );
    assert!(
        pool.is_none(),
        "a pool covering {} of {} paths should not have been kept",
        outcome.matched,
        corpus.len()
    );

    // A selective query still gets one.
    let narrow = Query::parse("src te", Matching::Fuzzy).unwrap();
    let (outcome, pool) = search_in(&corpus, &narrow, &options, None);

    assert!(outcome.matched > 0, "the query should match something");
    assert!(
        outcome.matched <= corpus.len() / 2,
        "the query should be selective"
    );
    assert_eq!(
        pool.map(|pool| pool.len()),
        Some(outcome.matched),
        "a selective query's pool should hold every match"
    );
}

