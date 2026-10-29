//! End-to-end tests: scan a directory tree, index it, and query it.

use fuzzylite_core::corpus::Corpus;
use fuzzylite_core::index::{EntryKind, Index};
use fuzzylite_core::query::{Query, SearchOptions};
use fuzzylite_core::scan::{scan_blocking, ScanConfig};
use fuzzylite_core::search::search;
use std::path::{Path, PathBuf};

/// Builds a small tree of files in a temporary directory.
fn make_tree() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fuzzylite-test-{}-{}",
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

    let query = Query::parse("epsilon").unwrap();
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

    let query = Query::parse("ALPHA").unwrap();
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
        &Query::parse("a").unwrap(),
        &SearchOptions::default(),
    );
    let filtered = search(
        &corpus,
        &Query::parse("a !alpha").unwrap(),
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
        &Query::parse("").unwrap(),
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
        &Query::parse("'.rs'").unwrap(),
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
        &Query::parse("alpha").unwrap(),
        &SearchOptions::default(),
    );
    let scores: Vec<i32> = outcome.results.iter().map(|r| r.score).collect();
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(scores, sorted, "results must be ordered by descending score");
    std::fs::remove_dir_all(&root).ok();
}
