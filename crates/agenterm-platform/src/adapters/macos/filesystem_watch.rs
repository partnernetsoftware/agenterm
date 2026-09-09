//! macOS FSEvents-backed direct-child directory watch.

use std::collections::BTreeSet;
use std::ffi::{CStr, OsStr, OsString};
use std::os::raw::{c_char, c_double, c_void};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use crate::filesystem_watch::{
    FilesystemWatchError, FilesystemWatchErrorKind, FilesystemWatchEvent, FilesystemWatchResult,
};

type CfIndex = isize;
type CfAllocatorRef = *const c_void;
type CfArrayRef = *const c_void;
type CfRunLoopRef = *mut c_void;
type CfStringRef = *const c_void;
type FseventStreamRef = *mut c_void;
type FseventStreamEventFlags = u32;
type FseventStreamEventId = u64;

const MAX_DURATION_MS: u64 = 86_400_000;
const EVENT_ID_SINCE_NOW: FseventStreamEventId = u64::MAX;
const UTF8_ENCODING: u32 = 0x0800_0100;
const STREAM_FLAGS: u32 = 0x0000_0002 | 0x0000_0004 | 0x0000_0010; // NoDefer | WatchRoot | FileEvents

const MUST_SCAN_SUBDIRS: u32 = 0x0000_0001;
const USER_DROPPED: u32 = 0x0000_0002;
const KERNEL_DROPPED: u32 = 0x0000_0004;
const EVENT_IDS_WRAPPED: u32 = 0x0000_0008;
const HISTORY_DONE: u32 = 0x0000_0010;
const ROOT_CHANGED: u32 = 0x0000_0020;
const MOUNT: u32 = 0x0000_0040;
const UNMOUNT: u32 = 0x0000_0080;
const ITEM_CREATED: u32 = 0x0000_0100;
const ITEM_REMOVED: u32 = 0x0000_0200;
const ITEM_INODE_META_MOD: u32 = 0x0000_0400;
const ITEM_RENAMED: u32 = 0x0000_0800;
const ITEM_MODIFIED: u32 = 0x0000_1000;
const ITEM_FINDER_INFO_MOD: u32 = 0x0000_2000;
const ITEM_CHANGE_OWNER: u32 = 0x0000_4000;
const ITEM_XATTR_MOD: u32 = 0x0000_8000;
const ITEM_IS_FILE: u32 = 0x0001_0000;
const ITEM_IS_DIR: u32 = 0x0002_0000;
const ITEM_IS_SYMLINK: u32 = 0x0004_0000;
const ITEM_OWN_EVENT: u32 = 0x0008_0000;
const ITEM_IS_HARDLINK: u32 = 0x0010_0000;
const ITEM_IS_LAST_HARDLINK: u32 = 0x0020_0000;
const ITEM_CLONED: u32 = 0x0040_0000;

#[repr(C)]
struct FseventStreamContext {
    version: CfIndex,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void)>,
    copy_description: Option<unsafe extern "C" fn(*const c_void) -> CfStringRef>,
}

type FseventStreamCallback = unsafe extern "C" fn(
    FseventStreamRef,
    *mut c_void,
    usize,
    *mut c_void,
    *const FseventStreamEventFlags,
    *const FseventStreamEventId,
);

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CfStringRef;
    static kCFTypeArrayCallBacks: u8;

    fn CFArrayCreate(
        allocator: CfAllocatorRef,
        values: *const *const c_void,
        count: CfIndex,
        callbacks: *const c_void,
    ) -> CfArrayRef;
    fn CFRelease(object: *const c_void);
    fn CFRunLoopGetCurrent() -> CfRunLoopRef;
    fn CFRunLoopRunInMode(mode: CfStringRef, seconds: c_double, return_after_source: u8) -> i32;
    fn CFRunLoopStop(run_loop: CfRunLoopRef);
    fn CFStringCreateWithBytes(
        allocator: CfAllocatorRef,
        bytes: *const u8,
        length: CfIndex,
        encoding: u32,
        external_representation: u8,
    ) -> CfStringRef;
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: CfAllocatorRef,
        callback: FseventStreamCallback,
        context: *mut FseventStreamContext,
        paths_to_watch: CfArrayRef,
        since_when: FseventStreamEventId,
        latency: c_double,
        flags: u32,
    ) -> FseventStreamRef;
    fn FSEventStreamInvalidate(stream: FseventStreamRef);
    fn FSEventStreamRelease(stream: FseventStreamRef);
    fn FSEventStreamScheduleWithRunLoop(
        stream: FseventStreamRef,
        run_loop: CfRunLoopRef,
        mode: CfStringRef,
    );
    fn FSEventStreamStart(stream: FseventStreamRef) -> u8;
    fn FSEventStreamStop(stream: FseventStreamRef);
}

struct CfOwned(NonNull<c_void>);

impl CfOwned {
    fn new(pointer: *const c_void, message: &str) -> Result<Self, FilesystemWatchError> {
        NonNull::new(pointer.cast_mut())
            .map(Self)
            .ok_or_else(|| native_error(message))
    }

    fn as_ptr(&self) -> *const c_void {
        self.0.as_ptr()
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: this is the unique owner of a non-null Create-rule object.
            CFRelease(self.as_ptr());
        }
    }
}

struct StreamOwner {
    pointer: NonNull<c_void>,
    started: bool,
}

impl StreamOwner {
    fn as_ptr(&self) -> FseventStreamRef {
        self.pointer.as_ptr()
    }
}

impl Drop for StreamOwner {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: this is the unique stream owner. Stop always precedes
            // invalidation and release for a successfully started stream.
            if self.started {
                FSEventStreamStop(self.as_ptr());
            }
            FSEventStreamInvalidate(self.as_ptr());
            FSEventStreamRelease(self.as_ptr());
        }
    }
}

struct CallbackState {
    root: PathBuf,
    run_loop: CfRunLoopRef,
    started: Instant,
    max_events: usize,
    events: Vec<FilesystemWatchEvent>,
    seen: BTreeSet<OsString>,
    truncated: bool,
    native_error: Option<String>,
}

impl CallbackState {
    fn push(&mut self, kind: &str, name: &OsStr, mask: Vec<String>) {
        if self.events.len() >= self.max_events {
            self.truncated = true;
            self.stop();
            return;
        }
        self.events.push(FilesystemWatchEvent {
            t_ms: self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            kind: kind.into(),
            name: String::from_utf8_lossy(name.as_bytes()).into_owned(),
            mask,
        });
        if self.events.len() >= self.max_events {
            self.truncated = true;
            self.stop();
        }
    }

    fn stop(&self) {
        unsafe {
            // SAFETY: callbacks execute on this exact retained current run loop.
            CFRunLoopStop(self.run_loop);
        }
    }
}

pub fn watch_directory(
    path: &Path,
    duration_ms: u64,
    max_events: usize,
) -> Result<FilesystemWatchResult, FilesystemWatchError> {
    if !(1..=MAX_DURATION_MS).contains(&duration_ms) || max_events == 0 {
        return Err(invalid_input(
            "duration_ms must be in 1..=86400000 and max_events must be positive",
        ));
    }
    let started = Instant::now();
    let Some(deadline) = started.checked_add(Duration::from_millis(duration_ms)) else {
        return Err(invalid_input(
            "duration_ms is too large for a monotonic wait",
        ));
    };
    let metadata = std::fs::metadata(path).map_err(map_io_error)?;
    if !metadata.is_dir() {
        return Err(FilesystemWatchError {
            kind: FilesystemWatchErrorKind::NotDirectory,
            message: "filesystem watch requires an existing directory path".into(),
        });
    }
    let root = std::fs::canonicalize(path).map_err(map_io_error)?;
    let bytes = root.as_os_str().as_bytes();
    std::str::from_utf8(bytes)
        .map_err(|_| invalid_input("macOS FSEvents requires a UTF-8 directory path"))?;
    let length = CfIndex::try_from(bytes.len())
        .map_err(|_| invalid_input("filesystem watch path is too long"))?;
    let root_string = CfOwned::new(
        unsafe {
            // SAFETY: bytes is live and CoreFoundation copies it into the string.
            CFStringCreateWithBytes(std::ptr::null(), bytes.as_ptr(), length, UTF8_ENCODING, 0)
        },
        "could not create the FSEvents directory string",
    )?;
    let root_value = root_string.as_ptr();
    let paths = CfOwned::new(
        unsafe {
            // SAFETY: standard callbacks retain this live CFString in the array.
            CFArrayCreate(
                std::ptr::null(),
                &root_value,
                1,
                (&raw const kCFTypeArrayCallBacks).cast(),
            )
        },
        "could not create the FSEvents path array",
    )?;
    let run_loop = unsafe {
        // SAFETY: returns a borrowed reference to this thread's current run loop.
        CFRunLoopGetCurrent()
    };
    if run_loop.is_null() {
        return Err(native_error("could not acquire the current CFRunLoop"));
    }
    let mut state = CallbackState {
        root,
        run_loop,
        started,
        max_events,
        events: Vec::with_capacity(max_events.min(64)),
        // Since the stream starts at "now", pre-existing names
        // need not be enumerated. Names enter this set only after a created
        // record, which disambiguates FSEvents' cumulative Created bit on a
        // later modify or rename without opening a scan-to-start race.
        seen: BTreeSet::new(),
        truncated: false,
        native_error: None,
    };
    let mut context = FseventStreamContext {
        version: 0,
        info: (&mut state as *mut CallbackState).cast(),
        retain: None,
        release: None,
        copy_description: None,
    };
    let pointer = NonNull::new(unsafe {
        // SAFETY: callback context and path objects remain live until the
        // stream is stopped, invalidated and released below.
        FSEventStreamCreate(
            std::ptr::null(),
            filesystem_event_callback,
            &mut context,
            paths.as_ptr(),
            EVENT_ID_SINCE_NOW,
            0.02,
            STREAM_FLAGS,
        )
    })
    .ok_or_else(|| native_error("could not create the FSEvents stream"))?;
    let mut stream = StreamOwner {
        pointer,
        started: false,
    };
    unsafe {
        // SAFETY: stream/run loop are live and default mode is process-lifetime.
        FSEventStreamScheduleWithRunLoop(stream.as_ptr(), run_loop, kCFRunLoopDefaultMode);
    }
    if unsafe {
        // SAFETY: the stream is scheduled on this exact current run loop.
        FSEventStreamStart(stream.as_ptr())
    } == 0
    {
        return Err(native_error("could not start the FSEvents stream"));
    }
    stream.started = true;

    while Instant::now() < deadline && !state.truncated && state.native_error.is_none() {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(100));
        unsafe {
            // SAFETY: all objects referenced by the scheduled source remain
            // live. This blocks on native notification or the remaining
            // monotonic duration; it never scans the directory on a timer.
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, remaining.as_secs_f64(), 1);
        }
    }
    unsafe {
        // SAFETY: this stream was started and remains owned by this thread.
        FSEventStreamStop(stream.as_ptr());
    }
    stream.started = false;
    // Invalidate and release the stream before moving any field out of the
    // callback context. Stop guarantees that no later callback can observe it.
    drop(stream);
    if let Some(message) = state.native_error.take() {
        return Err(native_error(message));
    }

    Ok(FilesystemWatchResult {
        provider: "macos-fsevents".into(),
        mode: "native-events".into(),
        path: path.to_string_lossy().into_owned(),
        duration_ms,
        max_events,
        emitted: state.events.len(),
        events: state.events,
        completed: !state.truncated,
        truncated: state.truncated,
    })
}

unsafe extern "C" fn filesystem_event_callback(
    _stream: FseventStreamRef,
    info: *mut c_void,
    event_count: usize,
    event_paths: *mut c_void,
    event_flags: *const FseventStreamEventFlags,
    _event_ids: *const FseventStreamEventId,
) {
    if info.is_null() || event_count == 0 {
        return;
    }
    if event_paths.is_null() || event_flags.is_null() {
        unsafe {
            // SAFETY: info is the live callback context checked above.
            let state = &mut *info.cast::<CallbackState>();
            state.native_error = Some("FSEvents returned a null callback array".into());
            state.stop();
        }
        return;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        // SAFETY: FSEvents supplies the context borrowed at stream creation and
        // parallel arrays containing event_count elements for this callback.
        let state = &mut *info.cast::<CallbackState>();
        let paths = std::slice::from_raw_parts(event_paths.cast::<*const c_char>(), event_count);
        let flags = std::slice::from_raw_parts(event_flags, event_count);

        // Loss/root signals are batch-global authority. Inspect the complete
        // batch before enforcing the result event ceiling.
        if flags.iter().any(|flags| {
            flags & (MUST_SCAN_SUBDIRS | USER_DROPPED | KERNEL_DROPPED | EVENT_IDS_WRAPPED) != 0
        }) {
            state.native_error = Some(
                "FSEvents dropped events or invalidated the event-id sequence; directory truth is unavailable"
                    .into(),
            );
            state.stop();
            return;
        }
        if flags.iter().any(|flags| flags & ROOT_CHANGED != 0) {
            state.native_error =
                Some("filesystem watch root moved, was removed, or became unavailable".into());
            state.stop();
            return;
        }

        for (&path, &flags) in paths.iter().zip(flags) {
            if flags & HISTORY_DONE != 0 || path.is_null() {
                continue;
            }
            let bytes = CStr::from_ptr(path).to_bytes();
            let event_path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
            if event_path.parent() != Some(state.root.as_path()) {
                continue;
            }
            let Some(name) = event_path.file_name().map(OsStr::to_owned) else {
                continue;
            };
            let exists = std::fs::symlink_metadata(&event_path).is_ok();
            if let Some((kind, labels)) = classify_event(flags, exists, state.seen.contains(&name))
            {
                if kind == "removed" {
                    state.seen.remove(&name);
                } else if kind == "created" {
                    state.seen.insert(name.clone());
                }
                state.push(kind, &name, labels);
                if state.truncated {
                    return;
                }
            }
        }
    }));
    if result.is_err() {
        unsafe {
            // SAFETY: same live callback context; catch_unwind prevented a Rust
            // panic from crossing the C callback ABI.
            let state = &mut *info.cast::<CallbackState>();
            state.native_error = Some("filesystem watch callback panicked".into());
            state.stop();
        }
    }
}

fn classify_event(flags: u32, exists: bool, was_seen: bool) -> Option<(&'static str, Vec<String>)> {
    let mut labels = Vec::new();
    for (flag, label) in [
        (ITEM_CREATED, "create"),
        (ITEM_REMOVED, "remove"),
        (ITEM_INODE_META_MOD, "inode-meta"),
        (
            ITEM_RENAMED,
            if exists { "rename-to" } else { "rename-from" },
        ),
        (ITEM_MODIFIED, "modify"),
        (ITEM_FINDER_INFO_MOD, "finder-info"),
        (ITEM_CHANGE_OWNER, "change-owner"),
        (ITEM_XATTR_MOD, "xattr"),
        (ITEM_IS_FILE, "file"),
        (ITEM_IS_DIR, "directory"),
        (ITEM_IS_SYMLINK, "symlink"),
        (ITEM_OWN_EVENT, "own-event"),
        (ITEM_IS_HARDLINK, "hardlink"),
        (ITEM_IS_LAST_HARDLINK, "last-hardlink"),
        (ITEM_CLONED, "cloned"),
        (MOUNT, "mount"),
        (UNMOUNT, "unmount"),
    ] {
        if flags & flag != 0 {
            labels.push(label.into());
        }
    }
    let kind = if flags & ITEM_RENAMED != 0 && !exists || flags & ITEM_REMOVED != 0 && !exists {
        "removed"
    } else if flags & ITEM_CREATED != 0 && !was_seen || flags & ITEM_RENAMED != 0 && exists {
        "created"
    } else if flags
        & (ITEM_INODE_META_MOD
            | ITEM_MODIFIED
            | ITEM_FINDER_INFO_MOD
            | ITEM_CHANGE_OWNER
            | ITEM_XATTR_MOD
            | ITEM_CLONED)
        != 0
        || flags & ITEM_CREATED != 0 && was_seen
    {
        "modified"
    } else {
        return None;
    };
    Some((kind, labels))
}

fn invalid_input(message: impl Into<String>) -> FilesystemWatchError {
    FilesystemWatchError {
        kind: FilesystemWatchErrorKind::InvalidInput,
        message: message.into(),
    }
}

fn native_error(message: impl Into<String>) -> FilesystemWatchError {
    FilesystemWatchError {
        kind: FilesystemWatchErrorKind::Native,
        message: message.into(),
    }
}

fn map_io_error(error: std::io::Error) -> FilesystemWatchError {
    FilesystemWatchError {
        kind: FilesystemWatchErrorKind::Native,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agenterm-macos-fs-watch-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    #[test]
    fn create_modify_and_remove_are_direct_child_events() {
        let root = fixture_root("lifecycle");
        std::fs::create_dir(&root).expect("fixture dir");
        let first = root.join("first.txt");
        let renamed = root.join("renamed.txt");
        let nested = root.join("nested");
        std::fs::create_dir(&nested).expect("nested dir");
        let actor = std::thread::spawn({
            let first = first.clone();
            let renamed = renamed.clone();
            let nested = nested.clone();
            move || {
                std::thread::sleep(Duration::from_millis(300));
                std::fs::write(&first, b"one").expect("create");
                std::thread::sleep(Duration::from_millis(250));
                std::fs::write(&first, b"one-two").expect("modify");
                std::thread::sleep(Duration::from_millis(200));
                std::fs::rename(&first, &renamed).expect("rename");
                std::thread::sleep(Duration::from_millis(200));
                std::fs::write(nested.join("not-recursive.txt"), b"nested").expect("nested write");
                std::thread::sleep(Duration::from_millis(200));
                std::fs::remove_file(&renamed).expect("remove");
            }
        });
        let result = watch_directory(&root, 1_700, 32).expect("watch");
        actor.join().expect("actor");
        assert_eq!(result.provider, "macos-fsevents");
        assert!(result.completed);
        assert!(!result.truncated);
        for kind in ["created", "modified"] {
            assert!(
                result
                    .events
                    .iter()
                    .any(|event| { event.kind == kind && event.name == "first.txt" }),
                "missing {kind}: {:?}",
                result.events
            );
        }
        assert!(result.events.iter().any(|event| {
            event.kind == "removed"
                && event.name == "first.txt"
                && event.mask.iter().any(|label| label == "rename-from")
        }));
        assert!(result.events.iter().any(|event| {
            event.kind == "created"
                && event.name == "renamed.txt"
                && event.mask.iter().any(|label| label == "rename-to")
        }));
        assert!(
            result
                .events
                .iter()
                .any(|event| event.kind == "removed" && event.name == "renamed.txt")
        );
        assert!(
            !result
                .events
                .iter()
                .any(|event| event.name == "not-recursive.txt")
        );
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn event_ceiling_reports_truncation() {
        let root = fixture_root("truncated");
        std::fs::create_dir(&root).expect("fixture dir");
        let actor = std::thread::spawn({
            let root = root.clone();
            move || {
                std::thread::sleep(Duration::from_millis(300));
                std::fs::write(root.join("a"), b"a").expect("first create");
                std::fs::write(root.join("b"), b"b").expect("second create");
            }
        });
        let result = watch_directory(&root, 1_000, 1).expect("watch");
        actor.join().expect("actor");
        assert_eq!(result.events.len(), 1);
        assert!(result.truncated);
        assert!(!result.completed);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn event_classification_preserves_rename_direction_and_masks() {
        let (kind, labels) =
            classify_event(ITEM_RENAMED | ITEM_IS_FILE, true, false).expect("rename to");
        assert_eq!(kind, "created");
        assert!(labels.iter().any(|label| label == "rename-to"));
        let (kind, labels) =
            classify_event(ITEM_RENAMED | ITEM_IS_FILE, false, true).expect("rename from");
        assert_eq!(kind, "removed");
        assert!(labels.iter().any(|label| label == "rename-from"));
        let (kind, labels) =
            classify_event(ITEM_RENAMED | ITEM_IS_FILE, true, true).expect("rename over seen name");
        assert_eq!(kind, "created");
        assert!(labels.iter().any(|label| label == "rename-to"));
        let (kind, labels) = classify_event(
            ITEM_MODIFIED | ITEM_INODE_META_MOD | ITEM_XATTR_MOD | ITEM_IS_FILE,
            true,
            true,
        )
        .expect("modified");
        assert_eq!(kind, "modified");
        assert_eq!(labels, ["inode-meta", "modify", "xattr", "file"]);

        let (kind, labels) =
            classify_event(ITEM_CREATED | ITEM_MODIFIED | ITEM_IS_FILE, true, true)
                .expect("cumulative created bit");
        assert_eq!(kind, "modified");
        assert_eq!(labels, ["create", "modify", "file"]);

        let (kind, labels) = classify_event(
            ITEM_CREATED | ITEM_REMOVED | ITEM_MODIFIED | ITEM_IS_FILE,
            true,
            false,
        )
        .expect("removed then recreated");
        assert_eq!(kind, "created");
        assert_eq!(labels, ["create", "remove", "modify", "file"]);
    }

    #[test]
    fn invalid_inputs_and_non_directory_are_typed() {
        let root = fixture_root("invalid");
        std::fs::write(&root, b"file").expect("fixture file");
        assert_eq!(
            watch_directory(&root, 10, 1)
                .expect_err("not directory")
                .kind,
            FilesystemWatchErrorKind::NotDirectory
        );
        assert_eq!(
            watch_directory(Path::new("."), 0, 1)
                .expect_err("zero duration")
                .kind,
            FilesystemWatchErrorKind::InvalidInput
        );
        assert_eq!(
            watch_directory(Path::new("."), MAX_DURATION_MS + 1, 1)
                .expect_err("oversize duration")
                .kind,
            FilesystemWatchErrorKind::InvalidInput
        );
        std::fs::remove_file(root).expect("cleanup");
    }
}
