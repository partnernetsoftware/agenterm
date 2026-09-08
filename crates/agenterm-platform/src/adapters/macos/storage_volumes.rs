use std::{collections::BTreeMap, ffi::CStr, os::unix::ffi::OsStrExt, path::PathBuf};

use crate::{
    contract::storage::{StorageError, StorageErrorKind, checked_mounted_space, checked_product},
    storage::{MountedVolume, MountedVolumeInventory},
};

pub(crate) fn mounted_volumes(max: usize) -> Result<MountedVolumeInventory, StorageError> {
    let mut raw = std::ptr::null_mut::<libc::statfs>();
    let count = unsafe { libc::getmntinfo(&mut raw, libc::MNT_NOWAIT) };
    if count <= 0 || raw.is_null() {
        return Err(StorageError::new(
            StorageErrorKind::Query,
            format!("query mounted volumes: {}", std::io::Error::last_os_error()),
        ));
    }
    let records = unsafe { std::slice::from_raw_parts(raw, count as usize) };
    let mut paths = BTreeMap::new();
    let mut read_errors = 0usize;
    for record in records {
        let path = unsafe { CStr::from_ptr(record.f_mntonname.as_ptr()) };
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes()));
        let total = checked_product(record.f_blocks, u64::from(record.f_bsize), "total blocks");
        let free = checked_product(record.f_bfree, u64::from(record.f_bsize), "free blocks");
        let available = checked_product(
            record.f_bavail,
            u64::from(record.f_bsize),
            "available blocks",
        );
        match (total, free, available) {
            (Ok(total), Ok(free), Ok(available)) => {
                match checked_mounted_space(total, free, available, u64::from(record.f_bsize)) {
                    Ok(space) => {
                        paths.insert(path, space);
                    }
                    Err(_) => read_errors += 1,
                }
            }
            _ => read_errors += 1,
        }
    }
    let visited = records.len();
    let truncated = paths.len() > max;
    let volumes = paths
        .into_iter()
        .take(max)
        .map(|(mount_path, space)| MountedVolume { mount_path, space })
        .collect();
    Ok(MountedVolumeInventory {
        volumes,
        visited,
        read_errors,
        truncated,
    })
}
