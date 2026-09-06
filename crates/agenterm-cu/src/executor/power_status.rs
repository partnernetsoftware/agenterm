use agenterm_platform::host_resource_snapshot::{
    HostResourceSnapshotError, HostResourceSnapshotErrorKind,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;

use crate::{reply::CuError, target_binding::CurrentIdentityProvider};

pub(super) fn power_status_payload() -> Result<Value, CuError> {
    let provider = CurrentIdentityProvider::default_for_current_user()
        .map_err(|_| identity_error("the installation identity location is unavailable"))?;
    let host_identity = provider
        .load_installation_identity()
        .map_err(|_| identity_error("the installation identity is not enrolled; run setup"))?;
    let before = boot_anchor_identity()?;
    let snapshot =
        agenterm_platform::host_resource_snapshot::snapshot().map_err(resource_snapshot_error)?;
    let after = boot_anchor_identity()?;
    if before != after {
        return Err(CuError::new(
            "host_boot_identity_changed",
            "the operating-system boot anchor changed during observation",
        ));
    }
    let boot_identity = boot_identity(&host_identity, &before);
    Ok(json!({
        "host_identity": host_identity,
        "host_identity_scope": "installation",
        "boot_identity": boot_identity,
        "boot_identity_scope": "installation-and-boot",
        "boot_anchor": "native-boot-instance",
        "uptime_milliseconds": snapshot.uptime_milliseconds,
        "verified": true,
        "atomic_snapshot": false,
    }))
}

fn boot_anchor_identity() -> Result<[u8; 32], CuError> {
    agenterm_platform::host_boot_identity::query()
        .map(|identity| *identity.as_bytes())
        .map_err(|error| {
            CuError::new(
                "host_boot_identity_unavailable",
                "the operating-system boot anchor could not be verified",
            )
            .with_detail(json!({ "kind": format!("{:?}", error.kind()) }))
        })
}

fn boot_identity(host_identity: &str, boot_anchor: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"agenterm-cu/power-status/boot/v1\0");
    digest.update((host_identity.len() as u64).to_le_bytes());
    digest.update(host_identity.as_bytes());
    digest.update((boot_anchor.len() as u64).to_le_bytes());
    digest.update(boot_anchor);
    let digest = digest.finalize();
    let mut encoded = String::with_capacity("agt-cu-boot-v1-".len() + digest.len() * 2);
    encoded.push_str("agt-cu-boot-v1-");
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn identity_error(message: &'static str) -> CuError {
    CuError::new("host_identity_unavailable", message)
}

fn resource_snapshot_error(error: HostResourceSnapshotError) -> CuError {
    let code = match error.kind() {
        HostResourceSnapshotErrorKind::UptimeQuery => "host_uptime_query_failed",
        HostResourceSnapshotErrorKind::InvalidNativeValue => "host_resource_invalid",
        HostResourceSnapshotErrorKind::Overflow => "host_resource_overflow",
        _ => "host_resource_query_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_identity_is_scoped_to_installation_and_boot() {
        let first = boot_identity("host-a", b"boot-a");
        assert_eq!(first, boot_identity("host-a", b"boot-a"));
        assert_ne!(first, boot_identity("host-b", b"boot-a"));
        assert_ne!(first, boot_identity("host-a", b"boot-b"));
        assert!(first.starts_with("agt-cu-boot-v1-"));
        assert_eq!(first.len(), "agt-cu-boot-v1-".len() + 64);
    }
}
