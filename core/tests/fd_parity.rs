//! Differential test harness: compares this project's traversal against the
//! real `fd` binary.
//!
//! fzf's ranking has a parity test (`fzf_parity.rs`). fd's traversal
//! deserves the same treatment: the rule is that seek shows exactly
//! what fd shows, no more and no less. A divergence is a bug in one of the
//! two.
//!
//! The `fd` binary is not required for `cargo test` to pass; when it is not
//! on `PATH` these tests are skipped. Run them explicitly with:
//!
//! ```text
//! cargo test --test fd_parity -- --ignored --nocapture
//! ```

use seek_core::scan::{scan_blocking, ScanConfig};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A file tree to compare the two traversals over.
fn build_fixture(root: &Path) {
    let dirs = [
        "sub",
        "sub/deeper",
        ".hidden-dir",
        "node_modules",
        "target",
    ];

    for dir in dirs {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }

    let files = [
        "a.txt",
        "b.md",
        ".hidden-file",
        "sub/c.txt",
        "sub/deeper/d.rs",
        "node_modules/pkg.js",
        "target/debug.bin",
        ".hidden-dir/secret.txt",
    ];

    for file in files {
        std::fs::write(root.join(file), "x").unwrap();
    }

    // A `.gitignore` that fd honours, and so must this project. fd only
    // reads a `.gitignore` when it is inside a git repository, so the
    // fixture is made one.
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".gitignore"), "ignored-by-gitignore.txt\n").unwrap();
    std::fs::write(root.join("ignored-by-gitignore.txt"), "x").unwrap();

    // A `.fdignore`, which is fd's own exclusion file.
    std::fs::write(root.join(".fdignore"), "ignored-by-fdignore.txt\n").unwrap();
    std::fs::write(root.join("ignored-by-fdignore.txt"), "x").unwrap();
}

/// The paths `fd` prints for the fixture, relative to the root.
///
/// fd prints the path it was given, so the root's prefix is stripped here
/// to leave the same shape that this project produces.
fn fd_paths(root: &Path) -> Option<BTreeSet<String>> {
    let output = Command::new("fd")
        .arg("--type")
        .arg("f")
        .arg("--type")
        .arg("d")
        .arg("--exclude")
        .arg("node_modules")
        .arg("--exclude")
        .arg("target")
        .arg(".")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?
        .wait_with_output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                // fd prints the paths relative to the root, with a trailing
                // slash on the directories.
                let line = line.trim_end_matches('/');

                Path::new(line)
                    .strip_prefix(root)
                    .unwrap_or(Path::new(line))
                    .to_string_lossy()
                    .to_string()
            })
            .collect(),
    )
}

/// The paths this project finds for the fixture, relative to the root.
fn our_paths(root: &Path) -> BTreeSet<String> {
    let config = ScanConfig {
        roots: vec![root.to_path_buf()],
        exclude_patterns: vec!["node_modules".to_string(), "target".to_string()],
        ..Default::default()
    };

    let (entries, _) = scan_blocking(&config);

    entries
        .into_iter()
        .map(|entry| {
            entry
                .path
                .strip_prefix(root)
                .unwrap_or(&entry.path)
                .to_string_lossy()
                .to_string()
        })
        .collect()
}

/// The fixture's directory, cleaned up afterwards.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("seek-fd-parity-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        build_fixture(&root);
        Fixture { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
#[ignore = "needs the fd binary"]
fn parity_the_same_paths_are_found_as_fd() {
    let fixture = Fixture::new("paths");

    let Some(expected) = fd_paths(&fixture.root) else {
        eprintln!("fd is not on PATH; skipping");
        return;
    };

    let actual = our_paths(&fixture.root);

    let missing: Vec<_> = expected.difference(&actual).collect();
    let extra: Vec<_> = actual.difference(&expected).collect();

    assert!(
        missing.is_empty(),
        "fd found these and we did not: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "we found these and fd did not: {extra:?}"
    );
}

#[test]
#[ignore = "needs the fd binary"]
fn parity_hidden_files_are_skipped_like_fd() {
    let fixture = Fixture::new("hidden");

    let Some(expected) = fd_paths(&fixture.root) else {
        eprintln!("fd is not on PATH; skipping");
        return;
    };

    let actual = our_paths(&fixture.root);

    for path in &actual {
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        assert!(
            !name.starts_with('.'),
            "the hidden file {path} should have been skipped"
        );
    }

    assert!(
        expected.contains("a.txt"),
        "the fixture is broken: {expected:?}"
    );
}

#[test]
#[ignore = "needs the fd binary"]
fn parity_gitignore_and_fdignore_are_honoured() {
    let fixture = Fixture::new("ignores");

    let Some(expected) = fd_paths(&fixture.root) else {
        eprintln!("fd is not on PATH; skipping");
        return;
    };

    let actual = our_paths(&fixture.root);

    // The `.gitignore` and `.fdignore` files are themselves hidden, so
    // they are not in either set; the files they exclude must be absent
    // from both.
    assert!(
        !actual.contains("ignored-by-gitignore.txt"),
        "the .gitignore was not honoured"
    );
    assert!(
        !actual.contains("ignored-by-fdignore.txt"),
        "the .fdignore was not honoured"
    );
    assert!(
        !expected.contains("ignored-by-gitignore.txt"),
        "fd did not honour the .gitignore"
    );
    assert!(
        !expected.contains("ignored-by-fdignore.txt"),
        "fd did not honour the .fdignore"
    );
}

#[test]
#[ignore = "needs the fd binary"]
fn parity_the_excluded_patterns_leave_the_tree_like_fd() {
    let fixture = Fixture::new("excludes");

    let Some(expected) = fd_paths(&fixture.root) else {
        eprintln!("fd is not on PATH; skipping");
        return;
    };

    let actual = our_paths(&fixture.root);

    for path in &actual {
        assert!(
            !path.contains("node_modules"),
            "the excluded directory {path} is still there"
        );
        assert!(
            !path.contains("target"),
            "the excluded directory {path} is still there"
        );
    }

    assert_eq!(
        expected.len(),
        actual.len(),
        "fd found {} paths and we found {}",
        expected.len(),
        actual.len()
    );
}
