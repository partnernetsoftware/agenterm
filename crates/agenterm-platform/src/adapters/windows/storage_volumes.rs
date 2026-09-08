use std::{collections::BTreeSet, os::windows::ffi::OsStringExt, path::PathBuf};

use crate::{
    contract::storage::{StorageError, StorageErrorKind},
    storage::{MountedVolume, MountedVolumeInventory},
};

pub(crate) fn mounted_volumes(max: usize) -> Result<MountedVolumeInventory, StorageError> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDriveStringsW};

    // Win32 DRIVE_* values are stable API constants. Keep them local so the
    // storage feature does not pull the unrelated WindowsProgramming module.
    const DRIVE_FIXED_KIND: u32 = 3;
    const DRIVE_RAMDISK_KIND: u32 = 6;

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
    let mut skipped_unsafe = 0usize;
    let mut skipped_zero_capacity = 0usize;
    let mut truncated = false;
    for mount_path in paths {
        let encoded = wide(&mount_path)?;
        let drive_type = unsafe { GetDriveTypeW(encoded.as_ptr()) };
        if drive_type != DRIVE_FIXED_KIND && drive_type != DRIVE_RAMDISK_KIND {
            skipped_unsafe += 1;
            continue;
        }
        if volumes.len() == max {
            truncated = true;
            continue;
        }
        match crate::storage::mounted_volume_space(&mount_path) {
            Ok(space) => volumes.push(MountedVolume { mount_path, space }),
            Err(error) if error.kind() == StorageErrorKind::ZeroCapacity => {
                skipped_zero_capacity += 1;
            }
            Err(_) => read_errors += 1,
        }
    }
    Ok(MountedVolumeInventory {
        volumes,
        visited,
        read_errors,
        skipped_unsafe,
        skipped_zero_capacity,
        truncated,
        coverage: "drive-letters-fixed-only",
        coverage_complete: false,
    })
}

fn wide(path: &std::path::Path) -> Result<Vec<u16>, StorageError> {
    use std::os::windows::ffi::OsStrExt;
    let mut encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if encoded.contains(&0) {
        return Err(StorageError::new(
            StorageErrorKind::Path,
            "mount path contains an embedded NUL",
        ));
    }
    encoded.push(0);
    Ok(encoded)
}

fn query_error(operation: &str) -> StorageError {
    StorageError::new(
        StorageErrorKind::Query,
        format!("{operation}: {}", std::io::Error::last_os_error()),
    )
}
