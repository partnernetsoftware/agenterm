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

use crate::{
    command::{JobEnvironment, JobOutputCursor, JobStateFilter},
    idempotency_store::FinalReplay,
    managed_job_ipc::{
        JobState, ManagedJobOperation, ManagedJobProtocolError, ManagedJobResult, OutputStream,
        base64_decode, base64_encode, client_request,
    },
    managed_job_owner::{LAUNCH_SCHEMA_VERSION, ManagedJobEnvironment, ManagedJobLaunch},
    managed_job_store::{
        ManagedJobRecord, ManagedJobState, ManagedJobStore, OwnerLiveness, ResidentOwnerIdentity,
    },
};

use super::{CuError, now_utc_ms};

const START_READY_TIMEOUT: Duration = Duration::from_secs(5);
const START_POLL: Duration = Duration::from_millis(20);
const OUTPUT_CAPACITY_BYTES: usize = 1024 * 1024;
const IPC_PAGE_BYTES: usize = 64 * 1024;

pub(super) struct JobRequestContext<'a> {
    pub session_id: &'a str,
}

pub(super) fn replay_payload(replay: &FinalReplay) -> Value {
    match replay {
        FinalReplay::JobSpawn { job_id, generation } => json!({
            "job_id": job_id,
            "generation": generation,
        }),
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

pub(super) fn job_spawn_payload(
    command: &[String],
    environment: &[JobEnvironment],
    cwd: Option<&str>,
    ttl_seconds: u64,
    session_id: &str,
) -> Result<Value, CuError> {
    let store = ManagedJobStore::open()?;
    let now = now_utc_ms().ok_or_else(clock_error)?;
    let record = store.reserve_start(Some(session_id), now)?;
    let launch = match build_launch(&store, &record, command, environment, cwd, ttl_seconds) {
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
    let encoded = match serde_json::to_vec(&launch) {
        Ok(encoded) => encoded,
        Err(_) => {
            let error = CuError::new(
                "managed_job_launch_invalid",
                "managed-job launch document could not be encoded",
            );
            mark_clean_owner_failure(&store, &record, error.code.as_str(), now)?;
            return Err(error);
        }
    };
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(_) => {
            let error = CuError::new(
                "managed_job_owner_spawn_failed",
                "agenterm-cu executable identity is unavailable",
            );
            mark_clean_owner_failure(&store, &record, error.code.as_str(), now)?;
            return Err(error);
        }
    };
    let mut command = ProcessCommand::new(executable);
    command
        .arg(crate::MANAGED_JOB_OWNER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut owner_child, mode) = match spawn_detached_child(&mut command) {
        Ok(spawned) => spawned,
        Err(_) => {
            let error = CuError::new(
                "managed_job_owner_spawn_failed",
                "managed-job resident owner could not start",
            );
            mark_clean_owner_failure(&store, &record, error.code.as_str(), now)?;
            return Err(error);
        }
    };
    if mode != DetachedSpawnMode::Independent {
        let _ = owner_child.kill();
        let _ = owner_child.wait();
        mark_clean_owner_failure(&store, &record, "owner_detach_unavailable", now)?;
        return Err(CuError::new(
            "managed_job_detach_unavailable",
            "host kept the resident owner inside the caller lifetime",
        ));
    }
    let Some(mut input) = owner_child.stdin.take() else {
        let _ = owner_child.kill();
        let _ = owner_child.wait();
        mark_clean_owner_failure(&store, &record, "owner_stdin_unavailable", now)?;
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
        return classify_post_spawn_failure(&store, &record, "owner_launch_write_failed", now);
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
            return classify_post_spawn_failure(&store, &record, "owner_exited_before_ready", now);
        }
        if Instant::now() >= deadline {
            let _ = owner_child.kill();
            let _ = owner_child.wait();
            return classify_post_spawn_failure(&store, &record, "owner_ready_timeout", now);
        }
        thread::sleep(START_POLL);
    }
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

pub(super) fn job_wait_payload(
    job_id: &str,
    generation: u64,
    timeout_ms: u64,
    expect_exit: Option<i32>,
) -> Result<Value, CuError> {
    let record = checked_record(job_id, generation)?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let slice = remaining.min(Duration::from_secs(300));
        let result = client_request(
            &record.handle(),
            ManagedJobOperation::Wait {
                timeout_ms: slice.as_millis().try_into().unwrap_or(300_000),
            },
        )
        .map_err(client_error)?;
        let ManagedJobResult::Wait { completed, status } = result else {
            return Err(response_kind_error());
        };
        if completed || remaining.is_zero() {
            verify_expected_exit(&status.state, completed, expect_exit)?;
            return Ok(json!({
                "job_id": job_id,
                "generation": generation,
                "completed": completed,
                "status": status,
            }));
        }
    }
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
    json!({
        "job_id": record.job_id,
        "generation": record.generation,
        "session_id": record.session_id,
        "state": state,
        "terminal": terminal,
        "created_at_utc_ms": record.created_at_utc_ms,
        "updated_at_utc_ms": record.updated_at_utc_ms,
        "terminal_at_utc_ms": record.terminal_at_utc_ms,
        "owner_pid": record.owner.as_ref().map(|owner| owner.pid),
        "process_pid": record.process.as_ref().map(|process| process.pid),
        "io_available": live.is_some(),
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

fn build_launch(
    store: &ManagedJobStore,
    record: &ManagedJobRecord,
    command: &[String],
    environment: &[JobEnvironment],
    cwd: Option<&str>,
    ttl_seconds: u64,
) -> Result<ManagedJobLaunch, CuError> {
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
    use super::*;

    fn record(session_id: Option<&str>) -> ManagedJobRecord {
        ManagedJobRecord {
            job_id: "00000000-0000-4000-8000-000000000001".into(),
            generation: 1,
            nonce: "00000000000000000000000000000000".into(),
            session_id: session_id.map(str::to_owned),
            owner: None,
            process: None,
            state: ManagedJobState::StartIntent,
            created_at_utc_ms: 1,
            updated_at_utc_ms: 1,
            terminal_at_utc_ms: None,
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
}
