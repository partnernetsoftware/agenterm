use std::{path::Path, time::Instant};

use serde_json::{Map, Value};

use crate::{
    contract::device_inventory::{
        DEVICE_INVENTORY_FIELD_CEILING, DEVICE_INVENTORY_SCAN_CEILING, DeviceIdentityContinuity,
        DeviceKind, DeviceProviderState, DeviceProviderStatus, DeviceSelector,
    },
    device_inventory::{
        DeviceInventoryError, DeviceInventoryErrorKind, NativeDeviceInventory, NativeDeviceRecord,
        error,
    },
    storage_device_inventory::{
        StorageDeviceError, StorageDeviceErrorKind, parse_json, run_fixed_provider,
    },
};

const PROVIDER: &str = "macos-system-profiler-json-v1";
const SYSTEM_PROFILER: &str = "/usr/sbin/system_profiler";

const DATA_TYPES: &[(DeviceKind, &str)] = &[
    (DeviceKind::Usb, "SPUSBDataType"),
    (DeviceKind::Bluetooth, "SPBluetoothDataType"),
    (DeviceKind::Audio, "SPAudioDataType"),
    (DeviceKind::Camera, "SPCameraDataType"),
    (DeviceKind::Gpu, "SPDisplaysDataType"),
];

fn map_provider_error(failure: StorageDeviceError) -> DeviceInventoryError {
    let (kind, code) = match failure.kind() {
        StorageDeviceErrorKind::InvalidLimit => (
            DeviceInventoryErrorKind::InvalidLimit,
            "device-inventory-provider-limit",
        ),
        StorageDeviceErrorKind::ProviderUnavailable => (
            DeviceInventoryErrorKind::ProviderFailed,
            "device-inventory-provider-unavailable",
        ),
        StorageDeviceErrorKind::PermissionDenied => (
            DeviceInventoryErrorKind::PermissionDenied,
            "device-inventory-provider-permission",
        ),
        StorageDeviceErrorKind::ProviderFailed => (
            DeviceInventoryErrorKind::ProviderFailed,
            "device-inventory-provider-failed",
        ),
        StorageDeviceErrorKind::Timeout => (
            DeviceInventoryErrorKind::Timeout,
            "device-inventory-provider-timeout",
        ),
        StorageDeviceErrorKind::OutputLimit => (
            DeviceInventoryErrorKind::OutputLimit,
            "device-inventory-provider-output-limit",
        ),
        StorageDeviceErrorKind::MalformedSnapshot => (
            DeviceInventoryErrorKind::MalformedSnapshot,
            "device-inventory-provider-malformed",
        ),
        StorageDeviceErrorKind::ResourceLimit => (
            DeviceInventoryErrorKind::ResourceLimit,
            "device-inventory-provider-resource-limit",
        ),
        StorageDeviceErrorKind::CleanupFailed => (
            DeviceInventoryErrorKind::CleanupFailed,
            "device-inventory-provider-cleanup-failed",
        ),
    };
    error(kind, code, failure.detail().to_owned())
}

pub(crate) fn enumerate_native(
    selector: DeviceSelector,
    deadline: Instant,
) -> Result<NativeDeviceInventory, DeviceInventoryError> {
    let mut arguments = vec!["-json", "-detailLevel", "mini"];
    arguments.extend(
        DATA_TYPES
            .iter()
            .filter(|(kind, _)| selector.includes(*kind))
            .map(|(_, name)| *name),
    );
    let output = run_fixed_provider(
        Path::new(SYSTEM_PROFILER),
        &arguments,
        None,
        deadline,
        "macOS system_profiler",
    )
    .map_err(map_provider_error)?;
    parse_inventory(&output.stdout, selector)
}

fn parse_inventory(
    bytes: &[u8],
    selector: DeviceSelector,
) -> Result<NativeDeviceInventory, DeviceInventoryError> {
    let value = parse_json(bytes).map_err(map_provider_error)?;
    let root = value.as_object().ok_or_else(|| {
        error(
            DeviceInventoryErrorKind::MalformedSnapshot,
            "device-inventory-provider-shape",
            "macOS system_profiler emitted a non-object root",
        )
    })?;
    let mut devices = Vec::new();
    let mut providers = Vec::new();
    let mut scanned = 0_usize;
    for (kind, key) in DATA_TYPES
        .iter()
        .copied()
        .filter(|(kind, _)| selector.includes(*kind))
    {
        let Some(section) = root.get(key) else {
            providers.push(status(
                kind,
                DeviceProviderState::Unavailable,
                0,
                0,
                false,
                Some("provider-section-unavailable"),
            ));
            continue;
        };
        let rows = section.as_array().ok_or_else(|| malformed("section"))?;
        let before = scanned;
        let mut truncated = false;
        visit_rows(kind, rows, &mut scanned, &mut truncated, &mut devices)?;
        providers.push(status(
            kind,
            if truncated {
                DeviceProviderState::Partial
            } else {
                DeviceProviderState::Complete
            },
            scanned - before,
            0,
            truncated,
            truncated.then_some("provider-scan-limit"),
        ));
    }
    Ok(NativeDeviceInventory { devices, providers })
}

fn visit_rows(
    kind: DeviceKind,
    rows: &[Value],
    scanned: &mut usize,
    truncated: &mut bool,
    devices: &mut Vec<NativeDeviceRecord>,
) -> Result<(), DeviceInventoryError> {
    for row in rows {
        if *scanned >= DEVICE_INVENTORY_SCAN_CEILING {
            *truncated = true;
            return Ok(());
        }
        *scanned += 1;
        let object = row.as_object().ok_or_else(|| malformed("device row"))?;
        if let Some(record) = parse_record(kind, object)? {
            devices.try_reserve(1).map_err(|_| {
                error(
                    DeviceInventoryErrorKind::ResourceLimit,
                    "device-inventory-allocation",
                    "macOS inventory allocation failed",
                )
            })?;
            devices.push(record);
        }
        if let Some(children) = object.get("_items") {
            let children = children
                .as_array()
                .ok_or_else(|| malformed("nested device rows"))?;
            visit_rows(kind, children, scanned, truncated, devices)?;
        }
    }
    Ok(())
}

fn parse_record(
    kind: DeviceKind,
    object: &Map<String, Value>,
) -> Result<Option<NativeDeviceRecord>, DeviceInventoryError> {
    let Some(name) = field(object, &["_name", "device_name", "sppci_model"])? else {
        return Ok(None);
    };
    let vendor = field(object, &["manufacturer", "vendor", "spdisplays_vendor"])?;
    let model = field(
        object,
        &["model_name", "product_id", "device_id", "sppci_model"],
    )?;
    // Serial/instance material is permitted only inside the HMAC boundary.
    // Prefer it over topology so reconnecting the same device on another port
    // retains its installation-scoped public pseudonym. It is never copied to
    // DeviceRecord or diagnostics.
    let stable_identity = field(
        object,
        &[
            "serial_num",
            "serial_number",
            "_spdisplays_display-serial-number",
        ],
    )?;
    let topology_identity = field(
        object,
        &["location_id", "_spdisplays_displayID", "sppci_bus"],
    )?;
    let locator = field(object, &["bsd_name"])?.and_then(|name| {
        (name.starts_with("cu.") || name.starts_with("tty.")).then(|| {
            crate::device_inventory::NativeDeviceLocator {
                value: Path::new("/dev").join(name).into_os_string(),
            }
        })
    });
    let mut identity = Vec::new();
    append_part(
        &mut identity,
        if stable_identity.is_some() {
            b"stable".as_slice()
        } else {
            b"topology".as_slice()
        },
    )?;
    let vendor_id = field(object, &["vendor_id", "_spdisplays_display-vendor-id"])?;
    let device_id = field(
        object,
        &["spdisplays_device-id", "_spdisplays_display-product-id"],
    )?;
    for value in [
        vendor.as_deref(),
        model.as_deref(),
        vendor_id.as_deref(),
        device_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        append_part(&mut identity, value.as_bytes())?;
    }
    if let Some(stable) = stable_identity.as_deref() {
        append_part(&mut identity, stable.as_bytes())?;
    } else {
        append_part(&mut identity, name.as_bytes())?;
        if let Some(topology) = topology_identity.as_deref() {
            append_part(&mut identity, topology.as_bytes())?;
        }
    }
    Ok(Some(NativeDeviceRecord {
        identity_material: identity,
        identity_continuity: if stable_identity.is_some() {
            DeviceIdentityContinuity::ProviderStable
        } else {
            DeviceIdentityContinuity::Topology
        },
        kind,
        name: Some(name),
        vendor,
        model,
        transport: Some(kind.as_str().to_owned()),
        locator,
    }))
}

fn field(
    object: &Map<String, Value>,
    names: &[&str],
) -> Result<Option<String>, DeviceInventoryError> {
    for name in names {
        let Some(value) = object.get(*name) else {
            continue;
        };
        let Some(text) = value.as_str() else {
            return Err(malformed("text field"));
        };
        if text.is_empty() {
            continue;
        }
        if text.len() > DEVICE_INVENTORY_FIELD_CEILING || text.chars().any(char::is_control) {
            return Err(malformed("text field"));
        }
        return Ok(Some(text.to_owned()));
    }
    Ok(None)
}

fn append_part(target: &mut Vec<u8>, part: &[u8]) -> Result<(), DeviceInventoryError> {
    if target.len().saturating_add(part.len()).saturating_add(8) > DEVICE_INVENTORY_FIELD_CEILING {
        return Err(malformed("identity material"));
    }
    target.extend_from_slice(&(part.len() as u64).to_le_bytes());
    target.extend_from_slice(part);
    Ok(())
}

fn malformed(field: &'static str) -> DeviceInventoryError {
    error(
        DeviceInventoryErrorKind::MalformedSnapshot,
        "device-inventory-provider-malformed",
        format!("macOS system_profiler emitted an invalid {field}"),
    )
}

fn status(
    kind: DeviceKind,
    state: DeviceProviderState,
    visited: usize,
    read_errors: usize,
    truncated: bool,
    code: Option<&'static str>,
) -> DeviceProviderStatus {
    DeviceProviderStatus {
        kind,
        state,
        provider: PROVIDER,
        visited,
        read_errors,
        truncated,
        code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn fixture_uses_serial_only_as_private_pseudonym_material() {
        let raw = br#"{
          "SPUSBDataType": [{"_name":"Keyboard","manufacturer":"Example","product_id":"0x0001","vendor_id":"0x0002","location_id":"0x1234","serial_num":"SECRET"}]
        }"#;
        let inventory = parse_inventory(raw, DeviceSelector::Usb).unwrap();
        assert_eq!(inventory.devices.len(), 1);
        let device = &inventory.devices[0];
        assert_eq!(device.name.as_deref(), Some("Keyboard"));
        assert!(
            device
                .identity_material
                .windows(6)
                .any(|value| value == b"SECRET")
        );
        assert!(device.name.as_deref() != Some("SECRET"));
        assert!(device.vendor.as_deref() != Some("SECRET"));
        assert!(device.model.as_deref() != Some("SECRET"));
        assert!(inventory.providers[0].state == DeviceProviderState::Complete);
    }

    #[test]
    fn missing_requested_section_is_typed_unavailable() {
        let inventory = parse_inventory(b"{}", DeviceSelector::Camera).unwrap();
        assert!(inventory.devices.is_empty());
        assert_eq!(
            inventory.providers[0].state,
            DeviceProviderState::Unavailable
        );
        assert_eq!(
            inventory.providers[0].code,
            Some("provider-section-unavailable")
        );
    }

    #[test]
    fn malformed_nested_rows_fail_closed() {
        let raw = br#"{"SPAudioDataType":[{"_name":"Audio","_items":{}}]}"#;
        assert_eq!(
            parse_inventory(raw, DeviceSelector::Audio)
                .err()
                .expect("malformed fixture must fail")
                .kind(),
            DeviceInventoryErrorKind::MalformedSnapshot
        );
    }

    #[test]
    #[ignore = "read-only live host peripheral inventory"]
    fn live_inventory_is_bounded_and_has_one_status_per_kind() {
        let deadline = Instant::now() + Duration::from_secs(15);
        let inventory = enumerate_native(DeviceSelector::All, deadline).unwrap();
        assert_eq!(inventory.providers.len(), DeviceKind::ALL.len());
        assert!(inventory.devices.len() <= DEVICE_INVENTORY_SCAN_CEILING);
        assert!(
            inventory
                .providers
                .iter()
                .all(|provider| provider.visited <= DEVICE_INVENTORY_SCAN_CEILING)
        );
    }
}
