//! A search must not wait for index maintenance.
//!
//! The watcher takes the engine's write lock every few hundred
//! milliseconds, and while it holds it the corpus is rebuilt over every
//! indexed entry. The searches used to take the read lock on the same
//! state, and `std::sync::RwLock` prefers writers, so one queued writer was
//! enough to block every search behind it. A launcher whose searches block
//! is a launcher that shows nothing.
//!
//! The searchable state is therefore published as an immutable snapshot:
//! a search takes a reference to it and then works outside any lock. This
//! test holds the maintenance lock for the whole call and asserts that the
//! three calls the panel makes on every keystroke — the search, the row
//! paths, the highlight offsets — all still answer.

use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::mpsc;
use std::time::Duration;

use seek_core::ffi::*;

/// The engine pointer, so it can be handed to the worker thread.
///
/// The engine guards its own state, so using it from another thread is
/// exactly what the app does.
#[derive(Clone, Copy)]
struct Handle(*mut Engine);
unsafe impl Send for Handle {}

/// How long the panel is allowed to wait before the wait is a bug.
const BUDGET: Duration = Duration::from_millis(2_000);

/// Creates a tree with enough files to be worth searching.
fn make_tree() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("seek-maintenance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    for i in 0..40 {
        let sub = dir.join(format!("package{i}/src"));
        std::fs::create_dir_all(&sub).unwrap();
        for j in 0..10 {
            std::fs::write(sub.join(format!("readme{j}.md")), b"x").unwrap();
        }
    }

    dir
}

/// Builds an engine over a tree and returns it indexed.
fn indexed_engine(root: &std::path::Path) -> *mut Engine {
    let engine = sk_engine_new();

    let roots = CString::new(root.to_string_lossy().as_bytes()).unwrap();
    unsafe {
        assert_eq!(sk_engine_set_roots(engine, roots.as_ptr()), 0);
        assert_eq!(sk_engine_start_scan(engine), 0);

        while sk_engine_scan_finished(engine) == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(sk_engine_finish_scan(engine) > 0);
    }

    engine
}

/// Runs `work` on another thread and returns its result, or `None` when it
/// did not finish inside the budget.
///
/// The work is run off the test's thread so that a call that blocks for
/// ever fails the test instead of hanging it.
fn within_budget<T: Send + 'static>(
    handle: Handle,
    work: impl FnOnce(Handle) -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        let _ = sender.send(work(handle));
    });

    receiver.recv_timeout(BUDGET).ok()
}

#[test]
fn a_search_answers_while_the_index_is_being_maintained() {
    let tree = make_tree();
    let handle = Handle(indexed_engine(&tree));

    // The maintenance lock is taken and held for the rest of the test, the
    // way the watcher holds it while it rebuilds the corpus.
    let held = unsafe { (*handle.0).write() };

    // The search.
    let results = within_budget(handle, |handle| {
        let query = CString::new("readme").unwrap();
        let mut out = vec![SearchResultEntry::default(); 200];

        let found = unsafe {
            sk_engine_search(
                handle.0,
                query.as_ptr(),
                out.as_mut_ptr(),
                out.len() as i32,
                std::ptr::null_mut(),
            )
        };

        out.truncate(found.max(0) as usize);
        out.iter().map(|r| r.id).collect::<Vec<u32>>()
    })
    .expect("the search waited for index maintenance");

    assert!(!results.is_empty(), "the search found nothing");

    // The row paths. A result that cannot be resolved is a row that is
    // found but never shown.
    let ids = results.clone();
    let resolved = within_budget(handle, move |handle| {
        let mut paths = vec![0 as c_char; 256 * 1024];
        let mut offsets = vec![0i32; ids.len() * 2];

        unsafe {
            sk_engine_resolve(
                handle.0,
                ids.as_ptr(),
                ids.len() as i32,
                paths.as_mut_ptr(),
                paths.len() as i32,
                offsets.as_mut_ptr(),
            )
        }
    })
    .expect("resolving the rows waited for index maintenance");

    assert_eq!(
        resolved as usize,
        results.len(),
        "not every row was resolved"
    );

    // The highlight offsets.
    let ids = results.clone();
    let written = within_budget(handle, move |handle| {
        let query = CString::new("readme").unwrap();
        let mut positions = vec![0i32; ids.len() * 32];
        let mut counts = vec![0i32; ids.len()];

        unsafe {
            sk_engine_positions_batch(
                handle.0,
                ids.as_ptr(),
                ids.len() as i32,
                query.as_ptr(),
                positions.as_mut_ptr(),
                positions.len() as i32,
                counts.as_mut_ptr(),
            )
        }
    })
    .expect("the highlights waited for index maintenance");

    assert!(written > 0, "no highlight offsets came back");

    drop(held);
    unsafe { sk_engine_free(handle.0) };
    let _ = std::fs::remove_dir_all(&tree);
}
