//! End-to-end tests: scan a directory tree, index it, and query it.

use seek_core::corpus::Corpus;
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, Query, SearchOptions};
use seek_core::scan::{scan_blocking, ScanConfig};
use seek_core::search::search;
use std::path::{Path, PathBuf};

/// Builds a small tree of files in a temporary directory.
fn make_tree() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "seek-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let files = [
        "alpha.txt",
        "beta.md",
        "gamma/alpha.rs",
        "gamma/delta.rs",
        "gamma/sub/epsilon.rs",
        "delta/zeta.txt",
        "README.md",
    ];
    for f in files {
        let path = dir.join(f);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, b"hello world\n").unwrap();
    }
    dir
}

/// Scans a tree and builds an index and corpus from the result.
fn index_tree(root: &Path) -> (Index, Corpus) {
    let config = ScanConfig {
        roots: vec![root.to_path_buf()],
        ..Default::default()
    };
    let (entries, summary) = scan_blocking(&config);
    assert!(summary.files > 0, "scan should find files");

    let mut index = Index::new();

    // Insert the root itself so paths can be rebuilt from it.
    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    let root_id = index.add_named(None, &root_name, EntryKind::Directory, 0, 0);

    // Map every directory path to its entry id so children can be linked.
    let mut dir_ids: std::collections::HashMap<PathBuf, u32> = std::collections::HashMap::new();
    dir_ids.insert(root.to_path_buf(), root_id);

    // Directories first, so parents always exist before their children.
    let mut sorted = entries.clone();
    sorted.sort_by_key(|e| e.path.components().count());

    for entry in sorted {
        let parent_path = entry
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| root.to_path_buf());
        let parent = dir_ids.get(&parent_path).copied();

        let id = index.add_named(
            parent,
            &entry.file_name,
            if entry.is_dir {
                EntryKind::Directory
            } else {
                EntryKind::File
            },
            entry.size.unwrap_or(0),
            entry.modified.unwrap_or(0),
        );

        if entry.is_dir {
            dir_ids.insert(entry.path.clone(), id);
        }
    }

    index.touch();
    let corpus = Corpus::build(&index);
    (index, corpus)
}

#[test]
fn scan_finds_all_files() {
    let root = make_tree();
    let config = ScanConfig {
        roots: vec![root.clone()],
        ..Default::default()
    };
    let (entries, summary) = scan_blocking(&config);

    assert_eq!(summary.files, 7, "expected 7 files, got {}", summary.files);
    assert!(summary.dirs >= 3);
    assert!(entries.iter().any(|e| e.file_name == "epsilon.rs"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn index_builds_full_paths() {
    let root = make_tree();
    let (index, _) = index_tree(&root);
    let paths = index.full_paths();

    assert!(
        paths.values().any(|p| p.ends_with("gamma/sub/epsilon.rs")),
        "nested path should be rebuilt: {:?}",
        paths.values().collect::<Vec<_>>()
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn query_finds_a_nested_file() {
    let root = make_tree();
    let (index, corpus) = index_tree(&root);
    let paths = index.full_paths();

    let query = Query::parse("epsilon", Matching::Fuzzy).unwrap();
    let outcome = search(&corpus, &query, &SearchOptions::default());

    assert!(!outcome.results.is_empty(), "query should find epsilon.rs");
    let names: Vec<String> = outcome
        .results
        .iter()
        .map(|r| paths[&r.id].file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(names.contains(&"epsilon.rs".to_string()));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn query_is_case_insensitive_by_default() {
    let root = make_tree();
    let (_, corpus) = index_tree(&root);

    let query = Query::parse("ALPHA", Matching::Fuzzy).unwrap();
    let outcome = search(&corpus, &query, &SearchOptions::default());
    assert!(!outcome.results.is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn inverse_term_excludes_results() {
    let root = make_tree();
    let (index, corpus) = index_tree(&root);
    let paths = index.full_paths();

    let all = search(
        &corpus,
        &Query::parse("a", Matching::Fuzzy).unwrap(),
        &SearchOptions::default(),
    );
    let filtered = search(
        &corpus,
        &Query::parse("a !alpha", Matching::Fuzzy).unwrap(),
        &SearchOptions::default(),
    );

    assert!(all.results.len() > filtered.results.len());
    let names: Vec<String> = filtered
        .results
        .iter()
        .map(|r| paths[&r.id].file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(!names.contains(&"alpha.txt".to_string()));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn empty_query_matches_everything() {
    let root = make_tree();
    let (_, corpus) = index_tree(&root);

    let outcome = search(
        &corpus,
        &Query::parse("", Matching::Fuzzy).unwrap(),
        &SearchOptions::default(),
    );
    assert_eq!(outcome.results.len(), corpus.len());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn exact_term_filters_by_substring() {
    let root = make_tree();
    let (_, corpus) = index_tree(&root);

    let outcome = search(
        &corpus,
        &Query::parse("'.rs'", Matching::Fuzzy).unwrap(),
        &SearchOptions::default(),
    );
    assert_eq!(outcome.results.len(), 3, "expected 3 .rs files");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn results_are_ordered_by_score() {
    let root = make_tree();
    let (_, corpus) = index_tree(&root);

    let outcome = search(
        &corpus,
        &Query::parse("alpha", Matching::Fuzzy).unwrap(),
        &SearchOptions::default(),
    );
    let scores: Vec<i32> = outcome.results.iter().map(|r| r.score).collect();
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(scores, sorted, "results must be ordered by descending score");
    std::fs::remove_dir_all(&root).ok();
}

/// A wide tree, with many paths that tie on every ranking criterion.
///
/// The names are deliberately repetitive: the same file names under many
/// equally deep directories of equal length score identically, which is what
/// makes the tie order visible.
fn make_wide_tree() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "seek-wide-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    for group in 0..24 {
        for area in ["src", "lib", "bin"] {
            let sub = dir.join(format!("proj{group:02}")).join(area);
            std::fs::create_dir_all(&sub).unwrap();
            for file in ["alpha.rs", "parser.rs", "render.rs", "loader.rs", "config.rs"] {
                std::fs::write(sub.join(file), b"x").unwrap();
            }
        }
    }

    dir
}

/// A capped search must be the top of an uncapped one.
///
/// The ranking cuts the list down to `limit` before sorting it, because
/// ordering the matches nobody will see is the most expensive thing left
/// after the matching itself. That is only correct if the rows it keeps are
/// exactly the rows a full sort would have put at the top — and in the same
/// order, including which of several tied rows comes first.
#[test]
fn a_capped_search_is_the_top_of_an_uncapped_one() {
    let root = make_wide_tree();
    let (_, corpus) = index_tree(&root);

    // Patterns that match a large share of the tree, so the cap really
    // cuts, and that produce plenty of exact ties.
    for pattern in ["r", "rs", "er", "ar", "con", "load", "src"] {
        let query = Query::parse(pattern, Matching::Fuzzy).unwrap();

        let uncapped = search(
            &corpus,
            &query,
            &SearchOptions {
                limit: corpus.entries.len() + 1,
                ..Default::default()
            },
        );

        for limit in [1usize, 7, 50, 200] {
            let capped = search(
                &corpus,
                &query,
                &SearchOptions {
                    limit,
                    ..Default::default()
                },
            );

            let expected = limit.min(uncapped.results.len());
            assert_eq!(
                capped.results.len(),
                expected,
                "a cap of {limit} on {pattern:?} should give {expected} rows"
            );

            let capped_ids: Vec<_> = capped.results.iter().map(|r| r.id).collect();
            let head_ids: Vec<_> = uncapped
                .results
                .iter()
                .take(expected)
                .map(|r| r.id)
                .collect();

            assert_eq!(
                capped_ids, head_ids,
                "a cap of {limit} on {pattern:?} must give the top of the full ranking"
            );
        }
    }

    std::fs::remove_dir_all(&root).ok();
}
