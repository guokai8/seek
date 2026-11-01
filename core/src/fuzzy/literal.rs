//! Literal matching that folds case the way the fuzzy matcher does.
//!
//! The `^`, `$` and `'...'` operators are literal rather than fuzzy, but
//! they are still part of the same query, typed into the same field, and
//! they have to obey the same case rule. Reaching for `str::contains` and
//! friends instead makes them byte-exact: `^Docs` finds nothing in a tree
//! full of `docs/`, and `pdf$` walks past `Report.PDF`. Two case rules in
//! one search box is not a feature anyone asked for.
//!
//! Each predicate takes `case_sensitive` and hands the work to the
//! standard library when it is set, so the sensitive path costs nothing.

/// Whether `haystack` contains `needle`.
pub fn contains(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        return haystack.contains(needle);
    }
    if needle.is_empty() {
        return true;
    }

    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return false;
    }

    if haystack.is_ascii() && needle.is_ascii() {
        // The first byte in both cases is looked for with a vectorised
        // scan, so most starting positions are rejected without ever
        // being compared. A plain window-by-window walk costs the length
        // of the path for every candidate, on every keystroke.
        let lower = n[0].to_ascii_lowercase();
        let upper = n[0].to_ascii_uppercase();
        let mut from = 0usize;

        while let Some(found) = memchr::memchr2(lower, upper, &h[from..]) {
            let at = from + found;
            if at + n.len() > h.len() {
                return false;
            }
            if h[at..at + n.len()].eq_ignore_ascii_case(n) {
                return true;
            }
            from = at + 1;
        }

        return false;
    }

    let folded = fold(haystack);
    let wanted = fold(needle);

    folded.windows(wanted.len()).any(|w| w == wanted.as_slice())
}

/// Whether `haystack` holds `needle` at the start or right after a byte
/// `follows` accepts.
///
/// This is the boundary-aware form of [`contains`], and it exists for the
/// search's prune: see [`crate::fuzzy::FuzzyMatcher::ceiling_boundary`].
///
/// It answers conservatively. A haystack that is not plain ASCII reports
/// `true` without looking, because folding changes both the characters
/// and where the boundaries between them fall — and the caller uses a
/// `false` to dismiss a candidate, so a wrong `false` loses a row while a
/// wrong `true` only costs the work of scoring one.
pub fn contains_after(
    haystack: &str,
    needle: &str,
    case_sensitive: bool,
    follows: impl Fn(Option<u8>) -> bool,
) -> bool {
    if needle.is_empty() {
        return true;
    }
    if !haystack.is_ascii() || !needle.is_ascii() {
        return true;
    }

    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return false;
    }

    // The first byte is looked for with a vectorised scan, as in
    // `contains`. What is different is the byte before it: checking that
    // first rejects most starting positions without comparing the needle
    // at all, which is why this is no dearer than the plain substring
    // test it replaces even though it answers a narrower question.
    let (lower, upper) = if case_sensitive {
        (n[0], n[0])
    } else {
        (n[0].to_ascii_lowercase(), n[0].to_ascii_uppercase())
    };
    let mut from = 0usize;

    while let Some(found) = memchr::memchr2(lower, upper, &h[from..]) {
        let at = from + found;
        if at + n.len() > h.len() {
            return false;
        }
        let prev = if at == 0 { None } else { Some(h[at - 1]) };
        if follows(prev) {
            let window = &h[at..at + n.len()];
            let hit = if case_sensitive {
                window == n
            } else {
                window.eq_ignore_ascii_case(n)
            };
            if hit {
                return true;
            }
        }
        from = at + 1;
    }

    false
}

/// Whether `haystack` starts with `needle`.
pub fn starts_with(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        return haystack.starts_with(needle);
    }

    if haystack.is_ascii() && needle.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();

        return n.len() <= h.len() && h[..n.len()].eq_ignore_ascii_case(n);
    }

    fold(haystack).starts_with(&fold(needle))
}

/// Whether `haystack` ends with `needle`.
pub fn ends_with(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        return haystack.ends_with(needle);
    }

    if haystack.is_ascii() && needle.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();

        return n.len() <= h.len() && h[h.len() - n.len()..].eq_ignore_ascii_case(n);
    }

    fold(haystack).ends_with(&fold(needle))
}

/// Whether `haystack` is `needle`, under the same case rule.
pub fn equals(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        return haystack == needle;
    }

    if haystack.is_ascii() && needle.is_ascii() {
        return haystack.as_bytes().eq_ignore_ascii_case(needle.as_bytes());
    }

    fold(haystack) == fold(needle)
}

/// The characters of `text`, lowercased.
///
/// Only the non-ASCII path reaches this. Lowercasing a character can
/// produce more than one — `İ` becomes `i` plus a combining dot — so the
/// result is a sequence of characters rather than a string of the same
/// length, and the comparisons above are done over that sequence.
///
/// It allocates, which the ASCII paths above are careful not to. A path
/// with non-Latin characters in it is the uncommon case, and the
/// alternative is re-decoding the haystack from every starting position.
fn fold(text: &str) -> Vec<char> {
    text.chars().flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_is_folded_unless_it_is_asked_for() {
        assert!(starts_with("docs/notes.md", "Docs", false));
        assert!(!starts_with("docs/notes.md", "Docs", true));

        assert!(ends_with("Test_Report.PDF", "pdf", false));
        assert!(!ends_with("Test_Report.PDF", "pdf", true));

        assert!(contains("Test_Report.PDF", "report", false));
        assert!(!contains("Test_Report.PDF", "report", true));
    }

    #[test]
    fn the_sensitive_path_still_matches_what_it_should() {
        assert!(starts_with("docs/notes.md", "docs", true));
        assert!(ends_with("report.pdf", ".pdf", true));
        assert!(contains("docs/report.pdf", "s/re", true));
    }

    #[test]
    fn a_needle_longer_than_the_haystack_is_not_found() {
        assert!(!contains("ab", "abc", false));
        assert!(!starts_with("ab", "abc", false));
        assert!(!ends_with("ab", "abc", false));
    }

    #[test]
    fn the_empty_needle_is_everywhere() {
        assert!(contains("anything", "", false));
        assert!(starts_with("anything", "", false));
        assert!(ends_with("anything", "", false));
    }

    #[test]
    fn a_false_start_does_not_stop_the_scan() {
        // The first `r` is not the start of a match, so the scan has to
        // carry on past it rather than give up.
        assert!(contains("r_report.txt", "report", false));
        assert!(contains("RrRreport", "report", false));
    }

    /// The path scheme's boundary: the start, or just after a separator.
    fn after_separator(prev: Option<u8>) -> bool {
        match prev {
            None => true,
            Some(b) => b == b'/',
        }
    }

    #[test]
    fn a_run_counts_at_the_start_and_after_a_boundary() {
        assert!(contains_after("test/a.md", "test", false, after_separator));
        assert!(contains_after("src/test/a.md", "test", false, after_separator));
        assert!(contains_after("a/b/test.md", "test", false, after_separator));
    }

    #[test]
    fn a_run_elsewhere_does_not_count() {
        assert!(!contains_after("latest.md", "test", false, after_separator));
        assert!(!contains_after("src/x-test.md", "test", false, after_separator));
        assert!(!contains_after("src/t-e-s-t.md", "test", false, after_separator));
    }

    #[test]
    fn a_boundary_that_does_not_match_does_not_stop_the_scan() {
        // The `t` at offset 0 is at a boundary but is not the start of the
        // run. A scan that gave up on the first boundary candidate would
        // miss the real one after the separator.
        assert!(contains_after("t/test.md", "test", false, after_separator));
    }

    #[test]
    fn the_case_rule_is_the_same_one() {
        assert!(contains_after("src/TEST.md", "test", false, after_separator));
        assert!(!contains_after("src/TEST.md", "test", true, after_separator));
    }

    #[test]
    fn a_non_ascii_haystack_is_answered_conservatively() {
        // Folding changes the characters and moves the boundaries between
        // them, so this says "maybe" rather than risking a wrong "no".
        // The caller uses a `false` to dismiss a candidate.
        assert!(contains_after("süß/plan.md", "test", false, after_separator));
    }

    #[test]
    fn the_empty_needle_is_at_every_boundary() {
        assert!(contains_after("anything", "", false, after_separator));
    }

    #[test]
    fn non_ascii_is_folded_too() {
        assert!(contains("Übersicht/Plan.md", "übersicht", false));
        assert!(starts_with("Übersicht/Plan.md", "über", false));
        assert!(ends_with("notes/RÉSUMÉ", "résumé", false));
        assert!(!contains("Übersicht", "ubersicht", false));
    }

    #[test]
    fn a_non_ascii_haystack_still_finds_an_ascii_needle() {
        assert!(contains("文档/report.pdf", "REPORT", false));
        assert!(ends_with("文档/report.pdf", ".PDF", false));
    }
}
