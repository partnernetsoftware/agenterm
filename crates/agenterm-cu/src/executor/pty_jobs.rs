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
use crate::{CuError, TerminalWaitCondition, receipt::ReceiptLog};

use super::{
    parse_output, request, terminal_close_with_client, terminal_inventory_with_client,
    terminal_new_with_client, terminal_output_with_client, terminal_send_with_client,
    terminal_wait_with_client,
};

const READY_TIMEOUT: Duration = Duration::from_secs(5);

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

fn job_directory(name: &str) -> Result<PathBuf, CuError> {
    let root = host_directories()
        .map_err(|error| CuError::new("pty_job_state_unavailable", error.to_string()))?
        .local_data
        .join("agenterm")
        .join("pty-jobs");
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
    loop {
        match terminal_inventory_with_client(&client) {
            Ok(inventory) if inventory["tabs"].as_array().is_some_and(Vec::is_empty) => {
                return Ok(mode);
            }
            Ok(_) => {
                return Err(CuError::new(
                    "pty_job_state_conflict",
                    "new headless PTY authority did not start with an empty tab set",
                ));
            }
            Err(_) if Instant::now() < deadline => {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(CuError::new(
                        "pty_server_start_failed",
                        format!("AgenTerm headless server exited before readiness: {status}"),
                    ));
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(CuError::new(
                    "pty_server_ready_timeout",
                    format!("headless server did not become ready: {}", error.message),
                ));
            }
        }
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
}
