//! The C ABI that the Swift front end calls.
//!
//! The Swift side talks to this library through a plain C interface so that
//! the Rust core can be compiled as a static library and linked into the app
//! bundle. The interface is deliberately small: the app owns the UI, the
//! core owns the index and the search.
//!
//! # Ownership
//!
//! Every `const char *` returned by this module is owned by the Rust side
//! and stays valid until the next call that returns a string. The Swift side
//! copies it into a Swift `String` immediately.
//!
//! # Threading
//!
//! All functions are safe to call from any thread. The engine keeps its
//! state behind a mutex, so a search on the main thread cannot observe a
//! half-applied index update.

use std::ffi::{c_char, c_int, CStr, CString};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::corpus::Corpus;
use crate::fuzzy::Scheme;
use crate::index::{EntryKind, Index};
use crate::query::{Query, QueryError, SearchOptions};
use crate::scan::{ScanConfig, Scanner};
use crate::search::search;

/// The opaque engine handle.
///
/// The whole state is guarded by a single reader-writer lock. Searches take
/// the read lock, so they can run in parallel with each other, while the
/// operations that change the index — rebuilding it after a scan, loading a
/// snapshot — take the write lock. Without the lock, a search running while
/// the index is being rebuilt would read a half-written corpus.
pub struct Engine {
    /// The state of the engine.
    inner: RwLock<Inner>,
}

/// The state that the lock guards.
pub struct Inner {
    /// The index of the filesystem.
    pub index: Index,
    /// The searchable corpus, rebuilt whenever the index changes.
    pub corpus: Corpus,
    /// The roots that are watched.
    pub roots: Vec<std::path::PathBuf>,
    /// The options used for searches.
    pub options: SearchOptions,
    /// A background scanner, when one is running.
    pub scanner: Option<Scanner>,
}

impl Engine {
    fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                index: Index::new(),
                corpus: Corpus::default(),
                roots: Vec::new(),
                options: SearchOptions::default(),
                scanner: None,
            }),
        }
    }

    /// Takes the read lock, recovering from a poisoned lock.
    ///
    /// A panic while the lock was held would poison it, but the state behind
    /// it is still consistent, so the lock is taken anyway rather than
    /// bringing the whole app down.
    pub fn read(&self) -> RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes the write lock, recovering from a poisoned lock.
    pub fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

impl Inner {
    /// Rebuilds the index from a list of scanned entries.
    fn rebuild_index(&mut self, entries: Vec<crate::scan::ScannedEntry>) {
        let mut index = Index::new();
        let mut dir_ids: std::collections::HashMap<std::path::PathBuf, u32> =
            std::collections::HashMap::new();

        for root in &self.roots {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| root.to_string_lossy().to_string());
            let id = index.add_named(None, &name, EntryKind::Directory, 0, 0);
            dir_ids.insert(root.clone(), id);
            index.roots.push(id);
            index.config_roots.push(root.clone());
        }

        // Sort by depth so that a directory always exists before its children.
        let mut sorted = entries;
        sorted.sort_by_key(|e| e.path.components().count());

        for entry in &sorted {
            let parent_path = entry
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
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
        index.touch();

        self.corpus = Corpus::build(&index);
        self.index = index;
    }
}

/// A search result returned to Swift.
///
/// The struct is passed by pointer and filled in by the core.
#[repr(C)]
pub struct SearchResultEntry {
    /// The index entry id.
    pub id: u32,
    /// The match score.
    pub score: i32,
    /// The byte offset of the first matched character.
    pub start: i32,
    /// The byte offset just past the last matched character.
    pub end: i32,
}

/// Creates a new engine.
///
/// Returns null on failure.
#[no_mangle]
pub extern "C" fn fl_engine_new() -> *mut Engine {
    Box::into_raw(Box::new(Engine::new()))
}

/// Destroys an engine.
///
/// # Safety
///
/// `engine` must have been returned by [`fl_engine_new`] and must not be
/// used afterwards.
#[no_mangle]
pub unsafe extern "C" fn fl_engine_free(engine: *mut Engine) {
    if !engine.is_null() {
        drop(Box::from_raw(engine));
    }
}

/// Sets the roots to index.
///
/// The paths are comma-separated. A leading `~` is expanded.
///
/// # Safety
///
/// `roots` must be a valid null-terminated string.
#[no_mangle]
pub unsafe extern "C" fn fl_engine_set_roots(engine: *mut Engine, roots: *const c_char) -> c_int {
    if engine.is_null() || roots.is_null() {
        return -1;
    }
    let Ok(text) = CStr::from_ptr(roots).to_str() else {
        return -1;
    };

    let paths: Vec<std::path::PathBuf> = text
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(crate::scan::expand_home)
        .collect();

    (*engine).write().roots = paths;
    0
}

/// Sets the exclusion patterns.
///
/// The patterns are comma-separated globs, matched the way fd's `--exclude`
/// works.
///
/// # Safety
///
/// `patterns` must be a valid null-terminated string.
#[no_mangle]
pub unsafe extern "C" fn fl_engine_set_excludes(
    engine: *mut Engine,
    patterns: *const c_char,
) -> c_int {
    if engine.is_null() || patterns.is_null() {
        return -1;
    }
    let Ok(text) = CStr::from_ptr(patterns).to_str() else {
        return -1;
    };
    let list: Vec<String> = text
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (*engine).write().options.exclude_patterns = list;
    0
}

/// Sets whether matching is case sensitive.
///
/// Returns 0 on success, -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn fl_engine_set_case_sensitive(
    engine: *mut Engine,
    case_sensitive: c_int,
) -> c_int {
    if engine.is_null() {
        return -1;
    }
    (*engine).write().options.case_sensitive = case_sensitive != 0;
    0
}

/// Sets the scoring scheme.
///
/// The scheme is 0 for the default scheme and 1 for the path scheme.
///
/// # Safety
///
/// `scheme` must be 0 or 1.
#[no_mangle]
pub unsafe extern "C" fn fl_engine_set_scheme(engine: *mut Engine, scheme: c_int) -> c_int {
    if engine.is_null() {
        return -1;
    }
    let selected = match scheme {
        0 => Scheme::Default,
        1 => Scheme::Path,
        _ => return -1,
    };
    (*engine).write().options.scheme = selected;
    0
}

/// Sets the maximum number of results a search returns.
///
/// Returns 0 on success, -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn fl_engine_set_limit(engine: *mut Engine, limit: c_int) -> c_int {
    if engine.is_null() || limit <= 0 {
        return -1;
    }
    (*engine).write().options.limit = limit as usize;
    0
}

/// Starts a background scan of the configured roots.
///
/// Returns 0 on success, -1 on failure.
#[no_mangle]
pub extern "C" fn fl_engine_start_scan(engine: *mut Engine) -> c_int {
    if engine.is_null() {
        return -1;
    }
    let eng = unsafe { &*engine };

    let config = {
        let inner = eng.read();
        if inner.roots.is_empty() {
            return -1;
        }

        ScanConfig {
            roots: inner.roots.clone(),
            exclude_patterns: inner.options.exclude_patterns.clone(),
            ..Default::default()
        }
    };

    let scanner = match Scanner::start(config) {
        Ok(s) => s,
        Err(_) => return -1,
    };

    eng.write().scanner = Some(scanner);
    0
}

/// Collects the results of a finished scan into the index.
///
/// Returns the number of entries in the index, or -1 on failure.
///
/// The scan is complete when [`fl_engine_scan_finished`] returns non-zero.
#[no_mangle]
pub extern "C" fn fl_engine_finish_scan(engine: *mut Engine) -> c_int {
    if engine.is_null() {
        return -1;
    }
    let eng = unsafe { &*engine };

    // The scanner is taken out of the lock, because draining it can take a
    // while and there is no reason to block the searches meanwhile.
    let Some(mut scanner) = eng.write().scanner.take() else {
        return 0;
    };

    // The buffer is drained until it is empty. The walk has already
    // returned, so nothing more will be added to it.
    let mut entries = Vec::new();
    while let Some(entry) = scanner.next_entry() {
        entries.push(entry);
    }
    let _summary = scanner.join();

    let mut inner = eng.write();
    inner.rebuild_index(entries);
    inner.index.len() as c_int
}

/// Whether the background scan has finished.
///
/// Returns 1 when it has, 0 when it is still running.
#[no_mangle]
pub extern "C" fn fl_engine_scan_finished(engine: *mut Engine) -> c_int {
    if engine.is_null() {
        return 0;
    }
    let eng = unsafe { &*engine };
    eng.read()
        .scanner
        .as_ref()
        .map(|s| {
            // The scan is finished when the walk has returned and every
            // entry has been consumed.
            if s.is_finished() {
                1
            } else {
                0
            }
        })
        .unwrap_or(1)
}

/// The number of entries in the index.
#[no_mangle]
pub extern "C" fn fl_engine_count(engine: *mut Engine) -> c_int {
    if engine.is_null() {
        return 0;
    }
    let eng = unsafe { &*engine };
    eng.read().index.len() as c_int
}

/// Runs a search and writes the results into a caller-provided buffer.
///
/// Returns the number of results written, or -1 on failure.
///
/// # Safety
///
/// `out` must point to a buffer of at least `capacity` entries, and
/// `query` must be a valid null-terminated string.
#[no_mangle]
pub unsafe extern "C" fn fl_engine_search(
    engine: *mut Engine,
    query: *const c_char,
    out: *mut SearchResultEntry,
    capacity: c_int,
) -> c_int {
    if engine.is_null() || query.is_null() {
        return -1;
    }
    let Ok(text) = CStr::from_ptr(query).to_str() else {
        return -1;
    };

    let parsed = match Query::parse(text) {
        Ok(q) => q,
        Err(QueryError::UnterminatedQuote) => {
            // A partially typed quote should still search for what is there.
            Query::parse(text.trim_end_matches('\'')).unwrap_or_default()
        }
        Err(_) => Query::default(),
    };

    let eng = unsafe { &*engine };
    let inner = eng.read();

    let outcome = search(&inner.corpus, &parsed, &inner.options);
    let count = outcome.results.len().min(capacity.max(0) as usize);

    for (i, result) in outcome.results.iter().take(count).enumerate() {
        *out.add(i) = SearchResultEntry {
            id: result.id,
            score: result.score,
            start: result.start,
            end: result.end,
        };
    }
    count as c_int
}

/// Returns the path of an entry as a newly allocated C string.
///
/// The caller owns the returned string and must free it with
/// [`fl_string_free`]. Returns null when the id is unknown.
///
/// # Safety
///
/// `id` must be a valid entry id.
#[no_mangle]
pub unsafe extern "C" fn fl_engine_path(engine: *mut Engine, id: u32) -> *mut c_char {
    if engine.is_null() {
        return std::ptr::null_mut();
    }
    let eng = unsafe { &*engine };
    let inner = eng.read();

    let Some(path) = inner.index.full_path(id) else {
        return std::ptr::null_mut();
    };
    match CString::new(path.to_string_lossy().as_bytes()) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Returns the matched byte offsets for a result.
///
/// The offsets are recomputed from the file name, which is cheap for the
/// handful of results that are on screen. Writes at most `capacity` offsets
/// and returns the total count.
///
/// # Safety
///
/// `out` must point to a buffer of at least `capacity` integers.
#[no_mangle]
pub unsafe extern "C" fn fl_engine_positions(
    engine: *mut Engine,
    id: u32,
    pattern: *const c_char,
    out: *mut i32,
    capacity: c_int,
) -> c_int {
    if engine.is_null() || pattern.is_null() {
        return 0;
    }
    let Ok(text) = CStr::from_ptr(pattern).to_str() else {
        return 0;
    };

    let eng = unsafe { &*engine };
    let inner = eng.read();

    let Some(entry) = inner.index.get(id) else {
        return 0;
    };
    let Some(name) = inner.index.names.get(entry.name) else {
        return 0;
    };

    let parsed = match Query::parse(text) {
        Ok(q) => q,
        Err(_) => return 0,
    };
    let Some(term) = parsed.positive.iter().find(|t| {
        matches!(
            t.kind,
            crate::query::TermKind::Fuzzy | crate::query::TermKind::Exact
        )
    }) else {
        return 0;
    };

    let matcher = crate::fuzzy::FuzzyMatcher::with_options(
        &term.value,
        inner.options.case_sensitive,
        inner.options.scheme,
        inner.options.normalize,
    );
    let (_, positions) = matcher.match_text_with_positions(name);

    let count = positions.len();
    let written = count.min(capacity.max(0) as usize);
    for (i, pos) in positions.iter().take(written).enumerate() {
        *out.add(i) = *pos;
    }
    count as c_int
}

/// Frees a string returned by this library.
///
/// # Safety
///
/// `s` must have been returned by this library and must not be used after.
#[no_mangle]
pub unsafe extern "C" fn fl_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// The version of the core library, as a static C string.
#[no_mangle]
pub extern "C" fn fl_version() -> *const c_char {
    // "0.1.0\0"
    b"0.1.0\0".as_ptr() as *const c_char
}
