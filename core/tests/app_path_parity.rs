//! Differential test of the *app's* search path against real fzf.
//!
//! The existing parity test exercises the matcher with `Scheme::Default`.
//! The app does not use that: it uses `Scheme::Path` and it scores the
//! path relative to the root, not the bare file name. So the parity that
//! the test proves is not the parity the user experiences.
//!
//! This test goes through the same path the app takes — the FFI, the query
//! parser, the corpus of relative paths — and compares the result with
//! `fzf --filter` over the very same candidates.

use seek_core::corpus::Corpus;
use seek_core::fuzzy::{compare_keys, trim_length, Scheme, Scratch, SortKeys};
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, Query, SearchOptions};
use seek_core::scan::{scan_blocking, ScanConfig};
use seek_core::search::{search, span, Scorer};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Runs `fzf --filter` and returns the matching lines in fzf's order.
///
/// fzf is told which scoring scheme to use and nothing else. The tiebreak
/// is deliberately left alone: `--scheme=path` sets `pathname,length` by
/// itself, and the engine's chain comes from its scheme the same way, so
/// letting each side apply its own default is what proves they agree.
fn fzf_order(
    pattern: &str,
    candidates: &[String],
    ignore_case: bool,
    scheme: Scheme,
    matching: Matching,
) -> Option<Vec<String>> {
    let scheme = match scheme {
        Scheme::Path => "--scheme=path",
        _ => "--scheme=default",
    };

    let mut child = Command::new("fzf")
        .arg("--filter")
        .arg(pattern)
        .arg("--no-multi-line")
        .arg(scheme)
        .args(match matching {
            // fzf's own name for it. `--exact` is the long form.
            Matching::Exact => vec!["-e"],
            Matching::Fuzzy => vec![],
        })
        .args(if ignore_case { vec!["-i"] } else { vec!["+i"] })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    {
        let stdin = child.stdin.as_mut()?;
        for c in candidates {
            writeln!(stdin, "{c}").ok()?;
        }
    }

    let out = child.wait_with_output().ok()?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect(),
    )
}

/// The sort keys the app would rank `text` by.
///
/// This is the same computation `search` does, and it is what fzf's order
/// is checked against below.
fn keys_of(text: &str, query: &Query, options: &SearchOptions) -> SortKeys {
    // Which term scores, and how, is the search's own decision — `'src`
    // in exact mode is a fuzzy term and scores, a bare `src` in the same
    // query is a substring and scores over the run it found, and a glob
    // only filters. Asking `Scorer` is what keeps this test measuring the
    // app's ranking rather than a second opinion about it.
    let scorers = Scorer::compile(&query.positive, options);
    let mut scratch = Scratch::new();

    let mut score = 0i32;
    let mut score_start = 0i32;
    let mut score_end = 0i32;

    for scorer in scorers.iter() {
        let hit = scorer.match_text(text, &mut scratch);
        score += hit.score;
        span(&mut score_start, &mut score_end, hit.start, hit.end);
    }

    SortKeys::compute(
        text,
        trim_length(text),
        score,
        score_start,
        score_end,
        &options.criteria,
    )
}

/// The app's own search over a corpus, with the app's options.
fn app_order(
    corpus: &Corpus,
    pattern: &str,
    options: &SearchOptions,
    matching: Matching,
) -> Vec<String> {
    let parsed = Query::parse(pattern, matching).unwrap();
    let outcome = search(corpus, &parsed, options);

    outcome
        .results
        .iter()
        .filter_map(|result| {
            corpus
                .entries
                .iter()
                .position(|entry| entry.id == result.id)
                .map(|i| corpus.relative_path(i).unwrap_or_default().to_string())
        })
        .collect()
}

/// Builds an index and corpus from a real directory.
fn index_tree(root: &Path) -> (Index, Corpus) {
    let config = ScanConfig {
        roots: vec![root.to_path_buf()],
        ..Default::default()
    };
    let (entries, _summary) = scan_blocking(&config);

    let mut index = Index::new();
    let root_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".to_string());
    let root_id = index.add_named(None, &root_name, EntryKind::Directory, 0, 0);
    index.roots.push(root_id);
    index.config_roots.push(root.to_path_buf());

    let mut dir_ids: std::collections::HashMap<PathBuf, u32> = std::collections::HashMap::new();
    dir_ids.insert(root.to_path_buf(), root_id);

    let mut sorted = entries;
    sorted.sort_by_key(|e| e.path.components().count());

    for entry in &sorted {
        let parent_path = entry
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| root.to_path_buf());
        let parent = dir_ids.get(&parent_path).copied();
        let kind = if entry.is_dir {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        let id = index.add_named(
            parent,
            &entry.file_name,
            kind,
            entry.size.unwrap_or(0),
            entry.modified.unwrap_or(0),
        );
        if entry.is_dir {
            dir_ids.insert(entry.path.clone(), id);
        }
    }

    index.touch();
    let corpus = Corpus::build(&index);
    (index, corpus)
}

/// Builds a tree to compare over.
///
/// The tree is generated rather than borrowed from the machine the test
/// runs on. A test that points at somebody's home directory proves nothing
/// anywhere else: it passes or fails on what happens to be on that disk,
/// and it cannot fail usefully on a machine that does not have it.
///
/// The names are the ones a project tree is made of, so the patterns below
/// have something to match in file names, in directory names, and in the
/// middle of a path.
fn build_tree(tag: &str) -> PathBuf {
    // The tag keeps the two modes' trees apart. Cargo runs the tests of
    // one file in parallel threads of one process, so a root named after
    // the pid alone is a root both tests build in and one of them deletes
    // while the other is still reading it.
    let root = std::env::temp_dir().join(format!(
        "seek-app-parity-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);

    let projects = [
        "seek", "indexer", "search-core", "util-kit", "confetti", "libparse",
    ];
    let areas = ["src", "tests", "examples", "src/util", "src/index", "conf"];
    let files = [
        "main.rs",
        "lib.rs",
        "mod.rs",
        "parse.rs",
        "search.rs",
        "index.rs",
        "utils.rs",
        "core.rs",
        "README.md",
        "readme.txt",
        "Cargo.toml",
        "config.json",
        "settings.toml",
        "test_parser.rs",
        // Extensionless twins of the two commonest endings. They are
        // what makes a suffix term's *score* vary: `rs$` opens after the
        // delimiter in `src/rs` and after the dot in `src/main.rs`, and
        // fzf pays those two different bonuses. Without a pair like this
        // every path ending in `rs` opens the same way and a suffix term
        // that only filtered would rank them the same.
        "rs",
        "md",
    ];

    for project in projects {
        for area in areas {
            let dir = root.join(project).join(area);
            std::fs::create_dir_all(&dir).unwrap();

            for file in files {
                std::fs::write(dir.join(file), b"x").unwrap();
            }
        }
    }

    root
}

/// Compares the app's ranking against fzf's, over every pattern in
/// `patterns` and under both schemes.
fn parity(tag: &str, matching: Matching, patterns: &[&str]) {
    // A real tree can be compared over instead, which is useful when a
    // ranking difference is suspected on a particular machine.
    let generated = std::env::var_os("SEEK_PARITY_ROOT").is_none();
    let root = match std::env::var_os("SEEK_PARITY_ROOT") {
        Some(path) => PathBuf::from(path),
        None => build_tree(tag),
    };

    // The candidates fzf is given are the same relative paths the app
    // scores: the path from the root, which is what `fd` prints.
    let (_index, corpus) = index_tree(&root);
    let candidates: Vec<String> = corpus.paths().map(|p| p.to_string()).collect();
    assert!(
        candidates.len() > 100,
        "the corpus has only {} entries, which is too few to rank",
        candidates.len()
    );

    // The patterns are chosen to make the match *span* matter, not only the
    // score. A pattern whose characters can be found both inside a
    // directory name and inside the file name has more than one match, all
    // scoring the same, and which one is reported decides the ranking. The
    // single-letter and short fuzzy patterns are the ones that do this;
    // "src" is the case that first exposed it.
    for scheme in [Scheme::Default, Scheme::Path] {
        // The limit is lifted above the corpus, because the set that fzf
        // reports is its whole match set. Comparing a capped list against
        // an uncapped one says the sets differ as soon as a query matches
        // more entries than the panel would show, which is not a ranking
        // difference at all.
        //
        // The chain is the scheme's own, which is what the app runs with.
        let options = SearchOptions {
            scheme,
            criteria: scheme.criteria().to_vec(),
            limit: candidates.len() + 1,
            ..Default::default()
        };

        for pattern in patterns.iter().copied() {
            let Some(fzf) = fzf_order(pattern, &candidates, true, scheme, matching) else {
                eprintln!("fzf not available, skipping");
                return;
            };

            let ours = app_order(&corpus, pattern, &options, matching);



            // fzf returns the lines it matched; the app returns the same
            // set but in its own ranking. The *set* has to be identical
            // first, and that is what this test checks.
            let mut fzf_sorted = fzf.clone();
            let mut ours_sorted = ours.clone();
            fzf_sorted.sort();
            ours_sorted.sort();

            assert_eq!(
                fzf_sorted, ours_sorted,
                "the matched sets differ for {pattern:?} with {scheme:?}\n\
                 fzf: {:?}\n ours: {:?}",
                fzf_sorted, ours_sorted
            );

            // And the order has to agree too. This is what the user sees:
            // the best match has to be at the top, the way fzf puts it.
            //
            // The check is that fzf's order never contradicts the app's sort
            // keys, rather than that the two lists are identical. fzf
            // appends `index` to every tiebreak chain, so entries that tie
            // on every criterion come back in the order they were fed in —
            // and for an input this size fzf matches it in parallel chunks,
            // so which member of a tie group surfaces first is an artifact
            // of how it chunked, not a ranking it promises. Shuffling the
            // input reorders a tie group while the ranking is unchanged.
            //
            // Comparing position by position would therefore test fzf's
            // chunking. What has to hold is that every step down fzf's list
            // is a step the app agrees with: the keys never improve. Since
            // the two matched *sets* are already known to be equal and the
            // app's own list is sorted by these keys, that pins the ranking
            // and leaves only the tie order free.
            for pair in fzf.windows(2) {
                let parsed = Query::parse(pattern, matching).unwrap();
                let before = keys_of(&pair[0], &parsed, &options);
                let after = keys_of(&pair[1], &parsed, &options);

                if compare_keys(&before, &after, &options.criteria)
                    != std::cmp::Ordering::Greater
                {
                    continue;
                }

                // The app disagrees with the order fzf printed. That is not
                // yet a bug, because the printed order is not always fzf's
                // own ranking: over a list this long `fzf --filter` matches
                // in parallel and merges the partial results, and the tail
                // of the output can come out in an order fzf itself does
                // not stand behind. Feeding such an output back into fzf
                // moves entries, and asking fzf about the pair on its own
                // reverses them.
                //
                // So the pair is put back to fzf alone, which is the one
                // mode where its answer is exact. The two are fed in the
                // order fzf printed them, so input order favours the first:
                // fzf has to actively reverse them to agree with the app.
                // If it does, the printed order was the artifact. If it
                // keeps them as they were, fzf really does rank the first
                // higher and the app's ranking is wrong.
                let pair_input = vec![pair[0].clone(), pair[1].clone()];
                let Some(verdict) = fzf_order(pattern, &pair_input, true, scheme, matching) else {
                    eprintln!("fzf not available, skipping");
                    return;
                };

                assert_eq!(
                    verdict,
                    vec![pair[1].clone(), pair[0].clone()],
                    "fzf ranks {:?} above {:?} for {pattern:?} with {scheme:?}, \
                     and stands by it when asked about the two alone\n  \
                     keys of the first:  {:?}\n  keys of the second: {:?}",
                    pair[0],
                    pair[1],
                    before,
                    after
                );
            }
        }
    }

    if generated {
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
#[ignore = "requires the fzf binary; run with --ignored --nocapture"]
fn the_apps_path_scheme_matches_fzf() {
    // The patterns are chosen to make the match *span* matter, not only
    // the score. A pattern whose characters can be found both inside a
    // directory name and inside the file name has more than one match,
    // all scoring the same, and which one is reported decides the
    // ranking. The single-letter and short fuzzy patterns are the ones
    // that do this; "src" is the case that first exposed it.
    parity(
        "fuzzy",
        Matching::Fuzzy,
        &[
            "a", "s", "r", "e", "i", "t", "src", "sr", "se", "ut", "mn", "ir", "pr", "lb", "ts",
            "rm", "cn", "rs", "readme", "Cargo", "mod", "parse", "conf", "json", "toml", "main",
            "test", "lib", "index", "util", "core", "search", "src/m", "ue", "xl",
        ],
    );
}

#[test]
#[ignore = "requires the fzf binary; run with --ignored --nocapture"]
fn the_exact_mode_matches_fzfs_exact_mode() {
    // Substrings that are really there, so that there is an order to
    // disagree about, and a few that are not. `sr` and `ue` are the
    // interesting pair: they match fuzzily all over the tree and nowhere
    // as a substring, which is the whole point of the mode.
    //
    // The quoted patterns are the inversion: in exact mode `'...` is the
    // fuzzy term, and fzf reads it the same way round.
    parity(
        "exact",
        Matching::Exact,
        &[
            "s", "rs", "src", "readme", "README", "main.rs", "parse", "mod.rs", "Cargo", "conf",
            "json", "toml", "test_", "index", "util", "core", "search", "src/util", "src/m",
            "sr", "ue", "xl", "'sr", "'ue", "'src", "'readme",
        ],
    );
}

#[test]
#[ignore = "requires the fzf binary; run with --ignored --nocapture"]
fn the_anchored_terms_match_fzf() {
    // An anchored term is a literal that has to sit at one end of the
    // path, and fzf *scores* it: `PrefixMatch` and `SuffixMatch` both end
    // in the same `calculateScore` an exact term ends in. A term that only
    // filters leaves every candidate on score zero and the ranking falls
    // through to the tiebreaks, which is a different list.
    //
    // `^a$` is fzf's third anchored form, `EqualMatch`, and it is the one
    // this parser used to read as a prefix term whose text ends in a
    // dollar — a query that could never match anything.
    parity(
        "anchor",
        Matching::Exact,
        &[
            "^src",
            "^seek",
            "^indexer/src",
            "^c",
            "^s",
            "rs$",
            ".rs$",
            // The one suffix whose *score* varies: the `s` of `main.rs`
            // opens on the delimiter that precedes it, the `s` of `tests`
            // opens inside a word. A suffix term that only filtered would
            // leave the two on the same score.
            "s$",
            "e$",
            "l$",
            "README.md$",
            "^confetti$",
            "^libparse$",
            "^seek/src/main.rs$",
            "^seek rs$",
            "^seek s$",
            "^indexer s$",
            "^indexer src",
            "src s$",
        ],
    );
}
