use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::fs::OpenOptionsExt as _,
    path::Path,
};

use crate::file_attributes::FileAttributeError;

pub fn list_xattrs(_: &File, _: usize) -> Result<Vec<String>, FileAttributeError> {
    Err(FileAttributeError::unsupported("file-xattr-list"))
}

pub fn get_xattr(_: &File, _: &str, _: usize) -> Result<Option<Vec<u8>>, FileAttributeError> {
    Err(FileAttributeError::unsupported("file-xattr-get"))
}

pub fn set_xattr(_: &File, _: &str, _: &[u8]) -> Result<(), FileAttributeError> {
    Err(FileAttributeError::unsupported("file-xattr-set"))
}

pub fn remove_xattr(_: &File, _: &str) -> Result<(), FileAttributeError> {
    Err(FileAttributeError::unsupported("file-xattr-remove"))
}

pub fn mode(_: &File) -> Result<u32, FileAttributeError> {
    Err(FileAttributeError::unsupported("file-mode-read"))
}

pub fn set_mode(_: &File, _: u32) -> Result<(), FileAttributeError> {
    Err(FileAttributeError::unsupported("file-mode-set"))
}

pub fn quarantine_plan_supported() -> Result<(), FileAttributeError> {
    Err(FileAttributeError::unsupported("file-quarantine-plan"))
}

pub fn path_entry_identity(path: &Path) -> Result<crate::file_identity::FileIdentity, io::Error> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    crate::file_identity::file_identity(&file)
}
