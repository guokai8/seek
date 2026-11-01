//! The match count that the limit hides.
//!
//! The panel can only show a few hundred rows, so a broad query's result
//! list saturates at the limit and then stops changing — the top two
//! hundred matches for `readme` and for `readm` are, over a real home
//! directory, the same two hundred paths in the same order, and fzf
//! agrees. A footer built from the row count therefore reads the same for
//! both and makes an edited query look like a frozen panel.
//!
//! `SearchOutcome::matched` is the number that still moves. These tests
//! pin it to the count *before* the limit, which is the one thing it
//! would be easy to get wrong: `results.len()` is right only until the
//! matches outgrow the buffer.

use seek_core::corpus::Corpus;
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, Query, SearchOptions};
use seek_core::search::search;

/// A corpus of `count` files that all match `needle`, plus one that does not.
fn corpus(count: usize, needle: &str) -> Corpus {
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    for i in 0..count {
        index.add_named(Some(root), &format!("{needle}-{i}.txt"), EntryKind::File, 1, 0);
    }
    index.add_named(Some(root), "unrelated.bin", EntryKind::File, 1, 0);

    index.touch();
    Corpus::build(&index)
}

fn options(limit: usize) -> SearchOptions {
    SearchOptions {
        limit,
        ..Default::default()
    }
}

#[test]
fn matched_counts_every_match_not_every_row() {
    let corpus = corpus(500, "readme");
    let query = Query::parse("readme", Matching::Fuzzy).unwrap();

    let outcome = search(&corpus, &query, &options(200));

    assert_eq!(outcome.results.len(), 200, "the limit should cap the rows");
    assert_eq!(outcome.matched, 500, "the count should ignore the limit");
}

#[test]
fn matched_equals_the_rows_when_nothing_is_cut() {
    let corpus = corpus(12, "readme");
    let query = Query::parse("readme", Matching::Fuzzy).unwrap();

    let outcome = search(&corpus, &query, &options(200));

    assert_eq!(outcome.results.len(), 12);
    assert_eq!(
        outcome.matched, 12,
        "an untruncated search should not inflate the count"
    );
}

#[test]
fn matched_is_zero_when_nothing_matches() {
    let corpus = corpus(10, "readme");
    let query = Query::parse("zzzznotthere", Matching::Fuzzy).unwrap();

    let outcome = search(&corpus, &query, &options(200));

    assert!(outcome.results.is_empty());
    assert_eq!(outcome.matched, 0);
}

#[test]
fn matched_does_not_count_what_a_filter_removed() {
    // The negative term throws half the corpus out. Counting candidates
    // before the filters, rather than after, would report the whole lot.
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    for i in 0..10 {
        index.add_named(Some(root), &format!("readme-keep-{i}.md"), EntryKind::File, 1, 0);
        index.add_named(Some(root), &format!("readme-drop-{i}.md"), EntryKind::File, 1, 0);
    }
    index.touch();
    let corpus = Corpus::build(&index);

    let query = Query::parse("readme !drop", Matching::Fuzzy).unwrap();
    let outcome = search(&corpus, &query, &options(200));

    assert_eq!(
        outcome.matched, 10,
        "the count should be of what survived the filters"
    );
    assert_eq!(outcome.results.len(), 10);
}

#[test]
fn a_shorter_query_changes_the_count_even_when_the_rows_hold_still() {
    // This is the bug in miniature. Every `readme` path also contains
    // `readm`, and there are more matches than the limit, so the two
    // searches return the same rows. Only the count tells them apart.
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/tmp/root".into());

    for i in 0..300 {
        index.add_named(Some(root), &format!("readme-{i}.md"), EntryKind::File, 1, 0);
    }
    // Matches `readm`, but not `readme`.
    for i in 0..50 {
        index.add_named(Some(root), &format!("readmx-{i}.md"), EntryKind::File, 1, 0);
    }
    index.touch();
    let corpus = Corpus::build(&index);

    let long = search(&corpus, &Query::parse("readme", Matching::Fuzzy).unwrap(), &options(200));
    let short = search(&corpus, &Query::parse("readm", Matching::Fuzzy).unwrap(), &options(200));

    assert_eq!(long.results.len(), short.results.len(), "both are capped");
    assert_eq!(long.matched, 300);
    assert_eq!(short.matched, 350);
    assert_ne!(
        long.matched, short.matched,
        "the count is the only signal left that the query changed"
    );
}
