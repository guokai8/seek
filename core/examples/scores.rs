//! Prints the score of each candidate against a pattern, for debugging
//! ranking differences against fzf.
//!
//! Usage: `cargo run --example scores -- <pattern> <file-with-candidates> [ignore_case]`

use fuzzylite_core::fuzzy::{FuzzyMatcher, Scheme};
use std::env;
use std::fs;

fn main() {
    let args: Vec<String> = env::args().collect();
    let pattern = args.get(1).cloned().unwrap_or_else(|| "a".into());
    let file = args.get(2).cloned().unwrap_or_else(|| "/dev/stdin".into());
    let ignore_case = args.get(3).map(|s| s != "0").unwrap_or(true);

    let content = fs::read_to_string(&file).unwrap_or_default();
    let candidates: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();

    let m = FuzzyMatcher::new(&pattern, !ignore_case, Scheme::Default);
    for c in &candidates {
        let (r, _) = m.match_text_with_positions(c);
        println!(
            "score={:5} start={:4} end={:4} len={:4}  {}",
            r.score,
            r.start,
            r.end,
            c.chars().count(),
            c
        );
    }
}
