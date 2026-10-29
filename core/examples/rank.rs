//! Prints this implementation's ranking for a candidate list, for manual
//! comparison against `fzf --filter`.
//!
//! Usage: `cargo run --example rank -- <pattern> <file-with-candidates> [ignore_case]`

use fuzzylite_core::fuzzy::{rank_candidates, FuzzyMatcher, Scheme, DEFAULT_CRITERIA};
use std::env;
use std::fs;

fn main() {
    let args: Vec<String> = env::args().collect();
    let pattern = args.get(1).cloned().unwrap_or_else(|| "a".into());
    let file = args.get(2).cloned().unwrap_or_else(|| "/dev/stdin".into());
    let ignore_case = args.get(3).map(|s| s != "0").unwrap_or(true);
    let scheme = match args.get(4).map(|s| s.as_str()) {
        Some("path") => Scheme::Path,
        _ => Scheme::Default,
    };

    let content = fs::read_to_string(&file).unwrap_or_default();
    let candidates: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();

    let m = FuzzyMatcher::new(&pattern, !ignore_case, scheme);
    let ranked = rank_candidates(&m, &candidates, DEFAULT_CRITERIA);

    for r in ranked {
        println!("{}", candidates[r.index]);
    }
}
