use std::{path::Path, time::Instant};

use serde_json::{Map, Value};

use super::{
    STORAGE_DEVICE_SCAN_CEILING, StorageDevice, StorageDeviceError, StorageDeviceErrorKind,
    StorageDeviceInventory, bounded_text, malformed, optional_bool, optional_u64, parse_json,
    run_fixed_provider,
};

const DISKUTIL: &str = "/usr/sbin/diskutil";
const PLUTIL: &str = "/usr/bin/plutil";
const PROVIDER: &str = "darwin-diskutil-plist";

pub(crate) fn enumerate_native(
    deadline: Instant,
) -> Result<StorageDeviceInventory, StorageDeviceError> {
    let listed = run_fixed_provider(
        Path::new(DISKUTIL),
        &["list", "-plist"],
        None,
        deadline,
        "diskutil",
    )?;
    let list = plist_json(&listed.stdout, deadline)?;
    let whole_disks = list
        .as_object()
        .and_then(|root| root.get("WholeDisks"))
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("WholeDisks array"))?;
    let visited = whole_disks.len().min(STORAGE_DEVICE_SCAN_CEILING);
    let truncated_scan = whole_disks.len() > STORAGE_DEVICE_SCAN_CEILING;
    let mut devices = Vec::new();
    devices.try_reserve(visited).map_err(|_| {
        StorageDeviceError::new(
            StorageDeviceErrorKind::ResourceLimit,
            "storage device inventory allocation failed",
        )
    })?;
    for value in whole_disks.iter().take(STORAGE_DEVICE_SCAN_CEILING) {
        let id = bounded_text(Some(value), "whole-disk identifier")?
            .ok_or_else(|| malformed("whole-disk identifier"))?;
        if !valid_disk_id(&id) {
            return Err(malformed("whole-disk identifier"));
        }
        let info = run_fixed_provider(
            Path::new(DISKUTIL),
            &["info", "-plist", &id],
            None,
            deadline,
            "diskutil",
        )?;
        let info = plist_json(&info.stdout, deadline)?;
        let record = info
            .as_object()
            .ok_or_else(|| malformed("disk info object"))?;
        devices.push(parse_disk(record, &id)?);
    }
    Ok(StorageDeviceInventory {
        devices,
        visited,
        read_errors: 0,
        truncated_scan,
        truncated: truncated_scan,
        complete: false,
        provider: PROVIDER,
    })
}

fn plist_json(bytes: &[u8], deadline: Instant) -> Result<Value, StorageDeviceError> {
    let plist = std::str::from_utf8(bytes).map_err(|_| malformed("plist encoding"))?;
    let converted = run_fixed_provider(
        Path::new(PLUTIL),
        &["-convert", "json", "-o", "-", "--", "-"],
        Some(plist.to_owned()),
        deadline,
        "plutil",
    )?;
    parse_json(&converted.stdout)
}

fn parse_disk(record: &Map<String, Value>, expected_id: &str) -> Result<StorageDevice, StorageDeviceError> {
    let id = bounded_text(record.get("DeviceIdentifier"), "device identifier")?
        .ok_or_else(|| malformed("device identifier"))?;
    if id != expected_id || !valid_disk_id(&id) {
        return Err(malformed("device identifier"));
    }
    let node = bounded_text(record.get("DeviceNode"), "device node")?;
    let name = bounded_text(record.get("MediaName"), "media name")?
        .or(bounded_text(record.get("VolumeName"), "volume name")?)
        .unwrap_or_else(|| id.clone());
    let size_bytes = match optional_u64(record.get("TotalSize"), "total size")? {
        Some(value) => Some(value),
        None => optional_u64(record.get("IOKitSize"), "IOKit size")?,
    };
    let media_type = bounded_text(record.get("MediaType"), "media type")?;
    let bus = bounded_text(record.get("BusProtocol"), "bus protocol")?;
    let health = bounded_text(record.get("SMARTStatus"), "SMART status")?;
    let writable = optional_bool(record.get("Writable"), "writable flag")?;
    let virtual_device = match bounded_text(record.get("VirtualOrPhysical"), "virtual kind")?
        .as_deref()
    {
        None => None,
        Some(value) if value.eq_ignore_ascii_case("virtual") => Some(true),
        Some(value) if value.eq_ignore_ascii_case("physical") => Some(false),
        Some(_) => return Err(malformed("virtual kind")),
    };
    Ok(StorageDevice {
        id,
        node,
        name,
        kind: Some("disk".to_owned()),
        size_bytes,
        media_type,
        bus,
        health_semantics: health.as_ref().map(|_| "diskutil-smart-status"),
        health,
        operational: Vec::new(),
        internal: optional_bool(record.get("Internal"), "internal flag")?,
        removable: optional_bool(record.get("RemovableMedia"), "removable flag")?,
        ejectable: optional_bool(record.get("Ejectable"), "ejectable flag")?,
        solid_state: optional_bool(record.get("SolidState"), "solid-state flag")?,
        read_only: writable.map(|value| !value),
        virtual_device,
    })
}

fn valid_disk_id(value: &str) -> bool {
    value
        .strip_prefix("disk")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_preserves_capacity_and_never_needs_hardware_identity() {
        let record = serde_json::json!({
            "DeviceIdentifier": "disk7",
            "DeviceNode": "/dev/disk7",
            "MediaName": "Example disk",
            "TotalSize": 9007199254740993_u64,
            "BusProtocol": "NVMe",
            "SMARTStatus": "Verified",
            "Internal": true,
            "Writable": true,
            "VirtualOrPhysical": "Physical"
        });
        let device = parse_disk(record.as_object().unwrap(), "disk7").unwrap();
        assert_eq!(device.id, "disk7");
        assert_eq!(device.size_bytes, Some(9_007_199_254_740_993));
        assert_eq!(device.health_semantics, Some("diskutil-smart-status"));
        assert_eq!(device.virtual_device, Some(false));
        assert_eq!(device.read_only, Some(false));
    }

    #[test]
    fn disk_identity_is_closed_before_provider_arguments() {
        for invalid in ["", "disk", "disk1s2", "../disk1", "disk-1", "Disk1"] {
            assert!(!valid_disk_id(invalid), "{invalid:?}");
        }
        assert!(valid_disk_id("disk0"));
    }

    #[test]
    fn mismatched_and_malformed_values_fail_closed() {
        let wrong = serde_json::json!({"DeviceIdentifier":"disk2", "TotalSize": 1});
        assert_eq!(
            parse_disk(wrong.as_object().unwrap(), "disk1")
                .unwrap_err()
                .kind(),
            StorageDeviceErrorKind::MalformedSnapshot
        );
        let bad = serde_json::json!({"DeviceIdentifier":"disk1", "TotalSize": -1});
        assert!(parse_disk(bad.as_object().unwrap(), "disk1").is_err());
    }

    #[test]
    #[ignore = "read-only live host inventory"]
    fn live_inventory_is_bounded_and_private() {
        let inventory = enumerate_native(Instant::now() + std::time::Duration::from_secs(15))
            .expect("live storage inventory");
        assert!(inventory.visited <= STORAGE_DEVICE_SCAN_CEILING);
        assert_eq!(inventory.devices.len(), inventory.visited);
    }
}
