//! Exact substring matching, scored the way fzf scores it.
//!
//! Port of fzf's `exactMatchNaive` (src/algo/algo.go), without the
//! `boundaryCheck` variant, which only `--nth`-style boundary terms use.
//!
//! An exact term could be answered with [`crate::fuzzy::literal::contains`]
//! and left at that — true or false, in the running or not. That is what
//! this crate did, and it is wrong in the way that matters: fzf *scores* an
//! exact match. It picks the occurrence whose first character sits on the
//! best boundary and then runs the same scoring function the fuzzy matcher
//! ends with, so `src` ranks `foo/src/a.rs` above `unsrcoped.txt` for the
//! same reason the fuzzy matcher does. A term that only filters leaves
//! every candidate on score zero, the ranking falls through to the
//! tiebreaks, and the order is one fzf does not stand behind.
//!
//! It is also what the panel highlights. The fuzzy matcher asked about an
//! exact term reports its own alignment — the five letters of `nihao`
//! scattered down the path — and the row then underlines characters the
//! term did not ask for.

use super::matcher::Direction;
use super::normalize::normalize_char;
use super::score::{
    CharClasses, MatchResult, Position, Scheme, BONUS_BOUNDARY, BONUS_CONSECUTIVE,
    BONUS_FIRST_CHAR_MULTIPLIER, SCORE_MATCH,
};

/// Where in the haystack an exact term is allowed to sit.
///
/// fzf has a matcher for each of these — `exactMatchNaive`,
/// `PrefixMatch`, `SuffixMatch` and `EqualMatch` — and the last three are
/// the same literal comparison at a position arithmetic decides, followed
/// by the same scoring. So they are this matcher with the search for the
/// occurrence replaced by that arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    /// Anywhere in the haystack: a bare substring.
    #[default]
    Anywhere,
    /// At the start of it: `^term`.
    Start,
    /// At the end of it: `term$`.
    End,
    /// The whole of it and nothing else: `^term$`.
    Whole,
}

/// A compiled exact term, ready to be matched against many haystacks.
pub struct ExactMatcher {
    /// Pattern characters, folded the way the haystack will be.
    pattern: Vec<char>,
    /// The pattern as written, kept for the ASCII fast path.
    raw: String,
    /// Whether matching is case sensitive.
    case_sensitive: bool,
    /// Whether accented Latin letters are folded to their base letter.
    normalize: bool,
    /// Character classification tables.
    classes: CharClasses,
    /// Which equally bonused occurrence is reported as the match.
    direction: Direction,
    /// Where in the haystack the term is allowed to sit.
    anchor: Anchor,
}

impl ExactMatcher {
    /// Compiles a term into a matcher.
    ///
    /// The arguments are the fuzzy matcher's, in the same order, because
    /// the two are built side by side from the same options.
    pub fn new(pattern: &str, case_sensitive: bool, scheme: Scheme, normalize: bool) -> Self {
        let folded: Vec<char> = pattern
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
            pattern: folded,
            raw: pattern.to_string(),
            case_sensitive,
            normalize,
            classes: CharClasses::new(scheme),
            direction: Direction::Forward,
            anchor: Anchor::Anywhere,
        }
    }

    /// Sets which occurrence is reported when several open on an equally
    /// good boundary.
    ///
    /// fzf hands the same flag to its exact matcher as to its fuzzy one,
    /// and for the same reason: the reported span is what the `Pathname`
    /// tiebreak is measured from, and that criterion wants the match as
    /// late in the path as it can have it, so that `s` in
    /// `indexer/src/search.rs` is credited to the file name rather than
    /// to the directory. See [`crate::fuzzy::scan_direction`].
    pub fn scanning(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// Anchors the term to one end of the haystack, or to both.
    pub fn anchored(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// Whether the term is empty, which matches everything at score zero.
    pub fn is_empty(&self) -> bool {
        self.pattern.is_empty()
    }

    /// Matches the term against `haystack`.
    pub fn match_text(&self, haystack: &str) -> MatchResult {
        let Some((start, end)) = self.best_span(haystack) else {
            return MatchResult::NO_MATCH;
        };

        MatchResult {
            start: start as Position,
            end: end as Position,
            score: match self.anchor {
                // fzf's `EqualMatch` does not walk the span: the whole
                // line is the match, so every character is taken to open
                // on a whitespace boundary and the sum is closed form.
                Anchor::Whole => {
                    let white = self.classes.bonus_boundary_white() as i32;
                    let len = self.pattern.len() as i32;

                    (SCORE_MATCH as i32 + white) * len
                        + (BONUS_FIRST_CHAR_MULTIPLIER as i32 - 1) * white
                }
                _ => self.score_span(haystack, start, end),
            },
        }
    }

    /// Matches the term and reports the byte offset of every matched
    /// character, for the panel to underline.
    pub fn match_text_with_positions(&self, haystack: &str) -> (MatchResult, Vec<Position>) {
        let result = self.match_text(haystack);
        if !result.is_match() {
            return (result, Vec::new());
        }

        let positions = haystack[result.start as usize..result.end as usize]
            .char_indices()
            .map(|(offset, _)| result.start + offset as Position)
            .collect();

        (result, positions)
    }

    /// The occurrence fzf would report: the first one, in scanning
    /// order, whose opening character has the best boundary bonus.
    ///
    /// fzf walks the haystack once, restarting a character later whenever
    /// a partial match breaks, and stops at the first occurrence that
    /// opens on a boundary — nothing further on can beat it, because the
    /// boundary bonus is the largest one on offer. The walk here is the
    /// same one read from the other end: every position the term could
    /// open at, in scanning order, which for an ASCII haystack is found
    /// with a vectorised scan for the term's first byte rather than a
    /// character at a time.
    ///
    /// Scanning backward is the same walk over the occurrences in reverse.
    /// fzf reverses the haystack and the term instead, which finds the
    /// occurrence nearest the end first; the occurrences of a term all
    /// have its length, so taking them by descending start is the same
    /// order.
    fn best_span(&self, haystack: &str) -> Option<(usize, usize)> {
        if self.pattern.is_empty() {
            // Only reachable defensively: the parser drops an anchored
            // term with nothing to anchor, the way fzf drops it.
            return (self.anchor != Anchor::Whole).then_some((0, 0));
        }

        match self.anchor {
            Anchor::Anywhere => self.best_occurrence(haystack),
            Anchor::Start => {
                let start = self.leading_trim(haystack);
                self.matches_at(haystack, start).map(|end| (start, end))
            }
            Anchor::End => {
                let end = self.trailing_trim(haystack);
                let start = self.opening_of(haystack, end)?;
                self.matches_at(haystack, start).map(|got| (start, got))
            }
            Anchor::Whole => {
                let start = self.leading_trim(haystack);
                let end = self.trailing_trim(haystack);
                let got = self.matches_at(haystack, start)?;

                (got == end).then_some((start, end))
            }
        }
    }

    /// Where the trimmed haystack starts, the way fzf trims it.
    ///
    /// A term that opens with a space is asking for the space, so the
    /// trimming is skipped — which is fzf's rule and the reason it is
    /// written as a guard rather than an unconditional `trim_start`.
    fn leading_trim(&self, haystack: &str) -> usize {
        match self.pattern.first() {
            Some(c) if c.is_whitespace() => 0,
            _ => haystack.len() - haystack.trim_start().len(),
        }
    }

    /// Where the trimmed haystack ends.
    fn trailing_trim(&self, haystack: &str) -> usize {
        match self.pattern.last() {
            Some(c) if c.is_whitespace() => haystack.len(),
            _ => haystack.trim_end().len(),
        }
    }

    /// The byte offset the term would have to open at to end at `end`.
    fn opening_of(&self, haystack: &str, end: usize) -> Option<usize> {
        haystack
            .get(..end)?
            .char_indices()
            .rev()
            .nth(self.pattern.len() - 1)
            .map(|(offset, _)| offset)
    }

    /// The occurrence an unanchored term reports.
    fn best_occurrence(&self, haystack: &str) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize)> = None;
        let mut best_bonus = i16::MIN;

        for start in self.starts(haystack) {
            let Some(end) = self.matches_at(haystack, start) else {
                continue;
            };

            let bonus = self.opening_bonus(haystack, start);
            if bonus > best_bonus {
                best = Some((start, end));
                best_bonus = bonus;
            }
            // A boundary is the best an opening character can do, so no
            // later occurrence can displace this one.
            if bonus >= BONUS_BOUNDARY {
                break;
            }
        }

        best
    }

    /// Every byte offset the term could open at, in scanning order.
    fn starts<'a>(&'a self, haystack: &'a str) -> Box<dyn Iterator<Item = usize> + 'a> {
        // The fast path needs both sides to be plain ASCII. A haystack
        // that is not cannot be scanned for a byte — and with
        // normalization on, an ASCII term legitimately matches a haystack
        // that is not: `e` is `é` folded.
        if !haystack.is_ascii() || !self.raw.is_ascii() {
            let offsets = haystack.char_indices().map(|(offset, _)| offset);

            return match self.direction {
                Direction::Forward => Box::new(offsets),
                Direction::Backward => Box::new(offsets.collect::<Vec<_>>().into_iter().rev()),
            };
        }

        let first = self.pattern[0] as u8;
        let (lower, upper) = if self.case_sensitive {
            (first, first)
        } else {
            (first.to_ascii_lowercase(), first.to_ascii_uppercase())
        };
        let bytes = haystack.as_bytes();

        match self.direction {
            Direction::Forward => {
                let mut from = 0usize;

                Box::new(std::iter::from_fn(move || {
                    let found = memchr::memchr2(lower, upper, &bytes[from..])?;
                    let at = from + found;
                    from = at + 1;
                    Some(at)
                }))
            }
            Direction::Backward => {
                let mut until = bytes.len();

                Box::new(std::iter::from_fn(move || {
                    let at = memchr::memrchr2(lower, upper, &bytes[..until])?;
                    until = at;
                    Some(at)
                }))
            }
        }
    }

    /// Whether the whole term sits in `haystack` from `start`, and where
    /// it ends if it does.
    fn matches_at(&self, haystack: &str, start: usize) -> Option<usize> {
        let mut chars = haystack[start..].chars();
        let mut end = start;

        for &wanted in &self.pattern {
            let c = chars.next()?;
            if self.fold(c) != wanted {
                return None;
            }
            end += c.len_utf8();
        }

        Some(end)
    }

    /// The bonus fzf's `bonusAt` reports for the character at `at`.
    ///
    /// This decides which occurrence is reported, not what it scores. The
    /// start of the haystack is a whitespace boundary whatever the
    /// character there turns out to be, which is fzf's rule and not the
    /// one [`Self::score_span`] uses below — see the comment there.
    fn opening_bonus(&self, haystack: &str, at: usize) -> i16 {
        if at == 0 {
            return self.classes.bonus_boundary_white();
        }

        let Some(prev) = haystack[..at].chars().next_back() else {
            return self.classes.bonus_boundary_white();
        };
        let Some(c) = haystack[at..].chars().next() else {
            return 0;
        };

        self.classes
            .bonus_for(self.classes.class_of_char(prev), self.classes.class_of_char(c))
    }

    /// fzf's `calculateScore` over a span that matches character for
    /// character.
    ///
    /// The general function walks the span looking for the pattern and
    /// pays a gap penalty where it does not find it. Here every character
    /// matches, in order, with nothing between them, so the gaps and the
    /// `pidx` bookkeeping fall away and what is left is the match score,
    /// the opening bonus counted twice, and the consecutive bonus for the
    /// rest.
    ///
    /// The class of the character before the span is the scheme's own —
    /// a delimiter for the path scheme — where the occurrence above takes
    /// the start of the haystack to be whitespace. fzf does both, in those
    /// two places, and the two answers differ for the path scheme.
    fn score_span(&self, haystack: &str, start: usize, end: usize) -> i32 {
        let mut prev_class = match haystack[..start].chars().next_back() {
            Some(prev) => self.classes.class_of_char(prev),
            None => self.classes.initial_char_class(),
        };

        let mut score = 0i32;
        let mut first_bonus = 0i16;

        for (index, c) in haystack[start..end].chars().enumerate() {
            let class = self.classes.class_of_char(c);
            score += SCORE_MATCH as i32;

            let mut bonus = self.classes.bonus_for(prev_class, class);
            if index == 0 {
                first_bonus = bonus;
                score += (bonus * BONUS_FIRST_CHAR_MULTIPLIER) as i32;
            } else {
                // A boundary inside the run — `src` in `my_src` — is
                // taken as the run's bonus from there on, which is how
                // fzf lets a later boundary rescue a run that opened in
                // the middle of a word.
                if bonus >= BONUS_BOUNDARY && bonus > first_bonus {
                    first_bonus = bonus;
                }
                bonus = bonus.max(first_bonus).max(BONUS_CONSECUTIVE);
                score += bonus as i32;
            }

            prev_class = class;
        }

        score
    }

    /// Folds a haystack character for comparison, the way the fuzzy
    /// matcher folds it.
    #[inline]
    fn fold(&self, c: char) -> char {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The score of `pattern` against `haystack` under the path scheme.
    fn score(pattern: &str, haystack: &str) -> i32 {
        ExactMatcher::new(pattern, false, Scheme::Path, true)
            .match_text(haystack)
            .score
    }

    /// The score of `pattern` anchored at `anchor`, under the path scheme.
    fn anchored_score(pattern: &str, anchor: Anchor, haystack: &str) -> i32 {
        ExactMatcher::new(pattern, false, Scheme::Path, true)
            .anchored(anchor)
            .match_text(haystack)
            .score
    }

    #[test]
    fn a_prefix_term_matches_only_at_the_start() {
        let m = ExactMatcher::new("src", false, Scheme::Path, true).anchored(Anchor::Start);

        assert!(m.match_text("src/main.rs").is_match());
        assert!(!m.match_text("app/src/main.rs").is_match());
    }

    #[test]
    fn a_suffix_term_matches_only_at_the_end() {
        let m = ExactMatcher::new(".rs", false, Scheme::Path, true).anchored(Anchor::End);
        let hit = m.match_text("src/main.rs");

        assert!(hit.is_match());
        assert_eq!((hit.start, hit.end), (8, 11));
        assert!(!m.match_text("src/main.rs.bak").is_match());
    }

    #[test]
    fn an_equal_term_wants_the_whole_path() {
        let m = ExactMatcher::new("confetti", false, Scheme::Path, true).anchored(Anchor::Whole);

        assert!(m.match_text("confetti").is_match());
        assert!(!m.match_text("confetti/src").is_match());
        assert!(!m.match_text("x/confetti").is_match());
    }

    #[test]
    fn an_anchored_term_is_scored_and_not_merely_filtered() {
        // This is the whole point of the change: fzf's `PrefixMatch` and
        // `SuffixMatch` both end in the same `calculateScore` an exact
        // term ends in, so the candidates are ranked rather than left on
        // score zero for the tiebreaks to sort out.
        assert!(anchored_score("src", Anchor::Start, "src/main.rs") > 0);
        assert!(anchored_score(".rs", Anchor::End, "src/main.rs") > 0);
        assert!(anchored_score("confetti", Anchor::Whole, "confetti") > 0);

        // A suffix opening on a delimiter beats one opening inside a word,
        // for the same reason it does anywhere else in the path.
        assert!(
            anchored_score("main.rs", Anchor::End, "src/main.rs")
                > anchored_score("ain.rs", Anchor::End, "src/main.rs")
        );
    }

    #[test]
    fn an_anchor_measures_from_the_trimmed_text() {
        // fzf strips the leading and trailing whitespace of the line
        // before anchoring, unless the term itself asks for it.
        let m = ExactMatcher::new("a", false, Scheme::Path, true).anchored(Anchor::Start);
        assert!(m.match_text("  a/b").is_match());

        let end = ExactMatcher::new("b", false, Scheme::Path, true).anchored(Anchor::End);
        assert!(end.match_text("a/b  ").is_match());
    }

    #[test]
    fn a_term_that_is_not_there_does_not_match() {
        let m = ExactMatcher::new("nihao", false, Scheme::Path, true);

        // The five letters are all in this path, in order, which is
        // exactly what an exact term refuses.
        assert!(!m
            .match_text("Hao Law NIW EB1 Documents List.pdf")
            .is_match());
        assert!(m.match_text("notes/nihao.txt").is_match());
    }

    #[test]
    fn the_span_is_the_run_itself() {
        let m = ExactMatcher::new("src", false, Scheme::Path, true);
        let hit = m.match_text("app/src/main.rs");

        assert_eq!((hit.start, hit.end), (4, 7));
    }

    #[test]
    fn a_run_on_a_boundary_scores_above_one_inside_a_word() {
        assert!(score("src", "app/src/main.rs") > score("src", "unsrcoped.txt"));
    }

    #[test]
    fn scanning_backward_reports_the_last_of_several_boundaries() {
        // The `Pathname` tiebreak is measured from the reported span, and
        // it wants the match credited to the file name: both occurrences
        // open on a delimiter, and the one in the name is the one fzf
        // reports under `--scheme=path`.
        let m = ExactMatcher::new("s", false, Scheme::Path, true)
            .scanning(Direction::Backward);
        let hit = m.match_text("indexer/src/search.rs");

        assert_eq!(hit.start, 12);

        let forward = ExactMatcher::new("s", false, Scheme::Path, true);
        assert_eq!(forward.match_text("indexer/src/search.rs").start, 8);
    }

    #[test]
    fn the_occurrence_on_a_boundary_is_the_one_reported() {
        // Two occurrences: one inside `unsrc`, one after the delimiter.
        // fzf reports the one that opens on the boundary even though it
        // comes later.
        let m = ExactMatcher::new("src", false, Scheme::Path, true);
        let hit = m.match_text("unsrc/src.rs");

        assert_eq!((hit.start, hit.end), (6, 9));
    }

    #[test]
    fn case_is_folded_unless_it_is_asked_for() {
        let folding = ExactMatcher::new("readme", false, Scheme::Path, true);
        let strict = ExactMatcher::new("readme", true, Scheme::Path, true);

        assert!(folding.match_text("docs/README.md").is_match());
        assert!(!strict.match_text("docs/README.md").is_match());
    }

    #[test]
    fn an_accent_is_folded_the_way_the_fuzzy_matcher_folds_it() {
        let m = ExactMatcher::new("resume", false, Scheme::Path, true);

        assert!(m.match_text("docs/résumé.pdf").is_match());
    }

    #[test]
    fn the_positions_are_the_characters_of_the_run() {
        let m = ExactMatcher::new("src", false, Scheme::Path, true);
        let (_, positions) = m.match_text_with_positions("app/src/main.rs");

        assert_eq!(positions, vec![4, 5, 6]);
    }

    #[test]
    fn the_positions_of_a_multibyte_run_are_byte_offsets() {
        let m = ExactMatcher::new("背景", false, Scheme::Path, true);
        let (hit, positions) = m.match_text_with_positions("a/背景.txt");

        assert_eq!((hit.start, hit.end), (2, 8));
        assert_eq!(positions, vec![2, 5]);
    }
}
