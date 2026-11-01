//! Parallel search over the corpus.
//!
//! Scoring a few hundred thousand candidates is CPU-bound, and the work
//! divides perfectly: every candidate is independent. Rayon splits the
//! candidate list across the available cores, which turns a 25 ms search
//! into a 4 ms one on an 8-core machine.
//!
//! The results are merged afterwards and ranked with fzf's tiebreak chain.

use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::corpus::Corpus;
use crate::fuzzy::{
    compare_keys, literal, scan_direction, Anchor, CeilingBoundary, ExactMatcher, FuzzyMatcher,
    Scratch, SortKeys,
};
use crate::index::Index;
use crate::query::{Query, SearchOptions, SearchOutcome, SearchResult, TermKind};

/// A candidate that survived the filters.
#[derive(Debug, Clone)]
struct Candidate {
    /// The index entry id.
    id: u32,
    /// The precomputed sort keys.
    keys: SortKeys,
    /// The position of the candidate in the corpus.
    ///
    /// The matched positions are collected after the ranking, for the
    /// results that are actually shown, and this says which path to
    /// collect them for.
    index: usize,
    /// Whether the candidate was scored.
    ///
    /// An unscored candidate matched — it counts, and it belongs in the
    /// pool the next keystroke searches — but it provably cannot reach the
    /// rows that will be shown, so it has no sort keys and takes no part
    /// in the ranking. See [`Prune`].
    scored: bool,
}

/// What it takes to leave a match unscored.
///
/// A broad query matches far more paths than the panel shows, so nearly
/// every match is scored, ranked and discarded. Two facts together make
/// most of that work skippable:
///
/// * no match can score above the term's ceiling — every character worth
///   `SCORE_MATCH` plus the best bonus the scheme pays, the first one's
///   counted twice, and no gaps; and
/// * a path that does not hold the term as one consecutive run *at a
///   boundary* scores at least three below that ceiling. Without a run
///   at all it pays a gap, and a gap costs at least `SCORE_GAP_START`;
///   with a run somewhere else it pays one bonus less for every
///   character of the run, because the later characters inherit the
///   first one's bonus. [`crate::fuzzy::FuzzyMatcher::ceiling_boundary`]
///   is where that is written down, and what says which patterns it
///   holds for.
///
/// So once `limit` candidates have turned up within two of the ceiling,
/// the limit-th best score is at least that high, and a path with no run
/// at a boundary cannot be among the rows shown. It still has to be
/// counted, which [`crate::fuzzy::FuzzyMatcher::occurs_in`] does without
/// scoring it.
///
/// The boundary half is what reaches the queries that hurt most. `te`
/// over a home directory matches 196,680 paths and 88.5% of them hold it
/// as a consecutive run, so the run test alone dismisses one match in
/// nine; asking where the run sits dismisses about three in four.
///
/// This only bites when the rows shown are at the top of the scoring
/// range, which is exactly the case for the queries that are slow: over a
/// home directory the five-hundredth best score for `test`, `src` and
/// `index` *is* the best possible score. For a query whose best rows sit
/// below the ceiling the prune never arms, and costs one relaxed load per
/// candidate.
struct Prune<'a> {
    /// The lowest score that counts as being at the ceiling: one above
    /// the most a path without a consecutive run can reach.
    near_ceiling: i32,
    /// The term, for the substring test.
    pattern: &'a str,
    /// The matcher the term was compiled into, for counting a candidate
    /// the prune left unscored.
    matcher: &'a FuzzyMatcher,
    /// Where a run of the term has to sit to be worth scoring.
    boundary: CeilingBoundary,
    /// How many candidates at the ceiling have been seen.
    ///
    /// Shared across the workers rather than kept per worker, because
    /// rayon splits the candidates into many more pieces than there are
    /// cores and a per-piece count would rarely reach a limit of several
    /// hundred. Which candidates end up pruned therefore depends on the
    /// order the work happens to run in — the rows do not, because a
    /// pruned candidate is one that provably cannot be shown.
    seen: AtomicUsize,
}

/// A positive term that contributes a score, compiled.
///
/// Fuzzy and exact terms both score — fzf ranks an exact match by the
/// same scoring function it ends a fuzzy match with — while a glob, a
/// regex and an anchor only decide whether the candidate is in the
/// running. The two scoring kinds are kept in one list, in the order they
/// were typed, so that "the first term" means what it says: it is the
/// span the chunk and pathname tiebreaks are measured from.
pub enum Scorer {
    /// A subsequence match, scored by the DP matrix.
    Fuzzy(FuzzyMatcher),
    /// A substring match, scored over the run it found.
    Exact(ExactMatcher),
}

impl Scorer {
    /// Compiles the scoring terms of a parsed query's positive terms, in
    /// the order they were typed.
    ///
    /// The panel asks for this too: the characters it underlines have to
    /// be the ones the ranking was measured from, so there is one place
    /// that says which term scores and how.
    pub fn compile(terms: &[crate::query::Term], options: &SearchOptions) -> Vec<Scorer> {
        terms
            .iter()
            .filter_map(|t| match t.kind {
                TermKind::Fuzzy => Some(Scorer::Fuzzy(
                    FuzzyMatcher::with_options(
                        &t.value,
                        options.case_sensitive,
                        options.scheme,
                        options.normalize,
                    )
                    .scanning(scan_direction(&options.criteria)),
                )),
                TermKind::Exact => Some(Scorer::Exact(
                    ExactMatcher::new(
                        &t.value,
                        options.case_sensitive,
                        options.scheme,
                        options.normalize,
                    )
                    .scanning(scan_direction(&options.criteria)),
                )),
                // An anchored term sits where the arithmetic puts it, so
                // there is no occurrence to choose and the scan direction
                // says nothing. What it is not is a filter: fzf scores it
                // through the same function an exact term goes through.
                TermKind::Prefix | TermKind::Suffix | TermKind::Equal => {
                    let anchor = match t.kind {
                        TermKind::Prefix => Anchor::Start,
                        TermKind::Suffix => Anchor::End,
                        _ => Anchor::Whole,
                    };

                    Some(Scorer::Exact(
                        ExactMatcher::new(
                            &t.value,
                            options.case_sensitive,
                            options.scheme,
                            options.normalize,
                        )
                        .anchored(anchor),
                    ))
                }
                _ => None,
            })
            .collect()
    }

    /// Matches the term against `text`.
    ///
    /// `scratch` is the fuzzy matcher's working buffers, reused across
    /// candidates; the exact matcher needs none and ignores it.
    pub fn match_text(&self, text: &str, scratch: &mut Scratch) -> crate::fuzzy::MatchResult {
        match self {
            Scorer::Fuzzy(m) => m.match_text_fast(text, scratch),
            Scorer::Exact(m) => m.match_text(text),
        }
    }

    /// Where the term matched in `text`, as byte offsets.
    pub fn positions(&self, text: &str, scratch: &mut Scratch) -> Vec<crate::fuzzy::Position> {
        match self {
            Scorer::Fuzzy(m) => m.match_text_with_scratch(text, scratch).1,
            Scorer::Exact(m) => m.match_text_with_positions(text).1,
        }
    }
}

/// The literal terms of a query, grouped by how they anchor.
///
/// They are pulled out of the term list once rather than filtered inside
/// the per-candidate closure, which would re-walk the query for every one
/// of a quarter of a million candidates.
struct Literals<'a> {
    /// Terms that must appear somewhere in the path.
    contains: Vec<&'a str>,
    /// Terms the path must start with.
    prefixes: Vec<&'a str>,
    /// Terms the path must end with.
    suffixes: Vec<&'a str>,
    /// Terms the path must be.
    equals: Vec<&'a str>,
}

impl<'a> Literals<'a> {
    /// Collects every literal term of `terms`.
    ///
    /// This is the form the negative terms take. An inverted term only
    /// has to decide whether the candidate is out, so there is nothing to
    /// score and `contains` is the whole of the question.
    fn of(terms: &'a [crate::query::Term]) -> Literals<'a> {
        Literals {
            contains: pick(terms, TermKind::Exact),
            prefixes: pick(terms, TermKind::Prefix),
            suffixes: pick(terms, TermKind::Suffix),
            equals: pick(terms, TermKind::Equal),
        }
    }

    /// Whether `text` satisfies any literal term.
    fn any_match(&self, text: &str, case_sensitive: bool) -> bool {
        self.contains
            .iter()
            .any(|t| literal::contains(text, t, case_sensitive))
            || self
                .prefixes
                .iter()
                .any(|t| literal::starts_with(text, t, case_sensitive))
            || self
                .suffixes
                .iter()
                .any(|t| literal::ends_with(text, t, case_sensitive))
            || self
                .equals
                .iter()
                .any(|t| literal::equals(text, t, case_sensitive))
    }
}

/// Folds one term's match into the span the tiebreaks are measured from.
///
/// fzf's `buildResult` takes the earliest beginning and the latest end of
/// all the terms' offsets, not the first term's — `^src rs$` is credited
/// with the whole stretch from the `s` of `src` to the end of `.rs`,
/// which is what the chunk and pathname criteria then measure. A span of
/// no width is not a match at a place; fzf skips it, and so does this.
///
/// `start` and `end` are both zero until the first term with a width
/// lands, which is the state fzf calls "no valid offset found".
pub fn span(start: &mut i32, end: &mut i32, from: i32, to: i32) {
    if to <= from {
        return;
    }
    if *end <= *start {
        *start = from;
        *end = to;
        return;
    }

    *start = (*start).min(from);
    *end = (*end).max(to);
}

/// The values of every term of one kind.
fn pick<'a>(terms: &'a [crate::query::Term], kind: TermKind) -> Vec<&'a str> {
    terms
        .iter()
        .filter(|t| t.kind == kind)
        .map(|t| t.value.as_str())
        .collect()
}

/// Compiles the glob terms of a term list.
///
/// A pattern that does not compile is dropped rather than returned as an
/// error, because it cannot happen: `Query::parse` compiled every one of
/// them already and refused the query if any failed. The case rule is the
/// search's own, so a glob obeys the same one as everything else typed
/// into the field.
fn compile_globs(terms: &[crate::query::Term], case_sensitive: bool) -> Vec<globset::GlobMatcher> {
    terms
        .iter()
        .filter(|t| t.kind == TermKind::Glob)
        .filter_map(|t| crate::query::compile_glob(&t.value, case_sensitive).ok())
        .collect()
}

/// Compiles the regex terms of a term list.
///
/// The case rule is the search's own, so that a regex obeys the same one
/// as everything else typed into the field.
fn compile_regexes(terms: &[crate::query::Term], case_sensitive: bool) -> Vec<regex::Regex> {
    terms
        .iter()
        .filter(|t| t.kind == TermKind::Regex)
        .filter_map(|t| {
            regex::RegexBuilder::new(&t.value)
                .case_insensitive(!case_sensitive)
                .build()
                .ok()
        })
        .collect()
}

/// Whether a streaming search should carry on after a publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Keep searching.
    Continue,
    /// Abandon the search, and report only what had been found.
    ///
    /// This is what makes a superseded keystroke cheap: a search the user
    /// has already typed past is stopped where it stands, instead of
    /// spending tens of milliseconds on an answer nobody will see.
    Stop,
}

/// Runs a query in chunks, publishing the best rows so far as it goes.
///
/// `chunk` is how many candidates are scored between publishes. The whole
/// corpus as one chunk is [`search_in`]: one publish, at the end.
///
/// `on_progress` is called once per chunk with the best rows found so far
/// and how many candidates have matched so far, and says whether to carry
/// on. It runs on the calling thread, between chunks, so it is not
/// contending with the workers.
///
/// # What a publish is, and is not
///
/// The ranking is global: the best match can be the very last candidate.
/// So a publish is the top rows *of the candidates seen so far*, and a
/// later one is allowed to replace it — which is what the panel has to be
/// prepared for. What never happens is a row that a plain ranking of that
/// prefix would not have shown.
///
/// The rows handed to `on_progress` carry no matched positions. They are
/// wanted only for the handful of rows a caller actually draws, and
/// collecting them for every page of every chunk costs more than the
/// chunk. [`SearchOutcome::results`] has them, for the final page.
///
/// # A stopped search returns no pool
///
/// The pool is the query's *complete* match set, and the next keystroke
/// searches it instead of the corpus. A stopped search has only part of
/// that set, so handing it back would silently lose every row the search
/// never got to. It returns `None`, which sends the next keystroke to the
/// corpus — slower, and right.
pub fn search_streaming(
    corpus: &Corpus,
    query: &Query,
    options: &SearchOptions,
    pool: Option<&[u32]>,
    chunk: usize,
    on_progress: &mut dyn FnMut(&[SearchResult], usize) -> Progress,
) -> (SearchOutcome, Option<Vec<u32>>) {
    let start = std::time::Instant::now();
    let mut outcome = SearchOutcome::default();

    // Build the matchers once, outside the parallel section.
    let scorers = Scorer::compile(&query.positive, options);

    // The prune applies to a single fuzzy term. With several the score is
    // their sum, and the bound would have to be taken term by term — more
    // work, for a kind of query that is already narrow. A term that is not
    // plain ASCII is left out too: the substring test below works on
    // bytes, while the matcher folds accented letters, so for anything but
    // ASCII the two ask different questions.
    //
    // A one-character term is left out because for it the prune cannot
    // save anything. "Contains the character" and "matches the character"
    // are the same question, so every candidate the prune would dismiss is
    // one that does not match at all — and it would pay a substring search
    // on every path to find that out. Measured over a home directory,
    // arming it for `s` cost 9%.
    //
    // `ceiling_boundary` is what refuses the rest: a term of one
    // character, a term that is not plain ASCII, and a term that carries
    // its own boundary. It returning `None` is what leaves the prune off.
    let prune: Option<Prune> = match scorers.as_slice() {
        [Scorer::Fuzzy(only)] if options.limit > 0 => only.ceiling_boundary().map(|boundary| Prune {
            near_ceiling: only.score_ceiling() - 2,
            pattern: only.pattern(),
            matcher: only,
            boundary,
            seen: AtomicUsize::new(0),
        }),
        _ => None,
    };


    // A glob and a regex are compiled once per query and then asked about
    // every candidate. The compile is microseconds; the asking is the part
    // that runs a quarter of a million times.
    //
    // The parser already compiled both to check them, so neither can fail
    // here. A pattern that does not compile never reaches this point: it
    // leaves `Query::parse` as an error the panel shows.
    let globs: Vec<globset::GlobMatcher> = compile_globs(&query.positive, options.case_sensitive);
    let regexes: Vec<regex::Regex> = compile_regexes(&query.positive, options.case_sensitive);

    // An inverse term excludes what it matches, using whatever kind of
    // matching it was written with. `!*.tmp` is a glob and `!re:^build/`
    // is a regex; treating every negative term as a fuzzy one made `!`
    // mean something different from the term it negated.
    let negative_literals = Literals::of(&query.negative);
    let negative_globs: Vec<globset::GlobMatcher> =
        compile_globs(&query.negative, options.case_sensitive);
    let negative_regexes: Vec<regex::Regex> =
        compile_regexes(&query.negative, options.case_sensitive);

    let negative_matchers: Vec<FuzzyMatcher> = query
        .negative
        .iter()
        .filter(|t| t.kind == TermKind::Fuzzy)
        .map(|t| {
            FuzzyMatcher::with_options(
                &t.value,
                options.case_sensitive,
                options.scheme,
                options.normalize,
            )
            .scanning(scan_direction(&options.criteria))
        })
        .collect();

    // What one candidate amounts to. It is a closure rather than the body
    // of the loop because there are two loops — over the pool and over the
    // corpus — and they must not be two copies of this.
    let consider = |i: usize, scratch: &mut Scratch| -> Option<Candidate> {
        // The paths live in one contiguous arena, so a candidate is
        // addressed by its position rather than handed out as a separate
        // allocation.
        let name = corpus.relative_path(i)?;

        // Negative terms exclude the entry, each by its own kind.
        if negative_literals.any_match(name, options.case_sensitive)
            || negative_globs.iter().any(|g| g.is_match(name))
            || negative_regexes.iter().any(|r| r.is_match(name))
            || negative_matchers
                .iter()
                .any(|m| m.match_text_fast(name, scratch).is_match())
        {
            return None;
        }

        // A glob and a regex are hard filters: they decide whether the
        // candidate is in the running at all and contribute nothing to
        // its score. Every literal term scores, anchored or not, which is
        // why none of them is filtered here.
        if !globs.iter().all(|g| g.is_match(name)) {
            return None;
        }
        if !regexes.iter().all(|r| r.is_match(name)) {
            return None;
        }

        if scorers.is_empty() {
            let keys = SortKeys::compute(name, corpus.trim_lengths[i], 0, 0, 0, &options.criteria);
            return Some(Candidate {
                id: corpus.entries[i].id,
                keys,
                index: i,
                scored: true,
            });
        }

        // Once the prune has armed, a path that does not hold the term as
        // a run at a boundary is counted but not scored. The load is
        // relaxed and the counter is written to only until it arms, so
        // until then this is the whole of the prune's cost, and after
        // that the line sits in every core's cache.
        if let Some(prune) = &prune {
            if prune.seen.load(Ordering::Relaxed) >= options.limit
                && name.is_ascii()
                && !literal::contains_after(name, prune.pattern, options.case_sensitive, |prev| {
                    prune.boundary.follows(prev)
                })
            {
                return prune.matcher.occurs_in(name).then(|| Candidate {
                    id: corpus.entries[i].id,
                    keys: SortKeys::default(),
                    index: i,
                    scored: false,
                });
            }
        }

        // Fuzzy terms must all match. The score is the sum of the
        // individual scores, which mirrors how fzf combines a multi-term
        // query.
        //
        // The matched positions are not collected here. They are only
        // needed for the handful of results that are actually shown, and
        // collecting them for every candidate costs more than the
        // matching itself.
        let mut total_score = 0i32;
        let mut match_start = 0i32;
        let mut match_end = 0i32;

        for scorer in scorers.iter() {
            let result = scorer.match_text(name, scratch);
            if !result.is_match() {
                return None;
            }
            total_score += result.score;
            span(&mut match_start, &mut match_end, result.start, result.end);
        }

        // Candidates at the top of the scoring range are what arm the
        // prune. The counting stops once it has armed, so the shared
        // counter is written to only briefly.
        if let Some(prune) = &prune {
            if total_score >= prune.near_ceiling
                && prune.seen.load(Ordering::Relaxed) < options.limit
            {
                prune.seen.fetch_add(1, Ordering::Relaxed);
            }
        }

        let keys = SortKeys::compute(
            name,
            corpus.trim_lengths[i],
            total_score,
            match_start,
            match_end,
            &options.criteria,
        );
        Some(Candidate {
            id: corpus.entries[i].id,
            keys,
            index: i,
            scored: true,
        })
    };

    // fzf appends `index` — the position in the input — to every tiebreak
    // chain, so two candidates that tie on every criterion come out in the
    // order they were fed in. Saying that outright makes the comparison a
    // *total* order, and that is what allows the list to be cut down
    // before it is sorted: only `limit` rows are ever shown, and a common
    // query matches a hundred thousand paths. Ordering the ones nobody
    // will see is the single most expensive thing left after the matching.
    //
    // It takes the candidates as an argument rather than capturing them,
    // because the list is still being appended to between chunks and a
    // closure holding a borrow of it could not be.
    let compare = |all: &[Candidate], a: usize, b: usize| {
        compare_keys(&all[a].keys, &all[b].keys, &options.criteria)
            .then_with(|| all[a].index.cmp(&all[b].index))
    };

    let total = pool.map_or(corpus.len(), |pool| pool.len());
    let chunk = chunk.max(1);

    // Every match, in corpus order. This is what the pool is taken from
    // and what the match count counts, so it holds the pruned candidates
    // too — they matched, they just cannot be shown.
    let mut candidates: Vec<Candidate> = Vec::new();
    // The best `limit` of them, as positions in `candidates`, ordered.
    let mut best: Vec<usize> = Vec::new();
    let mut scored = 0usize;
    let mut consumed = 0usize;
    let mut stopped = false;
    // Reused between publishes so that a page is not reallocated per chunk.
    let mut page: Vec<SearchResult> = Vec::new();

    while consumed < total {
        let end = consumed.saturating_add(chunk).min(total);

        // Score this chunk in parallel. Each worker keeps its own scratch
        // buffer, so no locking is needed inside the loop and the
        // matcher's working memory is allocated once per worker rather
        // than once per candidate. `map_init` is what makes that true: the
        // buffer has to be created outside the per-candidate closure, or
        // every candidate reallocates all of it and the allocator becomes
        // the bottleneck instead of the matching.
        //
        // Both a `Range` and a slice are *indexed* parallel iterators, so
        // rayon's `collect` keeps the original order either way. That is
        // what keeps the candidates in corpus order, which is what lets
        // the comparison above break ties the way fzf's trailing `index`
        // criterion does — and it is also why the pool is ascending, and
        // so can be handed straight back as the input to the next
        // keystroke.
        //
        // The options are unwrapped with `filter_map` rather than
        // `flatten`. The two say the same thing, but `flatten` makes a
        // one-element iterator out of every candidate, which measured
        // about a fifth slower over a corpus of this size.
        #[allow(clippy::filter_map_identity)]
        let found: Vec<Candidate> = match pool {
            Some(pool) => pool[consumed..end]
                .par_iter()
                .map_init(Scratch::new, |scratch, &i| consider(i as usize, scratch))
                .filter_map(|candidate| candidate)
                .collect(),
            None => (consumed..end)
                .into_par_iter()
                .map_init(Scratch::new, |scratch, i| consider(i, scratch))
                .filter_map(|candidate| candidate)
                .collect(),
        };

        let base = candidates.len();
        candidates.extend(found);
        consumed = end;

        // Merge this chunk's scored candidates into the running best. Once
        // the page is full, a candidate that does not beat the row at the
        // bottom of it cannot enter, and saying so here keeps the merge
        // proportional to what actually competes rather than to the chunk:
        // for a broad query, after the first chunk or two that is a small
        // fraction of it.
        //
        // The floor is kept current *inside* the loop, not taken once at
        // the top of the chunk. A chunk can be the whole corpus —
        // `search_in` has exactly one — and then a floor fixed before the
        // loop is no floor at all: every scored candidate goes into `best`
        // and a single selection then runs over the lot, with a comparator
        // that reads two candidates out of several megabytes for every
        // comparison. Measured over a home directory, that was 4.8 ms of
        // `s`'s 14; keeping it current makes the same merge 1.4 ms.
        let mut floor = (options.limit > 0 && best.len() >= options.limit)
            .then(|| best[best.len() - 1]);
        for i in base..candidates.len() {
            if !candidates[i].scored {
                continue;
            }
            scored += 1;
            if floor.is_some_and(|w| compare(&candidates, i, w) != std::cmp::Ordering::Less) {
                continue;
            }
            best.push(i);

            // Trim when the list has grown to twice the limit rather than
            // as soon as it passes it. Selecting on every push beyond the
            // limit is a selection per candidate, which measured slower
            // than having no floor at all; trimming at twice the limit
            // amortises one selection over `limit` pushes.
            // Saturating, because a caller that wants everything says so
            // with `usize::MAX` — `qrank` does, to compare whole match
            // sets against fzf's — and doubling that overflows.
            if options.limit > 0 && best.len() >= options.limit.saturating_mul(2) {
                best.select_nth_unstable_by(options.limit - 1, |&a, &b| {
                    compare(&candidates, a, b)
                });
                best.truncate(options.limit);
                floor = Some(best[options.limit - 1]);
            }
        }

        if best.len() > options.limit {
            // Partition so the rows that will be shown are at the front.
            // This is allowed to be unstable precisely because the
            // comparison is a total order: there are no ties left for it
            // to disturb.
            best.select_nth_unstable_by(options.limit, |&a, &b| compare(&candidates, a, b));
            best.truncate(options.limit);
        }
        best.sort_unstable_by(|&a, &b| compare(&candidates, a, b));

        // Publish. The rows carry no positions; see the note above.
        page.clear();
        page.extend(best.iter().map(|&i| SearchResult {
            id: candidates[i].id,
            score: candidates[i].keys.score,
            positions: Vec::new(),
            start: candidates[i].keys.begin,
            end: candidates[i].keys.end,
        }));

        if on_progress(&page, candidates.len()) == Progress::Stop {
            stopped = true;
            break;
        }
    }

    // How many candidates were actually considered — the whole pool or
    // corpus for a search that ran to the end, and only what it reached
    // for one that was stopped. This is the number that shows whether
    // narrowing happened at all.
    outcome.scanned = consumed;

    // Every candidate that got this far matched, so this is the count
    // before the limit is applied.
    outcome.matched = candidates.len();
    outcome.scored = scored;

    // This query's own pool, for the next keystroke. A pool that covers
    // most of the corpus is not taken at all, and nor is one from a
    // stopped search; see the doc comment for why either would cost more
    // than it saves, or be wrong.
    let narrowed: Option<Vec<u32>> = (!stopped && candidates.len() <= corpus.len() / 2)
        .then(|| candidates.iter().map(|c| c.index as u32).collect());

    // The matched positions are only needed for the results that are
    // actually shown, so they are collected here rather than for every
    // candidate during the scan, and once rather than per publish.
    let mut scratch = Scratch::new();

    outcome.results = best
        .iter()
        .map(|&i| {
            let mut positions = Vec::new();

            let name = corpus
                .relative_path(candidates[i].index)
                .unwrap_or_default();

            for scorer in &scorers {
                positions.extend(scorer.positions(name, &mut scratch));
            }

            positions.sort_unstable();

            SearchResult {
                id: candidates[i].id,
                score: candidates[i].keys.score,
                positions,
                start: candidates[i].keys.begin,
                end: candidates[i].keys.end,
            }
        })
        .collect();

    outcome.elapsed_micros = start.elapsed().as_micros() as u64;
    (outcome, narrowed)
}

/// Runs a query against the whole corpus, using all available cores.
///
/// Every entry's path is matched. With a few hundred thousand entries this
/// takes a few milliseconds, which is fast enough to run on every
/// keystroke. [`search_in`] is the same search over a smaller candidate
/// pool.
pub fn search(corpus: &Corpus, query: &Query, options: &SearchOptions) -> SearchOutcome {
    search_in(corpus, query, options, None).0
}

/// Runs a query over a pool of corpus positions, and returns its own pool.
///
/// `pool` is the set of corpus positions the previous query matched. It may
/// only be passed when [`crate::query::narrows`] says that this query's
/// matches are a subset of that query's — otherwise a path this query
/// should find is simply absent from the candidates, and the result is a
/// search that quietly loses rows. The rule, and why it has to be
/// conservative, is documented on `narrows`.
///
/// The returned pool is this query's complete match set, in ascending
/// corpus order, so it can be handed back here for the next keystroke. It
/// is the set *before* the limit: the limit decides how many rows are
/// shown, not which paths matched, so asking the same query again for a
/// longer page is answered from the pool as well.
///
/// It is `None` when the match set covers more than half the corpus. A
/// fuzzy term of one or two characters matches almost everything — `c`
/// matches 208,000 of 208,156 paths over a home directory — and a pool
/// that size was measured to save two tenths of a millisecond on a ten
/// millisecond search, while building it allocates four bytes per match
/// and copies them. The next keystroke then goes back to the corpus,
/// which is what a pool that big would have amounted to anyway.
///
/// With `None` for `pool` the whole corpus is searched, which is what the
/// first keystroke of a query does.
pub fn search_in(
    corpus: &Corpus,
    query: &Query,
    options: &SearchOptions,
    pool: Option<&[u32]>,
) -> (SearchOutcome, Option<Vec<u32>>) {
    // One chunk: the whole pool, or the whole corpus. There is no second
    // implementation of the search — this is `search_streaming` with
    // nothing to report until it has finished.
    search_streaming(corpus, query, options, pool, usize::MAX, &mut |_, _| {
        Progress::Continue
    })
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
