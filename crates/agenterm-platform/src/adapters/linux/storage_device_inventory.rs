use std::{path::Path, time::Instant};

use serde_json::Value;

use super::{
    STORAGE_DEVICE_SCAN_CEILING, StorageDevice, StorageDeviceError, StorageDeviceErrorKind,
    StorageDeviceInventory, bounded_text, malformed, optional_bool, optional_u64, parse_json,
    run_fixed_provider,
};

const LSBLK_CANDIDATES: [&str; 2] = ["/usr/bin/lsblk", "/bin/lsblk"];
const PROVIDER: &str = "linux-lsblk-json";
const MAX_TREE_DEPTH: usize = 64;

pub(crate) fn enumerate_native(
    deadline: Instant,
) -> Result<StorageDeviceInventory, StorageDeviceError> {
    let lsblk = fixed_lsblk()?;
    let output = run_fixed_provider(
        lsblk,
        &[
            "-J",
            "-b",
            "-o",
            "NAME,KNAME,PATH,TYPE,SIZE,ROTA,RO,RM,TRAN,MODEL,STATE,HOTPLUG",
        ],
        None,
        deadline,
        "lsblk",
    )?;
    parse_inventory(&output.stdout)
}

fn fixed_lsblk() -> Result<&'static Path, StorageDeviceError> {
    LSBLK_CANDIDATES
        .iter()
        .map(Path::new)
        .find(|path| {
            std::fs::symlink_metadata(path)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        })
        .ok_or_else(|| {
            StorageDeviceError::new(
                StorageDeviceErrorKind::ProviderUnavailable,
                "no fixed regular system lsblk executable is available",
            )
        })
}

fn parse_inventory(bytes: &[u8]) -> Result<StorageDeviceInventory, StorageDeviceError> {
    let root = parse_json(bytes)?;
    let roots = root
        .as_object()
        .and_then(|root| root.get("blockdevices"))
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("blockdevices array"))?;
    let mut state = WalkState {
        devices: Vec::new(),
        visited: 0,
        truncated_scan: false,
    };
    state
        .devices
        .try_reserve(roots.len().min(STORAGE_DEVICE_SCAN_CEILING))
        .map_err(|_| {
            StorageDeviceError::new(
                StorageDeviceErrorKind::ResourceLimit,
                "storage device inventory allocation failed",
            )
        })?;
    for row in roots {
        walk(row, 1, &mut state)?;
    }
    Ok(StorageDeviceInventory {
        devices: state.devices,
        visited: state.visited,
        read_errors: 0,
        truncated_scan: state.truncated_scan,
        truncated: state.truncated_scan,
        complete: false,
        provider: PROVIDER,
    })
}

struct WalkState {
    devices: Vec<StorageDevice>,
    visited: usize,
    truncated_scan: bool,
}

fn walk(row: &Value, depth: usize, state: &mut WalkState) -> Result<(), StorageDeviceError> {
    if state.visited >= STORAGE_DEVICE_SCAN_CEILING {
        state.truncated_scan = true;
        return Ok(());
    }
    if depth > MAX_TREE_DEPTH {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::ResourceLimit,
            "lsblk hierarchy exceeds the 64-level depth ceiling",
        ));
    }
    let record = row
        .as_object()
        .ok_or_else(|| malformed("block device row"))?;
    state.visited += 1;
    state.devices.push(parse_device(record)?);
    match record.get("children") {
        None | Some(Value::Null) => {}
        Some(Value::Array(children)) => {
            for child in children {
                walk(child, depth + 1, state)?;
            }
        }
        Some(_) => return Err(malformed("block device children")),
    }
    Ok(())
}

fn parse_device(
    record: &serde_json::Map<String, Value>,
) -> Result<StorageDevice, StorageDeviceError> {
    let id = bounded_text(record.get("kname"), "kernel device name")?
        .or(bounded_text(record.get("name"), "device name")?)
        .ok_or_else(|| malformed("kernel device name"))?;
    let kind = bounded_text(record.get("type"), "device kind")?;
    let bus = bounded_text(record.get("tran"), "transport")?;
    let name = bounded_text(record.get("model"), "device model")?
        .or_else(|| display_name(kind.as_deref(), bus.as_deref(), &id))
        .unwrap_or_else(|| id.clone());
    let rotating = optional_bool(record.get("rota"), "rotating flag")?;
    let removable = optional_bool(record.get("rm"), "removable flag")?;
    let hotplug = optional_bool(record.get("hotplug"), "hotplug flag")?;
    let operational = operational_state(record.get("state"))?;
    let virtual_device = infer_virtual_device(kind.as_deref(), bus.as_deref());
    Ok(StorageDevice {
        id,
        node: bounded_text(record.get("path"), "device node")?,
        name,
        kind,
        size_bytes: optional_u64(record.get("size"), "device size")?,
        media_type: media_type_from_rotating(rotating),
        bus,
        health_semantics: None,
        health: None,
        operational,
        internal: removable.map(|value| !value),
        removable,
        ejectable: ejectable_from_flags(removable, hotplug),
        solid_state: rotating.map(|value| !value),
        read_only: optional_bool(record.get("ro"), "read-only flag")?,
        virtual_device,
    })
}

fn display_name(kind: Option<&str>, bus: Option<&str>, id: &str) -> Option<String> {
    let label = match (kind, bus) {
        (Some(kind), Some(bus)) => format!("{bus} {kind}"),
        (Some(kind), None) => kind.to_owned(),
        (None, Some(bus)) => bus.to_owned(),
        (None, None) => return None,
    };
    Some(format!("{label} {id}"))
}

fn media_type_from_rotating(rotating: Option<bool>) -> Option<String> {
    rotating.map(|value| {
        if value {
            "rotational".to_owned()
        } else {
            "solid-state".to_owned()
        }
    })
}

fn infer_virtual_device(kind: Option<&str>, bus: Option<&str>) -> Option<bool> {
    if kind == Some("loop") {
        return Some(true);
    }
    match bus {
        Some("virtio") | Some("vhost") => Some(true),
        Some("sata") | Some("sas") | Some("ata") | Some("scsi") | Some("usb") | Some("nvme") => {
            Some(false)
        }
        _ => None,
    }
}

fn ejectable_from_flags(removable: Option<bool>, hotplug: Option<bool>) -> Option<bool> {
    match (removable, hotplug) {
        (Some(true), Some(true)) => Some(true),
        (Some(true), Some(false)) => Some(false),
        (Some(false), _) => Some(false),
        _ => None,
    }
}

fn operational_state(value: Option<&Value>) -> Result<Vec<String>, StorageDeviceError> {
    Ok(bounded_text(value, "device state")?.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_is_flattened_with_exact_large_capacity() {
        let raw = br#"{"blockdevices":[{"name":"sda","kname":"sda","path":"/dev/sda","type":"disk","size":9007199254740993,"rota":false,"ro":false,"rm":false,"tran":"nvme","model":"Example","state":"running","hotplug":false,"children":[{"name":"sda1","kname":"sda1","path":"/dev/sda1","type":"part","size":"4096"}]}]}"#;
        let inventory = parse_inventory(raw).unwrap();
        assert_eq!(inventory.visited, 2);
        assert_eq!(inventory.devices[0].size_bytes, Some(9_007_199_254_740_993));
        assert_eq!(inventory.devices[0].solid_state, Some(true));
        assert_eq!(
            inventory.devices[0].media_type.as_deref(),
            Some("solid-state")
        );
        assert_eq!(inventory.devices[0].virtual_device, Some(false));
        assert_eq!(inventory.devices[0].operational, ["running"]);
        assert_eq!(inventory.devices[1].size_bytes, Some(4_096));
        assert!(!inventory.complete); // the facade establishes completeness
    }

    #[test]
    fn virtio_disks_are_virtual_with_transport_fallback_name() {
        let raw = br#"{"blockdevices":[{"name":"vda","kname":"vda","path":"/dev/vda","type":"disk","size":128,"rota":true,"ro":false,"rm":false,"tran":"virtio","model":null,"state":null,"hotplug":false}]}"#;
        let device = parse_inventory(raw).unwrap().devices[0];
        assert_eq!(device.name, "virtio disk vda");
        assert_eq!(device.virtual_device, Some(true));
        assert_eq!(device.internal, Some(true));
        assert_eq!(device.media_type.as_deref(), Some("rotational"));
        assert!(device.health.is_none());
        assert!(device.operational.is_empty());
    }

    #[test]
    fn malformed_children_and_sizes_fail_closed() {
        for raw in [
            br#"{"blockdevices":[{"name":"sda","children":{}}]}"#.as_slice(),
            br#"{"blockdevices":[{"name":"sda","size":-1}]}"#,
        ] {
            assert_eq!(
                parse_inventory(raw).unwrap_err().kind(),
                StorageDeviceErrorKind::MalformedSnapshot
            );
        }
    }
}
