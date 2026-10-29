//! Rust port of fzf's V2 fuzzy matcher.
//!
//! Original: https://github.com/junegunn/fzf — MIT License, Copyright (c) 2013-2024 Junegunn Choi.
//!
//! The algorithm is a Smith-Waterman style dynamic program over
//! (pattern char, haystack char) pairs. For each cell it computes the best
//! score of:
//!
//! * `s1` — matching the pattern character here (diagonal), or
//! * `s2` — skipping the haystack character (left),
//!
//! and keeps whichever is larger. Omission of pattern characters is never
//! allowed, so every pattern char must be matched, but the *alignment* may
//! skip any number of haystack chars in between (at a gap penalty).
//!
//! This is a direct transcription of fzf's `FuzzyMatchV2`, with two
//! deliberate simplifications:
//!
//! * The SIMD byte-search helpers (`indexByteTwo`) are replaced with plain
//!   scans, which give identical results.
//! * fzf's `util.Slab` buffer reuse is replaced by fresh `Vec` allocations.
//!   The allocation cost is negligible next to the O(N*M) matrix fill.
//!
//! # Performance
//!
//! fzf operates on *bytes* when the input is pure ASCII and on *runes*
//! otherwise. This port does the same, because the byte path needs no
//! allocation at all — which matters enormously when matching hundreds of
//! thousands of candidates on every keystroke. The two paths are the same
//! algorithm, written once against the [`CharSource`] trait so that the
//! compiler specialises it for each representation.

use super::normalize::normalize_char;
use super::scratch::Scratch;
use super::score::{
    CharClasses, MatchResult, Position, Scheme, BONUS_BOUNDARY, BONUS_CONSECUTIVE,
    BONUS_FIRST_CHAR_MULTIPLIER, SCORE_GAP_EXTENSION, SCORE_GAP_START, SCORE_MATCH,
};

/// A sequence of characters that the matcher can walk.
///
/// Implemented for raw ASCII bytes (the common case, no allocation) and for
/// decoded characters (non-ASCII names).
pub trait CharSource {
    /// The number of characters.
    fn len(&self) -> usize;

    /// The character at an index.
    fn get(&self, index: usize) -> char;

    /// The byte offset of the character at an index.
    fn byte_offset(&self, index: usize) -> usize;

    /// The byte offset just past the last character.
    fn byte_len(&self) -> usize;

    /// The byte offset just past the character at `index`.
    ///
    /// This is `byte_offset(index + 1)`, except for the last character,
    /// where it is the total byte length.
    fn byte_offset_after(&self, index: usize) -> usize {
        if index + 1 < self.len() {
            self.byte_offset(index + 1)
        } else {
            self.byte_len()
        }
    }
}

/// A haystack of pure ASCII bytes.
///
/// Byte offsets are character offsets, so no lookup table is needed.
pub struct AsciiBytes<'a> {
    bytes: &'a [u8],
}

impl<'a> AsciiBytes<'a> {
    /// Wraps a byte slice.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl CharSource for AsciiBytes<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    fn get(&self, index: usize) -> char {
        self.bytes[index] as char
    }

    #[inline]
    fn byte_offset(&self, index: usize) -> usize {
        index
    }

    #[inline]
    fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}

/// A haystack of decoded characters.
///
/// Byte offsets require a lookup table, so this is only used for names that
/// are not pure ASCII.
pub struct DecodedChars {
    chars: Vec<char>,
    /// The byte offset of each character, plus a trailing end offset.
    offsets: Vec<usize>,
}

impl DecodedChars {
    /// Decodes a string into characters and a byte-offset table.
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let mut offsets = Vec::with_capacity(chars.len() + 1);
        let mut offset = 0usize;
        for &c in &chars {
            offsets.push(offset);
            offset += c.len_utf8();
        }
        offsets.push(offset);
        Self { chars, offsets }
    }
}

impl CharSource for DecodedChars {
    #[inline]
    fn len(&self) -> usize {
        self.chars.len()
    }

    #[inline]
    fn get(&self, index: usize) -> char {
        self.chars[index]
    }

    #[inline]
    fn byte_offset(&self, index: usize) -> usize {
        self.offsets[index]
    }

    #[inline]
    fn byte_len(&self) -> usize {
        self.offsets[self.chars.len()]
    }
}

/// A compiled query, ready to be matched against many haystacks.
pub struct FuzzyMatcher {
    /// Pattern characters, lowercased when case-insensitive.
    pattern: Vec<char>,
    /// Original pattern, kept for exact-phrase matching.
    pattern_raw: String,
    /// Whether matching is case sensitive.
    case_sensitive: bool,
    /// Whether accented Latin letters are folded to their base letter.
    /// This mirrors fzf's default behaviour; `fzf --literal` disables it.
    normalize: bool,
    /// Character classification tables.
    classes: CharClasses,
}

impl FuzzyMatcher {
    /// Compiles a pattern into a matcher.
    pub fn new(pattern: &str, case_sensitive: bool, scheme: Scheme) -> Self {
        Self::with_options(pattern, case_sensitive, scheme, true)
    }

    /// Compiles a pattern into a matcher with full control over behaviour.
    ///
    /// `normalize` folds accented Latin letters to their base letter, which
    /// is what fzf does by default. Pass `false` to match `fzf --literal`.
    pub fn with_options(
        pattern: &str,
        case_sensitive: bool,
        scheme: Scheme,
        normalize: bool,
    ) -> Self {
        let pattern_raw = pattern.to_string();
        let pattern: Vec<char> = pattern
            .chars()
            .map(|c| {
                let c = if case_sensitive {
                    c
                } else {
                    c.to_lowercase().next().unwrap_or(c)
                };
                if normalize {
                    normalize_char(c).unwrap_or(c)
                } else {
                    c
                }
            })
            .collect();

        Self {
            pattern,
            pattern_raw,
            case_sensitive,
            normalize,
            classes: CharClasses::new(scheme),
        }
    }

    /// Returns true when the matcher has no pattern (matches everything).
    pub fn is_empty(&self) -> bool {
        self.pattern.is_empty()
    }

    /// The original, unmodified pattern.
    pub fn pattern(&self) -> &str {
        &self.pattern_raw
    }

    /// Matches `haystack` against the compiled pattern.
    ///
    /// This is the allocation-free path: it reports the score and the match
    /// span, but not the individual matched offsets. Use
    /// [`FuzzyMatcher::match_text_with_positions`] when the positions are
    /// needed for highlighting.
    pub fn match_text(&self, haystack: &str) -> MatchResult {
        if self.pattern.is_empty() {
            return MatchResult {
                start: 0,
                end: 0,
                score: 0,
            };
        }

        let mut scratch = Scratch::new();
        self.match_text_fast(haystack, &mut scratch)
    }

    /// Matches `haystack`, reporting the matched byte offsets.
    ///
    /// The returned positions are byte offsets into `haystack`, in ascending
    /// order, one per pattern character. The UI uses them to highlight the
    /// matched characters.
    pub fn match_text_with_positions(&self, haystack: &str) -> (MatchResult, Vec<Position>) {
        let mut scratch = Scratch::new();
        self.match_text_with_scratch(haystack, &mut scratch)
    }

    /// Matches `haystack`, reusing a caller-owned scratch buffer.
    ///
    /// This is the API to use when scoring many candidates: the buffer is
    /// allocated once and reused, so no allocation happens per candidate.
    pub fn match_text_with_scratch(
        &self,
        haystack: &str,
        scratch: &mut Scratch,
    ) -> (MatchResult, Vec<Position>) {
        if self.pattern.is_empty() {
            return (
                MatchResult {
                    start: 0,
                    end: 0,
                    score: 0,
                },
                Vec::new(),
            );
        }

        let bytes = haystack.as_bytes();
        if bytes.is_ascii() {
            self.run_inner::<AsciiBytes<'_>>(&AsciiBytes::new(bytes), true, scratch)
        } else {
            let decoded = DecodedChars::new(haystack);
            self.run_inner::<DecodedChars>(&decoded, true, scratch)
        }
    }

    /// Matches `haystack` without collecting the matched positions.
    ///
    /// This is the fast path used when ranking: the positions are only
    /// needed for the handful of results that are actually displayed.
    pub fn match_text_fast(&self, haystack: &str, scratch: &mut Scratch) -> MatchResult {
        if self.pattern.is_empty() {
            return MatchResult {
                start: 0,
                end: 0,
                score: 0,
            };
        }

        let bytes = haystack.as_bytes();
        if bytes.is_ascii() {
            self.run_inner::<AsciiBytes<'_>>(&AsciiBytes::new(bytes), false, scratch).0
        } else {
            let decoded = DecodedChars::new(haystack);
            self.run_inner::<DecodedChars>(&decoded, false, scratch).0
        }
    }

    /// The core algorithm, shared by both entry points.
    fn run_inner<S: CharSource>(
        &self,
        source: &S,
        want_positions: bool,
        scratch: &mut Scratch,
    ) -> (MatchResult, Vec<Position>) {
        let m = self.pattern.len();
        let n = source.len();
        if m > n {
            return (MatchResult::NO_MATCH, Vec::new());
        }

        // Phase 1: narrow the haystack to the window that can contain the
        // pattern.
        let (min_idx, max_idx) = match self.fuzzy_index(source) {
            Some(v) => v,
            None => return (MatchResult::NO_MATCH, Vec::new()),
        };

        // Single-character patterns have a dedicated fast path in fzf that
        // avoids the score matrix entirely.
        if m == 1 {
            return self.match_single(source, min_idx, max_idx);
        }

        // The candidate window, folded for comparison.
        //
        // The scratch buffers are borrowed all at once so that the compiler
        // can see the borrows do not overlap.
        let Scratch {
            window,
            bonus,
            h0,
            c0,
            f,
            h,
            c,
            positions: _,
        } = scratch;

        window.clear();
        window.reserve(max_idx - min_idx);
        for i in min_idx..max_idx {
            window.push(self.fold(source.get(i)));
        }
        let t = window.as_slice();

        // Phase 2: compute the per-position bonus, the first score row, and
        // the first occurrence of each pattern character.
        bonus.clear();
        bonus.reserve(t.len());
        h0.clear();
        h0.resize(t.len(), 0);
        c0.clear();
        c0.resize(t.len(), 0);
        f.clear();
        f.resize(m, 0);

        let mut prev_class = self.classes.initial_char_class();
        let mut pidx = 0usize;
        let mut last_idx = 0usize;
        let pchar0 = self.pattern[0];
        let mut pchar = pchar0;
        let mut prev_h0: i16 = 0;
        let mut in_gap = false;

        for off in 0..t.len() {
            let raw = source.get(min_idx + off);
            let class = self.classes.class_of_char(raw);
            let ch = t[off];

            let b = self.classes.bonus_for(prev_class, class);
            bonus.push(b);
            prev_class = class;

            if ch == pchar {
                if pidx < m {
                    f[pidx] = off as u32;
                    pidx += 1;
                    pchar = self.pattern[pidx.min(m - 1)];
                }
                last_idx = off;
            }

            if ch == pchar0 {
                h0[off] = SCORE_MATCH + b * BONUS_FIRST_CHAR_MULTIPLIER;
                c0[off] = 1;
                in_gap = false;
            } else {
                h0[off] = if in_gap {
                    (prev_h0 + SCORE_GAP_EXTENSION).max(0)
                } else {
                    (prev_h0 + SCORE_GAP_START).max(0)
                };
                c0[off] = 0;
                in_gap = true;
            }
            prev_h0 = h0[off];
        }

        if pidx != m {
            return (MatchResult::NO_MATCH, Vec::new());
        }

        // Phase 3: fill in the score matrix.
        let f0 = f[0] as usize;
        let width = last_idx - f0 + 1;

        h.clear();
        h.resize(width * m, 0);
        h[..width].copy_from_slice(&h0[f0..=last_idx]);

        c.clear();
        c.resize(width * m, 0);
        c[..width].copy_from_slice(&c0[f0..=last_idx]);

        let mut max_score: i16 = 0;
        let mut max_score_pos = 0usize;

        for row_idx in 1..m {
            let f_off = f[row_idx] as usize;
            let pchar = self.pattern[row_idx];
            let row = row_idx * width;
            let mut in_gap = false;

            for col in f_off..=last_idx {
                let local = col - f0;

                // s2: skip this haystack character.
                let hleft = if col == f_off { 0 } else { h[row + local - 1] };
                let s2 = if in_gap {
                    hleft + SCORE_GAP_EXTENSION
                } else {
                    hleft + SCORE_GAP_START
                };

                // s1: match the pattern character here.
                let mut s1 = 0i16;
                let mut consecutive = 0i16;

                if pchar == t[col] {
                    let hdiag = h[row + local - 1 - width];
                    let cdiag = c[row + local - 1 - width];
                    s1 = hdiag + SCORE_MATCH;
                    let mut b = bonus[col];
                    consecutive = cdiag + 1;
                    if consecutive > 1 {
                        let fb = bonus[col.saturating_sub(consecutive as usize - 1)];
                        // Break the consecutive chunk when a better boundary
                        // appears, otherwise keep the highest bonus.
                        if b >= BONUS_BOUNDARY && b > fb {
                            consecutive = 1;
                        } else {
                            b = b.max(BONUS_CONSECUTIVE).max(fb);
                        }
                    }
                    if s1 + b < s2 {
                        s1 += bonus[col];
                        consecutive = 0;
                    } else {
                        s1 += b;
                    }
                }

                c[row + local] = consecutive;
                in_gap = s1 < s2;
                let score = s1.max(s2).max(0);
                if row_idx == m - 1 && score > max_score {
                    max_score = score;
                    max_score_pos = col;
                }
                h[row + local] = score;
            }
        }

        // Phase 4: backtrace to recover the matched positions. This is only
        // done when the caller actually needs them.
        let positions = if want_positions {
            let raw = self.backtrace(h, c, f, m, width, f0, last_idx, max_score_pos);
            raw.into_iter()
                .map(|p| source.byte_offset(min_idx + p) as Position)
                .collect()
        } else {
            Vec::new()
        };

        let start_char = min_idx + f0;
        let end_char = (min_idx + max_score_pos + 1).min(source.len());
        (
            MatchResult {
                start: source.byte_offset(start_char) as Position,
                end: source.byte_offset_after(end_char.saturating_sub(1)) as Position,
                score: max_score as i32,
            },
            positions,
        )
    }

    /// Walks the score matrix backwards to recover matched char offsets.
    #[allow(clippy::too_many_arguments)]
    fn backtrace(
        &self,
        h: &[i16],
        c: &[i16],
        f: &[u32],
        m: usize,
        width: usize,
        f0: usize,
        last_idx: usize,
        max_score_pos: usize,
    ) -> Vec<usize> {
        let mut positions = Vec::with_capacity(m);
        let mut i = m as i32 - 1;
        let mut j = max_score_pos;
        let mut prefer_match = true;

        while i >= 0 && j >= f0 {
            // fzf captures the row index before `i` is decremented, and uses
            // that captured value for the look-ahead into the next row.
            let row_idx = i as usize;
            let row = row_idx * width;
            let j0 = j - f0;
            let s = h[row + j0];

            let mut s1 = 0i16;
            if i > 0 && j >= f[row_idx] as usize && j0 >= 1 {
                s1 = h[row - width + j0 - 1];
            }
            let mut s2 = 0i16;
            if j > f[row_idx] as usize && j0 >= 1 {
                s2 = h[row + j0 - 1];
            }

            if s > s1 && (s > s2 || (s == s2 && prefer_match)) {
                positions.push(j);
                if i == 0 {
                    break;
                }
                i -= 1;
            }

            // The look-ahead reads the next row of the matrix. The index must
            // stay inside the matrix, which this port sizes exactly.
            let look_ahead = row + width + j0 + 1;
            let next_row_ok = row_idx + 1 < m && j < last_idx && look_ahead < c.len();
            prefer_match = c[row + j0] > 1
                || (next_row_ok
                    && (j + 1) as u32 >= f[row_idx + 1]
                    && c[look_ahead] > 0);
            j = j.saturating_sub(1);
        }

        positions.reverse();
        positions
    }

    /// Matches a single-character pattern, mirroring fzf's
    /// `fuzzyMatchV2Single` fast path.
    ///
    /// fzf scans forward and *stops* at the first occurrence whose bonus
    /// reaches `BONUS_BOUNDARY`, rather than taking the highest-scoring
    /// occurrence. This matters: a later occurrence may score higher, but
    /// fzf never sees it. For example, in `regex-automata/dfa/accel.rs` the
    /// first boundary-bonus `a` follows a `-` (score 32), while a later `a`
    /// follows a `/` (score 34) — fzf reports 32.
    ///
    /// The best-scoring occurrence wins; ties keep the leftmost one.
    fn match_single<S: CharSource>(
        &self,
        source: &S,
        min_idx: usize,
        max_idx: usize,
    ) -> (MatchResult, Vec<Position>) {
        let needle = self.pattern[0];
        let mut max_score: i16 = 0;
        let mut max_score_pos: Option<usize> = None;

        for idx in min_idx..max_idx {
            let c = source.get(idx);
            if self.fold(c) != needle {
                continue;
            }

            let class = self.classes.class_of_char(c);
            let prev_class = if idx > 0 {
                self.classes.class_of_char(source.get(idx - 1))
            } else {
                self.classes.initial_char_class()
            };
            let bonus = self.classes.bonus_for(prev_class, class);
            let score = SCORE_MATCH + bonus * BONUS_FIRST_CHAR_MULTIPLIER;

            if score > max_score {
                max_score = score;
                max_score_pos = Some(idx);
            }

            // fzf stops as soon as a boundary bonus is reached.
            if bonus >= BONUS_BOUNDARY {
                break;
            }
        }

        match max_score_pos {
            Some(idx) => (
                MatchResult {
                    start: source.byte_offset(idx) as Position,
                    end: source.byte_offset_after(idx) as Position,
                    score: max_score as i32,
                },
                vec![source.byte_offset(idx) as Position],
            ),
            None => (MatchResult::NO_MATCH, Vec::new()),
        }
    }

    /// Finds the window of the haystack that can contain the pattern.
    ///
    /// Mirrors fzf's `asciiFuzzyIndex`: it locates the first occurrence of
    /// each pattern character in order, then extends the window to the last
    /// possible occurrence of the final pattern character.
    ///
    /// Returns `None` when the pattern is not a subsequence at all.
    fn fuzzy_index<S: CharSource>(&self, source: &S) -> Option<(usize, usize)> {
        let m = self.pattern.len();
        let mut idx = 0usize;
        let mut first_idx = 0usize;
        let mut last_idx = 0usize;

        for (pidx, &p) in self.pattern.iter().enumerate() {
            // `find_char` returns an absolute index.
            let abs = self.find_char(source, idx, p)?;
            if pidx == 0 && abs > 0 {
                // Step back one char so the first match can see the class of
                // the preceding character when the bonus is computed.
                first_idx = abs - 1;
            }
            last_idx = abs;
            idx = abs + 1;
        }

        // Extend the window through the last occurrence of the final pattern
        // character, so alternative alignments are considered.
        let last_char = self.pattern[m - 1];
        if let Some(end) = self.rfind_char(source, last_idx + 1, last_char) {
            return Some((first_idx, end + 1));
        }
        Some((first_idx, last_idx + 1))
    }

    /// Finds the next occurrence of `needle` at or after `from`.
    fn find_char<S: CharSource>(&self, source: &S, from: usize, needle: char) -> Option<usize> {
        (from..source.len()).find(|&i| self.fold(source.get(i)) == needle)
    }

    /// Finds the last occurrence of `needle` at or after `from`.
    fn rfind_char<S: CharSource>(&self, source: &S, from: usize, needle: char) -> Option<usize> {
        (from..source.len()).rev().find(|&i| self.fold(source.get(i)) == needle)
    }

    /// Folds a haystack character for comparison: lowercases it when
    /// case-insensitive, then applies accent normalization when enabled.
    #[inline]
    fn fold(&self, c: char) -> char {
        let c = if self.case_sensitive {
            c
        } else if c.is_ascii_uppercase() {
            c.to_ascii_lowercase()
        } else {
            c.to_lowercase().next().unwrap_or(c)
        };
        if self.normalize {
            normalize_char(c).unwrap_or(c)
        } else {
            c
        }
    }
}

/// Convenience wrapper: score a haystack against a pattern without keeping a
/// compiled matcher around. Useful for tests and one-off scoring.
pub fn score_text(pattern: &str, haystack: &str, case_sensitive: bool, scheme: Scheme) -> i32 {
    FuzzyMatcher::new(pattern, case_sensitive, scheme).match_text(haystack).score
}
