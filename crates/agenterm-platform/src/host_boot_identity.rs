//! Privacy-minimized identity for one exact operating-system boot.

use sha2::{Digest as _, Sha256};

#[path = "contract/host_boot_identity.rs"]
mod contract;

pub use contract::{HostBootIdentity, HostBootIdentityError, HostBootIdentityErrorKind};

#[cfg(target_os = "linux")]
#[path = "adapters/linux/host_boot_identity.rs"]
mod adapter;
#[cfg(target_os = "macos")]
#[path = "adapters/macos/host_boot_identity.rs"]
mod adapter;
#[cfg(windows)]
#[path = "adapters/windows/host_boot_identity.rs"]
mod adapter;

pub fn query() -> Result<HostBootIdentity, HostBootIdentityError> {
    let material = adapter::query_material()?;
    let mut digest = Sha256::new();
    digest.update(b"agenterm-platform/host-boot-identity/v1\0");
    digest.update((material.len() as u64).to_le_bytes());
    digest.update(&material);
    Ok(HostBootIdentity::new(digest.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_boot_identity_is_stable_and_nonzero() {
        let first = query().expect("current boot identity");
        let second = query().expect("repeated boot identity");
        assert_eq!(first, second);
        assert_ne!(first.as_bytes(), &[0; 32]);
    }
}
