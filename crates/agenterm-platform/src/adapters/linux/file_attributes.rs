use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io,
    os::{fd::AsRawFd as _, unix::fs::OpenOptionsExt as _},
    path::Path,
};

use crate::file_attributes::{FileAttributeError, FileAttributeErrorKind};

pub fn list_xattrs(file: &File, max_bytes: usize) -> Result<Vec<String>, FileAttributeError> {
    // SAFETY: the callback receives either a null pointer with length zero or
    // the writable allocation owned by `read_sized`; the borrowed fd is live.
    let bytes = read_sized("file-xattr-list", max_bytes, |buffer, length| unsafe {
        libc::flistxattr(file.as_raw_fd(), buffer.cast(), length)
    })?;
    parse_names(bytes)
}

pub fn get_xattr(
    file: &File,
    name: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, FileAttributeError> {
    let name = c_name(name)?;
    // SAFETY: `name` is NUL-terminated, the fd is borrowed and live, and the
    // callback buffer follows the same allocation contract as `list_xattrs`.
    read_optional_sized("file-xattr-get", max_bytes, |buffer, length| unsafe {
        libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), buffer.cast(), length)
    })
}

pub fn set_xattr(file: &File, name: &str, value: &[u8]) -> Result<(), FileAttributeError> {
    let name = c_name(name)?;
    // SAFETY: the name and value slices remain alive for this synchronous
    // syscall and the borrowed fd is valid for the duration of the call.
    let result = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
    cvt("file-xattr-set", result)
}

pub fn remove_xattr(file: &File, name: &str) -> Result<(), FileAttributeError> {
    let name = c_name(name)?;
    // SAFETY: the NUL-terminated name and borrowed live fd outlive the call.
    cvt("file-xattr-remove", unsafe {
        libc::fremovexattr(file.as_raw_fd(), name.as_ptr())
    })
}

pub fn mode(file: &File) -> Result<u32, FileAttributeError> {
    use std::os::unix::fs::MetadataExt as _;
    Ok(file
        .metadata()
        .map_err(|error| FileAttributeError::native("file-mode-read", error))?
        .mode()
        & 0o7777)
}

pub fn set_mode(file: &File, mode: u32) -> Result<(), FileAttributeError> {
    // SAFETY: `file` owns a live fd and the facade already bounded mode bits.
    cvt("file-mode-set", unsafe {
        libc::fchmod(file.as_raw_fd(), mode as libc::mode_t)
    })
}

pub fn quarantine_plan_supported() -> Result<(), FileAttributeError> {
    Err(FileAttributeError::unsupported("file-quarantine-plan"))
}

pub fn path_entry_identity(path: &Path) -> Result<crate::file_identity::FileIdentity, io::Error> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    crate::file_identity::file_identity(&file)
}

fn c_name(name: &str) -> Result<CString, FileAttributeError> {
    CString::new(name).map_err(|_| {
        FileAttributeError::new(
            FileAttributeErrorKind::InvalidName,
            "file-xattr-validate",
            "extended-attribute name contains NUL",
        )
    })
}

fn parse_names(bytes: Vec<u8>) -> Result<Vec<String>, FileAttributeError> {
    let mut names = Vec::new();
    for raw in bytes.split(|byte| *byte == 0).filter(|raw| !raw.is_empty()) {
        let name = std::str::from_utf8(raw).map_err(|_| {
            FileAttributeError::new(
                FileAttributeErrorKind::InvalidName,
                "file-xattr-list",
                "extended-attribute name is not UTF-8",
            )
        })?;
        names.push(name.to_owned());
    }
    Ok(names)
}

fn read_optional_sized(
    operation: &'static str,
    max_bytes: usize,
    mut call: impl FnMut(*mut u8, usize) -> libc::ssize_t,
) -> Result<Option<Vec<u8>>, FileAttributeError> {
    for _ in 0..2 {
        let needed = call(std::ptr::null_mut(), 0);
        if needed < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENODATA) {
                return Ok(None);
            }
            return Err(FileAttributeError::native(operation, error));
        }
        let needed = usize::try_from(needed).map_err(|_| budget_error(operation))?;
        if needed > max_bytes {
            return Err(budget_error(operation));
        }
        let mut bytes = vec![0; needed];
        let actual = call(bytes.as_mut_ptr(), bytes.len());
        if actual < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENODATA) {
                return Ok(None);
            }
            if error.raw_os_error() == Some(libc::ERANGE) {
                continue;
            }
            return Err(FileAttributeError::native(operation, error));
        }
        let actual = usize::try_from(actual).map_err(|_| budget_error(operation))?;
        if actual > bytes.len() {
            continue;
        }
        bytes.truncate(actual);
        return Ok(Some(bytes));
    }
    Err(FileAttributeError::new(
        FileAttributeErrorKind::PreconditionChanged,
        operation,
        "extended attribute changed while it was read",
    ))
}

fn read_sized(
    operation: &'static str,
    max_bytes: usize,
    call: impl FnMut(*mut u8, usize) -> libc::ssize_t,
) -> Result<Vec<u8>, FileAttributeError> {
    read_optional_sized(operation, max_bytes, call)?.ok_or_else(|| {
        FileAttributeError::new(
            FileAttributeErrorKind::PreconditionChanged,
            operation,
            "extended-attribute list disappeared",
        )
    })
}

fn cvt(operation: &'static str, result: libc::c_int) -> Result<(), FileAttributeError> {
    if result == 0 {
        Ok(())
    } else {
        Err(FileAttributeError::native(
            operation,
            io::Error::last_os_error(),
        ))
    }
}

fn budget_error(operation: &'static str) -> FileAttributeError {
    FileAttributeError::new(
        FileAttributeErrorKind::BudgetExceeded,
        operation,
        "extended-attribute bytes exceed the configured limit",
    )
}
