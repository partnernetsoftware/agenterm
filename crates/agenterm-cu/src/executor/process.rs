//! Cross-platform process observation through `agenterm-platform`.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{CuError, command::ProcessKillMode, receipt::ReceiptLog};

use super::error_payload;

const DEFAULT_MAX: usize = 200;
const MAX_RESULTS: usize = 5_000;
const DEFAULT_ARGV_LIMIT: usize = 100;
const MAX_ARGV_LIMIT: usize = 4_096;
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

pub(super) fn process_list_payload(
    pid: Option<u32>,
    parent: Option<u32>,
    name: Option<&str>,
    offset: Option<usize>,
    max: Option<usize>,
) -> Result<Value, CuError> {
    let offset = offset.unwrap_or(0);
    let max = max.unwrap_or(DEFAULT_MAX);
    if max == 0 || max > MAX_RESULTS {
        return Err(CuError::new(
            "invalid_input",
            format!("ps --max must be in 1..={MAX_RESULTS}"),
        ));
    }

    let mut rows = agenterm_platform::process::list().map_err(|error| {
        CuError::new("process_inventory_failed", error.to_string()).with_detail(json!({
            "kind": format!("{:?}", error.kind()),
        }))
    })?;
    let visited = rows.len();
    let name = name.map(str::to_ascii_lowercase);
    rows.retain(|row| {
        pid.is_none_or(|wanted| row.id == wanted)
            && parent.is_none_or(|wanted| row.parent_id == wanted)
            && name
                .as_ref()
                .is_none_or(|wanted| row.executable_name.to_ascii_lowercase().contains(wanted))
    });
    rows.sort_by_key(|row| row.id);
    let matched = rows.len();
    let processes = rows
        .into_iter()
        .skip(offset)
        .take(max)
        .map(|row| {
            json!({
                "pid": row.id,
                "parent_pid": row.parent_id,
                "executable_name": row.executable_name,
            })
        })
        .collect::<Vec<_>>();
    let returned = processes.len();
    Ok(json!({
        "processes": processes,
        "visited": visited,
        "matched": matched,
        "returned": returned,
        "offset": offset,
        "truncated": offset.saturating_add(returned) < matched,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_visible_by_exact_pid() {
        let pid = std::process::id();
        let value = process_list_payload(Some(pid), None, None, None, Some(10)).expect("list");
        assert_eq!(value["matched"], 1);
        assert_eq!(value["returned"], 1);
        assert_eq!(value["processes"][0]["pid"], pid);
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
        let error = process_list_payload(None, None, None, None, Some(0)).expect_err("zero");
        assert_eq!(error.code, "invalid_input");
        let error = process_list_payload(None, None, None, None, Some(MAX_RESULTS + 1))
            .expect_err("too large");
        assert_eq!(error.code, "invalid_input");
    }
}
