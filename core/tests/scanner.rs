//! A check of the scanner's lifecycle.
//!
//! The scanner streams entries from a background walk into a shared buffer.
//! This test verifies that the walk finishes and that every entry is
//! delivered, which is what the app relies on.

use fuzzylite_core::scan::{scan_blocking, ScanConfig};

#[test]
fn the_scan_delivers_every_entry() {
    let root = std::env::temp_dir().join(format!("fuzzylite-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    // A tree with a known number of files.
    std::fs::create_dir_all(root.join("a/b/c")).unwrap();
    for i in 0..10 {
        std::fs::write(root.join(format!("a/file{i}.txt")), b"x").unwrap();
    }
    for i in 0..5 {
        std::fs::write(root.join(format!("a/b/c/deep{i}.txt")), b"x").unwrap();
    }

    let config = ScanConfig {
        roots: vec![root.clone()],
        ..Default::default()
    };

    let (entries, summary) = scan_blocking(&config);

    // The tree holds 15 files and 3 sub-directories; the root itself is
    // not a result.
    assert_eq!(entries.len(), 18, "every file and directory must be delivered");
    assert_eq!(summary.files, 15);
    assert_eq!(summary.dirs, 3);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_scan_finishes_even_without_a_consumer() {
    // The walker must not block when the buffer is never drained, which is
    // what happens when the app waits for the scan before reading it.
    let root = std::env::temp_dir().join(format!("fuzzylite-scan2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    std::fs::create_dir_all(&root).unwrap();
    for i in 0..50 {
        std::fs::write(root.join(format!("f{i}.txt")), b"x").unwrap();
    }

    let config = ScanConfig {
        roots: vec![root.clone()],
        ..Default::default()
    };

    let mut scanner = fuzzylite_core::scan::Scanner::start(config).unwrap();

    // Wait for the walk without reading anything.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !scanner.walk_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "the walk did not finish while the buffer was not drained"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Now the entries can be drained.
    let mut count = 0;
    while scanner.next_entry().is_some() {
        count += 1;
    }
    assert_eq!(count, 50);

    let _ = std::fs::remove_dir_all(&root);
}
