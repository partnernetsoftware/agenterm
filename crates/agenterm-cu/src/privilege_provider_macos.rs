//! macOS fixed-identity boundary for the launchd privilege provider.
//!
//! This module deliberately stops short of installation. Production authority
//! exists only when launchd runs the signed, fixed-path SMAppService helper
//! installed by a root-authorized package. Ordinary drag installation is not
//! provider authority: the complete bundle ancestry must remain root-owned.

use std::{
    fs::{self, File, Metadata},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    str::FromStr,
};

use agenterm_platform::system_broker::SystemBrokerPeerFacts;
use core_foundation::{
    base::TCFType,
    dictionary::{CFDictionary, CFDictionaryGetValue, CFDictionaryRef},
    string::{CFString, CFStringRef},
    url::CFURL,
};
use security_framework::os::macos::code_signing::{Flags, SecRequirement, SecStaticCode};
use sha2::{Digest, Sha256};

use crate::{
    CuError, privilege_apply::PrivilegeProviderNamespace,
    privilege_provider::FixedProviderAuthority,
};

pub const INSTALLED_APP_BUNDLE: &str = "/Applications/AgenTerm.app";
pub const INSTALLED_PROVIDER_PATH: &str =
    "/Applications/AgenTerm.app/Contents/Resources/com.partnernetsoftware.agenterm.cu.privilege";
pub const PROVIDER_STATE_ROOT: &str = "/private/var/db/agenterm/cu-privilege";
const APP_IDENTIFIER: &str = "com.partnernetsoftware.agenterm";
const HELPER_IDENTIFIER: &str = "com.partnernetsoftware.agenterm.cu.privilege";
const PRINCIPAL_DOMAIN: &[u8] = b"agenterm-cu/macos-authorization-principal/v1\0";
const PROVIDER_DOMAIN: &[u8] = b"agenterm-cu/macos-authorization-provider/v1\0";

unsafe extern "C" {
    fn geteuid() -> u32;
    static kSecCodeInfoTeamIdentifier: CFStringRef;
    fn SecCodeCopySigningInformation(
        code: *mut std::ffi::c_void,
        flags: u32,
        information: *mut CFDictionaryRef,
    ) -> i32;
}

const SEC_CS_SIGNING_INFORMATION: u32 = 1 << 1;

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
    validate_provider_signatures()?;
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
        Path::new("/Applications"),
        Path::new(INSTALLED_APP_BUNDLE),
        Path::new("/Applications/AgenTerm.app/Contents"),
        Path::new("/Applications/AgenTerm.app/Contents/Resources"),
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

fn validate_provider_signatures() -> Result<(), CuError> {
    let app_code = static_code(Path::new(INSTALLED_APP_BUNDLE), true)?;
    let helper_code = static_code(Path::new(INSTALLED_PROVIDER_PATH), false)?;
    let app_team = signing_team_id(&app_code)?;
    let helper_team = signing_team_id(&helper_code)?;
    if app_team != helper_team {
        return Err(CuError::new(
            "privilege_provider_signature_invalid",
            "the app and embedded privilege helper have different signing teams",
        ));
    }
    validate_signed_code(&app_code, APP_IDENTIFIER, &app_team, true)?;
    validate_signed_code(&helper_code, HELPER_IDENTIFIER, &helper_team, false)
}

fn valid_team_id(team_id: &str) -> bool {
    team_id.len() == 10
        && team_id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn static_code(path: &Path, bundle: bool) -> Result<SecStaticCode, CuError> {
    let url = CFURL::from_path(path, bundle).ok_or_else(|| {
        provider_signature_error(format!("cannot create a code URL for {}", path.display()))
    })?;
    SecStaticCode::from_path(&url, Flags::NONE).map_err(provider_signature_error)
}

fn signing_team_id(code: &SecStaticCode) -> Result<String, CuError> {
    let mut information: CFDictionaryRef = std::ptr::null();
    // SAFETY: code is live; Security.framework initializes one create-rule
    // dictionary on success. The fixed key is framework-owned.
    let status = unsafe {
        SecCodeCopySigningInformation(
            code.as_CFTypeRef() as *mut std::ffi::c_void,
            SEC_CS_SIGNING_INFORMATION,
            &raw mut information,
        )
    };
    if status != 0 || information.is_null() {
        return Err(provider_signature_error(format!(
            "SecCodeCopySigningInformation failed with OSStatus {status}"
        )));
    }
    // SAFETY: success returned a create-rule dictionary.
    let information = unsafe {
        CFDictionary::<*const std::ffi::c_void, *const std::ffi::c_void>::wrap_under_create_rule(
            information,
        )
    };
    // SAFETY: both references remain live for this lookup.
    let value = unsafe {
        CFDictionaryGetValue(
            information.as_concrete_TypeRef(),
            kSecCodeInfoTeamIdentifier.cast(),
        )
    };
    if value.is_null() {
        return Err(CuError::new(
            "privilege_provider_unsigned",
            "the macOS privilege provider has no Apple Team identifier",
        ));
    }
    // SAFETY: the documented signing-information value for this key is a
    // CFString and the owning dictionary remains live through conversion.
    let team = unsafe { CFString::wrap_under_get_rule(value.cast()) }.to_string();
    if !valid_team_id(&team) {
        return Err(CuError::new(
            "privilege_provider_signature_invalid",
            "the provider signing Team identifier is malformed",
        ));
    }
    Ok(team)
}

fn validate_signed_code(
    code: &SecStaticCode,
    identifier: &str,
    team_id: &str,
    bundle: bool,
) -> Result<(), CuError> {
    let requirement_text = format!(
        "anchor apple generic and identifier \"{identifier}\" and certificate leaf[subject.OU] = \"{team_id}\""
    );
    let requirement =
        SecRequirement::from_str(&requirement_text).map_err(provider_signature_error)?;
    let mut flags =
        Flags::STRICT_VALIDATE | Flags::CHECK_ALL_ARCHITECTURES | Flags::NO_NETWORK_ACCESS;
    if bundle {
        flags |= Flags::CHECK_NESTED_CODE;
    }
    code.check_validity(flags, &requirement)
        .map_err(provider_signature_error)
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

fn provider_signature_error(error: impl std::fmt::Display) -> CuError {
    CuError::new(
        "privilege_provider_signature_invalid",
        format!("fixed macOS provider signature could not be verified: {error}"),
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

    #[test]
    fn team_identifier_shape_is_bounded() {
        assert!(valid_team_id("ABCDEFGHIJ"));
        assert!(valid_team_id("A1B2C3D4E5"));
        assert!(!valid_team_id(""));
        assert!(!valid_team_id("abcdefghij"));
        assert!(!valid_team_id("ABCDEFGHIJK"));
    }

    #[test]
    fn deployment_asset_matches_runtime_identity() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packaging/privilege/macos/deployment.json"
        ))
        .unwrap();
        assert_eq!(
            manifest["app"]["install_path"].as_str(),
            Some(INSTALLED_APP_BUNDLE)
        );
        assert_eq!(
            manifest["install_authority"].as_str(),
            Some("root-owned-package-only")
        );
        assert_eq!(
            manifest["ordinary_drag_install_supported"].as_bool(),
            Some(false)
        );
        assert_eq!(
            manifest["helper"]["installed_path"].as_str(),
            Some(INSTALLED_PROVIDER_PATH)
        );
        assert_eq!(
            manifest["app"]["bundle_identifier"].as_str(),
            Some(APP_IDENTIFIER)
        );
        assert_eq!(
            manifest["helper"]["label"].as_str(),
            Some(HELPER_IDENTIFIER)
        );
        assert_eq!(
            manifest["authorization"]["right"].as_str(),
            Some(agenterm_platform::privilege_authorization::PRIVILEGE_ACTION_ID)
        );
        assert_ne!(
            manifest["authorization"]["right"].as_str(),
            manifest["helper"]["label"].as_str()
        );
        assert!(!agenterm_platform::privilege_authorization::PRIVILEGE_ACTION_ID.contains('*'));
        assert_eq!(
            manifest["launchd"]["socket_path"].as_str(),
            Some(agenterm_platform::system_broker::SYSTEM_BROKER_SOCKET)
        );
        assert_eq!(
            manifest["launchd"]["socket_key"].as_str(),
            agenterm_platform::system_broker::SYSTEM_BROKER_LAUNCHD_SOCKET
                .to_str()
                .ok()
        );
        assert_eq!(
            manifest["launchd"]["socket_mode"].as_u64(),
            Some(u64::from(
                agenterm_platform::system_broker::SYSTEM_BROKER_SOCKET_MODE
            ))
        );
    }
}
