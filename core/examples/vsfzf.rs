//! Measures what a keystroke costs, over the same candidate list that fzf
//! is given, so the two numbers can be put side by side.
//!
//! Usage: `cargo run --release --example vsfzf -- <corpus-file> [scheme]`

use std::time::Instant;

use seek_core::corpus::Corpus;
use seek_core::index::{EntryKind, Index};
use seek_core::fuzzy::Scheme;
use seek_core::query::{Matching, Query, SearchOptions};
use seek_core::search::search;
use std::collections::HashMap;

fn main() {
    let file = std::env::args().nth(1).expect("a corpus file");
    let scheme = match std::env::args().nth(2).as_deref() {
        Some("default") => Scheme::Default,
        _ => Scheme::Path,
    };

    let text = std::fs::read_to_string(&file).expect("read the corpus");
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();

    // Build an index whose relative paths are exactly the input lines, so
    // that what is matched here is what fzf is given.
    let start = Instant::now();
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/root".into());

    let mut dirs: HashMap<String, u32> = HashMap::new();
    let mut sorted = lines.clone();
    sorted.sort_by_key(|l| l.matches('/').count());

    for line in &sorted {
        let parts: Vec<&str> = line.split('/').collect();
        let mut parent = root;
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                index.add_named(Some(parent), part, EntryKind::File, 1, 0);
                continue;
            }
            let key = parts[..=i].join("/");
            parent = match dirs.get(&key) {
                Some(id) => *id,
                None => {
                    let id = index.add_named(Some(parent), part, EntryKind::Directory, 0, 0);
                    dirs.insert(key, id);
                    id
                }
            };
        }
    }
    index.touch();
    let build = start.elapsed();

    let start = Instant::now();
    let corpus = Corpus::build(&index);
    let corpus_build = start.elapsed();

    println!("corpus: {} lines in, {} entries", lines.len(), corpus.len());
    println!("index build: {build:?}  corpus build: {corpus_build:?}");
    println!("scheme: {scheme:?}");
    println!();

    // The keystrokes of someone typing `readme`, which is what
    // responsiveness actually means, plus a few other shapes.
    let queries = [
        "r", "re", "rea", "read", "readm", "readme",
        "src", "main.rs", "png", "zqjvw", "config",
    ];

    println!("{:<10} {:>9} {:>9} {:>10} {:>12}", "query", "limit200", "unlimited", "matched", "per-entry");
    for q in queries {
        let parsed = Query::parse(q, Matching::Fuzzy).unwrap();

        let capped = SearchOptions { limit: 200, scheme, ..Default::default() };
        let full = SearchOptions { limit: usize::MAX, scheme, ..Default::default() };

        // Best of several runs: the interesting number is what the machine
        // can do, not what it did while something else ran.
        let mut best_capped = u128::MAX;
        let mut matched = 0;
        for _ in 0..7 {
            let t = Instant::now();
            let out = search(&corpus, &parsed, &capped);
            best_capped = best_capped.min(t.elapsed().as_micros());
            matched = out.matched;
        }

        let mut best_full = u128::MAX;
        for _ in 0..3 {
            let t = Instant::now();
            let _ = search(&corpus, &parsed, &full);
            best_full = best_full.min(t.elapsed().as_micros());
        }

        println!(
            "{:<10} {:>8.1}ms {:>8.1}ms {:>10} {:>10.0}ns",
            q,
            best_capped as f64 / 1000.0,
            best_full as f64 / 1000.0,
            matched,
            best_capped as f64 * 1000.0 / corpus.len() as f64,
        );
    }
}
