//! Bounded, privacy-minimized physical and block storage inventory.

use agenterm_platform::storage_device_inventory::{
    StorageDevice, StorageDeviceErrorKind, StorageDeviceInventory,
};
use serde_json::{Value, json};

use crate::reply::CuError;

const RESPONSE_CEILING_BYTES: usize = 1024 * 1024;
const RESPONSE_HEADROOM_BYTES: usize = 4096;

pub(super) fn storage_devices_payload(max: usize) -> Result<Value, CuError> {
    let inventory = agenterm_platform::storage_device_inventory::enumerate(max)
        .map_err(storage_device_error)?;
    storage_devices_from(inventory)
}

fn storage_devices_from(inventory: StorageDeviceInventory) -> Result<Value, CuError> {
    let StorageDeviceInventory {
        devices,
        visited,
        read_errors,
        truncated_scan,
        truncated,
        complete,
        provider,
    } = inventory;
    let mut rows = Vec::with_capacity(devices.len());
    let mut encoded_rows = 0usize;
    let row_budget = RESPONSE_CEILING_BYTES - RESPONSE_HEADROOM_BYTES;
    for device in devices {
        let row = row_value(device);
        let row_bytes = serde_json::to_vec(&row)
            .map_err(|error| CuError::new("storage_devices_encode_failed", error.to_string()))?
            .len();
        if encoded_rows.saturating_add(row_bytes) > row_budget {
            break;
        }
        encoded_rows += row_bytes;
        rows.push(row);
    }
    let returned = rows.len();
    let response_truncated = truncated || returned < visited.saturating_sub(read_errors);
    Ok(json!({
        "devices": rows,
        "visited": visited,
        "returned": returned,
        "read_errors": read_errors,
        "truncated": response_truncated,
        "truncated_scan": truncated_scan,
        "complete": complete,
        "provider": provider,
        "response_ceiling_bytes": RESPONSE_CEILING_BYTES,
    }))
}

fn row_value(device: StorageDevice) -> Value {
    let mut unavailable_fields = Vec::new();
    macro_rules! unavailable {
        ($field:ident) => {
            if device.$field.is_none() {
                unavailable_fields.push(stringify!($field));
            }
        };
    }
    unavailable!(node);
    unavailable!(kind);
    unavailable!(size_bytes);
    unavailable!(media_type);
    unavailable!(bus);
    unavailable!(health);
    unavailable!(health_semantics);
    unavailable!(internal);
    unavailable!(removable);
    unavailable!(ejectable);
    unavailable!(solid_state);
    unavailable!(read_only);
    if device.virtual_device.is_none() {
        unavailable_fields.push("virtual");
    }
    json!({
        "id": device.id,
        "node": device.node,
        "name": device.name,
        "kind": device.kind,
        "size_bytes": device.size_bytes.map(|value| value.to_string()),
        "media_type": device.media_type,
        "bus": device.bus,
        "health": device.health,
        "health_semantics": device.health_semantics,
        "operational": device.operational,
        "internal": device.internal,
        "removable": device.removable,
        "ejectable": device.ejectable,
        "solid_state": device.solid_state,
        "read_only": device.read_only,
        "virtual": device.virtual_device,
        "unavailable_fields": unavailable_fields,
    })
}

fn storage_device_error(
    error: agenterm_platform::storage_device_inventory::StorageDeviceError,
) -> CuError {
    let code = match error.kind() {
        StorageDeviceErrorKind::InvalidLimit => "storage_devices_invalid_limit",
        StorageDeviceErrorKind::ProviderUnavailable => "storage_devices_provider_unavailable",
        StorageDeviceErrorKind::PermissionDenied => "storage_devices_permission_denied",
        StorageDeviceErrorKind::ProviderFailed => "storage_devices_provider_failed",
        StorageDeviceErrorKind::Timeout => "storage_devices_timeout",
        StorageDeviceErrorKind::OutputLimit => "storage_devices_provider_output_limit",
        StorageDeviceErrorKind::MalformedSnapshot => "storage_devices_malformed_snapshot",
        StorageDeviceErrorKind::ResourceLimit => "storage_devices_resource_limit",
        StorageDeviceErrorKind::CleanupFailed => "storage_devices_cleanup_failed",
        _ => "storage_devices_failed",
    };
    CuError::new(code, error.detail())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_preserves_large_capacity_as_decimal_string_and_names_absence() {
        let row = row_value(StorageDevice {
            id: "disk0".into(),
            node: None,
            name: "Example".into(),
            kind: Some("disk".into()),
            size_bytes: Some(9_007_199_254_740_993),
            media_type: None,
            bus: Some("NVMe".into()),
            health: None,
            health_semantics: None,
            operational: vec!["running".into()],
            internal: Some(true),
            removable: None,
            ejectable: None,
            solid_state: Some(true),
            read_only: Some(false),
            virtual_device: Some(false),
        });
        assert_eq!(row["size_bytes"], "9007199254740993");
        assert_eq!(row["virtual"], false);
        assert!(
            row["unavailable_fields"]
                .as_array()
                .unwrap()
                .contains(&json!("node"))
        );
        assert!(row.get("serial").is_none());
        assert!(row.get("wwn").is_none());
    }

    #[test]
    fn public_payload_counts_and_bounds_rows() {
        let inventory = StorageDeviceInventory {
            devices: vec![],
            visited: 0,
            read_errors: 0,
            truncated_scan: false,
            truncated: false,
            complete: true,
            provider: "fixture",
        };
        let payload = storage_devices_from(inventory).unwrap();
        assert_eq!(payload["returned"], 0);
        assert_eq!(payload["complete"], true);
        assert!(serde_json::to_vec(&payload).unwrap().len() <= RESPONSE_CEILING_BYTES);
    }
}
