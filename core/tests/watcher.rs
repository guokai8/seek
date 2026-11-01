//! A check that the filesystem watcher keeps the index up to date.
//!
//! The watcher runs on its own thread and reports the changes through a
//! channel, so the test waits for the change to arrive rather than
//! assuming it is instantaneous.

use std::ffi::CString;
use std::time::{Duration, Instant};

use seek_core::ffi::*;

#[test]
fn the_watcher_reports_new_files() {
    let root = std::env::temp_dir().join(format!("seek-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let engine = sk_engine_new();
    let roots = CString::new(root.to_str().unwrap()).unwrap();
    unsafe { sk_engine_set_roots(engine, roots.as_ptr()) };

    // The watcher is started before the scan, so that the file created
    // below is caught either way.
    assert_eq!(sk_engine_start_watch(engine), 0);

    sk_engine_start_scan(engine);
    while sk_engine_scan_finished(engine) == 0 {
        std::thread::sleep(Duration::from_millis(20));
    }
    let initial = sk_engine_finish_scan(engine);
    assert!(initial >= 0);

    // A file is created after the scan has finished.
    let new_file = root.join("watched.txt");
    std::fs::write(&new_file, b"hello").unwrap();

    // The watcher reports it. FSEvents batches its events, so the change
    // arrives after a short delay rather than immediately.
    let mut buffer = vec![0i8; 16 * 1024];
    let mut saw_new_file = false;

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !saw_new_file {
        let found = unsafe {
            sk_engine_poll_changes(engine, buffer.as_mut_ptr(), buffer.len() as i32)
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

            if record.contains("watched.txt") {
                saw_new_file = true;
            }

            offset = end + 1;
        }

        if !saw_new_file {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    assert!(
        saw_new_file,
        "the watcher did not report the file that was created"
    );

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&root);
}
