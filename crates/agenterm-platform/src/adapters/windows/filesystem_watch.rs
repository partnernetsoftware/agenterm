//! Windows `ReadDirectoryChangesW`-backed directory watch.

use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::{size_of, size_of_val};
use std::os::windows::{
    ffi::{OsStrExt as _, OsStringExt as _},
    io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
};
use std::path::Path;
use std::ptr;
use std::time::{Duration, Instant};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, INVALID_HANDLE_VALUE,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED,
        FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_ATTRIBUTES,
        FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
        FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SECURITY, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
    },
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Threading::{CreateEventW, WaitForSingleObject},
    },
};

use crate::filesystem_watch::{
    FilesystemWatchError, FilesystemWatchErrorKind, FilesystemWatchEvent, FilesystemWatchResult,
};

const MAX_DURATION_MS: u64 = 86_400_000;
const NOTIFY_BUFFER_BYTES: usize = 64 * 1024;
const NOTIFY_HEADER_BYTES: usize = 12;
const NOTIFY_FILTER: u32 = FILE_NOTIFY_CHANGE_FILE_NAME
    | FILE_NOTIFY_CHANGE_DIR_NAME
    | FILE_NOTIFY_CHANGE_ATTRIBUTES
    | FILE_NOTIFY_CHANGE_SIZE
    | FILE_NOTIFY_CHANGE_LAST_WRITE
    | FILE_NOTIFY_CHANGE_CREATION
    | FILE_NOTIFY_CHANGE_SECURITY;

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
    let metadata = std::fs::metadata(path).map_err(map_io_error)?;
    if !metadata.is_dir() {
        return Err(FilesystemWatchError {
            kind: FilesystemWatchErrorKind::NotDirectory,
            message: "filesystem watch requires an existing directory path".into(),
        });
    }
    let path_wide = nul_terminated(path.as_os_str())?;
    let raw_handle = unsafe {
        // SAFETY: path_wide is NUL terminated and live for the call. The
        // returned directory handle is either invalid or transferred to the
        // OwnedHandle below.
        CreateFileW(
            path_wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        )
    };
    if raw_handle == INVALID_HANDLE_VALUE {
        return Err(map_io_error(io::Error::last_os_error()));
    }
    let handle = unsafe {
        // SAFETY: CreateFileW returned a new valid owned handle.
        OwnedHandle::from_raw_handle(raw_handle)
    };

    let started = Instant::now();
    let Some(deadline) = started.checked_add(Duration::from_millis(duration_ms)) else {
        return Err(invalid_input(
            "duration_ms is too large for a monotonic wait",
        ));
    };
    // ReadDirectoryChangesW requires a DWORD-aligned notification buffer.
    let mut buffer = vec![0_u32; NOTIFY_BUFFER_BYTES / size_of::<u32>()];
    let mut events = Vec::with_capacity(max_events.min(64));
    let mut truncated = false;

    while Instant::now() < deadline {
        let completion = read_changes(
            handle.as_raw_handle(),
            &mut buffer,
            deadline.saturating_duration_since(Instant::now()),
        )?;
        let Some(bytes_read) = completion else {
            break;
        };
        if bytes_read == 0 {
            return Err(native_error(
                "ReadDirectoryChangesW returned an empty notification buffer; directory changes may have been lost",
            ));
        }
        let bytes_read = usize::try_from(bytes_read)
            .map_err(|_| native_error("directory notification byte count does not fit usize"))?;
        let buffer_bytes = size_of_val(buffer.as_slice());
        if bytes_read > buffer_bytes {
            return Err(native_error(
                "ReadDirectoryChangesW returned more bytes than the notification buffer",
            ));
        }

        // Validate the complete native chain even when the public result is
        // about to hit its event ceiling. A malformed tail cannot be hidden by
        // truncation.
        let remaining = max_events - events.len();
        let notification_bytes = unsafe {
            // SAFETY: buffer is initialized DWORD storage, bytes_read was
            // checked against its byte capacity, and byte reads permit any
            // alignment.
            std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), bytes_read)
        };
        let parsed = parse_notifications(notification_bytes, remaining)?;
        let t_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        events.extend(parsed.into_iter().map(|event| FilesystemWatchEvent {
            t_ms,
            kind: event.kind.into(),
            name: event.name,
            mask: vec![event.mask],
        }));
        if events.len() >= max_events {
            truncated = true;
            break;
        }
    }

    Ok(FilesystemWatchResult {
        provider: "windows-read-directory-changes".into(),
        mode: "native-events".into(),
        path: path.to_string_lossy().into_owned(),
        duration_ms,
        max_events,
        emitted: events.len(),
        events,
        completed: !truncated,
        truncated,
    })
}

/// Starts one overlapped read and either returns its completed byte count or
/// `None` after the deadline. On every timeout path the request is cancelled
/// and drained before the borrowed OVERLAPPED and buffer may be released.
fn read_changes(
    handle: HANDLE,
    buffer: &mut [u32],
    timeout: Duration,
) -> Result<Option<u32>, FilesystemWatchError> {
    let event = Event::new().map_err(map_io_error)?;
    let mut overlapped = OVERLAPPED {
        hEvent: event.handle,
        ..Default::default()
    };
    let accepted = unsafe {
        // SAFETY: handle is an overlapped directory handle. buffer, overlapped
        // and event remain live until completion is synchronously drained.
        ReadDirectoryChangesW(
            handle,
            buffer.as_mut_ptr().cast(),
            size_of_val(buffer) as u32,
            0,
            NOTIFY_FILTER,
            ptr::null_mut(),
            &mut overlapped,
            None,
        )
    };
    if accepted == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(map_io_error(error));
        }
    }

    match unsafe {
        // SAFETY: event stays live and is the event installed in overlapped.
        WaitForSingleObject(event.handle, duration_ms(timeout))
    } {
        WAIT_OBJECT_0 => match completed_bytes(handle, &mut overlapped) {
            Ok(transferred) => Ok(Some(transferred)),
            Err(completion_error) => {
                // ERROR_IO_INCOMPLETE would still leave the borrowed buffer
                // live in the kernel. Cancel and drain defensively before
                // returning any completion error.
                let _ = cancel_and_drain(handle, &mut overlapped);
                Err(completion_error)
            }
        },
        WAIT_TIMEOUT => cancel_and_drain(handle, &mut overlapped),
        _ => {
            let wait_error = io::Error::last_os_error();
            cancel_and_drain(handle, &mut overlapped)?;
            Err(map_io_error(wait_error))
        }
    }
}

fn completed_bytes(
    handle: HANDLE,
    overlapped: &mut OVERLAPPED,
) -> Result<u32, FilesystemWatchError> {
    let mut transferred = 0;
    if unsafe {
        // SAFETY: the wait event signalled completion for this live request.
        GetOverlappedResult(handle, overlapped, &mut transferred, 0)
    } == 0
    {
        Err(map_io_error(io::Error::last_os_error()))
    } else {
        Ok(transferred)
    }
}

fn cancel_and_drain(
    handle: HANDLE,
    overlapped: &mut OVERLAPPED,
) -> Result<Option<u32>, FilesystemWatchError> {
    unsafe {
        // SAFETY: handle and overlapped identify the exact pending request.
        // A completion race is resolved by the mandatory blocking drain below.
        CancelIoEx(handle, overlapped);
    }
    let mut transferred = 0;
    if unsafe {
        // SAFETY: bWait=true keeps the OVERLAPPED and its borrowed buffer live
        // until cancellation or the raced completion is final.
        GetOverlappedResult(handle, overlapped, &mut transferred, 1)
    } != 0
    {
        // The request won the timeout race. Preserve its completed native
        // events rather than reporting a false quiet deadline.
        return Ok(Some(transferred));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) {
        Ok(None)
    } else {
        Err(map_io_error(error))
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ParsedEvent {
    kind: &'static str,
    name: String,
    mask: String,
}

fn parse_notifications(
    bytes: &[u8],
    event_limit: usize,
) -> Result<Vec<ParsedEvent>, FilesystemWatchError> {
    if bytes.is_empty() {
        return Err(native_error("directory notification buffer is empty"));
    }
    let mut offset = 0usize;
    let mut events = Vec::with_capacity(event_limit.min(64));
    loop {
        if !offset.is_multiple_of(4) || bytes.len().saturating_sub(offset) < NOTIFY_HEADER_BYTES {
            return Err(native_error(
                "ReadDirectoryChangesW returned a malformed notification record header",
            ));
        }
        let next = read_u32(bytes, offset)? as usize;
        let action = read_u32(bytes, offset + 4)?;
        let name_bytes = read_u32(bytes, offset + 8)? as usize;
        if name_bytes == 0 || !name_bytes.is_multiple_of(2) {
            return Err(native_error(
                "ReadDirectoryChangesW returned an invalid UTF-16 filename length",
            ));
        }
        let record_bytes = if next == 0 {
            bytes.len() - offset
        } else {
            if !next.is_multiple_of(4) || next < NOTIFY_HEADER_BYTES {
                return Err(native_error(
                    "ReadDirectoryChangesW returned an invalid next-record offset",
                ));
            }
            next
        };
        let record_end = offset
            .checked_add(record_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| native_error("directory notification record exceeds its buffer"))?;
        let name_end = offset
            .checked_add(NOTIFY_HEADER_BYTES)
            .and_then(|start| start.checked_add(name_bytes))
            .filter(|end| *end <= record_end)
            .ok_or_else(|| native_error("directory notification filename exceeds its record"))?;
        let name_start = offset + NOTIFY_HEADER_BYTES;
        let mut wide = Vec::with_capacity(name_bytes / 2);
        for pair in bytes[name_start..name_end].chunks_exact(2) {
            wide.push(u16::from_le_bytes([pair[0], pair[1]]));
        }
        if wide.contains(&0) {
            return Err(native_error(
                "ReadDirectoryChangesW returned a filename containing NUL",
            ));
        }
        let (kind, mask) = classify_action(action);
        let name = OsString::from_wide(&wide).to_string_lossy().into_owned();
        if events.len() < event_limit && !name.contains('/') && !name.contains('\\') {
            events.push(ParsedEvent { kind, name, mask });
        }

        if next == 0 {
            break;
        }
        offset = record_end;
    }
    Ok(events)
}

fn classify_action(action: u32) -> (&'static str, String) {
    match action {
        FILE_ACTION_ADDED => ("created", "added".into()),
        FILE_ACTION_REMOVED => ("removed", "removed".into()),
        FILE_ACTION_MODIFIED => ("modified", "modified".into()),
        FILE_ACTION_RENAMED_OLD_NAME => ("removed", "renamed-old-name".into()),
        FILE_ACTION_RENAMED_NEW_NAME => ("created", "renamed-new-name".into()),
        _ => ("other", format!("action-{action}")),
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, FilesystemWatchError> {
    let raw: [u8; 4] = bytes
        .get(offset..offset + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| native_error("directory notification integer exceeds its buffer"))?;
    Ok(u32::from_le_bytes(raw))
}

fn nul_terminated(path: &OsStr) -> Result<Vec<u16>, FilesystemWatchError> {
    let mut units = path.encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(invalid_input("path must not contain interior NUL units"));
    }
    units.push(0);
    Ok(units)
}

fn duration_ms(duration: Duration) -> u32 {
    duration.as_millis().min(u128::from(u32::MAX)) as u32
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

fn map_io_error(error: io::Error) -> FilesystemWatchError {
    FilesystemWatchError {
        kind: FilesystemWatchErrorKind::Native,
        message: error.to_string(),
    }
}

struct Event {
    handle: HANDLE,
}

impl Event {
    fn new() -> io::Result<Self> {
        let handle = unsafe {
            // SAFETY: no security descriptor or name is supplied; the event is
            // owned by this object and closed exactly once.
            CreateEventW(ptr::null(), 1, 0, ptr::null())
        };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self { handle })
        }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: handle is the unique live event created by Event::new.
            CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(action: u32, name: &str, next: bool) -> Vec<u8> {
        let wide = name.encode_utf16().collect::<Vec<_>>();
        let raw_len = NOTIFY_HEADER_BYTES + wide.len() * 2;
        let record_len = if next {
            raw_len.next_multiple_of(4)
        } else {
            raw_len
        };
        let mut bytes = vec![0_u8; record_len];
        bytes[..4].copy_from_slice(&(if next { record_len as u32 } else { 0 }).to_le_bytes());
        bytes[4..8].copy_from_slice(&action.to_le_bytes());
        bytes[8..12].copy_from_slice(&((wide.len() * 2) as u32).to_le_bytes());
        for (index, unit) in wide.into_iter().enumerate() {
            let start = NOTIFY_HEADER_BYTES + index * 2;
            bytes[start..start + 2].copy_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn parser_decodes_all_native_actions_and_rename_semantics() {
        let mut bytes = record(FILE_ACTION_ADDED, "created.txt", true);
        bytes.extend(record(FILE_ACTION_MODIFIED, "changed.txt", true));
        bytes.extend(record(FILE_ACTION_REMOVED, "removed.txt", true));
        bytes.extend(record(FILE_ACTION_RENAMED_OLD_NAME, "old-name.txt", true));
        bytes.extend(record(FILE_ACTION_RENAMED_NEW_NAME, "new-name.txt", false));

        let events = parse_notifications(&bytes, usize::MAX).expect("parse");
        assert_eq!(
            events,
            vec![
                ParsedEvent {
                    kind: "created",
                    name: "created.txt".into(),
                    mask: "added".into(),
                },
                ParsedEvent {
                    kind: "modified",
                    name: "changed.txt".into(),
                    mask: "modified".into(),
                },
                ParsedEvent {
                    kind: "removed",
                    name: "removed.txt".into(),
                    mask: "removed".into(),
                },
                ParsedEvent {
                    kind: "removed",
                    name: "old-name.txt".into(),
                    mask: "renamed-old-name".into(),
                },
                ParsedEvent {
                    kind: "created",
                    name: "new-name.txt".into(),
                    mask: "renamed-new-name".into(),
                },
            ]
        );
    }

    #[test]
    fn parser_validates_tail_before_applying_event_cap() {
        let mut bytes = record(FILE_ACTION_ADDED, "kept.txt", true);
        let mut malformed = record(FILE_ACTION_MODIFIED, "bad.txt", false);
        malformed[8..12].copy_from_slice(&3_u32.to_le_bytes());
        bytes.extend(malformed);

        let error = parse_notifications(&bytes, 1).expect_err("malformed tail");
        assert_eq!(error.kind, FilesystemWatchErrorKind::Native);
    }

    #[test]
    fn parser_caps_output_after_validating_complete_chain() {
        let mut bytes = record(FILE_ACTION_ADDED, "one.txt", true);
        bytes.extend(record(FILE_ACTION_MODIFIED, "two.txt", false));

        let events = parse_notifications(&bytes, 1).expect("parse");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "one.txt");
    }

    #[test]
    fn parser_rejects_misaligned_or_out_of_bounds_offsets() {
        let mut misaligned = record(FILE_ACTION_ADDED, "one.txt", false);
        misaligned[..4].copy_from_slice(&14_u32.to_le_bytes());
        assert!(parse_notifications(&misaligned, 8).is_err());

        let mut out_of_bounds = record(FILE_ACTION_ADDED, "one.txt", false);
        out_of_bounds[..4].copy_from_slice(&128_u32.to_le_bytes());
        assert!(parse_notifications(&out_of_bounds, 8).is_err());
    }

    #[test]
    fn parser_preserves_unknown_action_and_rejects_odd_utf16_length() {
        let unknown = record(99, "one.txt", false);
        let events = parse_notifications(&unknown, 8).expect("unknown action");
        assert_eq!(events[0].kind, "other");
        assert_eq!(events[0].mask, "action-99");

        let mut odd = record(FILE_ACTION_ADDED, "one.txt", false);
        odd[8..12].copy_from_slice(&3_u32.to_le_bytes());
        assert!(parse_notifications(&odd, 8).is_err());
    }

    #[test]
    fn parser_drops_names_outside_the_direct_directory_scope() {
        let nested = record(FILE_ACTION_ADDED, "nested\\child.txt", false);
        assert!(
            parse_notifications(&nested, 8)
                .expect("nested record")
                .is_empty()
        );
    }

    #[test]
    fn invalid_public_bounds_fail_before_native_access() {
        assert_eq!(
            watch_directory(Path::new("."), 0, 1)
                .expect_err("zero duration")
                .kind,
            FilesystemWatchErrorKind::InvalidInput
        );
        assert_eq!(
            watch_directory(Path::new("."), MAX_DURATION_MS + 1, 1)
                .expect_err("long duration")
                .kind,
            FilesystemWatchErrorKind::InvalidInput
        );
        assert_eq!(
            watch_directory(Path::new("."), 1, 0)
                .expect_err("zero event cap")
                .kind,
            FilesystemWatchErrorKind::InvalidInput
        );
    }
}
