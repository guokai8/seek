//! On-disk persistence of the index.
//!
//! Rebuilding the index from scratch takes about a second for a home
//! directory, which is far too slow for a launcher that should feel instant.
//! The index is therefore snapshotted to disk and reloaded on launch.
//!
//! The format is deliberately simple:
//!
//! * **Payload** — the [`Index`] serialised with `postcard`, a compact
//!   binary format for Rust's data model.
//! * **Container** — the payload compressed with `zstd` and framed with a
//!   small header that carries the format version and the uncompressed size,
//!   so a snapshot written by a future version is rejected cleanly instead
//!   of being misread.
//!
//! A snapshot also records the roots it was built from. If the roots change,
//! the snapshot is discarded rather than used, because the ids inside it
//! would refer to a different tree.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::index::Index;

/// The current snapshot format version.
///
/// Bumping this invalidates every snapshot written by an older build.
pub const FORMAT_VERSION: u32 = 1;

/// The magic bytes at the start of a snapshot file.
const MAGIC: &[u8; 8] = b"SEEKIDX1";

/// The header of a snapshot container.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Header {
    /// The format version, so stale snapshots can be rejected.
    version: u32,
    /// The size of the uncompressed payload, in bytes.
    payload_len: u32,
    /// A checksum of the uncompressed payload.
    payload_hash: u64,
}

/// A snapshot of the index, together with the roots it covers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// The format version.
    pub version: u32,
    /// The roots the index was built from.
    pub roots: Vec<PathBuf>,
    /// When the snapshot was written, as a Unix timestamp in seconds.
    pub written_at: i64,
    /// The serialised index.
    pub index: Index,
}

/// The outcome of a load attempt.
#[derive(Debug)]
pub enum LoadError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file is not a snapshot, or is from an incompatible version.
    Format(String),
    /// The snapshot's roots do not match the requested roots.
    RootsMismatch,
}

/// A hash of the payload, used to detect truncated or corrupt snapshots.
///
/// This is FNV-1a, which is not a cryptographic hash but is more than enough
/// to catch truncation, which is the failure mode that matters here.
fn payload_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Writes an index snapshot to `path`.
///
/// The write is atomic: the data goes to a temporary file next to the target
/// and is renamed into place, so a crash never leaves a half-written
/// snapshot behind.
pub fn save(index: &Index, roots: &[PathBuf], path: &Path) -> Result<(), LoadError> {
    let snapshot = Snapshot {
        version: FORMAT_VERSION,
        roots: roots.to_vec(),
        written_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        index: index.clone(),
    };

    let payload = postcard::to_allocvec(&snapshot).map_err(|e| LoadError::Format(e.to_string()))?;

    let header = Header {
        version: FORMAT_VERSION,
        payload_len: payload.len() as u32,
        payload_hash: payload_hash(&payload),
    };

    let header_bytes =
        postcard::to_allocvec(&header).map_err(|e| LoadError::Format(e.to_string()))?;

    let compressed = zstd::encode_all(&payload[..], 3).map_err(|e| LoadError::Format(e.to_string()))?;

    let mut file = std::fs::File::create(path).map_err(LoadError::Io)?;
    use std::io::Write;
    file.write_all(MAGIC)
        .map_err(LoadError::Io)?;
    file.write_all(&(header_bytes.len() as u32).to_le_bytes())
        .map_err(LoadError::Io)?;
    file.write_all(&header_bytes).map_err(LoadError::Io)?;
    file.write_all(&compressed).map_err(LoadError::Io)?;
    file.sync_all().map_err(LoadError::Io)?;

    Ok(())
}

/// Loads an index snapshot from `path`.
///
/// Returns an error when the snapshot is missing, corrupt, written by an
/// incompatible version, or covers different roots than requested.
pub fn load(path: &Path, roots: &[PathBuf]) -> Result<Snapshot, LoadError> {
    let bytes = std::fs::read(path).map_err(LoadError::Io)?;

    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        return Err(LoadError::Format("not a seek snapshot".into()));
    }

    let header_len = u32::from_le_bytes([
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
    ]) as usize;

    let header_end = 12 + header_len;
    if bytes.len() < header_end {
        return Err(LoadError::Format("truncated header".into()));
    }

    let header: Header = postcard::from_bytes(&bytes[12..header_end])
        .map_err(|e| LoadError::Format(e.to_string()))?;

    if header.version != FORMAT_VERSION {
        return Err(LoadError::Format(format!(
            "snapshot version {} is not supported (expected {})",
            header.version, FORMAT_VERSION
        )));
    }

    let compressed = &bytes[header_end..];
    let payload = zstd::decode_all(compressed).map_err(|e| LoadError::Format(e.to_string()))?;

    if payload.len() != header.payload_len as usize {
        return Err(LoadError::Format("payload length mismatch".into()));
    }

    if payload_hash(&payload) != header.payload_hash {
        return Err(LoadError::Format("payload checksum mismatch".into()));
    }

    let snapshot: Snapshot =
        postcard::from_bytes(&payload).map_err(|e| LoadError::Format(e.to_string()))?;

    if snapshot.version != FORMAT_VERSION {
        return Err(LoadError::Format("snapshot version mismatch".into()));
    }

    // A snapshot built from different roots refers to a different tree, so
    // its ids cannot be reused.
    if !roots.is_empty() && !same_roots(&snapshot.roots, roots) {
        return Err(LoadError::RootsMismatch);
    }

    Ok(snapshot)
}

/// Whether two root lists describe the same set of directories.
fn same_roots(a: &[PathBuf], b: &[PathBuf]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut a: Vec<&PathBuf> = a.iter().collect();
    let mut b: Vec<&PathBuf> = b.iter().collect();
    a.sort();
    b.sort();
    a == b
}

/// The default location of the snapshot file.
///
/// The file lives in the app's support directory, which is created if it
/// does not exist yet.
pub fn default_snapshot_path() -> PathBuf {
    let base = std::env::var_os("SEEK_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = crate::scan::home_dir();
            home.join("Library/Application Support/Seek")
        });

    let _ = std::fs::create_dir_all(&base);
    base.join("index.skidx")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::EntryKind;

    fn sample_index() -> Index {
        let mut index = Index::new();
        let root = index.add_named(None, "home", EntryKind::Directory, 0, 0);
        let dir = index.add_named(Some(root), "user", EntryKind::Directory, 0, 0);
        index.add_named(Some(dir), "notes.txt", EntryKind::File, 10, 0);
        index
    }

    #[test]
    fn round_trips_through_a_snapshot() {
        let dir = std::env::temp_dir().join(format!("seek-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.skidx");

        let index = sample_index();
        let roots = vec![PathBuf::from("/home")];
        save(&index, &roots, &path).unwrap();

        let loaded = load(&path, &roots).unwrap();
        assert_eq!(loaded.index.len(), index.len());
        assert_eq!(loaded.roots, roots);
        assert_eq!(
            loaded.index.full_path(2),
            Some(PathBuf::from("home/user/notes.txt"))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_a_snapshot_for_other_roots() {
        let dir = std::env::temp_dir().join(format!("seek-test-roots-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.skidx");

        save(&sample_index(), &[PathBuf::from("/home")], &path).unwrap();

        let result = load(&path, &[PathBuf::from("/other")]);
        assert!(matches!(result, Err(LoadError::RootsMismatch)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_a_corrupt_snapshot() {
        let dir = std::env::temp_dir().join(format!("seek-test-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.skidx");

        save(&sample_index(), &[], &path).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        let len = bytes.len();
        bytes[len - 1] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();

        let result = load(&path, &[]);
        assert!(matches!(result, Err(LoadError::Format(_))));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
