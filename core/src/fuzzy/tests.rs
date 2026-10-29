//! Validation of the fzf port against known fzf behaviour.
//!
//! These tests encode the ranking rules that fzf guarantees. They were
//! derived by observing the reference implementation's output ordering.

use super::{FuzzyMatcher, Scheme};

fn scores<'a>(pattern: &str, haystacks: &[&'a str]) -> Vec<(i32, &'a str)> {
    let m = FuzzyMatcher::new(pattern, true, Scheme::Default);
    let mut out: Vec<(i32, &str)> = haystacks
        .iter()
        .map(|h| (m.match_text(h).score, *h))
        .collect();
    // Stable sort by descending score, matching fzf's ordering.
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

#[test]
fn exact_match_outranks_partial() {
    let s = scores("abc", &["abc", "axbxc", "a_b_c"]);
    assert_eq!(s[0].1, "abc", "exact match must rank first");
}

#[test]
fn consecutive_beats_gapped() {
    // fzf prefers consecutive characters because gaps carry a penalty.
    let s = scores("abc", &["axbxc", "abc"]);
    assert_eq!(s[0].1, "abc");
}

#[test]
fn word_boundary_bonus_applies() {
    // Matching at the start of a word is worth more than mid-word.
    let s = scores("ab", &["xaby", "abx"]);
    assert_eq!(s[0].1, "abx", "boundary match must rank first");
}

#[test]
fn camel_case_hump_is_rewarded() {
    // Ignoring case, "b" matches both candidates, but the camelCase hump in
    // "FooBar" earns a bonus that "Foobar" does not, so it ranks first.
    // This matches fzf's ordering: `fzf --filter b -i` puts FooBar first.
    let m = FuzzyMatcher::new("b", false, Scheme::Default);
    let camel = m.match_text("FooBar").score;
    let plain = m.match_text("Foobar").score;
    assert!(
        camel > plain,
        "camelCase hump should score higher ({camel} vs {plain})"
    );
}

#[test]
fn camel_case_pair_matches_where_plain_cannot() {
    // "FB" matches "FooBar" as a subsequence; "Fb" does not match "Foobar"
    // because there is no capital B there.
    let m = FuzzyMatcher::new("FB", true, Scheme::Default);
    assert!(m.match_text("FooBar").is_match());
    assert!(!m.match_text("Foobar").is_match());
}

#[test]
fn non_match_returns_no_match() {
    let m = FuzzyMatcher::new("xyz", true, Scheme::Default);
    let r = m.match_text("abc");
    assert!(!r.is_match());
    assert_eq!(r.start, -1);
}

#[test]
fn empty_pattern_matches_everything() {
    let m = FuzzyMatcher::new("", true, Scheme::Default);
    assert!(m.is_empty());
    let r = m.match_text("anything");
    assert_eq!(r.score, 0);
}

#[test]
fn case_insensitive_matching() {
    let m = FuzzyMatcher::new("abc", false, Scheme::Default);
    assert!(m.match_text("ABC").is_match());
    assert!(m.match_text("aBc").is_match());
}

#[test]
fn case_sensitive_rejects_wrong_case() {
    let m = FuzzyMatcher::new("abc", true, Scheme::Default);
    assert!(!m.match_text("ABC").is_match());
}

#[test]
fn positions_are_reported_in_order() {
    let m = FuzzyMatcher::new("abc", true, Scheme::Default);
    let (_, pos) = m.match_text_with_positions("xaybzc");
    assert_eq!(pos.len(), 3);
    assert!(pos[0] < pos[1] && pos[1] < pos[2]);
    assert_eq!(&"xaybzc"[pos[0] as usize..=pos[0] as usize], "a");
    assert_eq!(&"xaybzc"[pos[1] as usize..=pos[1] as usize], "b");
    assert_eq!(&"xaybzc"[pos[2] as usize..=pos[2] as usize], "c");
}

#[test]
fn path_scheme_prefers_segment_starts() {
    // With the path scheme, "/" is a delimiter so matching right after a
    // slash earns a boundary bonus.
    let m = FuzzyMatcher::new("pd", true, Scheme::Path);
    let after_slash = m.match_text("/foo/pdbar").score;
    let mid_word = m.match_text("/foo/apdbar").score;
    assert!(
        after_slash > mid_word,
        "path scheme should reward segment starts ({after_slash} vs {mid_word})"
    );
}

#[test]
fn longer_haystack_with_early_match_ranks_well() {
    // fzf does not penalise length by itself; it penalises *gaps*.
    let s = scores("ab", &["ab", "axxxxb"]);
    assert_eq!(s[0].1, "ab");
}

#[test]
fn multi_byte_input_is_handled() {
    let m = FuzzyMatcher::new("é", true, Scheme::Default);
    assert!(m.match_text("café").is_match());
}

#[test]
fn accent_normalization_folds_to_base_letter() {
    // fzf folds accents by default, so "cafe" matches "cafe" with an acute
    // accent on the final e.
    let m = FuzzyMatcher::new("cafe", false, Scheme::Default);
    assert!(
        m.match_text("café").is_match(),
        "accented input should match its base form"
    );
}

#[test]
fn literal_mode_disables_accent_normalization() {
    // Mirrors `fzf --literal`: "cafe" must not match "cafe" with an accent.
    let m = FuzzyMatcher::with_options("cafe", false, Scheme::Default, false);
    assert!(
        !m.match_text("café").is_match(),
        "literal mode should not fold accents"
    );
}
