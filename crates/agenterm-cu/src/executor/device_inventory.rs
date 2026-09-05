//! Bounded peripheral inventory with installation-scoped opaque identities.

use agenterm_platform::device_inventory::{
    DeviceIdentityContinuity, DeviceInventory, DeviceInventoryError, DeviceInventoryErrorKind,
    DeviceKind, DeviceRecord, DeviceSelector, ProviderState,
};
use serde_json::{Value, json};

use crate::{DeviceInventorySelector, reply::CuError, target_binding::CurrentIdentityProvider};

const RESPONSE_CEILING_BYTES: usize = 1024 * 1024;
const RESPONSE_HEADROOM_BYTES: usize = 4096;

pub(super) fn device_inventory_payload(
    selector: DeviceInventorySelector,
    max: usize,
) -> Result<Value, CuError> {
    let provider = CurrentIdentityProvider::default_for_current_user().map_err(|_| {
        CuError::new(
            "device_identity_unavailable",
            "the private installation identity directory is unavailable",
        )
    })?;
    let inventory = agenterm_platform::device_inventory::enumerate(
        provider.private_state_dir(),
        platform_selector(selector),
        max,
    )
    .map_err(device_inventory_error)?;
    inventory_value(inventory)
}

fn platform_selector(selector: DeviceInventorySelector) -> DeviceSelector {
    match selector {
        DeviceInventorySelector::Usb => DeviceSelector::Usb,
        DeviceInventorySelector::Bluetooth => DeviceSelector::Bluetooth,
        DeviceInventorySelector::Audio => DeviceSelector::Audio,
        DeviceInventorySelector::Camera => DeviceSelector::Camera,
        DeviceInventorySelector::Gpu => DeviceSelector::Gpu,
        DeviceInventorySelector::All => DeviceSelector::All,
    }
}

fn inventory_value(inventory: DeviceInventory) -> Result<Value, CuError> {
    let observed = inventory.devices.len();
    let mut devices = Vec::with_capacity(inventory.devices.len());
    let mut encoded_rows = 0usize;
    let row_budget = RESPONSE_CEILING_BYTES - RESPONSE_HEADROOM_BYTES;
    for device in inventory.devices {
        let row = device_value(device);
        let bytes = serde_json::to_vec(&row)
            .map_err(|error| CuError::new("device_inventory_encode_failed", error.to_string()))?
            .len();
        if encoded_rows.saturating_add(bytes) > row_budget {
            break;
        }
        encoded_rows += bytes;
        devices.push(row);
    }
    let returned = devices.len();
    let response_truncated = inventory.truncated || returned < observed;
    let providers = inventory
        .providers
        .into_iter()
        .map(|provider| {
            json!({
                "kind": kind_name(provider.kind),
                "state": provider_state_name(provider.state),
                "provider": provider.provider,
                "visited": provider.visited,
                "read_errors": provider.read_errors,
                "truncated": provider.truncated,
                "code": provider.code,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "devices": devices,
        "providers": providers,
        "returned": returned,
        "truncated": response_truncated,
        "complete": inventory.complete && !response_truncated,
        "identity_scope": "installation",
        "response_ceiling_bytes": RESPONSE_CEILING_BYTES,
    }))
}

fn device_value(device: DeviceRecord) -> Value {
    json!({
        "id": device.id,
        "identity_continuity": match device.identity_continuity {
            DeviceIdentityContinuity::ProviderStable => "provider-stable",
            DeviceIdentityContinuity::Topology => "topology",
        },
        "kind": kind_name(device.kind),
        "name": device.name,
        "vendor": device.vendor,
        "model": device.model,
        "transport": device.transport,
    })
}

fn kind_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Usb => "usb",
        DeviceKind::Bluetooth => "bluetooth",
        DeviceKind::Audio => "audio",
        DeviceKind::Camera => "camera",
        DeviceKind::Gpu => "gpu",
    }
}

fn provider_state_name(state: ProviderState) -> &'static str {
    match state {
        ProviderState::Complete => "complete",
        ProviderState::Partial => "partial",
        ProviderState::Unavailable => "unavailable",
    }
}

fn device_inventory_error(error: DeviceInventoryError) -> CuError {
    let code = match error.kind() {
        DeviceInventoryErrorKind::InvalidLimit => "device_inventory_invalid_limit",
        DeviceInventoryErrorKind::IdentityMissing => "device_identity_uninitialized",
        DeviceInventoryErrorKind::IdentityInvalid => "device_identity_invalid",
        DeviceInventoryErrorKind::PermissionDenied => "device_inventory_permission_denied",
        DeviceInventoryErrorKind::ProviderFailed => "device_inventory_provider_failed",
        DeviceInventoryErrorKind::Timeout => "device_inventory_timeout",
        DeviceInventoryErrorKind::OutputLimit => "device_inventory_provider_output_limit",
        DeviceInventoryErrorKind::MalformedSnapshot => "device_inventory_malformed_snapshot",
        DeviceInventoryErrorKind::ResourceLimit => "device_inventory_resource_limit",
        DeviceInventoryErrorKind::CleanupFailed => "device_inventory_cleanup_failed",
        _ => "device_inventory_failed",
    };
    CuError::new(code, error.detail())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenterm_platform::device_inventory::DeviceProviderStatus;

    #[test]
    fn public_shape_omits_provider_private_identity() {
        let value = inventory_value(DeviceInventory {
            devices: vec![DeviceRecord {
                id: "agt-device-v1-example".into(),
                identity_continuity: DeviceIdentityContinuity::ProviderStable,
                kind: DeviceKind::Usb,
                name: Some("Fixture".into()),
                vendor: Some("Example".into()),
                model: None,
                transport: Some("usb".into()),
            }],
            providers: vec![DeviceProviderStatus {
                kind: DeviceKind::Usb,
                state: ProviderState::Complete,
                provider: "fixture",
                visited: 1,
                read_errors: 0,
                truncated: false,
                code: None,
            }],
            truncated: false,
            complete: true,
        })
        .unwrap();
        assert_eq!(value["identity_scope"], "installation");
        assert_eq!(value["returned"], 1);
        let row = &value["devices"][0];
        assert!(row.get("serial").is_none());
        assert!(row.get("path").is_none());
        assert!(row.get("address").is_none());
        assert!(row.get("instance_id").is_none());
    }
}
