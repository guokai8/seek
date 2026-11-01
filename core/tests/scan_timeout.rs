//! A scan must not be able to wedge the index for ever.
//!
//! `opendir()` on an unresponsive directory blocks in the kernel and there
//! is no way to cancel it. Measured on a dead NFS mount: fifty-four
//! minutes. For that whole time `sk_engine_finish_scan` sat in
//! `scanner.join()`, so the index was never published, the panel kept
//! saying "Indexing…", and the remaining walker threads burned 4% of a
//! core spinning on `ignore`'s 1ms quit handshake.
//!
//! So the scan carries a deadline. Once it passes, the scan reports itself
//! finished, the entries collected so far are published, and the thread
//! that is still stuck in the kernel is left to unwind on its own.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use seek_core::scan::{ScanConfig, Scanner};

/// A fresh root under the temp directory, named after the test.
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("seek-timeout-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

/// Builds a tree big enough that a single-threaded walk of it cannot be
/// mistaken for instantaneous.
fn wide_tree(root: &std::path::Path) {
    for dir in 0..40 {
        let sub = root.join(format!("d{dir}"));
        std::fs::create_dir_all(&sub).unwrap();
        for file in 0..200 {
            std::fs::write(sub.join(format!("f{file}.txt")), b"x").unwrap();
        }
    }
}

#[test]
fn an_expired_deadline_finishes_the_scan() {
    let root = temp_root("expired");
    wide_tree(&root);

    let config = ScanConfig {
        roots: vec![root.clone()],
        // One thread, 8,000 entries: tens of milliseconds of work, against
        // assertions that run microseconds after the thread is spawned.
        threads: Some(1),
        timeout: Some(Duration::ZERO),
        ..Default::default()
    };

    let scanner = Scanner::start(config).unwrap();

    assert!(
        !scanner.walk_finished(),
        "the walk cannot have returned yet; without this the test is vacuous"
    );
    assert!(
        scanner.is_finished(),
        "an expired deadline reports the scan as finished"
    );

    drop(scanner);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_expired_deadline_does_not_wait_for_the_walk() {
    let root = temp_root("nowait");
    wide_tree(&root);

    let config = ScanConfig {
        roots: vec![root.clone()],
        threads: Some(1),
        timeout: Some(Duration::ZERO),
        ..Default::default()
    };

    let scanner = Scanner::start(config).unwrap();
    assert!(!scanner.walk_finished());

    let started = Instant::now();
    let summary = scanner.finish();
    let waited = started.elapsed();

    // The point of the deadline is that this call returns rather than
    // joining a thread that may never come back.
    assert!(
        waited < Duration::from_millis(5),
        "finishing a timed-out scan waited {waited:?} for the walk"
    );
    // Nothing is claimed about a walk that was abandoned.
    assert_eq!(summary.files, 0);
    assert_eq!(summary.dirs, 0);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_deadline_that_has_not_passed_changes_nothing() {
    let root = temp_root("live");
    std::fs::write(root.join("a.txt"), b"x").unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/b.txt"), b"x").unwrap();

    let config = ScanConfig {
        roots: vec![root.clone()],
        timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    };

    let mut scanner = Scanner::start(config).unwrap();

    while !scanner.walk_finished() {
        std::thread::sleep(Duration::from_millis(5));
    }

    let mut drained = Vec::new();
    while scanner.drain_into(&mut drained) > 0 {}
    let found: Vec<String> = drained.into_iter().map(|e| e.file_name).collect();

    let summary = scanner.finish();

    assert!(found.iter().any(|n| n == "a.txt"), "{found:?}");
    assert!(found.iter().any(|n| n == "b.txt"), "{found:?}");
    assert_eq!(summary.files, 2, "the summary of a walk that did return");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_default_scan_carries_a_deadline() {
    // A launcher that runs all day cannot be left to wedge because the
    // caller forgot to set one.
    assert!(ScanConfig::default().timeout.is_some());
}
