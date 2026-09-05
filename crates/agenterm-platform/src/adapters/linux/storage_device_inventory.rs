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
            "NAME,KNAME,PATH,TYPE,SIZE,ROTA,RO,RM,TRAN,MODEL,STATE",
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
    let name = bounded_text(record.get("model"), "device model")?
        .or(bounded_text(record.get("name"), "device name")?)
        .unwrap_or_else(|| id.clone());
    let health = bounded_text(record.get("state"), "device state")?;
    let rotating = optional_bool(record.get("rota"), "rotating flag")?;
    Ok(StorageDevice {
        id,
        node: bounded_text(record.get("path"), "device node")?,
        name,
        kind: bounded_text(record.get("type"), "device kind")?,
        size_bytes: optional_u64(record.get("size"), "device size")?,
        media_type: None,
        bus: bounded_text(record.get("tran"), "transport")?,
        health_semantics: health.as_ref().map(|_| "lsblk-state"),
        health,
        operational: Vec::new(),
        internal: None,
        removable: optional_bool(record.get("rm"), "removable flag")?,
        ejectable: None,
        solid_state: rotating.map(|value| !value),
        read_only: optional_bool(record.get("ro"), "read-only flag")?,
        virtual_device: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_is_flattened_with_exact_large_capacity() {
        let raw = br#"{"blockdevices":[{"name":"sda","kname":"sda","path":"/dev/sda","type":"disk","size":9007199254740993,"rota":false,"ro":false,"rm":false,"tran":"nvme","model":"Example","state":"running","children":[{"name":"sda1","kname":"sda1","path":"/dev/sda1","type":"part","size":"4096"}]}]}"#;
        let inventory = parse_inventory(raw).unwrap();
        assert_eq!(inventory.visited, 2);
        assert_eq!(inventory.devices[0].size_bytes, Some(9_007_199_254_740_993));
        assert_eq!(inventory.devices[0].solid_state, Some(true));
        assert_eq!(inventory.devices[1].size_bytes, Some(4_096));
        assert!(!inventory.complete); // the facade establishes completeness
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
