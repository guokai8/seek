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
//! state behind a reader-writer lock: the searches take the read lock, and
//! the operations that change the index take the write lock. A search that
//! runs while the index is being rebuilt cannot observe a half-applied
//! update.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::corpus::Corpus;
use crate::fuzzy::Scheme;
use crate::index::{EntryKind, Index};
use crate::query::{narrows, Matching, Query, SearchOptions, SearchResult};
use crate::scan::{ScanConfig, Scanner};
use crate::fuzzy::Scratch;
use crate::search::{search_streaming, Progress, Scorer};

/// The opaque engine handle.
///
/// The state is split in two, and the split is what keeps the panel
/// responsive.
///
/// *Maintenance* state — the index, the scanner, the watchers — lives behind
/// `inner`. Rebuilding the index after a scan and applying the watcher's
/// changes take its write lock, and they hold it for as long as the work
/// takes: a corpus rebuild walks every entry in the index.
///
/// *Searchable* state is published separately, as an immutable [`Corpus`]
/// behind an `Arc`. A search clones the pointer — which takes as long as a
/// pointer copy — and then does all of its work outside every lock.
///
/// The two must not share a lock. `std::sync::RwLock` prefers writers, so a
/// single queued writer blocks every reader behind it; with one lock, the
/// watcher taking it every few hundred milliseconds was enough to stall
/// each keystroke for seconds, and the panel showed nothing. Publishing the
/// corpus instead means index maintenance and searching never wait for each
/// other. `core/tests/search_under_maintenance.rs` holds the maintenance
/// lock and asserts that a search still answers.
pub struct Engine {
    /// The state that changing the index needs.
    inner: RwLock<Inner>,
    /// The published searchable snapshot, and the number that names it.
    ///
    /// The lock guards the pointer, not the corpus: it is held just long
    /// enough to clone or replace an `Arc`. The number is bumped on every
    /// publish, so that something holding corpus positions can tell
    /// whether they still mean what they meant.
    corpus: RwLock<(u64, Arc<Corpus>)>,
    /// The options a search runs with, and the number that names them.
    ///
    /// These are small and are read on every keystroke, so they are kept
    /// out of `inner` for the same reason the corpus is.
    options: RwLock<(u64, SearchOptions)>,
    /// The previous query's match set.
    ///
    /// This is what makes typing get faster rather than slower: a query
    /// that extends the one before it can only match paths the previous
    /// one matched, so it is run over that set instead of the whole
    /// corpus. See [`narrows`] for when that is allowed, and
    /// [`crate::search::search_in`] for what is done with it.
    previous: RwLock<Option<Narrowing>>,
}

/// What a query matched, kept so the next query can narrow from it.
///
/// One of these is kept, not a history: a keystroke extends the query
/// before it, so one step back is the step that pays. It costs four bytes
/// per matching path — at most the corpus size, under a megabyte over a
/// home directory — and it is replaced on every search.
struct Narrowing {
    /// The query the pool came from.
    query: Query,
    /// The corpus positions it matched, ascending.
    pool: Arc<Vec<u32>>,
    /// What the pool was computed against.
    ///
    /// A position is an offset into one particular snapshot, and a match
    /// is a fact about one particular set of matching rules. Either
    /// changing makes the pool a description of something that no longer
    /// exists, and the symptom would not be an error — it would be a row
    /// that quietly stopped appearing.
    stamp: Stamp,
}

/// Identifies the corpus and the options a search ran against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    /// The published corpus.
    corpus: u64,
    /// The search options.
    options: u64,
}

/// Everything a keystroke needs, taken in one go.
///
/// The corpus, the options and the stamp have to be read together: taken
/// separately, a publish landing between two of them would hand back a
/// stamp that names a different corpus from the one being searched.
struct Searchable {
    corpus: Arc<Corpus>,
    options: SearchOptions,
    stamp: Stamp,
}

/// The state that the maintenance lock guards.
pub struct Inner {
    /// The index of the filesystem.
    pub index: Index,
    /// The roots that are watched.
    pub roots: Vec<std::path::PathBuf>,
    /// A background scanner, when one is running.
    pub scanner: Option<Scanner>,
    /// The filesystem watchers, one per root.
    pub watchers: Vec<crate::watch::Watcher>,
    /// Whether the last scan walked everything it was asked to.
    ///
    /// A scan that hit its deadline publishes what it had, which is a
    /// partial index. The caller is told, because a panel that reports a
    /// partial index as a complete one misleads in the same way the
    /// permanent "Indexing…" did.
    pub scan_complete: bool,
}

impl Engine {
    fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                index: Index::new(),
                roots: Vec::new(),
                scanner: None,
                watchers: Vec::new(),
                scan_complete: true,
            }),
            corpus: RwLock::new((0, Arc::new(Corpus::default()))),
            options: RwLock::new((0, SearchOptions::default())),
            previous: RwLock::new(None),
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

    /// The published searchable snapshot.
    ///
    /// The returned corpus is immutable and keeps itself alive, so the
    /// caller can search it for as long as it likes while the index is
    /// rebuilt underneath. A search that was already running simply
    /// finishes against the snapshot it started on.
    pub fn corpus(&self) -> Arc<Corpus> {
        Arc::clone(&self.corpus.read().unwrap_or_else(|e| e.into_inner()).1)
    }

    /// The options a search runs with.
    pub fn options(&self) -> SearchOptions {
        self.options
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .1
            .clone()
    }

    /// Changes the search options.
    ///
    /// The version is bumped whether or not the change altered anything,
    /// because the caller is the one who knows: a setter that compared
    /// would have to compare the whole of `SearchOptions`, and getting
    /// that comparison subtly wrong means a pool kept past the rules that
    /// produced it.
    pub fn with_options(&self, change: impl FnOnce(&mut SearchOptions)) {
        let mut options = self.options.write().unwrap_or_else(|e| e.into_inner());
        options.0 = options.0.wrapping_add(1);
        change(&mut options.1);
    }

    /// Takes the corpus, the options and the stamp that names them.
    fn searchable(&self) -> Searchable {
        let corpus = self.corpus.read().unwrap_or_else(|e| e.into_inner());
        let options = self.options.read().unwrap_or_else(|e| e.into_inner());

        Searchable {
            corpus: Arc::clone(&corpus.1),
            options: options.1.clone(),
            stamp: Stamp {
                corpus: corpus.0,
                options: options.0,
            },
        }
    }

    /// The previous query's pool, when it can answer `query`.
    ///
    /// Three things have to hold: the pool was computed against this
    /// corpus, under these options, and by a query whose matches contain
    /// this one's.
    fn pool_for(&self, query: &Query, stamp: Stamp) -> Option<Arc<Vec<u32>>> {
        let previous = self.previous.read().unwrap_or_else(|e| e.into_inner());
        let previous = previous.as_ref()?;

        if previous.stamp != stamp || !narrows(&previous.query, query) {
            return None;
        }

        Some(Arc::clone(&previous.pool))
    }

    /// Remembers what `query` matched.
    ///
    /// A `pool` of `None` means the search did not produce one worth
    /// keeping, and then nothing is remembered: the next keystroke
    /// searches the corpus, which is what a pool that large would have
    /// amounted to. The old entry is cleared rather than left in place,
    /// because it describes a query two keystrokes back and the rule
    /// would have to hold for the pair that was never searched together.
    fn remember(&self, query: Query, pool: Option<Vec<u32>>, stamp: Stamp) {
        let narrowing = pool.map(|pool| Narrowing {
            query,
            pool: Arc::new(pool),
            stamp,
        });

        *self.previous.write().unwrap_or_else(|e| e.into_inner()) = narrowing;
    }

    /// Replaces the published corpus and names the new one.
    fn set_corpus(&self, corpus: Arc<Corpus>) {
        let mut published = self.corpus.write().unwrap_or_else(|e| e.into_inner());
        published.0 = published.0.wrapping_add(1);
        published.1 = corpus;
    }

    /// Publishes a new snapshot of the index.
    ///
    /// This is the only way the searchable state changes. It is called
    /// after every change to the index, with the maintenance lock still
    /// held, so that the snapshot and the index agree.
    pub fn publish(&self, index: &mut Index) {
        self.set_corpus(Arc::new(Corpus::build(index)));

        // The whole index is in the snapshot, so nothing is left pending.
        // A rebuild records every entry it added, and letting that list
        // reach the next watcher batch would make it rebuild the lot.
        index.take_dirty();
    }

    /// Publishes a snapshot that differs from the current one in `changed`.
    ///
    /// This is the watcher's path. A batch touches a handful of entries,
    /// and [`Corpus::build`] would materialise every path in the index for
    /// them — several times a second, for as long as the app is running.
    /// The new snapshot carries over the paths that did not change instead.
    ///
    /// The current snapshot is cloned out before the new one is built, so
    /// the searches running against it are not held up by the write lock.
    pub fn publish_changed(&self, index: &Index, changed: &[crate::index::EntryId]) {
        let current = Arc::clone(&self.corpus.read().unwrap_or_else(|e| e.into_inner()).1);
        self.set_corpus(Arc::new(current.updated(index, changed)));
    }
}

impl Inner {
    /// Rebuilds the index from a list of scanned entries.
    fn rebuild_index(&mut self, entries: Vec<crate::scan::ScannedEntry>) {
        // The index being replaced is freed before its successor is built.
        // Keeping both alive is 13.5MB of peak on a home directory, and
        // nothing can search the old one anyway: the caller holds the
        // maintenance lock for the whole of this.
        self.index = Index::new();

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

        // Sort by depth so that a directory always exists before its
        // children. The key is cached: counting the components of a path
        // on every comparison cost 200ms over a quarter of a million
        // entries, for a number that does not change.
        let mut sorted = entries;
        sorted.sort_by_cached_key(|e| e.path.components().count());

        // The entries are consumed rather than borrowed, so each one's two
        // heap allocations are freed as it is inserted instead of all of
        // them staying alive until the index is complete — 28MB of peak on
        // a home directory. A directory's path is moved into `dir_ids`
        // rather than cloned.
        for entry in sorted {
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
                dir_ids.insert(entry.path, id);
            }
        }
        index.touch();

        // The searchable snapshot is published by the caller, which still
        // holds the maintenance lock. See [`Engine::publish`].
        self.index = index;
    }
}

/// A search result returned to Swift.
///
/// The struct is passed by pointer and filled in by the core.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchResultEntry {
    /// The index entry id.
    pub id: u32,
    /// The match score.
    pub score: i32,
    /// The byte offset of the first matched character.
    pub start: i32,
    /// The byte offset just past the last matched character.
    pub end: i32,
    /// Whether the entry is a directory.
    ///
    /// The index already knows this, so it is reported here rather than
    /// being looked up on the filesystem once per result. The lookup is
    /// a stat call, and doing five hundred of them costs more than the
    /// search itself.
    pub is_dir: u8,
    /// The file size in bytes, as the last scan saw it.
    pub size: u64,
    /// The modification time, as a Unix timestamp in seconds.
    pub modified: i64,
}

/// Creates a new engine.
///
/// Returns null on failure.
#[no_mangle]
pub extern "C" fn sk_engine_new() -> *mut Engine {
    Box::into_raw(Box::new(Engine::new()))
}

/// Destroys an engine.
///
/// # Safety
///
/// `engine` must have been returned by [`sk_engine_new`] and must not be
/// used afterwards.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_free(engine: *mut Engine) {
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
pub unsafe extern "C" fn sk_engine_set_roots(engine: *mut Engine, roots: *const c_char) -> c_int {
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
        .map(crate::scan::resolve_root_path)
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
pub unsafe extern "C" fn sk_engine_set_excludes(
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
    (*engine).with_options(|options| options.exclude_patterns = list);
    0
}

/// Sets whether matching is case sensitive.
///
/// Returns 0 on success, -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_set_case_sensitive(
    engine: *mut Engine,
    case_sensitive: c_int,
) -> c_int {
    if engine.is_null() {
        return -1;
    }
    (*engine).with_options(|options| options.case_sensitive = case_sensitive != 0);
    0
}

/// Sets what a bare term means: 1 for an exact substring, 0 for a fuzzy
/// subsequence. This is fzf's `--exact`.
///
/// Changing it bumps the options stamp, which is what drops the previous
/// query's match set: the same text means something else now, so the set
/// it matched cannot be narrowed from.
///
/// Returns 0 on success, -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_set_exact_match(
    engine: *mut Engine,
    exact: c_int,
) -> c_int {
    if engine.is_null() {
        return -1;
    }
    (*engine).with_options(|options| {
        options.matching = if exact != 0 {
            Matching::Exact
        } else {
            Matching::Fuzzy
        };
    });
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
pub unsafe extern "C" fn sk_engine_set_scheme(engine: *mut Engine, scheme: c_int) -> c_int {
    if engine.is_null() {
        return -1;
    }
    let selected = match scheme {
        0 => Scheme::Default,
        1 => Scheme::Path,
        _ => return -1,
    };
    // The tiebreak chain travels with the scheme: fzf's `--scheme=path`
    // also sets `--tiebreak=pathname,length`, and choosing one without the
    // other gives an ordering that is neither of fzf's.
    (*engine).with_options(|options| {
        options.scheme = selected;
        options.criteria = selected.criteria().to_vec();
    });
    0
}

/// Sets whether accented Latin letters are folded to their base letter.
///
/// Returns 0 on success, -1 on failure.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_set_normalize(
    engine: *mut Engine,
    normalize: c_int,
) -> c_int {
    if engine.is_null() {
        return -1;
    }
    (*engine).with_options(|options| options.normalize = normalize != 0);
    0
}

/// Starts a background scan of the configured roots.
///
/// Returns 0 on success, -1 on failure.
#[no_mangle]
pub extern "C" fn sk_engine_start_scan(engine: *mut Engine) -> c_int {
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
            exclude_patterns: eng.options().exclude_patterns,
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
/// The scan is complete when [`sk_engine_scan_finished`] returns non-zero.
#[no_mangle]
pub extern "C" fn sk_engine_finish_scan(engine: *mut Engine) -> c_int {
    if engine.is_null() {
        return -1;
    }
    let eng = unsafe { &*engine };

    // The scanner is taken out of the lock, because draining it can take a
    // while and there is no reason to block the searches meanwhile.
    let Some(mut scanner) = eng.write().scanner.take() else {
        return 0;
    };

    // The buffer is drained until it is empty. When the walk returned,
    // that is everything it found; when the scan hit its deadline instead,
    // it is everything it had found by then — the walker is still pushing
    // into the buffer and whatever arrives after this point is lost, which
    // is the price of not waiting for a thread stuck in `opendir()`.
    let complete = scanner.walk_finished();

    let mut entries = Vec::new();
    while scanner.drain_into(&mut entries) > 0 {}
    let _summary = scanner.finish();

    let mut inner = eng.write();
    inner.scan_complete = complete;
    inner.rebuild_index(entries);
    eng.publish(&mut inner.index);
    inner.index.len() as c_int
}

/// Whether the background scan has finished.
///
/// Returns 1 when it has, 0 when it is still running.
#[no_mangle]
pub extern "C" fn sk_engine_scan_finished(engine: *mut Engine) -> c_int {
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

/// Whether the last scan walked everything it was asked to.
///
/// Returns 1 when it did, 0 when it was cut short by its deadline and the
/// index is therefore partial.
#[no_mangle]
pub extern "C" fn sk_engine_scan_complete(engine: *mut Engine) -> c_int {
    if engine.is_null() {
        return 0;
    }
    let eng = unsafe { &*engine };
    c_int::from(eng.read().scan_complete)
}

/// The number of entries in the index.
#[no_mangle]
pub extern "C" fn sk_engine_count(engine: *mut Engine) -> c_int {
    if engine.is_null() {
        return 0;
    }
    let eng = unsafe { &*engine };
    eng.read().index.len() as c_int
}

/// Explains why a query does not parse, for the panel to show.
///
/// Returns 0 when the query is fine. Otherwise it writes the reason into
/// `out` as a null-terminated string and returns its length in bytes.
///
/// It takes no engine and no lock: whether a query parses is a property of
/// the text alone, so the panel can ask about what the user is typing
/// without waiting behind a search or an index rebuild.
///
/// The matching mode does not change that. It decides whether a bare term
/// is a subsequence or a substring, and both of those are terms; nothing
/// that parses in one mode fails in the other. The only errors are a bad
/// regex, a bad glob, a missing value and `content:`, and the mode is not
/// consulted for any of them.
///
/// # Safety
///
/// `query` must be a valid null-terminated string, and `out` must point to
/// a buffer of at least `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn sk_query_error(
    query: *const c_char,
    out: *mut c_char,
    capacity: c_int,
) -> c_int {
    if query.is_null() || out.is_null() || capacity <= 0 {
        return 0;
    }

    let Ok(text) = CStr::from_ptr(query).to_str() else {
        return 0;
    };

    let error = match Query::parse(text, Matching::default()) {
        Ok(_) => return 0,
        Err(e) => e.to_string(),
    };

    let Ok(message) = CString::new(error) else {
        return 0;
    };

    let bytes = message.as_bytes_with_nul();
    if bytes.len() > capacity as usize {
        return 0;
    }

    std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, out, bytes.len());

    (bytes.len() - 1) as c_int
}

/// The callback a streaming search reports its progress through.
///
/// It is handed the best rows found so far, how many candidates have
/// matched so far, and the context pointer it was registered with. It
/// returns zero to abandon the search and anything else to carry on.
///
/// The rows are borrowed for the duration of the call: the next publish
/// writes over the same buffer, so a callback that means to keep them has
/// to copy them.
pub type SearchProgress = Option<
    unsafe extern "C" fn(*const SearchResultEntry, c_int, c_int, *mut c_void) -> c_int,
>;

/// Runs a search and writes the results into a caller-provided buffer.
///
/// Returns the number of results written, or -1 on failure. This is
/// [`sk_engine_search_streaming`] with nothing to report until it has
/// finished.
///
/// # Safety
///
/// `out` must point to a buffer of at least `capacity` entries, and
/// `query` must be a valid null-terminated string. `matched` must be null
/// or point to a writable `int32_t`.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_search(
    engine: *mut Engine,
    query: *const c_char,
    out: *mut SearchResultEntry,
    capacity: c_int,
    matched: *mut c_int,
) -> c_int {
    run_search(
        engine,
        query,
        out,
        capacity,
        matched,
        0,
        None,
        std::ptr::null_mut(),
    )
}

/// Runs a search that reports the best rows it has found as it goes.
///
/// A search over a quarter of a million paths takes tens of milliseconds,
/// and a panel that shows nothing until it finishes feels like it is
/// thinking. `on_progress` is called every `chunk` candidates with the
/// best rows so far, so the first of them are on screen in a millisecond
/// or two.
///
/// The ranking is global — the best match can be the very last candidate —
/// so an intermediate page is *provisional*, and a later one may replace
/// it. What never happens is a row that a plain ranking of the candidates
/// seen so far would not have shown.
///
/// Returning zero from `on_progress` abandons the search. That is what
/// makes a superseded keystroke cheap, and it is also why a stopped search
/// does not leave the engine with a half-built pool for the next one.
///
/// `chunk` of zero or less means one chunk, and a null `on_progress` means
/// nothing is reported: either way this behaves exactly as
/// [`sk_engine_search`].
///
/// # Safety
///
/// `out` must point to `capacity` writable entries, `query` must be a
/// valid null-terminated string, and `context` must be valid for as long
/// as the search runs.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_search_streaming(
    engine: *mut Engine,
    query: *const c_char,
    out: *mut SearchResultEntry,
    capacity: c_int,
    matched: *mut c_int,
    chunk: c_int,
    on_progress: SearchProgress,
    context: *mut c_void,
) -> c_int {
    run_search(
        engine, query, out, capacity, matched, chunk, on_progress, context,
    )
}

/// The shared body of both searches.
unsafe fn run_search(
    engine: *mut Engine,
    query: *const c_char,
    out: *mut SearchResultEntry,
    capacity: c_int,
    matched: *mut c_int,
    chunk: c_int,
    on_progress: SearchProgress,
    context: *mut c_void,
) -> c_int {
    // The count is cleared before anything can fail, so that a caller
    // which reads it after an error or an unparseable query sees zero
    // rather than whatever the last search left there.
    if !matched.is_null() {
        *matched = 0;
    }

    if engine.is_null() || query.is_null() {
        return -1;
    }
    let Ok(text) = CStr::from_ptr(query).to_str() else {
        return -1;
    };

    let eng = unsafe { &*engine };

    // The snapshot is taken and the lock let go, so the search runs while
    // the index is free to change underneath it. It is taken before the
    // query is read, because the options say what a bare term means.
    let searchable = eng.searchable();
    let corpus = searchable.corpus;
    let mut options = searchable.options;

    let parsed = match Query::parse(text, options.matching) {
        Ok(q) => q,
        // A query that does not parse has no results — not every result.
        // `Query::default()` is the *empty* query, which matches
        // everything, so answering a malformed `re:[` with it put the
        // whole index on screen and called it a search.
        Err(_) => return 0,
    };

    // How many results are wanted is the caller's business, not the
    // engine's. It used to be engine state set through a separate call,
    // which meant that asking for more rows — to let the panel scroll
    // past the first screenful — changed what every *other* search in
    // flight would return. The caller's buffer already says how many it
    // wants, so that is the limit.
    options.limit = capacity.max(0) as usize;

    // The previous query's match set, when this query cannot match
    // anything outside it. Typing extends the query, so this is the
    // common case, and it is what makes the fourth keystroke of a word
    // cost a fraction of the first.
    //
    // The pool is the match set *before* the limit, so the panel asking
    // the same query again for a longer page is answered from it too.
    let pool = eng.pool_for(&parsed, searchable.stamp);

    // Zero, or less, means one chunk: a search with nothing to report
    // until it has finished, which is what every caller that passes no
    // callback wants.
    let chunk = if chunk > 0 { chunk as usize } else { usize::MAX };
    let pool_slice = pool.as_ref().map(|pool| pool.as_slice());

    // The page handed to the callback is built into a buffer that is
    // reused between publishes, so a search that publishes a dozen times
    // does not allocate a dozen pages.
    let mut page: Vec<SearchResultEntry> = Vec::new();

    let (outcome, narrowed) = match on_progress {
        Some(progress) => search_streaming(
            &corpus,
            &parsed,
            &options,
            pool_slice,
            chunk,
            &mut |rows, so_far| {
                page.clear();
                page.extend(rows.iter().map(|row| entry_for(&corpus, row)));

                // The rows are lent to the callback for the duration of
                // the call and no longer. A caller that keeps the pointer
                // is holding a buffer that the next publish overwrites,
                // which is why the header says so in as many words.
                let keep = unsafe {
                    progress(
                        page.as_ptr(),
                        page.len() as c_int,
                        so_far.min(c_int::MAX as usize) as c_int,
                        context,
                    )
                };

                if keep == 0 {
                    Progress::Stop
                } else {
                    Progress::Continue
                }
            },
        ),
        None => search_streaming(&corpus, &parsed, &options, pool_slice, chunk, &mut |_, _| {
            Progress::Continue
        }),
    };

    // What this query matched becomes the pool for the next keystroke,
    // unless it matched so much that a pool would not be worth keeping.
    // The stamp is the one the search ran under, not the current one: the
    // corpus may have been republished while the search was running, and
    // the pool describes the corpus it was computed against.
    eng.remember(parsed, narrowed, searchable.stamp);

    let count = outcome.results.len().min(capacity.max(0) as usize);

    // How many matched, which is not how many fit: the buffer is the
    // smaller of the two and says nothing about the size of the result.
    if !matched.is_null() {
        *matched = outcome.matched.min(c_int::MAX as usize) as c_int;
    }

    for (i, result) in outcome.results.iter().take(count).enumerate() {
        *out.add(i) = entry_for(&corpus, result);
    }
    count as c_int
}

/// Turns one ranked result into the row the C ABI hands out.
///
/// The kind, size and mtime are not part of the ranking, so the search does
/// not carry them and they are looked up here — one binary search per row,
/// over the rows that are shown rather than the ones that matched.
fn entry_for(corpus: &Corpus, result: &SearchResult) -> SearchResultEntry {
    let entry = corpus
        .position(result.id)
        .and_then(|position| corpus.entries.get(position));

    SearchResultEntry {
        id: result.id,
        score: result.score,
        start: result.start,
        end: result.end,
        is_dir: entry.map(|e| u8::from(e.is_dir)).unwrap_or(0),
        size: entry.map(|e| e.size).unwrap_or(0),
        modified: entry.map(|e| e.modified).unwrap_or(0),
    }
}

/// Returns the path of an entry as a newly allocated C string.
///
/// The caller owns the returned string and must free it with
/// [`sk_string_free`]. Returns null when the id is unknown.
///
/// # Safety
///
/// `id` must be a valid entry id.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_path(engine: *mut Engine, id: u32) -> *mut c_char {
    if engine.is_null() {
        return std::ptr::null_mut();
    }
    let eng = unsafe { &*engine };
    let corpus = eng.corpus();

    let Some(path) = corpus.position(id).and_then(|p| corpus.full_path(p)) else {
        return std::ptr::null_mut();
    };
    match CString::new(path.to_string_lossy().as_bytes()) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Returns the path of an entry relative to its root, as a newly allocated
/// C string.
///
/// This is the path the matcher scored, and it is what `fd` prints. The
/// caller owns the returned string and must free it with
/// [`sk_string_free`]. Returns null when the id is unknown.
///
/// # Safety
///
/// `id` must be a valid entry id.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_relative_path(engine: *mut Engine, id: u32) -> *mut c_char {
    if engine.is_null() {
        return std::ptr::null_mut();
    }
    let eng = unsafe { &*engine };
    let corpus = eng.corpus();

    let Some(path) = corpus.position(id).and_then(|p| corpus.relative_path(p)) else {
        return std::ptr::null_mut();
    };
    match CString::new(path.as_bytes()) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Resolves everything that is shown about a set of results, in one pass.
///
/// The panel shows a couple of hundred rows, and each row needs the full
/// path, the path relative to the root, and the matched offsets. Asking
/// for each of those separately means one lock acquisition and one walk of
/// the parent chain per row, which costs far more than the search itself.
/// This function does all of it at once, under one lock.
///
/// The paths are written into `out` as null-terminated strings, one after
/// another. The offset of each string is written into `offsets`: the full
/// path of result *n* first, then the path relative to the root.
///
/// Returns the number of results that were filled in, or -1 on failure.
///
/// # Safety
///
/// `ids` must point to a buffer of at least `count` entry ids, `out` must
/// point to a buffer of at least `capacity` bytes, and `offsets` must point
/// to a buffer of at least `count * 2` integers.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_resolve(
    engine: *mut Engine,
    ids: *const u32,
    count: c_int,
    out: *mut c_char,
    capacity: c_int,
    offsets: *mut i32,
) -> c_int {
    if engine.is_null() || ids.is_null() || out.is_null() || offsets.is_null() || count <= 0 {
        return -1;
    }

    let eng = unsafe { &*engine };
    let corpus = eng.corpus();

    let limit = capacity.max(0) as usize;
    let mut written = 0usize;
    let mut resolved = 0usize;

    for index in 0..count as usize {
        let id = unsafe { *ids.add(index) };

        let Some(position) = corpus.position(id) else {
            continue;
        };
        let Some(full) = corpus.full_path(position) else {
            continue;
        };

        let relative = corpus
            .relative_path(position)
            .map(|p| p.to_string())
            .unwrap_or_else(|| full.to_string_lossy().to_string());

        // The two paths are written one after the other, and the offsets
        // say where each of them starts.
        let mut starts = [0usize; 2];
        let mut written_here = 0usize;

        let full_bytes = full.to_string_lossy().as_bytes().to_vec();
        let relative_bytes = relative.as_bytes().to_vec();

        for (slot, bytes) in [full_bytes, relative_bytes].into_iter().enumerate() {
            if written + bytes.len() + 1 > limit {
                break;
            }

            starts[slot] = written;

            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    out.add(written) as *mut u8,
                    bytes.len(),
                );
                *out.add(written + bytes.len()) = 0;
            }

            written += bytes.len() + 1;
            written_here += 1;
        }

        if written_here < 2 {
            // The buffer ran out. The results that were written stay, and
            // the rest are left out.
            break;
        }

        unsafe {
            *offsets.add(resolved * 2) = starts[0] as i32;
            *offsets.add(resolved * 2 + 1) = starts[1] as i32;
        }

        resolved += 1;
    }

    resolved as c_int
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
pub unsafe extern "C" fn sk_engine_positions(
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
    let corpus = eng.corpus();
    let options = eng.options();

    let parsed = match Query::parse(text, options.matching) {
        Ok(q) => q,
        Err(_) => return 0,
    };
    // The first scoring term, compiled exactly as the search compiled
    // it: a fuzzy term underlines the alignment the DP matrix found, an
    // exact one underlines the run it matched, and the scan direction is
    // the one the ranking was measured with. Any of those read from
    // somewhere else would underline characters the ranking did not come
    // from.
    let scorers = Scorer::compile(&parsed.positive, &options);
    let Some(scorer) = scorers.first() else {
        return 0;
    };

    // The positions are against the path relative to the root, because
    // that is what the search scored. The UI splits the path into its
    // directory and its name and shifts the offsets accordingly.
    let Some(path) = corpus.position(id).and_then(|p| corpus.relative_path(p)) else {
        return 0;
    };

    let positions = scorer.positions(path, &mut Scratch::new());

    let count = positions.len();
    let written = count.min(capacity.max(0) as usize);
    for (i, pos) in positions.iter().take(written).enumerate() {
        *out.add(i) = *pos;
    }
    count as c_int
}

/// Returns the matched byte offsets for a set of results, in one pass.
///
/// The query is parsed once and the paths are resolved once, instead of
/// once per result.
///
/// The offsets are written into `out` one result after another, and the
/// number of offsets per result is written into `counts`. The offsets for
/// result *n* start at `out + offset[n]`, where `offset` is the running
/// total of the counts before *n*.
///
/// # Safety
///
/// `ids` must point to a buffer of at least `count` entry ids, `out` must
/// point to a buffer of at least `capacity` integers, and `counts` must
/// point to a buffer of at least `count` integers.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_positions_batch(
    engine: *mut Engine,
    ids: *const u32,
    count: c_int,
    pattern: *const c_char,
    out: *mut i32,
    capacity: c_int,
    counts: *mut i32,
) -> c_int {
    if engine.is_null() || ids.is_null() || counts.is_null() || pattern.is_null() {
        return 0;
    }

    let Ok(text) = CStr::from_ptr(pattern).to_str() else {
        return 0;
    };

    let eng = unsafe { &*engine };
    let corpus = eng.corpus();
    let options = eng.options();

    let parsed = match Query::parse(text, options.matching) {
        Ok(q) => q,
        Err(_) => return 0,
    };

    // The first scoring term, compiled as the search compiled it; see
    // `sk_engine_positions` for why it is taken from there and not built
    // from the term's text.
    let scorers = Scorer::compile(&parsed.positive, &options);
    let Some(scorer) = scorers.first() else {
        return 0;
    };

    // One scratch for the whole batch: the matcher's working buffers are
    // what it saves, and the rows are walked one after another.
    let mut scratch = Scratch::new();
    let mut written = 0usize;

    for index in 0..count as usize {
        let id = unsafe { *ids.add(index) };

        let Some(path) = corpus.position(id).and_then(|p| corpus.relative_path(p)) else {
            unsafe { *counts.add(index) = 0 };
            continue;
        };

        let positions = scorer.positions(path, &mut scratch);

        let available = (capacity.max(0) as usize).saturating_sub(written);

        for (offset, position) in positions.iter().take(available).enumerate() {
            unsafe { *out.add(written + offset) = *position };
        }

        let taken = positions.len().min(available);

        unsafe { *counts.add(index) = taken as i32 };

        written += taken;
    }

    written as c_int
}

/// Frees a string returned by this library.
///
/// # Safety
///
/// `s` must have been returned by this library and must not be used after.
#[no_mangle]
pub unsafe extern "C" fn sk_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// The version of the core library, as a static C string.
#[no_mangle]
pub extern "C" fn sk_version() -> *const c_char {
    // "0.1.0\0"
    b"0.1.0\0".as_ptr() as *const c_char
}

/// A change to the filesystem, reported to the engine by the watcher.
///
/// The struct is passed by pointer and filled in by the Swift side.
#[repr(C)]
pub struct FileChange {
    /// The path that changed.
    pub path: *const c_char,
    /// The kind of change: 0 created, 1 modified, 2 removed.
    pub kind: c_int,
    /// Whether the entry is a directory.
    pub is_dir: c_int,
}

/// Applies a batch of filesystem changes to the index.
///
/// The changes are the ones the watcher reported since the last call. They
/// are applied under the write lock, and the corpus is rebuilt afterwards so
/// that the searches see the new entries.
///
/// Returns the number of entries in the index, or -1 on failure.
///
/// # Safety
///
/// `changes` must point to a buffer of at least `count` entries, and every
/// `path` in it must be a valid null-terminated string.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_apply_changes(
    engine: *mut Engine,
    changes: *const FileChange,
    count: c_int,
) -> c_int {
    if engine.is_null() || changes.is_null() || count < 0 {
        return -1;
    }
    let eng = unsafe { &*engine };

    // The changes are decoded before the lock is taken, because the path
    // strings have to be copied out anyway.
    let mut decoded: Vec<(std::path::PathBuf, u8, bool)> = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let change = unsafe { &*changes.add(i) };
        if change.path.is_null() {
            continue;
        }
        let Ok(text) = CStr::from_ptr(change.path).to_str() else {
            continue;
        };
        decoded.push((
            std::path::PathBuf::from(text),
            change.kind as u8,
            change.is_dir != 0,
        ));
    }

    // What the scan was told to skip, the watcher must skip too. The
    // rules are compiled once for the batch, before the lock is taken.
    let filter = {
        let inner = eng.read();
        crate::scan::ChangeFilter::new(
            &inner.roots,
            &eng.options().exclude_patterns,
            true,
        )
    };

    decoded.retain(|(path, _, is_dir)| !filter.skips(path, *is_dir));

    // The metadata of each changed path is read before the lock is taken.
    // These are syscalls, and a batch of them under the write lock is time
    // the index spends unavailable for no reason.
    let stated: Vec<(std::path::PathBuf, u8, EntryKind, u64, i64)> = decoded
        .into_iter()
        .map(|(path, kind, is_dir)| {
            let metadata = std::fs::metadata(&path).ok();
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            let entry_kind = if is_dir {
                EntryKind::Directory
            } else {
                EntryKind::File
            };

            (path, kind, entry_kind, size, modified)
        })
        .collect();

    let mut inner = eng.write();

    for (path, kind, entry_kind, size, modified) in stated {
        match kind {
            // Removed.
            2 => {
                inner.index.remove(&path);
            }
            // Created or modified.
            _ => {
                inner.index.upsert(&path, entry_kind, size, modified);
            }
        }
    }

    // What actually changed comes from the index rather than from this
    // loop: `upsert` reports an entry it did not have to touch as a hit,
    // and it creates the missing parent directories on its way, which are
    // new entries nobody here hears about.
    let changed = inner.index.take_dirty();

    if !changed.is_empty() {
        inner.index.touch();

        // The new snapshot is built and published while the maintenance
        // lock is still held, so that it agrees with the index. Searches
        // run against the previous snapshot in the meantime rather than
        // waiting for this one.
        //
        // Only the changed entries are built. Rebuilding the whole corpus
        // here cost a fresh string for every path in the index on every
        // batch, which the watcher delivers several times a second.
        eng.publish_changed(&inner.index, &changed);
    }

    inner.index.live_len() as c_int
}

/// Starts watching the engine's roots for changes.
///
/// The watcher runs on its own thread. The changes it reports are applied
/// with [`sk_engine_apply_changes`].
///
/// Returns 0 on success, -1 on failure.
#[no_mangle]
pub extern "C" fn sk_engine_start_watch(engine: *mut Engine) -> c_int {
    if engine.is_null() {
        return -1;
    }
    let eng = unsafe { &*engine };
    let roots = eng.read().roots.clone();

    if roots.is_empty() {
        return -1;
    }

    // One watcher per root, each on its own thread. The channels are
    // collected so that the changes can be drained.
    let mut watchers = Vec::new();
    for root in &roots {
        match crate::watch::Watcher::new(root) {
            Ok(w) => watchers.push(w),
            Err(_) => continue,
        }
    }

    if watchers.is_empty() {
        return -1;
    }

    eng.write().watchers = watchers;
    0
}

/// Collects the changes the watchers have reported.
///
/// The changes are written into a caller-provided buffer as newline
/// separated records of the form `kind<TAB>is_dir<TAB>path`.
///
/// Returns the number of changes written, or -1 on failure.
///
/// # Safety
///
/// `out` must point to a buffer of at least `capacity` bytes.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_poll_changes(
    engine: *mut Engine,
    out: *mut c_char,
    capacity: c_int,
) -> c_int {
    if engine.is_null() || out.is_null() || capacity <= 0 {
        return -1;
    }
    let eng = unsafe { &*engine };

    // Draining the watchers only reads them, so the read lock is enough.
    // Taking the write lock here — which this used to do, every few
    // hundred milliseconds — queued a writer against a lock that prefers
    // writers, and that was enough to stall every search behind it.
    //
    // The drain does not block: the watchers are asked for a change with a
    // zero timeout.
    let drained: Vec<(std::path::PathBuf, u8)> = {
        let inner = eng.read();
        let mut drained = Vec::new();

        for watcher in &inner.watchers {
            while let Some(change) =
                watcher.next_change_timeout(std::time::Duration::from_millis(0))
            {
                let kind = match change.kind {
                    crate::watch::ChangeKind::Created => 0,
                    crate::watch::ChangeKind::Modified => 1,
                    crate::watch::ChangeKind::Removed => 2,
                    crate::watch::ChangeKind::Unknown => 3,
                };
                // A change that cannot be described precisely means the
                // subtree has to be rescanned, which is signalled by the
                // `3` kind.
                let kind = if change.needs_rescan { 3 } else { kind };

                drained.push((change.path, kind));
            }
        }

        drained
    };

    // Whether each path is a directory is looked up on the filesystem,
    // because the watcher does not report it. The lookups are syscalls, so
    // they are made after the lock has been let go.
    let records: Vec<String> = drained
        .into_iter()
        .map(|(path, kind)| {
            let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);

            format!(
                "{kind}\t{}\t{}",
                if is_dir { 1 } else { 0 },
                path.to_string_lossy()
            )
        })
        .collect();

    let mut written = 0usize;
    let mut offset = 0usize;
    let limit = capacity as usize;

    for record in records {
        let bytes = record.as_bytes();
        if offset + bytes.len() + 1 > limit {
            break;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                out.add(offset) as *mut u8,
                bytes.len(),
            );
            *out.add(offset + bytes.len()) = 0;
        }
        offset += bytes.len() + 1;
        written += 1;
    }

    written as c_int
}
