use std::{collections::BTreeSet, os::windows::ffi::OsStringExt, path::PathBuf};

use crate::{
    contract::storage::{StorageError, StorageErrorKind},
    storage::{MountedVolume, MountedVolumeInventory},
};

pub(crate) fn mounted_volumes(max: usize) -> Result<MountedVolumeInventory, StorageError> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDriveStringsW;

    let required = unsafe { GetLogicalDriveStringsW(0, std::ptr::null_mut()) };
    if required == 0 {
        return Err(query_error("size logical-drive inventory"));
    }
    let mut buffer = vec![0u16; required as usize + 1];
    let written = unsafe { GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr()) };
    if written == 0 || written as usize >= buffer.len() {
        return Err(query_error("read logical-drive inventory"));
    }
    let mut paths = BTreeSet::new();
    for encoded in buffer[..written as usize].split(|unit| *unit == 0) {
        if !encoded.is_empty() {
            paths.insert(PathBuf::from(std::ffi::OsString::from_wide(encoded)));
        }
    }
    let visited = paths.len();
    let mut volumes = Vec::with_capacity(max.min(visited));
    let mut read_errors = 0usize;
    for mount_path in paths {
        if volumes.len() == max {
            break;
        }
        match crate::storage::mounted_volume_space(&mount_path) {
            Ok(space) => volumes.push(MountedVolume { mount_path, space }),
            Err(_) => read_errors += 1,
        }
    }
    Ok(MountedVolumeInventory {
        truncated: volumes.len().saturating_add(read_errors) < visited,
        volumes,
        visited,
        read_errors,
    })
}

fn query_error(operation: &str) -> StorageError {
    StorageError::new(
        StorageErrorKind::Query,
        format!("{operation}: {}", std::io::Error::last_os_error()),
    )
}
