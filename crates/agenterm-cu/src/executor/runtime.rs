//! Durable runtime session and target-lock command adapters.

use super::*;
use crate::executor::managed_jobs::stop_session_jobs;
use crate::runtime_coordinator::RuntimeCoordinator;

fn runtime_now_utc_s() -> Result<i64, CuError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CuError::new("runtime_clock_invalid", "system clock predates Unix epoch"))?
        .as_secs();
    i64::try_from(seconds).map_err(|_| {
        CuError::new(
            "runtime_clock_invalid",
            "system clock exceeds runtime range",
        )
    })
}

fn runtime_json(value: impl serde::Serialize) -> Result<serde_json::Value, CuError> {
    serde_json::to_value(value).map_err(|_| {
        CuError::new(
            "runtime_state_serialization",
            "runtime command result could not be serialized",
        )
    })
}

pub(super) fn session_start_payload(
    label: Option<&str>,
    ttl_seconds: u64,
) -> Result<serde_json::Value, CuError> {
    runtime_json(RuntimeCoordinator::open()?.session_start(
        label,
        ttl_seconds,
        runtime_now_utc_s()?,
    )?)
}

pub(super) fn session_list_payload() -> Result<serde_json::Value, CuError> {
    let sessions = RuntimeCoordinator::open()?.session_list(runtime_now_utc_s()?)?;
    Ok(serde_json::json!({ "sessions": sessions, "count": sessions.len() }))
}

pub(super) fn session_status_payload(session_id: &str) -> Result<serde_json::Value, CuError> {
    runtime_json(RuntimeCoordinator::open()?.session_status(session_id, runtime_now_utc_s()?)?)
}

pub(super) fn session_renew_payload(
    session_id: &str,
    lease: &str,
    ttl_seconds: u64,
) -> Result<serde_json::Value, CuError> {
    runtime_json(RuntimeCoordinator::open()?.session_renew(
        session_id,
        lease,
        ttl_seconds,
        runtime_now_utc_s()?,
    )?)
}

pub(super) fn session_end_payload(
    coordinator: &RuntimeCoordinator,
    session_id: &str,
    lease: &str,
    confirm: bool,
) -> Result<serde_json::Value, CuError> {
    if !confirm {
        return Err(CuError::new(
            "confirmation_required",
            "session-end requires --confirm",
        ));
    }
    let _session_gate = coordinator.acquire_session_gate(session_id)?;
    let ended = coordinator.session_end(session_id, lease, runtime_now_utc_s()?)?;
    let mut payload = runtime_json(&ended)?;
    match stop_session_jobs(session_id) {
        Ok(cleanup) => {
            payload
                .as_object_mut()
                .expect("session end serializes as an object")
                .insert("jobs".to_owned(), cleanup);
            Ok(payload)
        }
        Err(mut error) => {
            let jobs = error
                .detail
                .take()
                .and_then(|detail| detail.get("jobs").cloned());
            error.detail = Some(serde_json::json!({
                "effect": "session_ended",
                "session": ended.session,
                "released_locks": ended.released_locks,
                "jobs": jobs,
            }));
            Err(error)
        }
    }
}

pub(super) fn lock_acquire_payload(
    session_id: &str,
    lease: &str,
    lock_target: &str,
    ttl_seconds: u64,
) -> Result<serde_json::Value, CuError> {
    runtime_json(RuntimeCoordinator::open()?.lock_acquire(
        session_id,
        lease,
        lock_target,
        ttl_seconds,
        runtime_now_utc_s()?,
    )?)
}

pub(super) fn lock_list_payload() -> Result<serde_json::Value, CuError> {
    let locks = RuntimeCoordinator::open()?.lock_list(runtime_now_utc_s()?)?;
    Ok(serde_json::json!({ "locks": locks, "count": locks.len() }))
}

pub(super) fn lock_release_payload(
    lock_id: &str,
    lease: &str,
) -> Result<serde_json::Value, CuError> {
    runtime_json(RuntimeCoordinator::open()?.lock_release(lock_id, lease, runtime_now_utc_s()?)?)
}
