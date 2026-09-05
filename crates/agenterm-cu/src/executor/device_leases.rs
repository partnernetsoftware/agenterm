use std::{
    io::Write as _,
    process::{Command as ProcessCommand, Stdio},
    thread,
    time::{Duration, Instant},
};

use agenterm_platform::{
    entropy::secure_random_array,
    process::{DetachedSpawnMode, spawn_detached_child},
    process_observation::{self, ProcessObservation},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    command::{
        DeviceDataEncoding, DeviceSerialConfiguration, DeviceSerialFlow, DeviceSerialParity,
    },
    device_lease_ipc::{
        DeviceOperation, DeviceProtocolError, DeviceResult, client_request, client_session_release,
    },
    device_lease_owner::{
        DeviceLeaseLaunch, LAUNCH_SCHEMA_VERSION, SerialConfigurationWire, SerialFlowWire,
        SerialParityWire,
    },
    device_lease_store::{DeviceLeaseRecord, DeviceLeaseState, DeviceLeaseStore},
    idempotency_store::FinalReplay,
    managed_job_ipc::{base64_decode, base64_encode},
    target_binding::CurrentIdentityProvider,
};

use super::{CuError, JobRequestContext, now_utc_ms};

const START_READY_TIMEOUT: Duration = Duration::from_secs(5);
const START_POLL: Duration = Duration::from_millis(20);

pub(super) fn device_replay_payload(replay: &FinalReplay) -> Value {
    match replay {
        FinalReplay::DeviceClaim {
            lease_id,
            generation,
        } => json!({
            "lease_id": lease_id,
            "generation": generation,
            "effect": "not_repeated",
            "idempotent": true,
            "lease_secret_replayed": false,
        }),
        FinalReplay::JobSpawn { .. } => unreachable!("device replay requires device identity"),
    }
}

pub(super) fn replay_from_device_claim(value: &Value) -> Result<FinalReplay, CuError> {
    let lease_id = value
        .get("lease_id")
        .and_then(Value::as_str)
        .ok_or_else(device_replay_error)?;
    let generation = value
        .get("generation")
        .and_then(Value::as_u64)
        .filter(|generation| *generation != 0)
        .ok_or_else(device_replay_error)?;
    Ok(FinalReplay::DeviceClaim {
        lease_id: lease_id.to_owned(),
        generation,
    })
}

pub(super) fn device_claim_payload(
    device_id: &str,
    ttl_seconds: u64,
    serial: Option<&DeviceSerialConfiguration>,
    request: &JobRequestContext<'_>,
) -> Result<Value, CuError> {
    let _refresh_fence = request.runtime.acquire_refresh_fence()?;
    let _session_gate = request.runtime.acquire_session_gate(request.session_id)?;
    let now = clock()?;
    request
        .runtime
        .session_verify(request.session_id, request.session_lease, now / 1_000)?;
    let lock_target = format!("device:{device_id}");
    let lock = request.runtime.lock_acquire(
        request.session_id,
        request.session_lease,
        &lock_target,
        ttl_seconds,
        now / 1_000,
    )?;
    let secret = random_secret()?;
    let store = DeviceLeaseStore::open()?;
    let expires_at_utc_ms = lock
        .lock
        .expires_at_utc_s
        .checked_mul(1_000)
        .ok_or_else(ttl_error)?;
    let owner_ttl_ms = expires_at_utc_ms
        .checked_sub(now)
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or_else(ttl_error)?;
    let record = match store.reserve_claim(
        request.session_id,
        &lock.lock.lock_id,
        device_id,
        &sha256_hex(&secret),
        expires_at_utc_ms,
        now,
    ) {
        Ok(record) => record,
        Err(error) => {
            let _ = request.runtime.lock_release(
                &lock.lock.lock_id,
                request.session_lease,
                now / 1_000,
            );
            return Err(error);
        }
    };
    let identity = match CurrentIdentityProvider::default_for_current_user() {
        Ok(identity) => identity,
        Err(_) => {
            return Err(abort_unclaimed_claim(
                &store,
                &record,
                request,
                "device_identity_unavailable",
                "installation-scoped device identity is unavailable",
                now,
            ));
        }
    };
    let launch = DeviceLeaseLaunch {
        schema_version: LAUNCH_SCHEMA_VERSION,
        state_path: store.path().to_path_buf(),
        identity_state_dir: identity.private_state_dir().to_path_buf(),
        handle: record.handle(),
        device_id: device_id.to_owned(),
        lease_secret: secret.clone(),
        session_id: request.session_id.to_owned(),
        session_lease: request.session_lease.to_owned(),
        ttl_ms: owner_ttl_ms,
        serial: serial_wire(serial),
    };
    let encoded = match serde_json::to_vec(&launch) {
        Ok(encoded) => encoded,
        Err(_) => {
            return Err(abort_unclaimed_claim(
                &store,
                &record,
                request,
                "device_owner_launch_invalid",
                "device owner launch could not be encoded",
                now,
            ));
        }
    };
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(_) => {
            return Err(abort_unclaimed_claim(
                &store,
                &record,
                request,
                "device_owner_spawn_failed",
                "agenterm-cu executable identity is unavailable",
                now,
            ));
        }
    };
    let mut command = ProcessCommand::new(executable);
    command
        .arg(crate::DEVICE_LEASE_OWNER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut owner_child, mode) = match spawn_detached_child(&mut command) {
        Ok(started) => started,
        Err(_) => {
            return Err(abort_unclaimed_claim(
                &store,
                &record,
                request,
                "device_owner_spawn_failed",
                "resident device owner could not start",
                now,
            ));
        }
    };
    if mode != DetachedSpawnMode::Independent {
        let _ = owner_child.kill();
        let _ = owner_child.wait();
        return Err(abort_unclaimed_claim(
            &store,
            &record,
            request,
            "device_owner_detach_unavailable",
            "host kept the device owner inside the caller lifetime",
            now,
        ));
    }
    let Some(mut input) = owner_child.stdin.take() else {
        let _ = owner_child.kill();
        let _ = owner_child.wait();
        return Err(abort_unclaimed_claim(
            &store,
            &record,
            request,
            "device_owner_stdin_unavailable",
            "resident device owner launch channel is unavailable",
            now,
        ));
    };
    if input
        .write_all(&encoded)
        .and_then(|()| input.flush())
        .is_err()
    {
        drop(input);
        let _ = owner_child.kill();
        let _ = owner_child.wait();
        return Err(classify_owner_exit(
            &store,
            &record,
            request,
            "device owner launch document was not delivered",
            now,
        ));
    }
    drop(input);

    let deadline = Instant::now() + START_READY_TIMEOUT;
    loop {
        let current = store
            .get(&record.lease_id)?
            .ok_or_else(|| startup_unknown("device claim intent disappeared after owner spawn"))?;
        match &current.state {
            DeviceLeaseState::Active => {
                detach_reaper(owner_child);
                let mut payload = public_record(&current);
                payload["lease"] = json!(secret);
                payload["lease_secret_replayed"] = json!(false);
                payload["effect"] = json!("committed");
                return Ok(payload);
            }
            DeviceLeaseState::OpenFailed { code } => {
                let code = code.clone();
                let _ = owner_child.wait();
                let _ = request.runtime.lock_release(
                    &lock.lock.lock_id,
                    request.session_lease,
                    clock().unwrap_or(now) / 1_000,
                );
                return Err(CuError::new(
                    code,
                    "resident device owner refused the native open",
                ));
            }
            DeviceLeaseState::OwnerLost | DeviceLeaseState::CleanupUncertain { .. } => {
                detach_reaper(owner_child);
                return Err(startup_unknown(
                    "device owner became uncertain during startup",
                ));
            }
            DeviceLeaseState::ClaimIntent | DeviceLeaseState::Opening => {}
            DeviceLeaseState::Released | DeviceLeaseState::Expired => {
                detach_reaper(owner_child);
                return Err(startup_unknown("device owner terminated before readiness"));
            }
        }
        if let Ok(Some(_)) = owner_child.try_wait() {
            return Err(classify_owner_exit(
                &store,
                &record,
                request,
                "device owner exited before readiness",
                now,
            ));
        }
        if Instant::now() >= deadline {
            let _ = owner_child.kill();
            let _ = owner_child.wait();
            return Err(classify_owner_exit(
                &store,
                &record,
                request,
                "device owner readiness timed out",
                now,
            ));
        }
        thread::sleep(START_POLL);
    }
}

pub(super) fn device_claims_payload(
    offset: Option<usize>,
    max: Option<usize>,
) -> Result<Value, CuError> {
    let store = DeviceLeaseStore::open()?;
    let mut records = store.list()?;
    reconcile_records(&store, &mut records);
    records.sort_by(|left, right| {
        right
            .created_at_utc_ms
            .cmp(&left.created_at_utc_ms)
            .then_with(|| left.lease_id.cmp(&right.lease_id))
    });
    let total = records.len();
    let offset = offset.unwrap_or(0).min(total);
    let max = max.unwrap_or(100);
    let rows: Vec<_> = records
        .into_iter()
        .skip(offset)
        .take(max)
        .map(|record| public_record(&record))
        .collect();
    Ok(json!({
        "schema_version": 1,
        "total": total,
        "offset": offset,
        "max": max,
        "count": rows.len(),
        "leases": rows,
        "secret_fields_present": false,
    }))
}

pub(super) fn device_status_payload(lease_id: &str, generation: u64) -> Result<Value, CuError> {
    let store = DeviceLeaseStore::open()?;
    let mut record = checked_record(&store, lease_id, generation)?;
    reconcile_records(&store, std::slice::from_mut(&mut record));
    Ok(public_record(&record))
}

/// Restore and close every native device bound to an already-terminal
/// session. The caller retains the session admission gate, so no new claim or
/// renewal can race this sweep. Session authority is accepted only for the
/// release operation and is never persisted in the device registry.
pub(super) fn stop_session_devices(
    session_id: &str,
    session_lease: &str,
) -> Result<Value, CuError> {
    let store = DeviceLeaseStore::open()?;
    let records = store
        .list()?
        .into_iter()
        .filter(|record| record.session_id == session_id)
        .collect::<Vec<_>>();
    let matched = records.len();
    let mut already_terminal = 0usize;
    let mut released = 0usize;
    let mut failed = Vec::new();

    for record in records {
        match record.state {
            DeviceLeaseState::Released
            | DeviceLeaseState::Expired
            | DeviceLeaseState::OpenFailed { .. } => already_terminal += 1,
            DeviceLeaseState::ClaimIntent => match store.mark_unclaimed_open_failed(
                &record.handle(),
                "runtime_session_ended",
                clock()?,
            ) {
                Ok(_) => released += 1,
                Err(error) => failed.push(json!({
                    "lease_id": record.lease_id,
                    "generation": record.generation,
                    "code": error.code,
                })),
            },
            DeviceLeaseState::Opening | DeviceLeaseState::Active => {
                match client_session_release(&record.handle(), session_id, session_lease) {
                    Ok(DeviceResult::Released { state, .. }) if state == "released" => {
                        released += 1;
                    }
                    Ok(DeviceResult::Released { .. }) => failed.push(json!({
                        "lease_id": record.lease_id,
                        "generation": record.generation,
                        "code": "device_release_unverified",
                    })),
                    Ok(_) => failed.push(json!({
                        "lease_id": record.lease_id,
                        "generation": record.generation,
                        "code": "device_owner_protocol_invalid",
                    })),
                    Err(error) => failed.push(json!({
                        "lease_id": record.lease_id,
                        "generation": record.generation,
                        "code": error.code,
                    })),
                }
            }
            DeviceLeaseState::OwnerLost | DeviceLeaseState::CleanupUncertain { .. } => {
                failed.push(json!({
                    "lease_id": record.lease_id,
                    "generation": record.generation,
                    "code": "device_cleanup_uncertain",
                }));
            }
        }
    }

    let summary = json!({
        "matched": matched,
        "already_terminal": already_terminal,
        "released": released,
        "failed": failed,
        "failed_count": failed.len(),
        "lease_redacted": true,
    });
    if failed.is_empty() {
        Ok(summary)
    } else {
        Err(CuError::new(
            "device_session_cleanup_uncertain",
            "session ended but one or more resident device owners were not proved released",
        )
        .with_detail(json!({ "devices": summary })))
    }
}

pub(super) fn device_read_payload(
    lease_id: &str,
    generation: u64,
    lease: &str,
    max_bytes: usize,
    timeout_ms: u64,
    encoding: DeviceDataEncoding,
    request: &JobRequestContext<'_>,
) -> Result<Value, CuError> {
    let _session_gate = request.runtime.acquire_session_gate(request.session_id)?;
    let record = checked_session_record(lease_id, generation, request)?;
    let result = client_request(
        &record.handle(),
        lease,
        DeviceOperation::Read {
            max_bytes,
            timeout_ms,
        },
    )
    .map_err(map_protocol_error)?;
    let DeviceResult::Read {
        data_base64,
        bytes,
        state,
        total_bytes_read,
    } = result
    else {
        return Err(protocol_shape_error());
    };
    let raw = base64_decode(&data_base64).map_err(|_| protocol_shape_error())?;
    if raw.len() != bytes {
        return Err(protocol_shape_error());
    }
    let data = match encoding {
        DeviceDataEncoding::Base64 => data_base64,
        DeviceDataEncoding::Hex => encode_hex(&raw),
    };
    Ok(json!({
        "schema_version": 1,
        "lease_id": lease_id,
        "generation": generation,
        "encoding": encoding.as_str(),
        "data": data,
        "bytes": bytes,
        "state": state,
        "total_bytes_read": total_bytes_read,
        "lease_redacted": true,
    }))
}

pub(super) fn device_write_payload(
    lease_id: &str,
    generation: u64,
    lease: &str,
    data: &str,
    encoding: DeviceDataEncoding,
    timeout_ms: u64,
    request: &JobRequestContext<'_>,
) -> Result<Value, CuError> {
    let _session_gate = request.runtime.acquire_session_gate(request.session_id)?;
    let record = checked_session_record(lease_id, generation, request)?;
    let raw = decode_data(data, encoding)?;
    let result = client_request(
        &record.handle(),
        lease,
        DeviceOperation::Write {
            data_base64: base64_encode(&raw),
            timeout_ms,
        },
    )
    .map_err(map_protocol_error)?;
    let DeviceResult::Write {
        requested_bytes,
        written_bytes,
        delivery,
        total_bytes_written,
    } = result
    else {
        return Err(protocol_shape_error());
    };
    Ok(json!({
        "schema_version": 1,
        "lease_id": lease_id,
        "generation": generation,
        "requested_bytes": requested_bytes,
        "written_bytes": written_bytes,
        "delivery": delivery,
        "total_bytes_written": total_bytes_written,
        "lease_redacted": true,
        "payload_redacted": true,
        "effect": if written_bytes == 0 { "none" } else { "committed" },
    }))
}

pub(super) fn device_renew_payload(
    lease_id: &str,
    generation: u64,
    lease: &str,
    ttl_seconds: u64,
    request: &JobRequestContext<'_>,
) -> Result<Value, CuError> {
    let _session_gate = request.runtime.acquire_session_gate(request.session_id)?;
    let record = checked_session_record(lease_id, generation, request)?;
    let now = clock()?;
    let runtime_lock = request.runtime.lock_acquire(
        request.session_id,
        request.session_lease,
        &format!("device:{}", record.device_id),
        ttl_seconds,
        now / 1_000,
    )?;
    if runtime_lock.lock.lock_id != record.runtime_lock_id {
        let _ = request.runtime.lock_release(
            &runtime_lock.lock.lock_id,
            request.session_lease,
            now / 1_000,
        );
        return Err(CuError::new(
            "device_runtime_lock_changed",
            "device runtime lock expired or changed before renewal",
        ));
    }
    let owner_ttl_ms = runtime_lock
        .lock
        .expires_at_utc_s
        .checked_mul(1_000)
        .and_then(|expires| expires.checked_sub(now))
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or_else(ttl_error)?;
    let result = client_request(
        &record.handle(),
        lease,
        DeviceOperation::Renew {
            ttl_ms: owner_ttl_ms,
        },
    )
    .map_err(map_protocol_error)?;
    let DeviceResult::Renewed { expires_at_utc_ms } = result else {
        return Err(protocol_shape_error());
    };
    Ok(json!({
        "schema_version": 1,
        "lease_id": lease_id,
        "generation": generation,
        "expires_at_utc_ms": expires_at_utc_ms,
        "lease_redacted": true,
        "effect": "committed",
    }))
}

pub(super) fn device_release_payload(
    lease_id: &str,
    generation: u64,
    lease: &str,
    request: &JobRequestContext<'_>,
) -> Result<Value, CuError> {
    let _session_gate = request.runtime.acquire_session_gate(request.session_id)?;
    let record = checked_session_record(lease_id, generation, request)?;
    let result = client_request(&record.handle(), lease, DeviceOperation::Release)
        .map_err(map_protocol_error)?;
    let DeviceResult::Released {
        state,
        bytes_read,
        bytes_written,
    } = result
    else {
        return Err(protocol_shape_error());
    };
    if let Err(error) = request.runtime.lock_release(
        &record.runtime_lock_id,
        request.session_lease,
        clock()? / 1_000,
    ) {
        return Err(CuError::new(
            "device_release_cleanup_uncertain",
            "native device was released but its runtime lock cleanup is uncertain",
        )
        .with_detail(json!({
            "effect": "committed",
            "device_state": state,
            "cause": error.code,
            "lease_redacted": true,
        })));
    }
    Ok(json!({
        "schema_version": 1,
        "lease_id": lease_id,
        "generation": generation,
        "state": state,
        "bytes_read": bytes_read,
        "bytes_written": bytes_written,
        "lease_redacted": true,
        "effect": "committed",
    }))
}

fn checked_session_record(
    lease_id: &str,
    generation: u64,
    request: &JobRequestContext<'_>,
) -> Result<DeviceLeaseRecord, CuError> {
    request
        .runtime
        .session_verify(request.session_id, request.session_lease, clock()? / 1_000)?;
    let record = checked_record(&DeviceLeaseStore::open()?, lease_id, generation)?;
    if record.session_id != request.session_id {
        return Err(CuError::new(
            "device_lease_session_mismatch",
            "device lease belongs to a different runtime session",
        ));
    }
    Ok(record)
}

fn checked_record(
    store: &DeviceLeaseStore,
    lease_id: &str,
    generation: u64,
) -> Result<DeviceLeaseRecord, CuError> {
    let record = store
        .get(lease_id)?
        .ok_or_else(|| CuError::new("device_lease_not_found", "device lease does not exist"))?;
    if record.generation != generation {
        return Err(CuError::new(
            "device_lease_identity_changed",
            "device lease generation no longer matches",
        ));
    }
    Ok(record)
}

fn reconcile_records(store: &DeviceLeaseStore, records: &mut [DeviceLeaseRecord]) {
    for record in records {
        if !matches!(
            record.state,
            DeviceLeaseState::Opening | DeviceLeaseState::Active
        ) {
            continue;
        }
        let Some(owner) = record.owner.as_ref() else {
            continue;
        };
        let lost = match process_observation::observe(owner.pid) {
            ProcessObservation::Dead { .. } => true,
            ProcessObservation::Live {
                start_identity: Some(identity),
            } => identity != owner.start_identity,
            ProcessObservation::Live {
                start_identity: None,
            }
            | ProcessObservation::Unknown { .. } => false,
            _ => false,
        };
        if lost
            && let Ok(updated) = store.mark_owner_lost(
                &record.handle(),
                owner,
                clock().unwrap_or(record.updated_at_utc_ms),
            )
        {
            *record = updated;
        }
    }
}

fn abort_unclaimed_claim(
    store: &DeviceLeaseStore,
    record: &DeviceLeaseRecord,
    request: &JobRequestContext<'_>,
    code: &'static str,
    message: &'static str,
    now_utc_ms: i64,
) -> CuError {
    let state = store.mark_unclaimed_open_failed(&record.handle(), code, now_utc_ms);
    let runtime_lock = request.runtime.lock_release(
        &record.runtime_lock_id,
        request.session_lease,
        now_utc_ms / 1_000,
    );
    if state.is_ok() && runtime_lock.is_ok() {
        CuError::new(code, message)
    } else {
        CuError::new(
            "device_claim_cleanup_uncertain",
            "device owner did not start and its durable claim cleanup is uncertain",
        )
        .with_detail(json!({
            "effect": "unknown",
            "cause": code,
            "state_cleanup": state.err().map(|error| error.code),
            "runtime_lock_cleanup": runtime_lock.err().map(|error| error.code),
            "lease_redacted": true,
        }))
    }
}

fn classify_owner_exit(
    store: &DeviceLeaseStore,
    reserved: &DeviceLeaseRecord,
    request: &JobRequestContext<'_>,
    message: &'static str,
    initial_now_utc_ms: i64,
) -> CuError {
    let current = match store.get(&reserved.lease_id) {
        Ok(Some(current)) => current,
        _ => return startup_unknown(message),
    };
    match &current.state {
        DeviceLeaseState::ClaimIntent => abort_unclaimed_claim(
            store,
            &current,
            request,
            "device_owner_spawn_failed",
            message,
            clock().unwrap_or(initial_now_utc_ms),
        ),
        DeviceLeaseState::OpenFailed { code } => {
            let released = request.runtime.lock_release(
                &current.runtime_lock_id,
                request.session_lease,
                clock().unwrap_or(initial_now_utc_ms) / 1_000,
            );
            if released.is_ok() {
                CuError::new(code.clone(), message)
            } else {
                startup_unknown(message)
            }
        }
        DeviceLeaseState::Released | DeviceLeaseState::Expired => {
            let _ = request.runtime.lock_release(
                &current.runtime_lock_id,
                request.session_lease,
                clock().unwrap_or(initial_now_utc_ms) / 1_000,
            );
            CuError::new("device_owner_exited_before_ready", message)
        }
        DeviceLeaseState::Opening | DeviceLeaseState::Active => {
            if let Some(owner) = current.owner.as_ref() {
                let _ = store.mark_owner_lost(
                    &current.handle(),
                    owner,
                    clock().unwrap_or(initial_now_utc_ms),
                );
            }
            startup_unknown(message)
        }
        DeviceLeaseState::OwnerLost | DeviceLeaseState::CleanupUncertain { .. } => {
            startup_unknown(message)
        }
    }
}

fn public_record(record: &DeviceLeaseRecord) -> Value {
    let owner_live =
        record
            .owner
            .as_ref()
            .and_then(|owner| match process_observation::observe(owner.pid) {
                ProcessObservation::Live {
                    start_identity: Some(identity),
                } => Some(identity == owner.start_identity),
                ProcessObservation::Dead { .. } => Some(false),
                ProcessObservation::Live {
                    start_identity: None,
                }
                | ProcessObservation::Unknown { .. } => None,
                _ => None,
            });
    json!({
        "schema_version": 1,
        "lease_id": record.lease_id,
        "generation": record.generation,
        "session_id": record.session_id,
        "device_id": record.device_id,
        "state": state_name(&record.state),
        "exclusive": record.exclusive,
        "serial": record.serial,
        "created_at_utc_ms": record.created_at_utc_ms,
        "updated_at_utc_ms": record.updated_at_utc_ms,
        "expires_at_utc_ms": record.expires_at_utc_ms,
        "terminal_at_utc_ms": record.terminal_at_utc_ms,
        "bytes_read": record.bytes_read.to_string(),
        "bytes_written": record.bytes_written.to_string(),
        "owner_live": owner_live,
        "lease_redacted": true,
        "locator_redacted": true,
    })
}

fn state_name(state: &DeviceLeaseState) -> &'static str {
    match state {
        DeviceLeaseState::ClaimIntent => "claim_intent",
        DeviceLeaseState::Opening => "opening",
        DeviceLeaseState::Active => "active",
        DeviceLeaseState::Released => "released",
        DeviceLeaseState::Expired => "expired",
        DeviceLeaseState::OpenFailed { .. } => "open_failed",
        DeviceLeaseState::OwnerLost => "owner_lost",
        DeviceLeaseState::CleanupUncertain { .. } => "cleanup_uncertain",
    }
}

fn serial_wire(serial: Option<&DeviceSerialConfiguration>) -> SerialConfigurationWire {
    let serial = serial.cloned().unwrap_or(DeviceSerialConfiguration {
        baud: 9_600,
        data_bits: 8,
        parity: DeviceSerialParity::None,
        stop_bits: 1,
        flow: DeviceSerialFlow::None,
    });
    SerialConfigurationWire {
        baud: serial.baud,
        data_bits: serial.data_bits,
        parity: match serial.parity {
            DeviceSerialParity::None => SerialParityWire::None,
            DeviceSerialParity::Even => SerialParityWire::Even,
            DeviceSerialParity::Odd => SerialParityWire::Odd,
        },
        stop_bits: serial.stop_bits,
        flow: match serial.flow {
            DeviceSerialFlow::None => SerialFlowWire::None,
            DeviceSerialFlow::Software => SerialFlowWire::Software,
            DeviceSerialFlow::Hardware => SerialFlowWire::Hardware,
        },
    }
}

fn decode_data(data: &str, encoding: DeviceDataEncoding) -> Result<Vec<u8>, CuError> {
    match encoding {
        DeviceDataEncoding::Base64 => base64_decode(data)
            .map_err(|_| CuError::new("device_encoding_invalid", "device write base64 is invalid")),
        DeviceDataEncoding::Hex => decode_hex(data),
    }
}

fn decode_hex(data: &str) -> Result<Vec<u8>, CuError> {
    if !data.len().is_multiple_of(2) {
        return Err(CuError::new(
            "device_encoding_invalid",
            "device write hex is invalid",
        ));
    }
    data.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|_| ())?;
            u8::from_str_radix(text, 16).map_err(|_| ())
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CuError::new("device_encoding_invalid", "device write hex is invalid"))
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn map_protocol_error(error: DeviceProtocolError) -> CuError {
    let detail = json!({
        "known_written_lower_bound": error.known_written_lower_bound,
        "delivery_uncertain": error.delivery_uncertain,
        "retry_safe": error.retry_safe,
        "lease_redacted": true,
    });
    CuError::new(error.code, "resident device owner refused the operation").with_detail(detail)
}

fn protocol_shape_error() -> CuError {
    CuError::new(
        "device_owner_protocol_invalid",
        "resident device owner returned the wrong reply shape",
    )
}

fn device_replay_error() -> CuError {
    CuError::new(
        "device_replay_projection_invalid",
        "successful device claim omitted its public identity",
    )
}

fn startup_unknown(message: &'static str) -> CuError {
    CuError::new("device_owner_outcome_unknown", message).with_detail(json!({
        "effect": "unknown",
        "retry_safe": false,
    }))
}

fn clock() -> Result<i64, CuError> {
    now_utc_ms()
        .ok_or_else(|| CuError::new("device_lease_clock_invalid", "system clock is unavailable"))
}

fn ttl_error() -> CuError {
    CuError::new("device_ttl_invalid", "device lease TTL overflows the clock")
}

fn random_secret() -> Result<String, CuError> {
    secure_random_array::<32>()
        .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        .map_err(|_| {
            CuError::new(
                "device_lease_entropy_unavailable",
                "OS CSPRNG is unavailable",
            )
        })
}

fn sha256_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn detach_reaper(mut child: std::process::Child) {
    let _ = thread::Builder::new()
        .name("agenterm-cu-device-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        });
}
