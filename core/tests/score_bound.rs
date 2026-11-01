//! The bound that lets a candidate go unscored.
//!
//! A broad query matches far more paths than the panel will ever show, and
//! scoring the ones that cannot reach the shown rows is the bulk of what a
//! slow query spends its time on. Leaving them unscored rests on one
//! claim, and these tests are that claim written down:
//!
//! * no match can score above the ceiling — every character worth
//!   [`SCORE_MATCH`] plus the best bonus the scheme offers, the first one's
//!   bonus counted twice, and no gaps; and
//! * a path that does not contain the pattern as a consecutive run has at
//!   least one gap in every alignment, and a gap costs at least
//!   `SCORE_GAP_START`, so it scores at most three below the ceiling.
//!
//! The second claim is the load-bearing one: it is what says a candidate
//! can be dismissed without being scored. If a scoring constant changes
//! and the ceiling is not changed with it, these tests go red — which is
//! the point of having them, because the alternative is a search that
//! silently drops rows it should have shown.
//!
//! The claim is about the matcher's own folding, not about bytes, and the
//! last test here is the reason that distinction matters.
//!
//! A sharper version of the second claim is pinned down further down:
//! holding the pattern as a consecutive run is not enough to reach the
//! ceiling either. The ceiling pays the scheme's best bonus for every
//! character, and that bonus is only earned by following one particular
//! class of character — a separator under the path scheme. A run anywhere
//! else is at least `m + 1` below the ceiling. That is what
//! [`FuzzyMatcher::ceiling_boundary`] states, and the tests for it include
//! the pattern that breaks it.

use seek_core::corpus::Corpus;
use seek_core::fuzzy::{literal, FuzzyMatcher, Scheme};
use seek_core::index::{EntryKind, Index};
use seek_core::query::SearchOptions;

/// The lowest a gap can cost, from fzf's scoring model.
const GAP: i32 = 3;

/// A corpus of a few thousand varied paths.
///
/// The names are deliberately mixed: some put the query right after a
/// separator, where it scores at the ceiling, and some scatter its
/// characters across the path, where it does not. A fixture of only one
/// kind would check only one side of the bound.
fn corpus() -> Corpus {
    const DIRS: [&str; 8] = [
        "src", "tests", "docs", "build", "vendor", "examples", "node_modules", "target",
    ];
    const NAMES: [&str; 13] = [
        "main.rs",
        "index.html",
        "package.json",
        "LICENSE",
        "notes.txt",
        "bundle.min.js",
        "app.py",
        "data.csv",
        // Three names that hold a query's characters in order but never
        // adjacent, which is the side of the bound that matters. Without
        // them a long pattern matches only the names that contain it
        // outright, and the assertion below has nothing to check.
        "t-e-s-t.md",
        "latest-subset.txt",
        "re-a-d-m-e.txt",
        "README.md",
        "Makefile",
    ];

    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
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

/// A matcher built the way the search builds one.
fn matcher(pattern: &str) -> FuzzyMatcher {
    let options = SearchOptions::default();
    FuzzyMatcher::with_options(pattern, false, options.scheme, options.normalize)
}

/// Every path of the corpus, in order.
fn paths(corpus: &Corpus) -> Vec<&str> {
    (0..corpus.len())
        .filter_map(|i| corpus.relative_path(i))
        .collect()
}

#[test]
fn no_match_scores_above_the_ceiling() {
    let corpus = corpus();
    let paths = paths(&corpus);
    let mut scratch = seek_core::fuzzy::Scratch::new();

    for pattern in ["t", "ab", "src", "test", "index", "readme", "makefile"] {
        let matcher = matcher(pattern);
        let ceiling = matcher.score_ceiling();

        for path in &paths {
            let m = matcher.match_text_fast(path, &mut scratch);
            if !m.is_match() {
                continue;
            }
            assert!(
                m.score <= ceiling,
                "{pattern:?} scored {} against {path:?}, above the ceiling {ceiling}",
                m.score
            );
        }
    }
}

#[test]
fn a_path_without_a_consecutive_run_scores_at_least_a_gap_below_the_ceiling() {
    let corpus = corpus();
    let paths = paths(&corpus);
    let mut scratch = seek_core::fuzzy::Scratch::new();

    for pattern in ["ab", "src", "test", "index", "readme"] {
        let matcher = matcher(pattern);
        let ceiling = matcher.score_ceiling();
        let mut checked = 0usize;

        for path in &paths {
            // The claim is about the matcher's folding, and for a path that
            // is not plain ASCII a byte-wise substring search does not
            // answer the same question. See the last test in this file.
            if !path.is_ascii() {
                continue;
            }
            if literal::contains(path, pattern, false) {
                continue;
            }
            let m = matcher.match_text_fast(path, &mut scratch);
            if !m.is_match() {
                continue;
            }
            checked += 1;
            assert!(
                m.score <= ceiling - GAP,
                "{pattern:?} scored {} against {path:?}, which has no consecutive run, \
                 but the bound allows only {}",
                m.score,
                ceiling - GAP
            );
        }

        assert!(
            checked > 0,
            "{pattern:?} matched nothing without a consecutive run, so the bound was never tested"
        );
    }
}

#[test]
fn a_consecutive_run_at_a_separator_reaches_the_ceiling() {
    // The other half of the bound: the ceiling is not merely an upper
    // limit nobody approaches. If it were loose, the prune would never
    // arm and would be dead code that still had to be maintained.
    let matcher = matcher("test");
    let mut scratch = seek_core::fuzzy::Scratch::new();
    let m = matcher.match_text_fast("src/test/main.rs", &mut scratch);

    assert_eq!(m.score, matcher.score_ceiling());
}

#[test]
fn the_ceiling_follows_the_scheme() {
    // The best bonus a scheme offers differs — the default scheme pays
    // more for a word boundary after whitespace than the path scheme pays
    // for one after a separator. A ceiling computed for the wrong scheme
    // would be too low, and a bound that is too low drops rows.
    let path = FuzzyMatcher::with_options("test", false, Scheme::Path, true);
    let default = FuzzyMatcher::with_options("test", false, Scheme::Default, true);

    assert!(
        default.score_ceiling() > path.score_ceiling(),
        "the default scheme pays a larger boundary bonus, so its ceiling is higher: \
         path {} default {}",
        path.score_ceiling(),
        default.score_ceiling()
    );
}

#[test]
fn an_accented_path_reaches_the_ceiling_without_containing_the_pattern() {
    // This is why the prune only looks at plain ASCII paths. The matcher
    // folds `é` to `e` by default, exactly as fzf does, so `tést` holds a
    // consecutive run of `test` and scores at the ceiling — while a
    // byte-wise substring search says the path does not contain the
    // pattern at all. Trusting that search here would dismiss a candidate
    // that belongs at the very top.
    let matcher = matcher("test");
    let mut scratch = seek_core::fuzzy::Scratch::new();
    let accented = "src/tést/main.rs";

    assert!(!literal::contains(accented, "test", false));

    let m = matcher.match_text_fast(accented, &mut scratch);
    assert_eq!(m.score, matcher.score_ceiling());
    assert!(m.score > matcher.score_ceiling() - GAP);
}

// --- The sharper bound: the ceiling is only reached at a boundary ---

/// The boundary of a pattern that qualifies for the sharper bound.
fn boundary(pattern: &str) -> seek_core::fuzzy::CeilingBoundary {
    matcher(pattern)
        .ceiling_boundary()
        .expect("this pattern should qualify for the boundary bound")
}

/// Whether `path` holds `pattern` as a run that could reach the ceiling.
fn holds_a_boundary_run(path: &str, pattern: &str) -> bool {
    let boundary = boundary(pattern);
    literal::contains_after(path, pattern, false, |prev| boundary.follows(prev))
}

#[test]
fn a_path_without_a_run_at_a_boundary_scores_at_least_a_gap_below_the_ceiling() {
    // The load-bearing claim of the sharper bound. Everything the search
    // dismisses on it is a path this test says cannot reach the rows
    // shown, so if this goes red the search is dropping rows.
    let corpus = corpus();
    let paths = paths(&corpus);
    let mut scratch = seek_core::fuzzy::Scratch::new();

    for pattern in ["ab", "src", "test", "index", "readme"] {
        let matcher = matcher(pattern);
        let ceiling = matcher.score_ceiling();
        let mut checked = 0usize;

        for path in &paths {
            // Byte-wise reasoning only applies to a plain ASCII path, for
            // the same reason as the test above.
            if !path.is_ascii() {
                continue;
            }
            if holds_a_boundary_run(path, pattern) {
                continue;
            }
            let m = matcher.match_text_fast(path, &mut scratch);
            if !m.is_match() {
                continue;
            }
            checked += 1;
            assert!(
                m.score <= ceiling - GAP,
                "{pattern:?} scored {} against {path:?}, which holds no run at a boundary, \
                 but the bound allows only {}",
                m.score,
                ceiling - GAP
            );
        }

        assert!(
            checked > 0,
            "{pattern:?} matched nothing outside a boundary, so the bound was never tested"
        );
    }
}

#[test]
fn a_run_in_the_middle_of_a_word_is_what_this_bound_catches() {
    // The whole point of the sharper bound: a path that *does* contain the
    // pattern, which the consecutive-run bound has to score, and which
    // this bound dismisses. Without a case like this in the corpus the
    // test above would be checking the same paths as the older one.
    let matcher = matcher("test");
    let mut scratch = seek_core::fuzzy::Scratch::new();
    let path = "docs/latest-subset.txt";

    assert!(literal::contains(path, "test", false));
    assert!(!holds_a_boundary_run(path, "test"));

    let m = matcher.match_text_fast(path, &mut scratch);
    assert!(m.is_match());
    assert!(m.score <= matcher.score_ceiling() - GAP);
}

#[test]
fn the_ceiling_is_only_reached_at_a_boundary() {
    // The converse direction, over paths written to probe it from both
    // sides. The corpus above is no use here: it reaches the ceiling for
    // almost no pattern, because every name in it carries an index prefix,
    // so a test over it would assert the implication without ever meeting
    // a path that satisfies it.
    //
    // Both halves are asserted at once — score at the ceiling if and only
    // if there is a run at a boundary — because a bound that held in one
    // direction only would either drop rows or prune nothing.
    let matcher = matcher("test");
    let ceiling = matcher.score_ceiling();
    let mut scratch = seek_core::fuzzy::Scratch::new();

    // Paths that should reach the ceiling: the run follows a separator, or
    // starts the path, which the path scheme treats as following one.
    let at_boundary = [
        "test",
        "test.md",
        "src/test/main.rs",
        "a/b/testing.rs",
        "src/Test.md", // case is folded, and the bonus does not depend on it
    ];
    // Paths that hold the run somewhere else, or do not hold it at all.
    let elsewhere = [
        "latest.md",
        "src/latest/notes.md",
        "x-test.md",   // after a non-word character: one bonus lower
        "a test.md",   // after whitespace, which the path scheme pays less for
        "subTest.rs",  // a camelCase hump, lower again
        "src/t-e-s-t", // no consecutive run at all
    ];

    for path in at_boundary {
        let m = matcher.match_text_fast(path, &mut scratch);
        assert!(
            holds_a_boundary_run(path, "test"),
            "{path:?} was meant to hold a run at a boundary"
        );
        assert_eq!(
            m.score, ceiling,
            "{path:?} holds a run at a boundary, so it should score the ceiling"
        );
    }

    for path in elsewhere {
        let m = matcher.match_text_fast(path, &mut scratch);
        assert!(
            !holds_a_boundary_run(path, "test"),
            "{path:?} was meant to hold no run at a boundary"
        );
        assert!(
            m.score <= ceiling - GAP,
            "{path:?} holds no run at a boundary, so it should be a gap below the ceiling, \
             but it scored {}",
            m.score
        );
    }
}

#[test]
fn the_boundary_of_a_path_pattern_is_the_separator() {
    let boundary = boundary("test");

    assert!(
        boundary.follows(None),
        "the start of a path is a boundary: the path scheme assumes a separator before it"
    );
    assert!(boundary.follows(Some(b'/')), "a separator is the boundary");
    assert!(
        !boundary.follows(Some(b' ')),
        "the path scheme pays less for whitespace than for a separator"
    );
    assert!(!boundary.follows(Some(b'-')));
    assert!(!boundary.follows(Some(b'a')));
    assert!(
        boundary.follows(Some(0xC3)),
        "a non-ASCII byte is answered conservatively, because folding moves the boundaries"
    );
}

#[test]
fn the_boundary_follows_the_scheme() {
    // The default scheme pays more for a word boundary after whitespace
    // than for one after a delimiter, so there the ceiling needs
    // whitespace and a separator is not enough. A boundary set copied
    // from the path scheme would dismiss the top rows.
    let default = FuzzyMatcher::with_options("test", false, Scheme::Default, true);
    let boundary = default
        .ceiling_boundary()
        .expect("a plain two-character pattern qualifies");

    assert!(boundary.follows(Some(b' ')), "whitespace pays the best bonus here");
    assert!(boundary.follows(Some(b'\t')));
    assert!(
        !boundary.follows(Some(b'/')),
        "a delimiter pays one less than whitespace in the default scheme"
    );
    assert!(boundary.follows(None));
}

#[test]
fn a_pattern_that_carries_its_own_boundary_is_not_bounded_this_way() {
    // `/b` reaches two below the ceiling without following a separator,
    // because the `/` inside the pattern pays the best bonus to the
    // character after it. Two below the ceiling is exactly where the
    // prune arms, so such a path can tie for the last place shown — and
    // the bound has to refuse the pattern rather than dismiss it.
    let matcher = matcher("/b");
    let mut scratch = seek_core::fuzzy::Scratch::new();
    let m = matcher.match_text_fast("x-/b", &mut scratch);

    assert_eq!(
        m.score,
        matcher.score_ceiling() - 2,
        "if this is no longer two below the ceiling, the guard below needs re-deriving"
    );
    assert!(
        matcher.ceiling_boundary().is_none(),
        "a pattern carrying its own boundary must not be bounded by one"
    );
}

#[test]
fn a_one_character_pattern_is_not_bounded_this_way() {
    // With one character there are no inherited bonuses to be one lower,
    // so a single character after any boundary-paying class reaches the
    // ceiling and the margin is only two.
    assert!(matcher("t").ceiling_boundary().is_none());
}

#[test]
fn a_pattern_that_is_not_plain_ascii_is_not_bounded_this_way() {
    // Same reason as the last test in the first half of this file: the
    // boundary test is byte-wise and the matcher folds.
    assert!(matcher("tést").ceiling_boundary().is_none());
    assert!(matcher("test").ceiling_boundary().is_some());
}
