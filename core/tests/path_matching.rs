//! A check that the search covers the whole path, the way fzf does.
//!
//! The result row shows a file name and a directory. A search that cannot
//! find the directory it displays would be a search that lies, so the
//! matcher scores the full path.

use seek_core::corpus::Corpus;
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, Query, SearchOptions};
use seek_core::scan::{scan_blocking, ScanConfig};
use seek_core::search::search;
use std::path::{Path, PathBuf};

/// Builds a small tree of files in a temporary directory.
fn make_tree() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "seek-path-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let files = [
        "alpha.txt",
        "gamma/alpha.rs",
        "gamma/delta.rs",
        "gamma/sub/epsilon.rs",
        "delta/zeta.txt",
        "uniquefolder/remarkable.txt",
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

    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    let root_id = index.add_named(None, &root_name, EntryKind::Directory, 0, 0);
    index.roots.push(root_id);
    index.config_roots.push(root.to_path_buf());

    // Every directory path is mapped to its entry id, so that the children
    // can be linked to their parents.
    let mut dir_ids: std::collections::HashMap<PathBuf, u32> = std::collections::HashMap::new();
    dir_ids.insert(root.to_path_buf(), root_id);

    let mut sorted = entries;
    sorted.sort_by_key(|e| e.path.components().count());

    for entry in &sorted {
        let parent_path = entry
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| root.to_path_buf());
        let parent = dir_ids.get(&parent_path).copied();

        let kind = if entry.is_dir {
            EntryKind::Directory
        } else {
            EntryKind::File
        };

        let id = index.add_named(
            parent,
            &entry.file_name,
            kind,
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

/// Runs a query and returns the matched file names.
fn query_names(corpus: &Corpus, text: &str) -> Vec<String> {
    let parsed = Query::parse(text, Matching::Fuzzy).unwrap();
    let outcome = search(corpus, &parsed, &SearchOptions::default());

    // The corpus carries the path, not the name: the name is its last
    // component, which is what the matcher scored at the end of the path.
    outcome
        .results
        .iter()
        .filter_map(|result| corpus.position(result.id))
        .filter_map(|position| corpus.relative_path(position))
        .map(|path| path.rsplit('/').next().unwrap_or(path).to_string())
        .collect()
}

#[test]
fn a_query_matches_a_file_by_its_name() {
    let dir = make_tree();
    let (_index, corpus) = index_tree(&dir);

    let names = query_names(&corpus, "alpha");

    assert!(
        names.contains(&"alpha.txt".to_string()),
        "the file name should match, got {names:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_query_matches_a_file_by_its_directory() {
    let dir = make_tree();
    let (_index, corpus) = index_tree(&dir);

    // "uniquefolder" appears nowhere in the file names, only in the paths.
    let names = query_names(&corpus, "uniquefolder");

    assert!(
        names.contains(&"remarkable.txt".to_string()),
        "the directory should match, got {names:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_query_matches_a_file_by_a_directory_in_the_middle_of_the_path() {
    let dir = make_tree();
    let (_index, corpus) = index_tree(&dir);

    // "sub" is a directory two levels down.
    let names = query_names(&corpus, "sub");

    assert!(
        names.contains(&"epsilon.rs".to_string()),
        "the nested directory should match, got {names:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_query_that_matches_nothing_still_matches_nothing() {
    let dir = make_tree();
    let (_index, corpus) = index_tree(&dir);

    let names = query_names(&corpus, "zzzznotpresent");

    assert!(names.is_empty(), "expected no matches, got {names:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_corpus_carries_the_path_relative_to_the_root() {
    let dir = make_tree();
    let (index, corpus) = index_tree(&dir);

    assert_eq!(corpus.paths().count(), corpus.entries.len());

    // Every path ends with the name the *index* holds for that entry, and
    // none of them carries the root's prefix. The prefix is the same for
    // every entry under the root, so searching it would be wasted work.
    //
    // The name is checked against the index rather than against the
    // corpus, because the corpus no longer carries one: a name beside the
    // path was a second copy of bytes the path already ends with.
    //
    // The root itself is the exception: it has nothing below it, so its
    // relative path is empty.
    for (entry, path) in corpus.entries.iter().zip(corpus.paths()) {
        if path.is_empty() {
            continue;
        }

        let name = index
            .get(entry.id)
            .and_then(|e| index.names.get(e.name))
            .expect("every corpus entry comes from an index entry with a name");

        assert!(
            path.ends_with(name),
            "the path {path} should end with the name {name}"
        );
        assert!(
            !path.starts_with('/'),
            "the path {path} should be relative to the root"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
