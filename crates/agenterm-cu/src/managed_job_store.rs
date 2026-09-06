//! Crash-safe metadata for the native managed-job owner.
//!
//! This store deliberately owns no process handles and starts no processes. It
//! seals the durable identity/state half of the contract so a later resident
//! owner can publish an intent before it detaches, claim that exact generation,
//! and only then launch the contained child. Every operation serializes through
//! one stable sidecar lock and atomically replaces one bounded private document.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use agenterm_platform::{
    entropy::secure_random_array,
    filesystem::{
        host_directories, metadata_is_link_like, protect_private_directory, write_private_atomic,
    },
    locking::{LockErrorKind, PathLock},
};
use serde::{Deserialize, Serialize};

use crate::CuError;

const SCHEMA_VERSION: u32 = 1;
const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_JOBS: usize = 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_SESSION_ID_BYTES: usize = 128;
const MAX_START_IDENTITY_BYTES: usize = 512;
const MAX_TERMINAL_CODE_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct ManagedJobRefreshBlockers {
    pub blocking: usize,
    pub start_intent: usize,
    pub starting: usize,
    pub running: usize,
    pub orphaned_uncertain: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ManagedJobPruneReport {
    pub apply: bool,
    pub total: usize,
    pub active: usize,
    pub detached: usize,
    pub uncertain: usize,
    pub terminal: usize,
    pub eligible: usize,
    pub retained_newest: usize,
    pub removed: usize,
    pub remaining: usize,
    pub candidate_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedJobHandle {
    pub job_id: String,
    pub generation: u64,
    pub nonce: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResidentOwnerIdentity {
    pub pid: u32,
    pub start_identity: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExactProcessIdentity {
    pub pid: u32,
    pub start_identity: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManagedJobOrigin {
    #[default]
    Spawned,
    Adopted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum ManagedJobState {
    StartIntent,
    Starting,
    Running,
    StartFailed { code: String },
    Exited { exit_code: i32 },
    Signaled { signal: u16 },
    Detached,
    OrphanedUncertain,
}

impl ManagedJobState {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::StartFailed { .. } | Self::Exited { .. } | Self::Signaled { .. } | Self::Detached
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagedJobRecord {
    pub job_id: String,
    pub generation: u64,
    pub nonce: String,
    pub session_id: Option<String>,
    #[serde(default)]
    pub origin: ManagedJobOrigin,
    pub owner: Option<ResidentOwnerIdentity>,
    pub process: Option<ExactProcessIdentity>,
    pub state: ManagedJobState,
    pub created_at_utc_ms: i64,
    pub updated_at_utc_ms: i64,
    pub terminal_at_utc_ms: Option<i64>,
}

impl ManagedJobRecord {
    pub(crate) fn handle(&self) -> ManagedJobHandle {
        ManagedJobHandle {
            job_id: self.job_id.clone(),
            generation: self.generation,
            nonce: self.nonce.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OwnerLiveness {
    Live(ResidentOwnerIdentity),
    Dead,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OwnerReconciliation {
    Live,
    PreservedUnknown,
    MarkedOrphanedUncertain,
    AlreadyTerminal,
}

#[derive(Debug)]
pub(crate) struct ManagedJobStore {
    path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    last_now_utc_ms: i64,
    jobs: BTreeMap<String, ManagedJobRecord>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            last_now_utc_ms: 0,
            jobs: BTreeMap::new(),
        }
    }
}

impl ManagedJobStore {
    pub(crate) fn open() -> Result<Self, CuError> {
        Self::open_creating_parent(Self::configured_path()?)
    }

    fn configured_path() -> Result<PathBuf, CuError> {
        let path = if let Some(path) = std::env::var_os("AGENTERM_CU_MANAGED_JOB_PATH") {
            PathBuf::from(path)
        } else {
            let directories = host_directories().map_err(|_| unavailable())?;
            directories
                .local_data
                .join("agenterm")
                .join("cu-managed-jobs.json")
        };
        if path.is_absolute() {
            Ok(path)
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .map_err(|_| unavailable())
        }
    }

    fn open_creating_parent(path: PathBuf) -> Result<Self, CuError> {
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|_| unavailable())?
                .join(path)
        };
        let parent = explicit_parent(&path)?;
        fs::create_dir_all(parent).map_err(|_| unavailable())?;
        protect_private_directory(parent).map_err(|_| unavailable())?;
        Self::open_at(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Open one caller-selected private state file.
    ///
    /// The parent must already be a direct directory. It is protected for the
    /// current user before any state or sidecar lock is created.
    pub(crate) fn open_at(path: impl Into<PathBuf>) -> Result<Self, CuError> {
        let path = path.into();
        let parent = explicit_parent(&path)?;
        let metadata = fs::symlink_metadata(parent).map_err(|_| unavailable())?;
        if metadata_is_link_like(&metadata) || !metadata.is_dir() {
            return Err(corrupt(
                "managed-job state parent must be a direct directory",
            ));
        }
        protect_private_directory(parent).map_err(|_| unavailable())?;
        Ok(Self { path })
    }

    /// Publish the sealed pre-owner intent. No command, environment, lease or
    /// bearer material is accepted by this API, so it cannot enter the registry.
    pub(crate) fn reserve_start(
        &self,
        session_id: Option<&str>,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        self.reserve_start_with_origin(session_id, ManagedJobOrigin::Spawned, now_utc_ms)
    }

    pub(crate) fn reserve_start_with_origin(
        &self,
        session_id: Option<&str>,
        origin: ManagedJobOrigin,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        validate_time(now_utc_ms)?;
        let session_id = session_id.map(validate_session_id).transpose()?;
        let job_id = random_uuid_v4()?;
        let nonce = random_hex::<16>()?;
        self.mutate(now_utc_ms, move |document| {
            if document.jobs.len() >= MAX_JOBS {
                return Err(CuError::new(
                    "managed_job_store_limit",
                    "managed-job registry reached its 1024-record ceiling",
                ));
            }
            let record = ManagedJobRecord {
                job_id: job_id.clone(),
                generation: 1,
                nonce,
                session_id,
                origin,
                owner: None,
                process: None,
                state: ManagedJobState::StartIntent,
                created_at_utc_ms: now_utc_ms,
                updated_at_utc_ms: now_utc_ms,
                terminal_at_utc_ms: None,
            };
            document.jobs.insert(job_id, record.clone());
            Ok(record)
        })
    }

    /// Bind a detached resident owner's exact process identity to one intent.
    pub(crate) fn claim_starting(
        &self,
        handle: &ManagedJobHandle,
        owner: ResidentOwnerIdentity,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        validate_owner(&owner)?;
        self.transition(handle, now_utc_ms, move |record| {
            if record.state != ManagedJobState::StartIntent {
                return Err(transition_error("only start_intent may become starting"));
            }
            record.owner = Some(owner);
            record.state = ManagedJobState::Starting;
            Ok(())
        })
    }

    /// Close an intent only while no resident owner ever claimed it. The
    /// launcher retains and reaps the attempted owner before using this edge.
    pub(crate) fn mark_unclaimed_start_failed(
        &self,
        handle: &ManagedJobHandle,
        code: &str,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        validate_bounded_token(code, MAX_TERMINAL_CODE_BYTES, "terminal code")?;
        let code = code.to_owned();
        self.transition(handle, now_utc_ms, move |record| {
            if record.state != ManagedJobState::StartIntent
                || record.owner.is_some()
                || record.process.is_some()
            {
                return Err(transition_error(
                    "unclaimed start failure requires an ownerless start intent",
                ));
            }
            record.state = ManagedJobState::StartFailed { code };
            record.terminal_at_utc_ms = Some(now_utc_ms);
            Ok(())
        })
    }

    /// Publish the exact contained child only after the resident owner has
    /// claimed this generation.
    pub(crate) fn mark_running(
        &self,
        handle: &ManagedJobHandle,
        owner: &ResidentOwnerIdentity,
        process: ExactProcessIdentity,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        validate_owner(owner)?;
        validate_process(&process)?;
        let owner = owner.clone();
        self.transition(handle, now_utc_ms, move |record| {
            require_owner(record, &owner)?;
            if record.state != ManagedJobState::Starting {
                return Err(transition_error("only starting may become running"));
            }
            record.process = Some(process);
            record.state = ManagedJobState::Running;
            Ok(())
        })
    }

    pub(crate) fn mark_start_failed(
        &self,
        handle: &ManagedJobHandle,
        owner: &ResidentOwnerIdentity,
        code: &str,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        validate_owner(owner)?;
        validate_bounded_token(code, MAX_TERMINAL_CODE_BYTES, "terminal code")?;
        let owner = owner.clone();
        let code = code.to_owned();
        self.transition(handle, now_utc_ms, move |record| {
            require_owner(record, &owner)?;
            if record.state != ManagedJobState::Starting || record.process.is_some() {
                return Err(transition_error(
                    "start failure requires starting without a child",
                ));
            }
            record.state = ManagedJobState::StartFailed { code };
            record.terminal_at_utc_ms = Some(now_utc_ms);
            Ok(())
        })
    }

    pub(crate) fn mark_exited(
        &self,
        handle: &ManagedJobHandle,
        owner: &ResidentOwnerIdentity,
        process: &ExactProcessIdentity,
        exit_code: i32,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        self.mark_running_terminal(
            handle,
            owner,
            process,
            ManagedJobState::Exited { exit_code },
            now_utc_ms,
        )
    }

    pub(crate) fn mark_signaled(
        &self,
        handle: &ManagedJobHandle,
        owner: &ResidentOwnerIdentity,
        process: &ExactProcessIdentity,
        signal: u16,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        if signal == 0 {
            return Err(CuError::new(
                "managed_job_signal_invalid",
                "terminal signal must be nonzero",
            ));
        }
        self.mark_running_terminal(
            handle,
            owner,
            process,
            ManagedJobState::Signaled { signal },
            now_utc_ms,
        )
    }

    pub(crate) fn mark_detached(
        &self,
        handle: &ManagedJobHandle,
        owner: &ResidentOwnerIdentity,
        process: &ExactProcessIdentity,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        self.mark_running_terminal(
            handle,
            owner,
            process,
            ManagedJobState::Detached,
            now_utc_ms,
        )
    }

    /// Reconcile only the resident owner. Dead or identity-reused owners are
    /// retained as `orphaned_uncertain`; `Unknown` is never treated as stale.
    pub(crate) fn reconcile_owner(
        &self,
        handle: &ManagedJobHandle,
        liveness: OwnerLiveness,
        now_utc_ms: i64,
    ) -> Result<OwnerReconciliation, CuError> {
        self.mutate(now_utc_ms, |document| {
            let record = checked_record_mut(document, handle)?;
            if record.state.is_terminal() {
                return Ok(OwnerReconciliation::AlreadyTerminal);
            }
            let Some(expected) = record.owner.as_ref() else {
                return match liveness {
                    OwnerLiveness::Unknown => Ok(OwnerReconciliation::PreservedUnknown),
                    _ => Err(transition_error(
                        "start intent has no resident owner to reconcile",
                    )),
                };
            };
            match liveness {
                OwnerLiveness::Unknown => Ok(OwnerReconciliation::PreservedUnknown),
                OwnerLiveness::Live(observed) if observed == *expected => {
                    Ok(OwnerReconciliation::Live)
                }
                OwnerLiveness::Live(_) | OwnerLiveness::Dead => {
                    record.state = ManagedJobState::OrphanedUncertain;
                    record.updated_at_utc_ms = now_utc_ms;
                    Ok(OwnerReconciliation::MarkedOrphanedUncertain)
                }
            }
        })
    }

    pub(crate) fn get(&self, job_id: &str) -> Result<Option<ManagedJobRecord>, CuError> {
        validate_bounded_token(job_id, MAX_ID_BYTES, "job id")?;
        self.inspect(|document| Ok(document.jobs.get(job_id).cloned()))
    }

    pub(crate) fn list(&self) -> Result<Vec<ManagedJobRecord>, CuError> {
        self.inspect(|document| Ok(document.jobs.values().cloned().collect()))
    }

    /// Plan or apply bounded terminal-receipt retention. Live, detached and
    /// uncertain records are never candidates. Apply recomputes the complete
    /// selection while holding the same store lock that publishes removal.
    pub(crate) fn prune(
        &self,
        max_age_ms: i64,
        keep_newest: usize,
        apply: bool,
        now_utc_ms: i64,
    ) -> Result<ManagedJobPruneReport, CuError> {
        if max_age_ms < 0 || keep_newest > MAX_JOBS {
            return Err(CuError::new(
                "managed_job_prune_invalid",
                "managed-job prune bounds are invalid",
            ));
        }
        if apply {
            self.mutate(now_utc_ms, |document| {
                let mut selection = prune_selection(document, max_age_ms, keep_newest, now_utc_ms)?;
                for job_id in &selection.candidate_ids {
                    document.jobs.remove(job_id);
                }
                selection.report.apply = true;
                selection.report.removed = selection.candidate_ids.len();
                selection.report.remaining = document.jobs.len();
                Ok(selection.report)
            })
        } else {
            self.inspect(|document| {
                if now_utc_ms < document.last_now_utc_ms {
                    return Err(CuError::new(
                        "managed_job_clock_rollback",
                        "managed-job prune clock moved backward",
                    ));
                }
                Ok(prune_selection(document, max_age_ms, keep_newest, now_utc_ms)?.report)
            })
        }
    }

    /// Count resident resources that make a runtime refresh defer. This path
    /// takes the durable store lock and is used under the coordinator refresh
    /// fence for apply admission.
    pub(crate) fn refresh_blockers(&self) -> Result<ManagedJobRefreshBlockers, CuError> {
        self.inspect(|document| Ok(summarize_refresh_blockers(document)))
    }

    /// Read the atomic document without creating a directory or lock file.
    /// Setup check is a diagnostic and must remain strictly zero-write; apply
    /// uses [`Self::refresh_blockers`] under the refresh fence instead.
    pub(crate) fn refresh_blockers_read_only() -> Result<ManagedJobRefreshBlockers, CuError> {
        let path = Self::configured_path()?;
        let parent = explicit_parent(&path)?;
        match fs::symlink_metadata(parent) {
            Ok(metadata) if metadata_is_link_like(&metadata) || !metadata.is_dir() => {
                return Err(corrupt(
                    "managed-job state parent must be a direct directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ManagedJobRefreshBlockers::default());
            }
            Err(_) => return Err(unavailable()),
        }
        let store = Self { path };
        let document = store.read_document()?.unwrap_or_default();
        Ok(summarize_refresh_blockers(&document))
    }

    fn mark_running_terminal(
        &self,
        handle: &ManagedJobHandle,
        owner: &ResidentOwnerIdentity,
        process: &ExactProcessIdentity,
        state: ManagedJobState,
        now_utc_ms: i64,
    ) -> Result<ManagedJobRecord, CuError> {
        validate_owner(owner)?;
        validate_process(process)?;
        debug_assert!(state.is_terminal());
        let owner = owner.clone();
        let process = process.clone();
        self.transition(handle, now_utc_ms, move |record| {
            require_owner(record, &owner)?;
            if record.state != ManagedJobState::Running || record.process.as_ref() != Some(&process)
            {
                return Err(transition_error(
                    "terminal transition requires the exact running child identity",
                ));
            }
            record.state = state;
            record.terminal_at_utc_ms = Some(now_utc_ms);
            Ok(())
        })
    }

    fn transition(
        &self,
        handle: &ManagedJobHandle,
        now_utc_ms: i64,
        edit: impl FnOnce(&mut ManagedJobRecord) -> Result<(), CuError>,
    ) -> Result<ManagedJobRecord, CuError> {
        validate_handle(handle)?;
        self.mutate(now_utc_ms, |document| {
            let record = checked_record_mut(document, handle)?;
            if now_utc_ms < record.updated_at_utc_ms {
                return Err(CuError::new(
                    "managed_job_clock_rollback",
                    "managed-job transition clock moved backward",
                ));
            }
            edit(record)?;
            record.updated_at_utc_ms = now_utc_ms;
            Ok(record.clone())
        })
    }

    fn inspect<T>(&self, read: impl FnOnce(&Document) -> Result<T, CuError>) -> Result<T, CuError> {
        let _lock = self.acquire_lock()?;
        let document = self.read_document()?.unwrap_or_default();
        read(&document)
    }

    fn mutate<T>(
        &self,
        now_utc_ms: i64,
        edit: impl FnOnce(&mut Document) -> Result<T, CuError>,
    ) -> Result<T, CuError> {
        validate_time(now_utc_ms)?;
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document()?.unwrap_or_default();
        if now_utc_ms < document.last_now_utc_ms {
            return Err(CuError::new(
                "managed_job_clock_rollback",
                "managed-job registry clock moved backward",
            ));
        }
        let result = edit(&mut document)?;
        document.last_now_utc_ms = now_utc_ms;
        validate_document(&document)?;
        self.write_document(&document)?;
        Ok(result)
    }

    fn acquire_lock(&self) -> Result<PathLock, CuError> {
        PathLock::try_acquire(&self.lock_path()?).map_err(|error| {
            if error.kind() == LockErrorKind::Contended {
                CuError::new(
                    "managed_job_store_contended",
                    "managed-job registry is owned by another operation",
                )
            } else {
                unavailable()
            }
        })
    }

    fn lock_path(&self) -> Result<PathBuf, CuError> {
        let name = self
            .path
            .file_name()
            .ok_or_else(|| corrupt("managed-job state path has no file name"))?;
        let mut sidecar = name.to_os_string();
        sidecar.push(".lock");
        Ok(explicit_parent(&self.path)?.join(sidecar))
    }

    fn read_document(&self) -> Result<Option<Document>, CuError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable()),
        };
        if metadata_is_link_like(&metadata) || !metadata.is_file() {
            return Err(corrupt(
                "managed-job state must be one regular, non-link file",
            ));
        }
        if metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(corrupt("managed-job state exceeds its byte ceiling"));
        }
        let bytes = fs::read(&self.path).map_err(|_| unavailable())?;
        let document: Document = serde_json::from_slice(&bytes)
            .map_err(|_| corrupt("managed-job state is not valid schema JSON"))?;
        validate_document(&document)?;
        Ok(Some(document))
    }

    fn write_document(&self, document: &Document) -> Result<(), CuError> {
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && (metadata_is_link_like(&metadata) || !metadata.is_file())
        {
            return Err(corrupt(
                "managed-job state target became a link-like or non-file entry",
            ));
        }
        let bytes = serde_json::to_vec(document).map_err(|_| {
            CuError::new(
                "managed_job_store_serialization",
                "managed-job state could not be serialized",
            )
        })?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(CuError::new(
                "managed_job_store_limit",
                "managed-job state exceeds its byte ceiling",
            ));
        }
        write_private_atomic(&self.path, &bytes).map_err(|_| {
            CuError::new(
                "managed_job_store_publish",
                "managed-job state could not be atomically published",
            )
        })
    }
}

fn summarize_refresh_blockers(document: &Document) -> ManagedJobRefreshBlockers {
    let mut summary = ManagedJobRefreshBlockers::default();
    for record in document.jobs.values() {
        match record.state {
            ManagedJobState::StartIntent => summary.start_intent += 1,
            ManagedJobState::Starting => summary.starting += 1,
            ManagedJobState::Running => summary.running += 1,
            ManagedJobState::OrphanedUncertain => summary.orphaned_uncertain += 1,
            ManagedJobState::StartFailed { .. }
            | ManagedJobState::Exited { .. }
            | ManagedJobState::Signaled { .. }
            | ManagedJobState::Detached => {}
        }
    }
    summary.blocking =
        summary.start_intent + summary.starting + summary.running + summary.orphaned_uncertain;
    summary
}

struct ManagedJobPruneSelection {
    report: ManagedJobPruneReport,
    candidate_ids: Vec<String>,
}

fn prune_selection(
    document: &Document,
    max_age_ms: i64,
    keep_newest: usize,
    now_utc_ms: i64,
) -> Result<ManagedJobPruneSelection, CuError> {
    let cutoff = now_utc_ms.checked_sub(max_age_ms).ok_or_else(|| {
        CuError::new(
            "managed_job_prune_invalid",
            "managed-job prune cutoff underflows UTC milliseconds",
        )
    })?;
    let mut active = 0;
    let mut detached = 0;
    let mut uncertain = 0;
    let mut terminal = Vec::new();
    for record in document.jobs.values() {
        match record.state {
            ManagedJobState::StartIntent | ManagedJobState::Starting | ManagedJobState::Running => {
                active += 1;
            }
            ManagedJobState::Detached => detached += 1,
            ManagedJobState::OrphanedUncertain => uncertain += 1,
            ManagedJobState::StartFailed { .. }
            | ManagedJobState::Exited { .. }
            | ManagedJobState::Signaled { .. } => {
                let terminal_at = record.terminal_at_utc_ms.ok_or_else(|| {
                    corrupt("terminal managed-job record has no terminal timestamp")
                })?;
                terminal.push((terminal_at, record.job_id.clone()));
            }
        }
    }
    terminal.sort_by(|left, right| right.cmp(left));
    let retained_newest = keep_newest.min(terminal.len());
    let mut candidate_ids = terminal
        .iter()
        .skip(retained_newest)
        .filter(|(terminal_at, _)| *terminal_at <= cutoff)
        .map(|(_, job_id)| job_id.clone())
        .collect::<Vec<_>>();
    candidate_ids.sort();
    let report = ManagedJobPruneReport {
        apply: false,
        total: document.jobs.len(),
        active,
        detached,
        uncertain,
        terminal: terminal.len(),
        eligible: candidate_ids.len(),
        retained_newest,
        removed: 0,
        remaining: document.jobs.len(),
        candidate_ids: candidate_ids.clone(),
    };
    Ok(ManagedJobPruneSelection {
        report,
        candidate_ids,
    })
}

fn checked_record_mut<'a>(
    document: &'a mut Document,
    handle: &ManagedJobHandle,
) -> Result<&'a mut ManagedJobRecord, CuError> {
    let record = document.jobs.get_mut(&handle.job_id).ok_or_else(|| {
        CuError::new("managed_job_not_found", "managed-job record does not exist")
    })?;
    if record.generation != handle.generation || record.nonce != handle.nonce {
        return Err(CuError::new(
            "managed_job_identity_changed",
            "managed-job generation or nonce no longer matches",
        ));
    }
    Ok(record)
}

fn require_owner(record: &ManagedJobRecord, owner: &ResidentOwnerIdentity) -> Result<(), CuError> {
    if record.owner.as_ref() == Some(owner) {
        Ok(())
    } else {
        Err(CuError::new(
            "managed_job_owner_changed",
            "resident owner identity no longer matches",
        ))
    }
}

fn validate_document(document: &Document) -> Result<(), CuError> {
    if document.schema_version != SCHEMA_VERSION || document.last_now_utc_ms < 0 {
        return Err(corrupt("managed-job state schema or clock is invalid"));
    }
    if document.jobs.len() > MAX_JOBS {
        return Err(corrupt("managed-job state exceeds its record ceiling"));
    }
    for (key, record) in &document.jobs {
        validate_record(key, record)?;
        if record.updated_at_utc_ms > document.last_now_utc_ms {
            return Err(corrupt(
                "managed-job record clock exceeds the document watermark",
            ));
        }
    }
    Ok(())
}

fn validate_record(key: &str, record: &ManagedJobRecord) -> Result<(), CuError> {
    validate_handle(&record.handle()).map_err(|_| corrupt("managed-job identity is invalid"))?;
    if key != record.job_id || record.generation == 0 {
        return Err(corrupt("managed-job key or generation is invalid"));
    }
    if let Some(session) = record.session_id.as_deref() {
        validate_session_id(session).map_err(|_| corrupt("managed-job session is invalid"))?;
    }
    if let Some(owner) = record.owner.as_ref() {
        validate_owner(owner).map_err(|_| corrupt("managed-job owner is invalid"))?;
    }
    if let Some(process) = record.process.as_ref() {
        validate_process(process).map_err(|_| corrupt("managed-job process is invalid"))?;
    }
    if record.created_at_utc_ms < 0 || record.updated_at_utc_ms < record.created_at_utc_ms {
        return Err(corrupt("managed-job timestamps are invalid"));
    }
    let shape_valid = match &record.state {
        ManagedJobState::StartIntent => {
            record.owner.is_none()
                && record.process.is_none()
                && record.terminal_at_utc_ms.is_none()
        }
        ManagedJobState::Starting => {
            record.owner.is_some()
                && record.process.is_none()
                && record.terminal_at_utc_ms.is_none()
        }
        ManagedJobState::Running => {
            record.owner.is_some()
                && record.process.is_some()
                && record.terminal_at_utc_ms.is_none()
        }
        ManagedJobState::OrphanedUncertain => {
            record.owner.is_some() && record.terminal_at_utc_ms.is_none()
        }
        ManagedJobState::StartFailed { code } => {
            validate_bounded_token(code, MAX_TERMINAL_CODE_BYTES, "terminal code").is_ok()
                && record.process.is_none()
                && record.terminal_at_utc_ms == Some(record.updated_at_utc_ms)
        }
        ManagedJobState::Exited { .. }
        | ManagedJobState::Signaled { .. }
        | ManagedJobState::Detached => {
            record.owner.is_some()
                && record.process.is_some()
                && record.terminal_at_utc_ms == Some(record.updated_at_utc_ms)
        }
    };
    if !shape_valid {
        return Err(corrupt("managed-job state transition shape is invalid"));
    }
    Ok(())
}

fn validate_handle(handle: &ManagedJobHandle) -> Result<(), CuError> {
    validate_uuid_v4(&handle.job_id)?;
    if handle.generation == 0 {
        return Err(CuError::new(
            "managed_job_generation_invalid",
            "managed-job generation must be nonzero",
        ));
    }
    if handle.nonce.len() != 32
        || !handle
            .nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(CuError::new(
            "managed_job_nonce_invalid",
            "managed-job nonce must be 32 lowercase hexadecimal bytes",
        ));
    }
    Ok(())
}

fn validate_uuid_v4(value: &str) -> Result<(), CuError> {
    let valid = value.len() <= MAX_ID_BYTES
        && value.len() == 36
        && value.as_bytes().get(8) == Some(&b'-')
        && value.as_bytes().get(13) == Some(&b'-')
        && value.as_bytes().get(18) == Some(&b'-')
        && value.as_bytes().get(23) == Some(&b'-')
        && value.as_bytes().get(14) == Some(&b'4')
        && matches!(value.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23)
                || byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
        });
    if valid {
        Ok(())
    } else {
        Err(CuError::new(
            "managed_job_id_invalid",
            "managed-job id must be a lowercase UUID v4",
        ))
    }
}

fn validate_session_id(value: &str) -> Result<String, CuError> {
    validate_bounded_token(value, MAX_SESSION_ID_BYTES, "session id")?;
    Ok(value.to_owned())
}

fn validate_owner(owner: &ResidentOwnerIdentity) -> Result<(), CuError> {
    validate_process_parts(owner.pid, &owner.start_identity, "resident owner")
}

fn validate_process(process: &ExactProcessIdentity) -> Result<(), CuError> {
    validate_process_parts(process.pid, &process.start_identity, "child process")
}

fn validate_process_parts(pid: u32, start_identity: &str, kind: &str) -> Result<(), CuError> {
    if pid == 0 {
        return Err(CuError::new(
            "managed_job_process_identity_invalid",
            format!("{kind} PID must be nonzero"),
        ));
    }
    validate_bounded_token(
        start_identity,
        MAX_START_IDENTITY_BYTES,
        "process start identity",
    )
}

fn validate_bounded_token(value: &str, max: usize, name: &str) -> Result<(), CuError> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(CuError::new(
            "managed_job_identity_invalid",
            format!("{name} must be 1..={max} non-control UTF-8 bytes"),
        ));
    }
    Ok(())
}

fn validate_time(now_utc_ms: i64) -> Result<(), CuError> {
    if now_utc_ms < 0 {
        Err(CuError::new(
            "managed_job_clock_invalid",
            "managed-job clock must be a non-negative UTC millisecond value",
        ))
    } else {
        Ok(())
    }
}

fn explicit_parent(path: &Path) -> Result<&Path, CuError> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| corrupt("managed-job state requires an explicit parent directory"))
}

fn unavailable() -> CuError {
    CuError::new(
        "managed_job_store_unavailable",
        "managed-job state or its sidecar lock is unavailable",
    )
}

fn corrupt(message: &'static str) -> CuError {
    CuError::new("managed_job_store_corrupt", message)
}

fn transition_error(message: &'static str) -> CuError {
    CuError::new("managed_job_transition_invalid", message)
}

fn random_uuid_v4() -> Result<String, CuError> {
    let mut bytes = secure_random_array::<16>().map_err(|_| {
        CuError::new(
            "managed_job_entropy_unavailable",
            "OS CSPRNG is unavailable",
        )
    })?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

fn random_hex<const N: usize>() -> Result<String, CuError> {
    let bytes = secure_random_array::<N>().map_err(|_| {
        CuError::new(
            "managed_job_entropy_unavailable",
            "OS CSPRNG is unavailable",
        )
    })?;
    let mut output = String::with_capacity(N * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(1);

    struct Scratch {
        root: PathBuf,
        path: PathBuf,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            let sequence = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "agenterm-managed-job-{label}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let path = root.join("jobs.json");
            Self { root, path }
        }

        fn store(&self) -> ManagedJobStore {
            ManagedJobStore::open_at(&self.path).unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn owner(pid: u32) -> ResidentOwnerIdentity {
        ResidentOwnerIdentity {
            pid,
            start_identity: format!("owner-start-{pid}"),
        }
    }

    fn process(pid: u32) -> ExactProcessIdentity {
        ExactProcessIdentity {
            pid,
            start_identity: format!("child-start-{pid}"),
        }
    }

    #[test]
    fn reopens_complete_state_machine_without_plaintext_authority_or_command() {
        let scratch = Scratch::new("reopen");
        let store = scratch.store();
        let intent = store.reserve_start(Some("session-one"), 10).unwrap();
        let handle = intent.handle();
        let resident = owner(101);
        let child = process(202);
        store.claim_starting(&handle, resident.clone(), 11).unwrap();
        store
            .mark_running(&handle, &resident, child.clone(), 12)
            .unwrap();

        let reopened = scratch.store();
        let running = reopened.get(&handle.job_id).unwrap().unwrap();
        assert_eq!(running.state, ManagedJobState::Running);
        assert_eq!(running.owner.as_ref(), Some(&resident));
        assert_eq!(running.process.as_ref(), Some(&child));
        let exited = reopened
            .mark_exited(&handle, &resident, &child, 7, 13)
            .unwrap();
        assert_eq!(exited.state, ManagedJobState::Exited { exit_code: 7 });

        let disk = fs::read_to_string(&scratch.path).unwrap();
        for forbidden in ["lease-plaintext-sentinel", "command-plaintext-sentinel"] {
            assert!(!disk.contains(forbidden));
        }
        assert!(!disk.contains("lease_id"));
        assert!(!disk.contains("executable"));
        assert!(!disk.contains("arguments"));
    }

    #[test]
    fn torn_and_corrupt_documents_fail_closed() {
        let scratch = Scratch::new("corrupt");
        let store = scratch.store();
        fs::write(&scratch.path, br#"{"schema_version":1,"jobs":{ "#).unwrap();
        assert_eq!(store.list().unwrap_err().code, "managed_job_store_corrupt");

        fs::write(&scratch.path, vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        assert_eq!(store.list().unwrap_err().code, "managed_job_store_corrupt");

        fs::write(
            &scratch.path,
            br#"{"schema_version":2,"last_now_utc_ms":0,"jobs":{}}"#,
        )
        .unwrap();
        assert_eq!(store.list().unwrap_err().code, "managed_job_store_corrupt");
    }

    #[test]
    fn schema_and_record_bounds_fail_closed() {
        let scratch = Scratch::new("bounds");
        let store = scratch.store();
        let mut jobs = serde_json::Map::new();
        for index in 0..=MAX_JOBS {
            let job_id = format!("00000000-0000-4000-8000-{index:012x}");
            jobs.insert(
                job_id.clone(),
                serde_json::json!({
                    "job_id": job_id,
                    "generation": 1,
                    "nonce": "00112233445566778899aabbccddeeff",
                    "session_id": null,
                    "owner": null,
                    "process": null,
                    "state": {"kind": "start_intent"},
                    "created_at_utc_ms": 1,
                    "updated_at_utc_ms": 1,
                    "terminal_at_utc_ms": null
                }),
            );
        }
        fs::write(
            &scratch.path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "last_now_utc_ms": 1,
                "jobs": jobs
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(store.list().unwrap_err().code, "managed_job_store_corrupt");

        fs::remove_file(&scratch.path).unwrap();
        let overlong = "s".repeat(MAX_SESSION_ID_BYTES + 1);
        assert_eq!(
            store.reserve_start(Some(&overlong), 2).unwrap_err().code,
            "managed_job_identity_invalid"
        );
    }

    #[test]
    fn stable_sidecar_lock_reports_contention_without_mutation() {
        let scratch = Scratch::new("contention");
        let store = scratch.store();
        let held = PathLock::try_acquire(&store.lock_path().unwrap()).unwrap();
        assert_eq!(
            store.reserve_start(None, 1).unwrap_err().code,
            "managed_job_store_contended"
        );
        assert!(!scratch.path.exists());
        drop(held);
        assert_eq!(
            store.reserve_start(None, 1).unwrap().state,
            ManagedJobState::StartIntent
        );
    }

    #[test]
    fn refresh_blockers_count_only_resident_or_uncertain_states() {
        let scratch = Scratch::new("refresh-blockers");
        let store = scratch.store();
        let intent = store.reserve_start(None, 1).unwrap();
        assert_eq!(
            store.refresh_blockers().unwrap(),
            ManagedJobRefreshBlockers {
                blocking: 1,
                start_intent: 1,
                ..ManagedJobRefreshBlockers::default()
            }
        );

        let handle = intent.handle();
        let resident = owner(501);
        let child = process(502);
        store.claim_starting(&handle, resident.clone(), 2).unwrap();
        assert_eq!(store.refresh_blockers().unwrap().starting, 1);
        store
            .mark_running(&handle, &resident, child.clone(), 3)
            .unwrap();
        assert_eq!(store.refresh_blockers().unwrap().running, 1);
        store.mark_exited(&handle, &resident, &child, 0, 4).unwrap();
        assert_eq!(
            store.refresh_blockers().unwrap(),
            ManagedJobRefreshBlockers::default()
        );
    }

    #[test]
    fn prune_is_plan_first_bounded_and_never_removes_live_or_detached() {
        let scratch = Scratch::new("prune");
        let store = scratch.store();
        let mut terminal_ids = Vec::new();
        let mut now = 1;
        for pid in [501, 502, 503] {
            let intent = store.reserve_start(None, now).unwrap();
            now += 1;
            let resident = owner(pid);
            let child = process(pid + 100);
            store
                .claim_starting(&intent.handle(), resident.clone(), now)
                .unwrap();
            now += 1;
            store
                .mark_running(&intent.handle(), &resident, child.clone(), now)
                .unwrap();
            now += 1;
            store
                .mark_exited(&intent.handle(), &resident, &child, 0, now)
                .unwrap();
            terminal_ids.push(intent.job_id);
            now += 1;
        }
        let live = store.reserve_start(None, now).unwrap();
        now += 1;
        let detached = store.reserve_start(None, now).unwrap();
        now += 1;
        let detached_owner = owner(701);
        let detached_child = process(702);
        store
            .claim_starting(&detached.handle(), detached_owner.clone(), now)
            .unwrap();
        now += 1;
        store
            .mark_running(
                &detached.handle(),
                &detached_owner,
                detached_child.clone(),
                now,
            )
            .unwrap();
        now += 1;
        store
            .mark_detached(&detached.handle(), &detached_owner, &detached_child, now)
            .unwrap();

        let before_plan = fs::read(&scratch.path).unwrap();
        let plan = store.prune(0, 1, false, now + 1).unwrap();
        assert!(!plan.apply);
        assert_eq!(plan.total, 5);
        assert_eq!(plan.active, 1);
        assert_eq!(plan.detached, 1);
        assert_eq!(plan.uncertain, 0);
        assert_eq!(plan.terminal, 3);
        assert_eq!(plan.eligible, 2);
        assert_eq!(plan.retained_newest, 1);
        assert_eq!(plan.removed, 0);
        assert_eq!(store.list().unwrap().len(), 5);
        assert_eq!(fs::read(&scratch.path).unwrap(), before_plan);

        let applied = store.prune(0, 1, true, now + 2).unwrap();
        assert!(applied.apply);
        assert_eq!(applied.candidate_ids, plan.candidate_ids);
        assert_eq!(applied.removed, 2);
        assert_eq!(applied.remaining, 3);
        assert!(store.get(&live.job_id).unwrap().is_some());
        assert!(store.get(&detached.job_id).unwrap().is_some());
        assert!(store.get(&terminal_ids[2]).unwrap().is_some());
        assert!(store.get(&terminal_ids[0]).unwrap().is_none());
        assert!(store.get(&terminal_ids[1]).unwrap().is_none());

        let replay = store.prune(0, 1, true, now + 3).unwrap();
        assert_eq!(replay.removed, 0);
        assert_eq!(replay.remaining, 3);
    }

    #[test]
    fn unknown_or_dead_owner_is_preserved_and_never_deleted() {
        let scratch = Scratch::new("owner-liveness");
        let store = scratch.store();
        let intent = store.reserve_start(None, 1).unwrap();
        let handle = intent.handle();
        let resident = owner(303);
        let child = process(404);
        store.claim_starting(&handle, resident.clone(), 2).unwrap();
        store
            .mark_running(&handle, &resident, child.clone(), 3)
            .unwrap();

        assert_eq!(
            store
                .reconcile_owner(&handle, OwnerLiveness::Live(resident.clone()), 4)
                .unwrap(),
            OwnerReconciliation::Live
        );
        assert_eq!(
            store
                .reconcile_owner(&handle, OwnerLiveness::Unknown, 4)
                .unwrap(),
            OwnerReconciliation::PreservedUnknown
        );
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(
            store
                .reconcile_owner(&handle, OwnerLiveness::Dead, 5)
                .unwrap(),
            OwnerReconciliation::MarkedOrphanedUncertain
        );
        let preserved = store.get(&handle.job_id).unwrap().unwrap();
        assert_eq!(preserved.state, ManagedJobState::OrphanedUncertain);
        assert_eq!(preserved.owner.as_ref(), Some(&resident));
        assert_eq!(preserved.process.as_ref(), Some(&child));
    }

    #[test]
    fn exact_generation_nonce_owner_and_child_gate_every_transition() {
        let scratch = Scratch::new("identity");
        let store = scratch.store();
        let intent = store.reserve_start(None, 10).unwrap();
        let handle = intent.handle();
        let resident = owner(10);
        store.claim_starting(&handle, resident.clone(), 11).unwrap();

        let mut stale = handle.clone();
        stale.generation += 1;
        assert_eq!(
            store
                .mark_running(&stale, &resident, process(11), 12)
                .unwrap_err()
                .code,
            "managed_job_identity_changed"
        );
        assert_eq!(
            store
                .mark_running(&handle, &owner(99), process(11), 12)
                .unwrap_err()
                .code,
            "managed_job_owner_changed"
        );
        let child = process(11);
        store
            .mark_running(&handle, &resident, child.clone(), 12)
            .unwrap();
        assert_eq!(
            store
                .mark_exited(&handle, &resident, &process(12), 0, 13)
                .unwrap_err()
                .code,
            "managed_job_transition_invalid"
        );
    }

    #[test]
    fn starting_and_running_terminal_shapes_are_closed() {
        let scratch = Scratch::new("terminal-shapes");
        let store = scratch.store();

        let failed = store.reserve_start(None, 1).unwrap();
        let failed_handle = failed.handle();
        let failed_owner = owner(501);
        store
            .claim_starting(&failed_handle, failed_owner.clone(), 2)
            .unwrap();
        assert_eq!(
            store
                .mark_start_failed(&failed_handle, &failed_owner, "spawn_failed", 3)
                .unwrap()
                .state,
            ManagedJobState::StartFailed {
                code: "spawn_failed".to_owned()
            }
        );

        let signaled = store.reserve_start(None, 4).unwrap();
        let signaled_handle = signaled.handle();
        let signaled_owner = owner(502);
        let signaled_child = process(602);
        store
            .claim_starting(&signaled_handle, signaled_owner.clone(), 5)
            .unwrap();
        store
            .mark_running(&signaled_handle, &signaled_owner, signaled_child.clone(), 6)
            .unwrap();
        assert_eq!(
            store
                .mark_signaled(&signaled_handle, &signaled_owner, &signaled_child, 9, 7,)
                .unwrap()
                .state,
            ManagedJobState::Signaled { signal: 9 }
        );

        let detached = store.reserve_start(None, 8).unwrap();
        let detached_handle = detached.handle();
        let detached_owner = owner(503);
        let detached_child = process(603);
        store
            .claim_starting(&detached_handle, detached_owner.clone(), 9)
            .unwrap();
        store
            .mark_running(
                &detached_handle,
                &detached_owner,
                detached_child.clone(),
                10,
            )
            .unwrap();
        assert_eq!(
            store
                .mark_detached(&detached_handle, &detached_owner, &detached_child, 11,)
                .unwrap()
                .state,
            ManagedJobState::Detached
        );
    }

    #[test]
    fn owner_death_while_starting_preserves_the_sealed_intent() {
        let scratch = Scratch::new("starting-owner-death");
        let store = scratch.store();
        let intent = store.reserve_start(Some("session-two"), 1).unwrap();
        let handle = intent.handle();
        let resident = owner(701);
        store.claim_starting(&handle, resident.clone(), 2).unwrap();
        assert_eq!(
            store
                .reconcile_owner(&handle, OwnerLiveness::Dead, 3)
                .unwrap(),
            OwnerReconciliation::MarkedOrphanedUncertain
        );
        let preserved = store.get(&handle.job_id).unwrap().unwrap();
        assert_eq!(preserved.state, ManagedJobState::OrphanedUncertain);
        assert_eq!(preserved.owner.as_ref(), Some(&resident));
        assert!(preserved.process.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn link_like_state_is_never_read_or_replaced() {
        use std::os::unix::fs::symlink;

        let scratch = Scratch::new("symlink");
        let target = scratch.root.join("target.json");
        fs::write(&target, b"sentinel").unwrap();
        symlink(&target, &scratch.path).unwrap();
        let store = scratch.store();
        assert_eq!(
            store.reserve_start(None, 1).unwrap_err().code,
            "managed_job_store_corrupt"
        );
        assert_eq!(fs::read(&target).unwrap(), b"sentinel");
    }
}
