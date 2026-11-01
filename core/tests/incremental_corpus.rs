//! The published snapshot, updated in place.
//!
//! A watcher batch touches a handful of entries out of a quarter of a
//! million. Rebuilding the whole corpus for them costs a fresh `String`
//! for every path in the index, which is what made the app burn a third
//! of a core while it sat idle and grow from 354 MB to 586 MB over an
//! afternoon.
//!
//! The incremental path only has one thing to prove: that it agrees with
//! the full rebuild, entry for entry. If it ever disagrees the panel
//! shows files that are not there, or hides files that are — so these
//! tests compare the two snapshots field by field after every kind of
//! change the watcher can report.

use seek_core::corpus::Corpus;
use seek_core::index::{EntryKind, Index};
use std::path::{Path, PathBuf};

/// Builds an index rooted at `/root` holding `paths`.
fn index_with(paths: &[&str]) -> Index {
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push(PathBuf::from("/root"));

    for path in paths {
        index.upsert(
            &PathBuf::from("/root").join(path),
            EntryKind::File,
            1,
            0,
        );
    }

    index.touch();
    index
}

/// The two snapshots, compared the way a search sees them.
fn assert_agrees(incremental: &Corpus, full: &Corpus, what: &str) {
    let ids = |c: &Corpus| c.entries.iter().map(|e| e.id).collect::<Vec<_>>();
    let paths = |c: &Corpus| c.paths().map(|p| p.to_string()).collect::<Vec<_>>();
    let dirs = |c: &Corpus| c.entries.iter().map(|e| e.is_dir).collect::<Vec<_>>();
    let roots = |c: &Corpus| c.entries.iter().map(|e| e.root).collect::<Vec<_>>();
    let sizes = |c: &Corpus| c.entries.iter().map(|e| e.size).collect::<Vec<_>>();
    let times = |c: &Corpus| c.entries.iter().map(|e| e.modified).collect::<Vec<_>>();

    assert_eq!(paths(incremental), paths(full), "paths disagree after {what}");
    assert_eq!(ids(incremental), ids(full), "ids disagree after {what}");
    assert_eq!(dirs(incremental), dirs(full), "is_dir disagrees after {what}");
    assert_eq!(roots(incremental), roots(full), "roots disagree after {what}");
    assert_eq!(
        incremental.trim_lengths, full.trim_lengths,
        "trim lengths disagree after {what}"
    );
    assert_eq!(incremental.roots, full.roots, "roots disagree after {what}");

    // The row shows both of these, and a watcher batch is the one thing
    // that changes them: a file saved is a new size and a new mtime at an
    // id that already existed. Carrying the old entry over instead of
    // rebuilding it would leave the row showing yesterday's size for as
    // long as the app ran.
    assert_eq!(sizes(incremental), sizes(full), "sizes disagree after {what}");
    assert_eq!(times(incremental), times(full), "mtimes disagree after {what}");
}

#[test]
fn a_created_file_lands_where_a_rebuild_would_put_it() {
    let mut index = index_with(&["a.txt", "docs/b.md", "src/c.rs"]);
    let before = Corpus::build(&index);

    index.take_dirty();
    index.upsert(Path::new("/root/docs/new.md"), EntryKind::File, 3, 0);

    let dirty = index.take_dirty();
    let incremental = before.updated(&index, &dirty);
    assert_agrees(&incremental, &Corpus::build(&index), "a file was created");
    assert!(incremental.paths().any(|p| p == "docs/new.md"));
}

#[test]
fn a_modified_file_keeps_its_place() {
    let mut index = index_with(&["a.txt", "docs/b.md"]);
    let before = Corpus::build(&index);

    index.take_dirty();
    index.upsert(Path::new("/root/docs/b.md"), EntryKind::File, 99, 42);

    let dirty = index.take_dirty();
    let incremental = before.updated(&index, &dirty);
    assert_agrees(&incremental, &Corpus::build(&index), "a file was modified");

    // Said outright as well as by agreement: the snapshot the panel draws
    // from carries the size the file has now, not the one it had.
    let position = incremental
        .paths()
        .position(|path| path == "docs/b.md")
        .expect("the file left the snapshot");
    let entry = incremental.entries[position];
    assert_eq!(entry.size, 99, "the snapshot kept the old size");
    assert_eq!(entry.modified, 42, "the snapshot kept the old mtime");
}

#[test]
fn a_removed_file_is_gone_from_the_snapshot() {
    let mut index = index_with(&["a.txt", "docs/b.md", "src/c.rs"]);
    let before = Corpus::build(&index);

    index.take_dirty();
    assert!(index.remove(Path::new("/root/docs/b.md")));

    let dirty = index.take_dirty();
    let incremental = before.updated(&index, &dirty);
    assert_agrees(&incremental, &Corpus::build(&index), "a file was removed");
    assert!(!incremental.paths().any(|p| p == "docs/b.md"));
}

#[test]
fn removing_a_directory_takes_its_children_with_it() {
    let mut index = index_with(&["a.txt", "docs/b.md", "docs/deep/c.md", "src/d.rs"]);
    let before = Corpus::build(&index);

    index.take_dirty();
    assert!(index.remove(Path::new("/root/docs")));

    // The children are tombstoned too, so the incremental path only
    // agrees with a rebuild if the index reported *their* ids as well.
    let dirty = index.take_dirty();
    let incremental = before.updated(&index, &dirty);
    assert_agrees(&incremental, &Corpus::build(&index), "a directory was removed");
    assert!(!incremental.paths().any(|p| p.starts_with("docs")));
}

#[test]
fn a_file_in_a_new_directory_brings_the_directory_with_it() {
    let mut index = index_with(&["a.txt"]);
    let before = Corpus::build(&index);

    index.take_dirty();
    // `upsert` creates the missing parents itself, and those are new
    // corpus rows that nobody told us about.
    index.upsert(Path::new("/root/fresh/deep/x.md"), EntryKind::File, 1, 0);

    let dirty = index.take_dirty();
    let incremental = before.updated(&index, &dirty);
    assert_agrees(&incremental, &Corpus::build(&index), "a new tree appeared");
    assert!(incremental.paths().any(|p| p == "fresh/deep"));
}

#[test]
fn a_batch_of_mixed_changes_agrees_with_a_rebuild() {
    let mut index = index_with(&["a.txt", "b.txt", "docs/c.md", "docs/d.md", "src/e.rs"]);
    let before = Corpus::build(&index);

    index.take_dirty();
    index.upsert(Path::new("/root/docs/c.md"), EntryKind::File, 7, 7);
    index.remove(Path::new("/root/b.txt"));
    index.upsert(Path::new("/root/new/f.rs"), EntryKind::File, 1, 0);
    index.remove(Path::new("/root/src"));

    let dirty = index.take_dirty();
    let incremental = before.updated(&index, &dirty);
    assert_agrees(&incremental, &Corpus::build(&index), "a mixed batch");
}

#[test]
fn an_unchanged_entry_is_carried_over_rather_than_rebuilt() {
    let mut index = index_with(&["a.txt", "docs/b.md"]);
    let before = Corpus::build(&index);

    index.take_dirty();
    index.upsert(Path::new("/root/new.txt"), EntryKind::File, 1, 0);

    let dirty = index.take_dirty();
    let incremental = before.updated(&index, &dirty);

    // This is the whole point of the exercise: the bytes of a path that
    // did not change are the *same bytes in memory*, not a fresh copy of
    // them. The arena holding them is shared with the old snapshot, so the
    // two agree down to the address. Without this the batch still copies
    // every path in the index and nothing has been fixed.
    let carried = before
        .paths()
        .find(|p| *p == "docs/b.md")
        .expect("the path should be in the old snapshot");
    let now = incremental
        .paths()
        .find(|p| *p == "docs/b.md")
        .expect("the path should be in the new snapshot");

    assert_eq!(
        carried.as_ptr(),
        now.as_ptr(),
        "an untouched path was copied instead of carried over"
    );
}

#[test]
fn nothing_changed_means_the_snapshot_is_unchanged() {
    let index = index_with(&["a.txt", "docs/b.md"]);
    let before = Corpus::build(&index);

    let incremental = before.updated(&index, &[]);
    assert_agrees(&incremental, &before, "an empty batch");
}
