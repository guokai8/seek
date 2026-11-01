//! Fuzzy matching engine, ported from fzf.
//!
//! See `matcher.rs` for the algorithm and `score.rs` for the scoring model.

pub mod exact;
pub mod literal;
pub mod matcher;
pub mod normalize;
pub mod rank;
pub mod scratch;
pub mod score;

#[cfg(test)]
mod tests;

pub use exact::{Anchor, ExactMatcher};
pub use matcher::{score_text, CeilingBoundary, Direction, FuzzyMatcher};
pub use normalize::normalize_char;
pub use rank::{
    compare_keys, rank_candidates, scan_direction, trim_length, Criterion, RankedMatch, SortKeys,
    DEFAULT_CRITERIA, PATH_CRITERIA,
};
pub use scratch::Scratch;
pub use score::{CharClass, CharClasses, MatchResult, Position, Scheme};
