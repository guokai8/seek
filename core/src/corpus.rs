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
//!
//! The corpus is also the *published snapshot* of the index: it carries
//! everything a keystroke needs — the text to match, the paths to show, and
//! whether each entry is a directory. A search therefore never has to reach
//! back into the index, which is what lets it run while the index is being
//! rebuilt. See [`crate::ffi::Engine`] for why that matters.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::index::{EntryId, EntryKind, Index};

/// A candidate for matching, with its text already materialised.
///
/// The entry carries no text of its own. The matcher scores the whole
/// path, which lives in [`Corpus::paths`] at the same position, so a name
/// here would be a second copy of bytes the corpus already holds.
#[derive(Debug, Clone, Copy)]
pub struct CorpusEntry {
    /// The index entry this candidate refers to.
    pub id: EntryId,
    /// Whether the entry is a directory.
    ///
    /// The index knows this, but a search that had to ask it would have to
    /// take the index's lock, so it is carried here instead.
    pub is_dir: bool,
    /// The position of the root this entry is under, in [`Corpus::roots`].
    ///
    /// This is what turns the relative path back into an absolute one,
    /// without walking the parent chain a second time.
    pub root: Option<u32>,
    /// The file size in bytes, as the last scan saw it.
    ///
    /// This and `modified` are here for the same reason `is_dir` is: the
    /// result row shows them, and the only other place they live is the
    /// index, behind the maintenance lock that a rebuild holds for the
    /// whole of a scan. Reading them from there per row would stall the
    /// panel for as long as a scan takes. They cost sixteen bytes a
    /// candidate -- about 4MB over a home directory.
    pub size: u64,
    /// The modification time, as a Unix timestamp in seconds.
    pub modified: i64,
}

/// Where a candidate's path lives in the arena.
///
/// The arena is `base` followed by `overflow`, addressed as one run of
/// bytes: an offset below `base.len()` is in `base`, anything at or above
/// it is in `overflow`. A path is never split across the two.
///
/// The offsets are `u32`, so the arena holds up to 4GB of path text — about
/// fifty million paths. [`Corpus::build`] stops rather than wrap around.
#[derive(Debug, Clone, Copy, Default)]
struct Span {
    /// The offset of the path's first byte.
    start: u32,
    /// The path's length in bytes.
    len: u32,
}

/// The searchable form of an index.
#[derive(Debug, Default, Clone)]
pub struct Corpus {
    /// The candidates, in increasing id order.
    ///
    /// The order is what lets [`Corpus::position`] binary-search, and what
    /// lets [`Corpus::updated`] merge a batch of changes in one pass.
    pub entries: Vec<CorpusEntry>,
    /// The path bytes carried over from the snapshot this was derived
    /// from, shared and never mutated.
    ///
    /// fzf matches against the whole path, so the matcher does too. The
    /// path is what the user sees in the result row, and a search that
    /// cannot find what it displays would be a search that lies.
    ///
    /// The paths used to be a `Vec<Arc<str>>`: a quarter of a million
    /// separate allocations, which is what pinned about 95MB of malloc's
    /// small-object regions. Freeing them returned nothing to the kernel,
    /// because every region stayed partly occupied —
    /// `malloc_zone_pressure_relief` handed back zero bytes even after the
    /// whole engine was dropped. One contiguous arena is a single large
    /// allocation, which is mmap-backed and does come back.
    base: Arc<String>,
    /// The path bytes this snapshot added, for the entries that changed.
    ///
    /// A watcher batch cannot be appended to `base`, which an older
    /// snapshot may still be reading. Its paths go here instead, and the
    /// two are addressed as one run of bytes. [`Corpus::updated`] folds
    /// them back together once this grows past a fraction of the base.
    overflow: String,
    /// Where each candidate's path lives, in the same order as `entries`.
    spans: Vec<Span>,
    /// The roots the entries are under, as absolute paths.
    pub roots: Vec<PathBuf>,
    /// Each path's length with surrounding whitespace removed.
    ///
    /// This is one of fzf's tiebreak keys, and it does not depend on the
    /// query, so it is worked out once here rather than for every candidate
    /// on every keystroke. fzf caches it on the item for the same reason.
    pub trim_lengths: Vec<u16>,
}

impl Corpus {
    /// Builds a corpus from an index, from scratch.
    ///
    /// This materialises every path in the index, which for a home
    /// directory is a quarter of a million strings and about a fifth of a
    /// second. It is what a scan and a restore need; a watcher batch uses
    /// [`Corpus::updated`] instead.
    pub fn build(index: &Index) -> Corpus {
        let mut entries = Vec::with_capacity(index.len());
        let mut spans = Vec::with_capacity(index.len());
        let mut trim_lengths = Vec::with_capacity(index.len());
        // The arena is reserved up front. Letting a `String` grow to
        // seventeen megabytes by doubling means the old and new buffers
        // are both live across each reallocation, which put about 8MB back
        // onto the peak the rest of this step had just taken off. The
        // paths of a home directory average 85 bytes; the slack left by
        // over-reserving is cheaper than the copy that `shrink_to_fit`
        // would cost.
        let mut base = String::with_capacity(index.len() * 96);

        for id in 0..index.len() as u32 {
            let Some((entry, path, trim)) = Self::row(index, id) else {
                continue;
            };

            // A `u32` offset covers 4GB of path text. Beyond that the
            // arena would wrap and every span past it would address the
            // wrong bytes, so the corpus stops instead. Reaching this
            // needs tens of millions of files under one root.
            if base.len() + path.len() > u32::MAX as usize {
                break;
            }

            spans.push(Span {
                start: base.len() as u32,
                len: path.len() as u32,
            });
            base.push_str(&path);
            entries.push(entry);
            trim_lengths.push(trim);
        }

        Corpus {
            entries,
            base: Arc::new(base),
            overflow: String::new(),
            spans,
            roots: index.config_roots.clone(),
            trim_lengths,
        }
    }

    /// Brings a snapshot up to date for the entries that have changed.
    ///
    /// The watcher reports a handful of changes at a time — a file saved,
    /// a temporary file created and deleted again — out of a quarter of a
    /// million entries. Rebuilding the whole corpus for them allocated a
    /// fresh string for every path in the index several times a second,
    /// which is what made the app burn a third of a core while it sat
    /// idle and grow by hundreds of megabytes over an afternoon.
    ///
    /// This walks the old snapshot and the changed ids together in one
    /// pass instead. A path that did not change is carried over as its
    /// eight-byte span — the arena holding its bytes is shared with the
    /// old snapshot, not copied — so only the changed entries are built.
    /// The result is the same snapshot a rebuild would produce; the tests
    /// in `tests/incremental_corpus.rs` compare the two field by field.
    ///
    /// `changed` comes from [`Index::take_dirty`] and need not be sorted
    /// or free of duplicates.
    pub fn updated(&self, index: &Index, changed: &[EntryId]) -> Corpus {
        if changed.is_empty() {
            return self.clone();
        }

        // The overflow grows with every watcher batch, and nothing ever
        // takes anything out of it. Left to itself that is the index's own
        // unbounded growth over again in a different place, so once it is
        // a quarter of the base the snapshot is rebuilt whole and the two
        // arenas become one. The floor keeps a small corpus from
        // rebuilding on every batch.
        if self.overflow.len() > self.base.len() / 4 + Self::COMPACT_FLOOR {
            return Self::build(index);
        }

        let mut ids: Vec<EntryId> = changed.to_vec();
        ids.sort_unstable();
        ids.dedup();

        let extra = ids.len();
        let mut entries = Vec::with_capacity(self.entries.len() + extra);
        let mut spans = Vec::with_capacity(self.spans.len() + extra);
        let mut trim_lengths = Vec::with_capacity(self.trim_lengths.len() + extra);
        let mut overflow = self.overflow.clone();

        // Where the overflow begins, in the addresses the spans use.
        let split = self.base.len();

        let mut old = 0usize;

        for id in ids {
            // Everything before the changed entry is carried over as it
            // stands. This is the cheap part, and it is nearly all of it.
            while old < self.entries.len() && self.entries[old].id < id {
                entries.push(self.entries[old]);
                spans.push(self.spans[old]);
                trim_lengths.push(self.trim_lengths[old]);
                old += 1;
            }

            // The old row for this entry is dropped, whatever it was. The
            // new one is built from the index below, and an entry that has
            // been removed simply does not come back.
            if old < self.entries.len() && self.entries[old].id == id {
                old += 1;
            }

            if let Some((entry, path, trim)) = Self::row(index, id) {
                spans.push(Span {
                    start: (split + overflow.len()) as u32,
                    len: path.len() as u32,
                });
                overflow.push_str(&path);
                entries.push(entry);
                trim_lengths.push(trim);
            }
        }

        while old < self.entries.len() {
            entries.push(self.entries[old]);
            spans.push(self.spans[old]);
            trim_lengths.push(self.trim_lengths[old]);
            old += 1;
        }

        Corpus {
            entries,
            base: Arc::clone(&self.base),
            overflow,
            spans,
            roots: index.config_roots.clone(),
            trim_lengths,
        }
    }

    /// The smallest overflow worth compacting, in bytes.
    ///
    /// Without it a corpus of a few hundred paths would rebuild itself on
    /// every watcher batch, because a quarter of a tiny base is nothing.
    const COMPACT_FLOOR: usize = 1 << 20;

    /// The text a span addresses.
    ///
    /// The arena is a `String`, so slicing it checks a character boundary
    /// in constant time rather than validating the bytes. A `Vec<u8>`
    /// arena would have meant either validating every path on every
    /// keystroke or reaching for `from_utf8_unchecked`.
    fn text(&self, span: Span) -> &str {
        let start = span.start as usize;
        let end = start + span.len as usize;
        let split = self.base.len();

        // A path is never split across the two arenas.
        if end <= split {
            &self.base[start..end]
        } else {
            &self.overflow[start - split..end - split]
        }
    }

    /// The paths of every candidate, in corpus order.
    pub fn paths(&self) -> impl Iterator<Item = &str> + '_ {
        self.spans.iter().map(|&span| self.text(span))
    }

    /// The snapshot row for an index entry.
    ///
    /// Returns `None` for an entry that does not belong in a snapshot, so
    /// that [`Corpus::build`] and [`Corpus::updated`] agree on what is
    /// searchable without having to repeat the reasons.
    fn row(index: &Index, id: EntryId) -> Option<(CorpusEntry, String, u16)> {
        let entry = index.get(id)?;

        // Tombstoned entries are kept in the index so that the ids stay
        // stable, but they must not be searchable.
        if entry.removed {
            return None;
        }

        let name = index.names.get(entry.name)?;

        // The full path is what the matcher scores, the way fzf does.
        // It is built once here, because building it per query would
        // cost more than the matching itself.
        //
        // The path is relative to the root, the way `fd` prints it when
        // it is given a root. The root's prefix is the same for every
        // entry under it, so searching it is wasted work.
        let (path, root) = index
            .relative_path_with_root(id)
            .unwrap_or_else(|| (name.to_string(), None));

        // A root has no path relative to itself, and a candidate with
        // no text can neither be matched nor shown. An empty path also
        // has `trim_length() == 0`, so fzf's `Length` tiebreak sorts it
        // ahead of everything whenever the scores are equal — which is
        // every entry under the empty query — and the panel draws it as
        // a blank, selected first row. The root is left out here rather
        // than filtered further down, so that every candidate the
        // search sees is known to have text.
        if path.is_empty() {
            return None;
        }

        let trim = crate::fuzzy::trim_length(&path);

        Some((
            CorpusEntry {
                id,
                is_dir: matches!(entry.kind, EntryKind::Directory),
                root: root.map(|r| r as u32),
                size: entry.size,
                modified: entry.modified,
            },
            path,
            trim,
        ))
    }

    /// The number of candidates.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the corpus is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The position of an entry id in the corpus.
    ///
    /// The entries are built in increasing id order and removed ones are
    /// left out, so the position is found by searching rather than by
    /// indexing. A binary search costs a handful of comparisons, which is
    /// cheaper than carrying a lookup table for a quarter of a million
    /// entries.
    pub fn position(&self, id: EntryId) -> Option<usize> {
        self.entries
            .binary_search_by_key(&id, |entry| entry.id)
            .ok()
    }

    /// The path of a candidate relative to its root.
    pub fn relative_path(&self, position: usize) -> Option<&str> {
        self.spans.get(position).map(|&span| self.text(span))
    }

    /// The absolute path of a candidate, for opening and revealing it.
    pub fn full_path(&self, position: usize) -> Option<PathBuf> {
        let relative = self.relative_path(position)?;
        let entry = self.entries.get(position)?;

        let Some(root) = entry.root.and_then(|r| self.roots.get(r as usize)) else {
            // Without a known root there is nothing to join to, so the
            // relative path is all there is. This is what an index built
            // without roots produces.
            return Some(PathBuf::from(relative));
        };

        if relative.is_empty() {
            // The entry *is* the root.
            return Some(root.clone());
        }

        Some(root.join(relative))
    }

    /// The approximate memory used by the corpus, in bytes.
    pub fn memory_usage(&self) -> usize {
        // The base arena is shared with the snapshot this one came from,
        // so while two of them overlap it is counted for both.
        self.base.len()
            + self.overflow.capacity()
            + self.spans.capacity() * std::mem::size_of::<Span>()
            + self.entries.capacity() * std::mem::size_of::<CorpusEntry>()
            + self.trim_lengths.capacity() * std::mem::size_of::<u16>()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::EntryKind;

    /// Builds an index with a root and one file under it.
    fn index_with_a_root() -> Index {
        let mut index = Index::new();

        let root = index.add_named(None, "project", EntryKind::Directory, 0, 0);
        index.roots.push(root);
        index.config_roots.push(PathBuf::from("/tmp/project"));
        index.add_named(Some(root), "main.rs", EntryKind::File, 1, 0);
        index.touch();

        index
    }

    #[test]
    fn the_root_is_not_a_candidate() {
        let corpus = Corpus::build(&index_with_a_root());

        // The root's path relative to itself is empty, so it is not a
        // candidate: there would be nothing to match and nothing to draw.
        assert_eq!(corpus.paths().collect::<Vec<_>>(), vec!["main.rs"]);
    }

    #[test]
    fn no_candidate_has_an_empty_path() {
        let corpus = Corpus::build(&index_with_a_root());

        assert!(
            corpus.paths().all(|path| !path.is_empty()),
            "an empty path would rank first and draw as a blank row"
        );
        assert_eq!(corpus.paths().count(), corpus.entries.len());
        assert_eq!(corpus.paths().count(), corpus.trim_lengths.len());
    }

    /// A candidate's fixed cost is paid a quarter of a million times.
    ///
    /// The corpus is the one structure sized by the number of files rather
    /// than by what the user is doing, so a field added here is multiplied
    /// by the whole index: over a home directory of about 258,000 entries,
    /// every eight bytes is 2MB that the app holds for as long as it runs.
    /// This is a tripwire rather than a budget - changing the number is
    /// fine, doing it without noticing is not.
    #[test]
    fn a_candidate_costs_what_it_is_supposed_to() {
        assert_eq!(
            std::mem::size_of::<CorpusEntry>(),
            32,
            "the per-candidate cost changed; see the doc comment above"
        );
    }

    /// The overflow arena must not grow for ever.
    ///
    /// Every watcher batch appends the changed paths to it and nothing
    /// ever takes anything out, so left alone it is the index's own
    /// unbounded growth in a different place — the bug the eighteenth step
    /// of `DEVELOPMENT.md` was about. Past a fraction of the base the
    /// snapshot is rebuilt whole and the two arenas become one again.
    #[test]
    fn the_overflow_is_compacted_rather_than_growing_for_ever() {
        let mut index = Index::new();
        let root = index.add_named(None, "project", EntryKind::Directory, 0, 0);
        index.roots.push(root);
        index.config_roots.push(PathBuf::from("/tmp/project"));

        // A long name, so the floor is reached in a few thousand batches
        // rather than a few hundred thousand.
        let name = "a".repeat(900);
        let file = index.add_named(Some(root), &name, EntryKind::File, 1, 0);
        index.touch();

        let mut corpus = Corpus::build(&index);
        let base = corpus.base.len();
        let mut compacted = false;

        for round in 1..=4000u64 {
            // The changed ids are passed explicitly below, so the entry
            // only has to differ — it does not have to go through
            // `upsert` to be recorded as dirty.
            index.entries[file as usize].size = round;

            let before = corpus.overflow.len();
            corpus = corpus.updated(&index, &[file]);

            if corpus.overflow.len() < before {
                compacted = true;
                break;
            }
        }

        assert!(
            compacted,
            "the overflow grew to {} bytes against a base of {base} without compacting",
            corpus.overflow.len()
        );
        assert!(
            corpus.overflow.is_empty(),
            "a compacted snapshot holds its paths in one arena"
        );
        // Compaction must not lose anything.
        assert_eq!(corpus.paths().count(), 1);
        assert_eq!(corpus.paths().next(), Some(name.as_str()));
    }
}
