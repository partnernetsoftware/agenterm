use std::os::unix::ffi::OsStrExt;

#[cfg(target_os = "linux")]
use crate::contract::storage::{MountedVolumeSpace, checked_mounted_space};
use crate::contract::storage::{
    StorageError, StorageErrorKind, VolumeSpace, checked_product, checked_space,
};

pub(crate) fn volume_space(path: &std::path::Path) -> Result<VolumeSpace, StorageError> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| StorageError::new(StorageErrorKind::Path, "path contains an embedded NUL"))?;
    let mut facts = unsafe { std::mem::zeroed::<libc::statvfs>() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut facts) } != 0 {
        return Err(StorageError::new(
            StorageErrorKind::Query,
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let allocation_unit = if facts.f_frsize == 0 {
        facts.f_bsize
    } else {
        facts.f_frsize
    };
    let total = checked_product(facts.f_blocks, allocation_unit, "total blocks")?;
    let available = checked_product(facts.f_bavail, allocation_unit, "available blocks")?;
    checked_space(total, available, allocation_unit)
}

#[cfg(target_os = "linux")]
pub(crate) fn mounted_volume_space(
    path: &std::path::Path,
) -> Result<MountedVolumeSpace, StorageError> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| StorageError::new(StorageErrorKind::Path, "path contains an embedded NUL"))?;
    let mut facts = unsafe { std::mem::zeroed::<libc::statvfs>() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut facts) } != 0 {
        return Err(StorageError::new(
            StorageErrorKind::Query,
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let allocation_unit = if facts.f_frsize == 0 {
        facts.f_bsize
    } else {
        facts.f_frsize
    };
    checked_mounted_space(
        checked_product(facts.f_blocks, allocation_unit, "total blocks")?,
        checked_product(facts.f_bfree, allocation_unit, "free blocks")?,
        checked_product(facts.f_bavail, allocation_unit, "available blocks")?,
        allocation_unit,
    )
}

pub(crate) fn volume_at(
    path: &std::path::Path,
) -> Result<crate::storage::NativePathVolume, StorageError> {
    #[cfg(target_os = "macos")]
    {
        crate::selected::storage_volumes::path_volume(path)
    }
    #[cfg(target_os = "linux")]
    {
        let space = mounted_volume_space(path)?;
        let mount = crate::selected::storage_volumes::path_mount(path)?;
        Ok(crate::storage::NativePathVolume {
            mount_path: mount.mount_path,
            mount_path_reason: mount.mount_path_reason,
            mount_proof: mount.mount_proof,
            space,
            drive_kind: None,
            in_inventory: mount.in_inventory,
        })
    }
}
