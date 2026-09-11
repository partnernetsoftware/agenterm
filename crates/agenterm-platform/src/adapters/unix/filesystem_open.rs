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
