//! The walk must not descend into a filesystem mounted inside a root.
//!
//! A launcher indexes the user's own disk. Whatever happens to be mounted
//! inside it today — a network share, a disk image, a removable drive, a
//! file provider's volume — is not part of that, and reaching into it is
//! how the index got wedged: a dead NFS mount blocked `opendir()` for
//! fifty-four minutes, which left the panel saying "Indexing…" the whole
//! time and the other walker threads spinning at 1ms.
//!
//! These tests mount a real disk image inside the root, because the bug is
//! about a filesystem boundary and nothing short of one exercises it.
//! `hdiutil` needs no privileges for this.

use std::path::{Path, PathBuf};
use std::process::Command;

use seek_core::scan::{scan_blocking, ScanConfig};

/// A disk image attached inside a directory, detached when dropped.
///
/// The detach runs in `Drop` so that a failing assertion does not leave a
/// volume mounted in the developer's `/var/folders`.
struct MountedImage {
    image: PathBuf,
    mountpoint: PathBuf,
}

impl MountedImage {
    /// Attaches a small image at `mountpoint`, or returns `None` when the
    /// machine cannot do it (no `hdiutil`, no permission, no space).
    fn attach(image: PathBuf, mountpoint: PathBuf) -> Option<Self> {
        std::fs::create_dir_all(&mountpoint).ok()?;

        let created = Command::new("hdiutil")
            .args(["create", "-size", "8m", "-fs", "APFS", "-volname", "flfs"])
            .arg(&image)
            .output()
            .ok()?;
        if !created.status.success() {
            return None;
        }

        let attached = Command::new("hdiutil")
            .arg("attach")
            .arg(&image)
            .arg("-mountpoint")
            .arg(&mountpoint)
            .args(["-nobrowse", "-noautoopen"])
            .output()
            .ok()?;
        if !attached.status.success() {
            return None;
        }

        Some(Self { image, mountpoint })
    }
}

impl Drop for MountedImage {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .arg("detach")
            .arg(&self.mountpoint)
            .arg("-force")
            .output();
        let _ = std::fs::remove_file(&self.image);
    }
}

/// Builds a root holding one ordinary file and a mounted volume holding
/// another, and returns the mount guard.
fn fixture(root: &Path) -> Option<MountedImage> {
    std::fs::write(root.join("on_root_volume.txt"), b"x").unwrap();

    let mount = MountedImage::attach(root.join("image.dmg"), root.join("mnt"))?;
    std::fs::write(mount.mountpoint.join("on_other_volume.txt"), b"x").unwrap();

    Some(mount)
}

/// A fresh root under the temp directory, named after the test.
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("seek-ofs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // The mount table reports `/private/var`, not `/var`.
    root.canonicalize().unwrap()
}

fn names(config: &ScanConfig) -> Vec<String> {
    let (entries, _) = scan_blocking(config);
    entries.iter().map(|e| e.file_name.clone()).collect()
}

#[test]
fn the_walk_stops_at_a_mounted_volume() {
    let root = temp_root("stops");

    let Some(_mount) = fixture(&root) else {
        eprintln!("skipped: cannot attach a disk image here");
        return;
    };

    let config = ScanConfig {
        roots: vec![root.clone()],
        ..Default::default()
    };

    let found = names(&config);

    assert!(
        found.iter().any(|n| n == "on_root_volume.txt"),
        "the root's own files are still indexed: {found:?}"
    );
    assert!(
        !found.iter().any(|n| n == "on_other_volume.txt"),
        "the walk descended into the mounted volume: {found:?}"
    );
    // The mountpoint itself is a directory in the root, so it is indexed —
    // the walk simply does not go through it.
    assert!(
        found.iter().any(|n| n == "mnt"),
        "the mountpoint itself should still be an entry: {found:?}"
    );

    drop(_mount);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn crossing_filesystems_can_be_asked_for() {
    let root = temp_root("crosses");

    let Some(_mount) = fixture(&root) else {
        eprintln!("skipped: cannot attach a disk image here");
        return;
    };

    let config = ScanConfig {
        roots: vec![root.clone()],
        same_file_system: false,
        ..Default::default()
    };

    let found = names(&config);

    // Without this the test above proves nothing: it would pass just as
    // well if the mounted volume were empty or the fixture were broken.
    assert!(
        found.iter().any(|n| n == "on_other_volume.txt"),
        "the file on the mounted volume exists and is reachable: {found:?}"
    );

    drop(_mount);
    let _ = std::fs::remove_dir_all(&root);
}
