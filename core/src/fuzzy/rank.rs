//! Result ranking, mirroring fzf's tiebreak chain.
//!
//! Original: https://github.com/junegunn/fzf — MIT License, Copyright (c) 2013-2024 Junegunn Choi.
//!
//! fzf does not sort by score alone. It builds a vector of sort points from
//! a list of criteria and compares them lexicographically. The criteria are
//! applied in the order given by `--tiebreak`, and the first criterion is
//! always `byScore`.
//!
//! # Why the sort keys are precomputed
//!
//! Several of the criteria depend on the text of the candidate: the trimmed
//! length, the length of the matched chunk and the distance to the last path
//! separator. Computing them inside the comparator would rescan the string
//! on every comparison — for a quarter of a million candidates that is
//! hundreds of millions of character operations. Instead each candidate's
//! keys are computed once, and the comparator only compares numbers.

use super::matcher::FuzzyMatcher;

/// A sort criterion, mirroring fzf's `criterion` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criterion {
    /// Higher match score is better.
    Score,
    /// Shorter matched chunk is better.
    Chunk,
    /// Shorter item is better.
    Length,
    /// Match closer to the start is better.
    Begin,
    /// Match closer to the end is better.
    End,
    /// Match nearer the final path separator is better.
    Pathname,
}

/// The default tiebreak chain, matching fzf's default `--tiebreak=length`.
pub const DEFAULT_CRITERIA: &[Criterion] = &[Criterion::Score, Criterion::Length];

/// The precomputed sort keys of a candidate.
#[derive(Debug, Clone, Copy, Default)]
pub struct SortKeys {
    /// The match score.
    pub score: i32,
    /// The length of the text after trailing whitespace is trimmed.
    pub trim_length: u16,
    /// The length of the whitespace-delimited chunk containing the match.
    pub chunk_length: u16,
    /// The distance from the match to the last path separator.
    pub pathname: u16,
    /// The byte offset of the first matched character.
    pub begin: i32,
    /// The byte offset just past the last matched character.
    pub end: i32,
}

impl SortKeys {
    /// Computes the sort keys for a candidate.
    pub fn compute(
        text: &str,
        score: i32,
        start: i32,
        end: i32,
        criteria: &[Criterion],
    ) -> SortKeys {
        let mut keys = SortKeys {
            score,
            begin: start,
            end,
            ..Default::default()
        };

        // Only the criteria that are actually used are computed.
        for &criterion in criteria {
            match criterion {
                Criterion::Score => {}
                Criterion::Length => {
                    keys.trim_length = trim_length(text);
                }
                Criterion::Chunk => {
                    keys.chunk_length = chunk_length(text, start, end);
                }
                Criterion::Pathname => {
                    keys.pathname = pathname_score(text, start);
                }
                Criterion::Begin | Criterion::End => {}
            }
        }
        keys
    }
}

/// Compares two sets of sort keys.
///
/// Returns `Ordering::Less` when `a` should come first.
pub fn compare_keys(
    a: &SortKeys,
    b: &SortKeys,
    criteria: &[Criterion],
) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    for &criterion in criteria {
        let ord = match criterion {
            // Higher score is better.
            Criterion::Score => b.score.cmp(&a.score),
            // Shorter item is better.
            Criterion::Length => a.trim_length.cmp(&b.trim_length),
            // Shorter matched chunk is better.
            Criterion::Chunk => a.chunk_length.cmp(&b.chunk_length),
            // Lower begin offset is better.
            Criterion::Begin => a.begin.cmp(&b.begin),
            // Later end offset is better.
            Criterion::End => b.end.cmp(&a.end),
            // Smaller distance to the last separator is better.
            Criterion::Pathname => a.pathname.cmp(&b.pathname),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

/// The length of the text after trailing whitespace is trimmed.
fn trim_length(text: &str) -> u16 {
    text.trim_end_matches(char::is_whitespace)
        .chars()
        .count()
        .min(u16::MAX as usize) as u16
}

/// The length of the whitespace-delimited chunk that contains the match.
///
/// Mirrors fzf's `byChunk`: the chunk is extended backwards and forwards to
/// the nearest whitespace.
fn chunk_length(text: &str, start: i32, end: i32) -> u16 {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return 0;
    }

    let start_idx = char_index_of(&chars, start.max(0) as usize);
    let end_idx = char_index_of(&chars, end.max(0) as usize);

    let mut b = start_idx;
    while b >= 1 && !chars[b - 1].is_whitespace() {
        b -= 1;
    }
    let mut e = end_idx;
    while e < chars.len() && !chars[e].is_whitespace() {
        e += 1;
    }
    (e - b).min(u16::MAX as usize) as u16
}

/// Maps a byte offset to a char index, clamping to the text bounds.
fn char_index_of(chars: &[char], byte_offset: usize) -> usize {
    let mut offset = 0usize;
    for (i, c) in chars.iter().enumerate() {
        if offset >= byte_offset {
            return i;
        }
        offset += c.len_utf8();
    }
    chars.len()
}

/// The `byPathname` criterion: how far the match is from the last path
/// separator. A smaller distance is better.
///
/// Mirrors fzf: the value is `minBegin - lastDelim`, so a match right after
/// a separator scores higher than one deep inside a directory name.
fn pathname_score(text: &str, start: i32) -> u16 {
    let chars: Vec<char> = text.chars().collect();
    let start_idx = char_index_of(&chars, start.max(0) as usize);

    let mut last_delim: i32 = -1;
    for i in (0..chars.len()).rev() {
        if chars[i] == '/' || chars[i] == '\\' {
            last_delim = i as i32;
            break;
        }
    }
    if last_delim <= start_idx as i32 {
        (start_idx as i32 - last_delim).min(u16::MAX as i32) as u16
    } else {
        0
    }
}

/// A scored, ranked result.
#[derive(Debug, Clone)]
pub struct RankedMatch {
    /// Index into the original candidate list.
    pub index: usize,
    /// The match score.
    pub score: i32,
    /// Byte offset of the first matched character.
    pub start: i32,
    /// Byte offset just past the last matched character.
    pub end: i32,
    /// Matched byte offsets, one per pattern character.
    pub positions: Vec<i32>,
}

/// Ranks candidate strings against a pattern using fzf's criteria chain.
pub fn rank_candidates(
    matcher: &FuzzyMatcher,
    candidates: &[&str],
    criteria: &[Criterion],
) -> Vec<RankedMatch> {
    let mut results: Vec<(usize, SortKeys, Vec<i32>)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, text)| {
            let (m, positions) = matcher.match_text_with_positions(text);
            if !m.is_match() {
                return None;
            }
            let keys = SortKeys::compute(text, m.score, m.start, m.end, criteria);
            Some((index, keys, positions))
        })
        .collect();

    results.sort_by(|a, b| compare_keys(&a.1, &b.1, criteria));

    results
        .into_iter()
        .map(|(index, keys, positions)| RankedMatch {
            index,
            score: keys.score,
            start: keys.begin,
            end: keys.end,
            positions,
        })
        .collect()
}
