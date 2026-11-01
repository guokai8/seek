//! A check of the C ABI's scan lifecycle.
//!
//! The Swift app drives the scan through the C ABI, so the lifecycle of the
//! scan has to be verified through that interface rather than through the
//! Rust API.

use std::ffi::CString;

use seek_core::ffi::*;

#[test]
fn the_ffi_scan_completes() {
    let root = std::env::temp_dir().join(format!("seek-ffi-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(&root).unwrap();
    for i in 0..20 {
        std::fs::write(root.join(format!("f{i}.txt")), b"x").unwrap();
    }

    let engine = sk_engine_new();
    assert!(!engine.is_null());

    let roots = CString::new(root.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { sk_engine_set_roots(engine, roots.as_ptr()) }, 0);

    assert_eq!(sk_engine_start_scan(engine), 0);

    // The scan must finish on its own, without the caller reading anything.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while sk_engine_scan_finished(engine) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the scan did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let count = sk_engine_finish_scan(engine);
    assert!(count >= 20, "expected at least 20 entries, got {count}");

    // The search must find one of the files.
    let query = CString::new("f1.txt").unwrap();
    let mut results: [SearchResultEntry; 16] = std::array::from_fn(|_| SearchResultEntry {
        id: 0,
        score: 0,
        start: 0,
        end: 0,
        is_dir: 0,
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

    assert!(found > 0, "the search found nothing");

    let first = &results[0];
    let path_ptr = unsafe { sk_engine_path(engine, first.id) };
    assert!(!path_ptr.is_null());
    let path = unsafe { std::ffi::CStr::from_ptr(path_ptr) }
        .to_string_lossy()
        .to_string();
    unsafe { sk_string_free(path_ptr) };

    assert!(path.ends_with("f1.txt"), "unexpected path {path}");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);
}
