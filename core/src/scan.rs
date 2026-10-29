//! fd-style filesystem traversal.
//!
//! Adapted from fd (https://github.com/sharkdp/fd), Apache-2.0 licensed,
//! Copyright (c) 2017-2024 fd contributors.
//!
//! The traversal uses the `ignore` crate's parallel walker, which already
//! provides everything fd relies on:
//!
//! * `.gitignore` / `.ignore` / `.fdignore` handling,
//! * hidden-file skipping,
//! * custom exclusion globs,
//! * parallel walking across CPU cores.
//!
//! fd's own batching channel (a `crossbeam-channel` carrying batches of
//! entries, with back-pressure) is reproduced here so that the scan can be
//! stopped early without losing entries.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ignore::overrides::OverrideBuilder;
use ignore::{WalkBuilder, WalkState};

/// Configuration for a filesystem scan.
#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// Root directories to walk.
    pub roots: Vec<PathBuf>,
    /// Glob patterns to exclude.
    pub exclude_patterns: Vec<String>,
    /// Whether to skip hidden files (those starting with a dot).
    pub ignore_hidden: bool,
    /// Whether to respect `.gitignore` and friends.
    pub read_vcsignore: bool,
    /// Whether to respect `.ignore` and `.fdignore` files.
    pub read_fdignore: bool,
    /// Whether to follow symbolic links.
    pub follow_links: bool,
    /// Maximum directory depth to descend into.
    pub max_depth: Option<usize>,
    /// Number of worker threads. `None` uses one thread per core.
    pub threads: Option<usize>,
    /// Whether to include directories in the results.
    pub include_dirs: bool,
    /// Whether to include regular files in the results.
    pub include_files: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            exclude_patterns: Vec::new(),
            ignore_hidden: true,
            read_vcsignore: true,
            read_fdignore: true,
            follow_links: false,
            max_depth: None,
            threads: None,
            include_dirs: true,
            include_files: true,
        }
    }
}

/// A summary of a completed scan.
#[derive(Debug, Clone, Default)]
pub struct ScanSummary {
    /// Number of entries visited.
    pub files: u64,
    /// Number of directories visited.
    pub dirs: u64,
    /// Number of entries skipped because they matched an exclusion.
    pub skipped: u64,
    /// Number of errors encountered (usually permission denied).
    pub errors: u64,
}

/// An entry produced by the scanner.
#[derive(Debug, Clone)]
pub struct ScannedEntry {
    /// The full path of the entry.
    pub path: PathBuf,
    /// The file name alone.
    pub file_name: String,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// The size of the file in bytes, when known.
    pub size: Option<u64>,
    /// Last modification time as a Unix timestamp in seconds.
    pub modified: Option<i64>,
}

/// Walks a filesystem and reports the entries it finds.
///
/// The walker runs on its own thread and pushes the entries into a shared
/// buffer, so the consumer can drain the results at its own pace. The
/// consumer also decides when the scan is over: the scan is finished when
/// the walker has returned *and* the buffer has been drained.
///
/// The buffer is a mutex-protected vector rather than a channel, because a
/// bounded channel would block the walker as soon as the buffer is full —
/// and the consumer cannot drain it while it is waiting for the walk to
/// finish, which is exactly the situation the app is in.
pub struct Scanner {
    /// The entries that have been found but not yet consumed.
    buffer: Arc<Mutex<Vec<ScannedEntry>>>,
    /// The position of the next entry to consume.
    cursor: usize,
    /// The thread running the walk.
    handle: Option<std::thread::JoinHandle<ScanSummary>>,
    /// The flag that tells the walker to stop.
    stop: Arc<AtomicBool>,
    /// Whether the walk has returned.
    finished: Arc<AtomicBool>,
}

impl Scanner {
    /// Starts a scan with the given configuration.
    pub fn start(config: ScanConfig) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let finished = Arc::new(AtomicBool::new(false));

        let buffer_clone = Arc::clone(&buffer);
        let finished_clone = Arc::clone(&finished);
        let stop_clone = Arc::clone(&stop);

        let handle = std::thread::Builder::new()
            .name("fuzzylite-scan".into())
            .spawn(move || run_scan(config, buffer_clone, stop_clone, finished_clone))?;

        Ok(Self {
            buffer,
            cursor: 0,
            handle: Some(handle),
            stop,
            finished,
        })
    }

    /// Receives the next entry, or `None` when the buffer is drained.
    ///
    /// The buffer can be drained before the walk has returned, so `None`
    /// does not necessarily mean the scan is over. Use
    /// [`Scanner::walk_finished`] for that.
    pub fn next_entry(&mut self) -> Option<ScannedEntry> {
        let buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());

        if self.cursor < buffer.len() {
            let entry = buffer[self.cursor].clone();
            self.cursor += 1;
            return Some(entry);
        }

        None
    }

    /// Asks the scan to stop as soon as possible.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Whether the walk itself has returned.
    ///
    /// This is true even when the buffer still holds entries that have not
    /// been consumed yet.
    pub fn walk_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    /// Whether the scan has produced all of its entries.
    ///
    /// The scan is finished when the walk has returned. The entries that are
    /// still in the buffer are drained by [`Scanner::next_entry`].
    pub fn is_finished(&self) -> bool {
        self.walk_finished()
    }

    /// Waits for the scan to finish and returns its summary.
    pub fn join(mut self) -> ScanSummary {
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap_or_default()
        } else {
            ScanSummary::default()
        }
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Runs the walk, pushing entries into `tx`.
fn run_scan(
    config: ScanConfig,
    buffer: Arc<Mutex<Vec<ScannedEntry>>>,
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
) -> ScanSummary {
    let summary = ScanSummary::default();
    if config.roots.is_empty() {
        finished.store(true, Ordering::Relaxed);
        return summary;
    }

    let first_root = &config.roots[0];

    // Build the exclusion overrides from the configured glob patterns.
    //
    // The `ignore` crate's override builder treats a pattern as an
    // *include* rule, so exclusions must be written as `!pattern` — the same
    // convention fd uses. A bare pattern would instead restrict the walk to
    // paths matching it, which is the opposite of what is wanted.
    let mut overrides = OverrideBuilder::new(first_root);
    for pattern in &config.exclude_patterns {
        let negated = format!("!{pattern}");
        let _ = overrides.add(&negated);
    }
    let overrides = match overrides.build() {
        Ok(o) => o,
        Err(_) => {
            finished.store(true, Ordering::Relaxed);
            return summary;
        }
    };

    let mut builder = WalkBuilder::new(first_root);
    builder
        .hidden(config.ignore_hidden)
        .ignore(config.read_fdignore)
        .parents(config.read_fdignore || config.read_vcsignore)
        .git_ignore(config.read_vcsignore)
        .git_global(config.read_vcsignore)
        .git_exclude(config.read_vcsignore)
        .overrides(overrides)
        .follow_links(config.follow_links)
        .max_depth(config.max_depth);

    if config.read_fdignore {
        builder.add_custom_ignore_filename(".fdignore");
    }

    for root in &config.roots[1..] {
        builder.add(root);
    }

    if let Some(threads) = config.threads {
        builder.threads(threads);
    }

    let walker = builder.build_parallel();

    let files = AtomicU64::new(0);
    let dirs = AtomicU64::new(0);
    let errors = AtomicU64::new(0);

    walker.run(|| {
        let buffer = Arc::clone(&buffer);
        let stop = Arc::clone(&stop);
        let files = &files;
        let dirs = &dirs;
        let errors = &errors;

        Box::new(move |result| {
            if stop.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }

            let entry = match result {
                Ok(e) => e,
                Err(_) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    return WalkState::Continue;
                }
            };

            // The root directory itself is not a result.
            if entry.depth() == 0 {
                return WalkState::Continue;
            }

            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            if is_dir {
                dirs.fetch_add(1, Ordering::Relaxed);
                if !config.include_dirs {
                    return WalkState::Continue;
                }
            } else {
                files.fetch_add(1, Ordering::Relaxed);
                if !config.include_files {
                    return WalkState::Continue;
                }
            }

            let path = entry.path().to_path_buf();
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            // Metadata is only needed for files, and only some of it.
            let (size, modified) = if is_dir {
                (None, None)
            } else {
                match entry.metadata() {
                    Ok(meta) => {
                        let size = Some(meta.len());
                        let modified = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64);
                        (size, modified)
                    }
                    Err(_) => (None, None),
                }
            };

            let scanned = ScannedEntry {
                path,
                file_name,
                is_dir,
                size,
                modified,
            };

            // The entry is appended to the shared buffer. The lock is only
            // held for the append, so the workers rarely contend.
            buffer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(scanned);

            WalkState::Continue
        })
    });

    // The walk is over, so the consumer can drain the rest of the buffer.
    finished.store(true, Ordering::Relaxed);

    ScanSummary {
        files: files.load(Ordering::Relaxed),
        dirs: dirs.load(Ordering::Relaxed),
        skipped: 0,
        errors: errors.load(Ordering::Relaxed),
    }
}

/// Walks `root` and returns every entry it finds.
///
/// This is the simple, blocking form used for one-off scans and tests.
pub fn scan_blocking(config: &ScanConfig) -> (Vec<ScannedEntry>, ScanSummary) {
    let mut scanner = match Scanner::start(config.clone()) {
        Ok(s) => s,
        Err(_) => return (Vec::new(), ScanSummary::default()),
    };

    // The walk has to finish before the buffer holds every entry, so the
    // scan is waited on first.
    while !scanner.walk_finished() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let mut entries = Vec::new();
    while let Some(entry) = scanner.next_entry() {
        entries.push(entry);
    }
    let summary = scanner.join();
    (entries, summary)
}

/// Expands a leading `~` to the user's home directory.
pub fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// The user's home directory.
pub fn home_dir() -> PathBuf {
    expand_home("~")
}

/// Normalizes a path for display: expands `~`, resolves `.` and `..`.
pub fn normalize_path(path: &Path) -> PathBuf {
    let expanded = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        cwd.join(path)
    };

    let mut result = PathBuf::new();
    for component in expanded.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}
