use std::{path::PathBuf, time::Instant};

use serde_json::{Map, Value};
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;

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

const PROVIDER: &str = "windows-cim-peripheral-json-v1";
const POWERSHELL_SCRIPT: &str = r#"$ErrorActionPreference='Stop'; function Rows($class,$filter){ try { $r=@(Get-CimInstance -ClassName $class | Where-Object $filter | ForEach-Object { [ordered]@{key=[string]$_.PNPDeviceID;name=[string]$_.Name;vendor=[string]$_.Manufacturer;model=[string]$_.Description} }); [ordered]@{state='complete';rows=$r} } catch { [ordered]@{state='unavailable';rows=@()} } }; $out=[ordered]@{usb=(Rows 'Win32_PnPEntity' {$_.PNPClass -eq 'USB' -or $_.PNPClass -eq 'Ports'});bluetooth=(Rows 'Win32_PnPEntity' {$_.PNPClass -eq 'Bluetooth'});audio=(Rows 'Win32_PnPEntity' {$_.PNPClass -eq 'AudioEndpoint' -or $_.PNPClass -eq 'MEDIA'});camera=(Rows 'Win32_PnPEntity' {$_.PNPClass -eq 'Camera' -or $_.PNPClass -eq 'Image'});gpu=(Rows 'Win32_VideoController' {$true})}; ConvertTo-Json -InputObject $out -Compress -Depth 5"#;

const KINDS: &[(DeviceKind, &str, &str)] = &[
    (DeviceKind::Usb, "usb", "usb"),
    (DeviceKind::Bluetooth, "bluetooth", "bluetooth"),
    (DeviceKind::Audio, "audio", "windows-audio"),
    (DeviceKind::Camera, "camera", "windows-camera"),
    (DeviceKind::Gpu, "gpu", "windows-display"),
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
        "Windows PowerShell peripheral provider",
    )
    .map_err(map_provider_error)?;
    parse_inventory(&output.stdout, selector)
}

fn system_powershell() -> Result<PathBuf, DeviceInventoryError> {
    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: buffer is writable for its advertised length; zero or a returned
    // length that does not fit is rejected before UTF-16 conversion.
    let length = unsafe {
        GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), u32::try_from(buffer.len()).unwrap())
    };
    if length == 0 || usize::try_from(length).map_or(true, |length| length >= buffer.len()) {
        return Err(error(
            DeviceInventoryErrorKind::ProviderFailed,
            "device-inventory-system-directory",
            "Windows system directory could not be resolved",
        ));
    }
    buffer.truncate(length as usize);
    let root = String::from_utf16(&buffer).map_err(|_| {
        error(
            DeviceInventoryErrorKind::ProviderFailed,
            "device-inventory-system-directory",
            "Windows system directory was not valid UTF-16",
        )
    })?;
    Ok(PathBuf::from(root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe"))
}

fn parse_inventory(
    bytes: &[u8],
    selector: DeviceSelector,
) -> Result<NativeDeviceInventory, DeviceInventoryError> {
    let value = parse_json(bytes).map_err(map_provider_error)?;
    let root = value.as_object().ok_or_else(|| malformed("root"))?;
    let mut devices = Vec::new();
    let mut providers = Vec::new();
    let mut scanned = 0_usize;
    for (kind, key, transport) in KINDS
        .iter()
        .copied()
        .filter(|(kind, _, _)| selector.includes(*kind))
    {
        let section = root
            .get(key)
            .and_then(Value::as_object)
            .ok_or_else(|| malformed("provider section"))?;
        let state = text(section, "state")?.ok_or_else(|| malformed("provider state"))?;
        let rows = section
            .get("rows")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("provider rows"))?;
        if state == "unavailable" {
            if !rows.is_empty() {
                return Err(malformed("unavailable provider rows"));
            }
            providers.push(status(
                kind,
                DeviceProviderState::Unavailable,
                0,
                false,
                Some("provider-query-unavailable"),
            ));
            continue;
        }
        if state != "complete" {
            return Err(malformed("provider state"));
        }
        let before = scanned;
        let mut truncated = false;
        for row in rows {
            if scanned >= DEVICE_INVENTORY_SCAN_CEILING {
                truncated = true;
                break;
            }
            scanned += 1;
            let row = row.as_object().ok_or_else(|| malformed("device row"))?;
            let key = text(row, "key")?.ok_or_else(|| malformed("device key"))?;
            if key.is_empty() {
                return Err(malformed("device key"));
            }
            devices.try_reserve(1).map_err(|_| {
                error(
                    DeviceInventoryErrorKind::ResourceLimit,
                    "device-inventory-allocation",
                    "Windows device inventory allocation failed",
                )
            })?;
            devices.push(NativeDeviceRecord {
                identity_material: key.into_bytes(),
                identity_continuity: DeviceIdentityContinuity::ProviderStable,
                kind,
                name: text(row, "name")?,
                vendor: text(row, "vendor")?,
                model: text(row, "model")?,
                transport: Some(transport.to_owned()),
            });
        }
        providers.push(status(
            kind,
            if truncated {
                DeviceProviderState::Partial
            } else {
                DeviceProviderState::Complete
            },
            scanned - before,
            truncated,
            truncated.then_some("provider-scan-limit"),
        ));
    }
    Ok(NativeDeviceInventory { devices, providers })
}

fn text(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, DeviceInventoryError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let text = value.as_str().ok_or_else(|| malformed(field))?;
    if text.is_empty() {
        return Ok(None);
    }
    if text.len() > DEVICE_INVENTORY_FIELD_CEILING || text.chars().any(char::is_control) {
        return Err(malformed(field));
    }
    Ok(Some(text.to_owned()))
}

fn malformed(field: &'static str) -> DeviceInventoryError {
    error(
        DeviceInventoryErrorKind::MalformedSnapshot,
        "device-inventory-provider-malformed",
        format!("Windows provider emitted an invalid {field}"),
    )
}

fn status(
    kind: DeviceKind,
    state: DeviceProviderState,
    visited: usize,
    truncated: bool,
    code: Option<&'static str>,
) -> DeviceProviderStatus {
    DeviceProviderStatus {
        kind,
        state,
        provider: PROVIDER,
        visited,
        read_errors: 0,
        truncated,
        code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_keeps_native_key_private() {
        let raw = br#"{"usb":{"state":"complete","rows":[{"key":"USB\\VID_0001\\SECRET","name":"Keyboard","vendor":"Example","model":"Input"}]},"bluetooth":{"state":"complete","rows":[]},"audio":{"state":"complete","rows":[]},"camera":{"state":"complete","rows":[]},"gpu":{"state":"complete","rows":[]}}"#;
        let inventory = parse_inventory(raw, DeviceSelector::Usb).unwrap();
        assert_eq!(inventory.devices.len(), 1);
        assert_eq!(inventory.devices[0].name.as_deref(), Some("Keyboard"));
        assert_eq!(
            inventory.devices[0].identity_material,
            b"USB\\VID_0001\\SECRET"
        );
        assert_eq!(inventory.providers[0].state, DeviceProviderState::Complete);
    }

    #[test]
    fn unavailable_provider_is_not_an_empty_complete_snapshot() {
        let raw = br#"{"camera":{"state":"unavailable","rows":[]}}"#;
        let inventory = parse_inventory(raw, DeviceSelector::Camera).unwrap();
        assert!(inventory.devices.is_empty());
        assert_eq!(
            inventory.providers[0].state,
            DeviceProviderState::Unavailable
        );
    }

    #[test]
    fn unknown_provider_state_fails_closed() {
        let raw = br#"{"gpu":{"state":"maybe","rows":[]}}"#;
        assert_eq!(
            parse_inventory(raw, DeviceSelector::Gpu)
                .err()
                .expect("unknown state must fail")
                .kind(),
            DeviceInventoryErrorKind::MalformedSnapshot
        );
    }
}
