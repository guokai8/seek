//! Validation of the fzf port against known fzf behaviour.
//!
//! These tests encode the ranking rules that fzf guarantees. They were
//! derived by observing the reference implementation's output ordering.

use super::{scan_direction, Direction, FuzzyMatcher, Scheme};

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

/// The reported start must be where the match actually begins.
///
/// It is tempting to report the first place the pattern's first character
/// could land, because the matrix already knows it and no backtrace is
/// needed. It is also wrong, and wrong in a way that only shows up in the
/// ranking: every span-based tiebreak is measured from the start.
#[test]
fn the_match_start_is_the_real_start() {
    let m = FuzzyMatcher::new("src", true, Scheme::Path);

    // "src" could begin at the `s` of "libparse" (index 6) and still reach
    // an `r` and a `c` further on. The match fzf settles on is the literal
    // "src" after the separator.
    let hit = m.match_text("libparse/src");
    assert!(hit.is_match());
    assert_eq!(
        hit.start, 9,
        "the start should be the `s` after the separator, not the one in \"libparse\""
    );
    assert_eq!(hit.end, 12);

    // The positions have to agree with the span they are reported beside.
    let (with_positions, positions) = m.match_text_with_positions("libparse/src");
    assert_eq!(positions, vec![9, 10, 11]);
    assert_eq!(with_positions.start, hit.start);
    assert_eq!(with_positions.end, hit.end);
}

/// The scan direction decides which of several equal scores is reported.
///
/// In "libparse/src/main.rs" the pattern `a` matches the `a` of "libparse"
/// and the `a` of "main.rs" for exactly the same score. Which one is
/// reported is the whole difference between a directory match and a file
/// name match, and so between ranking the entry last and ranking it first.
#[test]
fn the_scan_direction_picks_the_occurrence() {
    let path = "libparse/src/main.rs";

    let forward = FuzzyMatcher::new("a", true, Scheme::Path).scanning(Direction::Forward);
    let backward = FuzzyMatcher::new("a", true, Scheme::Path).scanning(Direction::Backward);

    let first = forward.match_text(path);
    let last = backward.match_text(path);

    assert_eq!(first.score, last.score, "the two occurrences score the same");
    assert_eq!(first.start, 4, "forward keeps the `a` of \"libparse\"");
    assert_eq!(last.start, 14, "backward keeps the `a` of \"main.rs\"");
}

/// The direction comes from the tiebreak chain, the way fzf derives it.
#[test]
fn the_chain_decides_the_direction() {
    use super::{Criterion, DEFAULT_CRITERIA, PATH_CRITERIA};

    // Nothing in the default chain cares where the match sits.
    assert_eq!(scan_direction(DEFAULT_CRITERIA), Direction::Forward);

    // `Pathname` wants the match as late as possible, so that a pattern
    // that could match a directory is still credited to the file name.
    assert_eq!(scan_direction(PATH_CRITERIA), Direction::Backward);

    // fzf resolves the chain from the back towards the front, so the
    // earliest criterion has the final say.
    assert_eq!(
        scan_direction(&[Criterion::Score, Criterion::Begin, Criterion::Pathname]),
        Direction::Forward
    );
    assert_eq!(
        scan_direction(&[Criterion::Score, Criterion::Pathname, Criterion::Begin]),
        Direction::Backward
    );

    // The score is always first and has no preference of its own.
    assert_eq!(scan_direction(&[Criterion::Score]), Direction::Forward);
    assert_eq!(scan_direction(&[]), Direction::Forward);
}

/// `Pathname` mixes a byte offset with a character index, the way fzf does.
///
/// fzf's `byPathname` finds the last separator by indexing the Go string,
/// which is a *byte* index, and compares it against `minBegin`, which is a
/// *rune* index:
///
/// ```go
/// lastDelim := -1
/// s := item.text.ToString()
/// for i := len(s) - 1; i >= 0; i-- {
///     if s[i] == '/' || s[i] == '\\' { lastDelim = i; break }
/// }
/// if lastDelim <= minBegin { val = util.AsUint16(minBegin - lastDelim) }
/// ```
///
/// On an ASCII path the two indices are the same number and nothing shows.
/// On a path with a multi-byte character they are not, and the difference
/// is visible in the ranking:
///
/// * `a/я/hellp.txt` — `я` is two bytes, so the separator sits at byte 4
///   while the match starts at character 4. The distance is 0, which beats
///   every ASCII path, where it is 1.
/// * `a/背/hellp.txt` — `背` is three bytes, so the separator is at byte 5
///   and the match starts at character 4. `lastDelim > minBegin`, so fzf
///   reads it as a match that spans a separator and gives it the worst
///   value there is.
///
/// Computing both indices in characters is the *reasonable* thing to do and
/// is what this port used to do. It also put `a/背/hellp.txt` first where
/// fzf puts it third, which is a ranking that disagrees with fzf — and a
/// ranking that disagrees with fzf is a bug, whichever implementation is
/// easier to defend. The expected order below is what
/// `fzf --filter=hellp --scheme=path -i` prints for these four lines.
#[test]
fn the_pathname_tiebreak_indexes_the_separator_in_bytes() {
    use super::{rank_candidates, Scheme, PATH_CRITERIA};

    let candidates = [
        "a/bc/hellp.txt",
        "a/背/hellp.txt",
        "a/я/hellp.txt",
        "a/bc/背hellp.txt",
    ];

    let matcher = FuzzyMatcher::new("hellp", true, Scheme::Path);
    let ranked = rank_candidates(matcher, &candidates, PATH_CRITERIA);
    let order: Vec<&str> = ranked.iter().map(|r| candidates[r.index]).collect();

    assert_eq!(
        order,
        vec![
            "a/я/hellp.txt",
            "a/bc/hellp.txt",
            "a/背/hellp.txt",
            "a/bc/背hellp.txt",
        ],
        "the path scheme ranks multi-byte paths differently from fzf"
    );
}
