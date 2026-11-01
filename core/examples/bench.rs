//! A benchmark of the search latency.
//!
//! The benchmark indexes a directory and then measures how long a query
//! takes, so that the numbers can be compared with fzf's.

use std::time::Instant;

use seek_core::corpus::Corpus;
use seek_core::index::Index;
use seek_core::query::{Matching, Query, SearchOptions};
use seek_core::scan::{scan_blocking, ScanConfig};

fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| ".".into()));

    println!("scanning {root} …");
    let start = Instant::now();

    let config = ScanConfig {
        roots: vec![std::path::PathBuf::from(&root)],
        exclude_patterns: vec![
            "Library".into(),
            ".Trash".into(),
            ".cache".into(),
            ".npm".into(),
            ".cargo".into(),
            "node_modules".into(),
            ".git".into(),
        ],
        ..Default::default()
    };

    let (entries, summary) = scan_blocking(&config);
    println!("  scanned {} files in {:?}", summary.files, start.elapsed());

    let index_start = Instant::now();
    let mut index = Index::new();
    let mut dir_ids = std::collections::HashMap::new();

    for root in &config.roots {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        let id = index.add_named(None, &name, seek_core::index::EntryKind::Directory, 0, 0);
        dir_ids.insert(root.clone(), id);
        index.roots.push(id);
        index.config_roots.push(root.clone());
    }

    let mut sorted = entries;
    sorted.sort_by_key(|e| e.path.components().count());

    for entry in &sorted {
        let parent = entry
            .path
            .parent()
            .and_then(|p| dir_ids.get(p).copied());
        let kind = if entry.is_dir {
            seek_core::index::EntryKind::Directory
        } else {
            seek_core::index::EntryKind::File
        };
        let id = index.add_named(
            parent,
            &entry.file_name,
            kind,
            entry.size.unwrap_or(0),
            entry.modified.unwrap_or(0),
        );
        if entry.is_dir {
            dir_ids.insert(entry.path.clone(), id);
        }
    }

    println!("  indexed {} entries in {:?}", index.len(), index_start.elapsed());

    let corpus_start = Instant::now();
    let corpus = Corpus::build(&index);
    println!(
        "  built the corpus in {:?} ({} candidates)",
        corpus_start.elapsed(),
        corpus.len()
    );

    let options = SearchOptions::default();
    // `te` is here because it is the shape that costs most: two letters,
    // so the full score matrix, and a run so common that nearly every
    // match holds it.
    let queries = ["src", "cargo", "seek", "index", "test", "te", "s"];

    // The candidate list that fzf is given, so that the two are measured
    // on the same corpus. It is the path relative to the root, which is
    // what `fd` prints and what the matcher scores.
    let candidates: Vec<String> = corpus.paths().map(|p| p.to_string()).collect();
    let mut fzf_input = String::new();
    for candidate in &candidates {
        fzf_input.push_str(candidate);
        fzf_input.push('\n');
    }

    println!("\nquery        mode    seek      fzf");
    println!("-----------  ------  -------------  -------------");

    // Both modes, because the app's default is `Matching::Exact` and
    // fzf's is `Matching::Fuzzy`: a benchmark of one of them measures
    // something half the users of this code never run.
    for (query, matching) in queries
        .iter()
        .flat_map(|q| [(*q, Matching::Exact), (*q, Matching::Fuzzy)])
    {
        let parsed = Query::parse(query, matching).unwrap();

        // The first run is cold, so it is run separately and reported.
        let cold_start = Instant::now();
        let cold = seek_core::search(&corpus, &parsed, &options);
        let cold_elapsed = cold_start.elapsed();

        // The remaining runs are warm.
        let mut total = std::time::Duration::ZERO;
        let runs = 20;
        for _ in 0..runs {
            let start = Instant::now();
            let _ = seek_core::search(&corpus, &parsed, &options);
            total += start.elapsed();
        }
        let warm = total / runs;

        // fzf is measured on the same corpus, so that the numbers can be
        // compared. It is skipped when it is not on PATH.
        let fzf_ms = measure_fzf(&fzf_input, query, matching);

        println!(
            "{query:<12} {:<7} {:>8.2} ms (cold {:>6.2} ms)  {:>8}  {} hits",
            match matching {
                Matching::Exact => "exact",
                Matching::Fuzzy => "fuzzy",
            },
            warm.as_secs_f64() * 1000.0,
            cold_elapsed.as_secs_f64() * 1000.0,
            fzf_ms,
            cold.results.len()
        );
    }

    // Three words that narrow differently: one that never does, one that
    // does from halfway, and one that collapses to nothing.
    for word in ["test", "readme", "seek"] {
        for matching in [Matching::Exact, Matching::Fuzzy] {
            measure_typing(&corpus, &options, word, matching);
        }
    }
}

/// Measures a word being typed, one keystroke at a time.
///
/// Each keystroke is timed twice: once over the whole corpus, and once
/// over what the previous keystroke matched. The second is what the app
/// actually does, and it is the number that says whether typing gets
/// faster or slower as the query grows.
fn measure_typing(
    corpus: &Corpus,
    options: &SearchOptions,
    word: &str,
    matching: Matching,
) {
    println!("\ntyping {word:?} ({matching:?})");
    println!("keys   considered     cold        narrowed   pool kept");
    println!("-----  -------------  ----------  ---------  ----------");

    let mut previous: Option<(Query, Vec<u32>)> = None;

    for end in 1..=word.len() {
        let text = &word[..end];
        let query = Query::parse(text, matching).unwrap();

        let pool = previous
            .as_ref()
            .filter(|(before, _)| seek_core::query::narrows(before, &query))
            .map(|(_, pool)| pool.as_slice());

        // The whole corpus, for comparison: what every keystroke used to
        // cost.
        let runs = 10;
        let mut cold_total = std::time::Duration::ZERO;
        for _ in 0..runs {
            let start = Instant::now();
            let _ = seek_core::search::search_in(corpus, &query, options, None);
            cold_total += start.elapsed();
        }

        // And over the pool. The pool is passed again on each run rather
        // than being consumed, so the ten runs measure the same work.
        let mut warm_total = std::time::Duration::ZERO;
        let mut carried = None;
        for _ in 0..runs {
            let start = Instant::now();
            let (_, next) = seek_core::search::search_in(corpus, &query, options, pool);
            warm_total += start.elapsed();
            carried = next;
        }

        let considered = pool.map_or(corpus.len(), |pool| pool.len());
        let kept = carried.as_ref().map_or("—".to_string(), |p| p.len().to_string());

        println!(
            "{text:<5}  {considered:>13}  {:>7.2} ms  {:>7.2} ms  {kept:>9}",
            cold_total.as_secs_f64() * 1000.0 / runs as f64,
            warm_total.as_secs_f64() * 1000.0 / runs as f64,
        );

        previous = carried.map(|carried| (query, carried));
    }
}

/// Measures how long fzf takes to filter the given candidates.
///
/// Returns an empty string when fzf is not on the PATH.
fn measure_fzf(input: &str, query: &str, matching: Matching) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let Ok(mut child) = Command::new("fzf")
        .arg("--filter")
        .arg(query)
        .arg("--no-multi-line")
        .args(match matching {
            Matching::Exact => vec!["-e"],
            Matching::Fuzzy => vec![],
        })
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return "—".to_string();
    };

    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(input.as_bytes());
    }

    let start = Instant::now();
    let _ = child.wait_with_output();
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;

    format!("{elapsed:.1} ms")
}
