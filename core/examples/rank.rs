//! Prints this implementation's ranking for a candidate list, for manual
//! comparison against `fzf --filter`.
//!
//! Usage: `cargo run --example rank -- <pattern> <file-with-candidates> [ignore_case]`

use seek_core::fuzzy::{rank_candidates, FuzzyMatcher, Scheme};
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

    // The chain has to come from the scheme. Passing `DEFAULT_CRITERIA`
    // here regardless of it drops `Pathname` from the path scheme, and the
    // ordering that comes out is neither of fzf's — which made this
    // example disagree with fzf while the library agreed with it.
    let m = FuzzyMatcher::new(&pattern, !ignore_case, scheme);
    let ranked = rank_candidates(m, &candidates, scheme.criteria());

    for r in ranked {
        println!("{}", candidates[r.index]);
    }
}
