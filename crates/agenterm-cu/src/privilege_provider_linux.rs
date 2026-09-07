//! Linux fixed-identity boundary for the ACU privilege provider.
//!
//! The system broker authenticates callers through kernel peer credentials,
//! refuses development/worktree copies, hashes the running inode via
//! `/proc/self/exe`, and enters the provider-private replay coordinator.

use std::{
    fs::{self, File, Metadata},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    CuError, privilege_apply::PrivilegeProviderNamespace,
    privilege_provider::FixedProviderAuthority,
};

pub const INSTALLED_PROVIDER_PATH: &str = "/usr/libexec/agenterm/agenterm-cu";
pub const PROVIDER_STATE_ROOT: &str = "/var/lib/agenterm/cu-privilege";
const SELF_EXE_PATH: &str = "/proc/self/exe";
const PRINCIPAL_DOMAIN: &[u8] = b"agenterm-cu/linux-polkit-principal/v1\0";
const PROVIDER_DOMAIN: &[u8] = b"agenterm-cu/linux-polkit-provider/v1\0";

/// Construct provider authority for a UID already authenticated by the Linux
/// system-broker platform boundary. No wire request or environment value can
/// call this function directly.
pub(crate) fn authority_for_uid(invoking_uid: u32) -> Result<FixedProviderAuthority, CuError> {
    require_elevated_provider()?;
    if invoking_uid == 0 {
        return Err(CuError::new(
            "privilege_origin_invalid",
            "the Linux privilege broker requires an ordinary-user peer",
        ));
    }
    validate_provider_identity()?;
    let provider_binary_digest = hash_file(Path::new(SELF_EXE_PATH))?;
    let principal_digest = digest_parts(PRINCIPAL_DOMAIN, &[&invoking_uid.to_string()]);
    let provider_identity_digest = digest_parts(
        PROVIDER_DOMAIN,
        &[INSTALLED_PROVIDER_PATH, &provider_binary_digest],
    );
    FixedProviderAuthority::from_native_boundary(
        PrivilegeProviderNamespace::LinuxPolkit,
        PathBuf::from(PROVIDER_STATE_ROOT),
        principal_digest,
        provider_identity_digest,
    )
}

fn require_elevated_provider() -> Result<(), CuError> {
    // SAFETY: libc exposes these process attributes without pointers or
    // ownership transfer. The fixed helper is required to run as root.
    let effective_uid = unsafe { libc::geteuid() };
    if effective_uid != 0 {
        return Err(CuError::new(
            "privilege_provider_not_elevated",
            "the Linux privilege provider must run as the fixed root service",
        ));
    }
    Ok(())
}

fn validate_provider_identity() -> Result<(), CuError> {
    let installed = Path::new(INSTALLED_PROVIDER_PATH);
    validate_fixed_path(installed)?;
    prepare_and_validate_state_root()?;
    let installed_metadata = fs::metadata(installed).map_err(provider_identity_error)?;
    let running_metadata = fs::metadata(SELF_EXE_PATH).map_err(provider_identity_error)?;
    if installed_metadata.dev() != running_metadata.dev()
        || installed_metadata.ino() != running_metadata.ino()
    {
        return Err(CuError::new(
            "privilege_provider_identity_invalid",
            "the running provider is not the root-owned installed executable",
        ));
    }
    Ok(())
}

fn prepare_and_validate_state_root() -> Result<(), CuError> {
    let product = Path::new("/var/lib/agenterm");
    let state = Path::new(PROVIDER_STATE_ROOT);
    fs::create_dir(product)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(provider_identity_error)?;
    fs::create_dir(state)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(provider_identity_error)?;
    agenterm_platform::filesystem::protect_private_directory(state)
        .map_err(provider_identity_error)?;
    for path in [Path::new("/var"), Path::new("/var/lib"), product, state] {
        validate_root_owned_component(
            path,
            &fs::symlink_metadata(path).map_err(provider_identity_error)?,
            false,
        )?;
    }
    Ok(())
}

fn validate_fixed_path(file: &Path) -> Result<(), CuError> {
    for path in [
        Path::new("/usr"),
        Path::new("/usr/libexec"),
        Path::new("/usr/libexec/agenterm"),
        file,
    ] {
        let metadata = fs::symlink_metadata(path).map_err(provider_identity_error)?;
        validate_root_owned_component(path, &metadata, path == file)?;
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
                "fixed provider component is not a root-owned non-writable {}: {}",
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
        format!("fixed Linux provider identity could not be verified: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn fixed_component_rejects_writable_or_non_executable_files() {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("agenterm-provider-metadata-{}", std::process::id()));
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
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn polkit_policy_and_service_bind_the_fixed_broker_without_pkexec() {
        let policy = include_str!(
            "../../../packaging/privilege/linux/com.partnernetsoftware.agenterm.cu.privilege.policy"
        );
        let service = include_str!(
            "../../../packaging/privilege/linux/com.partnernetsoftware.agenterm.cu.privilege.service"
        );
        assert!(policy.contains("<allow_active>auth_admin</allow_active>"));
        assert!(!policy.contains("org.freedesktop.policykit.exec."));
        assert!(service.contains(&format!(
            "ExecStart={INSTALLED_PROVIDER_PATH} {}",
            crate::PRIVILEGE_BROKER_ARG
        )));
        assert!(!service.contains("StandardInput=socket"));
        assert!(!policy.contains("allow_gui"));
    }
}
