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
use std::time::{Duration, Instant};

use ignore::overrides::OverrideBuilder;
use ignore::{WalkBuilder, WalkState};

/// Resolves a configured root to the path the filesystem reports.
///
/// The watcher reports real paths: on macOS a change under `/var` arrives
/// as `/private/var`, because `/var` is a symbolic link. A root kept as it
/// was typed would then never be the prefix of anything the watcher said,
/// and every reported change would be dropped as being outside the roots.
///
/// A root that does not exist is left as it is, so that the error surfaces
/// where the scan reports it rather than here.
pub fn resolve_root_path(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

/// The rules that decide what the index may hold.
///
/// The scan applies them through the walker. The watcher does not: it is
/// given the roots and nothing else, so it reports every change under
/// them, including the trees the walker was told to skip. Applying those
/// reports unfiltered put `Library`, `.git` and every other excluded tree
/// into the index a few entries at a time, for as long as the app ran.
///
/// Only the exclusion patterns and the hidden-file rule are applied here.
/// The walker also honours `.gitignore` and friends, which depend on the
/// ignore files found along a path; those are not consulted for a reported
/// change, so a directory that is only gitignored can still reach the
/// index this way.
#[derive(Debug)]
pub struct ChangeFilter {
    /// The roots a change has to be under, as the filesystem reports them.
    roots: Vec<PathBuf>,
    /// The compiled exclusion patterns, if any could be compiled.
    overrides: Option<ignore::overrides::Override>,
    /// Whether to skip paths with a leading dot, as the walker does.
    ignore_hidden: bool,
}

impl ChangeFilter {
    /// Compiles the rules once, for a batch of changes.
    pub fn new(roots: &[PathBuf], exclude_patterns: &[String], ignore_hidden: bool) -> Self {
        let overrides = roots.first().and_then(|root| {
            let mut builder = OverrideBuilder::new(root);

            // The override builder reads a pattern as an *include* rule,
            // so an exclusion is written `!pattern` — the convention fd
            // uses, and the one the walker is given a few lines below.
            for pattern in exclude_patterns {
                let _ = builder.add(&format!("!{pattern}"));
            }

            builder.build().ok()
        });

        Self {
            roots: roots.to_vec(),
            overrides,
            ignore_hidden,
        }
    }

    /// Whether a reported change should be left out of the index.
    pub fn skips(&self, path: &Path, is_dir: bool) -> bool {
        // A path outside every root has nowhere to hang off, so there is
        // nothing the index could do with it.
        let Some(below) = self
            .roots
            .iter()
            .find_map(|root| path.strip_prefix(root).ok())
        else {
            return true;
        };

        // Only the part below the root is judged. A root is allowed to be
        // a hidden directory itself — someone may index `~/.config` — and
        // the patterns are written relative to the root as well.
        //
        // Every ancestor is judged in turn, not just the path itself. The
        // walker excludes a directory by never descending into it, so a
        // pattern naming a directory never matches the paths underneath:
        // `Library` matches `Library`, and nothing about
        // `Library/Caches/junk.bin`. Testing each ancestor is what pruning
        // the tree amounts to.
        let mut ancestor = PathBuf::new();
        let mut components = below.components().peekable();

        while let Some(component) = components.next() {
            let std::path::Component::Normal(part) = component else {
                continue;
            };

            if self.ignore_hidden && part.to_string_lossy().starts_with('.') {
                return true;
            }

            ancestor.push(part);

            // Everything above the path itself is a directory, whatever
            // the change said the path at the end of it was.
            let is_dir = if components.peek().is_some() { true } else { is_dir };

            if self
                .overrides
                .as_ref()
                .is_some_and(|overrides| overrides.matched(&ancestor, is_dir).is_ignore())
            {
                return true;
            }
        }

        false
    }
}

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
    /// Whether to stay on the filesystem each root lives on.
    ///
    /// A launcher indexes the user's own disk. Whatever is mounted inside
    /// it — a network share, a disk image, a removable drive, a file
    /// provider's volume — is not part of that, and descending into one is
    /// how the index got wedged: `opendir()` on a dead NFS mount blocked
    /// in the kernel for fifty-four minutes. `fd` spells this
    /// `--one-file-system`; for something that runs all day it is the
    /// default.
    pub same_file_system: bool,
    /// Maximum directory depth to descend into.
    pub max_depth: Option<usize>,
    /// Number of worker threads. `None` uses one thread per core.
    pub threads: Option<usize>,
    /// Whether to include directories in the results.
    pub include_dirs: bool,
    /// Whether to include regular files in the results.
    pub include_files: bool,
    /// How long the walk is given before the scan reports itself finished.
    ///
    /// There is no way to cancel a thread blocked in `opendir()`, so the
    /// deadline does not stop the walk — it stops the *index* waiting for
    /// it. What has been collected by then is published, and the stuck
    /// thread is left to unwind on its own. `None` waits for ever.
    pub timeout: Option<Duration>,
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
            same_file_system: true,
            max_depth: None,
            threads: None,
            include_dirs: true,
            include_files: true,
            timeout: Some(Duration::from_secs(60)),
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
    /// The thread running the walk.
    handle: Option<std::thread::JoinHandle<ScanSummary>>,
    /// The flag that tells the walker to stop.
    stop: Arc<AtomicBool>,
    /// Whether the walk has returned.
    finished: Arc<AtomicBool>,
    /// When the index stops waiting for the walk.
    deadline: Option<Instant>,
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

        let deadline = config.timeout.map(|t| Instant::now() + t);

        let handle = std::thread::Builder::new()
            .name("seek-scan".into())
            .spawn(move || run_scan(config, buffer_clone, stop_clone, finished_clone))?;

        Ok(Self {
            buffer,
            handle: Some(handle),
            stop,
            finished,
            deadline,
        })
    }

    /// Moves every entry found so far into `out`, returning how many.
    ///
    /// The entries are *moved*. The previous version handed them out one at
    /// a time by cloning, leaving the originals in the buffer, so a quarter
    /// of a million entries existed twice at the moment the index was
    /// built — 44MB of peak on a home directory, measured by
    /// `examples/mem.rs`. When the caller's vector is empty the buffer's
    /// own allocation becomes the caller's and nothing is copied at all.
    ///
    /// The buffer can be empty before the walk has returned, so a zero
    /// does not mean the scan is over. Use [`Scanner::walk_finished`] for
    /// that.
    pub fn drain_into(&mut self, out: &mut Vec<ScannedEntry>) -> usize {
        let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());

        let taken = std::mem::take(&mut *buffer);
        let count = taken.len();

        if out.is_empty() {
            *out = taken;
        } else {
            out.extend(taken);
        }

        count
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

    /// Whether the walk has been given longer than its deadline.
    pub fn timed_out(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// Whether the consumer should stop waiting for the scan.
    ///
    /// Either the walk returned, or it was given long enough. The entries
    /// that are in the buffer are drained by [`Scanner::drain_into`]
    /// either way; a timed-out scan simply has fewer of them.
    pub fn is_finished(&self) -> bool {
        self.walk_finished() || self.timed_out()
    }

    /// Stops waiting for the scan and returns its summary.
    ///
    /// The walk is joined only when it has already returned. A walk that
    /// has not is asked to stop and detached, because a thread blocked in
    /// `opendir()` cannot be cancelled and joining it is what wedged the
    /// index for fifty-four minutes. Dropping its `JoinHandle` detaches
    /// it; it exits when the kernel lets go.
    pub fn finish(mut self) -> ScanSummary {
        if !self.walk_finished() {
            self.stop();
            return ScanSummary::default();
        }

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
        .same_file_system(config.same_file_system)
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
    while scanner.drain_into(&mut entries) > 0 {}
    let summary = scanner.finish();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh root under the temp directory, named after the test.
    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "seek-drain-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), b"x").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.txt"), b"x").unwrap();
        root.canonicalize().unwrap()
    }

    fn finished_scanner(root: PathBuf) -> Scanner {
        let config = ScanConfig {
            roots: vec![root],
            ..Default::default()
        };
        let scanner = Scanner::start(config).unwrap();
        while !scanner.walk_finished() {
            std::thread::sleep(Duration::from_millis(5));
        }
        scanner
    }

    /// The entries must be *moved* out of the buffer.
    ///
    /// Handing them out by cloning and leaving the originals behind meant a
    /// quarter of a million entries existed twice at the moment the index
    /// was built — 44MB of peak on a home directory.
    #[test]
    fn draining_leaves_nothing_behind() {
        let root = temp_root("moves");
        let mut scanner = finished_scanner(root.clone());

        let mut entries = Vec::new();
        let count = scanner.drain_into(&mut entries);

        assert_eq!(count, entries.len());
        assert!(count >= 3, "the fixture has three entries: {count}");

        let held = scanner.buffer.lock().unwrap().len();
        assert_eq!(held, 0, "the buffer still holds {held} entries");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn draining_twice_yields_nothing_the_second_time() {
        let root = temp_root("twice");
        let mut scanner = finished_scanner(root.clone());

        let mut entries = Vec::new();
        let first = scanner.drain_into(&mut entries);
        let before = entries.len();
        let second = scanner.drain_into(&mut entries);

        assert!(first > 0);
        assert_eq!(second, 0);
        assert_eq!(entries.len(), before, "nothing was handed out twice");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A second drain appends rather than replacing, because a timed-out
    /// scan is drained in more than one go.
    #[test]
    fn draining_appends_to_what_is_already_there() {
        let root = temp_root("appends");
        let mut scanner = finished_scanner(root.clone());

        let mut entries = vec![ScannedEntry {
            path: PathBuf::from("/sentinel"),
            file_name: "sentinel".into(),
            is_dir: false,
            size: None,
            modified: None,
        }];
        let count = scanner.drain_into(&mut entries);

        assert_eq!(entries.len(), count + 1);
        assert_eq!(entries[0].file_name, "sentinel", "the sentinel survived");

        let _ = std::fs::remove_dir_all(&root);
    }
}
