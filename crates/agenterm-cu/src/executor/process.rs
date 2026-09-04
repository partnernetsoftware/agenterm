//! Cross-platform process observation through `agenterm-platform`.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::CuError;

const DEFAULT_MAX: usize = 200;
const MAX_RESULTS: usize = 5_000;

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
    fn result_budget_is_closed_before_inventory() {
        let error = process_list_payload(None, None, None, None, Some(0)).expect_err("zero");
        assert_eq!(error.code, "invalid_input");
        let error = process_list_payload(None, None, None, None, Some(MAX_RESULTS + 1))
            .expect_err("too large");
        assert_eq!(error.code, "invalid_input");
    }
}
