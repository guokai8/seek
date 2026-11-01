//! A check that a search result carries the file's size and mtime.
//!
//! The result row shows them, and the UI must not go to the filesystem for
//! them: a stat per visible row is one thing, but the index already holds
//! both, and asking it would mean taking the maintenance lock that a scan
//! holds for the whole of a rebuild. So they travel with the result.

use std::ffi::CString;

use seek_core::ffi::*;

/// Scans a directory holding one file of `size` bytes and returns the row
/// the search gives back for it.
fn result_for_a_file_of(size: usize, tag: &str) -> SearchResultEntry {
    let root = std::env::temp_dir().join(format!("seek-meta-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("subject.bin"), vec![b'x'; size]).unwrap();

    let engine = sk_engine_new();
    let roots = CString::new(root.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { sk_engine_set_roots(engine, roots.as_ptr()) }, 0);
    assert_eq!(sk_engine_start_scan(engine), 0);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while sk_engine_scan_finished(engine) == 0 {
        assert!(std::time::Instant::now() < deadline, "the scan did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    sk_engine_finish_scan(engine);

    let query = CString::new("subject.bin").unwrap();
    let mut results = vec![SearchResultEntry::default(); 8];
    let found = unsafe {
        sk_engine_search(
            engine,
            query.as_ptr(),
            results.as_mut_ptr(),
            results.len() as i32,
            std::ptr::null_mut(),
        )
    };
    assert!(found > 0, "the search did not find the file it just indexed");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);

    results[0]
}

#[test]
fn a_result_carries_the_file_size() {
    let row = result_for_a_file_of(1234, "size");
    assert_eq!(row.size, 1234, "the row reports the wrong size");
}

#[test]
fn a_result_carries_the_modification_time() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let row = result_for_a_file_of(16, "mtime");

    // The file was written inside this test, so its mtime is now. A window
    // rather than an equality, because the clock ticks during the scan.
    assert!(
        row.modified >= before - 2 && row.modified <= before + 60,
        "the row reports an mtime of {}, which is nowhere near {before}",
        row.modified
    );
}
