//! Query parsing and execution.
//!
//! The query language borrows the useful parts of fzf's extended search
//! syntax without its full complexity:
//!
//! * `foo` — fuzzy match
//! * `'foo'` — exact substring
//! * `^foo` — prefix match
//! * `foo$` — suffix match
//! * `!foo` — inverse match (exclude results containing `foo`)
//! * `content:foo` — search file contents (optional, off by default)
//!
//! Anything that is not a recognised operator is treated as a fuzzy term, and
//! all fuzzy terms must match (AND). Inverse terms must not match (NOT).

use crate::fuzzy::{Criterion, Scheme};
use crate::index::EntryId;

/// Errors that can occur while parsing a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// An operator was used without a value.
    MissingValue(String),
    /// A quoted string was not closed.
    UnterminatedQuote,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::MissingValue(op) => write!(f, "operator {op:?} needs a value"),
            QueryError::UnterminatedQuote => write!(f, "unterminated quoted string"),
        }
    }
}

/// How a single term matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermKind {
    /// Fuzzy subsequence match.
    Fuzzy,
    /// Exact substring match.
    Exact,
    /// Prefix match.
    Prefix,
    /// Suffix match.
    Suffix,
    /// The term must *not* match.
    Inverse,
    /// Search the file's contents.
    Content,
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
    /// Parses a query string.
    pub fn parse(input: &str) -> Result<Query, QueryError> {
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

            let mut kind = TermKind::Fuzzy;
            let mut prefix = false;
            let mut suffix = false;

            // The `content:` operator is checked before the value is read.
            if chars[i..].starts_with(&['c', 'o', 'n', 't', 'e', 'n', 't', ':']) {
                kind = TermKind::Content;
                i += 8;
            }

            // The term value: either a quoted string or a bare run of
            // characters terminated by whitespace.
            let mut value: String = if i < chars.len() && chars[i] == '\'' {
                i += 1;
                let start = i;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(QueryError::UnterminatedQuote);
                }
                let value: String = chars[start..i].iter().collect();
                i += 1; // consume the closing quote
                value
            } else {
                let start = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                chars[start..i].iter().collect()
            };

            // A leading `^` marks a prefix match and a trailing `$` marks a
            // suffix match. The leading marker is stripped here rather than
            // before the value is read, so that a quoted value keeps its
            // literal characters.
            if let Some(rest) = value.strip_prefix('^') {
                prefix = true;
                value = rest.to_string();
            }
            if let Some(rest) = value.strip_suffix('$') {
                suffix = true;
                value = rest.to_string();
            }

            if value.is_empty() {
                if inverted {
                    return Err(QueryError::MissingValue("!".into()));
                }
                continue;
            }

            raw.push_str(&value);
            raw.push(' ');

            if prefix {
                kind = TermKind::Prefix;
            } else if suffix {
                kind = TermKind::Suffix;
            }

            let term = Term { kind, value };
            if inverted || term.kind == TermKind::Inverse {
                negative.push(Term {
                    kind: TermKind::Inverse,
                    value: term.value,
                });
            } else {
                positive.push(term);
            }
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
    pub results: Vec<SearchResult>,
    /// The number of entries that were considered.
    pub scanned: usize,
    /// How long the search took, in microseconds.
    pub elapsed_micros: u64,
}

/// Options that control how a search is run.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Whether matching is case sensitive.
    pub case_sensitive: bool,
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
            normalize: true,
            scheme: Scheme::Path,
            criteria: crate::fuzzy::DEFAULT_CRITERIA.to_vec(),
            limit: 500,
            exclude_patterns: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_term() {
        let q = Query::parse("hello").unwrap();
        assert_eq!(q.positive.len(), 1);
        assert_eq!(q.positive[0].kind, TermKind::Fuzzy);
        assert_eq!(q.positive[0].value, "hello");
    }

    #[test]
    fn parses_multiple_terms() {
        let q = Query::parse("foo bar").unwrap();
        assert_eq!(q.positive.len(), 2);
        assert_eq!(q.positive[1].value, "bar");
    }

    #[test]
    fn parses_a_quoted_term() {
        let q = Query::parse("'hello world'").unwrap();
        assert_eq!(q.positive.len(), 1);
        assert_eq!(q.positive[0].value, "hello world");
    }

    #[test]
    fn parses_an_inverted_term() {
        let q = Query::parse("foo !bar").unwrap();
        assert_eq!(q.positive.len(), 1);
        assert_eq!(q.negative.len(), 1);
        assert_eq!(q.negative[0].value, "bar");
    }

    #[test]
    fn parses_prefix_and_suffix_terms() {
        let q = Query::parse("^abc def$").unwrap();
        assert_eq!(q.positive[0].kind, TermKind::Prefix);
        assert_eq!(q.positive[1].kind, TermKind::Suffix);
    }

    #[test]
    fn rejects_an_unterminated_quote() {
        assert_eq!(
            Query::parse("'oops").unwrap_err(),
            QueryError::UnterminatedQuote
        );
    }
}
