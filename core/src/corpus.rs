//! The searchable corpus.
//!
//! Turning the index into searchable text is expensive: every entry's file
//! name has to be materialised as a `String`. Doing that on every keystroke
//! would cost more than the matching itself — about 150 ms for a quarter of a
//! million entries.
//!
//! The corpus solves this by holding the text in a flat, cache-friendly
//! layout that is built once and reused for every query. Because the layout
//! is a plain `Vec`, the search can slice it into chunks and hand them to
//! Rayon without any locking.

use std::path::{Path, PathBuf};

use crate::index::{EntryId, Index};

/// A candidate for matching, with its text already materialised.
#[derive(Debug, Clone)]
pub struct CorpusEntry {
    /// The index entry this candidate refers to.
    pub id: EntryId,
    /// The file name, which is what the matcher scores.
    pub name: String,
    /// The full path, for display.
    pub path: PathBuf,
}

/// The searchable form of an index.
#[derive(Debug, Default)]
pub struct Corpus {
    /// The candidates, in index order.
    pub entries: Vec<CorpusEntry>,
    /// The file names, in the same order, for slicing.
    pub names: Vec<String>,
}

impl Corpus {
    /// Builds a corpus from an index.
    pub fn build(index: &Index) -> Corpus {
        let mut entries = Vec::with_capacity(index.len());

        for id in 0..index.len() as u32 {
            let Some(entry) = index.get(id) else {
                continue;
            };

            // Tombstoned entries are kept in the index so that the ids stay
            // stable, but they must not be searchable.
            if entry.removed {
                continue;
            }

            let Some(name) = index.names.get(entry.name) else {
                continue;
            };

            // The full path is only needed for display, so it is built lazily
            // by the UI rather than stored here.
            entries.push(CorpusEntry {
                id,
                name: name.to_string(),
                path: PathBuf::new(),
            });
        }

        let names = entries.iter().map(|e| e.name.clone()).collect();
        Corpus { entries, names }
    }

    /// The number of candidates.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the corpus is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The approximate memory used by the corpus, in bytes.
    pub fn memory_usage(&self) -> usize {
        self.entries
            .iter()
            .map(|e| e.name.len() + e.path.as_os_str().len() + 48)
            .sum()
    }
}

/// Reconstructs the full path of an entry, for display.
pub fn full_path(index: &Index, id: EntryId) -> Option<PathBuf> {
    index.full_path(id)
}

/// The file name of a path.
pub fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}
