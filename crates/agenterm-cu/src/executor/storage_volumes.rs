//! Bounded mounted-volume capacity inventory without device identifiers.

use agenterm_platform::storage::{MountedVolume, MountedVolumeInventory, StorageErrorKind};
use serde_json::{Value, json};

use crate::reply::CuError;

pub(super) fn storage_volumes_payload(max: usize) -> Result<Value, CuError> {
    let inventory = agenterm_platform::storage::mounted_volumes(max).map_err(|error| {
        let code = match error.kind() {
            StorageErrorKind::InvalidValue => "storage_volumes_invalid_limit",
            StorageErrorKind::Path => "storage_volumes_path_failed",
            StorageErrorKind::Query => "storage_volumes_query_failed",
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
        "truncated": inventory.truncated,
        "complete": !inventory.truncated && inventory.read_errors == 0,
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
            truncated: false,
        });
        assert_eq!(reply["volumes"][0]["total_bytes"], "9007199254740993");
        assert_eq!(reply["volumes"][0]["mount_path_encoding"], "utf8");
        assert_eq!(reply["mutation_performed"], false);
        let encoded = serde_json::to_string(&reply["volumes"]).unwrap();
        for forbidden in ["device_path", "serial", "uuid", "wwn"] {
            assert!(!encoded.contains(forbidden));
        }
    }
}
