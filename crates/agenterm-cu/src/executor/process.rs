//! Cross-platform process observation through `agenterm-platform`.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    CuError,
    command::{ProcessKillMode, ProcessRunState, ProcessSignalKind},
    receipt::ReceiptLog,
};

use super::error_payload;
use super::process_signal_recovery::{RecoveryMemberInput, RecoveryStore};

const DEFAULT_MAX: usize = 200;
const MAX_RESULTS: usize = 5_000;
const DEFAULT_ARGV_LIMIT: usize = 100;
const MAX_ARGV_LIMIT: usize = 4_096;
const DEFAULT_ENVIRONMENT_LIMIT: usize = 256;
const MAX_ENVIRONMENT_LIMIT: usize = 5_000;
const MAX_ENVIRONMENT_ENTRIES: usize = 100_001;
const DEFAULT_INSPECTION_LIMIT: usize = 256;
const DEFAULT_INSPECTION_MAX_VISITED: usize = 4_096;
const DEFAULT_USAGE_INTERVAL_MS: u64 = 1_000;
const DEFAULT_USAGE_MAX_SAMPLES: usize = 120;
const MAX_USAGE_WATCH_MS: u64 = 86_400_000;
const MAX_USAGE_INTERVAL_MS: u64 = 60_000;
const MAX_USAGE_SAMPLES: usize = 4_096;
const DEFAULT_PROCESS_WATCH_INTERVAL_MS: u64 = 1_000;
const DEFAULT_PROCESS_WATCH_MAX_EVENTS: usize = 256;
const DEFAULT_PROCESS_WATCH_MAX_PROCESSES: usize = 1_000;

#[derive(Clone)]
struct WatchedProcess {
    pid: u32,
    parent_pid: u32,
    executable_name: String,
    start_identity: String,
}

struct ProcessWatchSnapshot {
    processes: BTreeMap<(u32, String), WatchedProcess>,
    excluded_unidentified: usize,
}

struct TreeSignalMember {
    pid: u32,
    depth: usize,
    identity: String,
    reference: agenterm_platform::process_reference::ProcessReference,
    was_stopped: bool,
    frozen_by_us: bool,
}

#[derive(Clone, Copy)]
pub(super) struct ProcessSignalOptions {
    pub timeout_ms: u64,
    pub force: bool,
    pub tree: bool,
    pub max_descendants: usize,
}

struct TreeSignalCheck {
    pid: u32,
    verified: Option<bool>,
    state: &'static str,
}

impl WatchedProcess {
    fn key(&self) -> (u32, String) {
        (self.pid, self.start_identity.clone())
    }

    fn json(&self) -> Value {
        json!({
            "pid": self.pid,
            "parent_pid": self.parent_pid,
            "executable_name": self.executable_name,
            "start_identity": self.start_identity,
        })
    }
}

pub(super) fn process_state_payload(pid: u32) -> Result<Value, CuError> {
    if pid == 0 {
        return Err(CuError::new(
            "invalid_input",
            "process-state --pid must be greater than zero",
        ));
    }
    let (state, start_identity, reason) = match agenterm_platform::process_observation::observe(pid)
    {
        agenterm_platform::process_observation::ProcessObservation::Live { start_identity } => {
            ("live", start_identity, None)
        }
        agenterm_platform::process_observation::ProcessObservation::Dead { reason } => {
            ("dead", None, Some(reason))
        }
        agenterm_platform::process_observation::ProcessObservation::Unknown { reason } => {
            ("unknown", None, Some(reason))
        }
        _ => (
            "unknown",
            None,
            Some("process_observation_variant_unsupported".to_owned()),
        ),
    };
    Ok(json!({
        "pid": pid,
        "state": state,
        "start_identity": start_identity,
        "reason": reason,
        "verified": state != "unknown",
    }))
}

pub(super) fn process_argv_payload(
    pid: u32,
    values: bool,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Value, CuError> {
    if pid == 0 {
        return Err(CuError::new(
            "invalid_input",
            "process-argv --pid must be greater than zero",
        ));
    }
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(DEFAULT_ARGV_LIMIT);
    if !(1..=MAX_ARGV_LIMIT).contains(&limit) {
        return Err(CuError::new(
            "invalid_input",
            format!("process-argv --limit must be in 1..={MAX_ARGV_LIMIT}"),
        ));
    }

    let start_identity = live_start_identity(pid)?;
    let executable = agenterm_platform::process_image::executable_path(pid).map_err(|error| {
        CuError::new("process_image_failed", error.to_string()).with_detail(json!({
            "kind": format!("{:?}", error.kind()),
        }))
    })?;
    let arguments = agenterm_platform::process::arguments(pid).map_err(|error| {
        CuError::new("process_arguments_failed", error.to_string()).with_detail(json!({
            "kind": format!("{:?}", error.kind()),
        }))
    })?;
    let after_identity = live_start_identity(pid)?;
    if after_identity != start_identity {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity changed while arguments were read",
        ));
    }

    let total = arguments.len();
    let rows = arguments
        .into_iter()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(index, value)| {
            let bytes = value.as_bytes();
            let mut row = serde_json::Map::from_iter([
                ("index".to_owned(), json!(index)),
                ("byte_length".to_owned(), json!(bytes.len().to_string())),
                (
                    "sha256".to_owned(),
                    json!(super::clipboard::clipboard_sha256_hex(bytes)),
                ),
            ]);
            if values {
                row.insert("value".to_owned(), json!(value));
            }
            Value::Object(row)
        })
        .collect::<Vec<_>>();
    let returned = rows.len();
    Ok(json!({
        "pid": pid,
        "start_identity": start_identity,
        "executable": executable.to_string_lossy(),
        "arguments": rows,
        "values_included": values,
        "total": total,
        "returned": returned,
        "offset": offset,
        "truncated": offset.saturating_add(returned) < total,
        "verified": true,
    }))
}

pub(super) fn process_cwd_payload(pid: u32) -> Result<Value, CuError> {
    if pid == 0 {
        return Err(CuError::new(
            "invalid_input",
            "process-cwd --pid must be greater than zero",
        ));
    }

    let start_identity = live_start_identity(pid)?;
    let path = agenterm_platform::process::current_directory(pid).map_err(process_cwd_error)?;
    let after_identity = live_start_identity(pid)?;
    if after_identity != start_identity {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity changed while its working directory was read",
        ));
    }
    let path = path.to_str().ok_or_else(|| {
        CuError::new(
            "process_cwd_invalid_data",
            "process working directory is not valid UTF-8",
        )
    })?;
    let bytes = path.as_bytes();
    Ok(json!({
        "pid": pid,
        "start_identity": start_identity,
        "provider": process_cwd_provider(),
        "path": path,
        "path_byte_length": bytes.len().to_string(),
        "path_sha256": super::clipboard::clipboard_sha256_hex(bytes),
        "verified": true,
    }))
}

pub(super) fn process_environment_payload(
    pid: u32,
    prefix: Option<&str>,
    values: bool,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Value, CuError> {
    if pid == 0 {
        return Err(CuError::new(
            "invalid_input",
            "process-environment --pid must be greater than zero",
        ));
    }
    let prefix = prefix.unwrap_or("").as_bytes();
    if prefix.len() > 256 || prefix.iter().any(|byte| matches!(byte, 0 | b'\r' | b'\n')) {
        return Err(CuError::new(
            "invalid_input",
            "process-environment --prefix must be at most 256 UTF-8 bytes without NUL/CR/LF",
        ));
    }
    let offset = offset.unwrap_or(0);
    if offset > 100_000 {
        return Err(CuError::new(
            "invalid_input",
            "process-environment --offset must be in 0..=100000",
        ));
    }
    let limit = limit.unwrap_or(DEFAULT_ENVIRONMENT_LIMIT);
    if !(1..=MAX_ENVIRONMENT_LIMIT).contains(&limit) {
        return Err(CuError::new(
            "invalid_input",
            format!("process-environment --limit must be in 1..={MAX_ENVIRONMENT_LIMIT}"),
        ));
    }

    let start_identity = live_start_identity(pid)?;
    let snapshot =
        agenterm_platform::process::environment_snapshot(pid).map_err(process_environment_error)?;
    let after_identity = live_start_identity(pid)?;
    if after_identity != start_identity {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity changed while its initial environment was read",
        ));
    }
    if snapshot.entries.len() > MAX_ENVIRONMENT_ENTRIES {
        return Err(CuError::new(
            "process_environment_too_large",
            format!("process initial environment exceeds {MAX_ENVIRONMENT_ENTRIES} entries"),
        ));
    }

    let mut entries = snapshot
        .entries
        .iter()
        .map(|entry| split_environment_entry(&entry.bytes))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(&right.1)));
    entries.retain(|(name, _)| name.starts_with(prefix));
    let total = entries.len();
    let malformed_entries = entries.iter().filter(|(_, value)| value.is_none()).count();
    let rows = entries
        .into_iter()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(index, (name, value))| environment_row(index, name, value, values))
        .collect::<Vec<_>>();
    let returned = rows.len();
    let next_offset = offset.checked_add(returned).filter(|next| *next < total);

    Ok(json!({
        "pid": pid,
        "start_identity": start_identity,
        "provider": process_environment_provider(),
        "semantics": "exec-initial",
        "values_included": values,
        "source_bytes": snapshot.source_bytes.to_string(),
        "total": total,
        "returned": returned,
        "offset": offset,
        "next_offset": next_offset,
        "truncated": next_offset.is_some(),
        "malformed_entries": malformed_entries,
        "entries": rows,
        "verified": true,
    }))
}

fn inspection_bounds(
    offset: Option<usize>,
    limit: Option<usize>,
    max_visited: Option<usize>,
) -> Result<(usize, usize, usize), CuError> {
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(DEFAULT_INSPECTION_LIMIT);
    let max_visited = max_visited.unwrap_or(DEFAULT_INSPECTION_MAX_VISITED);
    if offset > 100_000
        || !(1..=MAX_RESULTS).contains(&limit)
        || !(1..=10_000).contains(&max_visited)
    {
        return Err(CuError::new(
            "invalid_input",
            "process inspection requires offset 0..=100000, limit 1..=5000 and max-visited 1..=10000",
        ));
    }
    Ok((offset, limit, max_visited))
}

fn identity_bracket<T>(
    pid: u32,
    subject: &str,
    read: impl FnOnce() -> Result<T, agenterm_platform::process::ProcessError>,
) -> Result<(String, T), CuError> {
    let identity = live_start_identity(pid)?;
    let value = read().map_err(|error| process_inspection_error(subject, error))?;
    if live_start_identity(pid)? != identity {
        return Err(CuError::new(
            "process_identity_changed",
            format!("process start identity changed while {subject} were read"),
        ));
    }
    Ok((identity, value))
}

fn process_inspection_error(
    subject: &str,
    error: agenterm_platform::process::ProcessError,
) -> CuError {
    use agenterm_platform::process::ProcessErrorKind;
    let code = match error.kind() {
        ProcessErrorKind::IdOutOfRange => "invalid_input",
        ProcessErrorKind::NotFound => "process_not_found",
        ProcessErrorKind::PermissionDenied => "process_inspection_permission_denied",
        ProcessErrorKind::InventoryTooLarge => "process_inspection_too_large",
        ProcessErrorKind::InvalidData => "process_inspection_invalid_data",
        ProcessErrorKind::Unsupported => "process_inspection_unsupported",
        _ => "process_inspection_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "subject": subject,
        "kind": format!("{:?}", error.kind()),
    }))
}

fn raw_contains_ascii_case_insensitive(value: &[u8], needle: Option<&str>) -> bool {
    let Some(needle) = needle else { return true };
    let needle = needle.as_bytes().to_ascii_lowercase();
    value
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(&needle))
}

pub(super) fn process_fds_payload(
    pid: u32,
    kind: Option<&str>,
    target_filter: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
    max_visited: Option<usize>,
) -> Result<Value, CuError> {
    let (offset, limit, max_visited) = inspection_bounds(offset, limit, max_visited)?;
    let (start_identity, snapshot) = identity_bracket(pid, "file descriptors", || {
        agenterm_platform::process::file_descriptors(pid, max_visited)
    })?;
    let matching = snapshot
        .items
        .iter()
        .filter(|row| kind.is_none_or(|kind| row.kind.eq_ignore_ascii_case(kind)))
        .filter(|row| {
            target_filter.is_none()
                || row.target.as_deref().is_some_and(|target| {
                    raw_contains_ascii_case_insensitive(target, target_filter)
                })
        })
        .collect::<Vec<_>>();
    let matched = matching.len();
    let rows = matching
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|row| {
            let mut value = serde_json::Map::from_iter([
                ("fd".to_owned(), json!(row.descriptor)),
                ("type".to_owned(), json!(row.kind)),
                ("open_flags".to_owned(), json!(row.open_flags)),
                ("status_flags".to_owned(), json!(row.status_flags)),
                (
                    "offset_bytes".to_owned(),
                    json!(row.offset_bytes.map(|value| value.to_string())),
                ),
                ("file_type".to_owned(), json!(row.file_type)),
                ("guard_flags".to_owned(), json!(row.guard_flags)),
            ]);
            if let Some(target) = row.target.as_deref() {
                insert_raw_text(&mut value, "target", target, true);
            }
            Value::Object(value)
        })
        .collect::<Vec<_>>();
    let returned = rows.len();
    let next_offset = offset.checked_add(returned).filter(|next| *next < matched);
    Ok(json!({
        "pid": pid,
        "start_identity": start_identity,
        "provider": process_inspection_provider("fds"),
        "descriptors": rows,
        "visited_count": snapshot.visited_count,
        "matched_count": matched,
        "returned_count": returned,
        "read_errors": snapshot.read_errors,
        "offset": offset,
        "limit": limit,
        "max_visited": max_visited,
        "truncated_results": next_offset.is_some(),
        "truncated_scan": snapshot.truncated_scan,
        "next_offset": next_offset,
        "verified": true,
    }))
}

pub(super) fn process_maps_payload(
    pid: u32,
    path: Option<&str>,
    permissions: Option<&str>,
    executable_only: bool,
    offset: Option<usize>,
    limit: Option<usize>,
    max_visited: Option<usize>,
) -> Result<Value, CuError> {
    let (offset, limit, max_visited) = inspection_bounds(offset, limit, max_visited)?;
    let (start_identity, snapshot) = identity_bracket(pid, "memory regions", || {
        agenterm_platform::process::memory_regions(pid, max_visited)
    })?;
    let required = permissions.unwrap_or("").as_bytes();
    let matching = snapshot
        .items
        .iter()
        .filter(|row| !executable_only || row.permissions.contains('x'))
        .filter(|row| {
            required
                .iter()
                .all(|byte| row.permissions.as_bytes().contains(byte))
        })
        .filter(|row| {
            path.is_none()
                || row
                    .path
                    .as_deref()
                    .is_some_and(|mapped| raw_contains_ascii_case_insensitive(mapped, path))
        })
        .collect::<Vec<_>>();
    let matched = matching.len();
    let rows = matching
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|row| {
            let end = row
                .start_address
                .checked_add(row.size_bytes)
                .ok_or_else(|| {
                    CuError::new(
                        "process_inspection_invalid_data",
                        "memory-region range overflows its address space",
                    )
                })?;
            let mut value = serde_json::Map::from_iter([
                (
                    "start_address".to_owned(),
                    json!(format!("0x{:x}", row.start_address)),
                ),
                ("end_address".to_owned(), json!(format!("0x{end:x}"))),
                ("size_bytes".to_owned(), json!(row.size_bytes.to_string())),
                (
                    "offset_bytes".to_owned(),
                    json!(row.offset_bytes.to_string()),
                ),
                ("permissions".to_owned(), json!(row.permissions)),
                ("max_permissions".to_owned(), json!(row.max_permissions)),
                ("sharing".to_owned(), json!(row.sharing)),
                ("device".to_owned(), json!(row.device)),
                (
                    "inode".to_owned(),
                    json!(row.inode.map(|value| value.to_string())),
                ),
                ("flags".to_owned(), json!(row.flags)),
                ("user_tag".to_owned(), json!(row.user_tag)),
                ("depth".to_owned(), json!(row.depth)),
                ("resident_pages".to_owned(), json!(row.resident_pages)),
                (
                    "private_resident_pages".to_owned(),
                    json!(row.private_resident_pages),
                ),
                (
                    "shared_resident_pages".to_owned(),
                    json!(row.shared_resident_pages),
                ),
                ("swapped_pages".to_owned(), json!(row.swapped_pages)),
                ("dirtied_pages".to_owned(), json!(row.dirtied_pages)),
            ]);
            if let Some(path) = row.path.as_deref() {
                insert_raw_text(&mut value, "path", path, true);
            }
            Ok(Value::Object(value))
        })
        .collect::<Result<Vec<_>, CuError>>()?;
    let returned = rows.len();
    let next_offset = offset.checked_add(returned).filter(|next| *next < matched);
    Ok(json!({
        "pid": pid,
        "start_identity": start_identity,
        "provider": process_inspection_provider("maps"),
        "regions": rows,
        "visited_count": snapshot.visited_count,
        "matched_count": matched,
        "returned_count": returned,
        "read_errors": snapshot.read_errors,
        "offset": offset,
        "limit": limit,
        "max_visited": max_visited,
        "truncated_results": next_offset.is_some(),
        "truncated_scan": snapshot.truncated_scan,
        "next_offset": next_offset,
        "verified": true,
    }))
}

pub(super) fn process_threads_payload(
    pid: u32,
    name: Option<&str>,
    state: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
    max_visited: Option<usize>,
) -> Result<Value, CuError> {
    let (offset, limit, max_visited) = inspection_bounds(offset, limit, max_visited)?;
    let (start_identity, snapshot) = identity_bracket(pid, "threads", || {
        agenterm_platform::process::threads(pid, max_visited)
    })?;
    let matching = snapshot
        .items
        .iter()
        .filter(|row| state.is_none_or(|state| row.state.eq_ignore_ascii_case(state)))
        .filter(|row| {
            name.is_none()
                || row.name.as_deref().is_some_and(|thread_name| {
                    raw_contains_ascii_case_insensitive(thread_name, name)
                })
        })
        .collect::<Vec<_>>();
    let matched = matching.len();
    let rows = matching
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|row| {
            let mut value = serde_json::Map::from_iter([
                ("id".to_owned(), json!(row.id.to_string())),
                ("state".to_owned(), json!(row.state)),
                ("state_raw".to_owned(), json!(row.state_raw)),
                (
                    "user_time_raw".to_owned(),
                    json!(row.user_time_raw.to_string()),
                ),
                (
                    "system_time_raw".to_owned(),
                    json!(row.system_time_raw.to_string()),
                ),
                ("time_unit".to_owned(), json!(row.time_unit)),
                (
                    "cpu_usage_pct".to_owned(),
                    json!(
                        row.cpu_usage_tenths_percent
                            .map(|value| f64::from(value) / 10.0)
                    ),
                ),
                ("policy".to_owned(), json!(row.policy)),
                ("flags".to_owned(), json!(row.flags)),
                ("sleep_seconds".to_owned(), json!(row.sleep_seconds)),
                ("current_priority".to_owned(), json!(row.current_priority)),
                ("priority".to_owned(), json!(row.priority)),
                ("max_priority".to_owned(), json!(row.max_priority)),
                ("nice".to_owned(), json!(row.nice)),
                ("processor".to_owned(), json!(row.processor)),
            ]);
            if let Some(name) = row.name.as_deref() {
                insert_raw_text(&mut value, "name", name, true);
            }
            Value::Object(value)
        })
        .collect::<Vec<_>>();
    let returned = rows.len();
    let next_offset = offset.checked_add(returned).filter(|next| *next < matched);
    Ok(json!({
        "pid": pid,
        "start_identity": start_identity,
        "provider": process_inspection_provider("threads"),
        "threads": rows,
        "visited_count": snapshot.visited_count,
        "matched_count": matched,
        "returned_count": returned,
        "read_errors": snapshot.read_errors,
        "offset": offset,
        "limit": limit,
        "max_visited": max_visited,
        "truncated_results": next_offset.is_some(),
        "truncated_scan": snapshot.truncated_scan,
        "next_offset": next_offset,
        "verified": true,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn process_sockets_payload(
    pid: u32,
    family: Option<&str>,
    protocol: Option<&str>,
    state: Option<&str>,
    endpoint: Option<&str>,
    offset: Option<usize>,
    limit: Option<usize>,
    max_visited: Option<usize>,
) -> Result<Value, CuError> {
    let (offset, limit, max_visited) = inspection_bounds(offset, limit, max_visited)?;
    let (start_identity, snapshot) = identity_bracket(pid, "sockets", || {
        agenterm_platform::process::sockets(pid, max_visited)
    })?;
    let matching = snapshot
        .items
        .iter()
        .filter(|row| family.is_none_or(|value| row.family.eq_ignore_ascii_case(value)))
        .filter(|row| protocol.is_none_or(|value| row.protocol.eq_ignore_ascii_case(value)))
        .filter(|row| {
            state.is_none_or(|value| {
                row.state
                    .as_deref()
                    .is_some_and(|state| state.eq_ignore_ascii_case(value))
            })
        })
        .filter(|row| raw_contains_ascii_case_insensitive(&row.endpoint, endpoint))
        .collect::<Vec<_>>();
    let matched = matching.len();
    let rows = matching
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|row| {
            let mut value = serde_json::Map::from_iter([
                ("fd".to_owned(), json!(row.descriptor)),
                ("family".to_owned(), json!(row.family)),
                ("protocol".to_owned(), json!(row.protocol)),
                ("local".to_owned(), Value::Null),
                ("remote".to_owned(), Value::Null),
                ("state".to_owned(), json!(row.state)),
                (
                    "inode".to_owned(),
                    json!(row.inode.map(|value| value.to_string())),
                ),
            ]);
            if let Some(local) = row.local.as_deref() {
                insert_raw_text(&mut value, "local", local, true);
            }
            if let Some(remote) = row.remote.as_deref() {
                insert_raw_text(&mut value, "remote", remote, true);
            }
            insert_raw_text(&mut value, "endpoint", &row.endpoint, true);
            Value::Object(value)
        })
        .collect::<Vec<_>>();
    let returned = rows.len();
    let next_offset = offset.checked_add(returned).filter(|next| *next < matched);
    Ok(json!({
        "pid": pid,
        "start_identity": start_identity,
        "provider": process_inspection_provider("sockets"),
        "sockets": rows,
        "visited_count": snapshot.visited_count,
        "matched_count": matched,
        "returned_count": returned,
        "read_errors": snapshot.read_errors,
        "offset": offset,
        "limit": limit,
        "max_visited": max_visited,
        "truncated_results": next_offset.is_some(),
        "truncated_scan": snapshot.truncated_scan,
        "next_offset": next_offset,
        "verified": true,
    }))
}

pub(super) fn process_cgroup_payload(
    pid: u32,
    expected_start_identity: Option<&str>,
) -> Result<Value, CuError> {
    use agenterm_platform::process::{
        ProcessCgroupCounter, ProcessCgroupLimit, ProcessCgroupUnavailableKind,
    };

    fn limit(value: &ProcessCgroupLimit) -> Value {
        match value {
            ProcessCgroupLimit::Max => json!("max"),
            ProcessCgroupLimit::Value(value) => json!(value.to_string()),
        }
    }

    fn optional_limit(value: &Option<ProcessCgroupLimit>) -> Value {
        value.as_ref().map_or(Value::Null, limit)
    }

    fn optional_counter(value: Option<u64>) -> Value {
        value.map_or(Value::Null, |value| json!(value.to_string()))
    }

    fn counters(values: &[ProcessCgroupCounter]) -> Value {
        Value::Object(
            values
                .iter()
                .map(|counter| (counter.name.clone(), json!(counter.value.to_string())))
                .collect(),
        )
    }

    let snapshot = agenterm_platform::process::cgroup_v2(pid, expected_start_identity)
        .map_err(process_cgroup_error)?;
    let mut membership = serde_json::Map::from_iter([
        ("path".to_owned(), Value::Null),
        (
            "directory_device".to_owned(),
            json!(snapshot.directory_device.to_string()),
        ),
        (
            "directory_inode".to_owned(),
            json!(snapshot.directory_inode.to_string()),
        ),
    ]);
    insert_raw_text(&mut membership, "path", &snapshot.path, true);
    let io = snapshot
        .io
        .iter()
        .map(|device| {
            json!({
                "major": device.major,
                "minor": device.minor,
                "counters": counters(&device.counters),
            })
        })
        .collect::<Vec<_>>();
    let unavailable = snapshot
        .unavailable
        .iter()
        .map(|field| {
            json!({
                "field": field.field,
                "kind": match field.kind {
                    ProcessCgroupUnavailableKind::NotPresent => "not-present",
                    ProcessCgroupUnavailableKind::PermissionDenied => "permission-denied",
                },
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "schema": 1,
        "provider": snapshot.provider,
        "process": {
            "pid": snapshot.process_id,
            "start_identity": snapshot.start_identity,
            "verified": true,
        },
        "membership": Value::Object(membership),
        "controllers": {
            "available": snapshot.controllers,
            "enabled_for_children": snapshot.subtree_control,
        },
        "state": {
            "populated": snapshot.populated,
            "frozen": snapshot.frozen,
        },
        "limits": {
            "cpu": {
                "quota_usec": snapshot.cpu_max.as_ref().map_or(Value::Null, |value| limit(&value.quota)),
                "period_usec": snapshot.cpu_max.as_ref().map_or(Value::Null, |value| json!(value.period_microseconds.to_string())),
                "weight": optional_counter(snapshot.cpu_weight),
            },
            "memory": {
                "high_bytes": optional_limit(&snapshot.memory_high_bytes),
                "max_bytes": optional_limit(&snapshot.memory_max_bytes),
                "swap_max_bytes": optional_limit(&snapshot.memory_swap_max_bytes),
            },
            "pids": {
                "max": optional_limit(&snapshot.pids_max),
            },
        },
        "usage": {
            "cpu": counters(&snapshot.cpu_stat),
            "memory": {
                "current_bytes": optional_counter(snapshot.memory_current_bytes),
                "swap_current_bytes": optional_counter(snapshot.memory_swap_current_bytes),
                "events": counters(&snapshot.memory_events),
            },
            "pids": {
                "current": optional_counter(snapshot.pids_current),
                "events": counters(&snapshot.pids_events),
            },
            "io": io,
        },
        "unavailable": unavailable,
        "consistency": "exact-process-and-membership-bracketed-point-reads",
    }))
}

fn process_cgroup_error(error: agenterm_platform::process::ProcessCgroupError) -> CuError {
    use agenterm_platform::process::ProcessCgroupErrorKind;

    let code = match error.kind() {
        ProcessCgroupErrorKind::IdOutOfRange => "invalid_input",
        ProcessCgroupErrorKind::NotFound => "process_not_found",
        ProcessCgroupErrorKind::PermissionDenied => "process_cgroup_permission_denied",
        ProcessCgroupErrorKind::NotApplicable => "process_cgroup_not_applicable",
        ProcessCgroupErrorKind::V2Unavailable => "process_cgroup_v2_unavailable",
        ProcessCgroupErrorKind::InventoryTooLarge => "process_cgroup_too_large",
        ProcessCgroupErrorKind::InvalidData => "process_cgroup_invalid_data",
        ProcessCgroupErrorKind::IdentityChanged => "process_identity_changed",
        ProcessCgroupErrorKind::MembershipChanged => "process_cgroup_membership_changed",
        ProcessCgroupErrorKind::DirectoryChanged => "process_cgroup_directory_changed",
        ProcessCgroupErrorKind::Inspect => "process_cgroup_failed",
        _ => "process_cgroup_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}

fn process_inspection_provider(subject: &str) -> String {
    let host = if cfg!(target_os = "macos") {
        "darwin-libproc"
    } else if cfg!(target_os = "linux") {
        "linux-proc"
    } else {
        "windows-unsupported"
    };
    format!("{host}-{subject}")
}

fn split_environment_entry(bytes: &[u8]) -> (&[u8], Option<&[u8]>) {
    match bytes.iter().position(|byte| *byte == b'=') {
        Some(equals) => (&bytes[..equals], Some(&bytes[equals + 1..])),
        None => (bytes, None),
    }
}

fn environment_row(index: usize, name: &[u8], value: Option<&[u8]>, values: bool) -> Value {
    let mut row = serde_json::Map::from_iter([
        ("index".to_owned(), json!(index)),
        ("name_byte_length".to_owned(), json!(name.len().to_string())),
        (
            "name_sha256".to_owned(),
            json!(super::clipboard::clipboard_sha256_hex(name)),
        ),
        ("has_value".to_owned(), json!(value.is_some())),
    ]);
    insert_raw_text(&mut row, "name", name, true);
    if let Some(value) = value {
        row.insert(
            "value_byte_length".to_owned(),
            json!(value.len().to_string()),
        );
        row.insert(
            "value_sha256".to_owned(),
            json!(super::clipboard::clipboard_sha256_hex(value)),
        );
        if values {
            insert_raw_text(&mut row, "value", value, true);
        }
    }
    Value::Object(row)
}

fn insert_raw_text(
    row: &mut serde_json::Map<String, Value>,
    field: &str,
    bytes: &[u8],
    include_text: bool,
) {
    match std::str::from_utf8(bytes) {
        Ok(text) => {
            row.insert(format!("{field}_encoding"), json!("utf8"));
            if include_text {
                row.insert(field.to_owned(), json!(text));
            }
        }
        Err(_) => {
            row.insert(format!("{field}_encoding"), json!("hex"));
            row.insert(field.to_owned(), Value::Null);
            row.insert(format!("{field}_hex"), json!(encode_hex(bytes)));
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn process_environment_error(error: agenterm_platform::process::ProcessError) -> CuError {
    use agenterm_platform::process::ProcessErrorKind;

    let code = match error.kind() {
        ProcessErrorKind::IdOutOfRange => "invalid_input",
        ProcessErrorKind::NotFound => "process_not_found",
        ProcessErrorKind::PermissionDenied => "process_environment_permission_denied",
        ProcessErrorKind::Unavailable => "process_environment_empty_or_omitted",
        ProcessErrorKind::InventoryTooLarge => "process_environment_too_large",
        ProcessErrorKind::InvalidData => "process_environment_invalid_data",
        ProcessErrorKind::Unsupported => "process_environment_unsupported",
        _ => "process_environment_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
        "semantics": "exec-initial",
    }))
}

const fn process_environment_provider() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux-proc-environ"
    }
    #[cfg(target_os = "macos")]
    {
        "macos-kern-procargs2"
    }
    #[cfg(windows)]
    {
        "windows-unsupported"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        "unsupported"
    }
}

fn process_cwd_error(error: agenterm_platform::process::ProcessError) -> CuError {
    use agenterm_platform::process::ProcessErrorKind;

    let code = match error.kind() {
        ProcessErrorKind::IdOutOfRange => "invalid_input",
        ProcessErrorKind::NotFound => "process_not_found",
        ProcessErrorKind::PermissionDenied => "process_cwd_permission_denied",
        ProcessErrorKind::Unavailable => "process_cwd_unavailable",
        ProcessErrorKind::InvalidData => "process_cwd_invalid_data",
        ProcessErrorKind::Unsupported => "process_cwd_unsupported",
        _ => "process_cwd_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}

const fn process_cwd_provider() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux-proc-cwd"
    }
    #[cfg(target_os = "macos")]
    {
        "macos-libproc-vnodepath"
    }
    #[cfg(windows)]
    {
        "windows-unsupported"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        "unsupported"
    }
}

fn live_start_identity(pid: u32) -> Result<String, CuError> {
    match agenterm_platform::process_observation::observe(pid) {
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: Some(identity),
        } => Ok(identity),
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: None,
        } => Err(CuError::new(
            "process_identity_unavailable",
            "process is live but its start identity is unavailable",
        )),
        agenterm_platform::process_observation::ProcessObservation::Dead { reason } => {
            Err(CuError::new("process_not_found", reason))
        }
        agenterm_platform::process_observation::ProcessObservation::Unknown { reason } => {
            Err(CuError::new("process_identity_unknown", reason))
        }
        _ => Err(CuError::new(
            "process_identity_unknown",
            "process observation variant is unsupported",
        )),
    }
}

pub(super) fn process_usage_payload(pid: u32) -> Result<Value, CuError> {
    if pid == 0 {
        return Err(CuError::new(
            "invalid_input",
            "process-usage --pid must be greater than zero",
        ));
    }
    let identity = live_start_identity(pid)?;
    let sample = agenterm_platform::process_metrics::metrics(pid).map_err(|error| {
        CuError::new("process_metrics_failed", error.to_string()).with_detail(json!({
            "kind": format!("{:?}", error.kind()),
        }))
    })?;
    let after_identity = live_start_identity(pid)?;
    if after_identity != identity {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity changed while metrics were sampled",
        ));
    }
    Ok(json!({
        "pid": pid,
        "start_identity": identity,
        "cpu_time_ns": sample.cpu_time.as_nanos().to_string(),
        "resident_bytes": sample.resident_bytes.to_string(),
        "page_faults": {
            "total": sample.page_faults.total.to_string(),
            "soft": sample.page_faults.soft.map(|value| value.to_string()),
            "hard": sample.page_faults.hard.map(|value| value.to_string()),
        },
        "verified": true,
    }))
}

pub(super) fn process_usage_watch_payload(
    pid: u32,
    watch_ms: u64,
    interval_ms: Option<u64>,
    max_samples: Option<usize>,
) -> Result<Value, CuError> {
    let interval_ms = interval_ms.unwrap_or(DEFAULT_USAGE_INTERVAL_MS);
    let max_samples = max_samples.unwrap_or(DEFAULT_USAGE_MAX_SAMPLES);
    if pid == 0
        || !(1..=MAX_USAGE_WATCH_MS).contains(&watch_ms)
        || !(1..=MAX_USAGE_INTERVAL_MS).contains(&interval_ms)
        || !(1..=MAX_USAGE_SAMPLES).contains(&max_samples)
    {
        return Err(CuError::new(
            "invalid_input",
            "process-usage watch requires pid > 0, watch-ms in 1..=86400000, interval-ms in 1..=60000 and max-samples in 1..=4096",
        ));
    }

    let started = Instant::now();
    let deadline = started + Duration::from_millis(watch_ms);
    let mut samples = Vec::with_capacity(max_samples.min(256));
    let mut initial_identity = None::<String>;
    loop {
        let full_sample = process_usage_payload(pid)?;
        let identity = full_sample["start_identity"]
            .as_str()
            .ok_or_else(|| {
                CuError::new(
                    "process_identity_unavailable",
                    "process usage sample omitted its start identity",
                )
            })?
            .to_owned();
        if let Some(expected) = initial_identity.as_deref() {
            if expected != identity {
                return Err(CuError::new(
                    "process_identity_changed",
                    "process start identity changed during usage observation",
                )
                .with_detail(json!({
                    "pid": pid,
                    "expected_start_identity": expected,
                    "actual_start_identity": identity,
                    "samples_completed": samples.len(),
                })));
            }
        } else {
            initial_identity = Some(identity);
        }
        samples.push(json!({
            "t_ms": started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            "cpu_time_ns": full_sample["cpu_time_ns"].clone(),
            "resident_bytes": full_sample["resident_bytes"].clone(),
            "page_faults": full_sample["page_faults"].clone(),
        }));

        if Instant::now() >= deadline || samples.len() >= max_samples {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(Duration::from_millis(interval_ms).min(remaining));
    }
    let completed = Instant::now() >= deadline;
    Ok(json!({
        "pid": pid,
        "start_identity": initial_identity,
        "mode": "bounded-series",
        "duration_ms": watch_ms,
        "interval_ms": interval_ms,
        "max_samples": max_samples,
        "emitted": samples.len(),
        "completed": completed,
        "truncated": !completed,
        "verified": true,
        "samples": samples,
    }))
}

pub(super) fn process_wait_payload(
    pid: u32,
    expected_identity: &str,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    if pid == 0 || expected_identity.is_empty() || !(1..=86_400_000).contains(&timeout_ms) {
        return Err(CuError::new(
            "invalid_input",
            "process-wait requires a positive pid, non-empty start identity and timeout-ms in 1..=86400000",
        ));
    }
    let reference =
        agenterm_platform::process_reference::ProcessReference::open(pid).map_err(|error| {
            CuError::new("process_reference_failed", error.to_string())
                .with_detail(json!({ "pid": pid }))
        })?;
    let actual_identity = live_start_identity(pid)?;
    if actual_identity != expected_identity {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity does not match the prior observation",
        )
        .with_detail(json!({
            "pid": pid,
            "expected_start_identity": expected_identity,
            "actual_start_identity": actual_identity,
        })));
    }

    let started = Instant::now();
    let state = reference
        .wait_for_exit(Some(Duration::from_millis(timeout_ms)))
        .map_err(|error| CuError::new("process_wait_failed", error.to_string()))?;
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let (state, completed) = match state {
        agenterm_platform::process_reference::ProcessWait::Exited => ("exited", true),
        agenterm_platform::process_reference::ProcessWait::TimedOut => ("timeout", false),
    };
    Ok(json!({
        "pid": pid,
        "start_identity": expected_identity,
        "state": state,
        "completed": completed,
        "elapsed_ms": elapsed_ms,
        "timeout_ms": timeout_ms,
        "verified": true,
        "mechanism": "native-process-reference",
    }))
}

pub(super) fn process_kill_payload(
    pid: u32,
    expected_identity: &str,
    mode: ProcessKillMode,
    timeout_ms: u64,
    expect_exited: bool,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    if pid == 0
        || expected_identity.is_empty()
        || !(1..=86_400_000).contains(&timeout_ms)
        || !expect_exited
    {
        return Err(CuError::new(
            "invalid_input",
            "process-kill requires a positive pid, non-empty start identity, timeout-ms in 1..=86400000 and --expect exited",
        ));
    }
    let reference = agenterm_platform::process_reference::ProcessReference::open_for_termination(
        pid,
    )
    .map_err(|error| {
        CuError::new("process_reference_failed", error.to_string())
            .with_detail(json!({ "pid": pid }))
    })?;
    let actual_identity = live_start_identity(pid)?;
    if actual_identity != expected_identity {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity does not match the prior observation",
        )
        .with_detail(json!({
            "pid": pid,
            "expected_start_identity": expected_identity,
            "actual_start_identity": actual_identity,
            "effect": "not_performed",
        })));
    }
    if !reference
        .is_alive()
        .map_err(|error| CuError::new("process_reference_failed", error.to_string()))?
    {
        return Err(CuError::new(
            "process_not_found",
            "the exact process object exited before the effect was reserved",
        ));
    }

    let ticket = receipts.reserve(
        "process-kill",
        0,
        json!({
            "process": { "pid": pid, "start_identity": expected_identity },
            "mode": mode.as_str(),
            "expect": "exited",
            "before": { "state": "live" },
        }),
    )?;
    let platform_mode = match mode {
        ProcessKillMode::Graceful => agenterm_platform::process_control::TerminationMode::Graceful,
        ProcessKillMode::Forceful => agenterm_platform::process_control::TerminationMode::Forceful,
    };
    if let Err(error) = reference.terminate(platform_mode) {
        let code = if error.kind() == std::io::ErrorKind::Unsupported {
            "process_signal_unsupported"
        } else {
            "process_signal_failed"
        };
        let error = CuError::new(code, error.to_string());
        receipts.complete(
            &ticket,
            "process-kill",
            0,
            false,
            json!({ "performed": false, "error": error_payload(&error) }),
        )?;
        return Err(error.with_detail(json!({ "receipt": ticket.json() })));
    }

    let started = Instant::now();
    let wait = reference
        .wait_for_exit(Some(Duration::from_millis(timeout_ms)))
        .map_err(|error| CuError::new("process_wait_failed", error.to_string()))?;
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let exited = wait == agenterm_platform::process_reference::ProcessWait::Exited;
    receipts.complete(
        &ticket,
        "process-kill",
        0,
        exited,
        json!({
            "performed": true,
            "after": { "state": if exited { "exited" } else { "live" } },
            "verification": "native-process-reference",
            "elapsed_ms": elapsed_ms,
        }),
    )?;
    let payload = json!({
        "pid": pid,
        "start_identity": expected_identity,
        "mode": mode.as_str(),
        "performed": true,
        "state": if exited { "exited" } else { "live" },
        "verified": exited,
        "elapsed_ms": elapsed_ms,
        "timeout_ms": timeout_ms,
        "mechanism": "native-process-reference",
        "receipt": ticket.json(),
    });
    if exited {
        Ok(payload)
    } else {
        Err(CuError::new(
            "process_still_live",
            "termination was requested but the exact process object did not exit before the deadline",
        )
        .with_detail(json!({ "receipt": payload })))
    }
}

pub(super) fn process_set_state_payload(
    pid: u32,
    expected_identity: &str,
    desired: ProcessRunState,
    timeout_ms: u64,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    if pid == 0 || expected_identity.is_empty() || !(1..=60_000).contains(&timeout_ms) {
        return Err(CuError::new(
            "invalid_input",
            "process-set-state requires a positive pid, non-empty start identity and timeout-ms in 1..=60000",
        ));
    }
    let actual_identity = live_start_identity(pid)?;
    if actual_identity != expected_identity {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity does not match the prior observation",
        ));
    }
    let before_stopped = process_stopped(pid)?;
    let observed_identity = live_start_identity(pid)?;
    if observed_identity != expected_identity {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity changed while its scheduler state was observed",
        ));
    }
    let before = if before_stopped { "stopped" } else { "running" };
    if before_stopped == desired.is_stopped() {
        return Ok(json!({
            "pid": pid,
            "start_identity": expected_identity,
            "before": before,
            "after": before,
            "changed": false,
            "performed": false,
            "verified": true,
            "mechanism": "native-process-reference",
        }));
    }

    let reference = agenterm_platform::process_reference::ProcessReference::open_for_termination(
        pid,
    )
    .map_err(|error| {
        CuError::new("process_reference_failed", error.to_string())
            .with_detail(json!({ "pid": pid }))
    })?;
    let rechecked_identity = live_start_identity(pid)?;
    if rechecked_identity != expected_identity {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity changed before state mutation",
        ));
    }
    let ticket = receipts.reserve(
        "process-set-state",
        0,
        json!({
            "process": { "pid": pid, "start_identity": expected_identity },
            "before": before,
            "requested": desired.as_str(),
        }),
    )?;
    if let Err(error) = reference.set_suspended(desired.is_stopped()) {
        let code = if error.kind() == std::io::ErrorKind::Unsupported {
            "process_state_unsupported"
        } else {
            "process_state_change_failed"
        };
        let typed = CuError::new(code, error.to_string());
        receipts.complete(
            &ticket,
            "process-set-state",
            0,
            false,
            json!({ "performed": false, "error": error_payload(&typed) }),
        )?;
        return Err(typed.with_detail(json!({ "receipt": ticket.json() })));
    }

    let started = Instant::now();
    loop {
        let alive = match reference.is_alive() {
            Ok(alive) => alive,
            Err(error) => {
                let typed = CuError::new("process_reference_failed", error.to_string());
                return fail_process_state_after_effect(receipts, &ticket, typed);
            }
        };
        if !alive {
            let typed = CuError::new(
                "process_exited_during_state_change",
                "the exact process exited after the state signal",
            );
            return fail_process_state_after_effect(receipts, &ticket, typed);
        }
        let current_identity = match live_start_identity(pid) {
            Ok(identity) => identity,
            Err(error) => return fail_process_state_after_effect(receipts, &ticket, error),
        };
        if current_identity != expected_identity {
            let typed = CuError::new(
                "process_identity_changed",
                "PID was reused while state verification was in progress",
            );
            return fail_process_state_after_effect(receipts, &ticket, typed);
        }
        let stopped = match process_stopped(pid) {
            Ok(stopped) => stopped,
            Err(error) => return fail_process_state_after_effect(receipts, &ticket, error),
        };
        if stopped == desired.is_stopped() {
            let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            receipts.complete(
                &ticket,
                "process-set-state",
                0,
                true,
                json!({
                    "performed": true,
                    "after": desired.as_str(),
                    "verification": "native-scheduler-state",
                    "elapsed_ms": elapsed_ms,
                }),
            )?;
            return Ok(json!({
                "pid": pid,
                "start_identity": expected_identity,
                "before": before,
                "after": desired.as_str(),
                "changed": true,
                "performed": true,
                "verified": true,
                "elapsed_ms": elapsed_ms,
                "timeout_ms": timeout_ms,
                "mechanism": "native-process-reference",
                "receipt": ticket.json(),
            }));
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            let typed = CuError::new(
                "process_state_not_applied",
                "the exact process did not reach the requested scheduler state before timeout",
            );
            return fail_process_state_after_effect(receipts, &ticket, typed);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn process_signal_payload(
    pid: u32,
    expected_identity: Option<&str>,
    signal: ProcessSignalKind,
    options: ProcessSignalOptions,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    let ProcessSignalOptions {
        timeout_ms,
        force,
        tree,
        max_descendants,
    } = options;
    if pid == 0
        || expected_identity.is_some_and(str::is_empty)
        || !(1..=60_000).contains(&timeout_ms)
        || (signal == ProcessSignalKind::Kill) != force
        || !(1..=10_000).contains(&max_descendants)
    {
        return Err(CuError::new(
            "invalid_input",
            "process-signal requires a positive pid, bounded timeout and --force exactly for SIGKILL",
        ));
    }
    if tree {
        return process_tree_signal_payload(
            pid,
            expected_identity,
            signal,
            timeout_ms,
            force,
            max_descendants,
            receipts,
        );
    }
    let reference =
        agenterm_platform::process_reference::ProcessReference::open_for_termination(pid)
            .map_err(|error| CuError::new("process_reference_failed", error.to_string()))?;
    if !reference
        .is_alive()
        .map_err(|error| CuError::new("process_reference_failed", error.to_string()))?
    {
        return Err(CuError::new(
            "process_not_found",
            "the exact process object exited before signal preparation",
        ));
    }
    let identity = live_start_identity(pid)?;
    if expected_identity.is_some_and(|expected| expected != identity) {
        return Err(CuError::new(
            "process_identity_changed",
            "process start identity does not match the prior observation",
        ));
    }
    if !reference
        .is_alive()
        .map_err(|error| CuError::new("process_reference_failed", error.to_string()))?
    {
        return Err(CuError::new(
            "process_not_found",
            "the bound process object exited before signal reservation",
        ));
    }

    let ticket = receipts.reserve(
        "process-signal",
        0,
        json!({
            "process": { "pid": pid, "start_identity": identity },
            "signal": signal.as_str(),
        }),
    )?;
    let effect = match signal {
        ProcessSignalKind::Terminate => {
            reference.terminate(agenterm_platform::process_control::TerminationMode::Graceful)
        }
        ProcessSignalKind::Kill => {
            reference.terminate(agenterm_platform::process_control::TerminationMode::Forceful)
        }
        ProcessSignalKind::Stop => reference.set_suspended(true),
        ProcessSignalKind::Continue => reference.set_suspended(false),
        ProcessSignalKind::Hangup => {
            reference.send_signal(agenterm_platform::process_reference::ProcessSignal::Hangup)
        }
        ProcessSignalKind::Interrupt => {
            reference.send_signal(agenterm_platform::process_reference::ProcessSignal::Interrupt)
        }
        ProcessSignalKind::User1 => {
            reference.send_signal(agenterm_platform::process_reference::ProcessSignal::User1)
        }
        ProcessSignalKind::User2 => {
            reference.send_signal(agenterm_platform::process_reference::ProcessSignal::User2)
        }
    };
    if let Err(error) = effect {
        let code = if error.kind() == std::io::ErrorKind::Unsupported {
            "process_signal_unsupported"
        } else {
            "process_signal_failed"
        };
        let typed = CuError::new(code, error.to_string());
        receipts.complete(
            &ticket,
            "process-signal",
            0,
            false,
            json!({ "performed": false, "error": error_payload(&typed) }),
        )?;
        return Err(typed.with_detail(json!({ "receipt": ticket.json() })));
    }

    let started = Instant::now();
    let desired_stopped = match signal {
        ProcessSignalKind::Stop => Some(true),
        ProcessSignalKind::Continue => Some(false),
        _ => None,
    };
    let expects_exit = matches!(
        signal,
        ProcessSignalKind::Terminate | ProcessSignalKind::Kill
    );
    let (verified, after) = loop {
        let alive = match reference.is_alive() {
            Ok(alive) => alive,
            Err(error) => {
                let typed =
                    CuError::new("process_reference_failed_after_signal", error.to_string());
                return fail_process_signal_after_effect(receipts, &ticket, typed);
            }
        };
        if expects_exit && !alive {
            break (true, "exited");
        }
        if let Some(stopped) = desired_stopped {
            if !alive {
                break (false, "exited");
            }
            let observed_stopped = match process_stopped(pid) {
                Ok(value) => value,
                Err(error) => return fail_process_signal_after_effect(receipts, &ticket, error),
            };
            if observed_stopped == stopped {
                break (true, if stopped { "stopped" } else { "running" });
            }
        } else if !expects_exit {
            break (false, if alive { "live" } else { "exited" });
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            break (false, if alive { "live" } else { "exited" });
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let success = verified || (!expects_exit && desired_stopped.is_none());
    receipts.complete(
        &ticket,
        "process-signal",
        0,
        success,
        json!({
            "performed": true,
            "verified": verified,
            "after": after,
            "elapsed_ms": elapsed_ms,
        }),
    )?;
    let payload = json!({
        "pid": pid,
        "start_identity": identity,
        "signal": signal.as_str(),
        "performed": true,
        "delivered": true,
        "verified": verified,
        "state": after,
        "elapsed_ms": elapsed_ms,
        "timeout_ms": timeout_ms,
        "mechanism": "native-process-reference",
        "receipt": ticket.json(),
    });
    if success {
        Ok(payload)
    } else {
        Err(CuError::new(
            "process_signal_postcondition_failed",
            "signal was delivered but its required postcondition was not observed before timeout",
        )
        .with_detail(json!({ "receipt": payload })))
    }
}

fn fail_process_signal_after_effect(
    receipts: &mut ReceiptLog,
    ticket: &crate::receipt::ReceiptTicket,
    error: CuError,
) -> Result<Value, CuError> {
    receipts.complete(
        ticket,
        "process-signal",
        0,
        false,
        json!({ "performed": true, "verified": false, "error": error_payload(&error) }),
    )?;
    Err(error.with_detail(json!({
        "receipt": ticket.json(),
        "performed": true,
        "verified": false,
    })))
}

fn process_tree_signal_payload(
    root_pid: u32,
    expected_identity: Option<&str>,
    signal: ProcessSignalKind,
    timeout_ms: u64,
    _force: bool,
    max_descendants: usize,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    if cfg!(windows) {
        return Err(CuError::new(
            "process_tree_signal_unsupported",
            "Windows arbitrary process trees have no retained containment object; use an owned managed job",
        ));
    }
    if root_pid <= 1 {
        return Err(CuError::new(
            "invalid_input",
            "process-signal --tree requires a root pid greater than one",
        ));
    }
    let recovery_store = RecoveryStore::open_beside_receipt(receipts.path())?;
    let recovered_transactions = recovery_store.recover_pending(receipts)?;
    let root = open_tree_signal_member(root_pid, 0)?;
    if expected_identity.is_some_and(|expected| expected != root.identity) {
        return Err(CuError::new(
            "process_identity_changed",
            "process tree root identity does not match the prior observation",
        ));
    }
    let root_identity = root.identity.clone();
    let ticket = receipts.reserve(
        "process-signal-tree",
        0,
        json!({
            "root": { "pid": root_pid, "start_identity": root_identity },
            "signal": signal.as_str(),
            "max_descendants": max_descendants,
        }),
    )?;

    let mut known = BTreeMap::from([(root_pid, root)]);
    let transaction_id = match recovery_store.begin(
        &ticket.id,
        root_pid,
        &root_identity,
        signal,
        &[RecoveryMemberInput {
            pid: root_pid,
            depth: 0,
            start_identity: &root_identity,
            was_stopped: known.get(&root_pid).expect("root retained").was_stopped,
        }],
    ) {
        Ok(id) => id,
        Err(error) => {
            return fail_process_tree_signal(receipts, &ticket, error, false, &known);
        }
    };
    let stable = if signal == ProcessSignalKind::Continue {
        stable_unfrozen_tree(
            root_pid,
            &root_identity,
            max_descendants,
            &mut known,
            &recovery_store,
            &transaction_id,
        )
    } else {
        freeze_stable_tree(
            root_pid,
            &root_identity,
            max_descendants,
            &mut known,
            &recovery_store,
            &transaction_id,
        )
    };
    let members = match stable {
        Ok(members) => members,
        Err(error) => {
            let error = match recovery_store.recover_pending(receipts) {
                Ok(_) => error,
                Err(restore_error) => CuError::new(
                    "process_tree_recovery_failed",
                    format!("{}; rollback: {}", error.message, restore_error.message),
                ),
            };
            return Err(process_tree_error_detail(error, &ticket, &known));
        }
    };

    let mut delivery_error = None;
    for pid in members.iter().rev() {
        let member = known.get(pid).expect("stable member retained");
        if let Err(error) = recovery_store.before_delivery(&transaction_id, member.pid) {
            delivery_error = Some(error);
            break;
        }
        if let Err(error) = deliver_tree_signal(member, signal) {
            delivery_error = Some(CuError::new(
                if error.kind() == std::io::ErrorKind::Unsupported {
                    "process_tree_signal_unsupported"
                } else {
                    "process_tree_signal_failed"
                },
                format!("pid {}: {error}", member.pid),
            ));
            break;
        }
        if let Err(error) = recovery_store.after_delivery(&transaction_id, member.pid) {
            delivery_error = Some(error);
            break;
        }
    }
    if let Some(mut error) = delivery_error {
        match restore_tree_members(&known, true) {
            Ok(()) => {
                if let Err(recovery_error) =
                    recovery_store.finish_recovery(&transaction_id, false, receipts)
                {
                    error = CuError::new(
                        "process_tree_recovery_failed",
                        format!(
                            "{}; durable recovery close: {}",
                            error.message, recovery_error.message
                        ),
                    );
                }
            }
            Err(restore) => {
                error = CuError::new(
                    "process_tree_recovery_failed",
                    format!("{}; rollback: {}", error.message, restore.message),
                );
            }
        }
        return Err(process_tree_error_detail(error, &ticket, &known));
    }

    if !matches!(signal, ProcessSignalKind::Stop | ProcessSignalKind::Kill)
        && let Err(error) = restore_tree_members(&known, signal != ProcessSignalKind::Terminate)
    {
        return fail_process_tree_signal(receipts, &ticket, error, true, &known);
    }

    let started = Instant::now();
    let mut checks = match verify_tree_members(&known, &members, signal) {
        Ok(checks) => checks,
        Err(error) => {
            let error = recover_failed_tree_transaction(
                &recovery_store,
                &transaction_id,
                &known,
                receipts,
                error,
            );
            return Err(process_tree_error_detail(error, &ticket, &known));
        }
    };
    while checks.iter().any(|check| check.verified == Some(false))
        && started.elapsed() < Duration::from_millis(timeout_ms)
    {
        std::thread::sleep(Duration::from_millis(10));
        checks = match verify_tree_members(&known, &members, signal) {
            Ok(checks) => checks,
            Err(error) => {
                let error = recover_failed_tree_transaction(
                    &recovery_store,
                    &transaction_id,
                    &known,
                    receipts,
                    error,
                );
                return Err(process_tree_error_detail(error, &ticket, &known));
            }
        };
    }
    let verified = if checks.iter().all(|check| check.verified == Some(true)) {
        Some(true)
    } else if checks.iter().any(|check| check.verified == Some(false)) {
        Some(false)
    } else {
        None
    };
    let success = verified != Some(false);
    let member_rows = checks
        .into_iter()
        .map(|check| {
            let member = known.get(&check.pid).expect("verified member retained");
            json!({
                "pid": check.pid,
                "depth": member.depth,
                "start_identity": member.identity,
                "was_stopped": member.was_stopped,
                "verified": check.verified,
                "state": check.state,
            })
        })
        .collect::<Vec<_>>();
    if !success {
        let error = recover_failed_tree_transaction(
            &recovery_store,
            &transaction_id,
            &known,
            receipts,
            CuError::new(
                "process_tree_signal_postcondition_failed",
                "tree signal was delivered but at least one required postcondition was not observed",
            ),
        );
        return Err(process_tree_error_detail(error, &ticket, &known));
    }
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    if let Err(error) =
        recovery_store.finish_effect(&transaction_id, elapsed_ms, verified, receipts)
    {
        return Err(process_tree_error_detail(error, &ticket, &known));
    }
    let payload = json!({
        "root_pid": root_pid,
        "root_start_identity": root_identity,
        "tree": true,
        "signal": signal.as_str(),
        "performed": true,
        "delivered": true,
        "verified": verified,
        "member_count": member_rows.len(),
        "members": member_rows,
        "timeout_ms": timeout_ms,
        "max_descendants": max_descendants,
        "receipt": ticket.json(),
        "transaction_id": transaction_id,
        "recovered_transactions": recovered_transactions,
    });
    Ok(payload)
}

fn recover_failed_tree_transaction(
    recovery_store: &RecoveryStore,
    transaction_id: &str,
    members: &BTreeMap<u32, TreeSignalMember>,
    receipts: &mut ReceiptLog,
    mut error: CuError,
) -> CuError {
    match restore_tree_members(members, true) {
        Ok(()) => {
            if let Err(recovery_error) =
                recovery_store.finish_recovery(transaction_id, false, receipts)
            {
                error = CuError::new(
                    "process_tree_recovery_failed",
                    format!(
                        "{}; durable recovery close: {}",
                        error.message, recovery_error.message
                    ),
                );
            }
        }
        Err(restore) => {
            error = CuError::new(
                "process_tree_recovery_failed",
                format!("{}; rollback: {}", error.message, restore.message),
            );
        }
    }
    error
}

fn process_tree_error_detail(
    error: CuError,
    ticket: &crate::receipt::ReceiptTicket,
    members: &BTreeMap<u32, TreeSignalMember>,
) -> CuError {
    error.with_detail(json!({
        "receipt": ticket.json(),
        "performed": true,
        "verified": false,
        "member_count": members.len(),
    }))
}

fn stable_unfrozen_tree(
    root_pid: u32,
    root_identity: &str,
    max_descendants: usize,
    known: &mut BTreeMap<u32, TreeSignalMember>,
    recovery_store: &RecoveryStore,
    transaction_id: &str,
) -> Result<Vec<u32>, CuError> {
    let mut previous = Vec::new();
    for _ in 0..6 {
        let current = open_tree_snapshot(root_pid, root_identity, max_descendants)?;
        let signature = tree_signature(&current);
        merge_tree_members(known, current, recovery_store, transaction_id)?;
        if signature == previous {
            let final_ids = signature
                .into_iter()
                .map(|(pid, _, _)| pid)
                .collect::<Vec<_>>();
            retain_final_tree_members(known, &final_ids, recovery_store, transaction_id)?;
            recovery_store.mark_stable(transaction_id, &final_ids)?;
            return Ok(final_ids);
        }
        previous = signature;
        std::thread::sleep(Duration::from_millis(30));
    }
    Err(CuError::new(
        "process_tree_unstable",
        "process tree did not produce two equal bounded snapshots in six attempts",
    ))
}

fn freeze_stable_tree(
    root_pid: u32,
    root_identity: &str,
    max_descendants: usize,
    known: &mut BTreeMap<u32, TreeSignalMember>,
    recovery_store: &RecoveryStore,
    transaction_id: &str,
) -> Result<Vec<u32>, CuError> {
    for _ in 0..6 {
        let before = open_tree_snapshot(root_pid, root_identity, max_descendants)?;
        let before_signature = tree_signature(&before);
        merge_tree_members(known, before, recovery_store, transaction_id)?;
        for (pid, _, _) in &before_signature {
            let member = known.get_mut(pid).expect("snapshot member retained");
            if member.was_stopped || member.frozen_by_us {
                continue;
            }
            if exact_member_stopped(member)? {
                return Err(CuError::new(
                    "process_tree_freeze_state_changed",
                    format!(
                        "pid {} stopped after its scheduler state was captured",
                        member.pid
                    ),
                ));
            }
            recovery_store.before_freeze(transaction_id, member.pid)?;
            {
                member.reference.set_suspended(true).map_err(|error| {
                    CuError::new(
                        "process_tree_freeze_failed",
                        format!("pid {}: {error}", member.pid),
                    )
                })?;
            }
            recovery_store.after_freeze(transaction_id, member.pid)?;
            member.frozen_by_us = true;
        }
        std::thread::sleep(Duration::from_millis(30));
        let after = open_tree_snapshot(root_pid, root_identity, max_descendants)?;
        let after_signature = tree_signature(&after);
        merge_tree_members(known, after, recovery_store, transaction_id)?;
        let all_stopped = after_signature.iter().try_fold(true, |all, (pid, _, _)| {
            exact_member_stopped(known.get(pid).expect("snapshot member retained"))
                .map(|stopped| all && stopped)
        })?;
        if before_signature == after_signature && all_stopped {
            let final_ids = after_signature
                .into_iter()
                .map(|(pid, _, _)| pid)
                .collect::<Vec<_>>();
            retain_final_tree_members(known, &final_ids, recovery_store, transaction_id)?;
            recovery_store.mark_stable(transaction_id, &final_ids)?;
            return Ok(final_ids);
        }
    }
    Err(CuError::new(
        "process_tree_freeze_unstable",
        "process tree could not be frozen into one complete stable snapshot in six attempts",
    ))
}

fn open_tree_snapshot(
    root_pid: u32,
    root_identity: &str,
    max_descendants: usize,
) -> Result<Vec<TreeSignalMember>, CuError> {
    let ids = tree_snapshot_ids(root_pid, max_descendants)?;
    let mut members = Vec::with_capacity(ids.len());
    for (pid, depth) in ids {
        let member = open_tree_signal_member(pid, depth)?;
        if pid == root_pid && member.identity != root_identity {
            return Err(CuError::new(
                "process_identity_changed",
                "process tree root identity changed during inventory",
            ));
        }
        members.push(member);
    }
    Ok(members)
}

fn tree_snapshot_ids(root_pid: u32, max_descendants: usize) -> Result<Vec<(u32, usize)>, CuError> {
    let rows = agenterm_platform::process::list()
        .map_err(|error| CuError::new("process_tree_inventory_failed", error.to_string()))?;
    if !rows.iter().any(|row| row.id == root_pid) {
        return Err(CuError::new(
            "process_tree_root_missing",
            "the exact root is absent from the complete process inventory",
        ));
    }
    let mut children = BTreeMap::<u32, Vec<u32>>::new();
    for row in rows {
        children.entry(row.parent_id).or_default().push(row.id);
    }
    for ids in children.values_mut() {
        ids.sort_unstable();
    }
    let mut seen = BTreeSet::from([root_pid]);
    let mut queue = VecDeque::from([(root_pid, 0usize)]);
    let mut ids = vec![(root_pid, 0usize)];
    while let Some((parent, depth)) = queue.pop_front() {
        for child in children.get(&parent).into_iter().flatten() {
            if seen.insert(*child) {
                if seen.len() - 1 > max_descendants {
                    return Err(CuError::new(
                        "process_tree_too_large",
                        format!("process tree exceeds --max {max_descendants} descendants"),
                    ));
                }
                ids.push((*child, depth + 1));
                queue.push_back((*child, depth + 1));
            }
        }
    }
    Ok(ids)
}

fn open_tree_signal_member(pid: u32, depth: usize) -> Result<TreeSignalMember, CuError> {
    let reference =
        agenterm_platform::process_reference::ProcessReference::open_for_termination(pid)
            .map_err(|error| CuError::new("process_reference_failed", error.to_string()))?;
    if !reference
        .is_alive()
        .map_err(|error| CuError::new("process_reference_failed", error.to_string()))?
    {
        return Err(CuError::new(
            "process_tree_member_exited",
            format!("pid {pid} exited while opening its exact process object"),
        ));
    }
    let identity = live_start_identity(pid)?;
    let was_stopped = process_stopped(pid)?;
    if live_start_identity(pid)? != identity
        || !reference
            .is_alive()
            .map_err(|error| CuError::new("process_reference_failed", error.to_string()))?
    {
        return Err(CuError::new(
            "process_identity_changed",
            format!("pid {pid} changed while binding its scheduler state"),
        ));
    }
    Ok(TreeSignalMember {
        pid,
        depth,
        identity,
        reference,
        was_stopped,
        frozen_by_us: false,
    })
}

fn tree_signature(members: &[TreeSignalMember]) -> Vec<(u32, usize, String)> {
    members
        .iter()
        .map(|member| (member.pid, member.depth, member.identity.clone()))
        .collect()
}

fn merge_tree_members(
    known: &mut BTreeMap<u32, TreeSignalMember>,
    members: Vec<TreeSignalMember>,
    recovery_store: &RecoveryStore,
    transaction_id: &str,
) -> Result<(), CuError> {
    let inputs = members
        .iter()
        .map(|member| {
            let captured = known.get(&member.pid);
            RecoveryMemberInput {
                pid: member.pid,
                depth: member.depth,
                start_identity: captured
                    .map(|existing| existing.identity.as_str())
                    .unwrap_or(&member.identity),
                was_stopped: captured
                    .map(|existing| existing.was_stopped)
                    .unwrap_or(member.was_stopped),
            }
        })
        .collect::<Vec<_>>();
    recovery_store.register_members(transaction_id, &inputs)?;
    for member in members {
        if let Some(existing) = known.get_mut(&member.pid) {
            if existing.identity != member.identity {
                return Err(CuError::new(
                    "process_identity_changed",
                    format!("pid {} was reused during tree stabilization", member.pid),
                ));
            }
            existing.depth = member.depth;
        } else {
            known.insert(member.pid, member);
        }
    }
    Ok(())
}

fn retain_final_tree_members(
    known: &mut BTreeMap<u32, TreeSignalMember>,
    final_ids: &[u32],
    recovery_store: &RecoveryStore,
    transaction_id: &str,
) -> Result<(), CuError> {
    let final_ids = final_ids.iter().copied().collect::<BTreeSet<_>>();
    let mut failures = Vec::new();
    for member in known
        .values()
        .filter(|member| !final_ids.contains(&member.pid))
    {
        if member.was_stopped {
            continue;
        }
        if !member.frozen_by_us {
            if let Err(error) = recovery_store.released_without_freeze(transaction_id, member.pid) {
                failures.push(format!(
                    "pid {} release mark: {}",
                    member.pid, error.message
                ));
            }
            continue;
        }
        match member.reference.is_alive() {
            Ok(false) => {
                if let Err(error) = recovery_store.released_after_exit(transaction_id, member.pid) {
                    failures.push(format!(
                        "pid {} exited release: {}",
                        member.pid, error.message
                    ));
                }
            }
            Ok(true) => {
                if let Err(error) = recovery_store.before_release(transaction_id, member.pid) {
                    failures.push(format!(
                        "pid {} release intent: {}",
                        member.pid, error.message
                    ));
                    continue;
                }
                if let Err(error) = member.reference.set_suspended(false) {
                    failures.push(format!("pid {} resume: {error}", member.pid));
                } else if let Err(error) = recovery_store.after_release(transaction_id, member.pid)
                {
                    failures.push(format!(
                        "pid {} release mark: {}",
                        member.pid, error.message
                    ));
                }
            }
            Err(error) => failures.push(format!("pid {} liveness: {error}", member.pid)),
        }
    }
    if !failures.is_empty() {
        return Err(CuError::new(
            "process_tree_recovery_failed",
            failures.join("; "),
        ));
    }
    known.retain(|pid, _| final_ids.contains(pid));
    Ok(())
}

fn exact_member_stopped(member: &TreeSignalMember) -> Result<bool, CuError> {
    if !member
        .reference
        .is_alive()
        .map_err(|error| CuError::new("process_reference_failed", error.to_string()))?
    {
        return Err(CuError::new(
            "process_tree_member_exited",
            format!("pid {} exited during tree stabilization", member.pid),
        ));
    }
    let identity = live_start_identity(member.pid)?;
    let stopped = process_stopped(member.pid)?;
    if identity != member.identity || live_start_identity(member.pid)? != member.identity {
        return Err(CuError::new(
            "process_identity_changed",
            format!("pid {} changed during scheduler-state read", member.pid),
        ));
    }
    Ok(stopped)
}

fn deliver_tree_signal(
    member: &TreeSignalMember,
    signal: ProcessSignalKind,
) -> std::io::Result<()> {
    match signal {
        ProcessSignalKind::Terminate => member
            .reference
            .terminate(agenterm_platform::process_control::TerminationMode::Graceful),
        ProcessSignalKind::Kill => member
            .reference
            .terminate(agenterm_platform::process_control::TerminationMode::Forceful),
        ProcessSignalKind::Stop => Ok(()),
        ProcessSignalKind::Continue => member.reference.set_suspended(false),
        ProcessSignalKind::Hangup => member
            .reference
            .send_signal(agenterm_platform::process_reference::ProcessSignal::Hangup),
        ProcessSignalKind::Interrupt => member
            .reference
            .send_signal(agenterm_platform::process_reference::ProcessSignal::Interrupt),
        ProcessSignalKind::User1 => member
            .reference
            .send_signal(agenterm_platform::process_reference::ProcessSignal::User1),
        ProcessSignalKind::User2 => member
            .reference
            .send_signal(agenterm_platform::process_reference::ProcessSignal::User2),
    }
}

fn restore_tree_members(
    members: &BTreeMap<u32, TreeSignalMember>,
    only_frozen_by_us: bool,
) -> Result<(), CuError> {
    let mut failures = Vec::new();
    for member in members
        .values()
        .rev()
        .filter(|member| !only_frozen_by_us || member.frozen_by_us)
    {
        match member.reference.is_alive() {
            Ok(false) => continue,
            Ok(true) => {}
            Err(error) => {
                failures.push(format!("pid {} liveness: {error}", member.pid));
                continue;
            }
        }
        if let Err(error) = member.reference.set_suspended(false) {
            failures.push(format!("pid {} resume: {error}", member.pid));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CuError::new(
            "process_tree_recovery_failed",
            failures.join("; "),
        ))
    }
}

fn verify_tree_members(
    known: &BTreeMap<u32, TreeSignalMember>,
    members: &[u32],
    signal: ProcessSignalKind,
) -> Result<Vec<TreeSignalCheck>, CuError> {
    members
        .iter()
        .map(|pid| {
            let member = known.get(pid).expect("stable member retained");
            let alive = member.reference.is_alive().map_err(|error| {
                CuError::new("process_reference_failed_after_signal", error.to_string())
            })?;
            let (verified, state) = match signal {
                ProcessSignalKind::Terminate | ProcessSignalKind::Kill => {
                    (Some(!alive), if alive { "live" } else { "exited" })
                }
                ProcessSignalKind::Stop | ProcessSignalKind::Continue if !alive => {
                    (Some(false), "exited")
                }
                ProcessSignalKind::Stop => (Some(exact_member_stopped(member)?), "stopped"),
                ProcessSignalKind::Continue => (Some(!exact_member_stopped(member)?), "running"),
                _ => (None, if alive { "live" } else { "exited" }),
            };
            Ok(TreeSignalCheck {
                pid: *pid,
                verified,
                state,
            })
        })
        .collect()
}

fn fail_process_tree_signal(
    receipts: &mut ReceiptLog,
    ticket: &crate::receipt::ReceiptTicket,
    error: CuError,
    performed: bool,
    members: &BTreeMap<u32, TreeSignalMember>,
) -> Result<Value, CuError> {
    receipts.complete(
        ticket,
        "process-signal-tree",
        0,
        false,
        json!({
            "performed": performed,
            "verified": false,
            "member_count": members.len(),
            "error": error_payload(&error),
        }),
    )?;
    Err(error.with_detail(json!({
        "receipt": ticket.json(),
        "performed": performed,
        "verified": false,
        "member_count": members.len(),
    })))
}

fn fail_process_state_after_effect(
    receipts: &mut ReceiptLog,
    ticket: &crate::receipt::ReceiptTicket,
    error: CuError,
) -> Result<Value, CuError> {
    receipts.complete(
        ticket,
        "process-set-state",
        0,
        false,
        json!({ "performed": true, "verified": false, "error": error_payload(&error) }),
    )?;
    Err(error.with_detail(json!({
        "receipt": ticket.json(),
        "performed": true,
        "verified": false,
    })))
}

fn process_stopped(pid: u32) -> Result<bool, CuError> {
    agenterm_platform::process_metrics::is_stopped(pid).map_err(|error| {
        use agenterm_platform::process_metrics::ProcessMetricsErrorKind as Kind;
        let code = match error.kind() {
            Kind::Unsupported => "process_state_unsupported",
            Kind::NotFound => "process_not_found",
            _ => "process_state_observation_failed",
        };
        CuError::new(code, error.to_string())
    })
}

fn process_watch_snapshot(
    pid: Option<u32>,
    parent: Option<u32>,
    name: Option<&str>,
    max_processes: usize,
) -> Result<ProcessWatchSnapshot, CuError> {
    let name = name.map(str::to_ascii_lowercase);
    let rows = agenterm_platform::process::list().map_err(|error| {
        CuError::new("process_inventory_failed", error.to_string()).with_detail(json!({
            "kind": format!("{:?}", error.kind()),
        }))
    })?;
    let mut matched = rows
        .into_iter()
        .filter(|row| {
            pid.is_none_or(|wanted| row.id == wanted)
                && parent.is_none_or(|wanted| row.parent_id == wanted)
                && name
                    .as_ref()
                    .is_none_or(|wanted| row.executable_name.to_ascii_lowercase().contains(wanted))
        })
        .collect::<Vec<_>>();
    matched.sort_by_key(|row| row.id);
    if matched.len() > max_processes {
        return Err(CuError::new(
            "process_watch_inventory_too_large",
            "matched process inventory exceeds --max-processes",
        )
        .with_detail(json!({
            "matched": matched.len(),
            "max_processes": max_processes,
        })));
    }

    let mut snapshot = BTreeMap::new();
    let mut excluded_unidentified = 0usize;
    for row in matched {
        let start_identity = match agenterm_platform::process_observation::observe(row.id) {
            agenterm_platform::process_observation::ProcessObservation::Live {
                start_identity: Some(identity),
            } => identity,
            agenterm_platform::process_observation::ProcessObservation::Dead { .. } => continue,
            agenterm_platform::process_observation::ProcessObservation::Live {
                start_identity: None,
            } => {
                if pid.is_some() {
                    return Err(CuError::new(
                        "process_identity_unavailable",
                        "the exact watched process is live but has no start identity",
                    )
                    .with_detail(json!({ "pid": row.id })));
                }
                excluded_unidentified += 1;
                continue;
            }
            agenterm_platform::process_observation::ProcessObservation::Unknown { reason } => {
                if pid.is_some() {
                    return Err(CuError::new("process_identity_unknown", reason)
                        .with_detail(json!({ "pid": row.id })));
                }
                excluded_unidentified += 1;
                continue;
            }
            _ => {
                if pid.is_some() {
                    return Err(CuError::new(
                        "process_identity_unknown",
                        "process observation variant is unsupported",
                    )
                    .with_detail(json!({ "pid": row.id })));
                }
                excluded_unidentified += 1;
                continue;
            }
        };
        let watched = WatchedProcess {
            pid: row.id,
            parent_pid: row.parent_id,
            executable_name: row.executable_name,
            start_identity,
        };
        snapshot.insert(watched.key(), watched);
    }
    Ok(ProcessWatchSnapshot {
        processes: snapshot,
        excluded_unidentified,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn process_watch_payload(
    pid: Option<u32>,
    parent: Option<u32>,
    name: Option<&str>,
    all: bool,
    duration_ms: u64,
    interval_ms: Option<u64>,
    max_events: Option<usize>,
    max_processes: Option<usize>,
) -> Result<Value, CuError> {
    let interval_ms = interval_ms.unwrap_or(DEFAULT_PROCESS_WATCH_INTERVAL_MS);
    let max_events = max_events.unwrap_or(DEFAULT_PROCESS_WATCH_MAX_EVENTS);
    let max_processes = max_processes.unwrap_or(DEFAULT_PROCESS_WATCH_MAX_PROCESSES);
    if (pid.is_none() && parent.is_none() && name.is_none() && !all)
        || pid == Some(0)
        || parent == Some(0)
        || name.is_some_and(|value| value.trim().is_empty())
        || !(1..=MAX_USAGE_WATCH_MS).contains(&duration_ms)
        || !(1..=MAX_USAGE_INTERVAL_MS).contains(&interval_ms)
        || !(1..=MAX_USAGE_SAMPLES).contains(&max_events)
        || !(1..=MAX_RESULTS).contains(&max_processes)
    {
        return Err(CuError::new(
            "invalid_input",
            "process-watch requires one selector, duration-ms in 1..=86400000, interval-ms in 1..=60000, max-events in 1..=4096 and max-processes in 1..=5000",
        ));
    }

    let started = Instant::now();
    let deadline = started + Duration::from_millis(duration_ms);
    let initial = process_watch_snapshot(pid, parent, name, max_processes)?;
    let mut previous = initial.processes;
    let mut excluded_unidentified = initial.excluded_unidentified;
    let baseline = previous
        .values()
        .map(WatchedProcess::json)
        .collect::<Vec<_>>();
    let mut events = Vec::with_capacity(max_events.min(256));
    let mut truncated = false;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(Duration::from_millis(interval_ms).min(remaining));
        let next = process_watch_snapshot(pid, parent, name, max_processes)?;
        excluded_unidentified = excluded_unidentified.max(next.excluded_unidentified);
        let current = next.processes;
        let t_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        for (kind, row) in previous
            .iter()
            .filter(|(key, _)| !current.contains_key(*key))
            .map(|(_, row)| ("exited", row))
            .chain(
                current
                    .iter()
                    .filter(|(key, _)| !previous.contains_key(*key))
                    .map(|(_, row)| ("started", row)),
            )
        {
            if events.len() == max_events {
                truncated = true;
                break;
            }
            events.push(json!({ "t_ms": t_ms, "kind": kind, "process": row.json() }));
        }
        previous = current;
        if truncated || events.len() == max_events {
            truncated = Instant::now() < deadline;
            break;
        }
    }

    Ok(json!({
        "mode": "bounded-diff",
        "selector": { "pid": pid, "parent": parent, "name": name, "all": all },
        "duration_ms": duration_ms,
        "interval_ms": interval_ms,
        "max_events": max_events,
        "max_processes": max_processes,
        "baseline": baseline,
        "baseline_count": baseline.len(),
        "excluded_unidentified": excluded_unidentified,
        "coverage_complete": excluded_unidentified == 0,
        "events": events,
        "emitted": events.len(),
        "completed": !truncated,
        "truncated": truncated,
        "verified": true,
    }))
}

#[derive(Default)]
pub(super) struct ProcessInventoryOptions<'a> {
    pub pid: Option<u32>,
    pub parent: Option<u32>,
    pub name: Option<&'a str>,
    pub app: Option<&'a str>,
    pub command: Option<&'a str>,
    pub cpu_above_percent: Option<f64>,
    pub memory_above_mb: Option<f64>,
    pub sort: Option<&'a str>,
    pub sample_ms: Option<u64>,
    pub max_visited: Option<usize>,
    pub offset: Option<usize>,
    pub max: Option<usize>,
}

struct RichProcessRow {
    base: agenterm_platform::contract::process::ProcessInfo,
    command_sha256: Option<String>,
    command_bytes: Option<usize>,
    cpu_percent: Option<f64>,
    resident_bytes: Option<u64>,
}

fn lower_contains(value: &str, needle: Option<&str>) -> bool {
    needle.is_none_or(|needle| {
        value
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    })
}

fn process_row_json(
    row: &agenterm_platform::contract::process::ProcessInfo,
    depth: usize,
    child_count: usize,
) -> Value {
    json!({
        "pid": row.id,
        "parent_pid": row.parent_id,
        "executable_name": row.executable_name,
        "depth": depth,
        "child_count": child_count,
    })
}

fn optional_inspection(result: Result<Value, CuError>) -> Value {
    match result {
        Ok(value) => json!({ "status": "available", "data": value }),
        Err(error) => json!({
            "status": "unavailable",
            "error": {
                "code": error.code,
                "message": error.message,
                "detail": error.detail,
            }
        }),
    }
}

pub(super) fn process_tree_payload(
    pid: u32,
    max_depth: usize,
    max_descendants: usize,
    files: bool,
    ports: bool,
    max_visited: Option<usize>,
) -> Result<Value, CuError> {
    let max_visited = max_visited.unwrap_or(10_000);
    if pid == 0
        || max_depth > 64
        || !(1..=MAX_RESULTS).contains(&max_descendants)
        || !(1..=10_000).contains(&max_visited)
    {
        return Err(CuError::new(
            "invalid_input",
            "ps detail mode requires pid > 0, depth in 0..=64, max in 1..=5000 and max-visited in 1..=10000",
        ));
    }
    let inventory = agenterm_platform::process::list().map_err(|error| {
        CuError::new("process_inventory_failed", error.to_string()).with_detail(json!({
            "kind": format!("{:?}", error.kind()),
        }))
    })?;
    if inventory.len() > max_visited {
        return Err(CuError::new(
            "process_inventory_too_large",
            format!(
                "process inventory has {} rows, above --max-visited={max_visited}; raise the explicit bound",
                inventory.len()
            ),
        ));
    }
    let by_pid = inventory
        .iter()
        .cloned()
        .map(|row| (row.id, row))
        .collect::<BTreeMap<_, _>>();
    let root = by_pid
        .get(&pid)
        .ok_or_else(|| CuError::new("process_not_found", format!("process pid={pid} is absent")))?;
    let mut children = BTreeMap::<u32, Vec<u32>>::new();
    for row in &inventory {
        children.entry(row.parent_id).or_default().push(row.id);
    }
    for ids in children.values_mut() {
        ids.sort_unstable();
    }

    let mut ancestors = Vec::new();
    let mut ancestor_seen = BTreeSet::from([pid]);
    let mut parent = root.parent_id;
    while parent != 0 && ancestor_seen.insert(parent) {
        let Some(row) = by_pid.get(&parent) else {
            break;
        };
        ancestors.push(process_row_json(
            row,
            ancestors.len() + 1,
            children.get(&row.id).map_or(0, Vec::len),
        ));
        parent = row.parent_id;
    }
    ancestors.reverse();

    let mut descendants = Vec::new();
    let mut queue = VecDeque::new();
    for child in children.get(&pid).into_iter().flatten() {
        queue.push_back((*child, 1_usize));
    }
    let mut seen = BTreeSet::from([pid]);
    let mut truncated = false;
    while let Some((next, depth)) = queue.pop_front() {
        if !seen.insert(next) {
            continue;
        }
        if depth > max_depth {
            truncated = true;
            continue;
        }
        if descendants.len() == max_descendants {
            truncated = true;
            break;
        }
        let Some(row) = by_pid.get(&next) else {
            continue;
        };
        descendants.push(process_row_json(
            row,
            depth,
            children.get(&next).map_or(0, Vec::len),
        ));
        for child in children.get(&next).into_iter().flatten() {
            queue.push_back((*child, depth + 1));
        }
    }

    let inspection_limit = max_descendants.min(DEFAULT_INSPECTION_LIMIT);
    Ok(json!({
        "mode": "tree-detail",
        "root": process_row_json(root, 0, children.get(&pid).map_or(0, Vec::len)),
        "ancestors": ancestors,
        "descendants": descendants,
        "visited": inventory.len(),
        "max_visited": max_visited,
        "max_depth": max_depth,
        "max_descendants": max_descendants,
        "truncated": truncated,
        "files": files.then(|| optional_inspection(process_fds_payload(
            pid, None, None, None, Some(inspection_limit), Some(max_visited),
        ))),
        "ports": ports.then(|| optional_inspection(process_sockets_payload(
            pid, None, None, None, None, None, Some(inspection_limit), Some(max_visited),
        ))),
        "verified": true,
    }))
}

pub(super) fn process_list_payload(options: ProcessInventoryOptions<'_>) -> Result<Value, CuError> {
    let offset = options.offset.unwrap_or(0);
    let max = options.max.unwrap_or(DEFAULT_MAX);
    if max == 0 || max > MAX_RESULTS {
        return Err(CuError::new(
            "invalid_input",
            format!("ps --max must be in 1..={MAX_RESULTS}"),
        ));
    }

    let max_visited = options.max_visited.unwrap_or(1_000);
    if !(1..=10_000).contains(&max_visited) {
        return Err(CuError::new(
            "invalid_input",
            "ps --max-visited must be in 1..=10000",
        ));
    }
    let sample_ms = options.sample_ms.unwrap_or(100);
    if !(10..=10_000).contains(&sample_ms) {
        return Err(CuError::new(
            "invalid_input",
            "ps --sample-ms must be in 10..=10000",
        ));
    }
    let sort = options.sort.unwrap_or("pid");
    if !matches!(sort, "pid" | "cpu" | "mem" | "memory") {
        return Err(CuError::new(
            "invalid_input",
            "ps --sort must be pid|cpu|mem|memory",
        ));
    }

    let mut inventory = agenterm_platform::process::list().map_err(|error| {
        CuError::new("process_inventory_failed", error.to_string()).with_detail(json!({
            "kind": format!("{:?}", error.kind()),
        }))
    })?;
    inventory.sort_by_key(|row| row.id);
    let visited = inventory.len();
    inventory.retain(|row| {
        options.pid.is_none_or(|wanted| row.id == wanted)
            && options.parent.is_none_or(|wanted| row.parent_id == wanted)
            && lower_contains(&row.executable_name, options.name)
            && options
                .app
                .is_none_or(|wanted| row.executable_name.eq_ignore_ascii_case(wanted))
    });
    let prefiltered = inventory.len();
    let truncated_scan = inventory.len() > max_visited;
    inventory.truncate(max_visited);

    let needs_metrics = options.cpu_above_percent.is_some()
        || options.memory_above_mb.is_some()
        || matches!(sort, "cpu" | "mem" | "memory");
    let needs_cpu = options.cpu_above_percent.is_some() || sort == "cpu";
    let first_cpu = if needs_cpu {
        inventory
            .iter()
            .filter_map(|row| {
                let identity = live_start_identity(row.id).ok()?;
                agenterm_platform::process_metrics::metrics(row.id)
                    .ok()
                    .map(|sample| (row.id, (identity, sample.cpu_time.as_nanos())))
            })
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };
    if needs_cpu {
        thread::sleep(Duration::from_millis(sample_ms));
    }

    let mut detail_errors = 0_usize;
    let mut rows = inventory
        .into_iter()
        .filter_map(|base| {
            let needs_detail = options.command.is_some() || needs_metrics;
            let row_identity = if needs_detail {
                if needs_cpu {
                    match first_cpu.get(&base.id) {
                        Some((identity, _)) => Some(identity.clone()),
                        None => {
                            detail_errors += 1;
                            return None;
                        }
                    }
                } else {
                    match live_start_identity(base.id) {
                        Ok(identity) => Some(identity),
                        Err(_) => {
                            detail_errors += 1;
                            return None;
                        }
                    }
                }
            } else {
                None
            };
            let (command_sha256, command_bytes) = if options.command.is_some() {
                match agenterm_platform::process::command_line(base.id) {
                    Ok(value) if lower_contains(&value, options.command) => (
                        Some(super::clipboard::clipboard_sha256_hex(value.as_bytes())),
                        Some(value.len()),
                    ),
                    Ok(_) => return None,
                    Err(_) => {
                        detail_errors += 1;
                        return None;
                    }
                }
            } else {
                (None, None)
            };
            let (cpu_percent, resident_bytes) = if needs_metrics {
                match agenterm_platform::process_metrics::metrics(base.id) {
                    Ok(sample) => {
                        let cpu = first_cpu.get(&base.id).and_then(|(identity, before)| {
                            debug_assert_eq!(Some(identity), row_identity.as_ref());
                            sample
                                .cpu_time
                                .as_nanos()
                                .checked_sub(*before)
                                .map(|delta| {
                                    delta as f64 / (sample_ms as f64 * 1_000_000.0) * 100.0
                                })
                        });
                        if needs_cpu && cpu.is_none() {
                            detail_errors += 1;
                            return None;
                        }
                        (cpu, Some(sample.resident_bytes))
                    }
                    Err(_) => {
                        detail_errors += 1;
                        return None;
                    }
                }
            } else {
                (None, None)
            };
            if needs_detail && live_start_identity(base.id).ok().as_ref() != row_identity.as_ref() {
                detail_errors += 1;
                return None;
            }
            if options
                .cpu_above_percent
                .is_some_and(|threshold| cpu_percent.is_none_or(|value| value <= threshold))
                || options.memory_above_mb.is_some_and(|threshold| {
                    resident_bytes.is_none_or(|value| value as f64 <= threshold * 1024.0 * 1024.0)
                })
            {
                return None;
            }
            Some(RichProcessRow {
                base,
                command_sha256,
                command_bytes,
                cpu_percent,
                resident_bytes,
            })
        })
        .collect::<Vec<_>>();
    match sort {
        "cpu" => rows.sort_by(|left, right| {
            right
                .cpu_percent
                .partial_cmp(&left.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.base.id.cmp(&right.base.id))
        }),
        "mem" | "memory" => rows.sort_by(|left, right| {
            right
                .resident_bytes
                .cmp(&left.resident_bytes)
                .then_with(|| left.base.id.cmp(&right.base.id))
        }),
        _ => rows.sort_by_key(|row| row.base.id),
    }
    let matched = rows.len();
    let processes = rows
        .into_iter()
        .skip(offset)
        .take(max)
        .map(|row| {
            json!({
                "pid": row.base.id,
                "parent_pid": row.base.parent_id,
                "executable_name": row.base.executable_name,
                "command_sha256": row.command_sha256,
                "command_bytes": row.command_bytes,
                "cpu_percent": row.cpu_percent,
                "resident_bytes": row.resident_bytes.map(|value| value.to_string()),
            })
        })
        .collect::<Vec<_>>();
    let returned = processes.len();
    Ok(json!({
        "processes": processes,
        "visited": visited,
        "prefiltered": prefiltered,
        "matched": matched,
        "returned": returned,
        "offset": offset,
        "max_visited": max_visited,
        "sample_ms": needs_cpu.then_some(sample_ms),
        "detail_errors": detail_errors,
        "truncated_scan": truncated_scan,
        "coverage_complete": !truncated_scan && detail_errors == 0,
        "truncated": truncated_scan || offset.saturating_add(returned) < matched,
        "next_offset": (offset.saturating_add(returned) < matched).then_some(offset.saturating_add(returned)),
        "verified": true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn exact_process_signal_stops_resumes_and_terminates_one_owned_child() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn signal fixture");
        let identity = live_start_identity(child.id()).expect("fixture identity");
        let root = std::fs::canonicalize(std::env::temp_dir())
            .expect("temporary root")
            .join(format!("agenterm-cu-signal-{}", child.id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();

        let stopped = process_signal_payload(
            child.id(),
            Some(&identity),
            ProcessSignalKind::Stop,
            ProcessSignalOptions {
                timeout_ms: 5_000,
                force: false,
                tree: false,
                max_descendants: 500,
            },
            &mut receipts,
        )
        .expect("stop exact child");
        assert_eq!(stopped["state"], "stopped");
        assert_eq!(stopped["verified"], true);

        let running = process_signal_payload(
            child.id(),
            Some(&identity),
            ProcessSignalKind::Continue,
            ProcessSignalOptions {
                timeout_ms: 5_000,
                force: false,
                tree: false,
                max_descendants: 500,
            },
            &mut receipts,
        )
        .expect("resume exact child");
        assert_eq!(running["state"], "running");
        assert_eq!(running["verified"], true);

        let killed = process_signal_payload(
            child.id(),
            Some(&identity),
            ProcessSignalKind::Kill,
            ProcessSignalOptions {
                timeout_ms: 5_000,
                force: true,
                tree: false,
                max_descendants: 500,
            },
            &mut receipts,
        )
        .expect("kill exact child");
        assert_eq!(killed["state"], "exited");
        assert_eq!(killed["verified"], true);
        child.wait().expect("reap signal fixture");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn exact_tree_signal_freezes_every_member_and_preserves_the_bounded_snapshot() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "trap '' USR1; sleep 30 & sleep 30 & wait"])
            .spawn()
            .expect("spawn tree fixture");
        let identity = live_start_identity(child.id()).expect("tree root identity");
        for _ in 0..100 {
            let rows = tree_snapshot_ids(child.id(), 16).expect("tree inventory");
            if rows.len() >= 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let root = std::fs::canonicalize(std::env::temp_dir())
            .expect("temporary root")
            .join(format!("agenterm-cu-tree-signal-{}", child.id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();

        let stopped = process_signal_payload(
            child.id(),
            Some(&identity),
            ProcessSignalKind::Stop,
            ProcessSignalOptions {
                timeout_ms: 5_000,
                force: false,
                tree: true,
                max_descendants: 16,
            },
            &mut receipts,
        )
        .expect("stop exact tree");
        assert_eq!(stopped["verified"], true);
        assert!(stopped["member_count"].as_u64().unwrap() >= 3);
        assert!(
            stopped["members"]
                .as_array()
                .unwrap()
                .iter()
                .all(|member| member["state"] == "stopped")
        );

        let continued = process_signal_payload(
            child.id(),
            Some(&identity),
            ProcessSignalKind::Continue,
            ProcessSignalOptions {
                timeout_ms: 5_000,
                force: false,
                tree: true,
                max_descendants: 16,
            },
            &mut receipts,
        )
        .expect("resume exact tree");
        assert_eq!(continued["verified"], true);

        let descendant_pid = tree_snapshot_ids(child.id(), 16)
            .unwrap()
            .into_iter()
            .find_map(|(pid, depth)| (depth > 0).then_some(pid))
            .expect("tree descendant");
        let pre_stopped =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(
                descendant_pid,
            )
            .expect("retain pre-stopped descendant");
        pre_stopped
            .set_suspended(true)
            .expect("pre-stop descendant");
        assert!(process_stopped(descendant_pid).unwrap());
        let generic = process_signal_payload(
            child.id(),
            Some(&identity),
            ProcessSignalKind::User1,
            ProcessSignalOptions {
                timeout_ms: 5_000,
                force: false,
                tree: true,
                max_descendants: 16,
            },
            &mut receipts,
        )
        .expect("deliver generic tree signal");
        assert!(generic["verified"].is_null());
        assert!(
            process_stopped(descendant_pid).unwrap(),
            "a member stopped before the transaction must remain stopped"
        );
        pre_stopped
            .set_suspended(false)
            .expect("resume fixture descendant");

        let killed = process_signal_payload(
            child.id(),
            Some(&identity),
            ProcessSignalKind::Kill,
            ProcessSignalOptions {
                timeout_ms: 5_000,
                force: true,
                tree: true,
                max_descendants: 16,
            },
            &mut receipts,
        )
        .expect("kill exact tree");
        assert_eq!(killed["verified"], true);
        child.wait().expect("reap tree root");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_process_is_visible_by_exact_pid() {
        let pid = std::process::id();
        let value = process_list_payload(ProcessInventoryOptions {
            pid: Some(pid),
            max: Some(10),
            ..ProcessInventoryOptions::default()
        })
        .expect("list");
        assert_eq!(value["matched"], 1);
        assert_eq!(value["returned"], 1);
        assert_eq!(value["processes"][0]["pid"], pid);
    }

    #[test]
    fn rich_inventory_filters_without_returning_command_plaintext() {
        let pid = std::process::id();
        let value = process_list_payload(ProcessInventoryOptions {
            pid: Some(pid),
            command: Some("agenterm"),
            memory_above_mb: Some(0.0),
            sort: Some("memory"),
            max_visited: Some(10_000),
            max: Some(4),
            ..ProcessInventoryOptions::default()
        })
        .expect("rich list");
        assert_eq!(value["matched"], 1);
        assert_eq!(value["coverage_complete"], true);
        assert_eq!(value["processes"][0]["pid"], pid);
        assert_eq!(
            value["processes"][0]["command_sha256"]
                .as_str()
                .map(str::len),
            Some(64)
        );
        assert!(value["processes"][0].get("command").is_none());
        assert!(value["processes"][0]["resident_bytes"].is_string());
    }

    #[test]
    fn pid_detail_tree_is_bounded_and_cycle_safe() {
        let pid = std::process::id();
        let value =
            process_tree_payload(pid, 1, 16, false, false, Some(10_000)).expect("process tree");
        assert_eq!(value["mode"], "tree-detail");
        assert_eq!(value["root"]["pid"], pid);
        assert_eq!(value["max_depth"], 1);
        assert_eq!(value["max_descendants"], 16);
        assert_eq!(value["verified"], true);
    }

    #[test]
    fn current_process_state_is_live_and_identity_bound() {
        let pid = std::process::id();
        let value = process_state_payload(pid).expect("state");
        assert_eq!(value["pid"], pid);
        assert_eq!(value["state"], "live");
        assert_eq!(value["verified"], true);
        assert!(
            value["start_identity"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(value["reason"].is_null());
    }

    #[test]
    fn process_state_rejects_pid_zero() {
        let error = process_state_payload(0).expect_err("zero");
        assert_eq!(error.code, "invalid_input");
    }

    #[test]
    fn current_process_argv_is_identity_bound_hashed_and_values_are_opt_in() {
        let pid = std::process::id();
        let hidden = process_argv_payload(pid, false, None, Some(1)).expect("hidden argv");
        assert_eq!(hidden["pid"], pid);
        assert_eq!(hidden["verified"], true);
        assert_eq!(hidden["values_included"], false);
        assert_eq!(hidden["returned"], 1);
        assert!(hidden["arguments"][0].get("value").is_none());
        assert!(hidden["arguments"][0]["byte_length"].as_str().is_some());
        assert_eq!(
            hidden["arguments"][0]["sha256"].as_str().map(str::len),
            Some(64)
        );

        let visible = process_argv_payload(pid, true, Some(0), Some(1)).expect("visible argv");
        assert_eq!(visible["values_included"], true);
        assert!(visible["arguments"][0]["value"].as_str().is_some());
        assert!(
            visible["executable"]
                .as_str()
                .is_some_and(|path| !path.is_empty())
        );
        assert!(visible["start_identity"].as_str().is_some());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn current_process_cwd_is_identity_bound_and_exact() {
        let payload = process_cwd_payload(std::process::id()).expect("current cwd");
        let expected = std::env::current_dir().expect("std cwd");
        assert_eq!(
            payload["path"],
            expected.to_str().expect("test cwd is UTF-8")
        );
        assert_eq!(payload["verified"], true);
        assert_eq!(
            payload["path_byte_length"],
            expected.as_os_str().len().to_string()
        );
        assert!(payload["start_identity"].as_str().is_some());
    }

    #[test]
    fn process_cwd_rejects_zero_pid() {
        assert_eq!(
            process_cwd_payload(0).expect_err("zero pid").code,
            "invalid_input"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn current_process_environment_is_identity_bound_and_values_are_opt_in() {
        let pid = std::process::id();
        let hidden = process_environment_payload(pid, None, false, None, Some(8))
            .expect("hidden environment");
        assert_eq!(hidden["pid"], pid);
        assert_eq!(hidden["semantics"], "exec-initial");
        assert_eq!(hidden["values_included"], false);
        assert_eq!(hidden["verified"], true);
        assert!(hidden["source_bytes"].as_str().is_some());
        assert!(hidden["start_identity"].as_str().is_some());
        for row in hidden["entries"].as_array().expect("entry array") {
            assert!(row.get("value").is_none());
            if row["has_value"] == true {
                assert_eq!(row["value_sha256"].as_str().map(str::len), Some(64));
            }
        }
    }

    #[test]
    fn environment_rows_preserve_empty_duplicate_and_non_utf8_bytes() {
        let empty = environment_row(0, b"EMPTY", Some(b""), true);
        assert_eq!(empty["name"], "EMPTY");
        assert_eq!(empty["value"], "");
        assert_eq!(empty["has_value"], true);

        let raw = environment_row(1, b"NON_UTF8_\xff", Some(b"\xfe"), true);
        assert_eq!(raw["name"], Value::Null);
        assert_eq!(raw["name_encoding"], "hex");
        assert_eq!(raw["name_hex"], "4e4f4e5f555446385fff");
        assert_eq!(raw["value"], Value::Null);
        assert_eq!(raw["value_hex"], "fe");

        let malformed = environment_row(2, b"NO_EQUALS", None, false);
        assert_eq!(malformed["has_value"], false);
        assert!(malformed.get("value_sha256").is_none());
    }

    #[test]
    fn process_environment_rejects_unbounded_inputs_before_native_read() {
        assert_eq!(
            process_environment_payload(0, None, false, None, None)
                .expect_err("zero pid")
                .code,
            "invalid_input"
        );
        assert_eq!(
            process_environment_payload(
                std::process::id(),
                None,
                false,
                None,
                Some(MAX_ENVIRONMENT_LIMIT + 1),
            )
            .expect_err("oversized page")
            .code,
            "invalid_input"
        );
        assert_eq!(
            process_environment_payload(std::process::id(), Some("bad\n"), false, None, None,)
                .expect_err("control byte")
                .code,
            "invalid_input"
        );
    }

    #[test]
    fn process_argv_rejects_zero_pid_and_unbounded_page() {
        assert_eq!(
            process_argv_payload(0, false, None, None)
                .expect_err("zero pid")
                .code,
            "invalid_input"
        );
        assert_eq!(
            process_argv_payload(std::process::id(), false, None, Some(MAX_ARGV_LIMIT + 1))
                .expect_err("oversized page")
                .code,
            "invalid_input"
        );
    }

    #[test]
    fn current_process_usage_is_identity_bound_and_uses_lossless_counters() {
        let pid = std::process::id();
        let value = process_usage_payload(pid).expect("usage");
        assert_eq!(value["pid"], pid);
        assert_eq!(value["verified"], true);
        for path in ["cpu_time_ns", "resident_bytes"] {
            assert!(value[path].as_str().is_some_and(|value| !value.is_empty()));
        }
        assert!(value["page_faults"]["total"].as_str().is_some());
    }

    #[test]
    fn process_usage_watch_is_identity_bound_and_stops_at_its_sample_budget() {
        let pid = std::process::id();
        let value = process_usage_watch_payload(pid, 60_000, Some(10), Some(1)).expect("watch");
        assert_eq!(value["pid"], pid);
        assert_eq!(value["mode"], "bounded-series");
        assert_eq!(value["emitted"], 1);
        assert_eq!(value["completed"], false);
        assert_eq!(value["truncated"], true);
        assert_eq!(value["verified"], true);
        assert!(value["start_identity"].as_str().is_some());
        assert!(value["samples"][0].get("start_identity").is_none());
        assert!(value["samples"][0]["t_ms"].as_u64().is_some());
    }

    #[test]
    fn process_usage_watch_distinguishes_completed_duration_from_truncation() {
        let value = process_usage_watch_payload(std::process::id(), 1, Some(1), Some(10))
            .expect("completed watch");
        assert_eq!(value["completed"], true);
        assert_eq!(value["truncated"], false);
        assert!(value["emitted"].as_u64().is_some_and(|count| count >= 1));
    }

    #[test]
    fn process_usage_watch_rejects_unbounded_remote_wire_values() {
        let error = process_usage_watch_payload(std::process::id(), 0, Some(1), Some(1))
            .expect_err("zero duration");
        assert_eq!(error.code, "invalid_input");
        let error = process_usage_watch_payload(
            std::process::id(),
            1,
            Some(MAX_USAGE_INTERVAL_MS + 1),
            Some(1),
        )
        .expect_err("large interval");
        assert_eq!(error.code, "invalid_input");
    }

    #[test]
    fn process_wait_times_out_on_the_same_live_process_object() {
        let pid = std::process::id();
        let identity = live_start_identity(pid).expect("identity");
        let value = process_wait_payload(pid, &identity, 1).expect("bounded wait");
        assert_eq!(value["pid"], pid);
        assert_eq!(value["start_identity"], identity);
        assert_eq!(value["state"], "timeout");
        assert_eq!(value["completed"], false);
        assert_eq!(value["verified"], true);
        assert_eq!(value["mechanism"], "native-process-reference");
    }

    #[test]
    fn process_wait_refuses_a_recycled_or_invented_identity_before_waiting() {
        let error = process_wait_payload(std::process::id(), "not-this-process", 1)
            .expect_err("identity mismatch");
        assert_eq!(error.code, "process_identity_changed");
    }

    #[test]
    fn process_watch_returns_an_identity_bound_bounded_baseline() {
        let pid = std::process::id();
        let value =
            process_watch_payload(Some(pid), None, None, false, 1, Some(1), Some(4), Some(4))
                .expect("watch");
        assert_eq!(value["mode"], "bounded-diff");
        assert_eq!(value["baseline_count"], 1);
        assert_eq!(value["baseline"][0]["pid"], pid);
        assert!(value["baseline"][0]["start_identity"].as_str().is_some());
        assert_eq!(value["verified"], true);
        assert_eq!(value["coverage_complete"], true);
        assert_eq!(value["completed"], true);
        assert_eq!(value["truncated"], false);
    }

    #[test]
    fn process_watch_rejects_missing_or_unbounded_shapes() {
        let error = process_watch_payload(None, None, None, false, 1, Some(1), Some(1), Some(1))
            .expect_err("missing selector");
        assert_eq!(error.code, "invalid_input");
        let error = process_watch_payload(
            None,
            None,
            None,
            true,
            1,
            Some(1),
            Some(1),
            Some(MAX_RESULTS + 1),
        )
        .expect_err("oversized inventory");
        assert_eq!(error.code, "invalid_input");
    }

    #[test]
    fn result_budget_is_closed_before_inventory() {
        let error = process_list_payload(ProcessInventoryOptions {
            max: Some(0),
            ..ProcessInventoryOptions::default()
        })
        .expect_err("zero");
        assert_eq!(error.code, "invalid_input");
        let error = process_list_payload(ProcessInventoryOptions {
            max: Some(MAX_RESULTS + 1),
            ..ProcessInventoryOptions::default()
        })
        .expect_err("too large");
        assert_eq!(error.code, "invalid_input");
    }
}
