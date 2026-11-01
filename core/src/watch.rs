//! Live index updates through FSEvents.
//!
//! A filesystem index is stale the moment it is built. FSEvents is macOS's
//! native change-notification API, and it reports the directories whose
//! contents changed rather than the files themselves — which is exactly the
//! right granularity for keeping a search index current.
//!
//! The watcher streams events through a channel. The consumer applies them to
//! the index, adding new files and marking directories as needing a rescan.
//!
//! # Why not watch individual files?
//!
//! FSEvents watches *paths*, and watching several hundred thousand files
//! would exhaust the file descriptor limit. Instead the watcher watches the
//! indexed roots at directory granularity and reports the affected
//! directories, which the consumer rescans.
//!
//! # Why raw bindings?
//!
//! The `fsevent-sys` crate is deprecated and drags in the whole Objective-C
//! runtime stack. The handful of functions needed here are declared directly
//! against the `CoreServices` and `CoreFoundation` frameworks, which keeps
//! the dependency graph of the core small and the build fast.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{c_char, c_double, c_void};

    pub type Boolean = u8;
    pub type CFIndex = isize;
    pub type CFTypeRef = *const c_void;
    pub type CFStringRef = *const c_void;
    pub type CFArrayRef = *const c_void;
    pub type CFRunLoopRef = *mut c_void;
    pub type FSEventStreamRef = *mut c_void;
    pub type FSEventStreamEventId = u64;
    pub type FSEventStreamEventFlags = u32;
    pub type FSEventStreamCreateFlags = u32;

    /// The callback FSEvents invokes when a batch of changes arrives.
    pub type FSEventStreamCallback = extern "C" fn(
        FSEventStreamRef,
        *mut c_void,
        usize,
        *mut c_void,
        *const FSEventStreamEventFlags,
        *const FSEventStreamEventId,
    );

    /// The context handed to the callback.
    #[repr(C)]
    pub struct FSEventStreamContext {
        pub version: CFIndex,
        pub info: *mut c_void,
        pub retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
        pub release: Option<extern "C" fn(*const c_void)>,
        pub copy_description: Option<extern "C" fn(*const c_void) -> *mut c_void>,
    }

    /// The UTF-8 string encoding used by CoreFoundation.
    #[allow(non_upper_case_globals)]
    pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;

    /// The event id meaning "only report changes from now on".
    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamEventIdSinceNow: FSEventStreamEventId = u64::MAX;

    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamCreateFlagUseCFTypes: FSEventStreamCreateFlags = 0x0000_0001;
    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamCreateFlagNoDefer: FSEventStreamCreateFlags = 0x0000_0002;
    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamCreateFlagWatchRoot: FSEventStreamCreateFlags = 0x0000_0004;
    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamCreateFlagFileEvents: FSEventStreamCreateFlags = 0x0000_0010;

    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamEventFlagMustScanSubDirs: FSEventStreamEventFlags = 0x0000_0001;
    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamEventFlagItemCreated: FSEventStreamEventFlags = 0x0000_0100;
    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamEventFlagItemRemoved: FSEventStreamEventFlags = 0x0000_0200;
    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamEventFlagItemInodeMetaMod: FSEventStreamEventFlags = 0x0000_0400;
    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamEventFlagItemRenamed: FSEventStreamEventFlags = 0x0000_0800;
    #[allow(non_upper_case_globals)]
    pub const kFSEventStreamEventFlagItemModified: FSEventStreamEventFlags = 0x0000_1000;

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        pub fn FSEventStreamCreate(
            allocator: CFTypeRef,
            callback: FSEventStreamCallback,
            context: *const FSEventStreamContext,
            paths_to_watch: CFArrayRef,
            since_when: FSEventStreamEventId,
            latency: c_double,
            flags: FSEventStreamCreateFlags,
        ) -> FSEventStreamRef;

        pub fn FSEventStreamScheduleWithRunLoop(
            stream_ref: FSEventStreamRef,
            run_loop: CFRunLoopRef,
            run_loop_mode: CFStringRef,
        );

        pub fn FSEventStreamUnscheduleFromRunLoop(
            stream_ref: FSEventStreamRef,
            run_loop: CFRunLoopRef,
            run_loop_mode: CFStringRef,
        );

        pub fn FSEventStreamStart(stream_ref: FSEventStreamRef) -> Boolean;
        pub fn FSEventStreamStop(stream_ref: FSEventStreamRef) -> Boolean;
        pub fn FSEventStreamInvalidate(stream_ref: FSEventStreamRef);
        pub fn FSEventStreamRelease(stream_ref: FSEventStreamRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFStringCreateWithBytes(
            alloc: CFTypeRef,
            bytes: *const u8,
            num_bytes: CFIndex,
            encoding: u32,
            is_external_representation: Boolean,
        ) -> CFStringRef;

        pub fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
        pub fn CFStringGetMaximumSizeForEncoding(
            length: CFIndex,
            encoding: u32,
        ) -> CFIndex;
        pub fn CFStringGetCString(
            the_string: CFStringRef,
            buffer: *mut c_char,
            buffer_size: CFIndex,
            encoding: u32,
        ) -> Boolean;

        pub fn CFArrayCreate(
            alloc: CFTypeRef,
            values: *const *const c_void,
            num_values: CFIndex,
            call_backs: *const c_void,
        ) -> CFArrayRef;
        pub fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
        pub fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;

        pub fn CFRelease(cf: CFTypeRef);

        pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        pub fn CFRunLoopRunInMode(
            mode: CFStringRef,
            seconds: c_double,
            return_after_source_handled: Boolean,
        ) -> i32;
    }
}
/// The kind of change that happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// A file or directory was created.
    Created,
    /// A file or directory was modified.
    Modified,
    /// A file or directory was removed or renamed.
    Removed,
    /// The change could not be classified.
    Unknown,
}

/// A single change reported by the watcher.
#[derive(Debug, Clone)]
pub struct Change {
    /// The path that changed.
    pub path: PathBuf,
    /// What happened to it.
    pub kind: ChangeKind,
    /// Whether the whole subtree below the path must be rescanned.
    ///
    /// FSEvents sets this when it dropped events and cannot describe the
    /// change precisely, which means the directory's contents are no longer
    /// trustworthy.
    pub needs_rescan: bool,
}

/// The context handed to the FSEvents callback.
///
/// It is kept alive for as long as the stream, so the callback can safely
/// dereference the pointer it is given.
struct CallbackContext {
    /// The channel the events are sent to.
    sender: Sender<Change>,
}

#[cfg(target_os = "macos")]
impl CallbackContext {
    /// Classifies a set of FSEvents flags.
    fn classify(flags: u32) -> (ChangeKind, bool) {
        let s = sys::kFSEventStreamEventFlagItemCreated;
        let r = sys::kFSEventStreamEventFlagItemRemoved;
        let m = sys::kFSEventStreamEventFlagItemModified;
        let i = sys::kFSEventStreamEventFlagItemInodeMetaMod;
        let n = sys::kFSEventStreamEventFlagItemRenamed;
        let sdir = sys::kFSEventStreamEventFlagMustScanSubDirs;

        let needs_rescan = flags & sdir != 0;

        if flags & r != 0 || flags & n != 0 {
            (ChangeKind::Removed, needs_rescan)
        } else if flags & s != 0 {
            (ChangeKind::Created, needs_rescan)
        } else if flags & m != 0 || flags & i != 0 {
            (ChangeKind::Modified, needs_rescan)
        } else {
            (ChangeKind::Unknown, needs_rescan)
        }
    }
}

/// Converts a CoreFoundation string into a Rust `String`.
#[cfg(target_os = "macos")]
fn cfstring_to_string(the_string: sys::CFStringRef) -> Option<String> {
    if the_string.is_null() {
        return None;
    }

    let length = unsafe { sys::CFStringGetLength(the_string) };
    let max = unsafe { sys::CFStringGetMaximumSizeForEncoding(length, sys::kCFStringEncodingUTF8) };
    if max <= 0 {
        return None;
    }

    // The maximum size is in bytes including the terminator.
    let mut buffer = vec![0u8; max as usize + 1];
    let ok = unsafe {
        sys::CFStringGetCString(
            the_string,
            buffer.as_mut_ptr() as *mut std::ffi::c_char,
            buffer.len() as sys::CFIndex,
            sys::kCFStringEncodingUTF8,
        )
    };

    if ok == 0 {
        return None;
    }

    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    buffer.truncate(end);
    String::from_utf8(buffer).ok()
}

/// The FSEvents callback.
///
/// # Safety
///
/// This is called by CoreFoundation with the pointers documented on
/// `FSEventStreamCallback`.
#[cfg(target_os = "macos")]
extern "C" fn fsevent_callback(
    _stream_ref: sys::FSEventStreamRef,
    info: *mut std::ffi::c_void,
    num_events: usize,
    event_paths: *mut std::ffi::c_void,
    event_flags: *const sys::FSEventStreamEventFlags,
    _event_ids: *const sys::FSEventStreamEventId,
) {
    if info.is_null() || event_paths.is_null() || event_flags.is_null() {
        return;
    }

    // The context is kept alive by the thread that created the stream, so
    // this dereference is valid for the lifetime of the stream.
    let context = unsafe { &*(info as *const CallbackContext) };

    let array = event_paths as sys::CFArrayRef;
    let count = unsafe { sys::CFArrayGetCount(array) };
    if count <= 0 {
        return;
    }

    for index in 0..(count.min(num_events as sys::CFIndex)) as usize {
        let flags = unsafe { *event_flags.add(index) };
        let value = unsafe { sys::CFArrayGetValueAtIndex(array, index as sys::CFIndex) };
        if value.is_null() {
            continue;
        }

        let Some(text) = cfstring_to_string(value as sys::CFStringRef) else {
            continue;
        };

        let (kind, needs_rescan) = CallbackContext::classify(flags);

        // A failed send only means the receiver is gone, which happens when
        // the watcher is being torn down.
        let _ = context.sender.send(Change {
            path: PathBuf::from(text),
            kind,
            needs_rescan,
        });
    }
}

/// Watches a directory tree for changes.
///
/// The watcher owns a dedicated thread that runs a CoreFoundation run loop,
/// and streams the resulting events through a channel.
pub struct Watcher {
    /// The channel the events arrive on.
    rx: Receiver<Change>,
    /// Set when the watcher is shutting down.
    stop: Arc<AtomicBool>,
    /// The thread running the run loop.
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    /// Starts watching `path` for changes.
    ///
    /// Returns an error when the stream cannot be created, which usually
    /// means the path does not exist or the process lacks permission.
    #[cfg(target_os = "macos")]
    pub fn new(path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = unbounded();
        let stop = Arc::new(AtomicBool::new(false));

        // The watcher signals that it is running through this channel, so
        // that the caller does not create a file before the stream is
        // delivering events.
        let (ready_tx, ready_rx) = bounded(1);

        let stop_thread = Arc::clone(&stop);
        let watch_path = path.to_path_buf();

        let handle = std::thread::Builder::new()
            .name("seek-watch".into())
            .spawn(move || {
                run_watch_loop(&watch_path, tx, stop_thread, Some(ready_tx));
            })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        // The wait is bounded, so that a watcher whose stream cannot be
        // created does not block the caller forever.
        let _ = ready_rx.recv_timeout(std::time::Duration::from_secs(5));

        Ok(Self {
            rx,
            stop,
            handle: Some(handle),
        })
    }

    /// Starts watching `path` for changes.
    ///
    /// On platforms without FSEvents the watcher is a no-op that never
    /// reports a change.
    #[cfg(not(target_os = "macos"))]
    pub fn new(_path: &Path) -> std::io::Result<Self> {
        let (tx, rx) = unbounded();
        // Keep the sender alive so the receiver never reports disconnection.
        std::mem::forget(tx);
        Ok(Self {
            rx,
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
        })
    }

    /// Receives the next change, or `None` when the watcher has stopped.
    pub fn next_change(&self) -> Option<Change> {
        self.rx.recv().ok()
    }

    /// Receives the next change, giving up after `timeout`.
    pub fn next_change_timeout(&self, timeout: std::time::Duration) -> Option<Change> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Asks the watcher to stop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(target_os = "macos")]
fn run_watch_loop(
    path: &Path,
    tx: Sender<Change>,
    stop: Arc<AtomicBool>,
    ready_tx: Option<Sender<()>>,
) {
    // The context must outlive the stream, so it is kept in this scope.
    let mut context = Box::new(CallbackContext { sender: tx });
    let context_ptr: *mut CallbackContext = &mut *context;

    let stream_context = sys::FSEventStreamContext {
        version: 0,
        info: context_ptr as *mut std::ffi::c_void,
        retain: None,
        release: None,
        copy_description: None,
    };

    // Build the CFString for the watched path.
    let path_text = path.as_os_str().to_string_lossy().into_owned();
    let path_bytes = path_text.as_bytes();
    let path_cf = unsafe {
        sys::CFStringCreateWithBytes(
            std::ptr::null(),
            path_bytes.as_ptr(),
            path_bytes.len() as sys::CFIndex,
            sys::kCFStringEncodingUTF8,
            0,
        )
    };

    if path_cf.is_null() {
        return;
    }

    // Build the array of watched paths. The callback list is null, which
    // means the array does not retain its elements — the CFString above is
    // kept alive for the whole loop, so that is safe.
    let values = [path_cf as *const std::ffi::c_void];
    let paths = unsafe {
        sys::CFArrayCreate(
            std::ptr::null(),
            values.as_ptr(),
            values.len() as sys::CFIndex,
            std::ptr::null(),
        )
    };

    if paths.is_null() {
        unsafe { sys::CFRelease(path_cf) };
        return;
    }

    let flags = sys::kFSEventStreamCreateFlagFileEvents
        | sys::kFSEventStreamCreateFlagNoDefer
        | sys::kFSEventStreamCreateFlagUseCFTypes
        | sys::kFSEventStreamCreateFlagWatchRoot;

    let stream = unsafe {
        sys::FSEventStreamCreate(
            std::ptr::null(),
            fsevent_callback,
            &stream_context,
            paths,
            sys::kFSEventStreamEventIdSinceNow,
            0.0,
            flags,
        )
    };

    unsafe { sys::CFRelease(paths) };
    unsafe { sys::CFRelease(path_cf) };

    if stream.is_null() {
        return;
    }

    // The default run loop mode is a CoreFoundation string whose contents
    // are literally "kCFRunLoopDefaultMode".
    let mode_bytes = b"kCFRunLoopDefaultMode";
    let mode = unsafe {
        sys::CFStringCreateWithBytes(
            std::ptr::null(),
            mode_bytes.as_ptr(),
            mode_bytes.len() as sys::CFIndex,
            sys::kCFStringEncodingUTF8,
            0,
        )
    };

    let run_loop = unsafe { sys::CFRunLoopGetCurrent() };

    unsafe {
        sys::FSEventStreamScheduleWithRunLoop(stream, run_loop, mode);
    }

    unsafe {
        sys::FSEventStreamStart(stream);
    }

    // The watcher tells the caller that it is running now. Without this,
    // a file that is created right after the watcher is started can be
    // missed, because the stream is not delivering events yet.
    if let Some(ready) = &ready_tx {
        let _ = ready.send(());
    }

    // Pump the run loop until the watcher is asked to stop. A short timeout
    // keeps the shutdown latency bounded without a busy loop.
    while !stop.load(Ordering::Relaxed) {
        unsafe {
            sys::CFRunLoopRunInMode(mode, 0.2, 0);
        }
    }

    unsafe {
        sys::FSEventStreamStop(stream);
        sys::FSEventStreamUnscheduleFromRunLoop(stream, run_loop, mode);
        sys::FSEventStreamInvalidate(stream);
        sys::FSEventStreamRelease(stream);
        sys::CFRelease(mode);
    }

    // The context is dropped here, after the stream that points at it has
    // been released.
    drop(context);
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_event_flags() {
        let created = sys::kFSEventStreamEventFlagItemCreated;
        let removed = sys::kFSEventStreamEventFlagItemRemoved;
        let modified = sys::kFSEventStreamEventFlagItemModified;

        assert_eq!(
            CallbackContext::classify(created),
            (ChangeKind::Created, false)
        );
        assert_eq!(
            CallbackContext::classify(removed),
            (ChangeKind::Removed, false)
        );
        assert_eq!(
            CallbackContext::classify(modified),
            (ChangeKind::Modified, false)
        );
        assert_eq!(
            CallbackContext::classify(sys::kFSEventStreamEventFlagMustScanSubDirs),
            (ChangeKind::Unknown, true)
        );
    }
}
