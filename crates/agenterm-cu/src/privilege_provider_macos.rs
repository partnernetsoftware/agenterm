//! macOS fixed-identity boundary for the launchd privilege provider.
//!
//! This module deliberately stops short of installation. Production authority
//! exists only when launchd runs the root-owned binary at the fixed helper path;
//! a development copy cannot manufacture provider authority.

use std::{
    fs::{self, File, Metadata},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use agenterm_platform::system_broker::SystemBrokerPeerFacts;
use sha2::{Digest, Sha256};

use crate::{
    CuError, privilege_apply::PrivilegeProviderNamespace,
    privilege_provider::FixedProviderAuthority,
};

pub const INSTALLED_PROVIDER_PATH: &str =
    "/Library/PrivilegedHelperTools/com.partnernetsoftware.agenterm.cu.privilege";
pub const PROVIDER_STATE_ROOT: &str = "/private/var/db/agenterm/cu-privilege";
const PRINCIPAL_DOMAIN: &[u8] = b"agenterm-cu/macos-authorization-principal/v1\0";
const PROVIDER_DOMAIN: &[u8] = b"agenterm-cu/macos-authorization-provider/v1\0";

unsafe extern "C" {
    fn geteuid() -> u32;
}

/// Construct provider authority from the peer retained by the same launchd
/// socket which carries the request and its later consent proof.
pub(crate) fn authority_for_peer(
    peer: &SystemBrokerPeerFacts,
) -> Result<FixedProviderAuthority, CuError> {
    require_elevated_provider()?;
    if peer.effective_user_id == 0 {
        return Err(CuError::new(
            "privilege_origin_invalid",
            "the macOS privilege broker requires an ordinary-user peer",
        ));
    }
    validate_provider_identity()?;
    prepare_and_validate_state_root()?;
    let provider_binary_digest = hash_file(Path::new(INSTALLED_PROVIDER_PATH))?;
    let principal_digest = digest_parts(
        PRINCIPAL_DOMAIN,
        &[
            &peer.effective_user_id.to_string(),
            &peer.effective_group_id.to_string(),
        ],
    );
    let provider_identity_digest = digest_parts(
        PROVIDER_DOMAIN,
        &[INSTALLED_PROVIDER_PATH, &provider_binary_digest],
    );
    FixedProviderAuthority::from_native_boundary(
        PrivilegeProviderNamespace::MacosAuthorizationServices,
        PathBuf::from(PROVIDER_STATE_ROOT),
        principal_digest,
        provider_identity_digest,
    )
}

fn require_elevated_provider() -> Result<(), CuError> {
    // SAFETY: geteuid has no arguments and no failure contract.
    if unsafe { geteuid() } != 0 {
        return Err(CuError::new(
            "privilege_provider_not_elevated",
            "the macOS privilege provider must run as the fixed root launchd service",
        ));
    }
    Ok(())
}

fn validate_provider_identity() -> Result<(), CuError> {
    let installed = Path::new(INSTALLED_PROVIDER_PATH);
    for path in [
        Path::new("/Library"),
        Path::new("/Library/PrivilegedHelperTools"),
        installed,
    ] {
        let metadata = fs::symlink_metadata(path).map_err(provider_identity_error)?;
        validate_root_owned_component(path, &metadata, path == installed)?;
    }
    let running = std::env::current_exe().map_err(provider_identity_error)?;
    if running != installed {
        return Err(CuError::new(
            "privilege_provider_identity_invalid",
            "the running provider is not the fixed installed launchd helper",
        ));
    }
    let installed_metadata = fs::metadata(installed).map_err(provider_identity_error)?;
    let running_metadata = fs::metadata(&running).map_err(provider_identity_error)?;
    if installed_metadata.dev() != running_metadata.dev()
        || installed_metadata.ino() != running_metadata.ino()
    {
        return Err(CuError::new(
            "privilege_provider_identity_invalid",
            "the running provider does not match the fixed installed object",
        ));
    }
    Ok(())
}

fn prepare_and_validate_state_root() -> Result<(), CuError> {
    let product = Path::new("/private/var/db/agenterm");
    let state = Path::new(PROVIDER_STATE_ROOT);
    for path in [product, state] {
        match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = fs::symlink_metadata(path).map_err(provider_identity_error)?;
                validate_root_owned_component(path, &existing, false)?;
            }
            Err(error) => return Err(provider_identity_error(error)),
        }
        agenterm_platform::filesystem::protect_private_directory(path)
            .map_err(provider_identity_error)?;
        let protected = fs::symlink_metadata(path).map_err(provider_identity_error)?;
        validate_root_owned_component(path, &protected, false)?;
    }
    for path in [
        Path::new("/private"),
        Path::new("/private/var"),
        Path::new("/private/var/db"),
        product,
        state,
    ] {
        let metadata = fs::symlink_metadata(path).map_err(provider_identity_error)?;
        validate_root_owned_component(path, &metadata, false)?;
    }
    Ok(())
}

fn validate_root_owned_component(
    path: &Path,
    metadata: &Metadata,
    executable: bool,
) -> Result<(), CuError> {
    if !trusted_component(metadata, executable, 0) {
        return Err(CuError::new(
            "privilege_provider_identity_invalid",
            format!(
                "fixed macOS provider component is not a root-owned non-writable {}: {}",
                if executable {
                    "executable"
                } else {
                    "directory"
                },
                path.display()
            ),
        ));
    }
    Ok(())
}

fn trusted_component(metadata: &Metadata, executable: bool, expected_uid: u32) -> bool {
    !metadata.file_type().is_symlink()
        && if executable {
            metadata.is_file()
        } else {
            metadata.is_dir()
        }
        && metadata.uid() == expected_uid
        && metadata.mode() & 0o022 == 0
        && (!executable || metadata.mode() & 0o111 != 0)
}

fn hash_file(path: &Path) -> Result<String, CuError> {
    let mut file = File::open(path).map_err(provider_identity_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(provider_identity_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn digest_parts(domain: &[u8], parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn provider_identity_error(error: impl std::fmt::Display) -> CuError {
    CuError::new(
        "privilege_provider_identity_invalid",
        format!("fixed macOS provider identity could not be verified: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn fixed_component_rejects_writable_or_non_executable_files() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-macos-provider-metadata-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let file = root.join("provider");
        fs::write(&file, b"fixture").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        let owner = fs::metadata(&file).unwrap().uid();
        assert!(trusted_component(
            &fs::metadata(&file).unwrap(),
            true,
            owner
        ));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o775)).unwrap();
        assert!(!trusted_component(
            &fs::metadata(&file).unwrap(),
            true,
            owner
        ));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!trusted_component(
            &fs::metadata(&file).unwrap(),
            true,
            owner
        ));
        let link = root.join("provider-link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(!trusted_component(
            &fs::symlink_metadata(&link).unwrap(),
            true,
            owner
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn peer_principal_digest_uses_stable_user_identity_not_request_bytes() {
        let first = digest_parts(PRINCIPAL_DOMAIN, &["501", "20"]);
        let repeated = digest_parts(PRINCIPAL_DOMAIN, &["501", "20"]);
        let another = digest_parts(PRINCIPAL_DOMAIN, &["502", "20"]);
        assert_eq!(first, repeated);
        assert_ne!(first, another);
        assert_eq!(first.len(), 64);
    }
}
