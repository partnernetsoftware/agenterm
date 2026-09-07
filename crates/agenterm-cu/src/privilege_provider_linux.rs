//! Linux polkit boundary for the fixed ACU privilege provider.
//!
//! `pkexec` authenticates the user session and starts the root-owned installed
//! copy. This entry refuses every development/worktree copy, authenticates the
//! invoking uid from `PKEXEC_UID`, hashes the running inode through
//! `/proc/self/exe`, and only then enters the shared provider coordinator.

use std::{
    fs::{self, File, Metadata},
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::{
    CuError,
    privilege_apply::{MAX_REQUEST_BYTES, PrivilegeProviderNamespace},
    privilege_provider::{FixedProviderAuthority, execute_one_shot},
};

pub const INSTALLED_PROVIDER_PATH: &str = "/usr/libexec/agenterm/agenterm-cu";
pub const PROVIDER_STATE_ROOT: &str = "/var/lib/agenterm/cu-privilege";
const SELF_EXE_PATH: &str = "/proc/self/exe";
const PRINCIPAL_DOMAIN: &[u8] = b"agenterm-cu/linux-polkit-principal/v1\0";
const PROVIDER_DOMAIN: &[u8] = b"agenterm-cu/linux-polkit-provider/v1\0";

pub(crate) fn run_stdio() -> i32 {
    match run_stdio_result() {
        Ok(reply) => write_reply(&reply).unwrap_or(5),
        Err(error) => {
            let value = serde_json::json!({
                "ok": false,
                "error": { "code": error.code, "message": error.message }
            });
            write_reply(&value).unwrap_or(5).max(1)
        }
    }
}

fn run_stdio_result() -> Result<serde_json::Value, CuError> {
    let authority = native_authority()?;
    let mut request = Vec::new();
    std::io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut request)
        .map_err(|error| {
            CuError::new(
                "privilege_request_unavailable",
                format!("provider could not read its bounded request: {error}"),
            )
        })?;
    if request.len() > MAX_REQUEST_BYTES {
        return Err(CuError::new(
            "privilege_request_size_invalid",
            "privilege apply request exceeds its byte budget",
        ));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            CuError::new(
                "privilege_provider_clock_invalid",
                "provider clock is before the Unix epoch",
            )
        })?
        .as_millis();
    let now = i64::try_from(now).map_err(|_| {
        CuError::new(
            "privilege_provider_clock_invalid",
            "provider clock exceeds the supported millisecond range",
        )
    })?;
    serde_json::to_value(execute_one_shot(&authority, &request, now)?).map_err(|error| {
        CuError::new(
            "privilege_reply_invalid",
            format!("provider could not serialize its closed reply: {error}"),
        )
    })
}

fn native_authority() -> Result<FixedProviderAuthority, CuError> {
    // SAFETY: libc exposes these process attributes without pointers or
    // ownership transfer. The fixed helper is required to run as root.
    let effective_uid = unsafe { libc::geteuid() };
    if effective_uid != 0 {
        return Err(CuError::new(
            "privilege_provider_not_elevated",
            "the Linux privilege provider must be started by polkit as root",
        ));
    }
    let invoking_uid = parse_pkexec_uid()?;
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

fn parse_pkexec_uid() -> Result<u32, CuError> {
    let raw = std::env::var("PKEXEC_UID").map_err(|_| {
        CuError::new(
            "privilege_origin_unavailable",
            "polkit did not identify the invoking uid",
        )
    })?;
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CuError::new(
            "privilege_origin_invalid",
            "polkit invoking uid is not canonical decimal",
        ));
    }
    let uid = raw.parse::<u32>().map_err(|_| {
        CuError::new(
            "privilege_origin_invalid",
            "polkit invoking uid is outside the native uid range",
        )
    })?;
    if uid.to_string() != raw {
        return Err(CuError::new(
            "privilege_origin_invalid",
            "polkit invoking uid must not contain leading zeroes",
        ));
    }
    Ok(uid)
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

fn write_reply(value: &serde_json::Value) -> Result<i32, ()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value).map_err(|_| ())?;
    stdout.write_all(b"\n").map_err(|_| ())?;
    stdout.flush().map_err(|_| ())?;
    Ok(
        if value.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
            1
        } else {
            0
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn pkexec_uid_parser_is_canonical_and_bounded() {
        // SAFETY: this test serializes its environment changes inside the
        // process through one mutex and restores the prior value.
        static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV.lock().unwrap();
        let before = std::env::var_os("PKEXEC_UID");
        unsafe { std::env::set_var("PKEXEC_UID", "501") };
        assert_eq!(parse_pkexec_uid().unwrap(), 501);
        for invalid in ["", "0501", "-1", "1x", "4294967296"] {
            unsafe { std::env::set_var("PKEXEC_UID", invalid) };
            assert!(parse_pkexec_uid().is_err(), "accepted {invalid:?}");
        }
        match before {
            Some(value) => unsafe { std::env::set_var("PKEXEC_UID", value) },
            None => unsafe { std::env::remove_var("PKEXEC_UID") },
        }
    }

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
    fn polkit_policy_binds_the_fixed_path_and_only_provider_mode() {
        let policy = include_str!(
            "../../../packaging/privilege/linux/com.partnernetsoftware.agenterm.cu.privilege.policy"
        );
        assert!(policy.contains("<allow_active>auth_admin</allow_active>"));
        assert!(policy.contains(&format!(
            "<annotate key=\"org.freedesktop.policykit.exec.path\">{INSTALLED_PROVIDER_PATH}</annotate>"
        )));
        assert!(policy.contains(&format!(
            "<annotate key=\"org.freedesktop.policykit.exec.argv1\">{}</annotate>",
            crate::PRIVILEGE_PROVIDER_ARG
        )));
        assert!(!policy.contains("allow_gui"));
    }
}
