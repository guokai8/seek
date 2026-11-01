//! Throws random text at the query parser, the matchers and the search,
//! and reports anything that panics.
//!
//! The suite's tests ask whether the right answer comes back. This asks
//! the cruder question — whether *an* answer comes back — over inputs
//! nobody would think to write down: an operator with nothing after it,
//! a pattern longer than the path, a haystack that is one combining
//! mark, a term made of quotes.
//!
//! Usage: `cargo run --release --example fuzz -- [rounds]`

use std::panic::{catch_unwind, AssertUnwindSafe};

use seek_core::corpus::Corpus;
use seek_core::fuzzy::{ExactMatcher, FuzzyMatcher, Scheme, Scratch};
use seek_core::index::{EntryKind, Index};
use seek_core::query::{narrows, Matching, Query, SearchOptions};
use seek_core::search::{search, search_in};

/// A small deterministic generator, so a failure can be replayed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// The pieces a query is built from, operators over-represented on
/// purpose: that is where the parser makes decisions.
const PIECES: &[&str] = &[
    "^", "$", "!", "'", "*", "?", "re:", "content:", " ", "  ", "/", ".", "-", "a", "ab", "src",
    "README", "背景", "é", "\u{0301}", "\u{200b}", "\\", "[", "]", "(", ")", "|", "{", "}", "+",
    "..", "**", "''", "^$", "$^", "!!", "'''", "re:(", "\t", "\u{1F600}", "ß", "İ",
];

/// Paths the search runs over: the shapes that break offset arithmetic.
const PATHS: &[&str] = &[
    "a",
    "a/b",
    "src/main.rs",
    "src/index/rs",
    "背景/文档.txt",
    "é/résumé.pdf",
    "  leading.txt",
    "trailing.txt  ",
    "UPPER/Case.MD",
    "x/.hidden",
    "one.two.three.tar.gz",
    "\u{1F600}/emoji.txt",
    "i\u{0307}/combining.txt",
    "very/deep/nested/tree/of/directories/with/a/file.rs",
];

fn text(rng: &mut Rng, pieces: usize) -> String {
    (0..pieces).map(|_| *rng.pick(PIECES)).collect()
}

fn corpus() -> Corpus {
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/root".into());

    for path in PATHS {
        let parts: Vec<&str> = path.split('/').collect();
        let mut parent = root;
        for (i, part) in parts.iter().enumerate() {
            let kind = if i == parts.len() - 1 {
                EntryKind::File
            } else {
                EntryKind::Directory
            };
            parent = index.add_named(Some(parent), part, kind, 1, 0);
        }
    }

    index.touch();
    Corpus::build(&index)
}

fn main() {
    let rounds: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(200_000);

    let corpus = corpus();
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    let mut failures = 0usize;
    let mut parsed_ok = 0usize;

    std::panic::set_hook(Box::new(|_| {}));

    for round in 0..rounds {
        let seed = rng.0;
        let pieces = 1 + rng.below(5);
        let query = text(&mut rng, pieces);
        let matching = if rng.below(2) == 0 {
            Matching::Exact
        } else {
            Matching::Fuzzy
        };

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let Ok(parsed) = Query::parse(&query, matching) else {
                return false;
            };

            for scheme in [Scheme::Default, Scheme::Path] {
                let options = SearchOptions {
                    scheme,
                    criteria: scheme.criteria().to_vec(),
                    limit: 1 + (seed as usize % 8),
                    ..Default::default()
                };
                let _ = search(&corpus, &parsed, &options);
            }

            true
        }));

        match outcome {
            Ok(true) => parsed_ok += 1,
            Ok(false) => {}
            Err(_) => {
                failures += 1;
                println!("PANIC round {round}, seed {seed:#x}, query {query:?} {matching:?}");
                if failures > 20 {
                    break;
                }
            }
        }
    }

    // Narrowing, differentially: whenever the rule says the pool may be
    // reused, the answer from the pool has to be the answer from the
    // corpus. This is the one place where a wrong answer is silent —
    // a missing candidate looks exactly like a candidate that did not
    // match.
    let mut narrowing_rounds = 0usize;
    let mut narrowing_failures = 0usize;

    for _ in 0..rounds {
        let seed = rng.0;
        let pieces = 1 + rng.below(3);
        let first = text(&mut rng, pieces);
        let grown = format!("{first}{}", rng.pick(PIECES));
        let matching = if rng.below(2) == 0 {
            Matching::Exact
        } else {
            Matching::Fuzzy
        };

        let (Ok(before), Ok(after)) = (
            Query::parse(&first, matching),
            Query::parse(&grown, matching),
        ) else {
            continue;
        };

        if !narrows(&before, &after) {
            continue;
        }

        // The limits that have broken arithmetic before: none at all, and
        // everything there is.
        let limit = *rng.pick(&[1usize, 7, 200, usize::MAX]);
        let options = SearchOptions {
            limit,
            ..Default::default()
        };

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let (_, pool) = search_in(&corpus, &before, &options, None);
            let Some(pool) = pool else {
                return true;
            };

            let (narrowed, _) = search_in(&corpus, &after, &options, Some(&pool));
            let (cold, _) = search_in(&corpus, &after, &options, None);

            assert_eq!(
                narrowed.matched, cold.matched,
                "match count differs for {first:?} -> {grown:?}"
            );
            assert_eq!(
                narrowed.results.len(),
                cold.results.len(),
                "row count differs for {first:?} -> {grown:?}"
            );
            for (got, want) in narrowed.results.iter().zip(&cold.results) {
                assert_eq!(got.id, want.id, "row differs for {first:?} -> {grown:?}");
                assert_eq!(got.score, want.score, "score differs for {first:?} -> {grown:?}");
            }

            true
        }));

        narrowing_rounds += 1;
        if outcome.is_err() {
            narrowing_failures += 1;
            println!("NARROWING MISMATCH seed {seed:#x}, {first:?} -> {grown:?} {matching:?}");
            if narrowing_failures > 20 {
                break;
            }
        }
    }

    // The matchers on their own, where the pattern and the haystack are
    // free of each other.
    let mut matcher_failures = 0usize;
    for round in 0..rounds {
        let seed = rng.0;
        let pieces = 1 + rng.below(3);
        let pattern = text(&mut rng, pieces);
        let haystack = rng.pick(PATHS).to_string();
        let case_sensitive = rng.below(2) == 0;
        let normalize = rng.below(2) == 0;
        let scheme = if rng.below(2) == 0 {
            Scheme::Default
        } else {
            Scheme::Path
        };

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut scratch = Scratch::new();
            let fuzzy = FuzzyMatcher::with_options(&pattern, case_sensitive, scheme, normalize);
            let _ = fuzzy.match_text_fast(&haystack, &mut scratch);
            let _ = fuzzy.match_text_with_scratch(&haystack, &mut scratch);

            for anchor in [
                seek_core::fuzzy::Anchor::Anywhere,
                seek_core::fuzzy::Anchor::Start,
                seek_core::fuzzy::Anchor::End,
                seek_core::fuzzy::Anchor::Whole,
            ] {
                let exact = ExactMatcher::new(&pattern, case_sensitive, scheme, normalize)
                    .anchored(anchor);
                let (hit, positions) = exact.match_text_with_positions(&haystack);

                // Whatever it reports has to be a real span of the text.
                if hit.is_match() {
                    let (s, e) = (hit.start as usize, hit.end as usize);
                    assert!(s <= e && e <= haystack.len(), "span out of range");
                    assert!(haystack.is_char_boundary(s) && haystack.is_char_boundary(e));
                    for p in positions {
                        assert!(haystack.is_char_boundary(p as usize), "position mid-character");
                    }
                }
            }
        }));

        if outcome.is_err() {
            matcher_failures += 1;
            println!("MATCHER PANIC round {round}, seed {seed:#x}, pattern {pattern:?}");
            if matcher_failures > 20 {
                break;
            }
        }
    }

    let _ = std::panic::take_hook();
    println!(
        "{rounds} rounds: {parsed_ok} queries parsed and searched, \
         {failures} search panics, {narrowing_rounds} narrowing pairs with \
         {narrowing_failures} mismatches, {matcher_failures} matcher panics"
    );
}
