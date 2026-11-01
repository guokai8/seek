//! Prints the app's ranking for a query over a candidate list, through the
//! query parser, for comparison against `fzf --filter`.
//!
//! `rank.rs` asks the matcher directly, which is enough for a fuzzy term
//! and no use for anything else: a mode, a quote, a glob or an inverted
//! term only exists once the text has been parsed. This one goes through
//! the same parse-and-search the FFI goes through, so what it prints is
//! what the panel would show.
//!
//! Usage: `cargo run --release --example qrank -- <query> <file> [exact|fuzzy] [path|default]`
//!
//! The file holds one candidate path per line, as `fd` prints them. To
//! compare with fzf:
//!
//! ```text
//! fzf -e --scheme=path -i --filter=nihao < corpus.txt > /tmp/fzf.txt
//! cargo run --release --example qrank -- nihao corpus.txt exact path > /tmp/ours.txt
//! diff /tmp/fzf.txt /tmp/ours.txt
//! ```

use std::collections::HashMap;

use seek_core::corpus::Corpus;
use seek_core::fuzzy::Scheme;
use seek_core::index::{EntryKind, Index};
use seek_core::query::{Matching, Query, SearchOptions};
use seek_core::search::search;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let query = args.get(1).cloned().expect("a query");
    let file = args.get(2).cloned().expect("a corpus file");
    let matching = match args.get(3).map(String::as_str) {
        Some("fuzzy") => Matching::Fuzzy,
        _ => Matching::Exact,
    };
    let scheme = match args.get(4).map(String::as_str) {
        Some("default") => Scheme::Default,
        _ => Scheme::Path,
    };

    let text = std::fs::read_to_string(&file).expect("read the corpus");
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();

    // An index whose relative paths are exactly the input lines, so that
    // what is ranked here is what fzf is given.
    let mut index = Index::new();
    let root = index.add_named(None, "root", EntryKind::Directory, 0, 0);
    index.roots.push(root);
    index.config_roots.push("/root".into());

    let mut dirs: HashMap<String, u32> = HashMap::new();
    let mut sorted = lines.clone();
    sorted.sort_by_key(|l| l.matches('/').count());

    for line in &sorted {
        let parts: Vec<&str> = line.split('/').collect();
        let mut parent = root;

        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                index.add_named(Some(parent), part, EntryKind::File, 1, 0);
                continue;
            }

            let key = parts[..=i].join("/");
            parent = match dirs.get(&key) {
                Some(id) => *id,
                None => {
                    let id = index.add_named(Some(parent), part, EntryKind::Directory, 0, 0);
                    dirs.insert(key, id);
                    id
                }
            };
        }
    }

    index.touch();
    let corpus = Corpus::build(&index);

    let parsed = Query::parse(&query, matching).expect("the query should parse");
    let options = SearchOptions {
        scheme,
        criteria: scheme.criteria().to_vec(),
        // fzf's filter reports its whole match set, so the comparison has
        // to be against the whole of this one.
        limit: usize::MAX,
        ..Default::default()
    };

    let outcome = search(&corpus, &parsed, &options);

    // The ids are resolved back to paths through the corpus, the way the
    // FFI resolves them for the panel.
    for result in &outcome.results {
        let Some(position) = corpus.position(result.id) else {
            continue;
        };
        if let Some(path) = corpus.relative_path(position) {
            println!("{path}");
        }
    }

    eprintln!(
        "{} matched, {:?}, {:?}, {}µs",
        outcome.matched, matching, scheme, outcome.elapsed_micros
    );
}
