//! seek core: a lightweight macOS file search engine.
//!
//! The engine combines two ideas:
//!
//! * **fd-style traversal** — a fast, parallel, gitignore-aware filesystem
//!   walk with support for type / extension / size / time filters.
//! * **fzf-style matching** — the fuzzy matching and scoring algorithm ported
//!   from fzf (see the `fuzzy` module), so results rank the way fzf ranks
//!   them.
//!
//! Unlike Spotlight, the index is owned by the app: it is built at launch and
//! then kept current with FSEvents, so every query is served from memory.

pub mod corpus;
pub mod ffi;
pub mod ffi_persist;
pub mod fuzzy;
pub mod index;
pub mod persist;
pub mod query;
pub mod scan;
pub mod search;
pub mod watch;

pub use corpus::Corpus;
pub use fuzzy::{FuzzyMatcher, MatchResult, Scheme};
pub use index::{EntryKind, FileEntry, Index, IndexConfig, NamePool};
pub use persist::{LoadError, Snapshot};
pub use query::{Query, QueryError, SearchOptions, SearchOutcome, SearchResult};
pub use scan::{scan_blocking, ScanConfig, ScannedEntry, ScanSummary, Scanner};
pub use search::search;
pub use watch::{Change, ChangeKind, Watcher};
