//! Port of fzf's fuzzy matching algorithm (src/algo/algo.go) to Rust.
//!
//! Original: https://github.com/junegunn/fzf — MIT License, Copyright (c) 2013-2024 Junegunn Choi.
//!
//! This module re-implements fzf's V2 fuzzy matcher and its scoring model so that
//! results rank identically to fzf. The core ideas are:
//!
//! * Characters are classified (white / non-word / delimiter / lower / upper /
//!   letter / number) and a bonus is awarded for matches that land on a word
//!   boundary, after a delimiter, or at a camelCase hump.
//! * Matches are found with a dynamic-programming score matrix (Smith-Waterman
//!   style, no omission allowed) so that the *best* alignment is scored, not
//!   merely the leftmost one.
//! * Gaps are penalised (`-3` to open, `-1` to extend) while consecutive
//!   matches are rewarded, which is what makes `ab` rank `a_b` above `axb`.
//!
//! The scoring constants below are verbatim from fzf.

/// A match position within the haystack.
pub type Position = i32;

/// Scoring constants, verbatim from fzf's `src/algo/algo.go`.
pub const SCORE_MATCH: i16 = 16;
pub const SCORE_GAP_START: i16 = -3;
pub const SCORE_GAP_EXTENSION: i16 = -1;
pub const BONUS_BOUNDARY: i16 = SCORE_MATCH / 2;
pub const BONUS_CONSECUTIVE: i16 = -(SCORE_GAP_START + SCORE_GAP_EXTENSION);
pub const BONUS_FIRST_CHAR_MULTIPLIER: i16 = 2;

/// Result of a fuzzy match attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchResult {
    /// Byte offset of the first matched character.
    pub start: Position,
    /// Byte offset just past the last matched character.
    pub end: Position,
    /// Match quality score. Higher is better.
    pub score: i32,
}

impl MatchResult {
    /// No match sentinel, matching fzf's `Result{-1, -1, 0}`.
    pub const NO_MATCH: MatchResult = MatchResult {
        start: -1,
        end: -1,
        score: 0,
    };

    /// Returns true when this result represents an actual match.
    pub fn is_match(&self) -> bool {
        self.start >= 0
    }
}

// --- fzf scoring constants (verbatim from src/algo/algo.go) ---
//
// The public constants are declared at the top of this file.

/// Bonus for non-word characters.
const BONUS_NON_WORD: i16 = SCORE_MATCH / 2;
/// Edge-triggered bonus for camelCase humps; smaller than a word boundary
/// because camelCase matches do not accompany single-character gaps.
const BONUS_CAMEL123: i16 = BONUS_BOUNDARY + SCORE_GAP_EXTENSION;

/// Character classes, mirroring fzf's `charClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CharClass {
    White = 0,
    NonWord = 1,
    Delimiter = 2,
    Lower = 3,
    Upper = 4,
    Letter = 5,
    Number = 6,
}

const CHAR_NUMBER: usize = CharClass::Number as usize;

/// The scoring "scheme". fzf exposes these as `--scheme=`, and the `path`
/// variant is exactly what a file search UI wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// General purpose scheme.
    Default,
    /// Path-aware scheme: `/` is a delimiter and the initial class is
    /// `Delimiter`, which biases matches toward the start of a path segment.
    Path,
}

impl Scheme {
    /// Characters treated as delimiters.
    fn delimiter_chars(&self) -> &'static [u8] {
        match self {
            Scheme::Default => b"/,:;|",
            Scheme::Path => b"/",
        }
    }

    /// Extra bonus for a word boundary after whitespace.
    fn bonus_boundary_white(&self) -> i16 {
        match self {
            Scheme::Default => BONUS_BOUNDARY + 2,
            Scheme::Path => BONUS_BOUNDARY,
        }
    }

    /// Extra bonus for a word boundary after a delimiter.
    fn bonus_boundary_delimiter(&self) -> i16 {
        BONUS_BOUNDARY + 1
    }

    /// The class assumed for the character preceding the haystack.
    fn initial_char_class(&self) -> CharClass {
        match self {
            Scheme::Default => CharClass::White,
            Scheme::Path => CharClass::Delimiter,
        }
    }

    /// The tiebreak chain fzf uses with this scheme.
    ///
    /// fzf's `--scheme` changes more than the scoring: `--scheme=path` also
    /// sets `--tiebreak=pathname,length`. The chain belongs to the scheme
    /// for that reason — choosing one and leaving the other behind gives an
    /// ordering that is neither of fzf's.
    pub fn criteria(&self) -> &'static [crate::fuzzy::Criterion] {
        match self {
            Scheme::Default => crate::fuzzy::DEFAULT_CRITERIA,
            Scheme::Path => crate::fuzzy::PATH_CRITERIA,
        }
    }
}

/// Character classification tables, precomputed for a given scheme.
pub struct CharClasses {
    ascii: [CharClass; 128],
    delimiters: &'static [u8],
    bonus_matrix: [[i16; CHAR_NUMBER + 1]; CHAR_NUMBER + 1],
    initial_char_class: CharClass,
    /// The bonus for the character that opens the haystack.
    ///
    /// fzf's `bonusAt` pays this at index 0 whatever the character there
    /// is, which is not the same as `bonus_for(White, class)` for every
    /// class. See [`crate::fuzzy::exact::ExactMatcher`], which is the only
    /// caller: the fuzzy matcher reaches the same bonus through the
    /// matrix, from the scheme's initial class.
    bonus_boundary_white: i16,
    /// The largest bonus any class transition under this scheme pays.
    ///
    /// Taken from the matrix rather than written down, so that it cannot
    /// drift away from the bonuses actually in use. It is what bounds the
    /// best score a pattern can reach; see
    /// [`crate::fuzzy::FuzzyMatcher::score_ceiling`].
    max_bonus: i16,
}

impl CharClasses {
    /// Builds the classification tables for a scheme.
    pub fn new(scheme: Scheme) -> Self {
        let delimiters = scheme.delimiter_chars();
        let mut ascii = [CharClass::NonWord; 128];

        for (i, slot) in ascii.iter_mut().enumerate() {
            let c = i as u8;
            *slot = if c.is_ascii_lowercase() {
                CharClass::Lower
            } else if c.is_ascii_uppercase() {
                CharClass::Upper
            } else if c.is_ascii_digit() {
                CharClass::Number
            } else if c.is_ascii_whitespace() {
                CharClass::White
            } else if delimiters.contains(&c) {
                CharClass::Delimiter
            } else {
                CharClass::NonWord
            };
        }

        let mut bonus_matrix = [[0i16; CHAR_NUMBER + 1]; CHAR_NUMBER + 1];
        for i in 0..=CHAR_NUMBER {
            for j in 0..=CHAR_NUMBER {
                bonus_matrix[i][j] =
                    bonus_for(CharClass::from_index(i), CharClass::from_index(j), scheme);
            }
        }

        let max_bonus = bonus_matrix
            .iter()
            .flat_map(|row| row.iter())
            .copied()
            .max()
            .unwrap_or(0);

        Self {
            ascii,
            delimiters,
            bonus_matrix,
            initial_char_class: scheme.initial_char_class(),
            bonus_boundary_white: scheme.bonus_boundary_white(),
            max_bonus,
        }
    }

    /// The class assumed for the character preceding the haystack.
    pub fn initial_char_class(&self) -> CharClass {
        self.initial_char_class
    }

    /// Classifies a byte. Non-ASCII bytes are reported as `NonWord` here;
    /// callers that handle UTF-8 should classify the decoded `char` instead.
    pub fn class_of_byte(&self, b: u8) -> CharClass {
        self.ascii[b as usize]
    }

    /// Classifies a Unicode scalar value.
    pub fn class_of_char(&self, c: char) -> CharClass {
        if (c as u32) < 128 {
            return self.ascii[c as usize];
        }
        if c.is_lowercase() {
            CharClass::Lower
        } else if c.is_uppercase() {
            CharClass::Upper
        } else if c.is_numeric() {
            CharClass::Number
        } else if c.is_alphabetic() {
            CharClass::Letter
        } else if c.is_whitespace() {
            CharClass::White
        } else if self.delimiters.contains(&(c as u8)) {
            CharClass::Delimiter
        } else {
            CharClass::NonWord
        }
    }

    /// The bonus for the character that opens the haystack.
    pub fn bonus_boundary_white(&self) -> i16 {
        self.bonus_boundary_white
    }

    /// The largest bonus this scheme pays for any class transition.
    pub fn max_bonus(&self) -> i16 {
        self.max_bonus
    }

    /// Bonus for transitioning from `prev_class` to `class`.
    pub fn bonus_for(&self, prev_class: CharClass, class: CharClass) -> i16 {
        self.bonus_matrix[prev_class as usize][class as usize]
    }
}

impl CharClass {
    fn from_index(i: usize) -> CharClass {
        match i {
            0 => CharClass::White,
            1 => CharClass::NonWord,
            2 => CharClass::Delimiter,
            3 => CharClass::Lower,
            4 => CharClass::Upper,
            5 => CharClass::Letter,
            _ => CharClass::Number,
        }
    }
}

/// Computes the bonus for a class transition, mirroring fzf's `bonusFor`.
fn bonus_for(prev_class: CharClass, class: CharClass, scheme: Scheme) -> i16 {
    if class as u8 >= CharClass::NonWord as u8 {
        match prev_class {
            CharClass::White => return scheme.bonus_boundary_white(),
            CharClass::Delimiter => return scheme.bonus_boundary_delimiter(),
            CharClass::NonWord => return BONUS_BOUNDARY,
            _ => {}
        }
    }

    if (prev_class == CharClass::Lower && class == CharClass::Upper)
        || (prev_class != CharClass::Number && class == CharClass::Number)
    {
        return BONUS_CAMEL123;
    }

    match class {
        CharClass::NonWord | CharClass::Delimiter => BONUS_NON_WORD,
        CharClass::White => scheme.bonus_boundary_white(),
        _ => 0,
    }
}
