//! Where the resident set actually goes.
//!
//! A fully indexed home directory sits at 285MB of RSS for 258k entries,
//! which is 1.1KB an entry — far more than the structures can account for.
//! This walks the app's own path and reports RSS at each step, so the peak
//! and the steady state can be told apart.

use std::time::Instant;

// macOS keeps freed pages in the malloc zone rather than returning them to
// the kernel, so RSS stays at the high-water mark of the indexing peak long
// after the memory is free. This asks the zone to give them back.
#[link(name = "System", kind = "dylib")]
extern "C" {
    fn malloc_default_zone() -> *mut std::ffi::c_void;
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
}

fn release_free_pages() -> usize {
    unsafe { malloc_zone_pressure_relief(malloc_default_zone(), 0) }
}

use seek_core::index::{EntryKind, FileEntry, Index};
use seek_core::scan::{ScanConfig, ScannedEntry, Scanner};

/// The process's resident set in kilobytes.
fn rss_kb() -> u64 {
    let pid = std::process::id();
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

fn mark(label: &str) {
    println!("{:>9.1} MB  {label}", rss_kb() as f64 / 1024.0);
}

/// The app's own roots and exclusions.
fn app_config() -> ScanConfig {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    ScanConfig {
        roots: vec![std::path::PathBuf::from(home)],
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
    }
}

/// What the entries cost, counted rather than guessed.
fn entry_bytes(entries: &[ScannedEntry]) -> u64 {
    let fixed = std::mem::size_of::<ScannedEntry>() as u64;
    entries
        .iter()
        .map(|e| {
            fixed
                + e.path.as_os_str().len() as u64
                + e.file_name.len() as u64
                // Two heap allocations per entry, with malloc's own header.
                + 32
        })
        .sum()
}

/// What the app actually does at launch: restore last run's snapshot so the
/// panel is usable at once, then rescan the whole disk and replace it.
///
/// Both halves pay the full peak, which is why the app sits at twice what
/// this probe's scan alone costs.
fn app_launch_sequence() {
    use std::ffi::CString;

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let snapshot = format!("{home}/Library/Application Support/Seek/index.skidx");
    if !std::path::Path::new(&snapshot).exists() {
        println!("(no snapshot to restore; run the app once first)");
        return;
    }

    mark("baseline");

    let engine = seek_core::ffi::sk_engine_new();
    let roots = CString::new(home.as_str()).unwrap();
    let excludes = CString::new("Library,.Trash,.cache,.npm,.cargo,node_modules,.git,Applications").unwrap();
    unsafe {
        seek_core::ffi::sk_engine_set_roots(engine, roots.as_ptr());
        seek_core::ffi::sk_engine_set_excludes(engine, excludes.as_ptr());
    }

    let path = CString::new(snapshot).unwrap();
    let loaded = unsafe { seek_core::ffi_persist::sk_engine_load_snapshot(engine, path.as_ptr()) };
    println!("           snapshot loaded = {loaded}, count = {}",
        seek_core::ffi::sk_engine_count(engine));
    mark("after restoring the snapshot (this is what the panel opens with)");

    seek_core::ffi::sk_engine_start_scan(engine);
    while seek_core::ffi::sk_engine_scan_finished(engine) == 0 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    mark("after the walk");

    let count = seek_core::ffi::sk_engine_finish_scan(engine);
    println!("           finish_scan -> {count} entries");
    mark("after finish_scan (drain + rebuild + publish)");

    std::thread::sleep(std::time::Duration::from_secs(3));
    mark("three seconds later");

    let freed = release_free_pages();
    mark("after asking malloc to return its free pages");
    println!("           malloc returned {:.1} MB", freed as f64 / 1_048_576.0);

    unsafe { seek_core::ffi::sk_engine_free(engine) };
    mark("after freeing the engine");
    let freed = release_free_pages();
    println!("           malloc returned {:.1} MB", freed as f64 / 1_048_576.0);
    mark("after asking again");
}

fn main() {
    if std::env::args().any(|a| a == "--launch") {
        app_launch_sequence();
        return;
    }

    // Is the peak reused, or does it accumulate? Three full cycles in one
    // process tell a high-water mark from a leak.
    if std::env::args().any(|a| a == "--cycles") {
        mark("baseline");
        for round in 1..=3 {
            let engine = seek_core::ffi::sk_engine_new();
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            let roots = std::ffi::CString::new(home.as_str()).unwrap();
            let excludes = std::ffi::CString::new(
                "Library,.Trash,.cache,.npm,.cargo,node_modules,.git,Applications",
            )
            .unwrap();
            unsafe {
                seek_core::ffi::sk_engine_set_roots(engine, roots.as_ptr());
                seek_core::ffi::sk_engine_set_excludes(engine, excludes.as_ptr());
            }
            seek_core::ffi::sk_engine_start_scan(engine);
            while seek_core::ffi::sk_engine_scan_finished(engine) == 0 {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let count = seek_core::ffi::sk_engine_finish_scan(engine);
            mark(&format!("round {round}: indexed {count}, engine still alive"));
            unsafe { seek_core::ffi::sk_engine_free(engine) };
            mark(&format!("round {round}: engine freed"));
        }
        return;
    }

    println!("size_of ScannedEntry = {}", std::mem::size_of::<ScannedEntry>());
    println!("size_of FileEntry    = {}", std::mem::size_of::<FileEntry>());
    println!();

    mark("baseline");

    let config = app_config();
    let mut scanner = Scanner::start(config.clone()).unwrap();
    while !scanner.walk_finished() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    mark("after the walk (entries live in the scanner's buffer)");

    // This is what `sk_engine_finish_scan` does.
    let start = Instant::now();
    let mut entries = Vec::new();
    while scanner.drain_into(&mut entries) > 0 {}
    let drained = start.elapsed();
    mark("after draining the buffer into the caller's vector");
    println!(
        "           {} entries, {:.1} MB of them by count, drained in {drained:?}",
        entries.len(),
        entry_bytes(&entries) as f64 / 1_048_576.0
    );

    drop(scanner);
    mark("after dropping the scanner (buffer freed)");

    // The app's rebuild: sort by depth, then insert, with the entries alive
    // the whole time.
    let start = Instant::now();
    let mut index = Index::new();
    let mut dir_ids: std::collections::HashMap<std::path::PathBuf, u32> =
        std::collections::HashMap::new();
    for root in &config.roots {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string());
        let id = index.add_named(None, &name, EntryKind::Directory, 0, 0);
        dir_ids.insert(root.clone(), id);
        index.roots.push(id);
        index.config_roots.push(root.clone());
    }
    entries.sort_by_cached_key(|e| e.path.components().count());
    mark("after sorting by depth");
    println!("           sort took {:?}", start.elapsed());

    let start = Instant::now();
    for entry in &entries {
        let parent_path = entry.path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let parent = dir_ids.get(&parent_path).copied();
        let kind = if entry.is_dir {
            EntryKind::Directory
        } else {
            EntryKind::File
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
    mark("after building the index (entries still alive)");
    println!(
        "           build took {:?}, dir_ids holds {} paths",
        start.elapsed(),
        dir_ids.len()
    );

    drop(dir_ids);
    drop(entries);
    mark("after dropping the entries and dir_ids");

    let start = Instant::now();
    let corpus = seek_core::corpus::Corpus::build(&index);
    mark("after building the corpus");
    println!("           corpus built in {:?}", start.elapsed());

    println!();
    println!("what the structures say they hold:");
    println!(
        "  index  {:>7.1} MB   ({} entries)",
        index.memory_usage() as f64 / 1_048_576.0,
        index.entries.len()
    );
    println!(
        "  corpus {:>7.1} MB",
        corpus.memory_usage() as f64 / 1_048_576.0
    );

    println!();
    mark("steady state, both live");
    std::thread::sleep(std::time::Duration::from_secs(3));
    mark("three seconds later");
}
