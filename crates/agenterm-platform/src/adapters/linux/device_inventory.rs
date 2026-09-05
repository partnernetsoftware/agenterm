use std::{
    ffi::OsStr,
    fs,
    io::Read as _,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    time::Instant,
};

use crate::{
    contract::device_inventory::{
        DEVICE_INVENTORY_FIELD_CEILING, DEVICE_INVENTORY_SCAN_CEILING, DeviceIdentityContinuity,
        DeviceKind, DeviceProviderState, DeviceProviderStatus, DeviceSelector,
    },
    device_inventory::{
        DeviceInventoryError, DeviceInventoryErrorKind, NativeDeviceInventory, NativeDeviceRecord,
        error,
    },
};

const PROVIDER: &str = "linux-sysfs-v1";

struct ClassSpec {
    kind: DeviceKind,
    root: &'static str,
    prefix: Option<&'static str>,
    stable_identity_fields: &'static [&'static str],
    identity_fields: &'static [&'static str],
    name_fields: &'static [&'static str],
    vendor_fields: &'static [&'static str],
    model_fields: &'static [&'static str],
    transport: &'static str,
}

const SPECS: &[ClassSpec] = &[
    ClassSpec {
        kind: DeviceKind::Usb,
        root: "/sys/bus/usb/devices",
        prefix: None,
        stable_identity_fields: &["serial"],
        identity_fields: &["idVendor", "idProduct", "busnum", "devpath"],
        name_fields: &["product"],
        vendor_fields: &["manufacturer"],
        model_fields: &["idProduct"],
        transport: "usb",
    },
    ClassSpec {
        kind: DeviceKind::Bluetooth,
        root: "/sys/class/bluetooth",
        prefix: Some("hci"),
        stable_identity_fields: &["address"],
        identity_fields: &["name", "uevent"],
        name_fields: &["name"],
        vendor_fields: &[],
        model_fields: &[],
        transport: "bluetooth",
    },
    ClassSpec {
        kind: DeviceKind::Audio,
        root: "/sys/class/sound",
        prefix: Some("card"),
        stable_identity_fields: &["id"],
        identity_fields: &["id", "uevent"],
        name_fields: &["id"],
        vendor_fields: &[],
        model_fields: &[],
        transport: "native-audio",
    },
    ClassSpec {
        kind: DeviceKind::Camera,
        root: "/sys/class/video4linux",
        prefix: Some("video"),
        stable_identity_fields: &[],
        identity_fields: &["name", "uevent"],
        name_fields: &["name"],
        vendor_fields: &[],
        model_fields: &[],
        transport: "video4linux",
    },
    ClassSpec {
        kind: DeviceKind::Gpu,
        root: "/sys/class/drm",
        prefix: Some("card"),
        stable_identity_fields: &["device/uevent"],
        identity_fields: &["device/vendor", "device/device", "device/uevent"],
        name_fields: &[],
        vendor_fields: &["device/vendor"],
        model_fields: &["device/device"],
        transport: "drm",
    },
];

pub(crate) fn enumerate_native(
    selector: DeviceSelector,
    deadline: Instant,
) -> Result<NativeDeviceInventory, DeviceInventoryError> {
    let mut devices = Vec::new();
    let mut providers = Vec::new();
    let mut scanned = 0_usize;
    for spec in SPECS.iter().filter(|spec| selector.includes(spec.kind)) {
        providers.push(scan_class(spec, deadline, &mut scanned, &mut devices)?);
    }
    Ok(NativeDeviceInventory { devices, providers })
}

fn scan_class(
    spec: &ClassSpec,
    deadline: Instant,
    scanned: &mut usize,
    devices: &mut Vec<NativeDeviceRecord>,
) -> Result<DeviceProviderStatus, DeviceInventoryError> {
    if Instant::now() >= deadline {
        return Err(error(
            DeviceInventoryErrorKind::Timeout,
            "device-inventory-provider-timeout",
            "Linux sysfs inventory exceeded its shared deadline",
        ));
    }
    let entries = match fs::read_dir(spec.root) {
        Ok(entries) => entries,
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
            return Ok(status(
                spec.kind,
                DeviceProviderState::Unavailable,
                0,
                0,
                false,
                Some("provider-path-unavailable"),
            ));
        }
        Err(failure) if failure.kind() == std::io::ErrorKind::PermissionDenied => {
            return Ok(status(
                spec.kind,
                DeviceProviderState::Unavailable,
                0,
                0,
                false,
                Some("provider-permission-denied"),
            ));
        }
        Err(_) => {
            return Ok(status(
                spec.kind,
                DeviceProviderState::Unavailable,
                0,
                0,
                false,
                Some("provider-open-failed"),
            ));
        }
    };

    let mut visited = 0_usize;
    let mut read_errors = 0_usize;
    let mut truncated = false;
    for entry in entries {
        if Instant::now() >= deadline {
            return Err(error(
                DeviceInventoryErrorKind::Timeout,
                "device-inventory-provider-timeout",
                "Linux sysfs inventory exceeded its shared deadline",
            ));
        }
        if *scanned >= DEVICE_INVENTORY_SCAN_CEILING {
            truncated = true;
            break;
        }
        *scanned += 1;
        visited += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                read_errors += 1;
                continue;
            }
        };
        let entry_name = entry.file_name();
        if !matches_entry(spec, &entry_name) {
            continue;
        }
        let path = entry.path();
        let mut identity = Vec::new();
        let mut stable_parts = Vec::new();
        for field in spec.stable_identity_fields {
            match read_bounded(&path.join(field), deadline) {
                Ok(Some(bytes)) if !trim_ascii(&bytes).is_empty() => stable_parts.push(bytes),
                Ok(None) => {}
                Ok(Some(_)) => {}
                Err(_) => read_errors += 1,
            }
        }
        if stable_parts.is_empty() {
            append_part(&mut identity, b"topology")?;
            append_part(&mut identity, entry_name.as_bytes())?;
        } else {
            append_part(&mut identity, b"stable")?;
            for part in &stable_parts {
                append_part(&mut identity, trim_ascii(part))?;
            }
        }
        let mut identity_field_found = !stable_parts.is_empty();
        // Vendor/product context prevents reused low-entropy serials from
        // colliding, while a stable serial/address avoids binding the public
        // pseudonym to a transient device node or bus number.
        for field in spec.identity_fields {
            if !stable_parts.is_empty()
                && !matches!(
                    *field,
                    "idVendor" | "idProduct" | "device/vendor" | "device/device"
                )
            {
                continue;
            }
            match read_bounded(&path.join(field), deadline) {
                Ok(Some(bytes)) if !trim_ascii(&bytes).is_empty() => {
                    append_part(&mut identity, trim_ascii(&bytes))?;
                    identity_field_found = true;
                }
                Ok(None) | Ok(Some(_)) => {}
                Err(_) => read_errors += 1,
            }
        }
        if !identity_field_found {
            continue;
        }
        devices.try_reserve(1).map_err(|_| {
            error(
                DeviceInventoryErrorKind::ResourceLimit,
                "device-inventory-allocation",
                "Linux device inventory allocation failed",
            )
        })?;
        let locator = if spec.kind == DeviceKind::Usb {
            find_serial_locator(&path, deadline).map(|value| {
                crate::device_inventory::NativeDeviceLocator {
                    value: value.into_os_string(),
                }
            })
        } else {
            None
        };
        devices.push(NativeDeviceRecord {
            identity_material: identity,
            identity_continuity: if stable_parts.is_empty() {
                DeviceIdentityContinuity::Topology
            } else {
                DeviceIdentityContinuity::ProviderStable
            },
            kind: spec.kind,
            name: first_text(&path, spec.name_fields, deadline, &mut read_errors),
            vendor: first_text(&path, spec.vendor_fields, deadline, &mut read_errors),
            model: first_text(&path, spec.model_fields, deadline, &mut read_errors),
            transport: Some(spec.transport.to_owned()),
            locator,
        });
    }
    let state = if truncated || read_errors != 0 {
        DeviceProviderState::Partial
    } else {
        DeviceProviderState::Complete
    };
    Ok(status(
        spec.kind,
        state,
        visited,
        read_errors,
        truncated,
        (read_errors != 0).then_some("provider-read-errors"),
    ))
}

fn find_serial_locator(root: &Path, deadline: Instant) -> Option<PathBuf> {
    let mut pending = vec![(root.to_path_buf(), 0_u8)];
    let mut visited = 0_usize;
    while let Some((path, depth)) = pending.pop() {
        if Instant::now() >= deadline || visited >= 256 {
            return None;
        }
        visited += 1;
        let entries = fs::read_dir(path).ok()?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let bytes = name.as_bytes();
            if (bytes.starts_with(b"ttyUSB") || bytes.starts_with(b"ttyACM"))
                && fs::symlink_metadata(entry.path()).ok().is_some()
            {
                return Some(Path::new("/dev").join(name));
            }
            if depth < 3 && entry.file_type().ok().is_some_and(|kind| kind.is_dir()) {
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    None
}

fn matches_entry(spec: &ClassSpec, name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    let Some(prefix) = spec.prefix else {
        return true;
    };
    if !bytes.starts_with(prefix.as_bytes()) {
        return false;
    }
    if spec.kind == DeviceKind::Gpu {
        return bytes[prefix.len()..]
            .iter()
            .all(|byte| byte.is_ascii_digit());
    }
    true
}

fn first_text(
    root: &Path,
    fields: &[&str],
    deadline: Instant,
    read_errors: &mut usize,
) -> Option<String> {
    for field in fields {
        match read_bounded(&root.join(field), deadline) {
            Ok(Some(bytes)) => {
                let bytes = trim_ascii(&bytes);
                if !bytes.is_empty()
                    && let Ok(text) = std::str::from_utf8(bytes)
                    && !text.chars().any(char::is_control)
                {
                    return Some(text.to_owned());
                }
            }
            Ok(None) => {}
            Err(_) => *read_errors += 1,
        }
    }
    None
}

fn read_bounded(path: &Path, deadline: Instant) -> Result<Option<Vec<u8>>, ()> {
    if Instant::now() >= deadline {
        return Err(());
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let mut bytes = Vec::new();
    file.by_ref()
        .take((DEVICE_INVENTORY_FIELD_CEILING + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() > DEVICE_INVENTORY_FIELD_CEILING {
        return Err(());
    }
    Ok(Some(bytes))
}

fn append_part(target: &mut Vec<u8>, part: &[u8]) -> Result<(), DeviceInventoryError> {
    if target.len().saturating_add(part.len()).saturating_add(8) > DEVICE_INVENTORY_FIELD_CEILING {
        return Err(error(
            DeviceInventoryErrorKind::MalformedSnapshot,
            "device-inventory-identity-too-large",
            "Linux provider identity material exceeded its bound",
        ));
    }
    target.extend_from_slice(&(part.len() as u64).to_le_bytes());
    target.extend_from_slice(part);
    Ok(())
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
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

    #[test]
    fn gpu_filter_rejects_connector_names() {
        let spec = SPECS
            .iter()
            .find(|spec| spec.kind == DeviceKind::Gpu)
            .unwrap();
        assert!(matches_entry(spec, OsStr::new("card0")));
        assert!(!matches_entry(spec, OsStr::new("card0-HDMI-A-1")));
    }

    #[test]
    fn bounded_identity_builder_refuses_oversize_material() {
        let mut material = Vec::new();
        assert!(append_part(&mut material, &[1; DEVICE_INVENTORY_FIELD_CEILING]).is_err());
    }
}
