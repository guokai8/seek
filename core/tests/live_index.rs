//! A check that a file created after the launch shows up in the results.
//!
//! This is the end-to-end version of the watcher test: it goes through the
//! C ABI that the app itself uses.

use std::ffi::CString;
use std::time::{Duration, Instant};

use fuzzylite_core::ffi::*;

/// Waits for the watcher to report a change that mentions `needle`.
fn wait_for_change(engine: *mut Engine, buffer: &mut [i8], needle: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);

    while Instant::now() < deadline {
        let found = unsafe {
            fl_engine_poll_changes(engine, buffer.as_mut_ptr(), buffer.len() as i32)
        };
        assert!(found >= 0);

        let mut offset = 0usize;

        for _ in 0..found {
            // The records are separated by null bytes.
            let mut end = offset;
            while end < buffer.len() && buffer[end] != 0 {
                end += 1;
            }

            let record = String::from_utf8_lossy(
                &buffer[offset..end].iter().map(|b| *b as u8).collect::<Vec<u8>>()
            )
            .to_string();

            if record.contains(needle) {
                return true;
            }

            offset = end + 1;
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    false
}

#[test]
fn a_file_created_after_the_scan_is_found() {
    let root = std::env::temp_dir().join(format!("fuzzylite-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let engine = fl_engine_new();
    let roots = CString::new(root.to_str().unwrap()).unwrap();
    unsafe { fl_engine_set_roots(engine, roots.as_ptr()) };

    fl_engine_start_scan(engine);
    while fl_engine_scan_finished(engine) == 0 {
        std::thread::sleep(Duration::from_millis(20));
    }
    let before = fl_engine_finish_scan(engine);

    // The watcher is started after the scan.
    assert_eq!(fl_engine_start_watch(engine), 0);

    // The file is created after the scan.
    let new_file = root.join("appeared_later.txt");
    std::fs::write(&new_file, b"hello").unwrap();

    // The change is reported.
    let mut buffer = vec![0i8; 16 * 1024];
    assert!(
        wait_for_change(engine, &mut buffer, "appeared_later.txt"),
        "the watcher did not report the file that was created"
    );

    // The change is applied to the index.
    let path = CString::new(new_file.to_str().unwrap()).unwrap();
    let change = FileChange {
        path: path.as_ptr(),
        kind: 0,
        is_dir: 0,
    };

    let after = unsafe { fl_engine_apply_changes(engine, &change, 1) };
    assert!(after > before, "the index did not grow");

    // The new file is found by a search.
    let query = CString::new("appeared").unwrap();
    let mut results: [SearchResultEntry; 16] = std::array::from_fn(|_| SearchResultEntry {
        id: 0,
        score: 0,
        start: 0,
        end: 0,
    });

    let hits = unsafe {
        fl_engine_search(
            engine,
            query.as_ptr(),
            results.as_mut_ptr(),
            results.len() as i32,
        )
    };

    assert!(hits > 0, "the new file was not found by the search");

    unsafe { fl_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);
}
