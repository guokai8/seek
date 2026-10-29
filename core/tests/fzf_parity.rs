//! Differential test harness: compares this port's ranking against the real
//! `fzf` binary.
//!
//! The fzf binary is not required for `cargo test` to pass; when it is not on
//! PATH these tests are skipped. Run them explicitly with:
//!
//! ```text
//! cargo test --test fzf_parity -- --ignored --nocapture
//! ```
//!
//! The harness feeds the same candidate list to fzf (`--filter`) and to this
//! implementation, then asserts that the two orderings agree.

use fuzzylite_core::fuzzy::{rank_candidates, FuzzyMatcher, Scheme, DEFAULT_CRITERIA};
use std::io::Write;
use std::process::{Command, Stdio};

/// Runs `fzf --filter` over `candidates` and returns the matching lines in
/// the order fzf produced them.
fn fzf_order<S: AsRef<str>>(pattern: &str, candidates: &[S], ignore_case: bool) -> Option<Vec<String>> {
    let mut child = Command::new("fzf")
        .arg("--filter")
        .arg(pattern)
        .arg("--no-multi-line")
        .args(if ignore_case {
            vec!["-i"]
        } else {
            vec!["+i"]
        })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    {
        let stdin = child.stdin.as_mut()?;
        for c in candidates {
            writeln!(stdin, "{}", c.as_ref()).ok()?;
        }
    }

    let out = child.wait_with_output().ok()?;
    if !out.status.success() && !out.status.code().is_some_and(|c| c == 1) {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect(),
    )
}

/// This implementation's ranking of `candidates`.
fn our_order<S: AsRef<str>>(pattern: &str, candidates: &[S], ignore_case: bool) -> Vec<String> {
    let m = FuzzyMatcher::new(pattern, !ignore_case, Scheme::Default);
    let refs: Vec<&str> = candidates.iter().map(|c| c.as_ref()).collect();
    rank_candidates(&m, &refs, DEFAULT_CRITERIA)
        .into_iter()
        .map(|r| refs[r.index].to_string())
        .collect()
}

/// The set of candidates that both implementations agree match.
fn assert_same_matches(pattern: &str, candidates: &[&str], ignore_case: bool) {
    let Some(fzf) = fzf_order(pattern, candidates, ignore_case) else {
        eprintln!("fzf not available, skipping");
        return;
    };
    let ours = our_order(pattern, candidates, ignore_case);

    let fzf_set: Vec<&String> = fzf.iter().collect();
    let our_set: Vec<&String> = ours.iter().collect();

    assert_eq!(
        fzf_set, our_set,
        "match sets differ for pattern {pattern:?} (ignore_case={ignore_case})\n fzf: {fzf:?}\n ours: {ours:?}"
    );
}

/// The relative order of matched candidates must agree too.
fn assert_same_ranking(pattern: &str, candidates: &[&str], ignore_case: bool) {
    let Some(fzf) = fzf_order(pattern, candidates, ignore_case) else {
        eprintln!("fzf not available, skipping");
        return;
    };
    let ours = our_order(pattern, candidates, ignore_case);

    assert_eq!(
        fzf, ours,
        "rankings differ for pattern {pattern:?} (ignore_case={ignore_case})\n fzf: {fzf:?}\n ours: {ours:?}"
    );
}

const CANDIDATES: &[&str] = &[
    "src/main.rs",
    "src/lib.rs",
    "src/parser/mod.rs",
    "src/parser/lexer.rs",
    "tests/integration.rs",
    "README.md",
    "Cargo.toml",
    "docs/search-syntax.md",
    "src/parser/parser.rs",
    "MainThread.java",
    "src/MAIN.rs",
    "a_b_c.txt",
    "abc.txt",
    "axbxc.txt",
    "Application Support",
    "foo/bar/baz.rs",
    "FooBar.rs",
    "foobar.rs",
];

#[test]
#[ignore = "requires the fzf binary; run with --ignored --nocapture"]
fn parity_match_sets() {
    for pattern in ["a", "ab", "abc", "src", "rs", "mp", "p", "br", "foo", "b"] {
        for ignore_case in [true, false] {
            assert_same_matches(pattern, CANDIDATES, ignore_case);
        }
    }
}

#[test]
#[ignore = "requires the fzf binary; run with --ignored --nocapture"]
fn parity_rankings() {
    for pattern in ["a", "ab", "abc", "src", "rs", "mp", "p", "br", "foo", "b"] {
        for ignore_case in [true, false] {
            assert_same_ranking(pattern, CANDIDATES, ignore_case);
        }
    }
}

/// A wide sweep over many patterns and a large corpus of real file names.
///
/// This is the test that actually matters: it exercises the scoring model,
/// the tiebreak chain and the normalization table against the reference
/// implementation over realistic inputs.
#[test]
#[ignore = "requires the fzf binary; run with --ignored --nocapture"]
fn parity_sweep_over_real_corpus() {
    // Build a corpus from the filesystem so the inputs are realistic.
    let corpus = real_corpus();
    assert!(
        corpus.len() > 200,
        "expected a real corpus, got {} entries",
        corpus.len()
    );
    let patterns = [
        "a", "e", "s", "r", "t", "n", "i", "o", "rs", "st", "co", "re", "in", "ex", "on",
        "readme", "Cargo", "src", "lib", "test", "mod", "parse", "conf", "json", "toml",
        "abc", "xyz", "aa", "ss", "cfg", "md",
    ];

    let mut checked = 0;
    for pattern in patterns {
        for ignore_case in [true, false] {
            let Some(fzf) = fzf_order(pattern, &corpus, ignore_case) else {
                eprintln!("fzf not available, skipping");
                return;
            };
            let ours = our_order(pattern, &corpus, ignore_case);
            if fzf != ours {
                // Report the first divergence precisely.
                let first = fzf
                    .iter()
                    .zip(ours.iter())
                    .position(|(a, b)| a != b);
                panic!(
                    "ranking mismatch for pattern {pattern:?} (ignore_case={ignore_case})\n\
                     first divergence at index {:?}\n  fzf:  {:?}\n  ours: {:?}\n\
                     fzf len={} ours len={}",
                    first,
                    first.map(|i| &fzf[i]),
                    first.map(|i| &ours[i]),
                    fzf.len(),
                    ours.len()
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no comparisons were performed");
}

/// Collects real file names from the working tree and a few system
/// directories.
fn real_corpus() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let roots = [
        "/Users/bioguo/projects",
        "/Users/bioguo/tmp/fd-src/fd-master/src",
        "/Users/bioguo/tmp/fzf-src/fzf-master/src",
        "/Users/bioguo/.cargo/registry/src",
        "/opt/homebrew/share",
        "/etc",
    ];

    for root in roots {
        collect(root, &mut out, 0);
    }

    // Also add bare file names so the matcher sees leaf-style inputs.
    let leaves: Vec<String> = out
        .iter()
        .take(400)
        .filter_map(|path| path.rsplit('/').next().map(|s| s.to_string()))
        .collect();
    out.extend(leaves);

    out.sort();
    out.dedup();
    out
}

fn collect(dir: &str, out: &mut Vec<String>, depth: usize) {
    if depth > 6 || out.len() > 3000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        out.push(path.to_string_lossy().to_string());
        if path.is_dir() {
            collect(&path.to_string_lossy(), out, depth + 1);
        }
    }
}
