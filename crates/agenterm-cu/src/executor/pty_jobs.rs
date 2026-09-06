//! Durable headless PTY jobs backed by the existing AgenTerm server.
//!
//! One job name maps to one isolated logical instance. The server remains the
//! sole PTY/ConPTY, terminal tree, retention and process-lifecycle authority;
//! this module only supervises that product boundary.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    thread,
    time::{Duration, Instant},
};

use agenterm_control_client::{ControlClient, Intent};
use agenterm_platform::{
    filesystem::{host_directories, protect_private_directory},
    locking::{LockErrorKind, PathLock},
    process_spawn::spawn_detached_child,
};
use serde_json::{Value, json};

use crate::cdp::page::base64_decode;
use crate::{
    CuError, PtySignalKind, TerminalWaitCondition,
    pty_snapshot::{self, PtySnapshotStore},
    receipt::ReceiptLog,
};

use super::{
    parse_output, request, request_protocol, terminal_close_with_client,
    terminal_events_with_client, terminal_inventory_with_client, terminal_new_with_client,
    terminal_output_with_client, terminal_send_with_client, terminal_snapshot_with_client,
    terminal_wait_with_client,
};

const READY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_JOB_INVENTORY: usize = 4_096;

fn scan_exact_page(
    overlap: &mut Vec<u8>,
    page: &[u8],
    needle: &[u8],
    page_start_cursor: u64,
) -> Option<u64> {
    let overlap_len = overlap.len();
    overlap.extend_from_slice(page);
    if let Some(position) = overlap
        .windows(needle.len())
        .position(|part| part == needle)
    {
        return Some(
            page_start_cursor
                .saturating_sub(overlap_len as u64)
                .saturating_add(position as u64),
        );
    }
    let retain = needle.len().saturating_sub(1).min(overlap.len());
    if overlap.len() > retain {
        overlap.drain(..overlap.len() - retain);
    }
    None
}

fn validate_name(name: &str) -> Result<(), CuError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(CuError::new(
            "pty_job_name_invalid",
            "PTY job name must be 1..=64 ASCII letters, digits, '.', '_' or '-'",
        ))
    }
}

fn instance_name(name: &str) -> String {
    format!("ephemeral:acu-pty-{name}")
}

fn client_for(name: &str) -> Result<ControlClient, CuError> {
    validate_name(name)?;
    ControlClient::for_instance(&instance_name(name))
        .map_err(|error| CuError::new(error.code, error.message))
}

fn jobs_root(create: bool) -> Result<PathBuf, CuError> {
    let root = host_directories()
        .map_err(|error| CuError::new("pty_job_state_unavailable", error.to_string()))?
        .local_data
        .join("agenterm")
        .join("pty-jobs");
    if !create {
        return Ok(root);
    }
    fs::create_dir_all(&root).map_err(|error| {
        CuError::new(
            "pty_job_state_unavailable",
            format!("could not create PTY job state directory: {error}"),
        )
    })?;
    protect_private_directory(&root).map_err(|error| {
        CuError::new(
            "pty_job_state_unavailable",
            format!("could not protect PTY job state directory: {error}"),
        )
    })?;
    Ok(root)
}

fn job_directory(name: &str) -> Result<PathBuf, CuError> {
    let root = jobs_root(true)?;
    let directory = root.join(name);
    fs::create_dir_all(&directory).map_err(|error| {
        CuError::new(
            "pty_job_state_unavailable",
            format!("could not create named PTY job directory: {error}"),
        )
    })?;
    protect_private_directory(&directory).map_err(|error| {
        CuError::new(
            "pty_job_state_unavailable",
            format!("could not protect named PTY job directory: {error}"),
        )
    })?;
    Ok(directory)
}

pub(super) fn pty_list_payload() -> Result<Value, CuError> {
    let root = jobs_root(false)?;
    let root_metadata = match fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({
                "schema_version": 1,
                "jobs": [],
                "total": 0,
                "running": 0,
                "stale": 0,
                "conflicted": 0,
                "complete": true,
            }));
        }
        Err(error) => {
            return Err(CuError::new(
                "pty_job_state_unavailable",
                format!("could not inspect PTY job state directory: {error}"),
            ));
        }
    };
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(CuError::new(
            "pty_job_inventory_invalid",
            "PTY job state root is not a direct directory",
        ));
    }
    let entries = fs::read_dir(&root).map_err(|error| {
        CuError::new(
            "pty_job_state_unavailable",
            format!("could not read PTY job state directory: {error}"),
        )
    })?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            CuError::new(
                "pty_job_state_unavailable",
                format!("could not read a PTY job state entry: {error}"),
            )
        })?;
        if names.len() == MAX_JOB_INVENTORY {
            return Err(CuError::new(
                "pty_job_inventory_limit",
                "PTY job state contains more than 4096 named entries",
            ));
        }
        let kind = entry.file_type().map_err(|error| {
            CuError::new(
                "pty_job_state_unavailable",
                format!("could not classify a PTY job state entry: {error}"),
            )
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(CuError::new(
                "pty_job_inventory_invalid",
                "PTY job state contains a non-UTF-8 entry name",
            ));
        };
        if !kind.is_dir() || kind.is_symlink() || validate_name(&name).is_err() {
            return Err(CuError::new(
                "pty_job_inventory_invalid",
                "PTY job state contains an entry not owned by the named-job contract",
            ));
        }
        names.push(name);
    }
    names.sort_unstable();

    let mut jobs = Vec::with_capacity(names.len());
    let mut running = 0_usize;
    let mut stale = 0_usize;
    let mut conflicted = 0_usize;
    for name in names {
        let client = client_for(&name)?;
        match status_with_client(&client, &name) {
            Ok(status) => {
                running += 1;
                jobs.push(json!({ "name": name, "state": "running", "status": status }));
            }
            Err(error) if error.code == "pty_job_not_found" => {
                stale += 1;
                jobs.push(json!({ "name": name, "state": "stale" }));
            }
            Err(error) => {
                conflicted += 1;
                jobs.push(json!({
                    "name": name,
                    "state": "conflicted",
                    "error": { "code": error.code, "message": error.message },
                }));
            }
        }
    }
    Ok(json!({
        "schema_version": 1,
        "total": jobs.len(),
        "running": running,
        "stale": stale,
        "conflicted": conflicted,
        "complete": true,
        "jobs": jobs,
    }))
}

fn acquire_job_lock(directory: &Path) -> Result<PathLock, CuError> {
    PathLock::try_acquire(&directory.join("supervisor.lock")).map_err(|error| {
        let code = if error.kind() == LockErrorKind::Contended {
            "pty_job_busy"
        } else {
            "pty_job_state_unavailable"
        };
        CuError::new(code, error.to_string())
    })
}

fn acquire_registry_lock(root: &Path) -> Result<PathLock, CuError> {
    let path = root.with_extension("registry.lock");
    PathLock::try_acquire(&path).map_err(|error| {
        let code = if error.kind() == LockErrorKind::Contended {
            "pty_job_registry_busy"
        } else {
            "pty_job_state_unavailable"
        };
        CuError::new(code, error.to_string())
    })
}

fn remove_file_if_present(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn pty_prune_payload(
    name: &str,
    expect_stale: bool,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    validate_name(name)?;
    if !expect_stale {
        return Err(CuError::new(
            "pty_job_prune_intent_required",
            "pty-prune requires explicit --expect stale",
        ));
    }
    let root = jobs_root(false)?;
    let directory = root.join(name);
    let initial = fs::symlink_metadata(&directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CuError::new(
                "pty_job_state_not_found",
                format!("PTY job state for {name:?} does not exist"),
            )
        } else {
            CuError::new(
                "pty_job_state_unavailable",
                format!("could not inspect named PTY job state: {error}"),
            )
        }
    })?;
    if !initial.is_dir() || initial.file_type().is_symlink() {
        return Err(CuError::new(
            "pty_job_inventory_invalid",
            "named PTY job state is not a direct directory",
        ));
    }
    let initial_identity =
        agenterm_platform::file_identity::path_identity(&directory).map_err(|error| {
            CuError::new(
                "pty_job_state_unavailable",
                format!("could not identify named PTY job state: {error}"),
            )
        })?;

    let _registry_lock = acquire_registry_lock(&root)?;
    let current = fs::symlink_metadata(&directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CuError::new(
                "pty_job_state_changed",
                "named PTY job state disappeared while acquiring the registry lock",
            )
        } else {
            CuError::new(
                "pty_job_state_unavailable",
                format!("could not re-inspect named PTY job state: {error}"),
            )
        }
    })?;
    if !current.is_dir() || current.file_type().is_symlink() {
        return Err(CuError::new(
            "pty_job_state_changed",
            "named PTY job state changed identity while acquiring the registry lock",
        ));
    }
    let current_identity =
        agenterm_platform::file_identity::path_identity(&directory).map_err(|error| {
            CuError::new(
                "pty_job_state_changed",
                format!("could not re-identify named PTY job state: {error}"),
            )
        })?;
    if !initial_identity.same_object(current_identity) {
        return Err(CuError::new(
            "pty_job_state_changed",
            "named PTY job state was replaced while acquiring the registry lock",
        ));
    }
    let job_lock = acquire_job_lock(&directory)?;
    let client = client_for(name)?;
    match status_with_client(&client, name) {
        Ok(status) => {
            return Err(CuError::new(
                "pty_job_prune_live",
                "refusing to prune a PTY job whose authority is still reachable",
            )
            .with_detail(json!({ "name": name, "status": status })));
        }
        Err(error) if error.code == "pty_job_not_found" => {}
        Err(error) => {
            return Err(CuError::new(
                "pty_job_prune_unverified",
                "could not prove the named PTY job authority is stale",
            )
            .with_detail(json!({
                "name": name,
                "observation": { "code": error.code, "message": error.message },
            })));
        }
    }

    let mut removable = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|error| {
        CuError::new(
            "pty_job_state_unavailable",
            format!("could not inspect named PTY job contents: {error}"),
        )
    })? {
        let entry = entry.map_err(|error| {
            CuError::new(
                "pty_job_state_unavailable",
                format!("could not read a named PTY job entry: {error}"),
            )
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(CuError::new(
                "pty_job_prune_state_unknown",
                "named PTY job state contains an unknown non-UTF-8 entry",
            ));
        };
        if name == "supervisor.lock" {
            continue;
        }
        if !matches!(name, "workspace.json" | "settings.json") {
            return Err(CuError::new(
                "pty_job_prune_state_unknown",
                "named PTY job state contains an entry outside the reclaim contract",
            ));
        }
        let kind = entry.file_type().map_err(|error| {
            CuError::new(
                "pty_job_state_unavailable",
                format!("could not classify a named PTY job entry: {error}"),
            )
        })?;
        if kind.is_dir() {
            return Err(CuError::new(
                "pty_job_prune_state_unknown",
                "named PTY job state contains a directory outside the reclaim contract",
            ));
        }
        removable.push(entry.path());
    }

    let ticket = receipts.reserve(
        "pty-prune",
        0,
        json!({
            "name_bytes": name.len(),
            "expect": "stale",
            "before": { "authority": "unreachable", "state_directory": "present" },
            "known_entries": removable.len(),
        }),
    )?;
    let removal = (|| -> Result<(), CuError> {
        for path in &removable {
            remove_file_if_present(path).map_err(|error| {
                CuError::new(
                    "pty_job_prune_failed",
                    format!("could not remove a known PTY job state file: {error}"),
                )
            })?;
        }
        drop(job_lock);
        remove_file_if_present(&directory.join("supervisor.lock")).map_err(|error| {
            CuError::new(
                "pty_job_prune_failed",
                format!("could not remove the PTY job lock file: {error}"),
            )
        })?;
        fs::remove_dir(&directory).map_err(|error| {
            CuError::new(
                "pty_job_prune_failed",
                format!("could not remove the empty PTY job state directory: {error}"),
            )
        })?;
        match fs::symlink_metadata(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(CuError::new(
                "pty_job_prune_unverified",
                "PTY job state directory remained after removal",
            )),
            Err(error) => Err(CuError::new(
                "pty_job_prune_unverified",
                format!("could not verify PTY job state removal: {error}"),
            )),
        }
    })();
    if let Err(error) = removal {
        receipts.complete(
            &ticket,
            "pty-prune",
            0,
            false,
            json!({ "performed": true, "verified": false, "error": { "code": error.code, "message": error.message } }),
        )?;
        return Err(error.with_detail(json!({ "receipt": ticket.json() })));
    }
    receipts.complete(
        &ticket,
        "pty-prune",
        0,
        true,
        json!({
            "performed": true,
            "verified": true,
            "after": { "state_directory": "absent" },
        }),
    )?;
    Ok(json!({
        "name": name,
        "state": "pruned",
        "performed": true,
        "verified": true,
        "removed_known_entries": removable.len(),
        "receipt": ticket.json(),
    }))
}

fn product_executable() -> Result<PathBuf, CuError> {
    if let Some(path) = std::env::var_os("AGENTERM_CU_PRODUCT_EXECUTABLE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(CuError::new(
            "pty_server_executable_missing",
            "AGENTERM_CU_PRODUCT_EXECUTABLE does not name an ordinary file",
        ));
    }
    let current = std::env::current_exe().map_err(|error| {
        CuError::new(
            "pty_server_executable_missing",
            format!("current executable path is unavailable: {error}"),
        )
    })?;
    let filename = if cfg!(windows) {
        "agenterm.exe"
    } else {
        "agenterm"
    };
    let mut candidates = Vec::new();
    if let Some(parent) = current.parent() {
        candidates.push(parent.join(filename));
        if let Some(grandparent) = parent.parent() {
            candidates.push(grandparent.join(filename));
        }
    }
    candidates.into_iter().find(|path| path.is_file()).ok_or_else(|| {
        CuError::new(
            "pty_server_executable_missing",
            "agenterm executable was not found beside agenterm-cu; set AGENTERM_CU_PRODUCT_EXECUTABLE",
        )
    })
}

fn spawn_server(name: &str, cwd: Option<&str>, directory: &Path) -> Result<&'static str, CuError> {
    let executable = product_executable()?;
    let mut command = ProcessCommand::new(executable);
    command
        .args(["server", "--instance", &instance_name(name), "--empty"])
        .env("AGENTERM_NO_ACTIVATE", "1")
        .env("AGENTERM_WORKSPACE_PATH", directory.join("workspace.json"))
        .env("AGENTERM_SETTINGS_PATH", directory.join("settings.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        let path = Path::new(cwd);
        if !path.is_dir() {
            return Err(CuError::new(
                "pty_job_cwd_invalid",
                "pty-start --cwd must name an existing directory",
            ));
        }
        command.current_dir(path);
    }
    let (mut child, mode) = spawn_detached_child(&mut command).map_err(|error| {
        CuError::new(
            "pty_server_start_failed",
            format!("could not start the AgenTerm headless server: {error}"),
        )
    })?;
    let mode = mode.as_str();
    let client = client_for(name)?;
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut consecutive_empty_reads = 0_u8;
    loop {
        match terminal_inventory_with_client(&client) {
            Ok(inventory) if inventory["tabs"].as_array().is_some_and(Vec::is_empty) => {
                // A Windows named-pipe listener can accept one probe while the
                // server is still rotating from startup into its steady accept
                // loop. Requiring two independent query connections prevents
                // the first mutation from inheriting ERROR_BROKEN_PIPE (233).
                consecutive_empty_reads = consecutive_empty_reads.saturating_add(1);
                if consecutive_empty_reads >= 2 {
                    return Ok(mode);
                }
            }
            Ok(_) => {
                return Err(CuError::new(
                    "pty_job_state_conflict",
                    "new headless PTY authority did not start with an empty tab set",
                ));
            }
            Err(_) if Instant::now() < deadline => {
                consecutive_empty_reads = 0;
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(CuError::new(
                        "pty_server_start_failed",
                        format!("AgenTerm headless server exited before readiness: {status}"),
                    ));
                }
            }
            Err(error) => {
                return Err(CuError::new(
                    "pty_server_ready_timeout",
                    format!("headless server did not become ready: {}", error.message),
                ));
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn sole_job(client: &ControlClient, name: &str) -> Result<(Value, Value), CuError> {
    let inventory = terminal_inventory_with_client(client).map_err(|error| {
        if error.code == "control_unavailable" {
            CuError::new(
                "pty_job_not_found",
                format!("PTY job {name:?} is not running"),
            )
        } else {
            error
        }
    })?;
    let tabs = inventory["tabs"]
        .as_array()
        .ok_or_else(|| CuError::new("pty_job_state_invalid", "PTY inventory omitted tabs"))?;
    if tabs.len() != 1 {
        return Err(CuError::new(
            "pty_job_state_invalid",
            format!(
                "PTY job authority must contain exactly one tab, found {}",
                tabs.len()
            ),
        ));
    }
    let tab = tabs[0].clone();
    Ok((inventory, tab))
}

fn status_with_client(client: &ControlClient, name: &str) -> Result<Value, CuError> {
    let (inventory, tab) = sole_job(client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    let response = request(
        client,
        vec!["inspect".to_owned(), "-t".to_owned(), tab_id.to_owned()],
        "command.inspect",
        Intent::Query,
        Duration::from_secs(5),
    )?;
    let inspect = parse_output(response, "pty_job_status_invalid")?;
    let process = inspect["windows"]
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .ok_or_else(|| {
            CuError::new(
                "pty_job_status_invalid",
                "PTY inspect omitted its process row",
            )
        })?;
    Ok(json!({
        "schema_version": 1,
        "name": name,
        "instance": instance_name(name),
        "server_scope_id": inventory["server_scope_id"],
        "server_epoch": inventory["server_epoch"],
        "tab_id": tab_id,
        "identity": "job-name+server-scope+epoch+tab-id",
        "dead": process["dead"],
        "finalized": process["finalized"],
        "exit_code": process["exit_code"],
        "process_id": tab["process_id"],
        "rows": tab["rows"],
        "columns": tab["columns"],
    }))
}

fn shutdown_and_verify(client: &ControlClient) -> (bool, bool, Option<Value>) {
    let shutdown = request(
        client,
        vec!["shutdown".to_owned()],
        "workspace.shutdown",
        Intent::Mutation,
        Duration::from_secs(5),
    );
    let shutdown_acknowledged = shutdown.is_ok();
    let shutdown_transport_error = shutdown
        .err()
        .map(|error| json!({ "code": error.code, "message": error.message }));
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut last_observation_error = None;
    while Instant::now() < deadline {
        match terminal_inventory_with_client(client) {
            Err(error) if error.code == "control_unavailable" => {
                return (true, shutdown_acknowledged, shutdown_transport_error);
            }
            Err(error) => {
                last_observation_error = Some(json!({
                    "code": error.code,
                    "message": error.message,
                }));
            }
            Ok(_) => {}
        }
        thread::sleep(Duration::from_millis(25));
    }
    (
        false,
        shutdown_acknowledged,
        shutdown_transport_error.or(last_observation_error),
    )
}

pub(super) fn pty_start_payload(
    name: &str,
    cwd: Option<&str>,
    command: &[String],
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    validate_name(name)?;
    if command.is_empty()
        || command.len() > 256
        || command.iter().map(String::len).sum::<usize>() > 1_048_576
    {
        return Err(CuError::new(
            "pty_job_command_invalid",
            "pty-start requires 1..=256 arguments totaling at most 1048576 bytes",
        ));
    }
    let root = jobs_root(true)?;
    let _registry_lock = acquire_registry_lock(&root)?;
    let directory = job_directory(name)?;
    let _lock = acquire_job_lock(&directory)?;
    let client = client_for(name)?;
    let spawned = match terminal_inventory_with_client(&client) {
        Ok(inventory) if inventory["tabs"].as_array().is_some_and(Vec::is_empty) => false,
        Ok(_) => {
            return Err(CuError::new(
                "pty_job_exists",
                format!("PTY job {name:?} already exists"),
            ));
        }
        Err(error) if error.code == "control_unavailable" => {
            let _ = spawn_server(name, cwd, &directory)?;
            true
        }
        Err(error) => return Err(error),
    };
    let created = match terminal_new_with_client(&client, Some(name), None, true, command, receipts)
    {
        Ok(created) => created,
        Err(error) if spawned => {
            let (cleanup_verified, cleanup_acknowledged, cleanup_error) =
                shutdown_and_verify(&client);
            return Err(CuError::new(error.code, error.message).with_detail(json!({
                "operation_detail": error.detail,
                "cleanup_verified": cleanup_verified,
                "cleanup_acknowledged": cleanup_acknowledged,
                "cleanup_error": cleanup_error,
            })));
        }
        Err(error) => return Err(error),
    };
    let status = status_with_client(&client, name)?;
    Ok(json!({
        "name": name,
        "background": true,
        "performed": true,
        "verified": true,
        "command_arguments": command.len(),
        "command_bytes": command.iter().map(String::len).sum::<usize>(),
        "created": created,
        "status": status,
    }))
}

pub(super) fn pty_status_payload(name: &str) -> Result<Value, CuError> {
    status_with_client(&client_for(name)?, name)
}

pub(super) fn pty_read_payload(
    name: &str,
    cursor: &str,
    max_bytes: usize,
) -> Result<Value, CuError> {
    let client = client_for(name)?;
    let (inventory, tab) = sole_job(&client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    let mut output = terminal_output_with_client(&client, tab_id, cursor, max_bytes)?;
    output["name"] = json!(name);
    output["server_epoch"] = inventory["server_epoch"].clone();
    output["identity"] = json!("job-name+server-scope+epoch+tab-id+raw-output-cursor");
    Ok(output)
}

fn capture_pty_snapshot(name: &str) -> Result<Value, CuError> {
    let client = client_for(name)?;
    let (inventory, tab) = sole_job(&client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    let mut snapshot = terminal_snapshot_with_client(&client, tab_id)?;
    if snapshot["server_epoch"] != inventory["server_epoch"] {
        return Err(CuError::new(
            "pty_job_epoch_changed",
            "PTY job authority epoch changed while taking the structured snapshot",
        )
        .with_detail(json!({
            "name": name,
            "inventory_epoch": inventory["server_epoch"],
            "snapshot_epoch": snapshot["server_epoch"],
        })));
    }
    snapshot["name"] = json!(name);
    snapshot["identity"] = json!("job-name+server-scope+epoch+tab-id+event-cursor");
    Ok(snapshot)
}

pub(super) fn pty_snapshot_payload(name: &str, store: &PtySnapshotStore) -> Result<Value, CuError> {
    let mut snapshot = capture_pty_snapshot(name)?;
    let baseline = store.write(name, &snapshot)?;
    snapshot["baseline"] = baseline.meta_json();
    snapshot["snapshot_id"] = json!(baseline.snapshot_id);
    snapshot["next_actions"] = json!([format!(
        "pty-diff {name} --base {} --advance",
        snapshot["snapshot_id"].as_str().expect("stored id")
    )]);
    Ok(snapshot)
}

pub(super) fn pty_diff_payload(
    name: &str,
    base: &str,
    advance: bool,
    max: Option<usize>,
    store: &PtySnapshotStore,
) -> Result<Value, CuError> {
    let max = pty_snapshot::validate_diff_max(max)?;
    let baseline = store.load(name, base)?;
    let current = capture_pty_snapshot(name)?;
    let current_scope = current["server_scope_id"].as_str();
    let current_epoch = current["server_epoch"].as_str();
    let current_tab = current["tab"]["id"].as_str();
    if current_scope != Some(baseline.server_scope_id.as_str())
        || current_epoch != Some(baseline.server_epoch.as_str())
        || current_tab != Some(baseline.tab_id.as_str())
    {
        return Err(CuError::new(
            "pty_snapshot_authority_changed",
            "PTY screen baseline belongs to a different job authority",
        )
        .with_detail(json!({
            "name": name,
            "snapshot_id": base,
            "baseline": {
                "server_scope_id": baseline.server_scope_id,
                "server_epoch": baseline.server_epoch,
                "tab_id": baseline.tab_id,
            },
            "current": {
                "server_scope_id": current["server_scope_id"],
                "server_epoch": current["server_epoch"],
                "tab_id": current["tab"]["id"],
            },
        })));
    }
    let mut diff = pty_snapshot::diff_screens(&baseline.screen, &current["tab"]["screen"], max)?;
    let advanced = if advance {
        Some(store.write(name, &current)?)
    } else {
        None
    };
    diff["name"] = json!(name);
    diff["base"] = baseline.meta_json();
    diff["current"] = json!({
        "server_scope_id": current["server_scope_id"],
        "server_epoch": current["server_epoch"],
        "tab_id": current["tab"]["id"],
        "cursor_sequence": current["cursor"]["sequence"],
        "rows": current["tab"]["screen"]["rows"],
        "columns": current["tab"]["screen"]["columns"],
    });
    diff["advanced"] = advanced.as_ref().map_or(Value::Null, |row| row.meta_json());
    diff["next_base"] = json!(
        advanced
            .as_ref()
            .map(|row| row.snapshot_id.as_str())
            .unwrap_or(base)
    );
    diff["max"] = json!(max);
    diff["identity"] = json!("job-name+server-scope+epoch+tab-id+screen-baseline");
    diff["store"] = json!(store.root());
    Ok(diff)
}

pub(super) fn pty_events_payload(
    name: &str,
    epoch: &str,
    after: u64,
    limit: usize,
) -> Result<Value, CuError> {
    let client = client_for(name)?;
    let (inventory, tab) = sole_job(&client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    if inventory["server_epoch"].as_str() != Some(epoch) {
        return Err(CuError::new(
            "pty_job_epoch_changed",
            "PTY job authority epoch changed before event continuation",
        )
        .with_detail(json!({
            "name": name,
            "requested_epoch": epoch,
            "current_epoch": inventory["server_epoch"],
        })));
    }
    let mut events = terminal_events_with_client(&client, tab_id, epoch, after, limit)?;
    events["name"] = json!(name);
    events["identity"] = json!("job-name+server-scope+epoch+tab-id+event-cursor");
    Ok(events)
}

pub(super) fn pty_resize_payload(
    name: &str,
    rows: u16,
    columns: u16,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    if rows == 0 || rows > 512 || columns == 0 || columns > 512 {
        return Err(CuError::new(
            "pty_job_resize_invalid",
            "PTY rows and columns must be in 1..=512",
        ));
    }
    let directory = job_directory(name)?;
    let _lock = acquire_job_lock(&directory)?;
    let client = client_for(name)?;
    let (inventory, tab) = sole_job(&client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    let epoch = inventory["server_epoch"].as_str().ok_or_else(|| {
        CuError::new(
            "pty_job_state_invalid",
            "PTY job inventory omitted server epoch",
        )
    })?;
    let client_pid = std::process::id();
    let client_id = format!("acu-pty-resize-{client_pid}");
    let ticket = receipts.reserve(
        "pty-resize",
        0,
        json!({
            "name_bytes": name.len(),
            "tab_id": tab_id,
            "requested": { "rows": rows, "columns": columns },
        }),
    )?;

    let mut resize_attempted = false;
    let mut lease_attached = false;
    let operation = (|| -> Result<(Value, Value), CuError> {
        let hello = parse_output(
            request_protocol(
                &client,
                vec![
                    "ui-hello".to_owned(),
                    "--minimum".to_owned(),
                    "1".to_owned(),
                    "--maximum".to_owned(),
                    "1".to_owned(),
                    "--client-id".to_owned(),
                    client_id.clone(),
                ],
                Duration::from_secs(5),
            )?,
            "pty_job_resize_hello_invalid",
        )?;
        if hello["accepted"].as_bool() != Some(true)
            || hello["position"]["server_epoch"].as_str() != Some(epoch)
        {
            return Err(CuError::new(
                "pty_job_epoch_changed",
                "PTY job authority changed or rejected the resize protocol",
            ));
        }
        let lease = parse_output(
            request_protocol(
                &client,
                vec![
                    "ui-lease".to_owned(),
                    "attach".to_owned(),
                    "--client-id".to_owned(),
                    client_id,
                    "--client-pid".to_owned(),
                    client_pid.to_string(),
                ],
                Duration::from_secs(5),
            )?,
            "pty_job_resize_lease_invalid",
        )?;
        let lease_id = lease["lease_id"].as_str().ok_or_else(|| {
            CuError::new("pty_job_resize_lease_invalid", "UI lease omitted its id")
        })?;
        lease_attached = true;
        if lease["client_pid"].as_u64() != Some(u64::from(client_pid))
            || lease["position"]["server_epoch"].as_str() != Some(epoch)
        {
            let _ = request_protocol(
                &client,
                vec![
                    "ui-lease".to_owned(),
                    "detach".to_owned(),
                    "--lease-id".to_owned(),
                    lease_id.to_owned(),
                    "--client-pid".to_owned(),
                    client_pid.to_string(),
                ],
                Duration::from_secs(5),
            );
            return Err(CuError::new(
                "pty_job_epoch_changed",
                "PTY job authority changed while acquiring the resize lease",
            ));
        }

        resize_attempted = true;
        let resize = request_protocol(
            &client,
            vec![
                "ui-interact".to_owned(),
                "resize".to_owned(),
                "--lease-id".to_owned(),
                lease_id.to_owned(),
                "--client-pid".to_owned(),
                client_pid.to_string(),
                "-t".to_owned(),
                tab_id.to_owned(),
                "--rows".to_owned(),
                rows.to_string(),
                "--columns".to_owned(),
                columns.to_string(),
            ],
            Duration::from_secs(5),
        )
        .and_then(|response| parse_output(response, "pty_job_resize_result_invalid"));
        let detach = request_protocol(
            &client,
            vec![
                "ui-lease".to_owned(),
                "detach".to_owned(),
                "--lease-id".to_owned(),
                lease_id.to_owned(),
                "--client-pid".to_owned(),
                client_pid.to_string(),
            ],
            Duration::from_secs(5),
        )
        .and_then(|response| parse_output(response, "pty_job_resize_detach_invalid"));
        match (resize, detach) {
            (Ok(resize), Ok(detach)) => Ok((resize, detach)),
            (resize, Err(cleanup_error)) => Err(CuError::new(
                "pty_job_resize_cleanup_failed",
                "the temporary UI lease could not be detached after a resize attempt",
            )
            .with_detail(json!({
                "performed": true,
                "resize_error": resize.err().map(|error| json!({
                    "code": error.code,
                    "message": error.message,
                })),
                "cleanup_error": {
                    "code": cleanup_error.code,
                    "message": cleanup_error.message,
                },
            }))),
            (Err(resize_error), Ok(_)) => Err(resize_error),
        }
    })();

    let (resize, detach) = match operation {
        Ok(values) => values,
        Err(error) => {
            receipts.complete(
                &ticket,
                "pty-resize",
                0,
                false,
                json!({
                    "performed": resize_attempted,
                    "verified": false,
                    "lease_attached": lease_attached,
                    "error": { "code": error.code, "message": error.message },
                }),
            )?;
            return Err(error.with_detail(json!({ "receipt": ticket.json() })));
        }
    };
    let status = status_with_client(&client, name)?;
    let verified = resize["action"].as_str() == Some("resize")
        && resize["tab_id"].as_str() == Some(tab_id)
        && resize["rows"].as_u64() == Some(u64::from(rows))
        && resize["columns"].as_u64() == Some(u64::from(columns))
        && resize["position"]["server_epoch"].as_str() == Some(epoch)
        && detach["detached"].as_bool() == Some(true)
        && detach["client_pid"].as_u64() == Some(u64::from(client_pid))
        && detach["position"]["server_epoch"].as_str() == Some(epoch)
        && status["server_epoch"].as_str() == Some(epoch)
        && status["tab_id"].as_str() == Some(tab_id)
        && status["rows"].as_u64() == Some(u64::from(rows))
        && status["columns"].as_u64() == Some(u64::from(columns));
    receipts.complete(
        &ticket,
        "pty-resize",
        0,
        verified,
        json!({
            "performed": true,
            "verified": verified,
            "after": { "rows": status["rows"], "columns": status["columns"] },
            "lease_detached": detach["detached"],
        }),
    )?;
    let payload = json!({
        "name": name,
        "server_epoch": epoch,
        "tab_id": tab_id,
        "rows": status["rows"],
        "columns": status["columns"],
        "performed": true,
        "verified": verified,
        "lease_detached": detach["detached"],
        "identity": "job-name+server-scope+epoch+tab-id",
        "receipt": ticket.json(),
    });
    if verified {
        Ok(payload)
    } else {
        Err(CuError::new(
            "pty_job_resize_unverified",
            "PTY resize did not produce the exact requested grid and identity",
        )
        .with_detail(payload))
    }
}

pub(super) fn pty_send_payload(
    name: &str,
    text: &str,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    let directory = job_directory(name)?;
    let _lock = acquire_job_lock(&directory)?;
    let client = client_for(name)?;
    let (inventory, tab) = sole_job(&client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    let sent = terminal_send_with_client(&client, tab_id, text, receipts)?;
    Ok(json!({
        "name": name,
        "server_epoch": inventory["server_epoch"],
        "tab_id": tab_id,
        "text_bytes": text.len(),
        "performed": sent["performed"],
        "verified": sent["verified"],
        "identity": "job-name+server-scope+epoch+tab-id",
        "send": sent,
    }))
}

pub(super) fn pty_wait_payload(
    name: &str,
    contains: &str,
    cursor: &str,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    validate_name(name)?;
    if contains.is_empty() || contains.len() > 65_536 {
        return Err(CuError::new(
            "pty_job_wait_condition_invalid",
            "pty-wait --contains must be 1..=65536 bytes",
        ));
    }
    if cursor != "earliest" && cursor != "current" && cursor.parse::<u64>().is_err() {
        return Err(CuError::new(
            "pty_job_wait_cursor_invalid",
            "pty-wait --cursor must be earliest, current, or a non-negative integer",
        ));
    }
    if !(1..=86_400_000).contains(&timeout_ms) {
        return Err(CuError::new(
            "pty_job_wait_limit_invalid",
            "pty-wait --timeout-ms must be in 1..=86400000",
        ));
    }
    let client = client_for(name)?;
    let (inventory, tab) = sole_job(&client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    let needle = contains.as_bytes();
    let started = Instant::now();
    let deadline = started + Duration::from_millis(timeout_ms);
    let mut next = cursor.to_owned();
    let mut overlap = Vec::new();
    let mut scanned_bytes = 0_u64;
    loop {
        let page = terminal_output_with_client(&client, tab_id, &next, 1_048_576)?;
        let start_cursor = page["start_cursor"].as_u64().ok_or_else(|| {
            CuError::new(
                "pty_job_wait_output_invalid",
                "PTY output omitted start_cursor",
            )
        })?;
        let next_cursor = page["next_cursor"].as_u64().ok_or_else(|| {
            CuError::new(
                "pty_job_wait_output_invalid",
                "PTY output omitted next_cursor",
            )
        })?;
        let current_cursor = page["current_cursor"].as_u64().ok_or_else(|| {
            CuError::new(
                "pty_job_wait_output_invalid",
                "PTY output omitted current_cursor",
            )
        })?;
        let encoded = page["data_base64"].as_str().ok_or_else(|| {
            CuError::new(
                "pty_job_wait_output_invalid",
                "PTY output omitted data_base64",
            )
        })?;
        let bytes = base64_decode(encoded).map_err(|reason| {
            CuError::new(
                "pty_job_wait_output_invalid",
                format!("PTY output base64 was invalid: {reason}"),
            )
        })?;
        scanned_bytes = scanned_bytes.saturating_add(bytes.len() as u64);
        if let Some(matched_at_cursor) = scan_exact_page(&mut overlap, &bytes, needle, start_cursor)
        {
            return Ok(json!({
                "name": name,
                "server_epoch": inventory["server_epoch"],
                "tab_id": tab_id,
                "condition": { "kind": "contains", "bytes": needle.len() },
                "state": "matched",
                "completed": true,
                "matched_at_cursor": matched_at_cursor,
                "next_cursor": next_cursor,
                "scanned_bytes": scanned_bytes,
                "elapsed_ms": started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                "identity": "job-name+server-scope+epoch+tab-id+raw-output-cursor",
            }));
        }
        next = next_cursor.to_string();
        if next_cursor < current_cursor {
            continue;
        }
        if Instant::now() >= deadline {
            return Err(CuError::new(
                "pty_job_wait_timeout",
                "PTY output did not contain the requested bytes before the deadline",
            )
            .with_detail(json!({
                "name": name,
                "tab_id": tab_id,
                "timeout_ms": timeout_ms,
                "next_cursor": next_cursor,
                "scanned_bytes": scanned_bytes,
            })));
        }
        let status = status_with_client(&client, name)?;
        if status["finalized"].as_bool() == Some(true) {
            return Err(CuError::new(
                "pty_job_wait_unmatched_after_exit",
                "PTY job finalized before its output contained the requested bytes",
            )
            .with_detail(json!({
                "name": name,
                "tab_id": tab_id,
                "exit_code": status["exit_code"],
                "next_cursor": next_cursor,
                "scanned_bytes": scanned_bytes,
            })));
        }
        thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50)),
        );
    }
}

pub(super) fn pty_wait_exit_payload(
    name: &str,
    timeout_ms: u64,
    expect_status: Option<i32>,
) -> Result<Value, CuError> {
    let client = client_for(name)?;
    let (_, tab) = sole_job(&client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    let wait = terminal_wait_with_client(
        &client,
        tab_id,
        &TerminalWaitCondition::Finalized,
        timeout_ms,
    )?;
    let status = status_with_client(&client, name)?;
    let actual = status["exit_code"].as_i64();
    let verified = expect_status.is_none_or(|expected| actual == Some(i64::from(expected)));
    let payload = json!({
        "name": name,
        "completed": true,
        "verified": verified,
        "expected_exit_status": expect_status,
        "exit_status": actual,
        "wait": wait,
        "status": status,
    });
    if verified {
        Ok(payload)
    } else {
        Err(CuError::new(
            "pty_job_exit_status_mismatch",
            "PTY job finalized with a different exit status",
        )
        .with_detail(payload))
    }
}

pub(super) fn pty_signal_payload(
    name: &str,
    signal: PtySignalKind,
    expect: &str,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    if expect != signal.expected_postcondition() {
        return Err(CuError::new(
            "pty_signal_intent_required",
            format!(
                "pty-signal --signal {} requires --expect {}",
                signal.as_str(),
                signal.expected_postcondition()
            ),
        ));
    }
    let directory = job_directory(name)?;
    let _lock = acquire_job_lock(&directory)?;
    let client = client_for(name)?;
    let (inventory, tab) = sole_job(&client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    let ticket = receipts.reserve(
        "pty-signal",
        0,
        json!({
            "name_bytes": name.len(),
            "server_scope_id": inventory["server_scope_id"],
            "server_epoch": inventory["server_epoch"],
            "tab_id": tab_id,
            "signal": signal.as_str(),
            "expected_postcondition": expect,
        }),
    )?;
    let response = match request(
        &client,
        vec![
            "signal-terminal-foreground".to_owned(),
            "-t".to_owned(),
            tab_id.to_owned(),
            "--signal".to_owned(),
            signal.as_str().to_owned(),
            "--expect".to_owned(),
            expect.to_owned(),
        ],
        "command.signal.terminal.foreground",
        Intent::Mutation,
        Duration::from_secs(5),
    ) {
        Ok(response) => response,
        Err(error) => {
            receipts.complete(
                &ticket,
                "pty-signal",
                0,
                false,
                json!({ "performed": false, "error_code": error.code }),
            )?;
            return Err(attach_receipt(error, ticket.json()));
        }
    };
    let native = match parse_output(response, "pty_signal_receipt_invalid") {
        Ok(native) => native,
        Err(error) => {
            receipts.complete(
                &ticket,
                "pty-signal",
                0,
                false,
                json!({
                    "performed": null,
                    "outcome_unknown": true,
                    "error_code": error.code,
                }),
            )?;
            return Err(attach_receipt(error, ticket.json()));
        }
    };
    let delivered = native["delivered"].as_bool() == Some(true);
    let verified = native["verified"].as_bool() == Some(true);
    let postcondition = native["postcondition"].as_str();
    let expectation_met = match signal {
        PtySignalKind::Interrupt => delivered,
        PtySignalKind::Terminate | PtySignalKind::Stop | PtySignalKind::Continue => {
            delivered && verified && postcondition == Some(expect)
        }
    };
    let payload = json!({
        "name": name,
        "server_scope_id": inventory["server_scope_id"],
        "server_epoch": inventory["server_epoch"],
        "tab_id": tab_id,
        "identity": "job-name+server-scope+epoch+tab-id+retained-pty-foreground-group",
        "signal": signal.as_str(),
        "expected_postcondition": expect,
        "performed": delivered,
        "verified": expectation_met,
        "native": native,
        "receipt": ticket.json(),
    });
    receipts.complete(
        &ticket,
        "pty-signal",
        0,
        expectation_met,
        json!({
            "performed": delivered,
            "verified": expectation_met,
            "postcondition": postcondition,
        }),
    )?;
    if expectation_met {
        Ok(payload)
    } else {
        Err(CuError::new(
            "pty_signal_postcondition_unverified",
            "the native PTY signal did not prove its declared postcondition",
        )
        .with_detail(payload))
    }
}

fn attach_receipt(mut error: CuError, receipt: Value) -> CuError {
    let detail = match error.detail.take() {
        Some(Value::Object(mut detail)) => {
            detail.insert("receipt".to_owned(), receipt);
            Value::Object(detail)
        }
        Some(cause) => json!({ "cause": cause, "receipt": receipt }),
        None => json!({ "receipt": receipt }),
    };
    error.with_detail(detail)
}

pub(super) fn pty_stop_payload(
    name: &str,
    expect_stopped: bool,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    if !expect_stopped {
        return Err(CuError::new(
            "pty_job_stop_intent_required",
            "pty-stop requires explicit --expect stopped",
        ));
    }
    let directory = job_directory(name)?;
    let _lock = acquire_job_lock(&directory)?;
    let client = client_for(name)?;
    let (_, tab) = sole_job(&client, name)?;
    let tab_id = tab["id"].as_str().ok_or_else(|| {
        CuError::new("pty_job_state_invalid", "PTY job tab omitted its stable id")
    })?;
    let closed = terminal_close_with_client(&client, tab_id, true, receipts)?;
    let (verified, shutdown_acknowledged, shutdown_transport_error) = shutdown_and_verify(&client);
    if verified {
        return Ok(json!({
            "name": name,
            "performed": true,
            "verified": true,
            "state": "stopped",
            "closed": closed,
            "shutdown_acknowledged": shutdown_acknowledged,
            "shutdown_transport_error": shutdown_transport_error,
        }));
    }
    let detail = json!({
        "name": name,
        "performed": true,
        "verified": false,
        "shutdown_acknowledged": shutdown_acknowledged,
        "shutdown_transport_error": shutdown_transport_error,
    });
    Err(CuError::new(
        "pty_job_stop_unverified",
        "PTY job tab closed but the headless authority remained reachable",
    )
    .with_detail(detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_bounded_and_map_to_isolated_instances() {
        validate_name("build-01").unwrap();
        assert_eq!(instance_name("build-01"), "ephemeral:acu-pty-build-01");
        assert_eq!(
            validate_name("bad/name").unwrap_err().code,
            "pty_job_name_invalid"
        );
        assert_eq!(
            validate_name(&"x".repeat(65)).unwrap_err().code,
            "pty_job_name_invalid"
        );
    }

    #[test]
    fn exact_wait_preserves_a_match_split_across_pages() {
        let mut overlap = Vec::new();
        assert_eq!(scan_exact_page(&mut overlap, b"xxNE", b"NEED", 10), None);
        assert_eq!(
            scan_exact_page(&mut overlap, b"EDyy", b"NEED", 14),
            Some(12)
        );
    }

    #[test]
    fn registry_lock_serializes_start_and_prune() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-pty-registry-lock-{}",
            std::process::id()
        ));
        let _ = fs::remove_file(root.with_extension("registry.lock"));
        let first = acquire_registry_lock(&root).expect("first registry lock");
        let second = match acquire_registry_lock(&root) {
            Ok(_) => panic!("registry lock must contend"),
            Err(error) => error,
        };
        assert_eq!(second.code, "pty_job_registry_busy");
        drop(first);
        acquire_registry_lock(&root).expect("registry lock released");
        let _ = fs::remove_file(root.with_extension("registry.lock"));
    }

    #[test]
    fn receipt_attachment_preserves_typed_control_detail() {
        let error = CuError::new("typed", "failure").with_detail(json!({
            "category": "unsupported",
            "retryable": false,
        }));
        let enriched = attach_receipt(error, json!({ "id": "receipt-1" }));
        let detail = enriched.detail.expect("detail");
        assert_eq!(detail["category"], "unsupported");
        assert_eq!(detail["retryable"], false);
        assert_eq!(detail["receipt"]["id"], "receipt-1");
    }
}
