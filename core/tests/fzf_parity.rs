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

use seek_core::fuzzy::{rank_candidates, FuzzyMatcher, Scheme};
use std::io::Write;
use std::process::{Command, Stdio};

/// Runs `fzf --filter` over `candidates` and returns the matching lines in
/// the order fzf produced them.
fn fzf_order<S: AsRef<str>>(
    pattern: &str,
    candidates: &[S],
    ignore_case: bool,
    scheme: Scheme,
) -> Option<Vec<String>> {
    let mut child = Command::new("fzf")
        .arg("--filter")
        .arg(pattern)
        .arg("--no-multi-line")
        .arg(match scheme {
            Scheme::Path => "--scheme=path",
            _ => "--scheme=default",
        })
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
fn our_order<S: AsRef<str>>(
    pattern: &str,
    candidates: &[S],
    ignore_case: bool,
    scheme: Scheme,
) -> Vec<String> {
    let m = FuzzyMatcher::new(pattern, !ignore_case, scheme);
    let refs: Vec<&str> = candidates.iter().map(|c| c.as_ref()).collect();
    rank_candidates(m, &refs, scheme.criteria())
        .into_iter()
        .map(|r| refs[r.index].to_string())
        .collect()
}

/// The set of candidates that both implementations agree match.
fn assert_same_matches(pattern: &str, candidates: &[&str], ignore_case: bool, scheme: Scheme) {
    let Some(fzf) = fzf_order(pattern, candidates, ignore_case, scheme) else {
        eprintln!("fzf not available, skipping");
        return;
    };
    let ours = our_order(pattern, candidates, ignore_case, scheme);

    let fzf_set: Vec<&String> = fzf.iter().collect();
    let our_set: Vec<&String> = ours.iter().collect();

    assert_eq!(
        fzf_set, our_set,
        "match sets differ for pattern {pattern:?} (ignore_case={ignore_case}, {scheme:?})\n fzf: {fzf:?}\n ours: {ours:?}"
    );
}

/// The relative order of matched candidates must agree too.
fn assert_same_ranking(pattern: &str, candidates: &[&str], ignore_case: bool, scheme: Scheme) {
    let Some(fzf) = fzf_order(pattern, candidates, ignore_case, scheme) else {
        eprintln!("fzf not available, skipping");
        return;
    };
    let ours = our_order(pattern, candidates, ignore_case, scheme);

    assert_eq!(
        fzf, ours,
        "rankings differ for pattern {pattern:?} (ignore_case={ignore_case}, {scheme:?})\n fzf: {fzf:?}\n ours: {ours:?}"
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
            for scheme in [Scheme::Default, Scheme::Path] {
                assert_same_matches(pattern, CANDIDATES, ignore_case, scheme);
            }
        }
    }
}

#[test]
#[ignore = "requires the fzf binary; run with --ignored --nocapture"]
fn parity_rankings() {
    for pattern in ["a", "ab", "abc", "src", "rs", "mp", "p", "br", "foo", "b"] {
        for ignore_case in [true, false] {
            for scheme in [Scheme::Default, Scheme::Path] {
                assert_same_ranking(pattern, CANDIDATES, ignore_case, scheme);
            }
        }
    }
}

/// The path scheme over paths that are not ASCII.
///
/// This is the pair the rest of the harness could not see. `our_order` was
/// pinned to the default scheme while the app runs the path one, and the
/// corpus came from a source tree and `/etc`, where every byte is ASCII and
/// a byte offset and a character index are the same number. Between them
/// they hid a ranking that disagreed with fzf on 46,460 of the 231,123 rows
/// a real query returned.
#[test]
#[ignore = "requires the fzf binary; run with --ignored --nocapture"]
fn parity_over_multibyte_paths() {
    let patterns = [
        "hellp", "main", "rs", "readme", "test", "src", "lib", "menu", "code", "m",
    ];

    for pattern in patterns {
        for ignore_case in [true, false] {
            for scheme in [Scheme::Default, Scheme::Path] {
                assert_same_matches(pattern, MULTIBYTE, ignore_case, scheme);
                assert_same_ranking(pattern, MULTIBYTE, ignore_case, scheme);
            }
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
            // Both schemes. The app runs the path one, and for a long time
            // only the default one was ever compared with fzf — which is
            // how a ranking bug lived in the scheme that ships.
            for scheme in [Scheme::Default, Scheme::Path] {
                let Some(fzf) = fzf_order(pattern, &corpus, ignore_case, scheme) else {
                    eprintln!("fzf not available, skipping");
                    return;
                };
                let ours = our_order(pattern, &corpus, ignore_case, scheme);
                if fzf != ours {
                    // Report the first divergence precisely.
                    let first = fzf
                        .iter()
                        .zip(ours.iter())
                        .position(|(a, b)| a != b);
                    panic!(
                        "ranking mismatch for pattern {pattern:?} \
                         (ignore_case={ignore_case}, {scheme:?})\n\
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
    }
    assert!(checked > 0, "no comparisons were performed");
}

/// Paths with multi-byte characters in them.
///
/// Every shape that matters for the path scheme's tiebreak, which compares
/// the byte offset of the last separator against the character index of the
/// match: a two-byte character before the separator (the offsets drift by
/// one), a three-byte one (they drift by two), several of them, and
/// multi-byte characters sitting *after* the last separator, where they
/// move the match start instead.
const MULTIBYTE: &[&str] = &[
    "a/bc/hellp.txt",
    "a/\u{80cc}/hellp.txt",
    "a/\u{44f}/hellp.txt",
    "a/bc/\u{80cc}hellp.txt",
    "\u{80cc}\u{666f}\u{77e5}\u{8bc6}/code/main.rs",
    "code/\u{80cc}\u{666f}\u{77e5}\u{8bc6}/main.rs",
    "code/main/\u{80cc}\u{666f}.rs",
    "\u{6587}\u{6863}/README.md",
    "docs/\u{6587}\u{6863}/README.md",
    "\u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442}/src/lib.rs",
    "src/\u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442}/lib.rs",
    "\u{1f4c1}/main.rs",
    "a/\u{1f4c1}/main.rs",
    "Caf\u{e9}/menu.txt",
    "menu/Caf\u{e9}.txt",
    "\u{30c6}\u{30b9}\u{30c8}/test_main.py",
    "tests/\u{30c6}\u{30b9}\u{30c8}/test_main.py",
];

/// Collects real file names from the working tree and a few system
/// directories.
///
/// The repository is the one root that is there wherever this runs, so it
/// leads. The rest are added when they happen to exist; a corpus assembled
/// only from one developer's home directory makes the comparison mean
/// something different on every machine, and nothing at all on most.
/// `SEEK_PARITY_ROOT` adds a root of your choosing.
fn real_corpus() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").to_string());

    let mut roots = vec![repository];

    if let Some(extra) = std::env::var_os("SEEK_PARITY_ROOT") {
        roots.push(extra.to_string_lossy().to_string());
    }

    for optional in ["/opt/homebrew/share", "/usr/share", "/etc"] {
        roots.push(optional.to_string());
    }

    for root in &roots {
        collect(root, &mut out, 0);
    }

    assert!(
        out.len() > 200,
        "the corpus has only {} paths, which is too few to compare rankings over",
        out.len()
    );

    // Also add bare file names so the matcher sees leaf-style inputs.
    let leaves: Vec<String> = out
        .iter()
        .take(400)
        .filter_map(|path| path.rsplit('/').next().map(|s| s.to_string()))
        .collect();
    out.extend(leaves);

    // And paths that are not ASCII, which a corpus taken from a source tree
    // and `/etc` never is. The path scheme's tiebreak compares a byte offset
    // against a character index, so a multi-byte character anywhere before
    // the match changes the ranking — and a corpus without one cannot tell.
    out.extend(MULTIBYTE.iter().map(|s| s.to_string()));

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
