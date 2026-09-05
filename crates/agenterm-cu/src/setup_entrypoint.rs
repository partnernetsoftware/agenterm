//! Stable user entrypoint installation for the packaged `agenterm-cu` binary.
//!
//! The entrypoint is a tiny launcher, not a second copy of the executable. That
//! preserves the packaged binary's sibling `libagenterm` lookup and gives the
//! compatibility shell one fixed PATH entry without owning filesystem effects.

use std::{
    fs,
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
};

use agenterm_platform::{
    filesystem::user_home_directory, filesystem_publish::write_file_atomic, locking::PathLock,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::reply::CuError;

const SCHEMA: u32 = 1;
const MARKER: &str = "agenterm-cu-launcher";
const MAX_LAUNCHER_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupMode {
    Check,
    Apply,
}

impl SetupMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Apply => "apply",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryState {
    Missing,
    Ready,
    Stale,
    Conflict,
}

impl EntryState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Ready => "ready",
            Self::Stale => "stale",
            Self::Conflict => "conflict",
        }
    }
}

#[derive(Debug)]
struct Inspection {
    state: EntryState,
    reason: &'static str,
    observed_version: Option<String>,
}

#[derive(Serialize)]
struct EntrypointReply {
    schema: u32,
    mode: &'static str,
    status: &'static str,
    platform: &'static str,
    entrypoint: EntrypointDetail,
    action: ActionDetail,
    effect: &'static str,
}

#[derive(Serialize)]
struct EntrypointDetail {
    name: &'static str,
    kind: &'static str,
    path: String,
    expected_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_version: Option<String>,
    source_sha256: String,
    reason: &'static str,
}

#[derive(Serialize)]
struct ActionDetail {
    performed: bool,
    outcome: &'static str,
}

pub fn default_bin_dir() -> Result<PathBuf, CuError> {
    user_home_directory()
        .map(|home| home.join(".local").join("bin"))
        .map_err(|error| {
            CuError::new(
                "setup_home_unavailable",
                format!("current user home is unavailable: {error}"),
            )
        })
}

pub fn run(source: &Path, bin_dir: &Path, mode: SetupMode) -> Result<serde_json::Value, CuError> {
    let source = validate_source(source)?;
    let source_sha256 = sha256_file(&source).map_err(|error| {
        CuError::new(
            "setup_entrypoint_source_invalid",
            format!("read current agenterm-cu executable failed: {error}"),
        )
    })?;
    let target = bin_dir.join(entrypoint_name());
    reject_source_target_alias(&source, &target)?;
    let expected = launcher_bytes(&source, &source_sha256)?;
    let before = inspect(&target, &expected)?;

    if mode == SetupMode::Check {
        return reply(mode, &target, &source_sha256, before, false, "checked");
    }
    if before.state == EntryState::Conflict {
        return Err(conflict_error(&target, before.reason));
    }
    if before.state == EntryState::Ready {
        return reply(mode, &target, &source_sha256, before, false, "unchanged");
    }

    fs::create_dir_all(bin_dir).map_err(|error| {
        CuError::new(
            "setup_entrypoint_publish_failed",
            format!("create entrypoint directory failed: {error}"),
        )
        .with_detail(serde_json::json!({ "effect": "none" }))
    })?;
    let lock_path = bin_dir.join(format!(".{MARKER}.lock"));
    let _lock = PathLock::acquire(&lock_path).map_err(|error| {
        CuError::new(
            "setup_entrypoint_lock_failed",
            format!("acquire entrypoint publication lock failed: {error}"),
        )
        .with_detail(serde_json::json!({ "effect": "none" }))
    })?;

    let under_lock = inspect(&target, &expected)?;
    if under_lock.state == EntryState::Conflict {
        return Err(conflict_error(&target, under_lock.reason));
    }
    if under_lock.state == EntryState::Ready {
        return reply(
            mode,
            &target,
            &source_sha256,
            under_lock,
            false,
            "unchanged",
        );
    }
    let outcome = if under_lock.state == EntryState::Missing {
        "installed"
    } else {
        "repaired"
    };

    write_file_atomic(&target, |file| {
        file.write_all(&expected)?;
        set_launcher_permissions(file)
    })
    .map_err(|error| {
        let effect = if error.published() {
            "committed"
        } else {
            "none"
        };
        CuError::new(
            "setup_entrypoint_publish_failed",
            format!("publish complete entrypoint failed: {error}"),
        )
        .with_detail(serde_json::json!({
            "effect": effect,
            "durability": if error.published() { "unknown" } else { "unchanged" },
        }))
    })?;

    let after = inspect(&target, &expected)?;
    if after.state != EntryState::Ready {
        return Err(CuError::new(
            "setup_entrypoint_readback_failed",
            "published entrypoint did not match the exact expected bytes and mode",
        )
        .with_detail(serde_json::json!({ "effect": "committed" })));
    }
    reply(mode, &target, &source_sha256, after, true, outcome)
}

fn reply(
    mode: SetupMode,
    target: &Path,
    source_sha256: &str,
    inspection: Inspection,
    performed: bool,
    outcome: &'static str,
) -> Result<serde_json::Value, CuError> {
    serde_json::to_value(EntrypointReply {
        schema: SCHEMA,
        mode: mode.as_str(),
        status: inspection.state.as_str(),
        platform: platform_name(),
        entrypoint: EntrypointDetail {
            name: "agenterm-cu",
            kind: entrypoint_kind(),
            path: target.to_string_lossy().into_owned(),
            expected_version: env!("CARGO_PKG_VERSION"),
            observed_version: inspection.observed_version,
            source_sha256: source_sha256.to_owned(),
            reason: inspection.reason,
        },
        action: ActionDetail { performed, outcome },
        effect: if performed { "committed" } else { "none" },
    })
    .map_err(|error| {
        CuError::new(
            "setup_entrypoint_serialization_failed",
            format!("serialize setup result failed: {error}"),
        )
    })
}

fn validate_source(source: &Path) -> Result<PathBuf, CuError> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        CuError::new(
            "setup_entrypoint_source_invalid",
            format!("inspect current agenterm-cu executable failed: {error}"),
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(CuError::new(
            "setup_entrypoint_source_invalid",
            "current agenterm-cu executable must be a real regular file entry",
        ));
    }
    fs::canonicalize(source).map_err(|error| {
        CuError::new(
            "setup_entrypoint_source_invalid",
            format!("resolve current agenterm-cu executable failed: {error}"),
        )
    })
}

fn reject_source_target_alias(source: &Path, target: &Path) -> Result<(), CuError> {
    if fs::canonicalize(target).ok().as_deref() == Some(source) {
        return Err(CuError::new(
            "setup_entrypoint_source_target_alias",
            "entrypoint target is the running executable; refusing self-overwrite",
        ));
    }
    Ok(())
}

fn inspect(target: &Path, expected: &[u8]) -> Result<Inspection, CuError> {
    let metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Inspection {
                state: EntryState::Missing,
                reason: "absent",
                observed_version: None,
            });
        }
        Err(error) => {
            return Err(CuError::new(
                "setup_entrypoint_inspect_failed",
                format!("inspect entrypoint failed: {error}"),
            ));
        }
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Ok(Inspection {
            state: EntryState::Conflict,
            reason: "link-like-or-non-file-entry",
            observed_version: None,
        });
    }
    if metadata.len() > MAX_LAUNCHER_BYTES {
        return Ok(Inspection {
            state: EntryState::Conflict,
            reason: "foreign-entry",
            observed_version: None,
        });
    }
    let bytes = fs::read(target).map_err(|error| {
        CuError::new(
            "setup_entrypoint_inspect_failed",
            format!("read entrypoint failed: {error}"),
        )
    })?;
    if !owned_launcher(&bytes) {
        return Ok(Inspection {
            state: EntryState::Conflict,
            reason: "foreign-entry",
            observed_version: None,
        });
    }
    let observed_version = marker_value(&bytes, "version=");
    if bytes == expected && launcher_permissions_ready(&metadata) {
        return Ok(Inspection {
            state: EntryState::Ready,
            reason: "exact",
            observed_version,
        });
    }
    Ok(Inspection {
        state: EntryState::Stale,
        reason: if observed_version.as_deref() == Some(env!("CARGO_PKG_VERSION")) {
            "source-or-mode-changed"
        } else {
            "version-changed"
        },
        observed_version,
    })
}

fn owned_launcher(bytes: &[u8]) -> bool {
    let prefix = if cfg!(windows) {
        format!("@echo off\r\nrem {MARKER} schema={SCHEMA} ")
    } else {
        format!("#!/bin/sh\n# {MARKER} schema={SCHEMA} ")
    };
    bytes.starts_with(prefix.as_bytes())
}

fn marker_value(bytes: &[u8], key: &str) -> Option<String> {
    let first = std::str::from_utf8(bytes).ok()?.lines().nth(1)?;
    first
        .split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .map(str::to_owned)
}

fn launcher_bytes(source: &Path, source_sha256: &str) -> Result<Vec<u8>, CuError> {
    let source = source.to_str().ok_or_else(|| {
        CuError::new(
            "setup_entrypoint_source_invalid",
            "current agenterm-cu executable path is not valid UTF-8",
        )
    })?;
    if source.as_bytes().contains(&0) || source.contains(['\r', '\n']) {
        return Err(CuError::new(
            "setup_entrypoint_source_invalid",
            "current agenterm-cu executable path contains a control character",
        ));
    }
    #[cfg(windows)]
    {
        if source.contains('"') {
            return Err(CuError::new(
                "setup_entrypoint_source_invalid",
                "current agenterm-cu executable path contains a quote",
            ));
        }
        let source = source.replace('%', "%%");
        Ok(format!(
            "@echo off\r\nrem {MARKER} schema={SCHEMA} version={} source-sha256={source_sha256}\r\nsetlocal DisableDelayedExpansion\r\n\"{source}\" %*\r\nexit /b %errorlevel%\r\n",
            env!("CARGO_PKG_VERSION")
        )
        .into_bytes())
    }
    #[cfg(not(windows))]
    {
        let source = format!("'{}'", source.replace('\'', "'\"'\"'"));
        Ok(format!(
            "#!/bin/sh\n# {MARKER} schema={SCHEMA} version={} source-sha256={source_sha256}\nexec {source} \"$@\"\n",
            env!("CARGO_PKG_VERSION")
        )
        .into_bytes())
    }
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(unix)]
fn set_launcher_permissions(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    file.set_permissions(fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_launcher_permissions(_file: &fs::File) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn launcher_permissions_ready(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o777 == 0o700
}

#[cfg(not(unix))]
fn launcher_permissions_ready(_metadata: &fs::Metadata) -> bool {
    true
}

fn conflict_error(target: &Path, reason: &str) -> CuError {
    CuError::new(
        "setup_entrypoint_conflict",
        format!(
            "entrypoint target is not an ACU-owned regular launcher ({reason}): {}",
            target.display()
        ),
    )
    .with_detail(serde_json::json!({ "effect": "none", "reason": reason }))
}

#[cfg(windows)]
const fn entrypoint_name() -> &'static str {
    "agenterm-cu.cmd"
}

#[cfg(not(windows))]
const fn entrypoint_name() -> &'static str {
    "agenterm-cu"
}

#[cfg(windows)]
const fn entrypoint_kind() -> &'static str {
    "windows-cmd"
}

#[cfg(not(windows))]
const fn entrypoint_kind() -> &'static str {
    "posix-sh"
}

#[cfg(target_os = "windows")]
const fn platform_name() -> &'static str {
    "win32"
}

#[cfg(target_os = "macos")]
const fn platform_name() -> &'static str {
    "darwin"
}

#[cfg(target_os = "linux")]
const fn platform_name() -> &'static str {
    "linux"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-setup-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join(if cfg!(windows) {
            "source.exe"
        } else {
            "source"
        });
        fs::write(&source, b"exact agenterm-cu fixture").unwrap();
        let bin = root.join("bin");
        let target = bin.join(entrypoint_name());
        (root, source, target)
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[test]
    fn check_missing_is_zero_write() {
        let (root, source, target) = fixture();
        let bin = target.parent().unwrap();
        let value = run(&source, bin, SetupMode::Check).unwrap();
        assert_eq!(value["status"], "missing");
        assert_eq!(value["action"]["performed"], false);
        assert!(!bin.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn apply_is_exact_and_idempotent_then_repairs_owned_stale() {
        let (root, source, target) = fixture();
        let bin = target.parent().unwrap();
        let first = run(&source, bin, SetupMode::Apply).unwrap();
        assert_eq!(first["action"]["outcome"], "installed");
        assert_eq!(first["status"], "ready");
        let exact = fs::read(&target).unwrap();
        let second = run(&source, bin, SetupMode::Apply).unwrap();
        assert_eq!(second["action"]["outcome"], "unchanged");
        assert_eq!(fs::read(&target).unwrap(), exact);

        let mut stale = exact;
        stale.extend_from_slice(b"# stale\n");
        fs::write(&target, stale).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let check = run(&source, bin, SetupMode::Check).unwrap();
        assert_eq!(check["status"], "stale");
        let repaired = run(&source, bin, SetupMode::Apply).unwrap();
        assert_eq!(repaired["action"]["outcome"], "repaired");
        assert_eq!(repaired["status"], "ready");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn foreign_entry_is_preserved() {
        let (root, source, target) = fixture();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"user-owned file").unwrap();
        let error = run(&source, target.parent().unwrap(), SetupMode::Apply).unwrap_err();
        assert_eq!(error.code, "setup_entrypoint_conflict");
        assert_eq!(fs::read(&target).unwrap(), b"user-owned file");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_entry_is_preserved_with_its_referent() {
        use std::os::unix::fs::symlink;

        let (root, source, target) = fixture();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let referent = root.join("referent");
        fs::write(&referent, b"keep").unwrap();
        symlink(&referent, &target).unwrap();
        let error = run(&source, target.parent().unwrap(), SetupMode::Apply).unwrap_err();
        assert_eq!(error.code, "setup_entrypoint_conflict");
        assert_eq!(fs::read(&referent).unwrap(), b"keep");
        assert!(
            fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
