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

use super::matcher::{Direction, FuzzyMatcher};

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

/// The tiebreak chain fzf uses under `--scheme=path`.
///
/// fzf does not only change the scoring when the path scheme is chosen; it
/// also changes the tiebreak, to `pathname,length`. Leaving the chain at
/// `length` while using the path scheme is neither of fzf's two behaviours,
/// so the scheme carries its chain with it. See [`Scheme::criteria`].
pub const PATH_CRITERIA: &[Criterion] =
    &[Criterion::Score, Criterion::Pathname, Criterion::Length];

/// The scan direction a tiebreak chain calls for.
///
/// Mirrors fzf's choice in `core.go`: the criteria do not agree about which
/// of several equally scoring occurrences is the interesting one, so the
/// chain decides the direction of the matcher.
///
/// `Pathname` and `End` want the match as late in the text as possible, and
/// `Begin` wants it as early as possible. For `Pathname` this is the whole
/// point of the criterion: in `libparse/src/main.rs` the pattern `a` can be
/// matched in the directory or in the file name, and only the later of the
/// two makes it a match on the file name. Scanning forward would credit it
/// to the directory and rank the entry behind every file-name match.
///
/// fzf walks the chain from the back towards the front, so the *earliest*
/// criterion has the final say. The score is always first and has no
/// preference, so it is skipped.
pub fn scan_direction(criteria: &[Criterion]) -> Direction {
    let mut direction = Direction::Forward;

    for &criterion in criteria.iter().skip(1).rev() {
        match criterion {
            Criterion::End | Criterion::Pathname => direction = Direction::Backward,
            Criterion::Begin => direction = Direction::Forward,
            Criterion::Score | Criterion::Chunk | Criterion::Length => {}
        }
    }

    direction
}

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
    ///
    /// `trim_length` is the candidate's length with surrounding whitespace
    /// removed. It is passed in rather than computed here because it does
    /// not depend on the query: the corpus works it out once when it is
    /// built, instead of once per candidate per keystroke. fzf does the
    /// same, caching it on the item the first time it is asked for. Use
    /// [`trim_length`] for a candidate that has no corpus behind it.
    pub fn compute(
        text: &str,
        trim_length: u16,
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

        // fzf starts every sort point at `MaxUint16` and only lowers it when
        // the criterion has something to say. A criterion that does not
        // apply therefore ranks *last*, not first.
        //
        // The offsets are valid only when the match is a real span. An empty
        // query matches everything with no span, and fzf leaves the
        // span-based criteria at their worst value in that case.
        let valid = end > start;

        // Only the criteria that are actually used are computed.
        for &criterion in criteria {
            match criterion {
                Criterion::Score => {}
                Criterion::Length => {
                    keys.trim_length = trim_length;
                }
                Criterion::Chunk => {
                    keys.chunk_length = if valid {
                        chunk_length(text, start, end)
                    } else {
                        u16::MAX
                    };
                }
                Criterion::Pathname => {
                    keys.pathname = if valid {
                        pathname_score(text, start)
                    } else {
                        u16::MAX
                    };
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

/// The number of characters in `text` once surrounding whitespace is gone.
///
/// Mirrors fzf's `Chars.TrimLength`, which trims *both* ends — it takes the
/// first and last non-space characters and measures between them.
///
/// The result does not depend on the query, so it is computed once per
/// candidate when the corpus is built and handed to [`SortKeys::compute`].
/// fzf caches it on the item for the same reason.
pub fn trim_length(text: &str) -> u16 {
    let trimmed = text.trim_matches(char::is_whitespace);

    // A path is almost always ASCII, and for ASCII the byte count is the
    // character count. `is_ascii` is a vectorised scan, where counting
    // characters decodes the string one at a time.
    let count = if trimmed.is_ascii() {
        trimmed.len()
    } else {
        trimmed.chars().count()
    };

    count.min(u16::MAX as usize) as u16
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

/// The `byPathname` criterion: fzf prefers a match that lands in the file
/// name of the path rather than in one of its directories.
///
/// Mirrors fzf's `byPathname`: when the match starts at or after the last
/// separator — that is, inside the file name — the value is
/// `minBegin - lastDelim`, so a match right after the separator wins. When
/// the match starts *before* the last separator it is a match on a
/// directory, and fzf leaves the value at its worst, which puts those
/// entries behind every match in a file name.
///
/// Returning the best value for a directory match, rather than the worst,
/// inverts the whole criterion. A path with no separator at all has
/// `lastDelim == -1`, which is less than any start, so it is scored as a
/// file name — which is what it is.
///
/// This runs for every candidate that matches, so it walks the text once
/// and allocates nothing. Collecting the characters into a vector first
/// would be clearer to read and would also put a heap allocation and a full
/// copy of the path on the hot path of every keystroke.
fn pathname_score(text: &str, start: i32) -> u16 {
    let start_byte = start.max(0) as usize;

    // The match start as a *character* index and the last separator as a
    // *byte* offset, which is the pair fzf compares:
    //
    //     lastDelim := -1
    //     s := item.text.ToString()
    //     for i := len(s) - 1; i >= 0; i-- {
    //         if s[i] == '/' || s[i] == '\\' { lastDelim = i; break }
    //     }
    //     if lastDelim <= minBegin { val = AsUint16(minBegin - lastDelim) }
    //
    // `s[i]` indexes a Go string, so `lastDelim` counts bytes, while
    // `minBegin` comes from the match offsets and counts runes. On an ASCII
    // path the two are the same number and the mix never shows. On a path
    // with a multi-byte character they are not: `a/я/hellp.txt` gets a
    // distance of 0 and outranks every ASCII path, and `a/背/hellp.txt`
    // comes out with `lastDelim > minBegin` and is read as a match that
    // spans a separator, which is the worst value there is.
    //
    // Counting both in characters is the defensible reading and is what
    // this did until the ranking was compared with fzf's over a corpus with
    // Chinese directory names in it. A ranking that disagrees with fzf is a
    // bug, so the mix is reproduced rather than corrected.
    //
    // Only the index differs, not which separator is found: `/` and `\\`
    // are ASCII, and no continuation byte of a multi-byte character can
    // equal them.
    let mut start_idx: i32 = -1;
    let mut last_delim: i32 = -1;
    let mut count: i32 = 0;

    for (byte, c) in text.char_indices() {
        if start_idx < 0 && byte >= start_byte {
            start_idx = count;
        }
        if c == '/' || c == '\\' {
            last_delim = byte as i32;
        }
        count += 1;
    }

    // A start past the end of the text clamps to the end, which is what
    // indexing the characters would have done.
    if start_idx < 0 {
        start_idx = count;
    }

    if last_delim <= start_idx {
        (start_idx - last_delim).min(u16::MAX as i32) as u16
    } else {
        u16::MAX
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
///
/// The matcher is taken by value so that the chain's scan direction can be
/// bound to it here. Handing in a matcher that was built with a different
/// direction than the chain calls for would rank the candidates by offsets
/// the matcher never meant, so the two are not allowed to drift apart.
pub fn rank_candidates(
    matcher: FuzzyMatcher,
    candidates: &[&str],
    criteria: &[Criterion],
) -> Vec<RankedMatch> {
    let matcher = matcher.scanning(scan_direction(criteria));

    let mut results: Vec<(usize, SortKeys, Vec<i32>)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, text)| {
            let (m, positions) = matcher.match_text_with_positions(text);
            if !m.is_match() {
                return None;
            }
            let keys =
                SortKeys::compute(text, trim_length(text), m.score, m.start, m.end, criteria);
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
