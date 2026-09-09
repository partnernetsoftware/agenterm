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
    let mut skipped_zero_capacity = 0usize;
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
                    Err(error) if error.kind() == StorageErrorKind::ZeroCapacity => {
                        skipped_zero_capacity += 1;
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
        skipped_unsafe: 0,
        skipped_zero_capacity,
        truncated,
        coverage: "mounted-filesystems-cached",
        coverage_complete: true,
    })
}

pub(crate) fn path_volume(
    path: &std::path::Path,
) -> Result<crate::storage::NativePathVolume, StorageError> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| StorageError::new(StorageErrorKind::Path, "path contains an embedded NUL"))?;
    let mut facts = unsafe { std::mem::zeroed::<libc::statfs>() };
    if unsafe { libc::statfs(path.as_ptr(), &mut facts) } != 0 {
        return Err(StorageError::new(
            StorageErrorKind::Query,
            format!("query path mount: {}", std::io::Error::last_os_error()),
        ));
    }
    let mount = unsafe { CStr::from_ptr(facts.f_mntonname.as_ptr()) };
    let allocation_unit = u64::from(facts.f_bsize);
    let space = checked_mounted_space(
        checked_product(facts.f_blocks, allocation_unit, "total blocks")?,
        checked_product(facts.f_bfree, allocation_unit, "free blocks")?,
        checked_product(facts.f_bavail, allocation_unit, "available blocks")?,
        allocation_unit,
    )?;
    Ok(crate::storage::NativePathVolume {
        mount_path: Some(PathBuf::from(std::ffi::OsStr::from_bytes(mount.to_bytes()))),
        mount_path_reason: None,
        mount_proof: "statfs-mntonname",
        space,
        drive_kind: None,
        in_inventory: true,
    })
}
