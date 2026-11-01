//! What the watcher reports has to pass the same rules as the scan.
//!
//! The watcher is given the roots and nothing else: it reports every
//! change underneath them, including the trees the scan was told to skip.
//! Applying those reports unfiltered put `Library`, `.git` and every
//! other excluded tree into the index a few entries at a time, for as
//! long as the app was running — which is both wrong and unbounded.
//!
//! These tests go through the C ABI the app uses, and apply the changes
//! by hand rather than waiting for the watcher, so they are deterministic.

use std::ffi::CString;
use std::path::Path;
use std::time::Duration;

use seek_core::ffi::*;

/// Applies one change to the engine and returns the live entry count.
fn apply(engine: *mut Engine, path: &Path, is_dir: bool) -> i32 {
    let text = CString::new(path.to_str().unwrap()).unwrap();
    let change = FileChange {
        path: text.as_ptr(),
        kind: 0, // created
        is_dir: i32::from(is_dir),
    };

    unsafe { sk_engine_apply_changes(engine, &change, 1) }
}

/// A scanned engine over a tree with an excluded directory in it.
fn engine_over_tree(name: &str) -> (*mut Engine, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("seek-excl-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("Library")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

    // The watcher reports real paths, so the test uses them too: on macOS
    // the temporary directory lives under `/var`, which is a symbolic link
    // to `/private/var`, and FSEvents reports the latter.
    let root = std::fs::canonicalize(&root).unwrap();

    let engine = sk_engine_new();
    let roots = CString::new(root.to_str().unwrap()).unwrap();
    let excludes = CString::new("Library,.git,node_modules").unwrap();
    unsafe {
        sk_engine_set_roots(engine, roots.as_ptr());
        sk_engine_set_excludes(engine, excludes.as_ptr());
    }

    sk_engine_start_scan(engine);
    while sk_engine_scan_finished(engine) == 0 {
        std::thread::sleep(Duration::from_millis(20));
    }

    // The scan hands its entries over here. Without this the index is
    // still empty and every count below would be zero.
    assert!(sk_engine_finish_scan(engine) > 0, "the scan found nothing");

    (engine, root)
}

#[test]
fn a_change_under_an_excluded_directory_is_ignored() {
    let (engine, root) = engine_over_tree("dir");
    let before = apply(engine, &root.join("src/other.rs"), false);

    // The scan skipped `Library`, so the watcher's report of a file in it
    // must be skipped too.
    let after = apply(engine, &root.join("Library/Caches/junk.bin"), false);

    assert_eq!(
        after, before,
        "a file under an excluded directory was added to the index"
    );

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_hidden_file_is_ignored_the_way_the_scan_ignores_it() {
    let (engine, root) = engine_over_tree("hidden");
    let before = apply(engine, &root.join("src/other.rs"), false);
    let after = apply(engine, &root.join(".DS_Store"), false);

    assert_eq!(after, before, "a hidden file was added to the index");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_file_inside_a_hidden_directory_is_ignored() {
    let (engine, root) = engine_over_tree("hiddendir");
    let before = apply(engine, &root.join("src/other.rs"), false);
    let after = apply(engine, &root.join(".git/objects/ab/cdef"), false);

    assert_eq!(after, before, "a file inside a hidden directory was added");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_ordinary_change_still_arrives() {
    let (engine, root) = engine_over_tree("ordinary");

    // The filtering must not swallow the changes the watcher exists for.
    let before = sk_engine_count(engine);
    let after = apply(engine, &root.join("src/fresh.rs"), false);

    assert!(
        after > before,
        "an ordinary file was not added: {before} -> {after}"
    );

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_path_outside_every_root_is_ignored() {
    let (engine, root) = engine_over_tree("outside");
    let before = apply(engine, &root.join("src/other.rs"), false);

    // Nothing outside the roots belongs in the index, and a path that
    // cannot be resolved to a root has no place to hang off.
    let after = apply(engine, Path::new("/etc/hosts"), false);

    assert_eq!(after, before, "a path outside the roots was added");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);
}
