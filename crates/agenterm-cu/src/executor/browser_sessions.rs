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

#[cfg(windows)]
use crate::browser_bridge::install_for_current_user_host;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::browser_bridge::{ACU_NATIVE_HOST_NAME, materialize_for_owned_profile_host};
use crate::browser_bridge::{BrowserBridgeInstall, BrowserBridgeInstallError, BrowserSetupEffect};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use agenterm_platform::filesystem::protect_private_directory;

const MARKER_BYTES: &[u8] = b"agenterm-cu-browser-session-v1\n";
const MAX_INVENTORY: usize = 4_096;
#[cfg(any(target_os = "linux", target_os = "macos"))]
const OWNED_NATIVE_MANIFEST_MAX_BYTES: usize = 64 * 1024;
static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);

pub(super) fn browser_session_start_payload(
    name: &str,
    browser: &str,
    bridge: bool,
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
    let bridge_install = if bridge {
        let native_host = crate::owner_executable::resolve_current().map_err(|error| {
            CuError::new(
                "browser_bridge_native_host_unavailable",
                "the sibling agenterm-cu browser bridge host is unavailable",
            )
            .with_detail(json!({
                "reason": error.reason(),
                "io_kind": error.kind().map(|kind| format!("{kind:?}")),
                "resolution": "current_exe_sibling",
                "session_state_created": false,
            }))
        })?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let install = materialize_for_owned_profile_host(&native_host);
        #[cfg(windows)]
        let install = install_for_current_user_host(&native_host);
        Some(install.map_err(bridge_install_error)?)
    } else {
        None
    };
    create_session_directories(&paths).map_err(state_unavailable)?;
    if let Some(install) = bridge_install.as_ref()
        && let Err(error) =
            publish_owned_native_manifest(&paths.profile, &install.native_manifest_file)
    {
        let cleanup_verified = remove_tree(&paths.directory).is_ok();
        return Err(CuError::new(
            "browser_session_bridge_manifest_publish_failed",
            "the owned browser profile native-host manifest could not be published",
        )
        .with_detail(json!({
            "effect": "performed-partial",
            "retry_safe": false,
            "shared_publication": bridge_install_summary(install),
            "owned_manifest_written": false,
            "session_cleanup_verified": cleanup_verified,
            "failure_kind": format!("{:?}", error.kind()),
        })));
    }
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
        bridge_extension: bridge_install.map(|install| install.extension),
        ready_timeout_ms,
        ttl_ms,
    };
    publish_spec(&spec_path(&paths), &spec).map_err(state_unavailable)?;

    // Resolved at the owner-spawn boundary, not at entry: by this point the
    // session directory exists and the owner spec has already been published,
    // and this branch does not roll them back. The refusal is therefore
    // honestly pre-owner-spawn, not a zero-side-effect failure.
    let program = crate::owner_executable::resolve_current().map_err(|error| {
        CuError::new(
            "browser_owner_spawn_failed",
            "the sibling agenterm-cu owner executable is unavailable",
        )
        .with_detail(json!({
            "reason": error.reason(),
            "io_kind": error.kind().map(|kind| format!("{kind:?}")),
            "resolution": "current_exe_sibling",
            "owner_started": false,
            "session_state_retained": true,
        }))
    })?;
    let mut command = owner_command(&program, &paths.directory);
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

/// Builds one resident browser-session owner command for an explicit program.
///
/// Production passes the resolved sibling; the explicit program keeps the
/// spawn boundary testable without letting a test choose production's program.
fn owner_command(program: &std::path::Path, directory: &std::path::Path) -> ProcessCommand {
    let mut command = ProcessCommand::new(program);
    command
        .arg(OWNER_ARG)
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn bridge_install_error(error: BrowserBridgeInstallError) -> CuError {
    let code = error.code;
    match error.receipt {
        Some(receipt) => {
            CuError::new(code, "browser bridge materialization failed").with_detail(json!({
                "effect": receipt.effect,
                "retry_safe": receipt.effect == BrowserSetupEffect::NotPerformed,
                "shared_publication": bridge_install_summary(&receipt),
            }))
        }
        None => CuError::new(code, "browser bridge materialization failed").with_detail(json!({
            "effect": "not-performed",
            "retry_safe": true,
        })),
    }
}

fn bridge_install_summary(install: &BrowserBridgeInstall) -> Value {
    json!({
        "effect": install.effect,
        "complete": install.complete,
        "bundle_materialized": install.bundle_materialized,
        "native_manifest_file_written": install.native_manifest_file_written,
        "registration_count": install.registrations.len(),
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn publish_owned_native_manifest(profile: &Path, source: &Path) -> std::io::Result<()> {
    let file = open_existing_path(source, ExistingEntryType::File)?;
    let metadata = file.metadata()?;
    if metadata.len() > OWNED_NATIVE_MANIFEST_MAX_BYTES as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser bridge native manifest exceeds its byte ceiling",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((OWNED_NATIVE_MANIFEST_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > OWNED_NATIVE_MANIFEST_MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser bridge native manifest exceeds its byte ceiling",
        ));
    }
    let directory = profile.join("NativeMessagingHosts");
    fs::create_dir_all(&directory)?;
    protect_private_directory(&directory)?;
    write_private_atomic(
        &directory.join(format!("{ACU_NATIVE_HOST_NAME}.json")),
        &bytes,
    )
}

#[cfg(windows)]
fn publish_owned_native_manifest(_profile: &Path, _source: &Path) -> std::io::Result<()> {
    // Chromium resolves per-user native hosts from HKCU on Windows; setup has
    // already published that registration before the owned profile is created.
    Ok(())
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
    expect_orphaned_uncertain: bool,
) -> Result<Value, CuError> {
    let expected_state =
        expected_remove_state(expect_stopped, expect_failed, expect_orphaned_uncertain)?;
    let root = sessions_root(false).map_err(state_unavailable)?;
    browser_session_remove_from_root(&root, name, expected_state)
}

fn browser_session_remove_from_root(
    root: &Path,
    name: &str,
    expected_state: BrowserSessionState,
) -> Result<Value, CuError> {
    browser_session_remove_from_root_with(root, name, expected_state, process_is_absent)
}

fn browser_session_remove_from_root_with(
    root: &Path,
    name: &str,
    expected_state: BrowserSessionState,
    is_absent: impl Fn(&ProcessIdentity) -> Result<bool, CuError>,
) -> Result<Value, CuError> {
    let _registry_lock = registry_lock(root)?;
    let paths = session_paths(root, name).map_err(|code| CuError::new(code, code))?;
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
    // `unknown` stays a NAMED refusal: `process_is_absent` returns a `Result`, and
    // its error is propagated here with `?` rather than folded into a bool. Only
    // after both identities have been classified do the two absences reach the
    // production predicate below.
    let browser_absent = match record.browser.as_ref() {
        Some(browser) => is_absent(browser)?,
        None => true,
    };
    let owner_absent = is_absent(&record.owner)?;
    if !removal_is_authorised(expected_state, record.state, owner_absent, browser_absent) {
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

/// The ONE authorization predicate for a verified removal, called by
/// `browser_session_remove_payload` itself -- not a test-only model.
///
/// It is a pure conjunction so its weakening is demonstrable: the state must match
/// the caller's literal expectation, and BOTH recorded identities must be exactly
/// absent. Dropping the browser term would admit an orphaned record whose browser
/// is still live, which is precisely what must keep its directory.
///
/// Absence itself is NOT decided here: callers pass the result of
/// `process_is_absent`, which keeps `unknown` a named refusal rather than a bool.
fn removal_is_authorised(
    expected: BrowserSessionState,
    actual: BrowserSessionState,
    owner_absent: bool,
    browser_absent: bool,
) -> bool {
    actual == expected && owner_absent && browser_absent
}

fn expected_remove_state(
    expect_stopped: bool,
    expect_failed: bool,
    expect_orphaned_uncertain: bool,
) -> Result<BrowserSessionState, CuError> {
    match (expect_stopped, expect_failed, expect_orphaned_uncertain) {
        (true, false, false) => Ok(BrowserSessionState::Stopped),
        (false, true, false) => Ok(BrowserSessionState::Failed),
        // An orphaned record reached no terminal state of its own: its owner
        // vanished. It is still removable, because the verification below is what
        // authorises the delete, not the state name alone -- both recorded
        // processes must be exactly and independently absent, so an orphaned
        // record whose browser is still live is refused exactly like any other.
        (false, false, true) => Ok(BrowserSessionState::OrphanedUncertain),
        _ => Err(CuError::new(
            "browser_session_remove_intent_required",
            "browser-session-remove requires exactly one of --expect stopped, --expect failed or --expect orphaned_uncertain",
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
    absence_from_classification(classify_process(identity))
}

fn absence_from_classification(classification: &str) -> Result<bool, CuError> {
    match classification {
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

    fn orphaned_removal_fixture(
        label: &str,
        browser: Option<ProcessIdentity>,
    ) -> (PathBuf, BrowserSessionPaths) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("target/browser-session-tests")
            .join(format!("orphaned-removal-{label}-{}", new_nonce("test")));
        let paths = session_paths(&root, "orphaned").unwrap();
        create_session_directories(&paths).unwrap();
        write_private_atomic(&paths.profile.join(OWNER_MARKER_FILE), MARKER_BYTES).unwrap();
        let profile = open_existing_path(&paths.profile, ExistingEntryType::Directory).unwrap();
        let profile_identity = FileObjectIdentity::from(file_identity(&profile).unwrap());
        drop(profile);
        publish_record(
            &paths.registry,
            &BrowserSessionRecord {
                schema_version: crate::browser_session::REGISTRY_SCHEMA_VERSION,
                generation: 1,
                name: "orphaned".into(),
                session_nonce: "0123456789abcdef".into(),
                state: BrowserSessionState::OrphanedUncertain,
                owner: ProcessIdentity {
                    pid: i32::MAX as u32,
                    start_identity: "absent-owner".into(),
                },
                owner_spawn_mode: "fixture".into(),
                profile_identity,
                browser,
                endpoint: None,
                last_error_code: Some("owner_lost".into()),
            },
        )
        .unwrap();
        (root, paths)
    }

    fn clear_removal_fixture(root: &Path) {
        if root.exists() {
            remove_tree(root).unwrap();
        }
        remove_file_if_present(&root.with_extension("registry.lock")).unwrap();
    }

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
            expected_remove_state(true, false, false).unwrap(),
            BrowserSessionState::Stopped
        );
        assert_eq!(
            expected_remove_state(false, true, false).unwrap(),
            BrowserSessionState::Failed
        );
        assert_eq!(
            expected_remove_state(false, false, true).unwrap(),
            BrowserSessionState::OrphanedUncertain
        );
        // Exactly one intent. The zero case matters most: an absent `--expect` must
        // never be read as "reclaim the orphan".
        for (stopped, failed, orphaned) in [
            (false, false, false),
            (true, true, false),
            (true, false, true),
            (false, true, true),
            (true, true, true),
        ] {
            assert_eq!(
                expected_remove_state(stopped, failed, orphaned)
                    .unwrap_err()
                    .code,
                "browser_session_remove_intent_required",
                "({stopped},{failed},{orphaned}) must not select an intent"
            );
        }
    }

    /// The red gate for the orphaned leaf, expressed against the PRODUCTION
    /// predicate: `authorised` below IS `removal_is_authorised`, the same function
    /// `browser_session_remove_payload` calls. There is no parallel test model, so
    /// a real weakening of the production conjunction is what this catches.
    ///
    /// The weak form is written out only to DEMONSTRATE the difference; no test
    /// asserts it is used anywhere.
    #[test]
    fn removal_is_authorised_only_when_both_identities_are_absent() {
        fn authorised(
            expected: BrowserSessionState,
            actual: BrowserSessionState,
            owner_absent: bool,
            browser_absent: bool,
        ) -> bool {
            removal_is_authorised(expected, actual, owner_absent, browser_absent)
        }
        // What a careless change would ship: state plus owner absence only,
        // silently dropping the browser term.
        fn authorised_owner_only(
            expected: BrowserSessionState,
            actual: BrowserSessionState,
            owner_absent: bool,
        ) -> bool {
            actual == expected && owner_absent
        }

        let orphan = BrowserSessionState::OrphanedUncertain;
        // The one shape that may be removed.
        assert!(authorised(orphan, orphan, true, true));
        // An orphaned record whose browser somehow survives is NOT removable: it is
        // the exact case that must keep its directory and report a named refusal.
        assert!(!authorised(orphan, orphan, true, false));
        // ...and this is the red gate: the production predicate refuses exactly the
        // record the weakened one admits. If the browser term is dropped from
        // `removal_is_authorised`, the assertion above goes red.
        assert!(
            authorised_owner_only(orphan, orphan, true),
            "the weak form is what admits a live browser"
        );
        assert_ne!(
            authorised(orphan, orphan, true, false),
            authorised_owner_only(orphan, orphan, true),
            "dropping the browser term must change the verdict for a live browser"
        );
        // A live owner is never removable, whatever the browser says.
        assert!(!authorised(orphan, orphan, false, true));
        assert!(!authorised(orphan, orphan, false, false));
        assert!(!authorised_owner_only(orphan, orphan, false));
        // The state must still match the caller's literal expectation.
        assert!(!authorised(
            BrowserSessionState::Stopped,
            orphan,
            true,
            true
        ));
        assert!(!authorised(
            orphan,
            BrowserSessionState::Stopped,
            true,
            true
        ));
        // Stopped and failed keep working through the same conjunction.
        let stopped = BrowserSessionState::Stopped;
        assert!(authorised(stopped, stopped, true, true));
        assert!(!authorised(stopped, stopped, true, false));
    }

    #[test]
    fn orphaned_removal_deletes_only_after_both_exact_identities_are_absent() {
        let dead = ProcessIdentity {
            pid: i32::MAX as u32,
            start_identity: "absent-browser".into(),
        };
        let (root, paths) = orphaned_removal_fixture("absent", Some(dead));

        let value = browser_session_remove_from_root(
            &root,
            "orphaned",
            BrowserSessionState::OrphanedUncertain,
        )
        .unwrap();

        assert_eq!(value["state"], "removed");
        assert_eq!(value["verified"], true);
        assert!(!paths.directory.exists());
        clear_removal_fixture(&root);
    }

    #[test]
    fn orphaned_removal_refuses_a_live_browser_and_preserves_evidence() {
        let browser = ProcessIdentity {
            pid: std::process::id(),
            start_identity: start_identity(std::process::id()).unwrap(),
        };
        let (root, paths) = orphaned_removal_fixture("live-browser", Some(browser));

        let error = browser_session_remove_from_root(
            &root,
            "orphaned",
            BrowserSessionState::OrphanedUncertain,
        )
        .unwrap_err();

        assert_eq!(error.code, "browser_session_remove_unverified");
        assert!(paths.directory.is_dir());
        assert!(paths.registry.is_file());
        assert_eq!(
            read_record(&paths.registry).unwrap().state,
            BrowserSessionState::OrphanedUncertain
        );
        clear_removal_fixture(&root);
    }

    #[test]
    fn orphaned_removal_refuses_unknown_liveness_and_preserves_evidence() {
        let (root, paths) = orphaned_removal_fixture("unknown", None);

        let error = browser_session_remove_from_root_with(
            &root,
            "orphaned",
            BrowserSessionState::OrphanedUncertain,
            |_| {
                Err(CuError::new(
                    "browser_session_liveness_unknown",
                    "exact process liveness is unavailable",
                ))
            },
        )
        .unwrap_err();

        assert_eq!(error.code, "browser_session_liveness_unknown");
        assert!(paths.directory.is_dir());
        assert!(paths.registry.is_file());
        assert_eq!(
            read_record(&paths.registry).unwrap().state,
            BrowserSessionState::OrphanedUncertain
        );
        clear_removal_fixture(&root);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn owned_profile_receives_the_exact_native_manifest() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("target/browser-session-tests")
            .join(format!("owned-native-manifest-{}", new_nonce("test")));
        let profile = root.join("profile");
        let source = root.join("native-host.json");
        fs::create_dir_all(&profile).unwrap();
        write_private_atomic(&source, b"{\"name\":\"fixture\"}\n").unwrap();

        publish_owned_native_manifest(&profile, &source).unwrap();

        let destination = profile
            .join("NativeMessagingHosts")
            .join(format!("{ACU_NATIVE_HOST_NAME}.json"));
        assert_eq!(fs::read(destination).unwrap(), b"{\"name\":\"fixture\"}\n");
        remove_tree(&root).unwrap();
    }

    #[test]
    fn bridge_materialization_failure_preserves_effect_without_paths() {
        let receipt = BrowserBridgeInstall {
            effect: BrowserSetupEffect::PerformedPartial,
            requested_browsers: Vec::new(),
            discovered_roots: Vec::new(),
            extension: PathBuf::from("/synthetic/private/extension"),
            native_manifest_file: PathBuf::from("/synthetic/private/native-host.json"),
            replaced_extension: false,
            bundle_materialized: true,
            native_manifest_file_written: false,
            registrations: Vec::new(),
            extension_loaded: false,
            manual_activation_required: true,
            complete: false,
            idempotent_rerun: true,
        };
        let mapped = bridge_install_error(BrowserBridgeInstallError {
            code: "browser_bridge_native_manifest_publish_failed",
            receipt: Some(Box::new(receipt)),
        });
        let detail = mapped.detail.unwrap();
        assert_eq!(detail["effect"], "performed-partial");
        assert_eq!(detail["retry_safe"], false);
        assert_eq!(detail["shared_publication"]["bundle_materialized"], true);
        assert!(!detail.to_string().contains("/synthetic/private"));
    }
}
