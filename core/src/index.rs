//! The in-memory file index.
//!
//! The index holds every known path and the metadata needed to search and
//! display it. It is designed to answer a keystroke in well under 10 ms
//! across a few hundred thousand entries, which rules out touching the disk
//! during a query.
//!
//! Two techniques keep the memory footprint small:
//!
//! * **String interning** — file names are stored once in a `NamePool` and
//!   referenced by index. Directories share their name with any other
//!   directory of the same name, which is very common in a real tree.
//! * **Slab allocation** — entries live in a `Vec<SlabIndex>` addressed by a
//!   `u32`, so the index itself is a dense array with no per-entry `Box`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A reference to an entry in the index.
pub type EntryId = u32;

/// The kind of an indexed entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Anything else (socket, device, ...).
    Other,
}

/// A single indexed file or directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// The parent directory's entry id.
    pub parent: Option<EntryId>,
    /// Index of the file name in the [`NamePool`].
    pub name: u32,
    /// The entry kind.
    pub kind: EntryKind,
    /// File size in bytes.
    pub size: u64,
    /// Last modification time, as a Unix timestamp in seconds.
    pub modified: i64,
    /// Whether the entry has been removed from disk.
    ///
    /// Removed entries are kept as tombstones rather than deleted, because
    /// deleting from the middle of the entries vector would renumber every
    /// id after it, and those ids are handed out to the UI.
    #[serde(default)]
    pub removed: bool,
}

/// A pool of interned strings.
///
/// Names are deduplicated, so the many directories that share a name (and the
/// many files sharing a common extension-bearing name) cost only one copy.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct NamePool {
    /// The interned strings, in insertion order.
    names: Vec<String>,
    /// A lookup table from string to its index.
    lookup: HashMap<String, u32>,
}

impl NamePool {
    /// Creates an empty pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns a string, returning its index.
    ///
    /// The index of an already-interned string is returned unchanged.
    pub fn intern(&mut self, name: &str) -> u32 {
        if let Some(&index) = self.lookup.get(name) {
            return index;
        }
        let index = self.names.len() as u32;
        self.names.push(name.to_string());
        self.lookup.insert(name.to_string(), index);
        index
    }

    /// Returns the interned string for an index.
    pub fn get(&self, index: u32) -> Option<&str> {
        self.names.get(index as usize).map(|s| s.as_str())
    }

    /// The number of interned strings.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The approximate memory used by the pool, in bytes.
    pub fn memory_usage(&self) -> usize {
        self.names.iter().map(|s| s.len() + 24).sum()
    }
}

/// Configuration for building an index.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// The roots that were scanned.
    pub roots: Vec<PathBuf>,
    /// When the index was last written.
    pub updated_at: i64,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            updated_at: 0,
        }
    }
}

/// The in-memory index of the filesystem.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Index {
    /// The interned names.
    pub names: NamePool,
    /// The entries, addressed by [`EntryId`].
    pub entries: Vec<FileEntry>,
    /// The entry id of each root, used to identify entries for removal.
    pub roots: Vec<EntryId>,
    /// The roots that were scanned.
    pub config_roots: Vec<PathBuf>,
    /// When the index was last updated.
    pub updated_at: i64,
    /// The number of entries removed since the index was built.
    pub removed: u32,
    /// The entries that have changed since the snapshot was last published.
    ///
    /// A watcher batch touches a handful of entries out of a quarter of a
    /// million, and rebuilding the whole searchable corpus for them costs
    /// a fresh `String` for every path in the index. The index records
    /// what it changed, so that the snapshot can be brought up to date in
    /// place instead. See [`Corpus::updated`](crate::corpus::Corpus::updated).
    ///
    /// It is recorded here rather than returned to the caller because
    /// `upsert` creates the missing parent directories on its way, and
    /// those are new rows that the caller never hears about.
    ///
    /// It is not part of the snapshot on disk: an index that has just been
    /// restored has nothing pending, because its corpus is built whole.
    #[serde(skip)]
    dirty: Vec<EntryId>,
}

impl Index {
    /// Creates an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an entry, returning its id.
    pub fn add(&mut self, entry: FileEntry) -> EntryId {
        let id = self.entries.len() as u32;
        self.entries.push(entry);
        self.dirty.push(id);
        id
    }

    /// Takes the entries that have changed since this was last called.
    ///
    /// The caller publishes a snapshot for them and the list starts over.
    pub fn take_dirty(&mut self) -> Vec<EntryId> {
        std::mem::take(&mut self.dirty)
    }

    /// Adds an entry with a name, interning the name first.
    ///
    /// This is the usual way to add entries: the name is stored once in the
    /// [`NamePool`], so entries that share a name cost nothing extra.
    pub fn add_named(
        &mut self,
        parent: Option<EntryId>,
        name: &str,
        kind: EntryKind,
        size: u64,
        modified: i64,
    ) -> EntryId {
        let interned = self.names.intern(name);
        self.add(FileEntry {
            parent,
            name: interned,
            kind,
            size,
            modified,
            removed: false,
        })
    }

    /// Returns the entry for an id.
    pub fn get(&self, id: EntryId) -> Option<&FileEntry> {
        self.entries.get(id as usize)
    }

    /// The number of entries in the index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Rebuilds the path of an entry relative to the root it is under.
    ///
    /// The root's own name is not included, so the path of a file directly
    /// in the root is just its name. This is what `fd` prints when it is
    /// given a root, and it is what the matcher scores.
    ///
    /// The root's prefix is the same for every entry under it, so
    /// searching it is wasted work: it is the same characters for all of
    /// them, and no user ever types the home directory to find a file.
    pub fn relative_path(&self, id: EntryId) -> Option<String> {
        self.relative_path_with_root(id).map(|(path, _)| path)
    }

    /// Rebuilds the relative path of an entry and says which root it is
    /// under.
    ///
    /// The root comes back as its position in [`Index::config_roots`], so
    /// that the absolute path can be rebuilt later without walking the
    /// parent chain a second time. It is `None` when the chain does not
    /// end at a known root, which is what happens to an index that was
    /// built without one.
    pub fn relative_path_with_root(&self, id: EntryId) -> Option<(String, Option<usize>)> {
        let mut parts: Vec<&str> = Vec::new();
        let mut current = self.entries.get(id as usize)?;
        let mut root_id = id;

        loop {
            let name = self.names.get(current.name)?;
            parts.push(name);
            match current.parent {
                Some(parent) => {
                    root_id = parent;
                    current = self.entries.get(parent as usize)?;
                }
                None => break,
            }
        }

        parts.reverse();

        // The root's own name is dropped, because the relative path starts
        // below the root.
        let root = self.roots.iter().position(|&root| root == root_id);
        let start = if root.is_some() { 1 } else { 0 };

        Some((parts[start..].join("/"), root))
    }

    /// Rebuilds the full path of an entry by walking up to its root.
    ///
    /// Returns `None` if the entry's parent chain is broken.
    pub fn full_path(&self, id: EntryId) -> Option<PathBuf> {
        let mut parts: Vec<&str> = Vec::new();
        let mut current = self.entries.get(id as usize)?;
        let mut root_id = id;

        loop {
            let name = self.names.get(current.name)?;
            parts.push(name);
            match current.parent {
                Some(parent) => {
                    root_id = parent;
                    current = self.entries.get(parent as usize)?;
                }
                None => break,
            }
        }

        parts.reverse();

        // The first part is the root's own name, which is only the last
        // component of the root path. The absolute root path is used
        // instead, so that the result can be opened directly.
        let base = self
            .roots
            .iter()
            .position(|&root| root == root_id)
            .and_then(|position| self.config_roots.get(position));

        let mut path = match base {
            Some(base) => base.clone(),
            None => PathBuf::new(),
        };

        // The root's name is skipped when the absolute base is known,
        // because the base already ends with it.
        let start = if base.is_some() { 1 } else { 0 };
        for part in &parts[start..] {
            path.push(part);
        }

        Some(path)
    }

    /// Rebuilds the full path of every entry, keyed by entry id.
    ///
    /// This is the hot path for query execution: the matcher needs the text
    /// to match against, and building it per query would dominate the
    /// response time.
    pub fn full_paths(&self) -> HashMap<EntryId, PathBuf> {
        let mut paths = HashMap::with_capacity(self.entries.len());
        for id in 0..self.entries.len() as u32 {
            if let Some(path) = self.full_path(id) {
                paths.insert(id, path);
            }
        }
        paths
    }

    /// The approximate memory used by the index, in bytes.
    pub fn memory_usage(&self) -> usize {
        let entries = self.entries.len() * std::mem::size_of::<FileEntry>();
        let lookup = self.names.lookup.len() * 48;
        entries + lookup + self.names.memory_usage()
    }

    /// Marks the index as updated now.
    pub fn touch(&mut self) {
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
    }

    /// A map from full path to entry id.
    ///
    /// The map is rebuilt whenever the index changes, and is what makes
    /// incremental updates possible: a filesystem event carries a path, and
    /// the path is looked up here.
    pub fn path_map(&self) -> HashMap<PathBuf, EntryId> {
        let mut map = HashMap::with_capacity(self.entries.len());
        for id in 0..self.entries.len() as u32 {
            if let Some(path) = self.full_path(id) {
                map.insert(path, id);
            }
        }
        map
    }

    /// Looks up an entry by its full path.
    pub fn find_by_path(&self, path: &Path) -> Option<EntryId> {
        let name = path.file_name()?.to_string_lossy().to_string();
        let parent_path = path.parent()?;

        let parent_id = self.find_dir_by_path(parent_path)?;

        self.entries
            .iter()
            .position(|e| {
                !e.removed
                    && e.parent == Some(parent_id)
                    && self.names.get(e.name) == Some(name.as_str())
            })
            .map(|i| i as EntryId)
    }

    /// Resolves a path to the root it lies under and the parts below it.
    ///
    /// Everything the index holds hangs off one of its roots, and a path
    /// outside them all does not belong in it.
    ///
    /// This is what anchors a path handed in from the outside — by the
    /// watcher — to the tree the scan built. Walking from the first
    /// component instead never matched the root entry, because the first
    /// component has no parent to be a child of. The effect was that
    /// [`Index::find_dir_by_path`] never found anything, so a deleted file
    /// was never removed, and [`Index::ensure_dir`] built a fresh chain of
    /// ancestors for every path it was given, so the index grew without
    /// bound and showed the same file twice.
    ///
    /// The longest matching root wins, so a root nested inside another
    /// resolves to the nearer one.
    fn resolve_root(&self, path: &Path) -> Option<(EntryId, Vec<String>)> {
        let mut best: Option<(usize, EntryId, Vec<String>)> = None;

        for (position, root) in self.config_roots.iter().enumerate() {
            let Some(&id) = self.roots.get(position) else {
                continue;
            };
            let Ok(rest) = path.strip_prefix(root) else {
                continue;
            };

            let depth = root.components().count();
            if best.as_ref().is_some_and(|(best, _, _)| *best >= depth) {
                continue;
            }

            let parts = rest
                .components()
                .filter_map(|component| match component {
                    std::path::Component::Normal(part) => {
                        Some(part.to_string_lossy().to_string())
                    }
                    _ => None,
                })
                .collect();

            best = Some((depth, id, parts));
        }

        best.map(|(_, id, parts)| (id, parts))
    }

    /// Finds the id of the directory at `path`, if it is indexed.
    fn find_dir_by_path(&self, path: &Path) -> Option<EntryId> {
        let (root, parts) = self.resolve_root(path)?;

        let mut current = root;
        for name in &parts {
            let index = self.entries.iter().position(|e| {
                !e.removed
                    && e.parent == Some(current)
                    && e.kind == EntryKind::Directory
                    && self.names.get(e.name) == Some(name.as_str())
            })?;
            current = index as EntryId;
        }

        Some(current)
    }

    /// Finds the id of the directory at `path`, creating it if needed.
    ///
    /// The path is walked from the deepest known ancestor downwards, so only
    /// the missing tail of the path has to be created.
    pub fn ensure_dir(&mut self, path: &Path) -> Option<EntryId> {
        let (root, parts) = self.resolve_root(path)?;

        let mut current = root;
        for name in parts {
            let existing = self.entries.iter().position(|e| {
                !e.removed
                    && e.parent == Some(current)
                    && e.kind == EntryKind::Directory
                    && self.names.get(e.name) == Some(name.as_str())
            });

            current = match existing {
                Some(id) => id as EntryId,
                None => self.add_named(Some(current), &name, EntryKind::Directory, 0, 0),
            };
        }

        Some(current)
    }

    /// Adds or replaces an entry at `path`.
    ///
    /// Returns the id of the entry, whether it was newly added or updated.
    pub fn upsert(&mut self, path: &Path, kind: EntryKind, size: u64, modified: i64) -> Option<EntryId> {
        let name = path.file_name()?.to_string_lossy().to_string();
        let parent_path = path.parent()?;
        let parent = self.ensure_dir(parent_path)?;

        // An existing entry with the same name and parent is replaced.
        let existing = self.entries.iter().position(|e| {
            !e.removed
                && e.parent == Some(parent)
                && self.names.get(e.name) == Some(name.as_str())
        });

        match existing {
            Some(index) => {
                let id = index as EntryId;
                let entry = &mut self.entries[index];

                // An entry that already holds what the watcher reports has
                // not changed. Saying that it has costs a published
                // snapshot, and the watcher reports plenty of writes that
                // leave the entry exactly as it was.
                let changed = entry.kind != kind
                    || entry.size != size
                    || entry.modified != modified
                    || entry.removed;

                entry.kind = kind;
                entry.size = size;
                entry.modified = modified;
                entry.removed = false;

                if changed {
                    self.dirty.push(id);
                }

                Some(id)
            }
            None => Some(self.add_named(Some(parent), &name, kind, size, modified)),
        }
    }

    /// Marks the entry at `path` as removed.
    ///
    /// The entry is tombstoned rather than deleted, so that the ids of the
    /// other entries stay stable.
    pub fn remove(&mut self, path: &Path) -> bool {
        let Some(id) = self.find_by_path(path) else {
            return false;
        };

        // Removing a directory removes everything below it.
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let entry = &mut self.entries[current as usize];
            if entry.removed {
                continue;
            }
            entry.removed = true;
            self.removed += 1;
            self.dirty.push(current);

            let children: Vec<EntryId> = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.parent == Some(current) && !e.removed)
                .map(|(i, _)| i as EntryId)
                .collect();
            stack.extend(children);
        }

        true
    }

    /// The number of entries that have not been removed.
    pub fn live_len(&self) -> usize {
        self.entries.iter().filter(|e| !e.removed).count()
    }

    /// Removes the tombstoned entries and renumbers the surviving ones.
    ///
    /// The ids handed out to the UI become invalid after this, so it is only
    /// called when the whole corpus is rebuilt.
    pub fn compact(&mut self) {
        let mut keep = Vec::with_capacity(self.entries.len());
        let mut remap = vec![u32::MAX; self.entries.len()];

        for (index, entry) in self.entries.iter().enumerate() {
            if entry.removed {
                continue;
            }
            remap[index] = keep.len() as u32;
            keep.push(entry.clone());
        }

        for entry in &mut keep {
            if let Some(parent) = entry.parent {
                entry.parent = Some(remap[parent as usize]);
            }
        }

        self.entries = keep;
        self.removed = 0;

        // The ids have all moved, so a pending change now names the wrong
        // entry. Compaction is only done when the whole corpus is rebuilt
        // anyway, which needs no pending list.
        self.dirty.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_interned() {
        let mut pool = NamePool::new();
        let a = pool.intern("foo");
        let b = pool.intern("bar");
        let c = pool.intern("foo");
        assert_eq!(a, c);
        assert_ne!(a, b);
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.get(a), Some("foo"));
    }

    #[test]
    fn full_path_is_rebuilt_from_the_chain() {
        let mut index = Index::new();
        let root = index.add_named(None, "home", EntryKind::Directory, 0, 0);
        let dir = index.add_named(Some(root), "user", EntryKind::Directory, 0, 0);
        let file = index.add_named(Some(dir), "notes.txt", EntryKind::File, 10, 0);
        assert_eq!(
            index.full_path(file),
            Some(PathBuf::from("home/user/notes.txt"))
        );
    }

    #[test]
    fn names_are_shared_between_entries() {
        let mut index = Index::new();
        let a = index.add_named(None, "same", EntryKind::File, 0, 0);
        let b = index.add_named(None, "same", EntryKind::File, 0, 0);
        assert_eq!(
            index.get(a).unwrap().name,
            index.get(b).unwrap().name,
            "identical names should share one pool slot"
        );
        assert_eq!(index.names.len(), 1);
    }
}
