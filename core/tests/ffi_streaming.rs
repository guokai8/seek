//! The C ABI's streaming search.
//!
//! The Swift app drives the search through the C ABI, so the progress
//! callback has to be verified through that interface — a function
//! pointer, a context pointer and a borrowed row buffer — rather than
//! through the Rust API, which is where the easy mistakes are not.

use std::ffi::CString;
use std::os::raw::{c_int, c_void};

use seek_core::ffi::*;

/// How many files the fixture holds.
const FILES: usize = 3000;
/// How many candidates go by between publishes.
const CHUNK: c_int = 400;

/// What the callback writes back through the context pointer.
#[derive(Default)]
struct Record {
    /// The ids of each publish, in order.
    publishes: Vec<Vec<u32>>,
    /// The running match count of each publish.
    counts: Vec<c_int>,
    /// Stop once this many publishes have arrived. Zero never stops.
    stop_after: usize,
}

/// The progress callback, as C sees it.
///
/// # Safety
///
/// `rows` is borrowed for the duration of the call and must not be kept.
/// `context` must be the `Record` the search was given.
unsafe extern "C" fn record(
    rows: *const SearchResultEntry,
    count: c_int,
    matched: c_int,
    context: *mut c_void,
) -> c_int {
    let record = &mut *(context as *mut Record);

    let ids: Vec<u32> = if rows.is_null() || count <= 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(rows, count as usize)
            .iter()
            .map(|row| row.id)
            .collect()
    };

    record.publishes.push(ids);
    record.counts.push(matched);

    if record.stop_after > 0 && record.publishes.len() >= record.stop_after {
        0
    } else {
        1
    }
}

/// An empty row buffer of `n` entries.
fn buffer(n: usize) -> Vec<SearchResultEntry> {
    (0..n)
        .map(|_| SearchResultEntry {
            id: 0,
            score: 0,
            start: 0,
            end: 0,
            is_dir: 0,
        })
        .collect()
}

/// Builds an engine over a fresh directory of files, and returns both.
///
/// The names put `report` right after a separator for a few of them and
/// scatter its letters through the rest, so the ranking has something to
/// change its mind about as the chunks go by.
fn engine(tag: &str) -> (*mut Engine, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "seek-ffi-streaming-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    for i in 0..FILES {
        std::fs::write(root.join(format!("r-e-p-o-r-t-{i}.log")), b"x").unwrap();
    }
    let good = root.join("report");
    std::fs::create_dir_all(&good).unwrap();
    for i in 0..40 {
        std::fs::write(good.join(format!("{i}.txt")), b"x").unwrap();
    }

    let eng = sk_engine_new();
    assert!(!eng.is_null());
    let roots = CString::new(root.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { sk_engine_set_roots(eng, roots.as_ptr()) }, 0);
    assert_eq!(sk_engine_start_scan(eng), 0);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while sk_engine_scan_finished(eng) == 0 {
        assert!(std::time::Instant::now() < deadline, "the scan did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let count = sk_engine_finish_scan(eng);
    assert!(count as usize >= FILES, "indexed {count} entries");

    (eng, root)
}

#[test]
fn the_callback_is_called_more_than_once() {
    let (eng, root) = engine("many");
    let query = CString::new("report").unwrap();
    let mut rows = buffer(16);
    let mut record_state = Record::default();

    let found = unsafe {
        sk_engine_search_streaming(
            eng,
            query.as_ptr(),
            rows.as_mut_ptr(),
            rows.len() as c_int,
            std::ptr::null_mut(),
            CHUNK,
            Some(record),
            &mut record_state as *mut Record as *mut c_void,
        )
    };

    assert!(found > 0, "the search found nothing");
    assert!(
        record_state.publishes.len() > 1,
        "a search over {FILES}+ files in chunks of {CHUNK} published {} times",
        record_state.publishes.len()
    );

    // The running count only grows, and the last publish is the whole of it.
    for pair in record_state.counts.windows(2) {
        assert!(pair[1] >= pair[0], "the count went backwards");
    }

    unsafe { sk_engine_free(eng) };
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_last_publish_is_what_the_buffer_holds() {
    // The callback and the return value must not be able to disagree: the
    // panel draws the last publish and then the final rows, and a
    // mismatch between them is a list that flickers to a different answer
    // at the very end.
    let (eng, root) = engine("agree");
    let query = CString::new("report").unwrap();
    let mut rows = buffer(16);
    let mut record_state = Record::default();

    let found = unsafe {
        sk_engine_search_streaming(
            eng,
            query.as_ptr(),
            rows.as_mut_ptr(),
            rows.len() as c_int,
            std::ptr::null_mut(),
            CHUNK,
            Some(record),
            &mut record_state as *mut Record as *mut c_void,
        )
    };

    let final_ids: Vec<u32> = rows[..found as usize].iter().map(|r| r.id).collect();
    assert_eq!(record_state.publishes.last(), Some(&final_ids));

    unsafe { sk_engine_free(eng) };
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_null_callback_is_a_plain_search() {
    // The one-shot search is this one with nothing to report, so the two
    // must give the same answer. If they could differ, every caller that
    // does not want progress would be on a second code path.
    let (eng, root) = engine("null");
    let query = CString::new("report").unwrap();

    let mut streamed = buffer(32);
    let mut streamed_matched: c_int = 0;
    let a = unsafe {
        sk_engine_search_streaming(
            eng,
            query.as_ptr(),
            streamed.as_mut_ptr(),
            streamed.len() as c_int,
            &mut streamed_matched,
            CHUNK,
            None,
            std::ptr::null_mut(),
        )
    };

    let mut plain = buffer(32);
    let mut plain_matched: c_int = 0;
    let b = unsafe {
        sk_engine_search(
            eng,
            query.as_ptr(),
            plain.as_mut_ptr(),
            plain.len() as c_int,
            &mut plain_matched,
        )
    };

    assert_eq!(a, b);
    assert_eq!(streamed_matched, plain_matched);
    let left: Vec<(u32, i32)> = streamed[..a as usize].iter().map(|r| (r.id, r.score)).collect();
    let right: Vec<(u32, i32)> = plain[..b as usize].iter().map(|r| (r.id, r.score)).collect();
    assert_eq!(left, right);

    unsafe { sk_engine_free(eng) };
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn returning_zero_stops_the_search() {
    let (eng, root) = engine("stop");
    let query = CString::new("report").unwrap();

    let mut rows = buffer(16);
    let mut stopped_matched: c_int = 0;
    let mut record_state = Record {
        stop_after: 1,
        ..Record::default()
    };
    unsafe {
        sk_engine_search_streaming(
            eng,
            query.as_ptr(),
            rows.as_mut_ptr(),
            rows.len() as c_int,
            &mut stopped_matched,
            CHUNK,
            Some(record),
            &mut record_state as *mut Record as *mut c_void,
        )
    };

    assert_eq!(record_state.publishes.len(), 1, "it kept going");

    let mut whole = buffer(16);
    let mut whole_matched: c_int = 0;
    unsafe {
        sk_engine_search_streaming(
            eng,
            query.as_ptr(),
            whole.as_mut_ptr(),
            whole.len() as c_int,
            &mut whole_matched,
            CHUNK,
            None,
            std::ptr::null_mut(),
        )
    };

    assert!(
        stopped_matched < whole_matched,
        "a stopped search reported {stopped_matched} of {whole_matched} matches, \
         so it did not actually stop short"
    );

    unsafe { sk_engine_free(eng) };
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_stopped_search_does_not_poison_the_next_one() {
    // A stopped search has only part of the match set, and the engine
    // remembers a query's matches so that the next keystroke can search
    // them instead of the corpus. Remembering a partial set would make the
    // next search miss rows that are really there — and it would do it
    // silently, which is why this is a test and not a comment.
    let (eng, root) = engine("poison");
    let short = CString::new("repor").unwrap();
    let long = CString::new("report").unwrap();

    let mut rows = buffer(16);
    let mut record_state = Record {
        stop_after: 1,
        ..Record::default()
    };
    unsafe {
        sk_engine_search_streaming(
            eng,
            short.as_ptr(),
            rows.as_mut_ptr(),
            rows.len() as c_int,
            std::ptr::null_mut(),
            CHUNK,
            Some(record),
            &mut record_state as *mut Record as *mut c_void,
        )
    };

    // `report` extends `repor`, so the engine would narrow this search to
    // the previous one's matches if it had kept them.
    let mut after = buffer(16);
    let mut after_matched: c_int = 0;
    unsafe {
        sk_engine_search_streaming(
            eng,
            long.as_ptr(),
            after.as_mut_ptr(),
            after.len() as c_int,
            &mut after_matched,
            CHUNK,
            None,
            std::ptr::null_mut(),
        )
    };

    // A fresh engine, never given a stopped search, is the reference.
    let (clean, clean_root) = engine("clean");
    let mut reference = buffer(16);
    let mut reference_matched: c_int = 0;
    unsafe {
        sk_engine_search_streaming(
            clean,
            long.as_ptr(),
            reference.as_mut_ptr(),
            reference.len() as c_int,
            &mut reference_matched,
            CHUNK,
            None,
            std::ptr::null_mut(),
        )
    };

    assert_eq!(
        after_matched, reference_matched,
        "the search after a stopped one lost matches"
    );

    unsafe { sk_engine_free(eng) };
    unsafe { sk_engine_free(clean) };
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&clean_root);
}
