use std::{path::PathBuf, time::Instant};

use serde_json::Value;
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;

use super::{
    STORAGE_DEVICE_SCAN_CEILING, StorageDevice, StorageDeviceError, StorageDeviceErrorKind,
    StorageDeviceInventory, bounded_string_list, bounded_text, malformed, optional_u64, parse_json,
    run_fixed_provider,
};

const PROVIDER: &str = "windows-get-physical-disk-json";
const POWERSHELL_SCRIPT: &str = r#"$ErrorActionPreference='Stop'; $rows=@(Get-PhysicalDisk | ForEach-Object { [ordered]@{ id=[string]$_.DeviceId; name=[string]$_.FriendlyName; kind='physical-disk'; size=$_.Size; media_type=[string]$_.MediaType; bus=[string]$_.BusType; health=[string]$_.HealthStatus; operational=@($_.OperationalStatus | ForEach-Object { [string]$_ }) } }); ConvertTo-Json -InputObject $rows -Compress -Depth 4"#;

pub(crate) fn enumerate_native(
    deadline: Instant,
) -> Result<StorageDeviceInventory, StorageDeviceError> {
    let powershell = system_powershell()?;
    let output = run_fixed_provider(
        &powershell,
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            POWERSHELL_SCRIPT,
        ],
        None,
        deadline,
        "Windows PowerShell",
    )?;
    parse_inventory(&output.stdout)
}

fn system_powershell() -> Result<PathBuf, StorageDeviceError> {
    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: `buffer` is writable for its full advertised length. The API
    // returns a UTF-16 length excluding the terminator; zero is failure.
    let length = unsafe {
        GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), u32::try_from(buffer.len()).unwrap())
    };
    if length == 0 || usize::try_from(length).map_or(true, |length| length >= buffer.len()) {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::ProviderUnavailable,
            "the Windows system directory could not be resolved",
        ));
    }
    buffer.truncate(length as usize);
    let root = String::from_utf16(&buffer).map_err(|_| {
        StorageDeviceError::new(
            StorageDeviceErrorKind::ProviderUnavailable,
            "the Windows system directory was not valid UTF-16",
        )
    })?;
    Ok(PathBuf::from(root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe"))
}

fn parse_inventory(bytes: &[u8]) -> Result<StorageDeviceInventory, StorageDeviceError> {
    let value = parse_json(bytes)?;
    let rows = value
        .as_array()
        .ok_or_else(|| malformed("physical disk array"))?;
    let visited = rows.len().min(STORAGE_DEVICE_SCAN_CEILING);
    let truncated_scan = rows.len() > STORAGE_DEVICE_SCAN_CEILING;
    let mut devices = Vec::new();
    devices.try_reserve(visited).map_err(|_| {
        StorageDeviceError::new(
            StorageDeviceErrorKind::ResourceLimit,
            "storage device inventory allocation failed",
        )
    })?;
    for row in rows.iter().take(STORAGE_DEVICE_SCAN_CEILING) {
        devices.push(parse_device(row)?);
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

fn parse_device(value: &Value) -> Result<StorageDevice, StorageDeviceError> {
    let row = value
        .as_object()
        .ok_or_else(|| malformed("physical disk row"))?;
    let id = bounded_text(row.get("id"), "physical disk id")?
        .ok_or_else(|| malformed("physical disk id"))?;
    let name = bounded_text(row.get("name"), "physical disk name")?.unwrap_or_else(|| id.clone());
    let health = bounded_text(row.get("health"), "physical disk health")?;
    let bus = bounded_text(row.get("bus"), "physical disk bus")?;
    let virtual_device = bus.as_deref().and_then(|value| {
        ["file backed virtual", "spaces", "virtual"]
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
            .then_some(true)
    });
    Ok(StorageDevice {
        id,
        node: None,
        name,
        kind: bounded_text(row.get("kind"), "physical disk kind")?,
        size_bytes: optional_u64(row.get("size"), "physical disk size")?,
        media_type: bounded_text(row.get("media_type"), "physical disk media type")?,
        bus,
        health_semantics: health.as_ref().map(|_| "get-physical-disk-health-status"),
        health,
        operational: bounded_string_list(
            row.get("operational"),
            "physical disk operational state",
        )?,
        internal: None,
        removable: None,
        ejectable: None,
        solid_state: None,
        read_only: None,
        virtual_device,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_preserves_capacity_without_stable_hardware_identity() {
        let raw = br#"[{"id":"0","name":"Example","kind":"physical-disk","size":9007199254740993,"media_type":"SSD","bus":"NVMe","health":"Healthy","operational":["OK"]}]"#;
        let inventory = parse_inventory(raw).unwrap();
        assert_eq!(inventory.visited, 1);
        assert_eq!(inventory.devices[0].id, "0");
        assert_eq!(inventory.devices[0].size_bytes, Some(9_007_199_254_740_993));
        assert_eq!(inventory.devices[0].operational, ["OK"]);
        assert!(!inventory.complete);
    }

    #[test]
    fn malformed_rows_fail_closed() {
        for raw in [br#"{}"#.as_slice(), br#"[{"id":"0","size":-1}]"#] {
            assert_eq!(
                parse_inventory(raw).unwrap_err().kind(),
                StorageDeviceErrorKind::MalformedSnapshot
            );
        }
    }
}
