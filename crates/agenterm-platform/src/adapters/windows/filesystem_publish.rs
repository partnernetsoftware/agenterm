//! Windows atomic file publication adapter.

use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
    ptr::null_mut,
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION},
    Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, GetFileAttributesW, GetFullPathNameW, INVALID_FILE_ATTRIBUTES,
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
    },
};

pub fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    // Verbatim spelling only — the canonicalize() this replaced also resolved
    // symlinks, which owned-sibling publication deliberately no longer pays for.
    // What it must not drop is MAX_PATH escape: MoveFileExW on a plain 280-byte
    // path fails ERROR_PATH_NOT_FOUND.
    let source_display = verbatim(full_path(source)?);
    let destination_display = verbatim(full_path(destination)?);
    let destination = destination_display.clone();
    let source = source_display.clone();
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // 64 ms of retries (32 x 2 ms) is a guard against an instantaneous race,
    // not against a real holder. On Windows a replacement loses to any open
    // handle that did not permit delete-sharing, and to the scanner that opens
    // every freshly written file -- both of which last far longer than that.
    // A managed job's resident owner appending to an audit log put a release
    // gate in exactly this position: `audit_compact_publish_failed` with
    // `PermissionDenied`, reproduced every run in the Windows court.
    //
    // The bound is still bounded, and still ends: it is a hang guard, so it is
    // sized for the slowest legitimate holder rather than the fastest.
    const ATTEMPTS: usize = 400;
    for attempt in 0..ATTEMPTS {
        // WRITE_THROUGH asks the rename itself to be flushed, and paired with
        // REPLACE_EXISTING it is refused where a plain replacement is allowed:
        // a destination that is open -- even with delete-sharing granted --
        // answers ACCESS_DENIED. The caller has already `sync_all`ed the
        // source bytes and syncs the directory afterwards, so the durability
        // WRITE_THROUGH adds here is already provided either side of it.
        // Ask for it first, and fall back to the plain replacement rather than
        // failing a publish that Windows would otherwise accept.
        let flags = if attempt == 0 {
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
        } else {
            MOVEFILE_REPLACE_EXISTING
        };
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } != 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        let retryable = matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_ACCESS_DENIED as i32
                    || code == ERROR_SHARING_VIOLATION as i32
                    || code == ERROR_LOCK_VIOLATION as i32
        );
        // `MoveFileEx` refuses a destination that is open even where every
        // handle granted delete-sharing. `ReplaceFileW` exists for exactly that
        // case: it is the call Windows documents for replacing a file that may
        // be in use, and it keeps the replacement atomic. Try it once the move
        // has proved it will not proceed, before giving up.
        if retryable && attempt + 1 == ATTEMPTS {
            let replaced = unsafe {
                ReplaceFileW(
                    destination.as_ptr(),
                    source.as_ptr(),
                    null_mut(),
                    REPLACEFILE_IGNORE_MERGE_ERRORS,
                    null_mut(),
                    null_mut(),
                )
            };
            if replaced != 0 {
                return Ok(());
            }
        }
        if !retryable || attempt + 1 == ATTEMPTS {
            // Say what the destination looked like at the moment of refusal.
            // `Access is denied` names the syscall's verdict and nothing about
            // the cause, and three rebuild cycles went into guessing at it.
            let attributes = unsafe { GetFileAttributesW(destination.as_ptr()) };
            let source_attributes = unsafe { GetFileAttributesW(source.as_ptr()) };
            let source_state = std::fs::OpenOptions::new()
                .write(true)
                .open(source_display.as_path())
                .map(|_| "writable".to_owned())
                .unwrap_or_else(|probe| format!("unopenable: {probe}"));
            let reopen = std::fs::OpenOptions::new()
                .write(true)
                .open(destination_display.as_path())
                .map(|_| "writable".to_owned())
                .unwrap_or_else(|reopen_error| format!("unopenable: {reopen_error}"));
            return Err(std::io::Error::new(
                error.kind(),
                format!(
                    "{error} (destination {attributes:#x} {reopen}; source {source_attributes:#x} {source_state}; attempts {})",
                    attempt + 1
                ),
            ));
        }
        // Back off gently: the first contention is usually momentary, and a
        // scanner holding the file is not helped by spinning.
        let backoff = if attempt < 32 { 2 } else { 10 };
        std::thread::sleep(Duration::from_millis(backoff));
    }
    unreachable!("bounded replacement loop always returns")
}

pub fn install_file_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    let source = verbatim(full_path(source)?);
    let destination = verbatim(full_path(destination)?);
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

pub fn sync_parent(_parent: &Path) -> std::io::Result<()> {
    // MOVEFILE_WRITE_THROUGH owns the Windows durability barrier.
    Ok(())
}

pub fn normalize_owned_destination(destination: &Path) -> std::io::Result<PathBuf> {
    let destination = verbatim(full_path(destination)?);
    let parent = destination
        .parent()
        .ok_or_else(|| std::io::Error::other("destination parent required"))?;
    let parent = parent
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let attributes = unsafe {
        // SAFETY: parent is a live NUL-terminated UTF-16 path.
        GetFileAttributesW(parent.as_ptr())
    };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(std::io::Error::last_os_error());
    }
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "destination parent is not a directory",
        ));
    }
    Ok(destination)
}

/// Re-attach the verbatim prefix that `fs::canonicalize` used to supply.
///
/// `GetFullPathNameW` normalizes but returns a *plain* Win32 path, and plain
/// paths stay bound by MAX_PATH — every subsequent `GetFileAttributesW` /
/// `CreateFileW` / `MoveFileExW` on a deep destination would fail with
/// ERROR_PATH_NOT_FOUND. Prefixing is safe precisely because the path is
/// already fully normalized: verbatim only disables the normalization we just
/// performed ourselves.
fn verbatim(path: PathBuf) -> PathBuf {
    use std::path::{Component, Prefix};

    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return path;
    };
    // \\server\share\rest keeps its two leading separators only in the plain
    // form; the verbatim spelling replaces them with the UNC\ marker.
    let (skip, lead) = match prefix.kind() {
        Prefix::Disk(_) => (0_usize, r"\\?\"),
        Prefix::UNC(..) => (2_usize, r"\\?\UNC\"),
        _ => return path,
    };
    let mut units: Vec<u16> = lead.encode_utf16().collect();
    units.extend(path.as_os_str().encode_wide().skip(skip));
    PathBuf::from(OsString::from_wide(&units))
}

fn full_path(path: &Path) -> std::io::Result<PathBuf> {
    const INITIAL_UNITS: usize = 512;
    const MAX_UNITS: usize = 32_768;
    let input = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut output = vec![0_u16; INITIAL_UNITS];
    loop {
        let length = unsafe {
            // SAFETY: input is NUL-terminated; output is initialized writable
            // storage and no file-part pointer is requested.
            GetFullPathNameW(
                input.as_ptr(),
                output.len() as u32,
                output.as_mut_ptr(),
                null_mut(),
            )
        } as usize;
        if length == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if length < output.len() {
            output.truncate(length);
            return Ok(PathBuf::from(OsString::from_wide(&output)));
        }
        let capacity = (length + 1).max(output.len().saturating_mul(2));
        if capacity > MAX_UNITS {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "absolute destination path exceeds the Windows path bound",
            ));
        }
        output.resize(capacity, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_only_plain_win32_spellings() {
        assert_eq!(
            verbatim(PathBuf::from(r"C:\publish\manifest.json")),
            PathBuf::from(r"\\?\C:\publish\manifest.json")
        );
        assert_eq!(
            verbatim(PathBuf::from(r"\\host\share\manifest.json")),
            PathBuf::from(r"\\?\UNC\host\share\manifest.json")
        );
        // Already verbatim, and device namespaces, must pass through untouched.
        assert_eq!(
            verbatim(PathBuf::from(r"\\?\C:\publish\manifest.json")),
            PathBuf::from(r"\\?\C:\publish\manifest.json")
        );
        assert_eq!(
            verbatim(PathBuf::from(r"\\?\UNC\host\share\manifest.json")),
            PathBuf::from(r"\\?\UNC\host\share\manifest.json")
        );
    }

    #[test]
    fn normalized_destinations_survive_beyond_max_path() {
        let mut deep =
            std::env::temp_dir().join(format!("agenterm-publish-long-{}", std::process::id()));
        while deep.as_os_str().len() < 280 {
            deep.push("qualified-package-boundary-segment");
        }
        std::fs::create_dir_all(&deep).expect("deep parent");
        let normalized = normalize_owned_destination(&deep.join("manifest.json"))
            .expect("a deep destination must normalize");
        assert!(
            normalized
                .as_os_str()
                .to_string_lossy()
                .starts_with(r"\\?\"),
            "deep destinations must be handed on in verbatim form: {normalized:?}"
        );
        std::fs::remove_dir_all(
            std::env::temp_dir().join(format!("agenterm-publish-long-{}", std::process::id())),
        )
        .expect("cleanup");
    }
}
