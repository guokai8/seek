//! The persistence part of the C ABI.
//!
//! These are kept apart from [`crate::ffi`] because they deal with the
//! filesystem rather than with searches, and because they have a different
//! lifetime model: a snapshot outlives many searches.

use std::ffi::{c_char, c_int, CStr, CString};
use std::path::PathBuf;

use crate::persist;

/// Loads an index snapshot from `path` and installs it in the engine.
///
/// The roots of the snapshot must match the engine's roots, otherwise the
/// snapshot is rejected: its entry ids refer to a different tree.
///
/// Returns the number of entries that were loaded, or -1 on failure.
///
/// # Safety
///
/// `engine` must be a valid engine and `path` a valid null-terminated string.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_load_snapshot(
    engine: *mut crate::ffi::Engine,
    path: *const c_char,
) -> c_int {
    if engine.is_null() || path.is_null() {
        return -1;
    }
    let Ok(text) = CStr::from_ptr(path).to_str() else {
        return -1;
    };

    let eng = &*engine;

    // The snapshot is read outside the lock, because it can be a few
    // megabytes and the compression takes a moment.
    let snapshot = match persist::load(&PathBuf::from(text), &eng.read().roots) {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let count = snapshot.index.len() as c_int;

    let mut inner = eng.write();
    inner.index = snapshot.index;
    eng.publish(&mut inner.index);

    count
}

/// Writes the engine's index to `path` as a snapshot.
///
/// Returns 0 on success, -1 on failure.
///
/// # Safety
///
/// `engine` must be a valid engine and `path` a valid null-terminated string.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_save_snapshot(
    engine: *mut crate::ffi::Engine,
    path: *const c_char,
) -> c_int {
    if engine.is_null() || path.is_null() {
        return -1;
    }
    let Ok(text) = CStr::from_ptr(path).to_str() else {
        return -1;
    };

    let eng = &*engine;
    let inner = eng.read();

    match persist::save(&inner.index, &inner.roots, &PathBuf::from(text)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// The default path of the snapshot file, as a newly allocated C string.
///
/// The caller owns the string and must free it with [`crate::ffi::sk_string_free`].
///
/// # Safety
///
/// The returned pointer must be freed by the caller.
#[no_mangle]
pub unsafe extern "C" fn sk_engine_default_snapshot_path() -> *mut c_char {
    match CString::new(persist::default_snapshot_path().to_string_lossy().as_bytes()) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Whether a snapshot file exists at `path`.
///
/// Returns 1 when it does, 0 when it does not, -1 on failure.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
#[no_mangle]
pub unsafe extern "C" fn sk_snapshot_exists(path: *const c_char) -> c_int {
    if path.is_null() {
        return -1;
    }
    let Ok(text) = CStr::from_ptr(path).to_str() else {
        return -1;
    };
    match std::fs::metadata(PathBuf::from(text)) {
        Ok(meta) if meta.is_file() => 1,
        Ok(_) => 0,
        Err(_) => 0,
    }
}
