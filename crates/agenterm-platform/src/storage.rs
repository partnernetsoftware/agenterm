//! Capacity and allocation geometry for a path's backing volume.

pub use crate::contract::storage::{
    MountedVolumeSpace, PathVolume, StorageError, StorageErrorKind, VolumeDriveKind,
    VolumePathKind, VolumeSpace,
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
    pub skipped_unsafe: usize,
    pub skipped_zero_capacity: usize,
    pub truncated: bool,
    pub coverage: &'static str,
    pub coverage_complete: bool,
}

pub(crate) struct NativePathVolume {
    pub mount_path: Option<std::path::PathBuf>,
    pub mount_path_reason: Option<&'static str>,
    pub mount_proof: &'static str,
    pub space: MountedVolumeSpace,
    pub drive_kind: Option<VolumeDriveKind>,
    pub in_inventory: bool,
}

#[cfg(target_os = "linux")]
pub(crate) struct NativePathMount {
    pub mount_path: Option<std::path::PathBuf>,
    pub mount_path_reason: Option<&'static str>,
    pub mount_proof: &'static str,
    pub in_inventory: bool,
}

pub fn volume_at(path: &std::path::Path) -> Result<PathVolume, StorageError> {
    let symlink_followed = std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false);
    let canonical_path = std::fs::canonicalize(path).map_err(|source| {
        let kind = match source.kind() {
            std::io::ErrorKind::NotFound => StorageErrorKind::PathNotFound,
            std::io::ErrorKind::PermissionDenied => StorageErrorKind::PathDenied,
            _ => StorageErrorKind::Path,
        };
        StorageError::new(kind, format!("{}: {source}", path.display()))
    })?;
    let metadata = std::fs::metadata(&canonical_path).map_err(|source| {
        StorageError::new(
            StorageErrorKind::Path,
            format!("{}: {source}", canonical_path.display()),
        )
    })?;
    let path_kind = if metadata.is_dir() {
        VolumePathKind::Directory
    } else if metadata.is_file() {
        VolumePathKind::File
    } else {
        VolumePathKind::Other
    };
    let native = crate::selected::storage::volume_at(&canonical_path)?;
    Ok(PathVolume {
        canonical_path,
        path_kind,
        symlink_followed,
        mount_path: native.mount_path,
        mount_path_reason: native.mount_path_reason,
        mount_proof: native.mount_proof,
        space: native.space,
        drive_kind: native.drive_kind,
        in_inventory: native.in_inventory,
    })
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
    fn reports_the_same_volume_for_a_file_and_its_directory() {
        let file = std::env::current_exe().expect("current executable");
        let directory = file.parent().expect("executable parent");
        let file_volume = volume_at(&file).expect("query executable volume");
        let directory_volume = volume_at(directory).expect("query parent volume");
        assert_eq!(file_volume.path_kind, VolumePathKind::File);
        assert_eq!(directory_volume.path_kind, VolumePathKind::Directory);
        assert_eq!(file_volume.mount_path, directory_volume.mount_path);
        assert_eq!(
            file_volume.space.total_bytes,
            directory_volume.space.total_bytes
        );
        assert_eq!(
            file_volume.space.allocation_unit,
            directory_volume.space.allocation_unit
        );
    }

    #[test]
    fn path_volume_rejects_a_missing_path() {
        let missing = std::env::temp_dir().join("agenterm-volume-path-missing");
        let error = volume_at(&missing).expect_err("missing path");
        assert_eq!(error.kind(), StorageErrorKind::PathNotFound);
    }

    #[test]
    fn mounted_volume_inventory_is_bounded_and_coherent() {
        let inventory = mounted_volumes(16).expect("enumerate mounted volumes");
        assert!(inventory.volumes.len() <= 16);
        assert!(inventory.visited >= inventory.volumes.len());
        assert!(!inventory.coverage.is_empty());
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
