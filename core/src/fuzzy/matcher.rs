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
    CharClass, CharClasses, MatchResult, Position, Scheme, BONUS_BOUNDARY, BONUS_CONSECUTIVE,
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

    /// The underlying bytes, when every character is a single ASCII byte.
    ///
    /// The prefilter uses this to search with `memchr` rather than walking
    /// the characters one at a time, which is where it spends most of its
    /// time. A source that is not plain ASCII answers `None` and gets the
    /// character-by-character scan.
    fn ascii_bytes(&self) -> Option<&[u8]> {
        None
    }

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
    fn ascii_bytes(&self) -> Option<&[u8]> {
        Some(self.bytes)
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

/// Which of several equally scoring occurrences is reported as the match.
///
/// When a pattern character occurs more than once with the same score, the
/// matcher has to pick one, and the choice is visible in the ranking: the
/// reported span is what the `Begin`, `End`, `Chunk` and `Pathname`
/// tiebreaks are measured from.
///
/// fzf decides the direction from the tiebreak chain rather than fixing it,
/// because the criteria disagree about which occurrence is the interesting
/// one. `Pathname` wants the match as late as possible, so that a pattern
/// that could match a directory is still credited to the file name. See
/// [`scan_direction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Scan forward and keep the first best occurrence.
    #[default]
    Forward,
    /// Scan backward and keep the last best occurrence.
    Backward,
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
    /// Which equally scoring occurrence is reported as the match.
    direction: Direction,
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
            direction: Direction::Forward,
        }
    }

    /// Sets which equally scoring occurrence is reported as the match.
    ///
    /// The direction belongs to the tiebreak chain, not to the pattern, so
    /// it is derived with [`scan_direction`] rather than chosen by hand.
    pub fn scanning(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// Whether a score at this occurrence displaces the best seen so far.
    ///
    /// Scanning forward keeps the first of several equal scores and
    /// scanning backward keeps the last, which is the whole of the
    /// difference between the two directions.
    fn improves(&self, score: i16, best: i16) -> bool {
        match self.direction {
            Direction::Forward => score > best,
            Direction::Backward => score >= best,
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

    /// The highest score this pattern can reach against any haystack.
    ///
    /// Every matched character is worth [`SCORE_MATCH`] plus a bonus, the
    /// first character's bonus counts twice, and a gap only ever
    /// subtracts. So the best conceivable match is every character landing
    /// on the best boundary this scheme offers, with no gaps at all.
    ///
    /// This is what makes it possible to leave a candidate unscored: a
    /// candidate whose ceiling is below a score already achieved by enough
    /// others cannot reach the rows that will be shown.
    pub fn score_ceiling(&self) -> i32 {
        let m = self.pattern.len() as i32;
        if m == 0 {
            return 0;
        }
        let bonus = self.classes.max_bonus() as i32;

        m * SCORE_MATCH as i32
            + bonus * BONUS_FIRST_CHAR_MULTIPLIER as i32
            + (m - 1) * bonus
    }

    /// The characters a consecutive run of this pattern has to follow for
    /// the match to reach [`FuzzyMatcher::score_ceiling`] — or `None` when
    /// the ceiling does not require that.
    ///
    /// This sharpens the bound the search prunes with. A path that does
    /// not hold the pattern as a consecutive run pays a gap and so scores
    /// at least three below the ceiling; but *holding* the run is not
    /// enough to reach the ceiling either. The ceiling pays the scheme's
    /// best bonus for every character, and the best bonus is only ever
    /// earned by following one particular class of character — a path
    /// separator under [`Scheme::Path`], whitespace under
    /// [`Scheme::Default`]. A run anywhere else starts one bonus lower,
    /// and because a run's later characters inherit the run's first bonus,
    /// every character of it is one lower too: at least `m + 1` below the
    /// ceiling, so at least three for a pattern of two characters or more.
    ///
    /// So for the great majority of a broad query's matches — the ones
    /// that hold the query in the middle of a word — this says "cannot be
    /// shown" where the consecutive-run bound has to say "score it and
    /// see".
    ///
    /// `None` is returned when that argument does not hold:
    ///
    /// * a pattern of one character, where the run is the whole match and
    ///   there are no inherited bonuses to be one lower;
    /// * a pattern that is not plain ASCII, because the test this feeds is
    ///   byte-wise while the matcher folds accents; and
    /// * a pattern that **carries its own boundary** — one where some
    ///   character pays the best bonus to the character after it, as the
    ///   `/` of `/b` does. Such a run earns the best bonus from inside
    ///   itself no matter where it sits, which lifts it to two below the
    ///   ceiling: exactly the threshold the prune arms at, and so exactly
    ///   a row that could tie for the last place shown.
    pub fn ceiling_boundary(&self) -> Option<CeilingBoundary> {
        // The raw pattern, not the folded one: this feeds a byte-wise
        // test, and `tést` folds to the ASCII `test` while the bytes the
        // caller searches for are still the accented ones.
        if self.pattern.len() < 2 || !self.pattern_raw.is_ascii() {
            return None;
        }

        let max = self.classes.max_bonus();
        // A run's later characters inherit the greater of the run's first
        // bonus and `BONUS_CONSECUTIVE`, so the argument needs the best
        // bonus to be the larger of the two. It is, by a wide margin, in
        // both schemes — but it is a dependency between constants, and
        // the alternative to checking it is a bound that quietly stops
        // holding if one of them is ever changed.
        if max <= BONUS_CONSECUTIVE {
            return None;
        }

        // A character of the pattern may be matched in either case, which
        // can put it in a different class, so both are considered. Taking
        // the union rather than the exact case is the conservative
        // direction: it can only refuse more patterns.
        let classes_of = |c: char| {
            [
                self.classes.class_of_char(c.to_ascii_lowercase()),
                self.classes.class_of_char(c.to_ascii_uppercase()),
            ]
        };

        // Does the pattern carry its own boundary? If any character of it
        // can pay the best bonus to the character after it, a run earns
        // that bonus wherever it sits, and the bound does not hold. `/b`
        // is the case that matters: `score_bound.rs` has it scoring two
        // below the ceiling in the middle of a name, which is exactly
        // where the prune arms.
        for pair in self.pattern.windows(2) {
            for before in classes_of(pair[0]) {
                for after in classes_of(pair[1]) {
                    if self.classes.bonus_for(before, after) >= max {
                        return None;
                    }
                }
            }
        }

        // What is left is the set of predecessors that *do* pay the best
        // bonus to the pattern's first character. Derived from the bonus
        // matrix rather than written down, so that a change to the
        // scheme's constants moves the set with it.
        let first = classes_of(self.pattern[0]);
        let pays = |before: CharClass| {
            first
                .iter()
                .any(|&after| self.classes.bonus_for(before, after) >= max)
        };

        let mut bytes = 0u128;
        for b in 0u8..128 {
            if pays(self.classes.class_of_byte(b)) {
                bytes |= 1u128 << b;
            }
        }

        Some(CeilingBoundary {
            bytes,
            at_start: pays(self.classes.initial_char_class()),
        })
    }

    /// Whether the pattern occurs in `haystack` as a subsequence at all.
    ///
    /// This is the matcher's first phase on its own — it answers whether
    /// the candidate matches, without scoring it. A match that is going to
    /// be left out of the ranking still has to be counted, and counting is
    /// all this does.
    pub fn occurs_in(&self, haystack: &str) -> bool {
        if self.pattern.is_empty() {
            return true;
        }

        let bytes = haystack.as_bytes();
        if bytes.is_ascii() {
            if self.pattern.len() > bytes.len() {
                return false;
            }
            self.fuzzy_index(&AsciiBytes::new(bytes)).is_some()
        } else {
            let decoded = DecodedChars::new(haystack);
            if self.pattern.len() > decoded.len() {
                return false;
            }
            self.fuzzy_index(&decoded).is_some()
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

        // Phases 2 to 4 live in a function of their own, which is
        // deliberately not inlined. They are the bulk of the matcher's
        // code, and a single-character pattern never reaches them: with
        // them inline here, `run_inner` grew large enough that the
        // one-character path measured 15% slower purely from the code it
        // shared a function body with.
        self.score_matrix(source, min_idx, max_idx, want_positions, scratch)
    }

    /// Phases 2 to 4: the bonus row, the score matrix and the backtrace.
    ///
    /// Split out of [`FuzzyMatcher::run_inner`] so that the short paths — an
    /// empty pattern, a pattern longer than the haystack, and above all a
    /// single-character pattern, which is every query's first keystroke —
    /// do not pay for the size of this one.
    #[inline(never)]
    fn score_matrix<S: CharSource>(
        &self,
        source: &S,
        min_idx: usize,
        max_idx: usize,
        want_positions: bool,
        scratch: &mut Scratch,
    ) -> (MatchResult, Vec<Position>) {
        let m = self.pattern.len();

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
            positions: trail,
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

        // The matrix is not cleared, only grown. Every one of its cells is
        // either written by the loop below or zeroed by hand just before
        // the row that holds it, so none of the previous candidate's values
        // survives. Clearing it first would write `width * m` zeros per
        // candidate — a couple of kilobytes for a long path — and the loop
        // is about to overwrite most of them.
        h.resize(width * m, 0);
        h[..width].copy_from_slice(&h0[f0..=last_idx]);

        c.resize(width * m, 0);
        c[..width].copy_from_slice(&c0[f0..=last_idx]);

        let mut max_score: i16 = 0;
        let mut max_score_pos = 0usize;

        for row_idx in 1..m {
            let f_off = f[row_idx] as usize;
            let pchar = self.pattern[row_idx];
            let base = row_idx * width;

            // The two rows this iteration touches: the one filled last
            // time, which it reads diagonally, and the one it fills. They
            // are split out so that the inner loop indexes a slice of
            // known length instead of computing an offset into the whole
            // matrix on each of six reads.
            let (h_prev, h_cur) = h[base - width..base + width].split_at_mut(width);
            let (c_prev, c_cur) = c[base - width..base + width].split_at_mut(width);

            // The first column this row fills. The greedy first
            // occurrences are strictly increasing, so a row below the
            // first always starts at least one column in, which is what
            // makes the diagonal `lo - 1` a cell that exists.
            let lo = f_off - f0;
            let cols = last_idx + 1 - f_off;

            // The columns before the first one this row fills are never
            // written by the loop, and the backtrace reads them, so they
            // are zeroed here rather than by clearing the whole matrix.
            h_cur[..lo].fill(0);
            c_cur[..lo].fill(0);

            // Every one of these is exactly `cols` long and indexed by the
            // same `k`, which is what lets the bounds checks fall away.
            let t_row = &t[f_off..f_off + cols];
            let bonus_row = &bonus[f_off..f_off + cols];
            let h_diag = &h_prev[lo - 1..lo - 1 + cols];
            let c_diag = &c_prev[lo - 1..lo - 1 + cols];
            let h_row = &mut h_cur[lo..lo + cols];
            let c_row = &mut c_cur[lo..lo + cols];

            let mut in_gap = false;

            // The score of the cell to the left is the one the previous
            // iteration just computed, so it is carried in a register
            // rather than read back out of the row. The first column has
            // no left neighbour and fzf scores it as if it were zero.
            let mut hleft: i16 = 0;

            for k in 0..cols {
                // s2: skip this haystack character.
                let s2 = if in_gap {
                    hleft + SCORE_GAP_EXTENSION
                } else {
                    hleft + SCORE_GAP_START
                };

                // s1: match the pattern character here.
                let mut s1 = 0i16;
                let mut consecutive = 0i16;

                if pchar == t_row[k] {
                    s1 = h_diag[k] + SCORE_MATCH;
                    let mut b = bonus_row[k];
                    consecutive = c_diag[k] + 1;
                    if consecutive > 1 {
                        let col = f_off + k;
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
                        s1 += bonus_row[k];
                        consecutive = 0;
                    } else {
                        s1 += b;
                    }
                }

                c_row[k] = consecutive;
                in_gap = s1 < s2;
                let score = s1.max(s2).max(0);
                if row_idx == m - 1 && self.improves(score, max_score) {
                    max_score = score;
                    max_score_pos = f_off + k;
                }
                h_row[k] = score;
                hleft = score;
            }
        }

        // Phase 4: backtrace to recover the matched positions.
        //
        // The backtrace runs for every candidate, not only the ones whose
        // positions are wanted, because it is what tells us where the match
        // actually begins. `f0` is only the first place the pattern's first
        // character *could* land, which is a different thing: in
        // `libparse/src` the pattern `src` can begin at the `s` of
        // `libparse`, while the match fzf settles on begins at the `s` after
        // the separator. The tiebreaks are measured from the start, so
        // reporting `f0` ranks such an entry as a match on a directory.
        //
        // fzf makes the same distinction the other way around: it skips the
        // backtrace when no positions are wanted, and therefore sets
        // `withPos` whenever the tiebreak chain needs a true start. The
        // backtrace walks one row where the matrix filled `m` of them, so
        // running it always costs a fraction of the work already done and
        // leaves no way for the two to disagree.
        self.backtrace(h, c, f, m, width, f0, last_idx, max_score_pos, trail);

        let positions = if want_positions {
            trail
                .iter()
                .map(|&p| source.byte_offset(min_idx + p as usize) as Position)
                .collect()
        } else {
            Vec::new()
        };

        let start_char = min_idx + trail.first().map_or(f0, |&p| p as usize);
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
    ///
    /// The offsets are written into `positions` in ascending order, which is
    /// the caller's reusable buffer: the backtrace runs for every candidate,
    /// so it must not allocate.
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
        positions: &mut Vec<i32>,
    ) {
        positions.clear();
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
                positions.push(j as i32);
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
    }

    /// Matches a single-character pattern, mirroring fzf's
    /// `fuzzyMatchV2Single` fast path.
    ///
    /// When scanning forward, fzf *stops* at the first occurrence whose
    /// bonus reaches `BONUS_BOUNDARY`, rather than taking the
    /// highest-scoring occurrence. This matters: a later occurrence may
    /// score higher, but fzf never sees it. For example, in
    /// `regex-automata/dfa/accel.rs` the first boundary-bonus `a` follows a
    /// `-` (score 32), while a later `a` follows a `/` (score 34) — fzf
    /// reports 32.
    ///
    /// The stop only happens when the occurrence improved on the best score
    /// so far. An occurrence at a boundary that is *worse* than one already
    /// seen does not end the scan, so a still better one further on is
    /// still found.
    ///
    /// The best-scoring occurrence wins; which of several equal scores is
    /// kept is [`Direction`]'s to decide.
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

            if self.improves(score, max_score) {
                max_score = score;
                max_score_pos = Some(idx);

                // Scanning forward, fzf stops as soon as a boundary bonus
                // is reached. Scanning backward it cannot stop, because the
                // occurrence it wants is the last one.
                if self.direction == Direction::Forward && bonus >= BONUS_BOUNDARY {
                    break;
                }
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

    /// The byte, or pair of bytes, that fold to an ASCII needle.
    ///
    /// The pattern was folded when the matcher was compiled, so a
    /// case-insensitive needle is already lowercase and the only other
    /// byte that can fold onto it is its uppercase form. A needle that is
    /// not ASCII has no byte form and answers `None`.
    #[inline]
    fn ascii_needle(&self, needle: char) -> Option<(u8, u8)> {
        if !needle.is_ascii() {
            return None;
        }
        let lower = needle as u8;
        let upper = if self.case_sensitive {
            lower
        } else {
            lower.to_ascii_uppercase()
        };
        Some((lower, upper))
    }

    /// Finds the next occurrence of `needle` at or after `from`.
    fn find_char<S: CharSource>(&self, source: &S, from: usize, needle: char) -> Option<usize> {
        if let (Some(bytes), Some((lower, upper))) =
            (source.ascii_bytes(), self.ascii_needle(needle))
        {
            let hay = bytes.get(from..)?;
            return memchr::memchr2(lower, upper, hay).map(|i| i + from);
        }
        (from..source.len()).find(|&i| self.fold(source.get(i)) == needle)
    }

    /// Finds the last occurrence of `needle` at or after `from`.
    fn rfind_char<S: CharSource>(&self, source: &S, from: usize, needle: char) -> Option<usize> {
        if let (Some(bytes), Some((lower, upper))) =
            (source.ascii_bytes(), self.ascii_needle(needle))
        {
            let hay = bytes.get(from..)?;
            return memchr::memrchr2(lower, upper, hay).map(|i| i + from);
        }
        (from..source.len()).rev().find(|&i| self.fold(source.get(i)) == needle)
    }

    /// Folds a haystack character for comparison: lowercases it when
    /// case-insensitive, then applies accent normalization when enabled.
    #[inline]
    fn fold(&self, c: char) -> char {
        // Almost every character of almost every path is ASCII, and this
        // runs several times per character per candidate — once in the
        // prefilter's forward scan, once in its backward scan, and once
        // more when the window is built. The ASCII case is therefore split
        // out and kept to a comparison and a conditional subtraction.
        //
        // Nothing is skipped by doing so: `normalize_char` folds accented
        // Latin letters, and it already answers `None` for every code point
        // below U+00C0, so it has nothing to say about an ASCII character.
        if c.is_ascii() {
            return if !self.case_sensitive && c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            };
        }

        let c = if self.case_sensitive {
            c
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

/// The characters a consecutive run must follow to reach the ceiling.
///
/// Built by [`FuzzyMatcher::ceiling_boundary`], which is where the
/// argument for it is written down. It is a set of ASCII bytes plus
/// whether the start of the haystack counts as one, kept as a bitmask
/// because it is asked about once per candidate on every keystroke.
#[derive(Debug, Clone, Copy)]
pub struct CeilingBoundary {
    /// The ASCII bytes a run may follow, one bit each.
    bytes: u128,
    /// Whether a run at offset 0 can reach the ceiling.
    at_start: bool,
}

impl CeilingBoundary {
    /// Whether a run beginning after `prev` can reach the ceiling.
    ///
    /// `None` is the start of the haystack. A non-ASCII predecessor
    /// answers `true`: the only use of this is to dismiss a candidate, so
    /// a wrong `false` drops a row while a wrong `true` only costs the
    /// work of scoring one.
    pub fn follows(&self, prev: Option<u8>) -> bool {
        match prev {
            None => self.at_start,
            Some(b) if b.is_ascii() => self.bytes & (1u128 << b) != 0,
            Some(_) => true,
        }
    }
}
