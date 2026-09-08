//! Capacity and allocation geometry for a path's backing volume.

pub use crate::contract::storage::{
    MountedVolumeSpace, StorageError, StorageErrorKind, VolumeSpace,
};

pub const VOLUME_RESULTS_MAX: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountedVolume {
    pub mount_path: std::path::PathBuf,
    pub space: MountedVolumeSpace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountedVolumeInventory {
    pub volumes: Vec<MountedVolume>,
    pub visited: usize,
    pub read_errors: usize,
    pub truncated: bool,
}

pub fn mounted_volumes(max: usize) -> Result<MountedVolumeInventory, StorageError> {
    if !(1..=VOLUME_RESULTS_MAX).contains(&max) {
        return Err(StorageError::new(
            StorageErrorKind::InvalidValue,
            "volume result limit must be in 1..=512",
        ));
    }
    crate::selected::storage_volumes::mounted_volumes(max)
}

pub fn volume_space(path: &std::path::Path) -> Result<VolumeSpace, StorageError> {
    let canonical = std::fs::canonicalize(path).map_err(|source| {
        StorageError::new(
            StorageErrorKind::Path,
            format!("{}: {source}", path.display()),
        )
    })?;
    let metadata = std::fs::metadata(&canonical).map_err(|source| {
        StorageError::new(
            StorageErrorKind::Path,
            format!("{}: {source}", canonical.display()),
        )
    })?;
    if !metadata.is_dir() {
        return Err(StorageError::new(
            StorageErrorKind::Path,
            format!("{} is not a directory", canonical.display()),
        ));
    }
    crate::selected::storage::volume_space(&canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_space_for_the_process_temp_volume() {
        let space = volume_space(&std::env::temp_dir()).expect("query temp volume");
        assert!(space.available_bytes <= space.total_bytes.get());
        assert!(space.allocation_unit.get() <= space.total_bytes.get());
    }

    #[test]
    fn rejects_a_file_as_a_volume_directory() {
        let file = std::env::current_exe().expect("current executable");
        let error = volume_space(&file).expect_err("file is not a directory");
        assert_eq!(error.kind(), StorageErrorKind::Path);
    }

    #[test]
    fn mounted_volume_inventory_is_bounded_and_coherent() {
        let inventory = mounted_volumes(16).expect("enumerate mounted volumes");
        assert!(inventory.volumes.len() <= 16);
        assert!(inventory.visited >= inventory.volumes.len());
        for volume in inventory.volumes {
            assert!(volume.mount_path.is_absolute());
            assert!(volume.space.available_bytes <= volume.space.free_bytes);
            assert!(volume.space.free_bytes <= volume.space.total_bytes.get());
        }
    }

    #[test]
    fn mounted_volume_inventory_rejects_unbounded_limits() {
        for max in [0, VOLUME_RESULTS_MAX + 1] {
            let error = mounted_volumes(max).expect_err("reject invalid result limit");
            assert_eq!(error.kind(), StorageErrorKind::InvalidValue);
        }
    }
}

#[cfg(any(target_os = "linux", windows))]
pub(crate) fn mounted_volume_space(
    path: &std::path::Path,
) -> Result<MountedVolumeSpace, StorageError> {
    crate::selected::storage::mounted_volume_space(path)
}
