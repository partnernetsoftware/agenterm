use std::{
    ffi::{CString, OsStr},
    fs::File,
    io,
    os::{
        fd::{AsRawFd as _, FromRawFd as _},
        unix::ffi::OsStrExt as _,
    },
    path::Path,
};

use crate::filesystem_open::{ExistingEntryAccess, ExistingEntryType};

pub(crate) fn open_existing(
    path: &Path,
    expected: ExistingEntryType,
    access: ExistingEntryAccess,
) -> io::Result<File> {
    let path = c_string(path.as_os_str())?;
    descriptor(unsafe { libc::open(path.as_ptr(), flags(expected, access)) })
}

pub(crate) fn open_existing_child(
    parent: &File,
    name: &OsStr,
    expected: ExistingEntryType,
    access: ExistingEntryAccess,
) -> io::Result<File> {
    let name = c_string(name)?;
    descriptor(unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags(expected, access)) })
}

/// Exclusively create one regular-file child below a retained parent directory.
///
/// `O_CREAT | O_EXCL` refuses any existing object (file, directory, FIFO, socket,
/// symlink) without truncating or replacing it; `O_NOFOLLOW` refuses a final
/// symlink even in the race where one appears between check and create. Mode
/// `0600` keeps the new file owner-only, matching the platform's private state
/// files.
#[cfg(feature = "filesystem-create")]
pub(crate) fn create_new_regular_child(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = c_string(name)?;
    let flags = libc::O_WRONLY
        | libc::O_CREAT
        | libc::O_EXCL
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK;
    descriptor(unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o600) })
}

fn flags(expected: ExistingEntryType, access: ExistingEntryAccess) -> libc::c_int {
    // Append access opens for write without creating and keeps the no-follow and
    // non-blocking guards. Every other access stays read-only.
    let access_flags = match access {
        #[cfg(feature = "filesystem-append")]
        ExistingEntryAccess::Append => libc::O_WRONLY | libc::O_APPEND,
        ExistingEntryAccess::ReadOnly | ExistingEntryAccess::SecurityDescriptor => libc::O_RDONLY,
    };
    access_flags
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | match expected {
            ExistingEntryType::File => 0,
            ExistingEntryType::Directory => libc::O_DIRECTORY,
        }
}

fn c_string(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn descriptor(raw: libc::c_int) -> io::Result<File> {
    if raw < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(raw) })
    }
}
