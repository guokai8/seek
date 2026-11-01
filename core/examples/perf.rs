use std::time::Instant;

fn main() {
    // The cost of rebuilding the corpus for the home directory.
    let root = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let path = std::path::PathBuf::from(&root);

    let config = seek_core::scan::ScanConfig {
        roots: vec![path],
        exclude_patterns: vec![
            "Library".into(),
            ".Trash".into(),
            ".cache".into(),
            ".npm".into(),
            ".cargo".into(),
            "node_modules".into(),
            ".git".into(),
            "Applications".into(),
        ],
        ..Default::default()
    };

    let start = Instant::now();
    let (entries, summary) = seek_core::scan::scan_blocking(&config);
    println!(
        "scan: {:?} files={} dirs={}",
        start.elapsed(),
        summary.files,
        summary.dirs
    );

    let start = Instant::now();
    let mut index = seek_core::index::Index::new();
    let mut dir_ids: std::collections::HashMap<std::path::PathBuf, u32> =
        std::collections::HashMap::new();

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
        let parent_path = entry.path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let parent = dir_ids.get(&parent_path).copied();
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
    println!("index build: {:?}", start.elapsed());

    // The scan's own list of entries is a transient copy of the tree, and
    // it is the largest thing this process ever holds. Dropping it before
    // the memory is reported is the difference between the peak during
    // indexing and what the app actually sits on afterwards, which is the
    // number the memory budget is about.
    drop(sorted);
    drop(dir_ids);

    // The corpus build is the suspect: it walks the parent chain of every
    // entry.
    let start = Instant::now();
    let corpus = seek_core::corpus::Corpus::build(&index);
    println!(
        "corpus build: {:?} entries={}",
        start.elapsed(),
        corpus.len()
    );

    // The search itself.
    let query = seek_core::query::Query::parse("readme", seek_core::query::Matching::Fuzzy).unwrap();
    let options = seek_core::query::SearchOptions::default();

    let start = Instant::now();
    let outcome = seek_core::search::search(&corpus, &query, &options);
    println!(
        "search: {:?} hits={}",
        start.elapsed(),
        outcome.results.len()
    );

    // The second corpus build, which is what the watcher pays on every
    // batch of changes.
    let start = Instant::now();
    let _corpus = seek_core::corpus::Corpus::build(&index);
    println!("second corpus build: {:?}", start.elapsed());

    println!(
        "memory: index {:.1} MB + corpus {:.1} MB = {:.1} MB resident for {} entries",
        index.memory_usage() as f64 / 1e6,
        corpus.memory_usage() as f64 / 1e6,
        (index.memory_usage() + corpus.memory_usage()) as f64 / 1e6,
        corpus.entries.len()
    );
}
