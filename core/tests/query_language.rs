//! The query language, end to end.
//!
//! The parser has its own unit tests, but what a user types has to come
//! out the other side of the *search* — through the corpus, the filters
//! and the ranking. Every one of the bugs these tests pin looked fine in
//! the parser and was only visible from here: `*.pdf` parsed, and matched
//! nothing; `content:` parsed, and matched everything.

use seek_core::corpus::Corpus;
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, Query, QueryError, SearchOptions};
use seek_core::search::search;
use std::collections::HashMap;

/// The paths every test below searches over.
///
/// They are chosen so that the wrong answer is a different set from the
/// right one: `archive/old.pdf.bak` holds `.pdf` without being one,
/// `Test_Report.PDF` is only found if case is folded, and `a/p/d/f/x.txt`
/// is what a fuzzy match for `pdf` drags in when a literal one was meant.
const PATHS: &[&str] = &[
    "docs/report.pdf",
    "docs/notes.md",
    "papers/neurips2024.pdf",
    "papers/draft.tex",
    "src/main.rs",
    "src/fuzzy/rank.rs",
    "archive/old.pdf.bak",
    "a/p/d/f/x.txt",
    "pdf/readme.md",
    "Test_Report.PDF",
    "build/output.tmp",
];

/// Builds a corpus holding [`PATHS`].
fn corpus() -> Corpus {
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    let mut dirs: HashMap<String, u32> = HashMap::new();

    for path in PATHS {
        let parts: Vec<&str> = path.split('/').collect();
        let mut parent = root;

        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                index.add_named(Some(parent), part, EntryKind::File, 1, 0);
                continue;
            }

            let key = parts[..=i].join("/");
            parent = match dirs.get(&key) {
                Some(id) => *id,
                None => {
                    let id = index.add_named(Some(parent), part, EntryKind::Directory, 0, 0);
                    dirs.insert(key, id);
                    id
                }
            };
        }
    }

    index.touch();
    Corpus::build(&index)
}

/// The paths a query finds, sorted so the comparison is about the set.
fn hits(corpus: &Corpus, query: &str) -> Vec<String> {
    let parsed = Query::parse(query, Matching::Fuzzy).expect("the query should parse");
    let options = SearchOptions {
        limit: PATHS.len() * 2,
        ..Default::default()
    };

    let outcome = search(corpus, &parsed, &options);
    let mut found: Vec<String> = outcome
        .results
        .iter()
        .filter_map(|r| {
            corpus
                .position(r.id)
                .map(|i| corpus.relative_path(i).unwrap_or_default().to_string())
        })
        .collect();

    found.sort();
    found
}

#[test]
fn a_glob_finds_the_files_it_names() {
    let corpus = corpus();

    // This returned nothing at all before globs existed: `*` is not a
    // character any path holds, so the fuzzy matcher rejected everything.
    assert_eq!(
        hits(&corpus, "*.pdf"),
        vec![
            "Test_Report.PDF".to_string(),
            "docs/report.pdf".to_string(),
            "papers/neurips2024.pdf".to_string(),
        ]
    );
}

#[test]
fn a_glob_matches_the_whole_name_and_not_a_part_of_it() {
    let corpus = corpus();

    // `archive/old.pdf.bak` contains `.pdf` but is not a `.pdf`, which is
    // exactly the difference between a glob and a substring.
    assert!(!hits(&corpus, "*.pdf").contains(&"archive/old.pdf.bak".to_string()));
    assert_eq!(hits(&corpus, "*.bak"), vec!["archive/old.pdf.bak"]);
}

#[test]
fn a_glob_with_a_separator_stays_in_its_directory() {
    let corpus = corpus();

    assert_eq!(hits(&corpus, "src/*.rs"), vec!["src/main.rs"]);
    assert_eq!(hits(&corpus, "src/**/*.rs"), vec!["src/fuzzy/rank.rs", "src/main.rs"]);
}

#[test]
fn a_question_mark_stands_for_one_character() {
    let corpus = corpus();

    assert_eq!(hits(&corpus, "?.txt"), vec!["a/p/d/f/x.txt"]);
    assert!(hits(&corpus, "??.txt").is_empty());
}

#[test]
fn a_regex_finds_what_it_describes() {
    let corpus = corpus();

    assert_eq!(
        hits(&corpus, r"re:re.*port"),
        vec!["Test_Report.PDF".to_string(), "docs/report.pdf".to_string()]
    );
    assert_eq!(hits(&corpus, r"re:^src/.*\.rs$"), vec!["src/fuzzy/rank.rs", "src/main.rs"]);
}

#[test]
fn a_regex_is_matched_against_the_whole_relative_path() {
    let corpus = corpus();

    // Anchoring to the start means the start of the path, which is what
    // the panel shows and what the fuzzy matcher scores.
    assert_eq!(hits(&corpus, r"re:^docs/"), vec!["docs/notes.md", "docs/report.pdf"]);
}

#[test]
fn the_literal_operators_fold_case_like_everything_else() {
    let corpus = corpus();

    // Each of these found nothing, or too little, when the operators used
    // the standard library's byte-exact predicates.
    assert_eq!(hits(&corpus, "^Docs"), vec!["docs", "docs/notes.md", "docs/report.pdf"]);
    assert!(hits(&corpus, "PDF$").contains(&"docs/report.pdf".to_string()));
    assert!(hits(&corpus, "pdf$").contains(&"Test_Report.PDF".to_string()));
    assert!(hits(&corpus, "'REPORT'").contains(&"docs/report.pdf".to_string()));
}

#[test]
fn a_quoted_term_is_a_substring_and_not_a_subsequence() {
    let corpus = corpus();

    // The fuzzy reading of `pdf` picks p, d and f out of `papers/draft.tex`
    // and out of `a/p/d/f`. The quoted reading does not.
    let found = hits(&corpus, "'pdf'");

    assert!(!found.contains(&"papers/draft.tex".to_string()));
    assert!(!found.contains(&"a/p/d/f/x.txt".to_string()));
    assert!(found.contains(&"docs/report.pdf".to_string()));
}

#[test]
fn an_inverse_term_excludes_by_its_own_kind() {
    let corpus = corpus();

    // `!*.tmp` has to exclude the glob's matches. Read as a fuzzy term it
    // would exclude anything holding t, m and p in order — which is a
    // different set, and includes `Test_Report.PDF`.
    let found = hits(&corpus, "re:. !*.tmp");

    assert!(!found.contains(&"build/output.tmp".to_string()));
    assert!(found.contains(&"Test_Report.PDF".to_string()));
}

#[test]
fn an_anchor_at_each_end_asks_for_that_path_and_no_other() {
    let corpus = corpus();

    // fzf's `EqualMatch`. This used to parse as a prefix term whose text
    // ends in a dollar, so the panel answered it with nothing at all.
    assert_eq!(hits(&corpus, "^docs/report.pdf$"), vec!["docs/report.pdf"]);
    assert!(hits(&corpus, "^docs/report$").is_empty());
    assert!(hits(&corpus, "^ocs/report.pdf$").is_empty());

    // The directory itself is an entry of the index, so the whole-path
    // term finds it and nothing under it.
    assert_eq!(hits(&corpus, "^docs$"), vec!["docs"]);
}

#[test]
fn an_inverted_anchor_at_each_end_excludes_that_path_and_no_other() {
    let corpus = corpus();

    let found = hits(&corpus, "re:. !^docs/report.pdf$");

    assert!(!found.contains(&"docs/report.pdf".to_string()));
    assert!(found.contains(&"docs/notes.md".to_string()));
}

#[test]
fn an_anchored_term_ranks_its_matches_rather_than_only_filtering_them() {
    let corpus = corpus();

    // Both paths end in `.pdf`; the term opens on the dot in one and
    // inside `old.pdf.bak`'s word in the other, and fzf pays those two
    // different bonuses. A term that only filtered would score both zero
    // and leave the order to the tiebreaks.
    let parsed = Query::parse("pdf$", Matching::Exact).unwrap();
    let options = SearchOptions {
        limit: PATHS.len() * 2,
        ..Default::default()
    };
    let outcome = search(&corpus, &parsed, &options);

    assert!(
        outcome.results.iter().any(|r| r.score > 0),
        "every match scored zero, so the anchored term only filtered"
    );
}

#[test]
fn terms_of_different_kinds_combine() {
    let corpus = corpus();

    assert_eq!(hits(&corpus, "*.pdf ^docs"), vec!["docs/report.pdf"]);
    assert_eq!(hits(&corpus, "*.pdf !docs"), vec!["Test_Report.PDF", "papers/neurips2024.pdf"]);
}

#[test]
fn content_search_is_refused_rather_than_answered_wrongly() {
    // It used to parse into a term kind no filter knew, so the term was
    // dropped and the query matched the entire index.
    assert_eq!(
        Query::parse("content:hello", Matching::Fuzzy).unwrap_err(),
        QueryError::Unsupported("content:".into())
    );
}

#[test]
fn a_pattern_that_does_not_compile_is_an_error_not_an_empty_list() {
    assert!(matches!(
        Query::parse("re:[", Matching::Fuzzy).unwrap_err(),
        QueryError::InvalidRegex { .. }
    ));
    assert!(matches!(
        Query::parse("*[.pdf", Matching::Fuzzy).unwrap_err(),
        QueryError::InvalidGlob { .. }
    ));
}
