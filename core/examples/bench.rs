//! A benchmark of the search latency.
//!
//! The benchmark indexes a directory and then measures how long a query
//! takes, so that the numbers can be compared with fzf's.

use std::time::Instant;

use fuzzylite_core::corpus::Corpus;
use fuzzylite_core::index::Index;
use fuzzylite_core::query::{Query, SearchOptions};
use fuzzylite_core::scan::{scan_blocking, ScanConfig};

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
        let id = index.add_named(None, &name, fuzzylite_core::index::EntryKind::Directory, 0, 0);
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
            fuzzylite_core::index::EntryKind::Directory
        } else {
            fuzzylite_core::index::EntryKind::File
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
    let queries = ["src", "cargo", "fuzzylite", "index", "test", "s"];

    println!("\nquery        fuzzylite      fzf");
    println!("-----------  -------------  -------------");

    for query in queries {
        let parsed = Query::parse(query).unwrap();

        // The first run is cold, so it is run separately and reported.
        let cold_start = Instant::now();
        let cold = fuzzylite_core::search(&corpus, &parsed, &options);
        let cold_elapsed = cold_start.elapsed();

        // The remaining runs are warm.
        let mut total = std::time::Duration::ZERO;
        let runs = 20;
        for _ in 0..runs {
            let start = Instant::now();
            let _ = fuzzylite_core::search(&corpus, &parsed, &options);
            total += start.elapsed();
        }
        let warm = total / runs;

        println!(
            "{query:<12}  {:>8.2} ms (cold {:>6.2} ms)  {} hits",
            warm.as_secs_f64() * 1000.0,
            cold_elapsed.as_secs_f64() * 1000.0,
            cold.results.len()
        );
    }
}
