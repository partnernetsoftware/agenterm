//! Bounded, privacy-preserving peripheral inventory for the current host.
//!
//! Native identifiers are consumed only inside this facade. Public identifiers
//! are installation-scoped HMAC-SHA256 pseudonyms derived from the already
//! enrolled current-target binding. Inventory never enrolls or rotates that
//! identity.

use std::{
    path::Path,
    time::{Duration, Instant},
};

pub use crate::contract::device_inventory::{
    DEVICE_INVENTORY_FIELD_CEILING, DEVICE_INVENTORY_MAX_ROWS,
    DEVICE_INVENTORY_PROVIDER_OUTPUT_CEILING, DEVICE_INVENTORY_SCAN_CEILING,
    DeviceIdentityContinuity, DeviceInventory, DeviceInventoryError, DeviceInventoryErrorKind,
    DeviceKind, DeviceProviderState, DeviceProviderStatus, DeviceRecord, DeviceSelector,
    ProviderState,
};
use crate::{
    contract::current_target_binding::CurrentTargetBindingErrorKind, current_target_binding,
    selected,
};

const INVENTORY_TIMEOUT: Duration = Duration::from_secs(15);
const PSEUDONYM_DOMAIN: &[u8] = b"agenterm-platform/device-inventory-id/v1";

pub(crate) struct NativeDeviceRecord {
    pub(crate) identity_material: Vec<u8>,
    pub(crate) identity_continuity: DeviceIdentityContinuity,
    pub(crate) kind: DeviceKind,
    pub(crate) name: Option<String>,
    pub(crate) vendor: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) transport: Option<String>,
}

pub(crate) struct NativeDeviceInventory {
    pub(crate) devices: Vec<NativeDeviceRecord>,
    pub(crate) providers: Vec<DeviceProviderStatus>,
}

/// Enumerate a bounded projection of peripherals using an existing private
/// installation identity. This function never creates or rotates that identity.
pub fn enumerate(
    private_state_dir: &Path,
    selector: DeviceSelector,
    max_rows: usize,
) -> Result<DeviceInventory, DeviceInventoryError> {
    enumerate_with_timeout(private_state_dir, selector, max_rows, INVENTORY_TIMEOUT)
}

/// Enumerate under the caller's remaining monotonic budget.
///
/// The platform provider is still capped at its ordinary inventory timeout;
/// the shorter caller budget is used by bounded polling surfaces so one sample
/// cannot silently overrun the whole watch.
pub fn enumerate_with_timeout(
    private_state_dir: &Path,
    selector: DeviceSelector,
    max_rows: usize,
    timeout: Duration,
) -> Result<DeviceInventory, DeviceInventoryError> {
    if timeout.is_zero() {
        return Err(error(
            DeviceInventoryErrorKind::Timeout,
            "device-inventory-timeout",
            "inventory has no remaining time budget",
        ));
    }
    if !(1..=DEVICE_INVENTORY_MAX_ROWS).contains(&max_rows) {
        return Err(error(
            DeviceInventoryErrorKind::InvalidLimit,
            "device-inventory-limit",
            format!("max_rows must be between 1 and {DEVICE_INVENTORY_MAX_ROWS}, got {max_rows}"),
        ));
    }
    if !private_state_dir.exists() {
        return Err(error(
            DeviceInventoryErrorKind::IdentityMissing,
            "device-inventory-identity-missing",
            "installation identity has not been enrolled",
        ));
    }
    // Fail before native collection when the caller has not explicitly
    // enrolled an installation identity. The returned public identity is not
    // used as pseudonym key material.
    let _ = current_target_binding::load_provider_identity(private_state_dir)
        .map_err(map_identity_error)?;
    let deadline = Instant::now()
        .checked_add(timeout.min(INVENTORY_TIMEOUT))
        .ok_or_else(|| {
            error(
                DeviceInventoryErrorKind::Timeout,
                "device-inventory-deadline-overflow",
                "inventory deadline could not be represented",
            )
        })?;
    let native = selected::device_inventory::enumerate_native(selector, deadline)?;
    let materials = pseudonym_materials(&native.devices)?;
    let material_refs: Vec<&[u8]> = materials.iter().map(Vec::as_slice).collect();
    let digests = current_target_binding::derive_installation_scoped_digests(
        private_state_dir,
        PSEUDONYM_DOMAIN,
        &material_refs,
    )
    .map_err(map_identity_error)?;
    finish_inventory(selector, native, max_rows, &digests)
}

fn finish_inventory(
    selector: DeviceSelector,
    native: NativeDeviceInventory,
    max_rows: usize,
    digests: &[[u8; 32]],
) -> Result<DeviceInventory, DeviceInventoryError> {
    let requested_provider_count = DeviceKind::ALL
        .iter()
        .filter(|kind| selector.includes(**kind))
        .count();
    if native.providers.len() != requested_provider_count
        || native.devices.len() > DEVICE_INVENTORY_SCAN_CEILING
        || native.providers.iter().any(|provider| {
            !selector.includes(provider.kind)
                || provider.visited > DEVICE_INVENTORY_SCAN_CEILING
                || provider.read_errors > provider.visited
        })
        || native.devices.len() != digests.len()
        || native.devices.iter().any(|device| {
            !selector.includes(device.kind)
                || device.identity_material.is_empty()
                || device.identity_material.len() > DEVICE_INVENTORY_FIELD_CEILING
        })
    {
        return Err(error(
            DeviceInventoryErrorKind::MalformedSnapshot,
            "device-inventory-counts-invalid",
            "provider returned an incoherent bounded inventory",
        ));
    }

    let mut devices = Vec::new();
    devices.try_reserve(native.devices.len()).map_err(|_| {
        error(
            DeviceInventoryErrorKind::ResourceLimit,
            "device-inventory-allocation",
            "device inventory allocation failed",
        )
    })?;
    for (device, digest) in native.devices.into_iter().zip(digests) {
        validate_public_field(device.name.as_deref())?;
        validate_public_field(device.vendor.as_deref())?;
        validate_public_field(device.model.as_deref())?;
        validate_public_field(device.transport.as_deref())?;
        devices.push(DeviceRecord {
            id: format!("agt-device-v1-{}", encode_hex(digest)),
            identity_continuity: device.identity_continuity,
            kind: device.kind,
            name: device.name,
            vendor: device.vendor,
            model: device.model,
            transport: device.transport,
        });
    }
    devices.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.id.cmp(&right.id))
    });
    let projection_truncated = devices.len() > max_rows;
    devices.truncate(max_rows);
    let provider_truncated = native.providers.iter().any(|provider| provider.truncated);
    let complete = !projection_truncated
        && native
            .providers
            .iter()
            .all(|provider| provider.state == DeviceProviderState::Complete);
    Ok(DeviceInventory {
        devices,
        providers: native.providers,
        truncated: projection_truncated || provider_truncated,
        complete,
    })
}

fn pseudonym_materials(
    devices: &[NativeDeviceRecord],
) -> Result<Vec<Vec<u8>>, DeviceInventoryError> {
    let mut materials = Vec::new();
    materials.try_reserve(devices.len()).map_err(|_| {
        error(
            DeviceInventoryErrorKind::ResourceLimit,
            "device-inventory-allocation",
            "device pseudonym material allocation failed",
        )
    })?;
    for device in devices {
        let mut material = Vec::with_capacity(device.identity_material.len() + 16);
        let kind = device.kind.as_str().as_bytes();
        material.extend_from_slice(&(kind.len() as u64).to_le_bytes());
        material.extend_from_slice(kind);
        material.extend_from_slice(&(device.identity_material.len() as u64).to_le_bytes());
        material.extend_from_slice(&device.identity_material);
        materials.push(material);
    }
    Ok(materials)
}

fn validate_public_field(value: Option<&str>) -> Result<(), DeviceInventoryError> {
    if value.is_some_and(|value| {
        value.len() > DEVICE_INVENTORY_FIELD_CEILING || value.chars().any(char::is_control)
    }) {
        return Err(error(
            DeviceInventoryErrorKind::MalformedSnapshot,
            "device-inventory-field-invalid",
            "provider returned an invalid public device field",
        ));
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn map_identity_error(
    failure: current_target_binding::CurrentTargetBindingError,
) -> DeviceInventoryError {
    let (kind, code) = match failure.kind() {
        CurrentTargetBindingErrorKind::Missing => (
            DeviceInventoryErrorKind::IdentityMissing,
            "device-inventory-identity-missing",
        ),
        CurrentTargetBindingErrorKind::Permission => (
            DeviceInventoryErrorKind::PermissionDenied,
            "device-inventory-identity-permission",
        ),
        CurrentTargetBindingErrorKind::Contended => (
            DeviceInventoryErrorKind::ProviderFailed,
            "device-inventory-identity-contended",
        ),
        CurrentTargetBindingErrorKind::Unsupported
        | CurrentTargetBindingErrorKind::Corrupt
        | CurrentTargetBindingErrorKind::Entropy
        | CurrentTargetBindingErrorKind::Native => (
            DeviceInventoryErrorKind::IdentityInvalid,
            "device-inventory-identity-invalid",
        ),
    };
    error(kind, code, "installation identity could not be loaded")
}

pub(crate) fn error(
    kind: DeviceInventoryErrorKind,
    code: &'static str,
    detail: impl Into<String>,
) -> DeviceInventoryError {
    DeviceInventoryError::new(kind, code, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_MISSING: AtomicU64 = AtomicU64::new(1);

    fn provider(kind: DeviceKind, state: DeviceProviderState) -> DeviceProviderStatus {
        DeviceProviderStatus {
            kind,
            state,
            provider: "fixture",
            visited: 1,
            read_errors: 0,
            truncated: false,
            code: None,
        }
    }

    fn native(material: &[u8]) -> NativeDeviceInventory {
        NativeDeviceInventory {
            devices: vec![NativeDeviceRecord {
                identity_material: material.to_vec(),
                identity_continuity: DeviceIdentityContinuity::ProviderStable,
                kind: DeviceKind::Usb,
                name: Some("Example".into()),
                vendor: None,
                model: None,
                transport: Some("usb".into()),
            }],
            providers: vec![provider(DeviceKind::Usb, DeviceProviderState::Complete)],
        }
    }

    #[test]
    fn pseudonyms_are_opaque_and_raw_material_never_projects() {
        let first =
            finish_inventory(DeviceSelector::Usb, native(b"serial-secret"), 8, &[[1; 32]]).unwrap();
        assert_eq!(first.devices[0].id.len(), "agt-device-v1-".len() + 64);
        assert!(first.devices[0].id.starts_with("agt-device-v1-"));
        assert!(!first.devices[0].id.contains("serial"));
    }

    #[test]
    fn projection_and_partial_provider_are_truthful() {
        let mut inventory = native(b"one");
        inventory.devices.push(NativeDeviceRecord {
            identity_material: b"two".to_vec(),
            identity_continuity: DeviceIdentityContinuity::Topology,
            kind: DeviceKind::Usb,
            name: None,
            vendor: None,
            model: None,
            transport: None,
        });
        inventory.providers[0].state = DeviceProviderState::Partial;
        let inventory =
            finish_inventory(DeviceSelector::Usb, inventory, 1, &[[3; 32], [4; 32]]).unwrap();
        assert!(inventory.truncated);
        assert!(!inventory.complete);
        assert_eq!(inventory.devices.len(), 1);
    }

    #[test]
    fn malformed_or_unrequested_native_rows_fail_closed() {
        let mut inventory = native(b"one");
        inventory.devices[0].kind = DeviceKind::Gpu;
        assert_eq!(
            finish_inventory(DeviceSelector::Usb, inventory, 8, &[[4; 32]])
                .unwrap_err()
                .kind(),
            DeviceInventoryErrorKind::MalformedSnapshot
        );
    }

    #[test]
    fn ordinary_inventory_never_enrolls_a_missing_identity() {
        let path = std::env::temp_dir().join(format!(
            "agenterm-device-inventory-missing-{}-{}",
            std::process::id(),
            NEXT_MISSING.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(!path.exists());
        let failure = enumerate(&path, DeviceSelector::Usb, 1).unwrap_err();
        assert_eq!(failure.kind(), DeviceInventoryErrorKind::IdentityMissing);
        assert!(!path.exists());
    }
}
