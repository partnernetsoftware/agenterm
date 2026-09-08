//! Linux inotify-backed directory watch.

use std::ffi::CString;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::filesystem_watch::{
    FilesystemWatchError, FilesystemWatchErrorKind, FilesystemWatchEvent, FilesystemWatchResult,
};

const IN_ACCESS: u32 = 0x0000_0001;
const IN_MODIFY: u32 = 0x0000_0002;
const IN_ATTRIB: u32 = 0x0000_0004;
const IN_CLOSE_WRITE: u32 = 0x0000_0008;
const IN_CLOSE_NOWRITE: u32 = 0x0000_0010;
const IN_OPEN: u32 = 0x0000_0020;
const IN_MOVED_FROM: u32 = 0x0000_0040;
const IN_MOVED_TO: u32 = 0x0000_0080;
const IN_CREATE: u32 = 0x0000_0100;
const IN_DELETE: u32 = 0x0000_0200;
const IN_DELETE_SELF: u32 = 0x0000_0400;
const IN_MOVE_SELF: u32 = 0x0000_0800;

const WATCH_MASK: u32 = IN_CREATE
    | IN_MODIFY
    | IN_MOVED_TO
    | IN_MOVED_FROM
    | IN_CLOSE_WRITE
    | IN_DELETE
    | IN_ATTRIB
    | IN_DELETE_SELF
    | IN_MOVE_SELF;

const EVENT_BUF_LEN: usize = 16 * (std::mem::size_of::<libc::inotify_event>() + 256);

pub fn watch_directory(
    path: &Path,
    duration_ms: u64,
    max_events: usize,
) -> Result<FilesystemWatchResult, FilesystemWatchError> {
    if duration_ms == 0 || max_events == 0 {
        return Err(FilesystemWatchError {
            kind: FilesystemWatchErrorKind::InvalidInput,
            message: "duration_ms and max_events must be positive".into(),
        });
    }
    let metadata = std::fs::metadata(path).map_err(map_io_error)?;
    if !metadata.is_dir() {
        return Err(FilesystemWatchError {
            kind: FilesystemWatchErrorKind::NotDirectory,
            message: "filesystem watch requires an existing directory path".into(),
        });
    }

    let c_path = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| invalid_input("path must not contain interior NUL bytes"))?;
    let fd = retry_eintr(|| unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) })
        .map_err(map_io_error)?;
    if fd < 0 {
        return Err(native_error("inotify_init1 failed"));
    }
    let descriptor = unsafe { OwnedFd::from_raw_fd(fd) };
    let watch_id = retry_eintr(|| unsafe {
        libc::inotify_add_watch(descriptor.as_raw_fd(), c_path.as_ptr(), WATCH_MASK)
    })
    .map_err(map_io_error)?;
    if watch_id < 0 {
        return Err(native_error("inotify_add_watch failed"));
    }

    let started = Instant::now();
    let deadline = started + Duration::from_millis(duration_ms);
    let mut events = Vec::with_capacity(max_events.min(64));
    let mut truncated = false;
    let mut buffer = [0_u8; EVENT_BUF_LEN];

    while Instant::now() < deadline {
        if events.len() >= max_events {
            truncated = Instant::now() < deadline;
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout_ms = remaining.as_millis().min(100) as i32;
        let mut pollfd = libc::pollfd {
            fd: descriptor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let poll_result = retry_eintr(|| unsafe { libc::poll(&mut pollfd, 1, timeout_ms) })
            .map_err(map_io_error)?;
        if poll_result == 0 {
            continue;
        }
        if poll_result < 0 {
            return Err(native_error("poll on inotify descriptor failed"));
        }
        if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(native_error("inotify descriptor entered an error state"));
        }
        if pollfd.revents & libc::POLLIN == 0 {
            continue;
        }
        let read_len = retry_eintr_isize(|| unsafe {
            libc::read(
                descriptor.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        })
        .map_err(map_io_error)?;
        if read_len <= 0 {
            continue;
        }
        let mut offset = 0usize;
        while offset + std::mem::size_of::<libc::inotify_event>() <= read_len as usize {
            let header = unsafe { &*(buffer.as_ptr().add(offset) as *const libc::inotify_event) };
            let name_len = header.len as usize;
            let record_len = std::mem::size_of::<libc::inotify_event>() + name_len;
            if offset + record_len > read_len as usize {
                break;
            }
            let name = if name_len == 0 {
                String::new()
            } else {
                let start = offset + std::mem::size_of::<libc::inotify_event>();
                let end = start + name_len;
                let raw = &buffer[start..end];
                let trimmed = raw.split(|byte| *byte == 0).next().unwrap_or(raw);
                String::from_utf8_lossy(trimmed).into_owned()
            };
            let mask = header.mask;
            let (kind, mask_labels) = classify_mask(mask);
            if !kind.is_empty() {
                let t_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
                events.push(FilesystemWatchEvent {
                    t_ms,
                    kind,
                    name,
                    mask: mask_labels,
                });
                if events.len() >= max_events {
                    truncated = Instant::now() < deadline;
                    break;
                }
            }
            offset += record_len;
        }
        if truncated {
            break;
        }
    }

    Ok(FilesystemWatchResult {
        provider: "linux-inotify".into(),
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

fn classify_mask(mask: u32) -> (String, Vec<String>) {
    let mut labels = Vec::new();
    for (flag, label) in [
        (IN_ACCESS, "access"),
        (IN_MODIFY, "modify"),
        (IN_ATTRIB, "attrib"),
        (IN_CLOSE_WRITE, "close-write"),
        (IN_CLOSE_NOWRITE, "close-no-write"),
        (IN_OPEN, "open"),
        (IN_MOVED_FROM, "moved-from"),
        (IN_MOVED_TO, "moved-to"),
        (IN_CREATE, "create"),
        (IN_DELETE, "delete"),
        (IN_DELETE_SELF, "delete-self"),
        (IN_MOVE_SELF, "move-self"),
    ] {
        if mask & flag != 0 {
            labels.push(label.into());
        }
    }
    let kind = if mask & (IN_CREATE | IN_MOVED_TO) != 0 {
        "created".to_string()
    } else if mask & (IN_MODIFY | IN_CLOSE_WRITE | IN_ATTRIB) != 0 {
        "modified".to_string()
    } else if mask & (IN_DELETE | IN_MOVED_FROM | IN_DELETE_SELF | IN_MOVE_SELF) != 0 {
        "removed".to_string()
    } else if labels.is_empty() {
        String::new()
    } else {
        "other".to_string()
    };
    (kind, labels)
}

fn retry_eintr(mut operation: impl FnMut() -> libc::c_int) -> std::io::Result<libc::c_int> {
    loop {
        let result = operation();
        if result >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return if result >= 0 {
                Ok(result)
            } else {
                Err(std::io::Error::last_os_error())
            };
        }
    }
}

fn retry_eintr_isize(mut operation: impl FnMut() -> isize) -> std::io::Result<isize> {
    loop {
        let result = operation();
        if result >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return if result >= 0 {
                Ok(result)
            } else {
                Err(std::io::Error::last_os_error())
            };
        }
    }
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

    #[test]
    fn create_and_modify_emit_native_events() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-fs-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).expect("fixture dir");
        let child = root.join("fixture.txt");

        let handle = std::thread::spawn({
            let root = root.clone();
            let child = child.clone();
            move || {
                std::thread::sleep(Duration::from_millis(100));
                std::fs::write(&child, b"v1").expect("create");
                std::thread::sleep(Duration::from_millis(50));
                std::fs::write(&child, b"v2").expect("modify");
            }
        });

        let result = watch_directory(&root, 3_000, 8).expect("watch");
        handle.join().expect("writer");

        assert_eq!(result.provider, "linux-inotify");
        assert!(result.emitted >= 2);
        let kinds = result
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"created"));
        assert!(kinds.contains(&"modified"));
        assert!(
            result
                .events
                .iter()
                .any(|event| event.name == "fixture.txt"),
            "events: {:?}",
            result.events
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
