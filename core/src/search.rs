//! Parallel search over the corpus.
//!
//! Scoring a few hundred thousand candidates is CPU-bound, and the work
//! divides perfectly: every candidate is independent. Rayon splits the
//! candidate list across the available cores, which turns a 25 ms search
//! into a 4 ms one on an 8-core machine.
//!
//! The results are merged afterwards and ranked with fzf's tiebreak chain.

use rayon::prelude::*;

use crate::corpus::Corpus;
use crate::fuzzy::{compare_keys, FuzzyMatcher, Scratch, SortKeys};
use crate::index::Index;
use crate::query::{Query, SearchOptions, SearchOutcome, SearchResult, TermKind};

/// A candidate that survived the filters.
#[derive(Debug, Clone)]
struct Candidate {
    /// The index entry id.
    id: u32,
    /// The precomputed sort keys.
    keys: SortKeys,
    /// The matched byte offsets.
    positions: Vec<i32>,
}

/// Runs a query against the corpus, using all available cores.
///
/// The whole corpus is scanned linearly and each entry's file name is
/// matched. With a few hundred thousand entries this takes a few
/// milliseconds, which is fast enough to run on every keystroke.
pub fn search(
    corpus: &Corpus,
    query: &Query,
    options: &SearchOptions,
) -> SearchOutcome {
    let start = std::time::Instant::now();
    let mut outcome = SearchOutcome::default();

    let names = &corpus.names;
    outcome.scanned = names.len();

    // Build the matchers once, outside the parallel section.
    let fuzzy_matchers: Vec<FuzzyMatcher> = query
        .positive
        .iter()
        .filter(|t| t.kind == TermKind::Fuzzy)
        .map(|t| {
            FuzzyMatcher::with_options(
                &t.value,
                options.case_sensitive,
                options.scheme,
                options.normalize,
            )
        })
        .collect();

    let exact_matchers: Vec<&str> = query
        .positive
        .iter()
        .filter(|t| t.kind == TermKind::Exact)
        .map(|t| t.value.as_str())
        .collect();
    let prefix_matchers: Vec<&str> = query
        .positive
        .iter()
        .filter(|t| t.kind == TermKind::Prefix)
        .map(|t| t.value.as_str())
        .collect();
    let suffix_matchers: Vec<&str> = query
        .positive
        .iter()
        .filter(|t| t.kind == TermKind::Suffix)
        .map(|t| t.value.as_str())
        .collect();

    // Negative terms are matched as plain fuzzy searches.
    let negative_matchers: Vec<FuzzyMatcher> = query
        .negative
        .iter()
        .map(|t| {
            FuzzyMatcher::with_options(
                &t.value,
                options.case_sensitive,
                options.scheme,
                options.normalize,
            )
        })
        .collect();

    // Score the candidates in parallel. Each worker keeps its own scratch
    // buffer, so no locking is needed inside the loop.
    let mut candidates: Vec<Candidate> = names
        .par_iter()
        .enumerate()
        .filter_map(|(i, name)| {
            // Negative terms exclude the entry.
            if negative_matchers
                .iter()
                .any(|m| m.match_text(name).is_match())
            {
                return None;
            }

            // Exact, prefix and suffix terms are hard filters.
            if !exact_matchers.iter().all(|t| name.contains(t)) {
                return None;
            }
            if !prefix_matchers.iter().all(|t| name.starts_with(t)) {
                return None;
            }
            if !suffix_matchers.iter().all(|t| name.ends_with(t)) {
                return None;
            }

            if fuzzy_matchers.is_empty() {
                let keys = SortKeys::compute(name, 0, 0, 0, &options.criteria);
                return Some(Candidate {
                    id: corpus.entries[i].id,
                    keys,
                    positions: Vec::new(),
                });
            }

            // Fuzzy terms must all match. The score is the sum of the
            // individual scores, which mirrors how fzf combines a multi-term
            // query.
            let mut scratch = Scratch::new();
            let mut total_score = 0i32;
            let mut all_match = true;
            let mut positions = Vec::new();
            let mut match_start = 0i32;
            let mut match_end = 0i32;

            for (index, matcher) in fuzzy_matchers.iter().enumerate() {
                let (result, pos) = matcher.match_text_with_scratch(name, &mut scratch);
                if !result.is_match() {
                    all_match = false;
                    break;
                }
                total_score += result.score;
                // The first term defines the match span used by the chunk
                // and pathname tiebreaks.
                if index == 0 {
                    match_start = result.start;
                    match_end = result.end;
                }
                positions.extend(pos);
            }

            if !all_match {
                return None;
            }
            positions.sort_unstable();
            let keys = SortKeys::compute(name, total_score, match_start, match_end, &options.criteria);
            Some(Candidate {
                id: corpus.entries[i].id,
                keys,
                positions,
            })
        })
        .collect();

    // Rank the candidates with fzf's tiebreak chain. The sort keys were
    // computed during the scan, so the comparator only compares numbers.
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    order.sort_by(|&a, &b| {
        compare_keys(
            &candidates[a].keys,
            &candidates[b].keys,
            &options.criteria,
        )
    });

    outcome.results = order
        .into_iter()
        .take(options.limit)
        .map(|i| SearchResult {
            id: candidates[i].id,
            score: candidates[i].keys.score,
            positions: std::mem::take(&mut candidates[i].positions),
            start: candidates[i].keys.begin,
            end: candidates[i].keys.end,
        })
        .collect();

    outcome.elapsed_micros = start.elapsed().as_micros() as u64;
    outcome
}

/// Runs a query against the corpus and resolves the full paths of the results.
///
/// The paths are only needed for the results that are actually shown, so
/// they are rebuilt after the ranking rather than before it.
pub fn search_with_paths(
    index: &Index,
    corpus: &Corpus,
    query: &Query,
    options: &SearchOptions,
) -> (SearchOutcome, Vec<std::path::PathBuf>) {
    let outcome = search(corpus, query, options);
    let paths = outcome
        .results
        .iter()
        .map(|r| index.full_path(r.id).unwrap_or_default())
        .collect();
    (outcome, paths)
}
