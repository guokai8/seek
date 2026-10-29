//! Fuzzy matching engine, ported from fzf.
//!
//! See `matcher.rs` for the algorithm and `score.rs` for the scoring model.

pub mod matcher;
pub mod normalize;
pub mod rank;
pub mod scratch;
pub mod score;

#[cfg(test)]
mod tests;

pub use matcher::{score_text, FuzzyMatcher};
pub use normalize::normalize_char;
pub use rank::{
    compare_keys, rank_candidates, Criterion, RankedMatch, SortKeys, DEFAULT_CRITERIA,
};
pub use scratch::Scratch;
pub use score::{CharClass, CharClasses, MatchResult, Scheme};
