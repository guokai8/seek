//! Query parsing.
//!
//! The language takes the useful parts of fzf's extended search syntax and
//! the two things `fd` can do that fzf cannot — globs and regular
//! expressions:
//!
//! * `foo` — fuzzy match
//! * `*.pdf` — glob; any term holding `*` or `?`
//! * `re:^src/.*\.rs$` — regular expression
//! * `'foo bar'` — exact substring, and the way to put a space in a term
//! * `^foo` — prefix match
//! * `foo$` — suffix match
//! * `!foo` — inverse match (exclude the entries that match `foo`)
//!
//! Anything else is a fuzzy term. Every positive term must match (AND) and
//! no inverse term may (NOT).
//!
//! # Why glob needs no operator and regex does
//!
//! `fd` spells its glob mode `-g` because its default is a regex. Here the
//! default is a fuzzy match, and `*` and `?` are characters that do not
//! occur in real paths, so a term that holds one can only have been meant
//! as a glob. Typing `*.pdf` and getting nothing — which is what happened
//! before — is the behaviour nobody wants.
//!
//! A regex is the other way round. Its syntax is made of characters that
//! are ordinary in a filename: `.` alone would turn every query into one.
//! So it is asked for by name. `re:` is spelled as a prefix operator
//! because the grammar already had that shape.
//!
//! # What matches against what
//!
//! Everything is matched against the path relative to its root — the same
//! text the fuzzy matcher scores, and the same text the panel shows. A
//! glob with no `/` in it is given an implicit `**/` so that `*.pdf` means
//! what everybody means by it; a glob that does have a `/` is matched as
//! written.

use crate::fuzzy::{Criterion, Scheme};
use crate::index::EntryId;

/// Errors that can occur while parsing a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// An operator was used without a value.
    MissingValue(String),
    /// A glob could not be compiled.
    InvalidGlob { pattern: String, reason: String },
    /// A regular expression could not be compiled.
    InvalidRegex { pattern: String, reason: String },
    /// An operator that the engine does not implement.
    Unsupported(String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::MissingValue(op) => write!(f, "operator {op:?} needs a value"),
            QueryError::InvalidGlob { pattern, reason } => {
                // globset quotes the pattern it was handed, which is the
                // one with the implicit `**/` on the front. Showing that
                // back to someone who typed `*.pdf` would be answering a
                // question they did not ask, so only the explanation that
                // follows it is kept.
                let reason = reason.rsplit("': ").next().unwrap_or(reason);

                write!(f, "{pattern} is not a valid glob: {reason}")
            }
            QueryError::InvalidRegex { pattern, reason } => {
                // The regex crate's message runs to several lines: a
                // header, the pattern, a caret under the offending
                // character, and then what is actually wrong. That shape
                // is for a terminal; the panel has one line, and the last
                // line is the one that says something.
                let reason = reason
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or(reason)
                    .trim()
                    .trim_start_matches("error: ");

                write!(f, "{pattern} is not a valid regular expression: {reason}")
            }
            QueryError::Unsupported(op) => write!(f, "{op} is not supported yet"),
        }
    }
}

/// How a single term matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermKind {
    /// Fuzzy subsequence match.
    Fuzzy,
    /// Exact substring match.
    Exact,
    /// Prefix match.
    Prefix,
    /// Suffix match.
    Suffix,
    /// The whole path, and nothing else: fzf's `^a$`.
    Equal,
    /// Shell-style glob match.
    Glob,
    /// Regular expression match.
    Regex,
}

/// What a bare term means.
///
/// This is fzf's `--exact`, and it is a mode rather than an operator
/// because it is a decision about the whole query: either a run of
/// characters is a subsequence of the path or it is a substring of it.
///
/// The default here is [`Matching::Exact`], where fzf's is fuzzy. fzf is
/// a filter the user pipes a list into, and a loose first pass they then
/// narrow is exactly what they want; this is a launcher, where the top
/// row is usually the answer and a query of five letters matching a
/// hundred and seventy thousand paths is noise. The mode is a setting and
/// a keystroke away, and `'` reaches the other one for a single term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Matching {
    /// A bare term is a subsequence; `'term` is a substring.
    Fuzzy,
    /// A bare term is a substring; `'term` is a subsequence.
    #[default]
    Exact,
}

impl Matching {
    /// The kind a bare term gets.
    fn bare(self) -> TermKind {
        match self {
            Matching::Fuzzy => TermKind::Fuzzy,
            Matching::Exact => TermKind::Exact,
        }
    }

    /// The kind `'term` gets: the other one.
    fn quoted(self) -> TermKind {
        match self {
            Matching::Fuzzy => TermKind::Exact,
            Matching::Exact => TermKind::Fuzzy,
        }
    }

    /// The kind a bare term gets, given whether it is inverted.
    ///
    /// fzf makes an inverted term exact before it reads anything else,
    /// and the mode never gets a say: `!ab` keeps `axb` in either mode.
    /// An exclusion is a blunt instrument, and a fuzzy one excludes far
    /// more than the user who typed five letters had in mind.
    fn bare_when(self, inverted: bool) -> TermKind {
        if inverted {
            TermKind::Exact
        } else {
            self.bare()
        }
    }

    /// The kind `'term` gets, given whether it is inverted.
    ///
    /// The quote asks for the other matching, and for an inverted term
    /// there is only one other left: `!` has already made it exact.
    fn quoted_when(self, inverted: bool) -> TermKind {
        if inverted {
            TermKind::Fuzzy
        } else {
            self.quoted()
        }
    }
}

/// A single term in a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// How the term matches.
    pub kind: TermKind,
    /// The text to match.
    pub value: String,
}

/// A parsed query.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// The terms that must all match.
    pub positive: Vec<Term>,
    /// The terms that must not match.
    pub negative: Vec<Term>,
    /// The raw text the query was parsed from.
    pub raw: String,
}

impl Query {
    /// Parses a query string, reading bare terms the way `matching` says.
    pub fn parse(input: &str, matching: Matching) -> Result<Query, QueryError> {
        let mut positive = Vec::new();
        let mut negative = Vec::new();
        let mut raw = String::new();

        let chars: Vec<char> = input.chars().collect();
        let mut i = 0usize;

        while i < chars.len() {
            // Skip whitespace between terms.
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }

            let mut inverted = false;
            if chars[i] == '!' {
                inverted = true;
                i += 1;
                if i >= chars.len() {
                    return Err(QueryError::MissingValue("!".into()));
                }
            }

            // `content:` parses, so that it can be rejected. Letting it
            // through produced a term no filter recognised, which was then
            // dropped — and a query of nothing but dropped terms matches
            // the whole index. A search that answers a question it does
            // not understand with "everything" is worse than one that says
            // it does not understand.
            if starts_with_operator(&chars[i..], "content:") {
                return Err(QueryError::Unsupported("content:".into()));
            }

            let is_regex = starts_with_operator(&chars[i..], "re:");
            if is_regex {
                i += 3;
            }

            // The term value: either a quoted string or a bare run of
            // characters terminated by whitespace.
            let (value, quoting) = read_value(&chars, &mut i)?;

            if is_regex {
                if value.is_empty() {
                    return Err(QueryError::MissingValue("re:".into()));
                }

                // Compiling here is what turns a typo into a message the
                // panel can show, instead of a search that silently finds
                // nothing. The compiled form is not kept: `Term` stays a
                // plain value, and the search compiles it once per query,
                // which costs microseconds against a search that costs
                // milliseconds.
                regex::Regex::new(&value).map_err(|e| QueryError::InvalidRegex {
                    pattern: value.clone(),
                    reason: e.to_string(),
                })?;

                raw.push_str(&value);
                raw.push(' ');

                push(&mut positive, &mut negative, inverted, TermKind::Regex, value);
                continue;
            }

            // A quoted value is literal all the way through: the quotes
            // are how a term says "these characters", so `^` and `$`
            // inside them are characters and not operators.
            let (value, kind) = match quoting {
                Quoting::Paired => (value, TermKind::Exact),
                Quoting::Open => (value, matching.quoted_when(inverted)),
                Quoting::None => classify(value, matching, inverted)?,
            };

            if value.is_empty() {
                if inverted {
                    return Err(QueryError::MissingValue("!".into()));
                }
                continue;
            }

            raw.push_str(&value);
            raw.push(' ');

            push(&mut positive, &mut negative, inverted, kind, value);
        }

        Ok(Query {
            positive,
            negative,
            raw,
        })
    }

    /// Whether the query has no terms and should match everything.
    pub fn is_empty(&self) -> bool {
        self.positive.is_empty() && self.negative.is_empty()
    }
}

/// Whether `chars` opens with `operator`.
fn starts_with_operator(chars: &[char], operator: &str) -> bool {
    let operator: Vec<char> = operator.chars().collect();

    chars.len() >= operator.len() && chars[..operator.len()] == operator[..]
}

/// How a term's value was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quoting {
    /// No quote: a bare run of characters.
    None,
    /// fzf's form — a leading `'` and no closing one, running to the
    /// next space.
    Open,
    /// Ours — `'…'`, which may hold spaces.
    Paired,
}

/// Reads one term's value, and says how it was quoted.
///
/// A paired-quote value runs to its closing quote and may hold spaces; an
/// open quote and a bare value both run to the next space.
fn read_value(chars: &[char], i: &mut usize) -> Result<(String, Quoting), QueryError> {
    if *i < chars.len() && chars[*i] == '\'' {
        *i += 1;
        let start = *i;

        // A closing quote anywhere in what is left makes this the paired
        // form, which is the only way to put a space inside an exact
        // term: `'hello world'`.
        //
        // Without one, the quote is fzf's: `'` is a prefix operator and
        // the term runs to the next space, so `'nihao` is those five
        // characters read in the mode the query is not in. That is the
        // syntax fzf's users know. It used to come back as "unterminated
        // quote", which is an error message in place of the answer.
        let closes = chars[start..].contains(&'\'');

        while *i < chars.len() && chars[*i] != '\'' {
            if !closes && chars[*i].is_whitespace() {
                break;
            }
            *i += 1;
        }

        let value: String = chars[start..*i].iter().collect();

        if closes {
            *i += 1; // consume the closing quote
        }

        return Ok((
            value,
            if closes { Quoting::Paired } else { Quoting::Open },
        ));
    }

    let start = *i;
    while *i < chars.len() && !chars[*i].is_whitespace() {
        *i += 1;
    }

    Ok((chars[start..*i].iter().collect(), Quoting::None))
}

/// Decides what an unquoted term is, and strips its operator characters.
///
/// The order matters, and it is fzf's. The anchors are tested first
/// because they are anchors on a literal, not glob syntax; a term that
/// survives them and still holds `*` or `?` is a glob; anything left is
/// read the way `matching` says.
///
/// Between the two anchors the order matters as well: `$` is stripped
/// before `^`, so that a term wearing both is the *third* anchored form —
/// fzf's `EqualMatch`, the whole path and nothing else — rather than a
/// prefix term whose text happens to end in a dollar, which is a query
/// nothing can match. A term that is nothing but `$` is left alone, the
/// way fzf leaves it: it is a dollar sign to look for.
fn classify(
    mut value: String,
    matching: Matching,
    inverted: bool,
) -> Result<(String, TermKind), QueryError> {
    let suffixed = value != "$" && value.ends_with('$');
    if suffixed {
        value.pop();
    }

    if let Some(rest) = value.strip_prefix('^') {
        return Ok((
            rest.to_string(),
            if suffixed {
                TermKind::Equal
            } else {
                TermKind::Prefix
            },
        ));
    }
    if suffixed {
        return Ok((value, TermKind::Suffix));
    }

    if value.contains('*') || value.contains('?') {
        // Validated here for the same reason a regex is: so that a bad
        // pattern becomes a message rather than an empty list.
        // Whether a glob is *valid* does not depend on the case rule,
        // so validation can pick either. The search compiles it again
        // with the rule the user actually set.
        compile_glob(&value, false)?;

        return Ok((value, TermKind::Glob));
    }

    Ok((value, matching.bare_when(inverted)))
}

/// Compiles a glob the way the search will.
///
/// A glob with no `/` in it is given an implicit `**/`, so `*.pdf` matches
/// `docs/report.pdf` rather than only a `.pdf` sitting at a root. A glob
/// that already has a `/` is matched as written, against the whole
/// relative path.
///
/// `literal_separator` is what makes the distinction mean anything: with
/// it, `*` stops at a directory boundary, so `src/*.rs` is the files
/// directly under `src` and not everything beneath it.
pub fn compile_glob(pattern: &str, case_sensitive: bool) -> Result<globset::GlobMatcher, QueryError> {
    let anchored = if pattern.contains('/') {
        pattern.to_string()
    } else {
        format!("**/{pattern}")
    };

    globset::GlobBuilder::new(&anchored)
        .literal_separator(true)
        .case_insensitive(!case_sensitive)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|e| QueryError::InvalidGlob {
            pattern: pattern.to_string(),
            reason: e.to_string(),
        })
}

/// Whether `current` can be answered from `previous`'s result set.
///
/// This is the condition under which `matches(current)` is a subset of
/// `matches(previous)`, so that the search may run over the entries the
/// previous query matched instead of the whole corpus. It is what makes
/// typing get *faster* as the query gets longer, which is the trick fzf
/// uses: it reuses the previous candidate list when the new pattern
/// extends the old one.
///
/// The condition has to be conservative, because this query language is
/// richer than a plain fuzzy pattern and several ways of extending a
/// query *widen* what it matches:
///
/// * `!na` → `!nat` excludes less, so the result set grows.
/// * `a$` → `a$x` turns a suffix term into a fuzzy one that wants a
///   literal `$`; neither set contains the other.
/// * `re` → `re:a` turns two fuzzy characters into a regular expression.
/// * `sr` → `sr*` turns a fuzzy term into a glob.
///
/// So three things are required, and together they are sufficient:
///
/// 1. `previous` has no negative terms. Its set is then a plain
///    intersection of its positive terms, and `current` may add as many
///    exclusions as it likes.
/// 2. Every positive term of `previous` is fuzzy, exact or a prefix.
/// 3. Each corresponding term of `current` is of the *same* kind and has
///    the term of `previous` as a prefix. Any further term of `current`
///    is an additional filter of whatever kind.
///
/// A fuzzy term matches when its text is a subsequence of the path, an
/// exact one when it is a substring and a prefix term when it is a
/// prefix, and dropping the last character of any of the three leaves
/// one of the same sort — so a term that grew at the end can only match
/// fewer paths. Condition 3 is what turns that into a statement about
/// the whole query. The kinds have to agree because they answer the same
/// text differently: `src` as a substring is not a superset of `src` as
/// a subsequence.
///
/// A suffix term reads like it belongs here and does not. Its text is
/// pinned at the far end, so a character typed into it shifts every
/// other character one place away from that end: `a$` → `ab$` is a
/// question about a different pair of positions, and `xab` answers the
/// longer term without answering the shorter one. A whole-path term is
/// the same story at both ends — its two texts are simply two paths.
///
/// The relation is reflexive: a query narrows from itself, which is what
/// makes asking the same query again for a larger page nearly free.
pub fn narrows(previous: &Query, current: &Query) -> bool {
    if !previous.negative.is_empty() {
        return false;
    }
    if current.positive.len() < previous.positive.len() {
        return false;
    }

    previous
        .positive
        .iter()
        .zip(&current.positive)
        .all(|(before, after)| {
            matches!(
                before.kind,
                TermKind::Fuzzy | TermKind::Exact | TermKind::Prefix
            )
                && after.kind == before.kind
                && after.value.starts_with(&before.value)
        })
}

/// Files a parsed term into the positive or the negative list.
fn push(
    positive: &mut Vec<Term>,
    negative: &mut Vec<Term>,
    inverted: bool,
    kind: TermKind,
    value: String,
) {
    if inverted {
        // An inverse term excludes whatever it matches, and how it matches
        // is still its own business: `!*.tmp` is a glob, `!re:^build/` a
        // regex. The kind is carried over rather than flattened to a fuzzy
        // match, which is what made `!` mean something different from the
        // term it negates.
        negative.push(Term { kind, value });
    } else {
        positive.push(Term { kind, value });
    }
}

/// A single search result.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The entry that matched.
    pub id: EntryId,
    /// The match score.
    pub score: i32,
    /// The matched byte offsets within the file name.
    pub positions: Vec<i32>,
    /// Byte offset of the first matched character.
    pub start: i32,
    /// Byte offset just past the last matched character.
    pub end: i32,
}

/// The outcome of a search.
#[derive(Debug, Clone, Default)]
pub struct SearchOutcome {
    /// The matching entries, ranked.
    ///
    /// At most [`SearchOptions::limit`] of them. When `matched` is larger,
    /// this is the best slice of the matches, not all of them.
    pub results: Vec<SearchResult>,
    /// How many entries matched, before the limit was applied.
    ///
    /// This is what fzf's counter reports, and it is the only number that
    /// moves when a query changes but its best few hundred matches do
    /// not. `results.len()` is the size of the caller's buffer once the
    /// matches outgrow it, so it cannot stand in for this.
    pub matched: usize,
    /// The number of entries that were considered.
    pub scanned: usize,
    /// How many of the matches were given a full score.
    ///
    /// A match that provably cannot reach the rows that will be shown is
    /// counted but not scored, so this is at most `matched`. It is the
    /// only number that says whether that happened: a prune that silently
    /// scored everything anyway would leave the results untouched and the
    /// clock the only evidence.
    pub scored: usize,
    /// How long the search took, in microseconds.
    pub elapsed_micros: u64,
}

/// Options that control how a search is run.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Whether matching is case sensitive.
    pub case_sensitive: bool,
    /// What a bare term means.
    ///
    /// The search itself never reads this — by the time it runs, the
    /// query has been parsed and every term says what it is. It lives
    /// here because it is read on every keystroke, because it is set the
    /// way the other options are set, and because the stamp that names
    /// these options is what tells the narrowing cache that the previous
    /// query's match set no longer answers the same text.
    pub matching: Matching,
    /// Whether accented Latin letters are folded.
    pub normalize: bool,
    /// The scoring scheme to use.
    pub scheme: Scheme,
    /// The tiebreak chain.
    pub criteria: Vec<Criterion>,
    /// The maximum number of results to return.
    pub limit: usize,
    /// The glob patterns excluded from the index.
    pub exclude_patterns: Vec<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            matching: Matching::Exact,
            normalize: true,
            scheme: Scheme::Path,
            // The chain comes from the scheme, so that the two cannot drift
            // apart. See `Scheme::criteria`.
            criteria: Scheme::Path.criteria().to_vec(),
            limit: 500,
            exclude_patterns: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kind and value of every positive term, read in fuzzy mode.
    fn terms(input: &str) -> Vec<(TermKind, String)> {
        kinds(input, Matching::Fuzzy)
    }

    /// The kind and value of every positive term, read in exact mode.
    fn exact_terms(input: &str) -> Vec<(TermKind, String)> {
        kinds(input, Matching::Exact)
    }

    fn kinds(input: &str, matching: Matching) -> Vec<(TermKind, String)> {
        Query::parse(input, matching)
            .unwrap()
            .positive
            .into_iter()
            .map(|t| (t.kind, t.value))
            .collect()
    }

    #[test]
    fn parses_a_plain_term() {
        assert_eq!(terms("hello"), vec![(TermKind::Fuzzy, "hello".into())]);
    }

    #[test]
    fn parses_multiple_terms() {
        assert_eq!(
            terms("foo bar"),
            vec![
                (TermKind::Fuzzy, "foo".into()),
                (TermKind::Fuzzy, "bar".into()),
            ]
        );
    }

    #[test]
    fn a_quoted_term_is_an_exact_one() {
        // Quoting used to do nothing but hold a space: the term came back
        // fuzzy, so `'pdf'` matched `papers/draft.tex` through p-d-f.
        assert_eq!(
            terms("'hello world'"),
            vec![(TermKind::Exact, "hello world".into())]
        );
    }

    #[test]
    fn a_leading_quote_alone_is_fzf_s_exact_match() {
        // This is fzf's own syntax, and the only one its users know:
        // `'nihao` means "the characters n-i-h-a-o, together". There is
        // no closing quote in fzf — `'` is a prefix operator, and the
        // term runs to the next space.
        //
        // Requiring the pair made the one thing a user reaches for when
        // fuzzy matching is too loose — `nihao` matching
        // `Hao Law NIW EB1 Documents List.pdf` through five scattered
        // letters — come back as "unterminated quote" instead of an
        // answer.
        assert_eq!(terms("'nihao"), vec![(TermKind::Exact, "nihao".into())]);
    }

    #[test]
    fn a_leading_quote_ends_at_the_space() {
        assert_eq!(
            terms("'nihao docs"),
            vec![
                (TermKind::Exact, "nihao".into()),
                (TermKind::Fuzzy, "docs".into()),
            ]
        );
    }

    #[test]
    fn a_quoted_term_is_literal_through_and_through() {
        // The quotes say "these characters", so the operators inside them
        // are characters too.
        assert_eq!(terms("'^foo'"), vec![(TermKind::Exact, "^foo".into())]);
        assert_eq!(terms("'bar$'"), vec![(TermKind::Exact, "bar$".into())]);
        assert_eq!(terms("'*.pdf'"), vec![(TermKind::Exact, "*.pdf".into())]);
    }

    #[test]
    fn a_bare_term_is_a_substring_in_exact_mode() {
        // Five letters, in that order, with nothing between them. In
        // fuzzy mode the same five letters are a subsequence, which is
        // why `nihao` found `Hao Law NIW EB1 Documents List.pdf`: `ni`
        // from "NIW", `ha` from "Hao", `o` from "Documents".
        assert_eq!(
            exact_terms("nihao"),
            vec![(TermKind::Exact, "nihao".into())]
        );
    }

    #[test]
    fn a_leading_quote_asks_for_the_other_mode() {
        // fzf's `'` is not "match this exactly" — it is "match this the
        // other way round", and `--exact` is what decides which way that
        // is. Checked against fzf 0.70.0 over a 1.07M-entry corpus:
        //
        //     default  nihao  : 174504     -e  nihao  : 0
        //     default  'nihao : 0          -e  'nihao : 174504
        assert_eq!(terms("'nihao"), vec![(TermKind::Exact, "nihao".into())]);
        assert_eq!(
            exact_terms("'nihao"),
            vec![(TermKind::Fuzzy, "nihao".into())]
        );
    }

    #[test]
    fn a_paired_quote_groups_a_phrase_in_either_mode() {
        // The paired form is ours, not fzf's — fzf splits a query on
        // spaces and has no way to put one inside a term. Its job is
        // grouping, so it does not double as the mode switch: in exact
        // mode, inverting it would leave no way to ask for a phrase at
        // all, and a fuzzy match spanning a space matches almost
        // everything.
        assert_eq!(
            terms("'hello world'"),
            vec![(TermKind::Exact, "hello world".into())]
        );
        assert_eq!(
            exact_terms("'hello world'"),
            vec![(TermKind::Exact, "hello world".into())]
        );
    }

    #[test]
    fn the_other_operators_do_not_read_the_mode() {
        // Only the bare term and `'` change meaning. An anchor, a glob
        // and a regex say what they are.
        let expected = vec![
            (TermKind::Prefix, "abc".to_string()),
            (TermKind::Suffix, "def".to_string()),
            (TermKind::Glob, "*.pdf".to_string()),
            (TermKind::Regex, "a.c".to_string()),
        ];

        assert_eq!(terms("^abc def$ *.pdf re:a.c"), expected);
        assert_eq!(exact_terms("^abc def$ *.pdf re:a.c"), expected);
    }

    #[test]
    fn parses_an_inverted_term() {
        let q = Query::parse("foo !bar", Matching::Fuzzy).unwrap();

        assert_eq!(q.positive.len(), 1);
        assert_eq!(q.negative.len(), 1);
        assert_eq!(q.negative[0].value, "bar");
    }

    #[test]
    fn an_inverted_term_keeps_its_own_kind() {
        // `!*.tmp` excludes what the glob matches, not what the characters
        // `*`, `.`, `t`, `m`, `p` fuzzily match.
        let q = Query::parse("!*.tmp !re:^build/ !'exact'", Matching::Fuzzy).unwrap();

        let kinds: Vec<TermKind> = q.negative.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![TermKind::Glob, TermKind::Regex, TermKind::Exact]
        );
    }

    #[test]
    fn parses_prefix_and_suffix_terms() {
        assert_eq!(
            terms("^abc def$"),
            vec![
                (TermKind::Prefix, "abc".into()),
                (TermKind::Suffix, "def".into()),
            ]
        );
    }

    #[test]
    fn a_star_or_a_question_mark_makes_a_glob() {
        assert_eq!(terms("*.pdf"), vec![(TermKind::Glob, "*.pdf".into())]);
        assert_eq!(terms("test_?.py"), vec![(TermKind::Glob, "test_?.py".into())]);
        assert_eq!(terms("src/*.rs"), vec![(TermKind::Glob, "src/*.rs".into())]);
    }

    #[test]
    fn a_term_without_glob_characters_stays_fuzzy() {
        assert_eq!(terms("report.pdf"), vec![(TermKind::Fuzzy, "report.pdf".into())]);
    }

    /// The kind and value of every negative term.
    fn negatives(input: &str, matching: Matching) -> Vec<(TermKind, String)> {
        Query::parse(input, matching)
            .unwrap()
            .negative
            .into_iter()
            .map(|t| (t.kind, t.value))
            .collect()
    }

    #[test]
    fn an_inverted_term_is_a_substring_in_either_mode() {
        // fzf makes an inverted term exact before it reads anything else,
        // so `!ab` keeps `axb` even in fuzzy mode. Asked about it, fzf
        // prints `axb`.
        assert_eq!(
            negatives("!ab", Matching::Fuzzy),
            vec![(TermKind::Exact, "ab".into())]
        );
        assert_eq!(
            negatives("!ab", Matching::Exact),
            vec![(TermKind::Exact, "ab".into())]
        );
    }

    #[test]
    fn an_inverted_quoted_term_is_fuzzy_in_either_mode() {
        // The `'` flips the mode, and for an inverted term there is only
        // one place left to flip to: `!` has already made it exact.
        assert_eq!(
            negatives("!'ab", Matching::Fuzzy),
            vec![(TermKind::Fuzzy, "ab".into())]
        );
        assert_eq!(
            negatives("!'ab", Matching::Exact),
            vec![(TermKind::Fuzzy, "ab".into())]
        );
    }

    #[test]
    fn an_inverted_anchor_is_still_an_anchor() {
        assert_eq!(
            negatives("!^ab", Matching::Fuzzy),
            vec![(TermKind::Prefix, "ab".into())]
        );
        assert_eq!(
            negatives("!ab$", Matching::Exact),
            vec![(TermKind::Suffix, "ab".into())]
        );
    }

    #[test]
    fn an_anchor_at_each_end_asks_for_the_whole_path() {
        // fzf's `EqualMatch`. It used to come out as a prefix term whose
        // text ends in a dollar, which is a query nothing can match.
        assert_eq!(terms("^abc$"), vec![(TermKind::Equal, "abc".into())]);
        assert_eq!(exact_terms("^abc$"), vec![(TermKind::Equal, "abc".into())]);
    }

    #[test]
    fn a_lone_dollar_is_a_character_to_look_for() {
        // fzf guards the suffix operator with `text != "$"`, so a query
        // of one dollar sign looks for a dollar sign rather than ending
        // every path with nothing.
        assert_eq!(terms("$"), vec![(TermKind::Fuzzy, "$".into())]);
        assert_eq!(exact_terms("$"), vec![(TermKind::Exact, "$".into())]);
    }

    #[test]
    fn an_anchor_with_nothing_to_anchor_is_dropped() {
        assert!(Query::parse("^", Matching::Exact).unwrap().is_empty());
        assert!(Query::parse("^$", Matching::Exact).unwrap().is_empty());
    }

    #[test]
    fn the_anchors_win_over_the_glob_characters() {
        // `^` and `$` anchor a literal, so they are read first. A term
        // that is both would otherwise be two things at once.
        assert_eq!(terms("^src*"), vec![(TermKind::Prefix, "src*".into())]);
    }

    #[test]
    fn parses_a_regex_term() {
        assert_eq!(
            terms("re:^src/.*\\.rs$"),
            vec![(TermKind::Regex, "^src/.*\\.rs$".into())]
        );
    }

    #[test]
    fn a_regex_may_be_quoted_to_hold_a_space() {
        assert_eq!(
            terms("re:'a b+'"),
            vec![(TermKind::Regex, "a b+".into())]
        );
    }

    #[test]
    fn an_unclosed_quote_is_an_exact_term_not_an_error() {
        // There used to be an `UnterminatedQuote` error here, and two
        // places in the FFI that caught it and searched for nothing — so
        // typing fzf's own `'nihao` gave an empty panel with no message.
        assert_eq!(terms("'oops"), vec![(TermKind::Exact, "oops".into())]);
    }

    #[test]
    fn rejects_a_regex_that_does_not_compile() {
        let err = Query::parse("re:[", Matching::Fuzzy).unwrap_err();

        assert!(
            matches!(err, QueryError::InvalidRegex { .. }),
            "got {err:?}"
        );
        // The message is one line and says what is wrong, because it is
        // shown in a panel that has room for one line.
        let shown = err.to_string();
        assert!(!shown.contains('\n'), "the message has a newline: {shown:?}");
        assert!(shown.contains("unclosed character class"), "{shown:?}");
    }

    #[test]
    fn rejects_a_glob_that_does_not_compile() {
        let err = Query::parse("*[.pdf", Matching::Fuzzy).unwrap_err();

        assert!(matches!(err, QueryError::InvalidGlob { .. }), "got {err:?}");

        // The pattern in the message is the one that was typed, not the
        // one with the implicit `**/` the search adds.
        let shown = err.to_string();
        assert!(!shown.contains("**/"), "the rewrite leaked: {shown:?}");
    }

    #[test]
    fn rejects_content_search_rather_than_ignoring_it() {
        // It used to parse into a kind no filter recognised, which meant
        // the term vanished and `content:x` matched the whole index.
        assert_eq!(
            Query::parse("content:hello", Matching::Fuzzy).unwrap_err(),
            QueryError::Unsupported("content:".into())
        );
    }

    #[test]
    fn rejects_an_operator_with_nothing_after_it() {
        assert_eq!(
            Query::parse("re:", Matching::Fuzzy).unwrap_err(),
            QueryError::MissingValue("re:".into())
        );
        assert_eq!(
            Query::parse("!", Matching::Fuzzy).unwrap_err(),
            QueryError::MissingValue("!".into())
        );
    }

    #[test]
    fn a_glob_with_no_separator_is_anchored_to_the_name() {
        let glob = compile_glob("*.pdf", false).unwrap();

        assert!(glob.is_match("docs/report.pdf"));
        assert!(glob.is_match("report.pdf"));
        assert!(!glob.is_match("report.pdf.bak"));
    }

    #[test]
    fn a_glob_with_a_separator_is_matched_as_written() {
        let glob = compile_glob("src/*.rs", false).unwrap();

        assert!(glob.is_match("src/main.rs"));
        // `literal_separator` stops `*` at a directory boundary, so this
        // is the files directly under `src` and not everything beneath it.
        assert!(!glob.is_match("src/fuzzy/rank.rs"));
    }

    #[test]
    fn a_glob_follows_the_case_rule_it_is_given() {
        assert!(compile_glob("*.pdf", false).unwrap().is_match("Report.PDF"));
        assert!(!compile_glob("*.pdf", true).unwrap().is_match("Report.PDF"));
    }
}

#[cfg(test)]
mod narrowing_tests {
    use super::*;

    /// Whether `current` narrows `previous`, from the two query strings,
    /// read in fuzzy mode.
    fn narrows_from(previous: &str, current: &str) -> bool {
        narrows_in(Matching::Fuzzy, previous, current)
    }

    /// The same, in exact mode.
    fn narrows_exactly(previous: &str, current: &str) -> bool {
        narrows_in(Matching::Exact, previous, current)
    }

    fn narrows_in(matching: Matching, previous: &str, current: &str) -> bool {
        narrows(
            &Query::parse(previous, matching).unwrap(),
            &Query::parse(current, matching).unwrap(),
        )
    }

    #[test]
    fn extending_an_exact_term_narrows() {
        // The reason is the fuzzy one with one word changed: dropping the
        // last character of a *substring* leaves a substring, so `rea` →
        // `read` can only lose candidates. Without this the panel's
        // default mode pays for the whole corpus on every keystroke —
        // measured over a home directory, `readme` went from 5.8ms to
        // 1ms once the pool was reused.
        assert!(narrows_exactly("rea", "read"));
        assert!(narrows_exactly("read", "read"));
        assert!(narrows_exactly("src", "src t"));
    }

    #[test]
    fn extending_a_prefix_term_narrows() {
        // A path that starts with `src` starts with `sr`, so the pool
        // holds every candidate the longer term can match.
        assert!(narrows_exactly("^sr", "^src"));
        assert!(narrows_from("^sr", "^src"));
    }

    #[test]
    fn extending_a_suffix_term_is_not_narrowing() {
        // `ab` ends in `ab$` and not in `a$`: the term is pinned at the
        // far end, so typing into it moves the characters that were
        // already there. The pool would be missing exactly the paths the
        // longer term reaches.
        assert!(!narrows_exactly("a$", "ab$"));
        assert!(!narrows_exactly("^a$", "^ab$"));
    }

    #[test]
    fn a_term_changing_its_kind_is_not_narrowing() {
        // `src` is a substring in exact mode and `'src` is a
        // subsequence, so the second matches paths the first does not:
        // the pool is missing them.
        assert!(!narrows_exactly("src", "'src"));
        assert!(!narrows_from("'src", "src"));
    }

    #[test]
    fn extending_a_term_narrows() {
        // The whole point: `na` → `nat` can only lose candidates, so the
        // previous result set is a pool the new query can be run over.
        assert!(narrows_from("na", "nat"));
    }

    #[test]
    fn a_query_narrows_from_itself() {
        // The panel asks the same query again with a larger limit when the
        // user scrolls past the first page. That page is free if this holds.
        assert!(narrows_from("nat", "nat"));
    }

    #[test]
    fn adding_a_term_narrows() {
        assert!(narrows_from("src", "src t"));
    }

    #[test]
    fn a_trailing_space_changes_nothing() {
        assert!(narrows_from("src", "src "));
    }

    #[test]
    fn an_added_negative_term_narrows() {
        // An exclusion only ever removes candidates, so a negative term
        // the previous query did not have is safe.
        assert!(narrows_from("src", "src !tmp"));
    }

    #[test]
    fn an_added_term_of_any_kind_narrows() {
        // The added term is a hard filter whatever kind it is; only the
        // terms the previous query already had have to be fuzzy.
        assert!(narrows_from("src", "src *.rs"));
        assert!(narrows_from("src", "src re:^a"));
        assert!(narrows_from("src", "src ^a"));
    }

    #[test]
    fn a_previous_negative_term_is_not_narrowed() {
        // `!na` excludes more than `!nat` does, so the result set *grows*.
        // This is the case that makes the whole thing need a rule.
        assert!(!narrows_from("!na", "!nat"));
    }

    #[test]
    fn an_anchor_becoming_a_fuzzy_term_is_not_narrowing() {
        // `a$` is a suffix term; `a$x` is a fuzzy one that wants a literal
        // `$`. Neither set contains the other.
        assert!(!narrows_from("a$", "a$x"));
    }

    #[test]
    fn typing_the_regex_operator_is_not_narrowing() {
        // `re` is fuzzy `r`,`e`; `re:a` is the regex `a`.
        assert!(!narrows_from("re", "re:a"));
    }

    #[test]
    fn a_term_becoming_a_glob_is_not_narrowing() {
        assert!(!narrows_from("sr", "sr*"));
    }

    #[test]
    fn changing_a_character_is_not_narrowing() {
        assert!(!narrows_from("nat", "nap"));
    }

    #[test]
    fn deleting_a_character_is_not_narrowing() {
        // Backspace widens the set, which is exactly what the pool cannot
        // answer.
        assert!(!narrows_from("nat", "na"));
    }

    #[test]
    fn losing_a_term_is_not_narrowing() {
        assert!(!narrows_from("src t", "src"));
    }

    #[test]
    fn extending_a_quoted_term_narrows_too() {
        // A quoted term is exact in either mode, and an exact term
        // narrows for the same reason a fuzzy one does.
        assert!(narrows_from("'ab'", "'abc'"));
    }
}
