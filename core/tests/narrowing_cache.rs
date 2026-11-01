//! The engine's memory of the previous query.
//!
//! The narrowing itself is checked in `incremental_narrowing.rs`, over the
//! search function directly. What is checked here is the *cache* that feeds
//! it: the engine remembers the last query's match set and hands it to the
//! next search when the rule allows. That memory is the dangerous part,
//! because a pool kept past the moment it was true describes a corpus that
//! no longer exists — and the symptom is not an error but a row that
//! silently stops appearing.
//!
//! So each test here puts the pool and the truth out of step on purpose:
//! a file arrives after the pool was taken, the matching rules change under
//! it, or the query is one the rule must refuse.

use std::ffi::CString;
use std::time::Duration;

use seek_core::ffi::*;

/// A tree with a handful of files whose names overlap.
fn make_tree(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "seek-narrowing-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);

    for i in 0..20 {
        let sub = dir.join(format!("package{i}/src"));
        std::fs::create_dir_all(&sub).unwrap();
        for j in 0..5 {
            std::fs::write(sub.join(format!("template{j}.rs")), b"x").unwrap();
            std::fs::write(sub.join(format!("notes{j}.md")), b"x").unwrap();
        }
    }

    // Canonicalised, because the engine canonicalises its roots and a
    // change outside every root is dropped. On macOS the temporary
    // directory lives under `/var`, which is a symlink to `/private/var`,
    // so an uncanonicalised path here is silently outside its own root.
    // The real watcher is handed resolved paths by FSEvents.
    std::fs::canonicalize(&dir).unwrap_or(dir)
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

/// The paths a query returns, in order, with how many matched.
///
/// Paths rather than entry ids: an id is assigned during the scan, and two
/// engines over the same tree do not walk it in the same order, so their
/// ids disagree about the same file. A path is the same in both.
fn ask(engine: *mut Engine, text: &str) -> (Vec<String>, i32) {
    let query = CString::new(text).unwrap();
    let mut out = vec![
        SearchResultEntry::default();
        500
    ];
    let mut matched: i32 = 0;

    let count = unsafe {
        sk_engine_search(
            engine,
            query.as_ptr(),
            out.as_mut_ptr(),
            out.len() as i32,
            &mut matched,
        )
    };
    assert!(count >= 0, "the search failed for {text:?}");

    let paths = out
        .iter()
        .take(count as usize)
        .map(|entry| unsafe {
            let raw = sk_engine_relative_path(engine, entry.id);
            assert!(!raw.is_null(), "an id with no path");
            let path = std::ffi::CStr::from_ptr(raw).to_string_lossy().into_owned();
            sk_string_free(raw);
            path
        })
        .collect();

    (paths, matched)
}

#[test]
fn a_typed_sequence_gives_the_answer_the_first_search_gave() {
    // The comparison is made on one engine, because the answer legitimately
    // depends on the corpus: fzf's last tiebreak is the position in the
    // input, which here is the scan order, and two scans of one tree do not
    // agree on it. Within an engine the ranking is deterministic, so this
    // compares the rows and their order exactly.
    //
    // The first search is cold because the engine has just been built and
    // remembers nothing — which is not something the narrowing rule can get
    // wrong, so the reference is independent of the thing under test.
    let tree = make_tree("sequence");
    let engine = indexed_engine(&tree);

    let cold = ask(engine, "templa");

    // Each keystroke leaves a pool behind that the next one narrows from,
    // so by the end the answer is derived through five pools in a row.
    for text in ["t", "te", "tem", "temp", "templ"] {
        ask(engine, text);
    }
    let warm = ask(engine, "templa");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&tree);

    assert_eq!(warm, cold, "narrowing changed the answer");
    assert_eq!(cold.1, 100, "the tree holds a hundred template files");
}

#[test]
fn a_file_that_arrives_after_the_pool_is_still_found() {
    // The pool for `temp` is taken, then a file that matches both `temp`
    // and `tempe` appears. Reusing the stale pool would lose it, and the
    // only sign would be a row that is not there — so the assertion is on
    // the row itself, not on another search.
    //
    // The engine matches exactly, so `template0.rs` holds `temp` but not
    // `tempe`: the arrival is the whole of the second answer, and a
    // reused pool would leave the panel empty.
    let tree = make_tree("arrival");
    let engine = indexed_engine(&tree);

    let (_, before) = ask(engine, "temp");
    assert_eq!(before, 100, "a hundred template files to start with");

    let arrival = tree.join("package0/src/temperature.rs");
    std::fs::write(&arrival, b"x").unwrap();

    let path = CString::new(arrival.to_string_lossy().as_bytes()).unwrap();
    let change = FileChange {
        path: path.as_ptr(),
        kind: 0,
        is_dir: 0,
    };
    assert!(unsafe { sk_engine_apply_changes(engine, &change, 1) } > 0);

    let (rows, after) = ask(engine, "tempe");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&tree);

    assert!(
        rows.iter().any(|row| row == "package0/src/temperature.rs"),
        "the file that arrived after the pool was taken is missing"
    );
    assert_eq!(after, 1, "the file that arrived is the only one holding `tempe`");
}

#[test]
fn changing_the_case_rule_drops_the_pool() {
    // A pool taken under a case-sensitive rule is missing everything a
    // case-insensitive query would want. This one is empty, so reusing it
    // would answer `TEMPL` with nothing at all.
    let tree = make_tree("case");
    let engine = indexed_engine(&tree);

    assert_eq!(unsafe { sk_engine_set_case_sensitive(engine, 1) }, 0);
    let (_, strict) = ask(engine, "TEMP");
    assert_eq!(strict, 0, "no path is spelled in capitals");

    assert_eq!(unsafe { sk_engine_set_case_sensitive(engine, 0) }, 0);
    let (_, relaxed) = ask(engine, "TEMPL");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&tree);

    assert_eq!(
        relaxed, 100,
        "the empty pool from the strict rule was reused"
    );
}

#[test]
fn changing_the_matching_mode_drops_the_pool() {
    // The same text means something else after the switch, so the set it
    // matched cannot be narrowed from. `tempe` is in no path as a run,
    // and it is in a hundred of them as a subsequence — so a reused pool
    // would answer the fuzzy query with nothing.
    let tree = make_tree("matching");
    let engine = indexed_engine(&tree);

    let (_, exact) = ask(engine, "tempe");
    assert_eq!(exact, 0, "no path holds `tempe` as a run");

    assert_eq!(unsafe { sk_engine_set_exact_match(engine, 0) }, 0);
    let (_, fuzzy) = ask(engine, "tempe");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&tree);

    assert_eq!(
        fuzzy, 100,
        "the empty pool from the exact rule was reused"
    );
}

#[test]
fn a_widening_edit_is_answered_from_the_corpus() {
    // Backspace. `template0` matches one file per package; deleting the `0`
    // matches five. A pool can only ever shrink, so the only way to get the
    // larger number is to have gone back to the corpus.
    let tree = make_tree("widening");
    let engine = indexed_engine(&tree);

    let (_, narrow) = ask(engine, "template0");
    assert_eq!(narrow, 20, "one template0 in each of twenty packages");

    let (_, wide) = ask(engine, "template");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&tree);

    assert_eq!(wide, 100, "the widened query was answered from a stale pool");
}

#[test]
fn an_inverted_query_is_not_narrowed_from() {
    // `!template` excludes a hundred files, `!template0` only twenty, so
    // the second query matches *more* than the first. That is the direction
    // a pool cannot be reused in, and the rule has to refuse it.
    let tree = make_tree("inverted");
    let engine = indexed_engine(&tree);

    let (_, broad_exclusion) = ask(engine, "!template");
    let (_, narrow_exclusion) = ask(engine, "!template0");

    unsafe { sk_engine_free(engine) };
    let _ = std::fs::remove_dir_all(&tree);

    assert_eq!(
        narrow_exclusion - broad_exclusion,
        80,
        "excluding less should match the other eighty templates too"
    );
}
