use std::time::{Duration, Instant};

/// A check that a search finishes while the watcher is running.
///
/// The watcher polls the filesystem and takes the write lock each time.
/// The searches take the read lock. The lock is the standard library's,
/// which prefers writers, so a stream of writers can keep a reader
/// waiting.
fn main() {
    let engine = seek_core::ffi::sk_engine_new();
    let roots = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let excludes = "Library,.Trash,.cache,.npm,.cargo,node_modules,.git,Applications";

    unsafe {
        let roots_c = std::ffi::CString::new(roots.clone()).unwrap();
        let excludes_c = std::ffi::CString::new(excludes).unwrap();

        seek_core::ffi::sk_engine_set_roots(engine, roots_c.as_ptr());
        seek_core::ffi::sk_engine_set_excludes(engine, excludes_c.as_ptr());
        seek_core::ffi::sk_engine_start_scan(engine);

        while seek_core::ffi::sk_engine_scan_finished(engine) == 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        seek_core::ffi::sk_engine_finish_scan(engine);

        // The watcher is started, the way the app starts it.
        let watched = seek_core::ffi::sk_engine_start_watch(engine);
        println!("watch started: {watched}");

        // A search, while the watcher polls.
        let query = std::ffi::CString::new("r").unwrap();
        let mut results: Vec<seek_core::ffi::SearchResultEntry> = Vec::with_capacity(500);
        for _ in 0..500 {
            results.push(seek_core::ffi::SearchResultEntry {
                id: 0,
                score: 0,
                start: 0,
                end: 0,
                is_dir: 0,
            });
        }

        for round in 0..10 {
            let start = Instant::now();

            let found = seek_core::ffi::sk_engine_search(
                engine,
                query.as_ptr(),
                results.as_mut_ptr(),
                500,
                std::ptr::null_mut(),
            );

            println!(
                "round {round}: search took {:?}, hits={found}",
                start.elapsed()
            );

            // The watcher polls, which takes the write lock.
            let mut buffer = vec![0i8; 16 * 1024];
            let changes = seek_core::ffi::sk_engine_poll_changes(
                engine,
                buffer.as_mut_ptr(),
                16 * 1024,
            );
            if changes > 0 {
                let start = Instant::now();
                seek_core::ffi::sk_engine_apply_changes(engine, std::ptr::null(), 0);
                println!("  applied {changes} changes in {:?}", start.elapsed());
            }

            std::thread::sleep(Duration::from_millis(400));
        }
    }
}
