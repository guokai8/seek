//! A restored snapshot must not claim the index is whole when it is not.
//!
//! The scan has a deadline. When it runs out the index is partial, and the
//! engine knows: `sk_engine_scan_complete` says so and the panel puts
//! "scan cut short" in the footer. That knowledge used to live only in
//! memory. Writing the snapshot threw it away, so the next launch restored
//! a half-built index and reported it as a finished one -- the exact thing
//! the comment on `Inner::scan_complete` says not to do.

use std::ffi::CString;
use std::path::PathBuf;

use seek_core::ffi::*;
use seek_core::ffi_persist::sk_engine_load_snapshot;
use seek_core::index::{EntryKind, Index};
use seek_core::persist;

/// A snapshot written for `root`, with the scan marked as `complete` or not.
fn write_snapshot(root: &PathBuf, complete: bool) -> PathBuf {
    let mut index = Index::new();
    let top = index.add_named(None, "tree", EntryKind::Directory, 0, 0);
    index.add_named(Some(top), "notes.txt", EntryKind::File, 10, 0);

    let path = std::env::temp_dir().join(format!(
        "seek-restore-{}-{}.skidx",
        complete,
        std::process::id()
    ));
    persist::save(&index, std::slice::from_ref(root), complete, &path).unwrap();
    path
}

/// Loads `snapshot` into a fresh engine rooted at `root`.
fn load_into_engine(root: &PathBuf, snapshot: &PathBuf) -> bool {
    let engine = sk_engine_new();
    let roots = CString::new(root.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { sk_engine_set_roots(engine, roots.as_ptr()) }, 0);

    let path = CString::new(snapshot.to_str().unwrap()).unwrap();
    let loaded = unsafe { sk_engine_load_snapshot(engine, path.as_ptr()) };
    assert!(loaded > 0, "the snapshot did not load");

    let complete = sk_engine_scan_complete(engine) != 0;
    unsafe { sk_engine_free(engine) };
    complete
}

#[test]
fn a_restored_partial_index_still_knows_it_is_partial() {
    let root = PathBuf::from("/tmp/seek-restore-root");
    let snapshot = write_snapshot(&root, false);

    assert!(
        !load_into_engine(&root, &snapshot),
        "a half-built index was restored and reported as a finished scan"
    );

    let _ = std::fs::remove_file(&snapshot);
}

#[test]
fn a_restored_whole_index_is_not_reported_as_partial() {
    let root = PathBuf::from("/tmp/seek-restore-root");
    let snapshot = write_snapshot(&root, true);

    assert!(
        load_into_engine(&root, &snapshot),
        "a finished scan was restored and reported as cut short"
    );

    let _ = std::fs::remove_file(&snapshot);
}
