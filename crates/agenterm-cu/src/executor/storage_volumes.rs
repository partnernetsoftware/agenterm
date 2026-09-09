//! Bounded mounted-volume capacity inventory without device identifiers.

use agenterm_platform::storage::{MountedVolume, MountedVolumeInventory, StorageErrorKind};
use serde_json::{Value, json};

use crate::reply::CuError;

pub(super) fn storage_volume_at_payload(path: &str) -> Result<Value, CuError> {
    let volume =
        agenterm_platform::storage::volume_at(std::path::Path::new(path)).map_err(|error| {
            let code = match error.kind() {
                StorageErrorKind::PathNotFound => "storage_volume_at_path_not_found",
                StorageErrorKind::PathDenied => "storage_volume_at_path_denied",
                StorageErrorKind::Path => "storage_volume_at_path_invalid",
                StorageErrorKind::Query => "storage_volume_at_query_failed",
                StorageErrorKind::ZeroCapacity => "storage_volume_at_zero_capacity",
                StorageErrorKind::InvalidValue => "storage_volume_at_invalid",
                StorageErrorKind::Overflow => "storage_volume_at_overflow",
                _ => "storage_volume_at_failed",
            };
            CuError::new(code, error.detail())
        })?;
    let canonical_path_encoding = if volume.canonical_path.to_str().is_some() {
        "utf8"
    } else {
        "lossy"
    };
    let mount_path_encoding = volume.mount_path.as_ref().map(|path| {
        if path.to_str().is_some() {
            "utf8"
        } else {
            "lossy"
        }
    });
    Ok(json!({
        "path": path,
        "canonical_path": volume.canonical_path.to_string_lossy(),
        "canonical_path_encoding": canonical_path_encoding,
        "path_kind": match volume.path_kind {
            agenterm_platform::storage::VolumePathKind::Directory => "directory",
            agenterm_platform::storage::VolumePathKind::File => "file",
            agenterm_platform::storage::VolumePathKind::Other => "other",
        },
        "symlink_followed": volume.symlink_followed,
        "mount_path": volume.mount_path.as_ref().map(|path| path.to_string_lossy()),
        "mount_path_encoding": mount_path_encoding,
        "mount_path_reason": volume.mount_path_reason,
        "mount_proof": volume.mount_proof,
        "total_bytes": volume.space.total_bytes.get().to_string(),
        "free_bytes": volume.space.free_bytes.to_string(),
        "available_bytes": volume.space.available_bytes.to_string(),
        "allocation_unit": volume.space.allocation_unit.get().to_string(),
        "drive_kind": volume.drive_kind.map(|kind| match kind {
            agenterm_platform::storage::VolumeDriveKind::Fixed => "fixed",
            agenterm_platform::storage::VolumeDriveKind::RamDisk => "ramdisk",
            agenterm_platform::storage::VolumeDriveKind::Removable => "removable",
            agenterm_platform::storage::VolumeDriveKind::Remote => "remote",
            agenterm_platform::storage::VolumeDriveKind::CdRom => "cdrom",
            agenterm_platform::storage::VolumeDriveKind::Unknown => "unknown",
            agenterm_platform::storage::VolumeDriveKind::NoRoot => "no-root",
        }),
        "in_inventory": volume.in_inventory,
        "mutation_performed": false,
        "privacy": {
            "mount_path_returned": volume.mount_path.is_some(),
            "device_identifiers_returned": false,
            "serial_numbers_returned": false,
            "volume_guid_returned": false,
        },
    }))
}

pub(super) fn storage_volumes_payload(max: usize) -> Result<Value, CuError> {
    let inventory = agenterm_platform::storage::mounted_volumes(max).map_err(|error| {
        let code = match error.kind() {
            StorageErrorKind::InvalidValue => "storage_volumes_invalid_limit",
            StorageErrorKind::Path => "storage_volumes_path_failed",
            StorageErrorKind::PathNotFound => "storage_volumes_path_not_found",
            StorageErrorKind::PathDenied => "storage_volumes_path_denied",
            StorageErrorKind::Query => "storage_volumes_query_failed",
            StorageErrorKind::ZeroCapacity => "storage_volumes_zero_capacity",
            StorageErrorKind::Overflow => "storage_volumes_overflow",
            _ => "storage_volumes_failed",
        };
        CuError::new(code, error.detail())
    })?;
    Ok(inventory_value(inventory))
}

fn inventory_value(inventory: MountedVolumeInventory) -> Value {
    json!({
        "volumes": inventory.volumes.into_iter().map(volume_value).collect::<Vec<_>>(),
        "visited": inventory.visited,
        "read_errors": inventory.read_errors,
        "skipped_unsafe": inventory.skipped_unsafe,
        "skipped_zero_capacity": inventory.skipped_zero_capacity,
        "truncated": inventory.truncated,
        "coverage": inventory.coverage,
        "coverage_complete": inventory.coverage_complete,
        "complete": inventory.coverage_complete
            && !inventory.truncated
            && inventory.read_errors == 0
            && inventory.skipped_unsafe == 0
            && inventory.skipped_zero_capacity == 0,
        "mutation_performed": false,
        "privacy": {
            "mount_paths_returned": true,
            "device_identifiers_returned": false,
            "serial_numbers_returned": false,
        },
    })
}

fn volume_value(volume: MountedVolume) -> Value {
    let mount_path_encoding = if volume.mount_path.to_str().is_some() {
        "utf8"
    } else {
        "lossy"
    };
    json!({
        "mount_path": volume.mount_path.to_string_lossy(),
        "mount_path_encoding": mount_path_encoding,
        "total_bytes": volume.space.total_bytes.get().to_string(),
        "free_bytes": volume.space.free_bytes.to_string(),
        "available_bytes": volume.space.available_bytes.to_string(),
        "allocation_unit": volume.space.allocation_unit.get().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, path::PathBuf};

    use agenterm_platform::storage::MountedVolumeSpace;

    use super::*;

    #[test]
    fn response_keeps_wide_counts_exact_and_excludes_device_identity() {
        let reply = inventory_value(MountedVolumeInventory {
            volumes: vec![MountedVolume {
                mount_path: PathBuf::from("/example"),
                space: MountedVolumeSpace {
                    total_bytes: NonZeroU64::new(9_007_199_254_740_993).unwrap(),
                    free_bytes: 8_000_000_000_000_000,
                    available_bytes: 7_000_000_000_000_000,
                    allocation_unit: NonZeroU64::new(4096).unwrap(),
                },
            }],
            visited: 1,
            read_errors: 0,
            skipped_unsafe: 0,
            skipped_zero_capacity: 0,
            truncated: false,
            coverage: "test-complete",
            coverage_complete: true,
        });
        assert_eq!(reply["volumes"][0]["total_bytes"], "9007199254740993");
        assert_eq!(reply["volumes"][0]["mount_path_encoding"], "utf8");
        assert_eq!(reply["mutation_performed"], false);
        let encoded = serde_json::to_string(&reply["volumes"]).unwrap();
        for forbidden in ["device_path", "serial", "uuid", "wwn"] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn path_response_accepts_files_and_excludes_device_identity() {
        let executable = std::env::current_exe().unwrap();
        let reply = storage_volume_at_payload(executable.to_str().unwrap()).unwrap();
        assert_eq!(reply["path_kind"], "file");
        assert_eq!(reply["mutation_performed"], false);
        assert_eq!(reply["privacy"]["device_identifiers_returned"], false);
        assert_eq!(reply["privacy"]["serial_numbers_returned"], false);
        assert!(reply["total_bytes"].as_str().is_some());
        assert!(reply["free_bytes"].as_str().is_some());
        assert!(reply["available_bytes"].as_str().is_some());
        assert!(reply["allocation_unit"].as_str().is_some());
    }

    #[test]
    fn path_response_preserves_not_found_as_a_typed_code() {
        let missing = std::env::temp_dir().join("agenterm-storage-volume-at-missing");
        let error = storage_volume_at_payload(missing.to_str().unwrap()).unwrap_err();
        assert_eq!(error.code, "storage_volume_at_path_not_found");
    }
}
