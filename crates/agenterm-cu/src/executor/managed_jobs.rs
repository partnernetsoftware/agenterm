use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    thread,
    time::{Duration, Instant},
};

use agenterm_platform::process::{DetachedSpawnMode, spawn_detached_child};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    command::{
        JobEnvironment, JobExpiry, JobOutputCursor, JobOutputStream, JobPolicyAction,
        JobProcessLimits, JobResourcePolicy, JobStateFilter,
    },
    execution_control::ExecutionControl,
    idempotency_store::FinalReplay,
    managed_job_ipc::{
        JobState, JobStatus, ManagedJobOperation, ManagedJobProtocolError, ManagedJobResult,
        OutputStream, ROUND_TIMEOUT_CODE, WAIT_QUANTUM, WAIT_ROUND_BUDGET, WAIT_ROUND_MARGIN,
        base64_decode, base64_encode, client_request, client_request_before,
    },
    managed_job_owner::{
        LAUNCH_SCHEMA_VERSION, ManagedJobEnvironment, ManagedJobLaunch, ManagedJobProcessLimits,
    },
    managed_job_store::{
        ManagedJobDetachLiveness, ManagedJobHandle, ManagedJobOnExpiry, ManagedJobOrigin,
        ManagedJobRecord, ManagedJobState, ManagedJobStore, ManagedJobTerminalTrigger,
        OwnerLiveness, ResidentOwnerIdentity,
    },
    runtime_coordinator::RuntimeCoordinator,
};

#[cfg(unix)]
use crate::managed_job_owner::ManagedJobAdoption;

use super::{
    CuError, now_utc_ms,
    process::{ProcessSignalOptions, process_signal_payload},
};

const START_READY_TIMEOUT: Duration = Duration::from_secs(5);
const START_POLL: Duration = Duration::from_millis(20);
const OUTPUT_CAPACITY_BYTES: usize = 1024 * 1024;
const IPC_PAGE_BYTES: usize = 64 * 1024;
const JOB_RESOURCE_MAX_SAMPLES: usize = 1_000;
const JOB_RESOURCE_MAX_MEMBERS: usize = 256;
const JOB_RESOURCE_MAX_MEMBER_ROWS: usize = 512 * JOB_RESOURCE_MAX_MEMBERS;

fn member_rows_would_overflow(current: usize, next: usize) -> bool {
    current.saturating_add(next) > JOB_RESOURCE_MAX_MEMBER_ROWS
}

pub(super) struct JobRequestContext<'a> {
    pub session_id: &'a str,
    pub session_lease: &'a str,
    pub runtime: &'a RuntimeCoordinator,
}

pub(super) fn replay_payload(replay: &FinalReplay) -> Value {
    match replay {
        FinalReplay::JobSpawn { job_id, generation } => json!({
            "job_id": job_id,
            "generation": generation,
        }),
        FinalReplay::DeviceClaim { .. } | FinalReplay::PrivilegeApply { .. } => {
            unreachable!("managed-job replay requires managed-job identity")
        }
    }
}

pub(super) fn replay_from_spawn_reply(value: &Value) -> Result<FinalReplay, CuError> {
    let job_id = value
        .get("job_id")
        .and_then(Value::as_str)
        .ok_or_else(replay_error)?;
    let generation = value
        .get("generation")
        .and_then(Value::as_u64)
        .filter(|generation| *generation != 0)
        .ok_or_else(replay_error)?;
    if value.as_object().is_none_or(|object| object.len() != 2) {
        return Err(replay_error());
    }
    Ok(FinalReplay::JobSpawn {
        job_id: job_id.to_owned(),
        generation,
    })
}

/// The ONLY translation from a public expiry spelling to the internal policy.
///
/// Both surfaces funnel through here: `job-spawn`'s enum and `job-adopt`'s
/// boolean. One-way and centralized, so the internal model has exactly one
/// expiry truth and no second boolean is ever stored beside it.
fn managed_on_expiry(expiry: JobExpiry) -> ManagedJobOnExpiry {
    match expiry {
        JobExpiry::Stop => ManagedJobOnExpiry::Stop,
        JobExpiry::Detach => ManagedJobOnExpiry::Detach,
    }
}

/// The public `job-adopt` boolean translated into the canonical spelling, which
/// then goes through [`managed_on_expiry`] like every other surface.
///
/// `job-adopt` itself is a Unix contract (`#[cfg(unix)]`), so this translation
/// only exists there; on other hosts the verb is a typed refusal and there is no
/// boolean to translate.
#[cfg(unix)]
fn expiry_from_stop_on_expiry(stop_on_expiry: bool) -> JobExpiry {
    if stop_on_expiry {
        JobExpiry::Stop
    } else {
        JobExpiry::Detach
    }
}

pub(super) fn job_spawn_payload(
    command: &[String],
    environment: &[JobEnvironment],
    cwd: Option<&str>,
    limits: Option<JobProcessLimits>,
    ttl_seconds: u64,
    expiry: JobExpiry,
    request: &JobRequestContext<'_>,
) -> Result<Value, CuError> {
    // FIRST statement, before the refresh fence, the session gate, the store
    // open, the intent reservation or any owner/process spawn: a spawned job's
    // `detach` is retired because the managed owner must clean up its own child.
    // Refusing here (not in `validate()`) is what keeps the code typed.
    if !expiry.is_stop() {
        return Err(CuError::new(
            "managed_job_detach_retired",
            "job-spawn accepts only --expiry stop; detached expiry is retired because the managed owner must clean up its own child",
        ));
    }
    let _refresh_fence = request.runtime.acquire_refresh_fence()?;
    let _session_gate = request.runtime.acquire_session_gate(request.session_id)?;
    let admission_now = now_utc_ms().ok_or_else(clock_error)?;
    request.runtime.session_verify(
        request.session_id,
        request.session_lease,
        admission_now / 1_000,
    )?;
    let store = ManagedJobStore::open()?;
    let now = admission_now;
    let record = store.reserve_start(Some(request.session_id), now)?;
    let launch = match build_launch(
        &store,
        &record,
        SpawnRequest {
            command,
            environment,
            cwd,
            limits,
            ttl_seconds,
            on_expiry: expiry,
        },
    ) {
        Ok(launch) => launch,
        Err(error) => {
            let _ = store.mark_unclaimed_start_failed(
                &record.handle(),
                error.code.as_str(),
                now_utc_ms().unwrap_or(now),
            );
            return Err(error);
        }
    };
    start_resident_launch(&store, &record, &launch, now)
}

pub(super) fn job_adopt_payload(
    pid: u32,
    start_identity: &str,
    ttl_seconds: u64,
    stop_on_expiry: bool,
    request: &JobRequestContext<'_>,
) -> Result<Value, CuError> {
    #[cfg(not(unix))]
    {
        let _ = (pid, start_identity, ttl_seconds, stop_on_expiry, request);
        Err(CuError::new(
            "managed_job_adopt_unsupported",
            "this host cannot retain an existing process group safely",
        ))
    }
    #[cfg(unix)]
    {
        let _refresh_fence = request.runtime.acquire_refresh_fence()?;
        let _session_gate = request.runtime.acquire_session_gate(request.session_id)?;
        let now = now_utc_ms().ok_or_else(clock_error)?;
        request
            .runtime
            .session_verify(request.session_id, request.session_lease, now / 1_000)?;

        // Validate the complete current-user group before publishing durable
        // intent. The resident owner repeats this check after claiming the
        // generation, closing the admission-to-owner race fail-closed.
        let group =
            agenterm_platform::process::ProcessTreeGuard::adopt_group_leader_for_termination(
                pid,
                start_identity,
                JOB_RESOURCE_MAX_MEMBERS,
            )
            .map_err(|error| {
                CuError::new(
                    "managed_job_adopt_validation_failed",
                    "process group could not be identity-bound for adoption",
                )
                .with_detail(json!({ "reason": error }))
            })?;
        drop(group);

        let store = ManagedJobStore::open()?;
        // The one-way, centralized translation lives in the helper; this is the
        // only place the public boolean becomes the internal policy.
        let on_expiry = managed_on_expiry(expiry_from_stop_on_expiry(stop_on_expiry));
        let record = store.reserve_start_with_origin(
            Some(request.session_id),
            ManagedJobOrigin::Adopted,
            on_expiry,
            now,
        )?;
        let lease_ttl_ms = ttl_seconds.checked_mul(1_000).ok_or_else(|| {
            CuError::new(
                "managed_job_ttl_invalid",
                "managed-job TTL overflows milliseconds",
            )
        })?;
        let launch = ManagedJobLaunch {
            schema_version: LAUNCH_SCHEMA_VERSION,
            state_path: store.path().to_owned(),
            handle: record.handle(),
            program: PathBuf::new(),
            arguments: Vec::new(),
            current_directory: None,
            environment: Vec::new(),
            limits: None,
            adoption: Some(ManagedJobAdoption {
                process_id: pid,
                start_identity: start_identity.to_owned(),
            }),
            on_expiry,
            output_capacity_bytes: OUTPUT_CAPACITY_BYTES,
            lease_ttl_ms,
        };
        start_resident_launch(&store, &record, &launch, now)
    }
}

fn start_resident_launch(
    store: &ManagedJobStore,
    record: &ManagedJobRecord,
    launch: &ManagedJobLaunch,
    now: i64,
) -> Result<Value, CuError> {
    let encoded = match serde_json::to_vec(launch) {
        Ok(encoded) => encoded,
        Err(_) => {
            let error = CuError::new(
                "managed_job_launch_invalid",
                "managed-job launch document could not be encoded",
            );
            mark_clean_owner_failure(store, record, error.code.as_str(), now)?;
            return Err(error);
        }
    };
    // The owner is the distributed sibling `agenterm-cu`, not necessarily the
    // host that embedded this executor: an in-process `agenterm:acu` caller runs
    // inside the product binary, whose own command surface has no owner mode.
    // This refusal happens before the owner process exists, but the durable
    // start intent and its record are already published, so this is honestly
    // pre-owner-spawn and not a zero-effect failure; `mark_clean_owner_failure`
    // closes the record as it does for every other pre-spawn failure.
    let executable = match owner_launch_program(crate::owner_executable::resolve_current) {
        Ok(executable) => executable,
        Err(error) => {
            mark_clean_owner_failure(store, record, error.code.as_str(), now)?;
            return Err(error);
        }
    };
    let mut command = owner_command(&executable);
    let (mut owner_child, mode) = match spawn_detached_child(&mut command) {
        Ok(spawned) => spawned,
        Err(_) => {
            let error = CuError::new(
                "managed_job_owner_spawn_failed",
                "managed-job resident owner could not start",
            );
            mark_clean_owner_failure(store, record, error.code.as_str(), now)?;
            return Err(error);
        }
    };
    if mode != DetachedSpawnMode::Independent {
        let _ = owner_child.kill();
        let _ = owner_child.wait();
        mark_clean_owner_failure(store, record, "owner_detach_unavailable", now)?;
        return Err(CuError::new(
            "managed_job_detach_unavailable",
            "host kept the resident owner inside the caller lifetime",
        ));
    }
    let Some(mut input) = owner_child.stdin.take() else {
        let _ = owner_child.kill();
        let _ = owner_child.wait();
        mark_clean_owner_failure(store, record, "owner_stdin_unavailable", now)?;
        return Err(CuError::new(
            "managed_job_owner_stdin_unavailable",
            "resident owner launch channel is unavailable",
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
        return classify_post_spawn_failure(store, record, "owner_launch_write_failed", now);
    }
    drop(input);

    let deadline = Instant::now() + START_READY_TIMEOUT;
    loop {
        let current = match store.get(&record.job_id) {
            Ok(Some(current)) => current,
            Ok(None) => {
                return Err(outcome_unknown(
                    "managed-job intent disappeared after owner spawn",
                ));
            }
            Err(error)
                if error.code == "managed_job_store_contended" && Instant::now() < deadline =>
            {
                thread::sleep(START_POLL);
                continue;
            }
            Err(error) => return Err(error),
        };
        match current.state {
            ManagedJobState::Running
            | ManagedJobState::Exited { .. }
            | ManagedJobState::Signaled { .. }
            | ManagedJobState::Detached => {
                detach_reaper(owner_child)?;
                return Ok(public_job_identity(&current));
            }
            ManagedJobState::StartFailed { ref code } => {
                let code = code.clone();
                let _ = owner_child.wait();
                return Err(CuError::new(
                    "managed_job_start_failed",
                    format!("managed-job owner refused the contained launch: {code}"),
                ));
            }
            ManagedJobState::OrphanedUncertain => {
                detach_reaper(owner_child)?;
                return Err(outcome_unknown(
                    "managed-job owner became uncertain during startup",
                ));
            }
            ManagedJobState::StartIntent | ManagedJobState::Starting => {}
        }
        if let Ok(Some(_)) = owner_child.try_wait() {
            return classify_post_spawn_failure(store, record, "owner_exited_before_ready", now);
        }
        if Instant::now() >= deadline {
            let _ = owner_child.kill();
            let _ = owner_child.wait();
            return classify_post_spawn_failure(store, record, "owner_ready_timeout", now);
        }
        thread::sleep(START_POLL);
    }
}

/// Stop every live job and release every resident owner bound to a terminal session.
/// The caller must retain that session's admission gate for the entire call.
pub(super) fn stop_session_jobs(session_id: &str) -> Result<Value, CuError> {
    let store = ManagedJobStore::open()?;
    let records = store
        .list()?
        .into_iter()
        .filter(|record| record.session_id.as_deref() == Some(session_id))
        .collect::<Vec<_>>();
    let matched = records.len();
    let mut already_terminal = 0usize;
    let mut stopped = 0usize;
    let mut detached = 0usize;
    let mut failed = Vec::new();

    for record in records {
        match &record.state {
            ManagedJobState::StartFailed { .. }
            | ManagedJobState::Exited { .. }
            | ManagedJobState::Signaled { .. }
            | ManagedJobState::Detached => {
                if record.owner.is_none() {
                    already_terminal += 1;
                    continue;
                }
                match release_session_owner(&record, observe_session_owner, client_request) {
                    Ok(SessionOwnerRelease::AlreadyTerminal) => already_terminal += 1,
                    Ok(SessionOwnerRelease::Stopped | SessionOwnerRelease::Detached) => {
                        failed.push(cleanup_failure(
                            &record,
                            "managed_job_response_invalid".to_owned(),
                        ));
                    }
                    Err(code) => failed.push(cleanup_failure(&record, code)),
                }
            }
            ManagedJobState::StartIntent => {
                let now = now_utc_ms().ok_or_else(clock_error)?;
                match store.mark_unclaimed_start_failed(
                    &record.handle(),
                    "runtime_session_ended",
                    now,
                ) {
                    Ok(_) => stopped += 1,
                    Err(error) => failed.push(json!({
                        "job_id": record.job_id,
                        "generation": record.generation,
                        "code": error.code,
                    })),
                }
            }
            ManagedJobState::Starting | ManagedJobState::Running => {
                match release_session_owner(&record, observe_session_owner, client_request) {
                    Ok(SessionOwnerRelease::Stopped) => stopped += 1,
                    Ok(SessionOwnerRelease::Detached) => detached += 1,
                    Ok(SessionOwnerRelease::AlreadyTerminal) => already_terminal += 1,
                    Err(code) => failed.push(cleanup_failure(&record, code)),
                }
            }
            ManagedJobState::OrphanedUncertain => failed.push(json!({
                "job_id": record.job_id,
                "generation": record.generation,
                "code": "managed_job_orphaned_uncertain",
            })),
        }
    }

    let cleanup = json!({
        "matched": matched,
        "stopped": stopped,
        "detached": detached,
        "already_terminal": already_terminal,
        "failed": failed,
        "complete": failed.is_empty(),
    });
    if failed.is_empty() {
        Ok(cleanup)
    } else {
        Err(CuError::new(
            "runtime_session_cleanup_incomplete",
            "runtime session ended but one or more managed jobs were not verified terminal",
        )
        .with_detail(json!({
            "effect": "session_ended",
            "jobs": cleanup,
        })))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionOwnerRelease {
    AlreadyTerminal,
    Stopped,
    Detached,
}

/// What one exact observation of a record's recorded resident owner proved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionOwnerObservation {
    /// The recorded owner object is gone: dead, or its PID now names another object.
    Absent,
    /// The recorded owner object is still live under its exact start identity.
    Live,
    /// The observation could not decide.
    Unknown,
}

/// Observes one recorded owner identity exactly, with no PID guessing.
fn observe_session_owner(identity: &ResidentOwnerIdentity) -> SessionOwnerObservation {
    match agenterm_platform::process_observation::verify_identity(
        identity.pid,
        &identity.start_identity,
    ) {
        agenterm_platform::process_observation::IdentityVerdict::PidReused
        | agenterm_platform::process_observation::IdentityVerdict::Dead => {
            SessionOwnerObservation::Absent
        }
        agenterm_platform::process_observation::IdentityVerdict::Live => {
            SessionOwnerObservation::Live
        }
        agenterm_platform::process_observation::IdentityVerdict::IdentityUnavailable
        | agenterm_platform::process_observation::IdentityVerdict::Unobservable => {
            SessionOwnerObservation::Unknown
        }
    }
}

fn release_session_owner(
    record: &ManagedJobRecord,
    observe: impl FnOnce(&ResidentOwnerIdentity) -> SessionOwnerObservation,
    request: impl FnOnce(
        &ManagedJobHandle,
        ManagedJobOperation,
    ) -> Result<ManagedJobResult, ManagedJobProtocolError>,
) -> Result<SessionOwnerRelease, String> {
    let was_terminal = matches!(
        &record.state,
        ManagedJobState::StartFailed { .. }
            | ManagedJobState::Exited { .. }
            | ManagedJobState::Signaled { .. }
            | ManagedJobState::Detached
    );
    match request(&record.handle(), ManagedJobOperation::StopAndRelease) {
        Ok(ManagedJobResult::Stop { status }) => {
            if matches!(status.state, JobState::Running) {
                return Err("managed_job_stop_unverified".to_owned());
            }
            if was_terminal {
                Ok(SessionOwnerRelease::AlreadyTerminal)
            } else if matches!(status.state, JobState::Detached) {
                Ok(SessionOwnerRelease::Detached)
            } else {
                Ok(SessionOwnerRelease::Stopped)
            }
        }
        Ok(_) => Err("managed_job_response_invalid".to_owned()),
        // An unreachable owner is proof that a **terminal** record was already
        // released, and no proof at all for a record that still claims to be
        // live: that one belongs in `failed` with its typed code, so a release
        // that never happened is not reported as an already-terminal success.
        // A failed connect is not proof of release: the record must carry an
        // exact owner identity, and an observation of that identity must show it
        // absent (dead, or a recycled PID). A live same-identity owner, or an
        // undecidable read, stays a typed failure rather than a claimed release.
        Err(error) if error.code == "managed_job_owner_unavailable" && was_terminal => {
            match record.owner.as_ref().map(observe) {
                Some(SessionOwnerObservation::Absent) => Ok(SessionOwnerRelease::AlreadyTerminal),
                _ => Err(error.code),
            }
        }
        Err(error) => Err(error.code),
    }
}

fn cleanup_failure(record: &ManagedJobRecord, code: String) -> Value {
    json!({
        "job_id": record.job_id,
        "generation": record.generation,
        "code": code,
    })
}

pub(super) fn public_job_identity(record: &ManagedJobRecord) -> Value {
    json!({
        "job_id": record.job_id,
        "generation": record.generation,
    })
}

pub(super) fn job_list_payload(
    state: Option<JobStateFilter>,
    offset: Option<usize>,
    max: Option<usize>,
) -> Result<Value, CuError> {
    let store = ManagedJobStore::open()?;
    let mut records = store.list()?;
    records.sort_by_key(|record| std::cmp::Reverse(record.created_at_utc_ms));
    if let Some(state) = state {
        records.retain(|record| state_matches(state, &record.state));
    }
    let matched = records.len();
    let offset = offset.unwrap_or(0).min(matched);
    let max = max.unwrap_or(100);
    let jobs = records
        .into_iter()
        .skip(offset)
        .take(max)
        .map(|record| record_payload(&record, None))
        .collect::<Vec<_>>();
    Ok(json!({
        "jobs": jobs,
        "matched": matched,
        "returned": jobs.len(),
        "offset": offset,
        "truncated": offset.saturating_add(jobs.len()) < matched,
    }))
}

pub(super) fn job_status_payload(job_id: &str) -> Result<Value, CuError> {
    let store = ManagedJobStore::open()?;
    let mut record = required_record(&store, job_id)?;
    let live = if record.owner.is_some()
        && !matches!(
            record.state,
            ManagedJobState::StartFailed { .. } | ManagedJobState::OrphanedUncertain
        ) {
        match client_request(&record.handle(), ManagedJobOperation::Status) {
            Ok(ManagedJobResult::Status { status }) => Some(status),
            Ok(_) => return Err(response_kind_error()),
            Err(error) if error.code == "managed_job_owner_unavailable" => {
                record = reconcile_unavailable_owner(&store, &record)?;
                None
            }
            Err(error) => return Err(client_error(error)),
        }
    } else {
        None
    };
    Ok(record_payload(&record, live.as_ref()))
}

pub(super) fn job_resources_payload(
    job_id: &str,
    generation: u64,
    watch_ms: Option<u64>,
    requested_interval_ms: Option<u64>,
    requested_max_samples: Option<usize>,
    members_per_sample: bool,
) -> Result<Value, CuError> {
    let record = checked_record(job_id, generation)?;
    let expected = record.process.as_ref().ok_or_else(|| {
        CuError::new(
            "managed_job_process_identity_unavailable",
            "managed-job has no published exact child process identity",
        )
        .with_detail(json!({
            "job_id": job_id,
            "generation": generation,
            "state": record.state,
        }))
    })?;
    if watch_ms.is_none() {
        return resource_point_payload(&record, expected);
    }

    let duration_ms = watch_ms.expect("checked above");
    let max_samples = requested_max_samples.unwrap_or(JOB_RESOURCE_MAX_SAMPLES);
    let interval_ms = requested_interval_ms.unwrap_or_else(|| {
        if max_samples == 1 {
            duration_ms
        } else {
            duration_ms.div_ceil((max_samples - 1) as u64).max(1)
        }
    });
    let started = Instant::now();
    let deadline = started + Duration::from_millis(duration_ms);
    let mut samples = Vec::with_capacity(max_samples);
    let mut latest = None;
    let mut member_rows = 0usize;
    let ended_reason = loop {
        match resource_point_payload(&record, expected) {
            Ok(point) => {
                let point_members = point["members"].as_array().ok_or_else(|| {
                    CuError::new(
                        "managed_job_resource_shape_invalid",
                        "managed-job resource point omitted its member array",
                    )
                })?;
                if members_per_sample
                    && member_rows_would_overflow(member_rows, point_members.len())
                {
                    break "member-rows";
                }
                let mut sample = json!({
                    "t_ms": started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    "member_count": point["member_count"],
                    "membership_sha256": point["membership_sha256"],
                    "cpu_time_ns": point["cpu_time_ns"],
                    "cpu_ms": point["cpu_ms"],
                    "rss_bytes": point["rss_bytes"],
                    "page_faults": point["page_faults"],
                    "membership_complete": point["membership_complete"],
                });
                if members_per_sample {
                    member_rows += point_members.len();
                    sample["members"] = point["members"].clone();
                }
                samples.push(sample);
                latest = Some(point);
            }
            Err(error) if error.code == "managed_job_resources_terminal" && !samples.is_empty() => {
                break "job-terminal";
            }
            Err(error) => return Err(error),
        }
        if samples.len() >= max_samples {
            break "max-samples";
        }
        if Instant::now() >= deadline {
            break "duration";
        }
        thread::sleep(
            Duration::from_millis(interval_ms)
                .min(deadline.saturating_duration_since(Instant::now())),
        );
    };
    let latest = latest.expect("watch always attempts one sample");
    let membership_complete = samples
        .iter()
        .all(|sample| sample["membership_complete"] == Value::Bool(true));
    Ok(json!({
        "job_id": job_id,
        "generation": generation,
        "scope": "containment-group",
        "provider": latest["provider"],
        "breakaway_prevented": latest["breakaway_prevented"],
        "membership_complete": membership_complete,
        "tree_complete": latest["tree_complete"],
        "coherence": "stable-membership-sweep",
        "mode": "bounded-series",
        "duration_ms": duration_ms,
        "interval_ms": interval_ms,
        "max_samples": max_samples,
        "members_per_sample": members_per_sample,
        "member_rows": member_rows,
        "max_member_rows": JOB_RESOURCE_MAX_MEMBER_ROWS,
        "emitted": samples.len(),
        "completed": ended_reason == "duration",
        "truncated": ended_reason == "max-samples" || ended_reason == "member-rows",
        "ended_reason": ended_reason,
        "member_count": latest["member_count"],
        "members": latest["members"],
        "samples": samples,
        "verified": true,
    }))
}

pub(super) fn job_policy_payload(
    job_id: &str,
    generation: u64,
    action: JobPolicyAction,
    policy: Option<JobResourcePolicy>,
    session_id: Option<&str>,
) -> Result<Value, CuError> {
    let record = match action {
        JobPolicyAction::Status => checked_record(job_id, generation)?,
        JobPolicyAction::Set | JobPolicyAction::Clear => checked_owned_record(
            job_id,
            generation,
            session_id.ok_or_else(|| {
                CuError::new(
                    "managed_job_request_identity_required",
                    "job-policy set/clear requires request-id, session and session-lease",
                )
            })?,
        )?,
    };
    let operation = match (action, policy) {
        (JobPolicyAction::Status, None) => ManagedJobOperation::ResourcePolicyStatus,
        (JobPolicyAction::Set, Some(policy)) => ManagedJobOperation::ResourcePolicySet { policy },
        (JobPolicyAction::Clear, None) => ManagedJobOperation::ResourcePolicyClear,
        _ => {
            return Err(CuError::new(
                "managed_job_policy_invalid",
                "managed-job policy action and threshold payload disagree",
            ));
        }
    };
    let result = client_request(&record.handle(), operation).map_err(client_error)?;
    let ManagedJobResult::ResourcePolicy { result } = result else {
        return Err(response_kind_error());
    };
    Ok(json!({
        "job_id": job_id,
        "generation": generation,
        "policy": result.policy,
        "status": result.status,
        "changed": result.changed,
        "resident_enforcement": true,
        "verified": true,
    }))
}

fn resource_point_payload(
    record: &ManagedJobRecord,
    expected: &crate::managed_job_store::ExactProcessIdentity,
) -> Result<Value, CuError> {
    let result = client_request(
        &record.handle(),
        ManagedJobOperation::Resources {
            max_members: JOB_RESOURCE_MAX_MEMBERS,
        },
    )
    .map_err(client_error)?;
    let ManagedJobResult::Resources { snapshot } = result else {
        return Err(response_kind_error());
    };
    if !snapshot.members.iter().any(|member| {
        member.pid == expected.pid && member.start_identity == expected.start_identity
    }) {
        return Err(CuError::new(
            "managed_job_containment_root_missing",
            "resident containment snapshot omitted the durable root identity",
        ));
    }
    let after = checked_record(&record.job_id, record.generation)?;
    if after.process.as_ref() != Some(expected) {
        return Err(CuError::new(
            "managed_job_process_identity_changed",
            "managed-job durable child identity changed while resources were sampled",
        ));
    }
    let membership_sha256 = membership_digest(&snapshot.members);
    let members = snapshot
        .members
        .iter()
        .map(|member| {
            Ok(json!({
                "pid": member.pid,
                "start_identity": member.start_identity,
                "cpu_time_ns": member.cpu_time_ns,
                "cpu_ms": cpu_ms_decimal(&member.cpu_time_ns)?,
                "rss_bytes": member.rss_bytes,
                "page_faults": {
                    "total": member.page_faults_total,
                    "soft": member.page_faults_soft,
                    "hard": member.page_faults_hard,
                },
                "nice": member.nice,
                "verified": true,
            }))
        })
        .collect::<Result<Vec<_>, CuError>>()?;
    Ok(json!({
        "job_id": record.job_id,
        "generation": record.generation,
        "scope": "containment-group",
        "provider": snapshot.provider,
        "breakaway_prevented": snapshot.breakaway_prevented,
        "membership_complete": snapshot.membership_complete,
        "tree_complete": snapshot.breakaway_prevented,
        "coherence": "stable-membership-sweep",
        "mode": "point",
        "member_count": members.len(),
        "membership_sha256": membership_sha256,
        "cpu_time_ns": snapshot.cpu_time_ns,
        "cpu_ms": cpu_ms_decimal(&snapshot.cpu_time_ns)?,
        "rss_bytes": snapshot.rss_bytes,
        "page_faults": {
            "total": snapshot.page_faults_total,
            "soft": snapshot.page_faults_soft,
            "hard": snapshot.page_faults_hard,
        },
        "members": members,
        "verified": true,
    }))
}

fn membership_digest(members: &[crate::managed_job_owner::ResidentResourceMember]) -> String {
    let mut digest = Sha256::new();
    for member in members {
        digest.update(member.pid.to_be_bytes());
        digest.update((member.start_identity.len() as u64).to_be_bytes());
        digest.update(member.start_identity.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(unix)]
fn priority_membership_digest(
    members: &[crate::managed_job_owner::ResidentPriorityMember],
) -> String {
    let mut digest = Sha256::new();
    for member in members {
        digest.update(member.pid.to_be_bytes());
        digest.update((member.start_identity.len() as u64).to_be_bytes());
        digest.update(member.start_identity.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn cpu_ms_decimal(nanoseconds: &str) -> Result<String, CuError> {
    let nanoseconds = nanoseconds
        .parse::<u128>()
        .map_err(|_| metrics_contract_error())?;
    Ok(format!(
        "{}.{:06}",
        nanoseconds / 1_000_000,
        nanoseconds % 1_000_000
    ))
}

fn metrics_contract_error() -> CuError {
    CuError::new(
        "process_metrics_contract_invalid",
        "identity-bracketed process metrics returned an invalid payload",
    )
}

pub(super) fn job_write_payload(
    job_id: &str,
    generation: u64,
    data_base64: &str,
    close_stdin: bool,
    session_id: &str,
) -> Result<Value, CuError> {
    let record = checked_owned_record(job_id, generation, session_id)?;
    let result = client_request(
        &record.handle(),
        ManagedJobOperation::Write {
            data_base64: data_base64.to_owned(),
            close_stdin,
        },
    )
    .map_err(client_error)?;
    match result {
        ManagedJobResult::Write { accepted_bytes, .. } => Ok(json!({
            "job_id": job_id,
            "generation": generation,
            "accepted_bytes": accepted_bytes,
            "delivery": "complete",
            "stdin_closed": close_stdin,
        })),
        _ => Err(response_kind_error()),
    }
}

pub(super) fn job_renew_payload(
    job_id: &str,
    generation: u64,
    ttl_seconds: u64,
    session_id: &str,
) -> Result<Value, CuError> {
    let record = checked_owned_record(job_id, generation, session_id)?;
    let ttl_ms = ttl_seconds
        .checked_mul(1_000)
        .ok_or_else(|| CuError::new("managed_job_ttl_invalid", "TTL overflows milliseconds"))?;
    match client_request(&record.handle(), ManagedJobOperation::Renew { ttl_ms })
        .map_err(client_error)?
    {
        ManagedJobResult::Renew { renewed_ttl_ms } => Ok(json!({
            "job_id": job_id,
            "generation": generation,
            "renewed_ttl_ms": renewed_ttl_ms,
        })),
        _ => Err(response_kind_error()),
    }
}

/// What one cooperative round proved.
///
/// The classification is deliberately richer than `Result`: a round that ran
/// out of its own budget proved nothing, while a round that failed with a typed
/// transport or protocol error proved something that must outrank a
/// cancellation sample taken in the same round.
enum WaitRound {
    /// A complete reply frame that passed schema and request-identity validation.
    Reply(Box<ManagedJobResult>),
    /// The round exhausted its own absolute budget; nothing was proved.
    Inconclusive,
    /// A typed failure that was already formed when the round returned.
    Failed(ManagedJobProtocolError),
}

trait WaitRounds {
    fn round(&mut self, timeout_ms: u64, deadline: Instant) -> WaitRound;
}

/// The production round source: one bounded IPC exchange with the resident owner.
struct ResidentWaitRounds<'a> {
    handle: &'a ManagedJobHandle,
}

impl WaitRounds for ResidentWaitRounds<'_> {
    fn round(&mut self, timeout_ms: u64, deadline: Instant) -> WaitRound {
        match client_request_before(
            self.handle,
            ManagedJobOperation::Wait { timeout_ms },
            deadline,
        ) {
            Ok(result) => WaitRound::Reply(Box::new(result)),
            Err(error) if error.code == ROUND_TIMEOUT_CODE => WaitRound::Inconclusive,
            Err(error) => WaitRound::Failed(error),
        }
    }
}

/// The terminal or deadline-bounded observation `job-wait` reports.
#[derive(Debug)]
enum WaitOutcome {
    /// A complete, parsed terminal reply: the job finished.
    Terminal(JobStatus),
    /// The caller's deadline elapsed without a terminal reply.
    TimedOut(JobStatus),
}

/// Drives the cooperative wait loop.
///
/// Precedence at every boundary is fixed: a complete terminal reply wins, then
/// an already-formed typed error, then the call-scoped cancellation sample,
/// then the caller's absolute deadline; only then does the loop continue. The
/// cancellation check therefore happens strictly after the current round's
/// request, stream and callback have returned, and it never performs an effect
/// on the job.
fn drive_wait(
    rounds: &mut impl WaitRounds,
    deadline: Instant,
    control: ExecutionControl<'_>,
) -> Result<WaitOutcome, CuError> {
    let mut last: Option<JobStatus> = None;
    loop {
        let quantum = deadline
            .saturating_duration_since(Instant::now())
            .min(WAIT_QUANTUM);
        // One absolute deadline for this round's connect, writes, flush and
        // reads. It is never reset by a fragment, and the frozen budget caps it
        // so a slow host cannot turn one round into a hundreds-of-milliseconds
        // block.
        let round_deadline = Instant::now() + (quantum + WAIT_ROUND_MARGIN).min(WAIT_ROUND_BUDGET);
        let request_ms = u64::try_from(quantum.as_millis()).unwrap_or(0);
        match rounds.round(request_ms, round_deadline) {
            WaitRound::Reply(result) => match *result {
                ManagedJobResult::Wait { completed, status } => {
                    if completed {
                        return Ok(WaitOutcome::Terminal(status));
                    }
                    last = Some(status);
                }
                _ => return Err(response_kind_error()),
            },
            WaitRound::Failed(error) => return Err(client_error(error)),
            WaitRound::Inconclusive => {}
        }
        control.check_observe()?;
        if deadline.saturating_duration_since(Instant::now()).is_zero() {
            let status = last.ok_or_else(|| {
                CuError::new(
                    ROUND_TIMEOUT_CODE,
                    "managed-job wait ended before any status was observed",
                )
            })?;
            return Ok(WaitOutcome::TimedOut(status));
        }
    }
}

/// Maps a resolver result into the managed-job owner-launch contract.
///
/// It starts nothing: the caller refuses before any owner process exists. The
/// resolver is a parameter so a test can prove the refusal mapping without a
/// missing sibling on disk, and so production has exactly one choice of program.
fn owner_launch_program(
    resolve: impl FnOnce() -> Result<std::path::PathBuf, crate::owner_executable::OwnerExecutableError>,
) -> Result<std::path::PathBuf, CuError> {
    resolve().map_err(|error| {
        CuError::new(
            "managed_job_owner_spawn_failed",
            "the sibling agenterm-cu owner executable is unavailable",
        )
        .with_detail(json!({
            "reason": error.reason(),
            "io_kind": error.kind().map(|kind| format!("{kind:?}")),
            "resolution": "current_exe_sibling",
            "owner_started": false,
        }))
    })
}

/// Builds the resident managed-job owner command for an explicit program.
///
/// The launch document travels on stdin, so the internal owner argument is the
/// whole argv. An explicit program keeps the spawn boundary testable without
/// letting a test choose production's program.
fn owner_command(program: &std::path::Path) -> ProcessCommand {
    let mut command = ProcessCommand::new(program);
    command
        .arg(crate::MANAGED_JOB_OWNER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

pub(super) fn job_wait_payload(
    job_id: &str,
    generation: u64,
    timeout_ms: u64,
    expect_exit: Option<i32>,
    control: ExecutionControl<'_>,
) -> Result<Value, CuError> {
    // An already-cancelled call must be refused before `checked_record` opens
    // the store, because opening it creates the durable state parent. A
    // cancellation that arrives here must not leave a state root behind.
    control.check_observe()?;
    let record = checked_record(job_id, generation)?;
    let handle = record.handle();
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut rounds = ResidentWaitRounds { handle: &handle };
    let (completed, status) = match drive_wait(&mut rounds, deadline, control)? {
        WaitOutcome::Terminal(status) => (true, status),
        WaitOutcome::TimedOut(status) => (false, status),
    };
    verify_expected_exit(&status.state, completed, expect_exit)?;
    Ok(json!({
        "job_id": job_id,
        "generation": generation,
        "completed": completed,
        "status": status,
    }))
}

pub(super) fn job_prune_payload(
    max_age_seconds: u64,
    keep_newest: usize,
    apply: bool,
) -> Result<Value, CuError> {
    let max_age_ms = max_age_seconds
        .checked_mul(1_000)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| {
            CuError::new(
                "managed_job_prune_invalid",
                "managed-job prune age overflows milliseconds",
            )
        })?;
    let now = now_utc_ms().ok_or_else(clock_error)?;
    let report = ManagedJobStore::open()?.prune(max_age_ms, keep_newest, apply, now)?;
    serde_json::to_value(report).map_err(|_| response_kind_error())
}

pub(super) fn job_set_state_payload(
    job_id: &str,
    generation: u64,
    state: crate::command::ProcessRunState,
    timeout_ms: u64,
    session_id: &str,
    receipts: &mut crate::receipt::ReceiptLog,
) -> Result<Value, CuError> {
    let signal = match state {
        crate::command::ProcessRunState::Running => crate::command::ProcessSignalKind::Continue,
        crate::command::ProcessRunState::Stopped => crate::command::ProcessSignalKind::Stop,
    };
    let mut payload = job_tree_signal(
        job_id, generation, signal, timeout_ms, false, session_id, receipts,
    )?;
    let object = payload.as_object_mut().ok_or_else(response_kind_error)?;
    object.insert("job_id".into(), json!(job_id));
    object.insert("generation".into(), json!(generation));
    object.insert("requested_state".into(), json!(state.as_str()));
    Ok(payload)
}

pub(super) fn job_priority_payload(
    job_id: &str,
    generation: u64,
    nice: i32,
    session_id: &str,
    receipts: &mut crate::receipt::ReceiptLog,
) -> Result<Value, CuError> {
    #[cfg(windows)]
    {
        let _ = (job_id, generation, nice, session_id, receipts);
        Err(CuError::new(
            "managed_job_priority_unsupported",
            "Windows priority classes do not provide the Unix process-group nice model",
        ))
    }
    #[cfg(unix)]
    {
        let record = checked_owned_record(job_id, generation, session_id)?;
        let process = record.process.as_ref().ok_or_else(|| {
            CuError::new(
                "managed_job_process_identity_missing",
                "managed-job durable record has no exact process identity",
            )
        })?;
        let before_snapshot = resource_point_payload(&record, process)?;
        let before_members = before_snapshot["members"]
            .as_array()
            .ok_or_else(response_kind_error)?;
        let ticket = receipts.reserve(
            "job-priority",
            0,
            json!({
                "job_id": job_id,
                "generation": generation,
                "nice": nice,
                "process": process,
                "before": {
                    "membership_sha256": before_snapshot["membership_sha256"],
                    "member_count": before_members.len(),
                },
            }),
        )?;
        let result = match client_request(&record.handle(), ManagedJobOperation::Priority { nice })
        {
            Ok(ManagedJobResult::Priority { result }) => result,
            Ok(_) => {
                let error = response_kind_error();
                receipts.complete(
                    &ticket,
                    "job-priority",
                    0,
                    false,
                    json!({"effect": "unknown", "retry_safe": false, "error": error.code}),
                )?;
                return Err(error.with_detail(json!({
                    "effect": "unknown",
                    "retry_safe": false,
                    "receipt": ticket.json(),
                })));
            }
            Err(source) => {
                let error = client_error(source);
                let unknown = error.code == "managed_job_priority_outcome_unknown";
                receipts.complete(
                    &ticket,
                    "job-priority",
                    0,
                    false,
                    json!({
                        "effect": if unknown { "unknown" } else { "not_performed" },
                        "retry_safe": false,
                        "error": error.code,
                    }),
                )?;
                return Err(error.with_detail(json!({
                    "effect": if unknown { "unknown" } else { "not_performed" },
                    "retry_safe": false,
                    "receipt": ticket.json(),
                })));
            }
        };
        let membership_sha256 = priority_membership_digest(&result.after);
        let verified = !result.after.is_empty()
            && result.before.len() == result.after.len()
            && result
                .before
                .iter()
                .zip(&result.after)
                .all(|(before, after)| {
                    before.pid == after.pid
                        && before.start_identity == after.start_identity
                        && after.nice == nice
                });
        receipts.complete(
            &ticket,
            "job-priority",
            0,
            verified,
            json!({
                "effect": "performed",
                "after": {"membership_sha256": membership_sha256, "nice": nice},
                "verification": "stable-native-containment-membership-and-per-member-nice",
            }),
        )?;
        if !verified {
            return Err(CuError::new(
                "managed_job_priority_outcome_unknown",
                "managed-job priority write could not be attributed to one stable member set",
            )
            .with_detail(json!({
                "effect": "unknown",
                "retry_safe": false,
                "receipt": ticket.json(),
            })));
        }
        Ok(json!({
            "job_id": job_id,
            "generation": generation,
            "provider": result.provider,
            "requested_nice": nice,
            "membership_sha256": membership_sha256,
            "before": result.before,
            "after": result.after,
            "performed": true,
            "verified": true,
            "receipt": ticket.json(),
        }))
    }
}

pub(super) fn job_signal_payload(
    job_id: &str,
    generation: u64,
    signal: crate::command::ProcessSignalKind,
    timeout_ms: u64,
    force: bool,
    session_id: &str,
    receipts: &mut crate::receipt::ReceiptLog,
) -> Result<Value, CuError> {
    let mut payload = job_tree_signal(
        job_id, generation, signal, timeout_ms, force, session_id, receipts,
    )?;
    let object = payload.as_object_mut().ok_or_else(response_kind_error)?;
    object.insert("job_id".into(), json!(job_id));
    object.insert("generation".into(), json!(generation));
    Ok(payload)
}

fn job_tree_signal(
    job_id: &str,
    generation: u64,
    signal: crate::command::ProcessSignalKind,
    timeout_ms: u64,
    force: bool,
    session_id: &str,
    receipts: &mut crate::receipt::ReceiptLog,
) -> Result<Value, CuError> {
    let record = checked_owned_record(job_id, generation, session_id)?;
    let process = record.process.as_ref().ok_or_else(|| {
        CuError::new(
            "managed_job_process_identity_missing",
            "managed-job durable record has no exact process identity",
        )
    })?;
    process_signal_payload(
        process.pid,
        Some(&process.start_identity),
        signal,
        ProcessSignalOptions {
            timeout_ms,
            force,
            tree: true,
            max_descendants: JOB_RESOURCE_MAX_MEMBERS,
        },
        receipts,
    )
}

pub(super) fn job_stop_payload(
    job_id: &str,
    generation: u64,
    grace_ms: u64,
    expect_stopped: bool,
    session_id: &str,
) -> Result<Value, CuError> {
    let record = checked_owned_record(job_id, generation, session_id)?;
    if grace_ms > 0 {
        let _ = client_request(
            &record.handle(),
            ManagedJobOperation::Write {
                data_base64: String::new(),
                close_stdin: true,
            },
        );
        if let Ok(ManagedJobResult::Wait {
            completed: true,
            status,
        }) = client_request(
            &record.handle(),
            ManagedJobOperation::Wait {
                timeout_ms: grace_ms.min(300_000),
            },
        ) {
            return stopped_payload(job_id, generation, status, expect_stopped);
        }
    }
    match client_request(&record.handle(), ManagedJobOperation::Stop).map_err(client_error)? {
        ManagedJobResult::Stop { status } => {
            stopped_payload(job_id, generation, status, expect_stopped)
        }
        _ => Err(response_kind_error()),
    }
}

pub(super) fn job_events_payload(
    job_id: &str,
    generation: u64,
    stdout_cursor: &JobOutputCursor,
    stderr_cursor: &JobOutputCursor,
    timeout_ms: u64,
    max_bytes: usize,
) -> Result<Value, CuError> {
    let record = checked_record(job_id, generation)?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let stdout_budget = max_bytes.div_ceil(2);
    let stderr_budget = max_bytes / 2;
    loop {
        let stdout = collect_output(
            &record,
            OutputStream::Stdout,
            stdout_cursor.value(),
            stdout_budget,
        )?;
        let stderr = collect_output(
            &record,
            OutputStream::Stderr,
            stderr_cursor.value(),
            stderr_budget,
        )?;
        let status = live_status(&record)?;
        let changed =
            stdout["next_cursor"] != stdout["cursor"] || stderr["next_cursor"] != stderr["cursor"];
        let terminal = !matches!(status.state, JobState::Running);
        if changed || terminal || Instant::now() >= deadline {
            return Ok(json!({
                "job_id": job_id,
                "generation": generation,
                "stdout": stdout,
                "stderr": stderr,
                "status": status,
                "timed_out": !changed && !terminal && Instant::now() >= deadline,
            }));
        }
        thread::sleep(START_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

pub(super) fn job_output_payload(
    job_id: &str,
    generation: u64,
    stream: JobOutputStream,
    cursor: &JobOutputCursor,
    max_bytes: usize,
) -> Result<Value, CuError> {
    let record = checked_record(job_id, generation)?;
    let output = collect_output(
        &record,
        match stream {
            JobOutputStream::Stdout => OutputStream::Stdout,
            JobOutputStream::Stderr => OutputStream::Stderr,
        },
        cursor.value(),
        max_bytes,
    )?;
    let status = live_status(&record)?;
    Ok(json!({
        "job_id": job_id,
        "generation": generation,
        "stream": match stream {
            JobOutputStream::Stdout => "stdout",
            JobOutputStream::Stderr => "stderr",
        },
        "output": output,
        "status": status,
    }))
}

fn collect_output(
    record: &ManagedJobRecord,
    stream: OutputStream,
    mut cursor: u64,
    budget: usize,
) -> Result<Value, CuError> {
    let requested = cursor;
    let mut bytes = Vec::with_capacity(budget);
    let mut current = cursor;
    let mut finalized = false;
    let mut read_error = None;
    while bytes.len() < budget {
        let limit = (budget - bytes.len()).min(IPC_PAGE_BYTES);
        let result = client_request(
            &record.handle(),
            ManagedJobOperation::Output {
                stream,
                cursor,
                max_bytes: limit,
            },
        )
        .map_err(client_error)?;
        let ManagedJobResult::Output {
            next_cursor,
            current_cursor,
            data_base64,
            finalized: page_finalized,
            read_error: page_error,
            ..
        } = result
        else {
            return Err(response_kind_error());
        };
        let page = base64_decode(&data_base64).map_err(|_| response_kind_error())?;
        bytes.extend_from_slice(&page);
        cursor = next_cursor;
        current = current_cursor;
        finalized = page_finalized;
        read_error = page_error;
        if page.is_empty() || finalized || cursor >= current {
            break;
        }
    }
    Ok(json!({
        "cursor": requested.to_string(),
        "next_cursor": cursor.to_string(),
        "current_cursor": current.to_string(),
        "data_base64": base64_encode(&bytes),
        "bytes": bytes.len(),
        "finalized": finalized,
        "read_error": read_error,
    }))
}

fn live_status(record: &ManagedJobRecord) -> Result<crate::managed_job_ipc::JobStatus, CuError> {
    match client_request(&record.handle(), ManagedJobOperation::Status).map_err(client_error)? {
        ManagedJobResult::Status { status } => Ok(status),
        _ => Err(response_kind_error()),
    }
}

fn stopped_payload(
    job_id: &str,
    generation: u64,
    status: crate::managed_job_ipc::JobStatus,
    expect_stopped: bool,
) -> Result<Value, CuError> {
    let stopped = !matches!(status.state, JobState::Running);
    if expect_stopped && !stopped {
        return Err(CuError::new(
            "managed_job_stop_unverified",
            "resident owner did not verify a terminal child state",
        ));
    }
    Ok(json!({
        "job_id": job_id,
        "generation": generation,
        "stopped": stopped,
        "status": status,
    }))
}

fn verify_expected_exit(
    state: &JobState,
    completed: bool,
    expected: Option<i32>,
) -> Result<(), CuError> {
    if let Some(expected) = expected {
        match state {
            JobState::Exited { exit_code } if completed && *exit_code == expected => {}
            _ => {
                return Err(CuError::new(
                    "managed_job_exit_mismatch",
                    "managed-job did not complete with the expected exit code",
                ));
            }
        }
    }
    Ok(())
}

fn checked_record(job_id: &str, generation: u64) -> Result<ManagedJobRecord, CuError> {
    let store = ManagedJobStore::open()?;
    let record = required_record(&store, job_id)?;
    if record.generation != generation {
        return Err(CuError::new(
            "managed_job_identity_changed",
            "managed-job generation no longer matches",
        ));
    }
    Ok(record)
}

fn checked_owned_record(
    job_id: &str,
    generation: u64,
    session_id: &str,
) -> Result<ManagedJobRecord, CuError> {
    let record = checked_record(job_id, generation)?;
    require_owning_session(&record, session_id)?;
    Ok(record)
}

fn require_owning_session(record: &ManagedJobRecord, session_id: &str) -> Result<(), CuError> {
    if record.session_id.as_deref() == Some(session_id) {
        return Ok(());
    }
    Err(CuError::new(
        "managed_job_session_mismatch",
        "managed-job mutation session does not own this job",
    ))
}

fn required_record(store: &ManagedJobStore, job_id: &str) -> Result<ManagedJobRecord, CuError> {
    store
        .get(job_id)?
        .ok_or_else(|| CuError::new("managed_job_not_found", "managed-job record does not exist"))
}

fn reconcile_unavailable_owner(
    store: &ManagedJobStore,
    record: &ManagedJobRecord,
) -> Result<ManagedJobRecord, CuError> {
    let Some(expected) = record.owner.as_ref() else {
        return Ok(record.clone());
    };
    let liveness = match agenterm_platform::process_observation::observe(expected.pid) {
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: Some(start_identity),
        } => OwnerLiveness::Live(ResidentOwnerIdentity {
            pid: expected.pid,
            start_identity,
        }),
        agenterm_platform::process_observation::ProcessObservation::Dead { .. } => {
            OwnerLiveness::Dead
        }
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: None,
        }
        | agenterm_platform::process_observation::ProcessObservation::Unknown { .. }
        | _ => OwnerLiveness::Unknown,
    };
    store.reconcile_owner(
        &record.handle(),
        liveness,
        now_utc_ms().ok_or_else(clock_error)?,
    )?;
    required_record(store, &record.job_id)
}

fn state_matches(filter: JobStateFilter, state: &ManagedJobState) -> bool {
    matches!(
        (filter, state),
        (JobStateFilter::StartIntent, ManagedJobState::StartIntent)
            | (JobStateFilter::Starting, ManagedJobState::Starting)
            | (JobStateFilter::Running, ManagedJobState::Running)
            | (
                JobStateFilter::StartFailed,
                ManagedJobState::StartFailed { .. }
            )
            | (JobStateFilter::Exited, ManagedJobState::Exited { .. })
            | (JobStateFilter::Signaled, ManagedJobState::Signaled { .. })
            | (JobStateFilter::Detached, ManagedJobState::Detached)
            | (
                JobStateFilter::OrphanedUncertain,
                ManagedJobState::OrphanedUncertain
            )
    )
}

fn record_payload(
    record: &ManagedJobRecord,
    live: Option<&crate::managed_job_ipc::JobStatus>,
) -> Value {
    let (state, terminal) = match &record.state {
        ManagedJobState::StartIntent => ("start_intent", None),
        ManagedJobState::Starting => ("starting", None),
        ManagedJobState::Running => ("running", None),
        ManagedJobState::StartFailed { code } => ("start_failed", Some(json!({"code": code}))),
        ManagedJobState::Exited { exit_code } => ("exited", Some(json!({"exit_code": exit_code}))),
        ManagedJobState::Signaled { signal } => ("signaled", Some(json!({"signal": signal}))),
        ManagedJobState::Detached => ("detached", None),
        ManagedJobState::OrphanedUncertain => ("orphaned_uncertain", None),
    };
    let origin = match record.origin {
        ManagedJobOrigin::Spawned => "spawned",
        ManagedJobOrigin::Adopted => "adopted",
    };
    // Historical records have none of the three policy/terminal fields. They
    // must project explicit null -- never a default `stop`, never `live` -- and
    // their absence must not make an older record unreadable.
    let on_expiry = record.on_expiry.map(ManagedJobOnExpiry::as_str);
    let terminal_trigger = record
        .terminal_trigger
        .map(ManagedJobTerminalTrigger::as_str);
    let detach_liveness = record.detach_liveness.map(ManagedJobDetachLiveness::as_str);
    json!({
        "job_id": record.job_id,
        "generation": record.generation,
        "session_id": record.session_id,
        "origin": origin,
        "state": state,
        "terminal": terminal,
        "created_at_utc_ms": record.created_at_utc_ms,
        "updated_at_utc_ms": record.updated_at_utc_ms,
        "terminal_at_utc_ms": record.terminal_at_utc_ms,
        "on_expiry": on_expiry,
        "terminal_trigger": terminal_trigger,
        "detach_liveness": detach_liveness,
        "owner_pid": record.owner.as_ref().map(|owner| owner.pid),
        "process_pid": record.process.as_ref().map(|process| process.pid),
        "io_available": live.is_some_and(|status| !status.adopted),
        "live": live,
    })
}

fn client_error(error: ManagedJobProtocolError) -> CuError {
    let mut mapped = CuError::new(error.code.clone(), "managed-job resident request failed");
    if error.delivery_uncertain == Some(true) {
        mapped.detail = Some(json!({
            "delivery_uncertain": true,
            "known_written_lower_bound": error.known_written_lower_bound,
            "retry_safe": false,
        }));
    }
    mapped
}

fn response_kind_error() -> CuError {
    CuError::new(
        "managed_job_response_invalid",
        "resident owner returned a mismatched response kind",
    )
}

/// One `job-spawn`'s launch inputs, grouped so the builder takes a single value
/// instead of growing a long positional argument list as policies are added.
struct SpawnRequest<'a> {
    command: &'a [String],
    environment: &'a [JobEnvironment],
    cwd: Option<&'a str>,
    limits: Option<JobProcessLimits>,
    ttl_seconds: u64,
    on_expiry: JobExpiry,
}

fn build_launch(
    store: &ManagedJobStore,
    record: &ManagedJobRecord,
    request: SpawnRequest<'_>,
) -> Result<ManagedJobLaunch, CuError> {
    let SpawnRequest {
        command,
        environment,
        cwd,
        limits,
        ttl_seconds,
        on_expiry,
    } = request;
    let current_directory = resolve_directory(cwd)?;
    let program = resolve_program(&command[0], current_directory.as_deref(), environment)?;
    let lease_ttl_ms = ttl_seconds.checked_mul(1_000).ok_or_else(|| {
        CuError::new(
            "managed_job_ttl_invalid",
            "managed-job TTL overflows milliseconds",
        )
    })?;
    Ok(ManagedJobLaunch {
        schema_version: LAUNCH_SCHEMA_VERSION,
        state_path: store.path().to_owned(),
        handle: record.handle(),
        program,
        arguments: command[1..].to_vec(),
        current_directory,
        environment: environment
            .iter()
            .map(|entry| ManagedJobEnvironment {
                name: entry.name.clone(),
                value: entry.value.clone(),
            })
            .collect(),
        limits: limits.map(|limits| ManagedJobProcessLimits {
            cpu_seconds: limits.cpu_seconds,
            memory_bytes: limits.memory_bytes,
            file_size_bytes: limits.file_size_bytes,
            open_files: limits.open_files,
            processes: limits.processes,
        }),
        adoption: None,
        on_expiry: managed_on_expiry(on_expiry),
        output_capacity_bytes: OUTPUT_CAPACITY_BYTES,
        lease_ttl_ms,
    })
}

fn resolve_directory(cwd: Option<&str>) -> Result<Option<PathBuf>, CuError> {
    let Some(cwd) = cwd else {
        return Ok(None);
    };
    let path = PathBuf::from(cwd);
    let candidate = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map_err(|_| cwd_error())?.join(path)
    };
    let canonical = fs::canonicalize(candidate).map_err(|_| cwd_error())?;
    if !canonical.is_dir() {
        return Err(cwd_error());
    }
    Ok(Some(canonical))
}

fn resolve_program(
    raw: &str,
    cwd: Option<&Path>,
    environment: &[JobEnvironment],
) -> Result<PathBuf, CuError> {
    let path = PathBuf::from(raw);
    if path.is_absolute() || path.components().count() > 1 {
        let candidate = if path.is_absolute() {
            path
        } else {
            cwd.map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok())
                .ok_or_else(program_error)?
                .join(path)
        };
        return canonical_program(candidate);
    }
    let search = effective_environment("PATH", environment)
        .ok_or_else(program_error)
        .map(OsString::from)?;
    for directory in std::env::split_paths(&search) {
        let directory = if directory.is_absolute() {
            directory
        } else {
            cwd.map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok())
                .ok_or_else(program_error)?
                .join(directory)
        };
        for name in executable_names(OsStr::new(raw), environment) {
            let candidate = directory.join(name);
            if let Ok(path) = canonical_program(candidate) {
                return Ok(path);
            }
        }
    }
    Err(program_error())
}

fn effective_environment(name: &str, entries: &[JobEnvironment]) -> Option<String> {
    entries
        .iter()
        .rev()
        .find(|entry| {
            if cfg!(windows) {
                entry.name.eq_ignore_ascii_case(name)
            } else {
                entry.name == name
            }
        })
        .map(|entry| entry.value.clone())
        .unwrap_or_else(|| std::env::var(name).ok())
}

fn executable_names(name: &OsStr, environment: &[JobEnvironment]) -> Vec<OsString> {
    #[cfg(not(windows))]
    {
        let _ = environment;
        vec![name.to_owned()]
    }
    #[cfg(windows)]
    {
        let path = Path::new(name);
        if path.extension().is_some() {
            return vec![name.to_owned()];
        }
        let extensions = effective_environment("PATHEXT", environment)
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        extensions
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| {
                let mut value = name.to_os_string();
                value.push(extension);
                value
            })
            .collect()
    }
}

fn canonical_program(path: PathBuf) -> Result<PathBuf, CuError> {
    let canonical = fs::canonicalize(path).map_err(|_| program_error())?;
    if canonical.is_file() {
        Ok(canonical)
    } else {
        Err(program_error())
    }
}

fn detach_reaper(mut child: std::process::Child) -> Result<(), CuError> {
    agenterm_platform::threading::spawn_named_detached(
        "agenterm-cu-managed-job-owner-reaper",
        Box::new(move || {
            let _ = child.wait();
        }),
    )
    .map_err(|_| outcome_unknown("resident owner reaper could not start"))
}

fn mark_clean_owner_failure(
    store: &ManagedJobStore,
    record: &ManagedJobRecord,
    code: &str,
    fallback_now: i64,
) -> Result<(), CuError> {
    store
        .mark_unclaimed_start_failed(&record.handle(), code, now_utc_ms().unwrap_or(fallback_now))
        .map(|_| ())
}

fn classify_post_spawn_failure(
    store: &ManagedJobStore,
    record: &ManagedJobRecord,
    clean_code: &str,
    fallback_now: i64,
) -> Result<Value, CuError> {
    match store.get(&record.job_id)? {
        Some(current) if current.state == ManagedJobState::StartIntent => {
            mark_clean_owner_failure(store, record, clean_code, fallback_now)?;
            Err(CuError::new(
                "managed_job_owner_start_failed",
                "resident owner exited before claiming the managed-job intent",
            ))
        }
        _ => Err(outcome_unknown(
            "managed-job startup may have crossed the contained spawn boundary",
        )),
    }
}

fn outcome_unknown(message: &'static str) -> CuError {
    CuError::new("managed_job_outcome_unknown", message).with_detail(json!({
        "effect": "unknown",
        "retry_safe": false,
    }))
}

fn program_error() -> CuError {
    CuError::new(
        "managed_job_program_unavailable",
        "managed-job program could not be resolved to an existing file",
    )
}

fn cwd_error() -> CuError {
    CuError::new(
        "managed_job_cwd_unavailable",
        "managed-job working directory is unavailable",
    )
}

fn clock_error() -> CuError {
    CuError::new(
        "managed_job_clock_invalid",
        "managed-job system clock is unavailable",
    )
}

fn replay_error() -> CuError {
    CuError::new(
        "managed_job_replay_projection_invalid",
        "managed-job spawn reply cannot be sealed for exact replay",
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    fn running_status() -> JobStatus {
        JobStatus {
            state: JobState::Running,
            adopted: false,
            stdin_open: true,
            lease_remaining_ms: 1_000,
            stdout_earliest_cursor: 0,
            stdout_current_cursor: 0,
            stderr_earliest_cursor: 0,
            stderr_current_cursor: 0,
        }
    }

    fn exited_status(exit_code: i32) -> JobStatus {
        JobStatus {
            state: JobState::Exited { exit_code },
            ..running_status()
        }
    }

    /// One scripted round outcome.
    enum Step {
        Running,
        Terminal(i32),
        WrongKind,
        Inconclusive,
        Failed(&'static str),
    }

    /// A round source with no IPC, so the wait state machine is exercised purely.
    struct Scripted {
        steps: Vec<Step>,
        timeouts: Vec<u64>,
        cancel_after: Option<usize>,
        flag: Arc<AtomicBool>,
        completed: usize,
        /// The largest budget any round was handed, so the frozen production
        /// bound is observable rather than only implicitly asserted.
        max_round_budget: Duration,
    }

    impl Scripted {
        fn new(steps: Vec<Step>) -> Self {
            Self {
                steps,
                timeouts: Vec::new(),
                cancel_after: None,
                flag: Arc::new(AtomicBool::new(false)),
                completed: 0,
                max_round_budget: Duration::ZERO,
            }
        }

        /// Sets the cancellation probe after `rounds` rounds have returned, which
        /// models a cancel that arrives while the next round would be in flight.
        fn cancelling_after(mut self, rounds: usize) -> Self {
            self.cancel_after = Some(rounds);
            self
        }

        fn probe(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.flag)
        }
    }

    impl WaitRounds for Scripted {
        fn round(&mut self, timeout_ms: u64, deadline: Instant) -> WaitRound {
            // Every round carries one absolute deadline that no fragment resets.
            let budget = deadline.saturating_duration_since(Instant::now());
            self.max_round_budget = self.max_round_budget.max(budget);
            assert!(
                budget <= WAIT_QUANTUM + WAIT_ROUND_MARGIN,
                "a round deadline must stay inside one quantum plus its transport margin"
            );
            self.timeouts.push(timeout_ms);
            let index = self.completed.min(self.steps.len() - 1);
            self.completed += 1;
            if self.cancel_after == Some(self.completed) {
                self.flag.store(true, Ordering::Relaxed);
            }
            match &self.steps[index] {
                Step::Running => WaitRound::Reply(Box::new(ManagedJobResult::Wait {
                    completed: false,
                    status: running_status(),
                })),
                Step::Terminal(exit_code) => WaitRound::Reply(Box::new(ManagedJobResult::Wait {
                    completed: true,
                    status: exited_status(*exit_code),
                })),
                Step::WrongKind => WaitRound::Reply(Box::new(ManagedJobResult::Status {
                    status: running_status(),
                })),
                Step::Inconclusive => WaitRound::Inconclusive,
                Step::Failed(code) => WaitRound::Failed(ManagedJobProtocolError {
                    code: (*code).to_owned(),
                    known_written_lower_bound: None,
                    delivery_uncertain: None,
                }),
            }
        }
    }

    fn control(flag: &Arc<AtomicBool>) -> impl Fn() -> bool + '_ {
        let flag = Arc::clone(flag);
        move || flag.load(Ordering::Relaxed)
    }

    /// P0: the production wait loop hands every round the frozen absolute budget
    /// and never a wider one, so a fast healthy path cannot conceal a failure
    /// path that would block for hundreds of milliseconds.
    #[test]
    fn wait_every_round_deadline_stays_within_the_frozen_budget() {
        let mut scripted = Scripted::new(vec![Step::Running]);
        let flag = scripted.probe();
        let probe = control(&flag);
        let outcome = drive_wait(
            &mut scripted,
            Instant::now() + Duration::from_millis(120),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect("a deadline-bounded wait returns its last snapshot");
        assert!(matches!(outcome, WaitOutcome::TimedOut(_)));
        assert!(
            scripted.timeouts.len() >= 3,
            "a 120ms wait must be sliced into several bounded rounds"
        );
        assert!(
            scripted.max_round_budget <= WAIT_ROUND_BUDGET,
            "every round must stay inside the frozen budget, observed {:?}",
            scripted.max_round_budget
        );
        assert!(
            scripted.max_round_budget > Duration::ZERO,
            "the bound must be a real observed value, not an untested constant"
        );
    }

    /// P0: one cooperative round must return far inside the worker's hard-cancel
    /// grace (`cancel_grace_ms: 150` in `src/script_catalog.rs`), because that is
    /// the deadline after which the supervisor stops waiting for the callback and
    /// terminates the worker instead.
    #[test]
    fn wait_slice_and_margin_stay_inside_the_worker_cancel_grace() {
        let grace = Duration::from_millis(150);
        assert!(
            WAIT_QUANTUM + WAIT_ROUND_MARGIN < grace,
            "one wait round must finish well before the worker's hard-cancel grace"
        );
        assert!(WAIT_QUANTUM > Duration::ZERO);
    }

    /// P0: an already-cancelled call must be refused before the store is opened,
    /// because opening it creates the durable state parent as a side effect.
    #[test]
    fn wait_entry_cancellation_refuses_before_the_store_is_consulted() {
        let flag = Arc::new(AtomicBool::new(true));
        let probe = control(&flag);
        let error = job_wait_payload(
            "00000000-0000-4000-8000-00000000dead",
            1,
            0,
            None,
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect_err("a pre-cancelled wait must refuse");
        assert_eq!(error.code, "cancelled");
        let detail = error.detail.expect("typed cancellation detail");
        assert_eq!(detail["effect"], "not_performed");
        assert_eq!(detail["phase"], "observe_wait");
    }

    /// A complete terminal reply is accepted even when the same round also
    /// observed cancellation: parsed truth outranks the robustness signal.
    #[test]
    fn wait_terminal_reply_outranks_a_cancellation_in_the_same_round() {
        let scripted = Scripted::new(vec![Step::Terminal(0)]).cancelling_after(1);
        let flag = scripted.probe();
        let probe = control(&flag);
        let mut scripted = scripted;
        let outcome = drive_wait(
            &mut scripted,
            Instant::now() + Duration::from_secs(5),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect("a complete terminal reply must be accepted");
        match outcome {
            WaitOutcome::Terminal(status) => {
                assert!(matches!(status.state, JobState::Exited { exit_code: 0 }));
            }
            WaitOutcome::TimedOut(_) => {
                panic!("a terminal reply must not be reported as a timeout")
            }
        }
    }

    /// An incomplete round is followed by the cancellation sample, and no further
    /// round may start once that sample is observed.
    #[test]
    fn wait_incomplete_then_cancel_returns_cancelled_without_another_round() {
        let mut scripted = Scripted::new(vec![Step::Running]).cancelling_after(1);
        let flag = scripted.probe();
        let probe = control(&flag);
        let error = drive_wait(
            &mut scripted,
            Instant::now() + Duration::from_secs(30),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect_err("a cancelled wait must not report an outcome");
        assert_eq!(error.code, "cancelled");
        let detail = error.detail.expect("typed cancellation detail");
        assert_eq!(detail["effect"], "not_performed");
        assert_eq!(detail["phase"], "observe_wait");
        assert_eq!(
            scripted.timeouts.len(),
            1,
            "no round may start after the cancellation is observed"
        );
    }

    /// Cancellation answers without any terminal claim when the terminal state was
    /// never observed, so a cancel can never masquerade as job progress.
    #[test]
    fn wait_cancellation_never_fabricates_a_terminal_claim() {
        let mut scripted = Scripted::new(vec![Step::Running]).cancelling_after(1);
        let flag = scripted.probe();
        let probe = control(&flag);
        let error = drive_wait(
            &mut scripted,
            Instant::now() + Duration::from_secs(30),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect_err("a cancelled wait must not report an outcome");
        assert!(error.detail.is_some());
        assert_eq!(error.code, "cancelled");
    }

    /// Cancellation outranks an already-expired deadline.
    #[test]
    fn wait_cancellation_outranks_an_expired_deadline() {
        let mut scripted = Scripted::new(vec![Step::Running]);
        let flag = scripted.probe();
        flag.store(true, Ordering::Relaxed);
        let probe = control(&flag);
        let error = drive_wait(
            &mut scripted,
            Instant::now(),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect_err("cancellation must win the deadline race");
        assert_eq!(error.code, "cancelled");
    }

    /// A typed transport error that already formed outranks a cancellation
    /// observed in the same round; it must never be washed into `cancelled`.
    #[test]
    fn wait_typed_error_outranks_a_cancellation_in_the_same_round() {
        let mut scripted =
            Scripted::new(vec![Step::Failed("managed_job_protocol_io")]).cancelling_after(1);
        let flag = scripted.probe();
        let probe = control(&flag);
        let error = drive_wait(
            &mut scripted,
            Instant::now() + Duration::from_secs(30),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect_err("a typed transport error must be reported");
        assert_eq!(error.code, "managed_job_protocol_io");
    }

    /// Without cancellation the deadline still reports the last observed status,
    /// sliced rather than issued as one long blocking wait.
    #[test]
    fn wait_deadline_without_cancellation_reports_the_last_status_in_slices() {
        let mut scripted = Scripted::new(vec![Step::Running]);
        let flag = scripted.probe();
        let probe = control(&flag);
        let outcome = drive_wait(
            &mut scripted,
            Instant::now() + Duration::from_millis(30),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect("a deadline-bounded wait returns its last snapshot");
        assert!(matches!(outcome, WaitOutcome::TimedOut(_)));
        let quantum_ms = u64::try_from(WAIT_QUANTUM.as_millis()).expect("quantum fits u64");
        assert!(
            scripted.timeouts.iter().all(|ms| *ms <= quantum_ms),
            "every round must request at most one quantum"
        );
        assert!(
            scripted.timeouts.len() >= 2,
            "a 30ms deadline must be sliced, not issued as one long wait"
        );
    }

    /// A round that exhausted only its own budget proved nothing, so no status is
    /// invented for it.
    #[test]
    fn wait_inconclusive_rounds_never_fabricate_a_status() {
        let mut scripted = Scripted::new(vec![Step::Inconclusive]);
        let flag = scripted.probe();
        let probe = control(&flag);
        let error = drive_wait(
            &mut scripted,
            Instant::now(),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect_err("an unobserved status must not be reported as a timeout snapshot");
        assert_eq!(error.code, ROUND_TIMEOUT_CODE);
    }

    #[test]
    fn wait_rejects_a_mismatched_reply_kind() {
        let mut scripted = Scripted::new(vec![Step::WrongKind]);
        let flag = scripted.probe();
        let probe = control(&flag);
        let error = drive_wait(
            &mut scripted,
            Instant::now() + Duration::from_millis(30),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect_err("a non-wait reply must be refused");
        assert_eq!(error.code, "managed_job_response_invalid");
    }

    /// P0 measurement seam.
    ///
    /// A real AF_UNIX round against a peer that loops exactly like the resident
    /// owner (`accept` with the production tick, one request per connection)
    /// measures the two quantities the public reply does not carry: one
    /// cooperative round's cost, and the interval from the cancellation token
    /// being set to the callback returning.
    #[cfg(unix)]
    #[test]
    fn wait_round_cost_and_cancel_return_are_measured_on_a_real_socket() {
        use std::sync::Mutex;

        use agenterm_platform::ipc::NativeListener;

        let handle = ManagedJobHandle {
            job_id: "00000000-0000-4000-8000-0000000000aa".to_owned(),
            generation: 1,
            nonce: "b".repeat(32),
        };
        let endpoint = crate::managed_job_ipc::endpoint_for(&handle).expect("derived endpoint");
        let socket_path = endpoint.unix_socket_path().expect("unix socket endpoint");
        let mut listener = NativeListener::bind(&endpoint).expect("bind scripted owner");
        let limits = crate::deadline_frame_io::FrameLimits {
            max_bytes: crate::managed_job_ipc::FRAME_MAX_BYTES,
            little_endian: false,
        };
        let serving = Arc::new(AtomicBool::new(true));
        let stop = Arc::clone(&serving);
        let server = std::thread::spawn(move || {
            let mut rounds = 0_u64;
            while stop.load(Ordering::Relaxed) {
                let Ok(mut stream) = listener.accept(Duration::from_millis(100)) else {
                    continue;
                };
                let budget = Instant::now() + Duration::from_millis(500);
                let Ok(body) =
                    crate::deadline_frame_io::read_frame_before(&mut stream, budget, limits)
                else {
                    continue;
                };
                let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let reply = crate::managed_job_ipc::ManagedJobReply {
                    schema_version: 3,
                    request_id: request["request_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    ok: true,
                    result: Some(ManagedJobResult::Wait {
                        completed: false,
                        status: running_status(),
                    }),
                    error: None,
                };
                let encoded = serde_json::to_vec(&reply).expect("reply json");
                let _ = crate::deadline_frame_io::write_frame_before(
                    &mut stream,
                    &encoded,
                    budget,
                    limits,
                );
                let _ = crate::deadline_frame_io::flush_before(&mut stream, budget);
                rounds += 1;
            }
            rounds
        });

        // (a) one cooperative round, measured directly.
        let mut rounds = ResidentWaitRounds { handle: &handle };
        let single = Instant::now();
        let outcome = rounds.round(25, Instant::now() + WAIT_ROUND_BUDGET);
        let round_cost = single.elapsed();
        assert!(
            matches!(outcome, WaitRound::Reply(_)),
            "the scripted peer must answer one complete wait reply"
        );

        // (b) cancellation token set -> callback returned.
        let token_set = Arc::new(Mutex::new(None::<Instant>));
        let flag = Arc::new(AtomicBool::new(false));
        let canceller_flag = Arc::clone(&flag);
        let canceller_at = Arc::clone(&token_set);
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            *canceller_at.lock().expect("token instant") = Some(Instant::now());
            canceller_flag.store(true, Ordering::Relaxed);
        });
        let probe_flag = Arc::clone(&flag);
        let probe = move || probe_flag.load(Ordering::Relaxed);
        let started = Instant::now();
        let mut rounds = ResidentWaitRounds { handle: &handle };
        let error = drive_wait(
            &mut rounds,
            Instant::now() + Duration::from_secs(30),
            ExecutionControl::with_cancel_probe(&probe),
        )
        .expect_err("the cancellation must end the wait");
        let returned = Instant::now();
        canceller.join().expect("canceller");
        serving.store(false, Ordering::Relaxed);
        let total_rounds = server.join().expect("scripted owner");

        let token_at = token_set
            .lock()
            .expect("token instant")
            .expect("token instant");
        let interval = returned.saturating_duration_since(token_at);
        eprintln!(
            "MEASURED one_round={round_cost:?} cancel_to_return={interval:?} \
             wait_elapsed={:?} rounds_in_run={total_rounds}",
            returned.saturating_duration_since(started)
        );
        assert_eq!(error.code, "cancelled");
        assert!(
            interval < Duration::from_millis(150),
            "cancel -> callback return must stay inside the worker grace, measured {interval:?}"
        );
        let _ = std::fs::remove_file(socket_path);
    }

    /// The owner-launch boundary refuses before anything starts, and reports the
    /// resolver's own reason and native kind instead of a bare message.
    #[test]
    fn owner_launch_refusal_maps_the_resolver_error_without_starting_an_owner() {
        let error =
            owner_launch_program(|| Err(crate::owner_executable::OwnerExecutableError::Missing))
                .expect_err("a missing sibling must refuse the launch");
        assert_eq!(error.code, "managed_job_owner_spawn_failed");
        let detail = error.detail.expect("structured launch refusal");
        assert_eq!(detail["reason"], "owner_executable_missing");
        assert_eq!(detail["owner_started"], false);
        assert!(detail["io_kind"].is_null());

        let error = owner_launch_program(|| {
            Err(crate::owner_executable::OwnerExecutableError::Unavailable(
                std::io::ErrorKind::PermissionDenied,
            ))
        })
        .expect_err("an unreadable sibling must refuse the launch");
        let detail = error.detail.expect("structured launch refusal");
        assert_eq!(detail["reason"], "owner_executable_unavailable");
        assert_eq!(detail["io_kind"], "PermissionDenied");
    }

    /// The program the owner command starts is the resolved sibling, and the
    /// internal owner argument is its whole argv (the launch document is stdin).
    #[test]
    fn owner_launch_program_is_the_resolved_sibling_and_argv_is_exact() {
        let program =
            owner_launch_program(|| Ok(std::path::PathBuf::from("/opt/agenterm/agenterm-cu")))
                .expect("an available sibling resolves");
        let command = owner_command(&program);
        assert_eq!(
            command.get_program(),
            std::ffi::OsStr::new("/opt/agenterm/agenterm-cu")
        );
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(arguments, vec![crate::MANAGED_JOB_OWNER_ARG.to_owned()]);
    }

    #[test]
    fn wait_expected_exit_verification_is_unchanged() {
        assert!(verify_expected_exit(&exited_status(3).state, true, Some(3)).is_ok());
        assert_eq!(
            verify_expected_exit(&exited_status(3).state, true, Some(7))
                .expect_err("a different exit code must not satisfy the expectation")
                .code,
            "managed_job_exit_mismatch"
        );
        assert_eq!(
            verify_expected_exit(&running_status().state, false, Some(0))
                .expect_err("an incomplete wait must not satisfy an exit expectation")
                .code,
            "managed_job_exit_mismatch"
        );
    }

    fn record(session_id: Option<&str>) -> ManagedJobRecord {
        ManagedJobRecord {
            job_id: "00000000-0000-4000-8000-000000000001".into(),
            generation: 1,
            nonce: "00000000000000000000000000000000".into(),
            session_id: session_id.map(str::to_owned),
            origin: crate::managed_job_store::ManagedJobOrigin::Spawned,
            owner: None,
            process: None,
            state: ManagedJobState::StartIntent,
            created_at_utc_ms: 1,
            updated_at_utc_ms: 1,
            terminal_at_utc_ms: None,
            on_expiry: Some(ManagedJobOnExpiry::Stop),
            terminal_trigger: None,
            detach_liveness: None,
        }
    }

    #[test]
    fn mutations_require_the_exact_owning_runtime_session() {
        let owned = record(Some("session-a"));
        assert!(require_owning_session(&owned, "session-a").is_ok());
        assert_eq!(
            require_owning_session(&owned, "session-b")
                .expect_err("other session must not mutate")
                .code,
            "managed_job_session_mismatch"
        );
        assert_eq!(
            require_owning_session(&record(None), "session-a")
                .expect_err("ownerless record must not be mutable")
                .code,
            "managed_job_session_mismatch"
        );
    }

    #[test]
    fn session_cleanup_releases_a_terminal_owner_without_relabelling_terminal_truth() {
        let mut terminal = record(Some("session-a"));
        terminal.owner = Some(ResidentOwnerIdentity {
            pid: 42,
            start_identity: "owner-start".to_owned(),
        });
        terminal.state = ManagedJobState::Exited { exit_code: 7 };
        terminal.terminal_trigger = Some(ManagedJobTerminalTrigger::RootExit);
        let state_before = terminal.state.clone();
        let trigger_before = terminal.terminal_trigger;
        let result = release_session_owner(
            &terminal,
            |_| SessionOwnerObservation::Unknown,
            |handle, operation| {
                assert_eq!(handle, &terminal.handle());
                assert!(matches!(operation, ManagedJobOperation::StopAndRelease));
                Ok(ManagedJobResult::Stop {
                    status: exited_status(7),
                })
            },
        )
        .expect("terminal owner release succeeds");

        assert_eq!(result, SessionOwnerRelease::AlreadyTerminal);
        assert_eq!(terminal.state, state_before);
        assert_eq!(terminal.terminal_trigger, trigger_before);
    }

    #[test]
    fn session_cleanup_treats_an_absent_owner_as_an_idempotent_terminal_result() {
        let mut terminal = record(Some("session-a"));
        terminal.owner = Some(ResidentOwnerIdentity {
            pid: 42,
            start_identity: "owner-start".to_owned(),
        });
        terminal.state = ManagedJobState::Signaled { signal: 15 };
        terminal.terminal_trigger = Some(ManagedJobTerminalTrigger::ExplicitStop);

        for _ in 0..2 {
            let result = release_session_owner(
                &terminal,
                |_| SessionOwnerObservation::Absent,
                |_, operation| {
                    assert!(matches!(operation, ManagedJobOperation::StopAndRelease));
                    Err(ManagedJobProtocolError {
                        code: "managed_job_owner_unavailable".to_owned(),
                        known_written_lower_bound: None,
                        delivery_uncertain: None,
                    })
                },
            )
            .expect("an absent owner is already released");
            assert_eq!(result, SessionOwnerRelease::AlreadyTerminal);
        }
        assert!(matches!(
            terminal.state,
            ManagedJobState::Signaled { signal: 15 }
        ));
        assert_eq!(
            terminal.terminal_trigger,
            Some(ManagedJobTerminalTrigger::ExplicitStop)
        );
    }

    #[test]
    fn session_cleanup_preserves_non_absence_owner_failures() {
        let mut terminal = record(Some("session-a"));
        terminal.owner = Some(ResidentOwnerIdentity {
            pid: 42,
            start_identity: "owner-start".to_owned(),
        });
        terminal.state = ManagedJobState::StartFailed {
            code: "launch_failed".to_owned(),
        };
        let error = release_session_owner(
            &terminal,
            |_| SessionOwnerObservation::Unknown,
            |_, _| {
                Err(ManagedJobProtocolError {
                    code: "managed_job_protocol_io".to_owned(),
                    known_written_lower_bound: None,
                    delivery_uncertain: None,
                })
            },
        )
        .expect_err("non-absence failures remain cleanup failures");
        assert_eq!(error, "managed_job_protocol_io");
    }

    /// A failed connect proves nothing on its own: a terminal record counts as
    /// already released only when its recorded owner identity is observed absent.
    #[test]
    fn session_cleanup_requires_an_exact_absent_observation_to_claim_a_release() {
        for (observation, released) in [
            (SessionOwnerObservation::Absent, true),
            (SessionOwnerObservation::Live, false),
            (SessionOwnerObservation::Unknown, false),
        ] {
            let mut terminal = record(Some("session-a"));
            terminal.owner = Some(ResidentOwnerIdentity {
                pid: 42,
                start_identity: "owner-start".to_owned(),
            });
            terminal.state = ManagedJobState::Exited { exit_code: 7 };
            let observed = std::cell::Cell::new(None);
            let result = release_session_owner(
                &terminal,
                |identity| {
                    observed.set(Some(identity.clone()));
                    observation
                },
                |handle, operation| {
                    assert_eq!(handle, &terminal.handle());
                    assert!(matches!(operation, ManagedJobOperation::StopAndRelease));
                    Err(ManagedJobProtocolError {
                        code: "managed_job_owner_unavailable".to_owned(),
                        known_written_lower_bound: None,
                        delivery_uncertain: None,
                    })
                },
            );
            let seen = observed.into_inner().expect("the observer must run");
            assert_eq!(
                (seen.pid, seen.start_identity.as_str()),
                (42, "owner-start"),
                "the observer must receive the record's exact identity"
            );
            match (released, result) {
                (true, Ok(SessionOwnerRelease::AlreadyTerminal)) => {}
                (false, Err(code)) => assert_eq!(code, "managed_job_owner_unavailable"),
                other => panic!("unexpected outcome for {observation:?}: {other:?}"),
            }
        }
    }

    /// Without a recorded owner identity there is nothing to observe, so the
    /// unavailable owner stays a typed failure even for a terminal record.
    #[test]
    fn session_cleanup_never_claims_a_release_without_a_recorded_owner_identity() {
        let mut terminal = record(Some("session-a"));
        terminal.state = ManagedJobState::Signaled { signal: 15 };
        terminal.owner = None;
        let observed = std::cell::Cell::new(false);
        let error = release_session_owner(
            &terminal,
            |_| {
                observed.set(true);
                SessionOwnerObservation::Absent
            },
            |_, _| {
                Err(ManagedJobProtocolError {
                    code: "managed_job_owner_unavailable".to_owned(),
                    known_written_lower_bound: None,
                    delivery_uncertain: None,
                })
            },
        )
        .expect_err("no identity, no release claim");
        assert_eq!(error, "managed_job_owner_unavailable");
        assert!(
            !observed.into_inner(),
            "there is nothing to observe without an identity"
        );
    }

    /// The non-terminal half of the split is unchanged: even an absent owner is
    /// not proof that a record still claiming to be live was released.
    #[test]
    fn session_cleanup_keeps_the_nonterminal_split_even_with_an_absent_owner() {
        let mut live = record(Some("session-a"));
        live.owner = Some(ResidentOwnerIdentity {
            pid: 42,
            start_identity: "owner-start".to_owned(),
        });
        live.state = ManagedJobState::Running;
        let error = release_session_owner(
            &live,
            |_| SessionOwnerObservation::Absent,
            |_, _| {
                Err(ManagedJobProtocolError {
                    code: "managed_job_owner_unavailable".to_owned(),
                    known_written_lower_bound: None,
                    delivery_uncertain: None,
                })
            },
        )
        .expect_err("a live record's unavailable owner is a failure");
        assert_eq!(error, "managed_job_owner_unavailable");
    }

    #[test]
    fn session_cleanup_reports_an_absent_owner_on_a_live_record_as_a_failure() {
        let mut live = record(Some("session-a"));
        live.owner = Some(ResidentOwnerIdentity {
            pid: 42,
            start_identity: "owner-start".to_owned(),
        });
        live.state = ManagedJobState::Running;
        let requests = std::sync::atomic::AtomicUsize::new(0);
        let error = release_session_owner(
            &live,
            |_| SessionOwnerObservation::Unknown,
            |_, operation| {
                requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert!(matches!(operation, ManagedJobOperation::StopAndRelease));
                Err(ManagedJobProtocolError {
                    code: "managed_job_owner_unavailable".to_owned(),
                    known_written_lower_bound: None,
                    delivery_uncertain: None,
                })
            },
        )
        .expect_err("an absent owner on a live record is not proof of a release");
        assert_eq!(error, "managed_job_owner_unavailable");
        // The refusal must come from exactly one release attempt: the function
        // takes `&record`, so asserting the record itself is unchanged would be
        // tautological and would not notice a second request.
        assert_eq!(
            requests.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a live record's failed release must not be retried inside one call"
        );
    }

    #[test]
    fn resource_cpu_milliseconds_preserve_the_exact_nanosecond_counter() {
        assert_eq!(cpu_ms_decimal("0").unwrap(), "0.000000");
        assert_eq!(cpu_ms_decimal("1000001").unwrap(), "1.000001");
        assert!(cpu_ms_decimal("1.5").is_err());
    }

    #[test]
    fn per_sample_member_rows_cover_legacy_defaults_and_stop_before_overflow() {
        assert_eq!(JOB_RESOURCE_MAX_MEMBER_ROWS, 131_072);
        assert!(!member_rows_would_overflow(
            299 * JOB_RESOURCE_MAX_MEMBERS,
            JOB_RESOURCE_MAX_MEMBERS
        ));
        assert!(!member_rows_would_overflow(
            JOB_RESOURCE_MAX_MEMBER_ROWS - JOB_RESOURCE_MAX_MEMBERS,
            JOB_RESOURCE_MAX_MEMBERS
        ));
        assert!(member_rows_would_overflow(JOB_RESOURCE_MAX_MEMBER_ROWS, 1));
    }
}

#[cfg(test)]
mod expiry_policy_tests {
    use super::*;

    fn fixture() -> ManagedJobRecord {
        ManagedJobRecord {
            job_id: "00000000-0000-4000-8000-000000000002".into(),
            generation: 1,
            nonce: "00000000000000000000000000000001".into(),
            session_id: None,
            origin: ManagedJobOrigin::Spawned,
            owner: None,
            process: None,
            state: ManagedJobState::StartIntent,
            created_at_utc_ms: 1,
            updated_at_utc_ms: 1,
            terminal_at_utc_ms: None,
            on_expiry: Some(ManagedJobOnExpiry::Stop),
            terminal_trigger: None,
            detach_liveness: None,
        }
    }

    #[test]
    fn the_public_expiry_spellings_have_one_internal_translation() {
        // Both public surfaces funnel through one mapping, and `job-adopt`'s
        // boolean is translated once, one-way.
        assert_eq!(managed_on_expiry(JobExpiry::Stop), ManagedJobOnExpiry::Stop);
        assert_eq!(
            managed_on_expiry(JobExpiry::Detach),
            ManagedJobOnExpiry::Detach
        );
        // The boolean surface exists only where `job-adopt` does.
        #[cfg(unix)]
        {
            assert_eq!(
                managed_on_expiry(expiry_from_stop_on_expiry(true)),
                ManagedJobOnExpiry::Stop
            );
            assert_eq!(
                managed_on_expiry(expiry_from_stop_on_expiry(false)),
                ManagedJobOnExpiry::Detach
            );
        }
    }

    /// A historical record has none of the three policy/terminal fields. It must
    /// project explicit nulls -- never a default `stop` and never `live` -- and
    /// its absence must not make an older record unreadable.
    #[test]
    fn a_historical_record_projects_explicit_nulls() {
        let mut historical = fixture();
        historical.on_expiry = None;
        historical.terminal_trigger = None;
        historical.detach_liveness = None;
        let payload = record_payload(&historical, None);
        assert!(payload["on_expiry"].is_null(), "{payload}");
        assert!(payload["terminal_trigger"].is_null(), "{payload}");
        assert!(payload["detach_liveness"].is_null(), "{payload}");
        assert_eq!(payload["state"], "start_intent");
    }

    /// A detached record carries its liveness and trigger verbatim, so a
    /// `detached` state can never be read as "the process survived".
    #[test]
    fn a_detached_record_projects_its_liveness_and_trigger() {
        let mut detached = fixture();
        detached.state = ManagedJobState::Detached;
        detached.detach_liveness = Some(ManagedJobDetachLiveness::Absent);
        detached.terminal_trigger = Some(ManagedJobTerminalTrigger::LeaseExpiry);
        detached.on_expiry = Some(ManagedJobOnExpiry::Detach);
        let payload = record_payload(&detached, None);
        assert_eq!(payload["state"], "detached");
        assert_eq!(payload["detach_liveness"], "absent");
        assert_eq!(payload["terminal_trigger"], "lease_expiry");
        assert_eq!(payload["on_expiry"], "detach");
    }
}
