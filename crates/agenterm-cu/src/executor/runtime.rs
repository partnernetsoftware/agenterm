//! Durable runtime session and target-lock command adapters.

use super::*;
use crate::device_lease_store::{DeviceLeaseState, DeviceLeaseStore};
use crate::executor::device_leases::stop_session_devices;
use crate::executor::managed_jobs::stop_session_jobs;
use crate::managed_job_store::{ManagedJobState, ManagedJobStore};
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

/// Describe the actual ACU runtime topology without probing or mutating an
/// owned child. Durable records are declarations; liveness remains the owning
/// job command's responsibility so this summary cannot fabricate health.
pub(super) fn runtime_status_payload() -> Result<serde_json::Value, CuError> {
    let now = runtime_now_utc_s()?;
    let coordinator = RuntimeCoordinator::open()?;
    let counts = coordinator.status_counts(now)?;
    let jobs = ManagedJobStore::open()?.list()?;
    let device_leases = DeviceLeaseStore::open()?.list()?;

    let mut starting = 0usize;
    let mut running_declared = 0usize;
    let mut terminal = 0usize;
    let mut detached = 0usize;
    let mut orphaned_uncertain = 0usize;
    for job in &jobs {
        match job.state {
            ManagedJobState::StartIntent | ManagedJobState::Starting => starting += 1,
            ManagedJobState::Running => running_declared += 1,
            ManagedJobState::StartFailed { .. }
            | ManagedJobState::Exited { .. }
            | ManagedJobState::Signaled { .. } => terminal += 1,
            ManagedJobState::Detached => detached += 1,
            ManagedJobState::OrphanedUncertain => orphaned_uncertain += 1,
        }
    }
    let mut device_claiming = 0usize;
    let mut device_active = 0usize;
    let mut device_terminal = 0usize;
    let mut device_uncertain = 0usize;
    for lease in &device_leases {
        match lease.state {
            DeviceLeaseState::ClaimIntent | DeviceLeaseState::Opening => device_claiming += 1,
            DeviceLeaseState::Active => device_active += 1,
            DeviceLeaseState::Released
            | DeviceLeaseState::Expired
            | DeviceLeaseState::OpenFailed { .. } => device_terminal += 1,
            DeviceLeaseState::OwnerLost | DeviceLeaseState::CleanupUncertain { .. } => {
                device_uncertain += 1;
            }
        }
    }

    Ok(serde_json::json!({
        "schema": 1,
        "architecture": "on-demand-coordinator-with-resource-owners",
        "global_daemon": {
            "present": false,
            "required": false,
            "lifecycle_commands": "not-applicable",
        },
        "coordinator": {
            "state": "available",
            "activation": "on-demand-per-command",
        },
        "sessions": { "active": counts.active_sessions },
        "locks": { "active": counts.active_locks },
        "managed_jobs": {
            "total_records": jobs.len(),
            "starting": starting,
            "running_declared": running_declared,
            "terminal": terminal,
            "detached": detached,
            "orphaned_uncertain": orphaned_uncertain,
            "owner_liveness_probed": false,
        },
        "device_leases": {
            "total_records": device_leases.len(),
            "claiming": device_claiming,
            "active_declared": device_active,
            "terminal": device_terminal,
            "owner_or_cleanup_uncertain": device_uncertain,
            "owner_liveness_probed": false,
        },
        "action": {
            "performed": false,
            "reason": "read-only topology summary; use job-status for exact owner liveness",
        },
    }))
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
    let jobs = stop_session_jobs(session_id);
    let devices = stop_session_devices(session_id, lease);
    if let (Ok(jobs), Ok(devices)) = (&jobs, &devices) {
        let object = payload
            .as_object_mut()
            .expect("session end serializes as an object");
        object.insert("jobs".to_owned(), jobs.clone());
        object.insert("devices".to_owned(), devices.clone());
        return Ok(payload);
    }
    let project = |result: Result<serde_json::Value, CuError>, field: &str| match result {
        Ok(value) => value,
        Err(error) => error
            .detail
            .and_then(|detail| detail.get(field).cloned())
            .unwrap_or_else(|| serde_json::json!({ "code": error.code })),
    };
    Err(CuError::new(
        "runtime_session_cleanup_uncertain",
        "session ended but one or more resident resources were not proved stopped",
    )
    .with_detail(serde_json::json!({
        "effect": "session_ended",
        "session": ended.session,
        "released_locks": ended.released_locks,
        "jobs": project(jobs, "jobs"),
        "devices": project(devices, "devices"),
    })))
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
