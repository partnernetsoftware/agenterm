use std::os::windows::ffi::OsStrExt;

use crate::contract::storage::{
    MountedVolumeSpace, StorageError, StorageErrorKind, VolumeSpace, checked_mounted_space,
    checked_product, checked_space,
};

pub(crate) fn volume_space(path: &std::path::Path) -> Result<VolumeSpace, StorageError> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetVolumePathNameW,
    };

    let path = wide(path)?;
    let mut root = vec![0_u16; 32_768];
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return Err(query_error("resolve volume root"));
    }

    let mut available = 0_u64;
    let mut total = 0_u64;
    let mut free = 0_u64;
    if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        return Err(query_error("query volume capacity"));
    }

    let mut sectors_per_cluster = 0_u32;
    let mut bytes_per_sector = 0_u32;
    let mut free_clusters = 0_u32;
    let mut total_clusters = 0_u32;
    if unsafe {
        GetDiskFreeSpaceW(
            root.as_ptr(),
            &mut sectors_per_cluster,
            &mut bytes_per_sector,
            &mut free_clusters,
            &mut total_clusters,
        )
    } == 0
    {
        return Err(query_error("query volume allocation unit"));
    }
    let allocation_unit = checked_product(
        u64::from(sectors_per_cluster),
        u64::from(bytes_per_sector),
        "sectors per cluster",
    )?;
    checked_space(total, available, allocation_unit)
}

pub(crate) fn volume_at(
    path: &std::path::Path,
) -> Result<crate::storage::NativePathVolume, StorageError> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};

    let path = wide(path)?;
    let mut root = vec![0_u16; 32_768];
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return Err(query_error("resolve volume root"));
    }
    let root_end = root
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(root.len());
    let native_mount_path =
        std::path::PathBuf::from(std::ffi::OsString::from_wide(&root[..root_end]));
    let public_mount_path = public_mount_path(&root[..root_end]);
    let root = wide(&native_mount_path)?;
    let space = mounted_volume_space(&native_mount_path)?;
    let raw_kind = unsafe { GetDriveTypeW(root.as_ptr()) };
    let drive_kind = match raw_kind {
        2 => crate::storage::VolumeDriveKind::Removable,
        3 => crate::storage::VolumeDriveKind::Fixed,
        4 => crate::storage::VolumeDriveKind::Remote,
        5 => crate::storage::VolumeDriveKind::CdRom,
        6 => crate::storage::VolumeDriveKind::RamDisk,
        1 => crate::storage::VolumeDriveKind::NoRoot,
        _ => crate::storage::VolumeDriveKind::Unknown,
    };
    Ok(crate::storage::NativePathVolume {
        mount_path: Some(public_mount_path.clone()),
        mount_path_reason: None,
        mount_proof: "win32-volume-path-name",
        space,
        drive_kind: Some(drive_kind),
        in_inventory: matches!(
            drive_kind,
            crate::storage::VolumeDriveKind::Fixed | crate::storage::VolumeDriveKind::RamDisk
        ) && is_drive_root(&public_mount_path),
    })
}

fn public_mount_path(native: &[u16]) -> std::path::PathBuf {
    use std::os::windows::ffi::OsStringExt;

    const VERBATIM: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const VERBATIM_UNC: &[u16] = &[
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];
    let units = if let Some(tail) = native.strip_prefix(VERBATIM_UNC) {
        let mut value = vec![b'\\' as u16, b'\\' as u16];
        value.extend_from_slice(tail);
        value
    } else if let Some(tail) = native.strip_prefix(VERBATIM) {
        tail.to_vec()
    } else {
        native.to_vec()
    };
    std::path::PathBuf::from(std::ffi::OsString::from_wide(&units))
}

fn is_drive_root(path: &std::path::Path) -> bool {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    units.len() == 3
        && (units[0] >= b'A' as u16 && units[0] <= b'Z' as u16
            || units[0] >= b'a' as u16 && units[0] <= b'z' as u16)
        && units[1] == b':' as u16
        && matches!(units[2], value if value == b'\\' as u16 || value == b'/' as u16)
}

pub(crate) fn mounted_volume_space(
    path: &std::path::Path,
) -> Result<MountedVolumeSpace, StorageError> {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDiskFreeSpaceW};
    let root = wide(path)?;
    let mut available = 0_u64;
    let mut total = 0_u64;
    let mut free = 0_u64;
    if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
        return Err(query_error("query mounted volume capacity"));
    }
    let mut sectors_per_cluster = 0_u32;
    let mut bytes_per_sector = 0_u32;
    let mut free_clusters = 0_u32;
    let mut total_clusters = 0_u32;
    if unsafe {
        GetDiskFreeSpaceW(
            root.as_ptr(),
            &mut sectors_per_cluster,
            &mut bytes_per_sector,
            &mut free_clusters,
            &mut total_clusters,
        )
    } == 0
    {
        return Err(query_error("query mounted volume allocation unit"));
    }
    checked_mounted_space(
        total,
        free,
        available,
        checked_product(
            u64::from(sectors_per_cluster),
            u64::from(bytes_per_sector),
            "sectors per cluster",
        )?,
    )
}

fn wide(path: &std::path::Path) -> Result<Vec<u16>, StorageError> {
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err(StorageError::new(
            StorageErrorKind::Path,
            "path contains an embedded NUL",
        ));
    }
    value.push(0);
    Ok(value)
}

fn query_error(operation: &str) -> StorageError {
    StorageError::new(
        StorageErrorKind::Query,
        format!("{operation}: {}", std::io::Error::last_os_error()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_verbatim_prefixes_from_public_mount_paths() {
        let drive: Vec<u16> = r"\\?\C:\".encode_utf16().collect();
        assert_eq!(public_mount_path(&drive), std::path::Path::new(r"C:\"));

        let unc: Vec<u16> = r"\\?\UNC\server\share\".encode_utf16().collect();
        assert_eq!(
            public_mount_path(&unc),
            std::path::Path::new(r"\\server\share\")
        );
    }

    #[test]
    fn only_drive_roots_belong_to_the_bounded_inventory() {
        assert!(is_drive_root(std::path::Path::new(r"C:\")));
        assert!(!is_drive_root(std::path::Path::new(r"C:\mounted-volume\")));
        assert!(!is_drive_root(std::path::Path::new(r"\\server\share\")));
    }
}
