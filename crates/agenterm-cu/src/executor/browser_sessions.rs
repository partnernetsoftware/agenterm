//! Public lifecycle for ACU-owned isolated browser sessions.

use std::{
    fmt::Write as _,
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use agenterm_platform::{
    file_identity::{FileIdentity, file_identity},
    filesystem::write_private_atomic,
    filesystem_cleanup::remove_tree,
    filesystem_open::{ExistingEntryType, open_existing_path},
    locking::{LockErrorKind, PathLock},
    process::{ProcessObservation, observe, start_identity},
    process_spawn::{DetachedSpawnMode, spawn_detached_child},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    CuError,
    browser_session::{
        BrowserSessionPaths, BrowserSessionRecord, BrowserSessionState, FileObjectIdentity,
        OWNER_MARKER_FILE, ProcessIdentity, create_session_directories, publish_record,
        read_record, same_generation, session_paths, sessions_root,
    },
    browser_session_owner::{
        BrowserOwnerSpec, BrowserStopRequest, OWNER_ARG, OWNER_SPEC_SCHEMA_VERSION, publish_spec,
        spec_path,
    },
};

const MARKER_BYTES: &[u8] = b"agenterm-cu-browser-session-v1\n";
const MAX_INVENTORY: usize = 4_096;
static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);

pub(super) fn browser_session_start_payload(
    name: &str,
    browser: &str,
    ready_timeout_ms: u64,
    ttl_ms: u64,
) -> Result<Value, CuError> {
    let root = sessions_root(true).map_err(state_unavailable)?;
    let _registry_lock = registry_lock(&root)?;
    let paths = session_paths(&root, name).map_err(|code| CuError::new(code, code))?;
    match fs::symlink_metadata(&paths.directory) {
        Ok(_) => {
            return Err(CuError::new(
                "browser_session_exists",
                "named browser session state already exists; inspect or remove it first",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(state_unavailable(error)),
    }
    let browser = canonical_browser(browser)?;
    create_session_directories(&paths).map_err(state_unavailable)?;
    write_private_atomic(&paths.profile.join(OWNER_MARKER_FILE), MARKER_BYTES)
        .map_err(state_unavailable)?;
    let profile_identity = opened_identity(&paths.profile)?;
    let nonce = new_nonce(name);
    let spec = BrowserOwnerSpec {
        schema_version: OWNER_SPEC_SCHEMA_VERSION,
        generation: 1,
        name: name.to_owned(),
        session_nonce: nonce.clone(),
        executable: browser,
        ready_timeout_ms,
        ttl_ms,
    };
    publish_spec(&spec_path(&paths), &spec).map_err(state_unavailable)?;

    let executable = std::env::current_exe().map_err(|error| {
        CuError::new(
            "browser_owner_spawn_failed",
            format!("current executable is unavailable: {error}"),
        )
    })?;
    let mut command = ProcessCommand::new(executable);
    command
        .arg(OWNER_ARG)
        .arg(&paths.directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut owner_child, spawn_mode) = spawn_detached_child(&mut command).map_err(|error| {
        CuError::new(
            "browser_owner_spawn_failed",
            format!("could not start browser session owner: {error}"),
        )
    })?;
    let owner = ProcessIdentity {
        pid: owner_child.id(),
        start_identity: start_identity(owner_child.id()).map_err(|_| {
            let _ = owner_child.kill();
            let _ = owner_child.wait();
            CuError::new(
                "browser_owner_identity_unavailable",
                "browser session owner start identity is unavailable",
            )
        })?,
    };
    let mut starting = BrowserSessionRecord {
        schema_version: crate::browser_session::REGISTRY_SCHEMA_VERSION,
        generation: spec.generation,
        name: name.to_owned(),
        session_nonce: nonce,
        state: BrowserSessionState::Starting,
        owner,
        owner_spawn_mode: spawn_mode.as_str().into(),
        profile_identity: profile_identity.into(),
        browser: None,
        endpoint: None,
        last_error_code: None,
    };
    publish_record(&paths.registry, &starting).map_err(state_unavailable)?;
    if !matches!(
        spawn_mode,
        DetachedSpawnMode::Independent | DetachedSpawnMode::CallerJobFallback
    ) {
        let _ = owner_child.kill();
        let _ = owner_child.wait();
        starting.state = BrowserSessionState::Failed;
        starting.last_error_code = Some("browser_owner_spawn_mode_unsupported".into());
        publish_record(&paths.registry, &starting).map_err(state_unavailable)?;
        return Err(CuError::new(
            "browser_owner_spawn_mode_unsupported",
            "the platform returned an unsupported browser owner lifetime mode",
        ));
    }
    wait_for_start(&paths, &starting, &mut owner_child, ready_timeout_ms)
}

pub(super) fn browser_session_status_payload(name: &str) -> Result<Value, CuError> {
    let root = sessions_root(false).map_err(state_unavailable)?;
    let paths = session_paths(&root, name).map_err(|code| CuError::new(code, code))?;
    let record = read_record(&paths.registry).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CuError::new(
                "browser_session_not_found",
                "named browser session does not exist",
            )
        } else {
            state_unavailable(error)
        }
    })?;
    status_value(&record)
}

pub(super) fn browser_session_list_payload() -> Result<Value, CuError> {
    let root = sessions_root(false).map_err(state_unavailable)?;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"schema_version": 1, "sessions": [], "total": 0}));
        }
        Err(error) => return Err(state_unavailable(error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(state_unavailable)?;
        if names.len() == MAX_INVENTORY {
            return Err(CuError::new(
                "browser_session_inventory_limit",
                "browser session inventory exceeds 4096 entries",
            ));
        }
        let metadata = entry.file_type().map_err(state_unavailable)?;
        if !metadata.is_dir() || metadata.is_symlink() {
            return Err(CuError::new(
                "browser_session_inventory_invalid",
                "browser session inventory contains a link-like or non-directory entry",
            ));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            CuError::new(
                "browser_session_inventory_invalid",
                "browser session inventory contains a non-UTF-8 name",
            )
        })?;
        crate::browser_session::validate_session_name(&name)
            .map_err(|code| CuError::new(code, code))?;
        names.push(name);
    }
    names.sort_unstable();
    let mut rows = Vec::with_capacity(names.len());
    for name in names {
        let paths = session_paths(&root, &name).expect("validated name");
        let record = read_record(&paths.registry).map_err(state_unavailable)?;
        rows.push(status_value(&record)?);
    }
    Ok(json!({"schema_version": 1, "total": rows.len(), "sessions": rows}))
}

pub(super) fn browser_session_stop_payload(
    name: &str,
    expect_stopped: bool,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    if !expect_stopped {
        return Err(CuError::new(
            "browser_session_stop_intent_required",
            "browser-session-stop requires --expect stopped",
        ));
    }
    let root = sessions_root(false).map_err(state_unavailable)?;
    let _registry_lock = registry_lock(&root)?;
    let paths = session_paths(&root, name).map_err(|code| CuError::new(code, code))?;
    let ready = read_record(&paths.registry).map_err(state_unavailable)?;
    if ready.state == BrowserSessionState::Stopped {
        return status_value(&ready);
    }
    if ready.state != BrowserSessionState::Ready {
        return Err(CuError::new(
            "browser_session_not_ready",
            "only a ready browser session can accept a verified stop request",
        ));
    }
    let browser = ready.browser.clone().ok_or_else(|| {
        CuError::new(
            "browser_session_state_invalid",
            "ready browser session omitted browser identity",
        )
    })?;
    let request = BrowserStopRequest {
        schema_version: OWNER_SPEC_SCHEMA_VERSION,
        generation: ready.generation,
        session_nonce: ready.session_nonce.clone(),
        owner: ready.owner.clone(),
        browser,
    };
    let bytes = serde_json::to_vec(&request)
        .map_err(|error| CuError::new("browser_stop_request_invalid", error.to_string()))?;
    write_private_atomic(&paths.stop, &bytes).map_err(state_unavailable)?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let current = read_record(&paths.registry).map_err(state_unavailable)?;
        if !same_generation(&ready, &current) {
            return Err(CuError::new(
                "browser_session_generation_replaced",
                "browser session generation changed while stopping",
            ));
        }
        match current.state {
            BrowserSessionState::Stopped if process_is_absent(&current.owner)? => {
                if let Some(browser) = &current.browser
                    && !process_is_absent(browser)?
                {
                    return Err(CuError::new(
                        "browser_session_cleanup_unverified",
                        "browser owner stopped but the exact browser process is still live",
                    ));
                }
                return status_value(&current);
            }
            BrowserSessionState::Failed | BrowserSessionState::OrphanedUncertain => {
                return Err(CuError::new(
                    "browser_session_cleanup_unverified",
                    "browser session entered an unverified terminal state",
                )
                .with_detail(status_value(&current)?));
            }
            _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            _ => {
                return Err(CuError::new(
                    "browser_session_stop_timeout",
                    "browser session did not reach a verified stopped state before the deadline",
                ));
            }
        }
    }
}

pub(super) fn browser_session_remove_payload(
    name: &str,
    expect_stopped: bool,
    expect_failed: bool,
) -> Result<Value, CuError> {
    let expected_state = expected_remove_state(expect_stopped, expect_failed)?;
    let root = sessions_root(false).map_err(state_unavailable)?;
    let _registry_lock = registry_lock(&root)?;
    let paths = session_paths(&root, name).map_err(|code| CuError::new(code, code))?;
    let directory = open_existing_path(&paths.directory, ExistingEntryType::Directory)
        .map_err(state_unavailable)?;
    let directory_identity = file_identity(&directory).map_err(state_unavailable)?;
    let owner_lock = PathLock::try_acquire(&paths.owner_lock).map_err(|error| {
        let code = if error.kind() == LockErrorKind::Contended {
            "browser_session_live"
        } else {
            "browser_session_state_unavailable"
        };
        CuError::new(code, error.to_string())
    })?;
    let record = read_record(&paths.registry).map_err(state_unavailable)?;
    let browser_absent = match record.browser.as_ref() {
        Some(browser) => process_is_absent(browser)?,
        None => true,
    };
    if record.state != expected_state || !process_is_absent(&record.owner)? || !browser_absent {
        return Err(CuError::new(
            "browser_session_remove_unverified",
            "browser session does not match the acknowledged terminal state or its processes are not independently verified absent",
        ));
    }
    let profile = open_existing_path(&paths.profile, ExistingEntryType::Directory)
        .map_err(state_unavailable)?;
    let actual_profile = file_identity(&profile).map_err(state_unavailable)?;
    if FileObjectIdentity::from(actual_profile) != record.profile_identity {
        return Err(CuError::new(
            "browser_session_identity_changed",
            "browser profile directory identity changed",
        ));
    }
    let marker_path = paths.profile.join(OWNER_MARKER_FILE);
    let marker_file =
        open_existing_path(&marker_path, ExistingEntryType::File).map_err(state_unavailable)?;
    let marker_len = marker_file.metadata().map_err(state_unavailable)?.len();
    if marker_len != MARKER_BYTES.len() as u64 {
        return Err(CuError::new(
            "browser_session_marker_invalid",
            "browser profile does not carry the exact ACU owner marker",
        ));
    }
    let mut marker = Vec::with_capacity(MARKER_BYTES.len());
    marker_file
        .take(MARKER_BYTES.len() as u64 + 1)
        .read_to_end(&mut marker)
        .map_err(state_unavailable)?;
    if marker != MARKER_BYTES {
        return Err(CuError::new(
            "browser_session_marker_invalid",
            "browser profile does not carry the exact ACU owner marker",
        ));
    }
    verify_known_entries(&paths)?;
    drop(profile);
    remove_tree(&paths.profile).map_err(state_unavailable)?;
    for file in [
        &paths.registry,
        &paths.stop,
        &paths.done,
        &spec_path(&paths),
    ] {
        remove_file_if_present(file).map_err(state_unavailable)?;
    }
    drop(owner_lock);
    remove_file_if_present(&paths.owner_lock).map_err(state_unavailable)?;
    let current_directory = open_existing_path(&paths.directory, ExistingEntryType::Directory)
        .map_err(state_unavailable)?;
    let current_identity = file_identity(&current_directory).map_err(state_unavailable)?;
    if !directory_identity.same_object(current_identity) {
        return Err(CuError::new(
            "browser_session_identity_changed",
            "browser session directory changed during removal",
        ));
    }
    drop(current_directory);
    fs::remove_dir(&paths.directory).map_err(state_unavailable)?;
    Ok(json!({
        "schema_version": 1,
        "name": name,
        "state": "removed",
        "verified": !paths.directory.exists(),
    }))
}

fn expected_remove_state(
    expect_stopped: bool,
    expect_failed: bool,
) -> Result<BrowserSessionState, CuError> {
    match (expect_stopped, expect_failed) {
        (true, false) => Ok(BrowserSessionState::Stopped),
        (false, true) => Ok(BrowserSessionState::Failed),
        _ => Err(CuError::new(
            "browser_session_remove_intent_required",
            "browser-session-remove requires exactly one of --expect stopped or --expect failed",
        )),
    }
}

fn canonical_browser(value: &str) -> Result<PathBuf, CuError> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(CuError::new(
            "browser_executable_invalid",
            "--browser must name one absolute executable file",
        ));
    }
    let canonical = fs::canonicalize(path).map_err(|_| {
        CuError::new(
            "browser_executable_invalid",
            "--browser must name one existing executable file",
        )
    })?;
    if !canonical.is_file() {
        return Err(CuError::new(
            "browser_executable_invalid",
            "--browser must name one existing executable file",
        ));
    }
    Ok(canonical)
}

fn registry_lock(root: &Path) -> Result<PathLock, CuError> {
    PathLock::try_acquire(&root.with_extension("registry.lock")).map_err(|error| {
        let code = if error.kind() == LockErrorKind::Contended {
            "browser_session_registry_busy"
        } else {
            "browser_session_state_unavailable"
        };
        CuError::new(code, error.to_string())
    })
}

fn opened_identity(path: &Path) -> Result<FileIdentity, CuError> {
    let file = open_existing_path(path, ExistingEntryType::Directory).map_err(state_unavailable)?;
    file_identity(&file).map_err(state_unavailable)
}

fn new_nonce(name: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(name.as_bytes());
    hash.update(std::process::id().to_le_bytes());
    hash.update(NEXT_NONCE.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hash.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    hash.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

fn wait_for_start(
    paths: &BrowserSessionPaths,
    starting: &BrowserSessionRecord,
    owner: &mut std::process::Child,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.saturating_add(5_000));
    loop {
        let current = read_record(&paths.registry).map_err(state_unavailable)?;
        if !same_generation(starting, &current) {
            return Err(CuError::new(
                "browser_session_generation_replaced",
                "browser session generation changed during startup",
            ));
        }
        match current.state {
            BrowserSessionState::Ready => return status_value(&current),
            BrowserSessionState::Failed | BrowserSessionState::OrphanedUncertain => {
                let code = current
                    .last_error_code
                    .clone()
                    .unwrap_or_else(|| "browser_session_start_failed".into());
                return Err(
                    CuError::new(code, "browser session owner did not become ready")
                        .with_detail(status_value(&current)?),
                );
            }
            _ if Instant::now() < deadline => {
                if owner.try_wait().ok().flatten().is_some() {
                    return Err(CuError::new(
                        "browser_owner_exited_before_ready",
                        "browser session owner exited before publishing a terminal state",
                    ));
                }
                thread::sleep(Duration::from_millis(25));
            }
            _ => {
                return Err(CuError::new(
                    "browser_session_ready_timeout",
                    "browser session owner did not become ready before the deadline",
                ));
            }
        }
    }
}

fn status_value(record: &BrowserSessionRecord) -> Result<Value, CuError> {
    let mut value = serde_json::to_value(record)
        .map_err(|error| CuError::new("browser_session_state_invalid", error.to_string()))?;
    let owner_observation = classify_process(&record.owner);
    let recorded_state = value["state"].clone();
    if matches!(
        record.state,
        BrowserSessionState::Starting | BrowserSessionState::Ready | BrowserSessionState::Stopping
    ) && owner_observation != "live"
    {
        value["state"] = json!("orphaned_uncertain");
        value["recorded_state"] = recorded_state;
    }
    value["owner_observation"] = json!(owner_observation);
    value["owned"] = json!(true);
    Ok(value)
}

fn classify_process(identity: &ProcessIdentity) -> &'static str {
    match observe(identity.pid) {
        ProcessObservation::Live {
            start_identity: Some(current),
        } if current == identity.start_identity => "live",
        ProcessObservation::Live { .. } => "pid_reused",
        ProcessObservation::Dead { .. } => "dead",
        ProcessObservation::Unknown { .. } => "unknown",
        _ => "unknown",
    }
}

fn process_is_absent(identity: &ProcessIdentity) -> Result<bool, CuError> {
    match classify_process(identity) {
        "dead" | "pid_reused" => Ok(true),
        "live" => Ok(false),
        _ => Err(CuError::new(
            "browser_session_liveness_unknown",
            "exact process liveness is unavailable",
        )),
    }
}

fn verify_known_entries(paths: &BrowserSessionPaths) -> Result<(), CuError> {
    for entry in fs::read_dir(&paths.directory).map_err(state_unavailable)? {
        let entry = entry.map_err(state_unavailable)?;
        let name = entry.file_name().into_string().map_err(|_| {
            CuError::new(
                "browser_session_remove_unverified",
                "browser session state contains a non-UTF-8 entry",
            )
        })?;
        if !matches!(
            name.as_str(),
            "profile"
                | "registry.json"
                | "owner.lock"
                | "owner-spec.json"
                | "stop.json"
                | "done.json"
        ) {
            return Err(CuError::new(
                "browser_session_remove_unverified",
                "browser session state contains an unowned entry",
            ));
        }
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn state_unavailable(error: impl std::fmt::Display) -> CuError {
    CuError::new("browser_session_state_unavailable", error.to_string())
}

impl From<FileIdentity> for FileObjectIdentity {
    fn from(value: FileIdentity) -> Self {
        Self {
            filesystem_id: value.filesystem_id,
            object_id: value.object_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_is_bounded_hex_and_changes_per_generation_attempt() {
        let first = new_nonce("work");
        let second = new_nonce("work");
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn removal_requires_one_exact_terminal_state() {
        assert_eq!(
            expected_remove_state(true, false).unwrap(),
            BrowserSessionState::Stopped
        );
        assert_eq!(
            expected_remove_state(false, true).unwrap(),
            BrowserSessionState::Failed
        );
        for (stopped, failed) in [(false, false), (true, true)] {
            assert_eq!(
                expected_remove_state(stopped, failed).unwrap_err().code,
                "browser_session_remove_intent_required"
            );
        }
    }
}
