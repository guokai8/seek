//! A check that the engine survives concurrent use.
//!
//! The app searches on a background queue while the index is being rebuilt
//! and snapshotted on another. Those operations touch the same state, so
//! they have to be serialised — otherwise the app crashes with a data race.

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use seek_core::ffi::*;
use seek_core::ffi_persist::sk_engine_save_snapshot;

/// Builds a small tree to index.
fn build_tree(root: &std::path::Path) {
    for i in 0..40 {
        let dir = root.join(format!("dir{i}"));
        std::fs::create_dir_all(&dir).unwrap();
        for j in 0..10 {
            std::fs::write(dir.join(format!("file{i}_{j}.txt")), b"x").unwrap();
        }
    }
}

#[test]
fn searching_while_the_index_is_rebuilt_does_not_crash() {
    let root = std::env::temp_dir().join(format!("seek-race-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    build_tree(&root);

    let engine = sk_engine_new();
    let roots = CString::new(root.to_str().unwrap()).unwrap();
    unsafe { sk_engine_set_roots(engine, roots.as_ptr()) };

    let stop = Arc::new(AtomicBool::new(false));

    // The searcher keeps searching while the indexer rebuilds the index
    // and writes snapshots.
    let searcher = {
        let engine = engine as usize;
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let engine = engine as *mut seek_core::ffi::Engine;
            let query = CString::new("file").unwrap();

            while !stop.load(Ordering::Relaxed) {
                let mut results: [SearchResultEntry; 32] = std::array::from_fn(|_| {
                    SearchResultEntry::default()
                });

                let found = unsafe {
                    sk_engine_search(
                        engine,
                        query.as_ptr(),
                        results.as_mut_ptr(),
                        results.len() as i32,
                        std::ptr::null_mut(),
                    )
                };

                assert!(found >= 0);

                for entry in results.iter().take(found as usize) {
                    let path = unsafe { sk_engine_path(engine, entry.id) };
                    assert!(!path.is_null());
                    unsafe { sk_string_free(path) };
                }
            }
        })
    };

    // The indexer rebuilds the index and writes a snapshot, repeatedly.
    let indexer = {
        let engine = engine as usize;
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let engine = engine as *mut seek_core::ffi::Engine;

            while !stop.load(Ordering::Relaxed) {
                unsafe {
                    sk_engine_start_scan(engine);
                    while sk_engine_scan_finished(engine) == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    sk_engine_finish_scan(engine);

                    let snapshot = std::env::temp_dir().join(format!(
                        "seek-race-{}-{}.skidx",
                        std::process::id(),
                        std::thread::current().name().unwrap_or("x")
                    ));
                    let path = CString::new(snapshot.to_str().unwrap()).unwrap();
                    sk_engine_save_snapshot(engine, path.as_ptr());
                    let _ = std::fs::remove_file(&snapshot);
                }
            }
        })
    };

    std::thread::sleep(std::time::Duration::from_millis(500));
    stop.store(true, Ordering::Relaxed);

    searcher.join().unwrap();
    indexer.join().unwrap();

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);
}
