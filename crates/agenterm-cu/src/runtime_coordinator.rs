//! Persistent, bounded runtime-session and target-lock spine.
//!
//! The module is deliberately independent of the CLI surface.  A caller owns
//! command parsing and supplies UTC seconds explicitly; that makes expiry and
//! rollback a deterministic state-machine boundary rather than a hidden wall
//! clock read.  Every operation serializes through a sibling [`PathLock`],
//! re-reads the private document, sweeps expired sessions and their locks, and
//! publishes one fully validated replacement.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use agenterm_platform::{
    entropy::secure_random_array,
    filesystem::{host_directories, protect_private_directory, write_private_atomic},
    locking::{LockErrorKind, PathLock},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::CuError;

const SCHEMA_VERSION: u32 = 2;
const MAX_FILE_BYTES: usize = 256 * 1024;
/// A bounded shared runtime needs a bounded number of independent lease owners.
pub const MAX_SESSIONS: usize = 512;
/// Locks are independently bounded so one live session cannot grow state forever.
pub const MAX_LOCKS: usize = 4_096;
pub const MIN_TTL_SECONDS: u64 = 1;
pub const MAX_TTL_SECONDS: u64 = 86_400;
pub const DEFAULT_SESSION_LABEL: &str = "runtime";

#[derive(Clone, Debug)]
pub struct RuntimeCoordinator {
    path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionStart {
    pub session_id: String,
    /// Returned exactly once.  The durable document contains only its digest.
    pub lease: String,
    pub label: String,
    pub expires_at_utc_s: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Active,
    Ended,
    Expired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionStatus {
    pub session_id: String,
    pub label: String,
    pub state: SessionState,
    pub created_at_utc_s: i64,
    pub expires_at_utc_s: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionEnd {
    pub session: SessionStatus,
    pub released_locks: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LockStatus {
    pub lock_id: String,
    pub target: String,
    pub session_id: String,
    pub acquired_at_utc_s: i64,
    pub expires_at_utc_s: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LockAcquire {
    pub lock: LockStatus,
    pub idempotent: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    /// Monotonic state-clock high watermark.  A backward clock could renew an
    /// otherwise expired lease, so it is a typed refusal rather than a clamp.
    last_now_utc_s: i64,
    sessions: BTreeMap<String, SessionRecord>,
    locks: BTreeMap<String, LockRecord>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            last_now_utc_s: 0,
            sessions: BTreeMap::new(),
            locks: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionRecord {
    session_id: String,
    label: String,
    /// SHA-256 hex only; the plaintext lease never reaches persistent state.
    lease_sha256: String,
    state: SessionState,
    created_at_utc_s: i64,
    expires_at_utc_s: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_at_utc_s: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LockRecord {
    lock_id: String,
    target: String,
    session_id: String,
    acquired_at_utc_s: i64,
    expires_at_utc_s: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeStatusCounts {
    pub active_sessions: usize,
    pub active_locks: usize,
}

impl RuntimeCoordinator {
    /// Open the machine-local private spine at `~/.../agenterm/cu-runtime.json`.
    pub fn open() -> Result<Self, CuError> {
        if let Some(path) = std::env::var_os("AGENTERM_CU_RUNTIME_PATH") {
            return Self::open_at(PathBuf::from(path));
        }
        let directories =
            host_directories().map_err(|_| unavailable("runtime directory unavailable"))?;
        Self::open_at(
            directories
                .local_data
                .join("agenterm")
                .join("cu-runtime.json"),
        )
    }

    /// Open an explicit private state file.  The parent is created and
    /// protected before the file is read or later atomically published.
    pub fn open_at(path: impl Into<PathBuf>) -> Result<Self, CuError> {
        let path = path.into();
        let parent = parent(&path)?;
        fs::create_dir_all(parent).map_err(|_| unavailable("runtime directory unavailable"))?;
        protect_private_directory(parent)
            .map_err(|_| unavailable("runtime directory unavailable"))?;
        let coordinator = Self { path };
        // Fail early on a corrupt or unsupported durable document; missing is
        // an empty schema-zero state and does not publish until a mutation.
        let _ = coordinator.read_document()?;
        Ok(coordinator)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the effective live counts without sweeping, advancing the durable
    /// clock high-water mark or publishing an empty state document. Writers
    /// atomically replace the document, so one complete validated generation
    /// is enough for this topology summary; exact mutation admission continues
    /// to use the locked session APIs.
    pub fn status_counts(&self, now_utc_s: i64) -> Result<RuntimeStatusCounts, CuError> {
        if now_utc_s < 0 {
            return Err(CuError::new(
                "runtime_clock_invalid",
                "runtime clock must be a non-negative UTC second value",
            ));
        }
        let document = self.read_document()?.unwrap_or_default();
        if now_utc_s < document.last_now_utc_s {
            return Err(CuError::new(
                "runtime_clock_rollback",
                "runtime clock moved backward from persisted state",
            ));
        }
        let active_sessions = document
            .sessions
            .values()
            .filter(|session| {
                session.state == SessionState::Active && session.expires_at_utc_s > now_utc_s
            })
            .count();
        let active_locks = document
            .locks
            .values()
            .filter(|lock| {
                lock.expires_at_utc_s > now_utc_s
                    && document
                        .sessions
                        .get(&lock.session_id)
                        .is_some_and(|session| {
                            session.state == SessionState::Active
                                && session.expires_at_utc_s > now_utc_s
                        })
            })
            .count();
        Ok(RuntimeStatusCounts {
            active_sessions,
            active_locks,
        })
    }

    /// Serialize session-bound effect admission against terminal cleanup.
    ///
    /// The session digest selects one of 256 stable sidecar shards. This keeps
    /// the filesystem bounded while preserving mutual exclusion (a collision
    /// can only cause a typed busy result). The files are deliberately not
    /// removed: deleting a lock pathname while another process retains the
    /// opened lock object would create a second lock domain.
    pub(crate) fn acquire_session_gate(&self, session_id: &str) -> Result<PathLock, CuError> {
        if session_id.is_empty()
            || session_id.len() > 128
            || session_id.contains(['\0', '\n', '\r'])
        {
            return Err(CuError::new(
                "runtime_session_id_invalid",
                "runtime session id is invalid",
            ));
        }
        let digest = sha256_hex(session_id);
        let path =
            parent(&self.path)?.join(format!(".agenterm-session-gate-{}.lock", &digest[..2]));
        PathLock::try_acquire(&path).map_err(|error| {
            let code = if error.kind() == LockErrorKind::Contended {
                "runtime_session_busy"
            } else {
                "runtime_session_gate_unavailable"
            };
            CuError::new(code, "runtime session admission gate is unavailable")
        })
    }

    /// Serialize setup refresh with admission of future resident resources.
    /// The stable lock file is never deleted; doing so could split the lock
    /// domain while another process still owns the opened object.
    pub(crate) fn acquire_refresh_fence(&self) -> Result<PathLock, CuError> {
        let path = parent(&self.path)?.join(".agenterm-runtime-refresh.lock");
        PathLock::try_acquire(&path).map_err(|error| {
            let code = if error.kind() == LockErrorKind::Contended {
                "runtime_refresh_busy"
            } else {
                "runtime_refresh_unavailable"
            };
            CuError::new(code, "runtime refresh admission fence is unavailable")
        })
    }

    pub fn session_start(
        &self,
        label: Option<&str>,
        ttl_seconds: u64,
        now_utc_s: i64,
    ) -> Result<SessionStart, CuError> {
        let label = label.unwrap_or(DEFAULT_SESSION_LABEL);
        validate_label(label)?;
        let ttl = validate_ttl(ttl_seconds, "runtime_ttl_invalid")?;
        self.mutate(now_utc_s, |document, now| {
            evict_oldest_terminal_until_room(document);
            if document.sessions.len() >= MAX_SESSIONS {
                return Err(CuError::new(
                    "runtime_session_limit",
                    "runtime session record limit reached",
                ));
            }
            let session_id = random_uuid_v4()?;
            let lease = random_uuid_v4()?;
            let expires_at_utc_s = add_ttl(now, ttl, "runtime_ttl_invalid")?;
            document.sessions.insert(
                session_id.clone(),
                SessionRecord {
                    session_id: session_id.clone(),
                    label: label.to_owned(),
                    lease_sha256: sha256_hex(&lease),
                    state: SessionState::Active,
                    created_at_utc_s: now,
                    expires_at_utc_s,
                    terminal_at_utc_s: None,
                },
            );
            Ok(SessionStart {
                session_id,
                lease,
                label: label.to_owned(),
                expires_at_utc_s,
            })
        })
    }

    pub fn session_list(&self, now_utc_s: i64) -> Result<Vec<SessionStatus>, CuError> {
        self.inspect(now_utc_s, |document| {
            Ok(document.sessions.values().map(session_status).collect())
        })
    }

    pub fn session_status(
        &self,
        session_id: &str,
        now_utc_s: i64,
    ) -> Result<SessionStatus, CuError> {
        self.inspect(now_utc_s, |document| {
            document
                .sessions
                .get(session_id)
                .map(session_status)
                .ok_or_else(|| {
                    CuError::new(
                        "runtime_session_not_found",
                        "runtime session does not exist or has expired",
                    )
                })
        })
    }

    /// Verify that `lease` still owns one active session without renewing it.
    /// This is the admission check used by effects carrying a caller request
    /// identity; observation alone must never extend the lease.
    pub fn session_verify(
        &self,
        session_id: &str,
        lease: &str,
        now_utc_s: i64,
    ) -> Result<SessionStatus, CuError> {
        self.inspect(now_utc_s, |document| {
            let record = checked_session(document, session_id, lease)?;
            Ok(session_status(record))
        })
    }

    pub fn session_renew(
        &self,
        session_id: &str,
        lease: &str,
        ttl_seconds: u64,
        now_utc_s: i64,
    ) -> Result<SessionStatus, CuError> {
        let ttl = validate_ttl(ttl_seconds, "runtime_ttl_invalid")?;
        self.mutate(now_utc_s, |document, now| {
            let record = checked_session_mut(document, session_id, lease)?;
            record.expires_at_utc_s = add_ttl(now, ttl, "runtime_ttl_invalid")?;
            Ok(session_status(record))
        })
    }

    pub fn session_end(
        &self,
        session_id: &str,
        lease: &str,
        now_utc_s: i64,
    ) -> Result<SessionEnd, CuError> {
        self.mutate(now_utc_s, |document, now| {
            let record = document.sessions.get_mut(session_id).ok_or_else(|| {
                CuError::new(
                    "runtime_session_not_found",
                    "runtime session does not exist or has expired",
                )
            })?;
            if !constant_time_equal(record.lease_sha256.as_bytes(), sha256_hex(lease).as_bytes()) {
                return Err(CuError::new(
                    "runtime_session_lease_invalid",
                    "runtime session lease is invalid",
                ));
            }
            match record.state {
                SessionState::Active => {
                    record.state = SessionState::Ended;
                    record.terminal_at_utc_s = Some(now);
                }
                SessionState::Ended => {}
                SessionState::Expired => {
                    return Err(CuError::new(
                        "runtime_session_expired",
                        "runtime session has expired",
                    ));
                }
            }
            let session = session_status(record);
            let before = document.locks.len();
            document
                .locks
                .retain(|_, record| record.session_id != session_id);
            Ok(SessionEnd {
                session,
                released_locks: before - document.locks.len(),
            })
        })
    }

    pub fn lock_acquire(
        &self,
        session_id: &str,
        lease: &str,
        target: &str,
        ttl_seconds: u64,
        now_utc_s: i64,
    ) -> Result<LockAcquire, CuError> {
        validate_target(target)?;
        let ttl = validate_ttl(ttl_seconds, "runtime_lock_ttl_invalid")?;
        self.mutate(now_utc_s, |document, now| {
            let session = checked_session(document, session_id, lease)?;
            let expires_at_utc_s = add_ttl(now, ttl, "runtime_lock_ttl_invalid")?;
            if expires_at_utc_s > session.expires_at_utc_s {
                return Err(CuError::new(
                    "runtime_lock_ttl_invalid",
                    "lock TTL may not outlive its session lease",
                ));
            }
            if let Some(existing) = document.locks.get_mut(target) {
                if existing.session_id != session_id {
                    return Err(CuError::new(
                        "runtime_lock_conflict",
                        "target is leased by another runtime session",
                    ));
                }
                existing.expires_at_utc_s = expires_at_utc_s;
                return Ok(LockAcquire {
                    lock: lock_status(existing),
                    idempotent: true,
                });
            }
            if document.locks.len() >= MAX_LOCKS {
                return Err(CuError::new(
                    "runtime_lock_limit",
                    "runtime lock record limit reached",
                ));
            }
            let record = LockRecord {
                lock_id: random_uuid_v4()?,
                target: target.to_owned(),
                session_id: session_id.to_owned(),
                acquired_at_utc_s: now,
                expires_at_utc_s,
            };
            let result = LockAcquire {
                lock: lock_status(&record),
                idempotent: false,
            };
            document.locks.insert(target.to_owned(), record);
            Ok(result)
        })
    }

    pub fn lock_list(&self, now_utc_s: i64) -> Result<Vec<LockStatus>, CuError> {
        self.inspect(now_utc_s, |document| {
            Ok(document.locks.values().map(lock_status).collect())
        })
    }

    pub fn lock_release(
        &self,
        lock_id: &str,
        session_lease: &str,
        now_utc_s: i64,
    ) -> Result<LockStatus, CuError> {
        self.mutate(now_utc_s, |document, _| {
            let (target, existing) = document
                .locks
                .iter()
                .find(|(_, record)| record.lock_id == lock_id)
                .map(|(target, record)| (target.clone(), record.clone()))
                .ok_or_else(|| {
                    CuError::new(
                        "runtime_lock_not_found",
                        "runtime lock does not exist or has expired",
                    )
                })?;
            checked_session(document, &existing.session_id, session_lease)?;
            document.locks.remove(&target);
            Ok(lock_status(&existing))
        })
    }

    fn inspect<T>(
        &self,
        now_utc_s: i64,
        read: impl FnOnce(&Document) -> Result<T, CuError>,
    ) -> Result<T, CuError> {
        self.with_locked_document(now_utc_s, |document, changed| {
            let result = read(document)?;
            // The high watermark is a security invariant for observations as
            // well as mutations: a successful list/status call must prevent a
            // later caller from presenting an older clock value.
            *changed = true;
            Ok((result, true))
        })
    }

    fn mutate<T>(
        &self,
        now_utc_s: i64,
        edit: impl FnOnce(&mut Document, i64) -> Result<T, CuError>,
    ) -> Result<T, CuError> {
        self.with_locked_document(now_utc_s, |document, changed| {
            let result = edit(document, now_utc_s)?;
            *changed = true;
            Ok((result, true))
        })
    }

    fn with_locked_document<T>(
        &self,
        now_utc_s: i64,
        operation: impl FnOnce(&mut Document, &mut bool) -> Result<(T, bool), CuError>,
    ) -> Result<T, CuError> {
        if now_utc_s < 0 {
            return Err(CuError::new(
                "runtime_clock_invalid",
                "runtime clock must be a non-negative UTC second value",
            ));
        }
        let lock_path = self.lock_path()?;
        let _lock = PathLock::try_acquire(&lock_path).map_err(|error| {
            let code = if error.kind() == LockErrorKind::Contended {
                "runtime_lock_contended"
            } else {
                "runtime_state_unavailable"
            };
            CuError::new(code, "runtime state lock is unavailable")
        })?;
        let mut document = self.read_document()?.unwrap_or_default();
        if now_utc_s < document.last_now_utc_s {
            return Err(CuError::new(
                "runtime_clock_rollback",
                "runtime clock moved backward from persisted state",
            ));
        }
        let mut changed = sweep_expired(&mut document, now_utc_s);
        let (result, requested_write) = operation(&mut document, &mut changed)?;
        changed |= requested_write;
        if changed {
            document.last_now_utc_s = now_utc_s;
            validate_document(&document)?;
            self.write_document(&document)?;
        }
        Ok(result)
    }

    fn lock_path(&self) -> Result<PathBuf, CuError> {
        let file_name = self
            .path
            .file_name()
            .ok_or_else(|| unavailable("runtime state path has no file name"))?;
        let mut lock = file_name.to_os_string();
        lock.push(".lock");
        Ok(parent(&self.path)?.join(lock))
    }

    fn read_document(&self) -> Result<Option<Document>, CuError> {
        let raw = match fs::read(&self.path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable("runtime state could not be read")),
        };
        if raw.len() > MAX_FILE_BYTES {
            return Err(CuError::new(
                "runtime_state_corrupt",
                "runtime state exceeds its byte ceiling",
            ));
        }
        let document: Document = serde_json::from_slice(&raw).map_err(|_| {
            CuError::new(
                "runtime_state_corrupt",
                "runtime state is not a valid schema document",
            )
        })?;
        validate_document(&document)?;
        Ok(Some(document))
    }

    fn write_document(&self, document: &Document) -> Result<(), CuError> {
        let bytes = serde_json::to_vec(document).map_err(|_| {
            CuError::new(
                "runtime_state_serialization",
                "runtime state could not be serialized",
            )
        })?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(CuError::new(
                "runtime_state_limit",
                "runtime state exceeds its byte ceiling",
            ));
        }
        write_private_atomic(&self.path, &bytes).map_err(|_| {
            CuError::new(
                "runtime_state_publish",
                "runtime state could not be atomically published",
            )
        })
    }
}

fn parent(path: &Path) -> Result<&Path, CuError> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| unavailable("runtime state requires an explicit parent directory"))
}

fn unavailable(message: &'static str) -> CuError {
    CuError::new("runtime_state_unavailable", message)
}

fn validate_ttl(ttl: u64, code: &'static str) -> Result<i64, CuError> {
    if !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&ttl) {
        return Err(CuError::new(code, "TTL must be within 1..=86400 seconds"));
    }
    Ok(ttl as i64)
}

fn add_ttl(now: i64, ttl: i64, code: &'static str) -> Result<i64, CuError> {
    now.checked_add(ttl)
        .ok_or_else(|| CuError::new(code, "TTL overflows the runtime clock"))
}

fn validate_target(target: &str) -> Result<(), CuError> {
    let valid = (1..=512).contains(&target.len())
        && !target.contains(['\n', '\r'])
        && !target.chars().any(char::is_control)
        && target
            .split_once(':')
            .is_some_and(|(namespace, name)| !namespace.is_empty() && !name.is_empty());
    if valid {
        Ok(())
    } else {
        Err(CuError::new(
            "runtime_lock_target_invalid",
            "lock target must be one 1..=512-byte namespace:name line",
        ))
    }
}

fn validate_label(label: &str) -> Result<(), CuError> {
    if (1..=128).contains(&label.len())
        && !label.contains(['\n', '\r'])
        && !label.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(CuError::new(
            "runtime_session_label_invalid",
            "session label must be one 1..=128-byte line",
        ))
    }
}

fn checked_session<'a>(
    document: &'a Document,
    session_id: &str,
    lease: &str,
) -> Result<&'a SessionRecord, CuError> {
    let record = document.sessions.get(session_id).ok_or_else(|| {
        CuError::new(
            "runtime_session_not_found",
            "runtime session does not exist or has expired",
        )
    })?;
    if !constant_time_equal(record.lease_sha256.as_bytes(), sha256_hex(lease).as_bytes()) {
        Err(CuError::new(
            "runtime_session_lease_invalid",
            "runtime session lease is invalid",
        ))
    } else {
        match record.state {
            SessionState::Active => Ok(record),
            SessionState::Ended => Err(CuError::new(
                "runtime_session_ended",
                "runtime session has ended",
            )),
            SessionState::Expired => Err(CuError::new(
                "runtime_session_expired",
                "runtime session has expired",
            )),
        }
    }
}

fn checked_session_mut<'a>(
    document: &'a mut Document,
    session_id: &str,
    lease: &str,
) -> Result<&'a mut SessionRecord, CuError> {
    let record = document.sessions.get_mut(session_id).ok_or_else(|| {
        CuError::new(
            "runtime_session_not_found",
            "runtime session does not exist or has expired",
        )
    })?;
    if !constant_time_equal(record.lease_sha256.as_bytes(), sha256_hex(lease).as_bytes()) {
        Err(CuError::new(
            "runtime_session_lease_invalid",
            "runtime session lease is invalid",
        ))
    } else {
        match record.state {
            SessionState::Active => Ok(record),
            SessionState::Ended => Err(CuError::new(
                "runtime_session_ended",
                "runtime session has ended",
            )),
            SessionState::Expired => Err(CuError::new(
                "runtime_session_expired",
                "runtime session has expired",
            )),
        }
    }
}

fn sweep_expired(document: &mut Document, now: i64) -> bool {
    let mut changed = false;
    for record in document.sessions.values_mut() {
        if record.state == SessionState::Active && record.expires_at_utc_s <= now {
            record.state = SessionState::Expired;
            record.terminal_at_utc_s = Some(now);
            changed = true;
        }
    }
    let before = document.locks.len();
    document.locks.retain(|_, record| {
        record.expires_at_utc_s > now
            && document
                .sessions
                .get(&record.session_id)
                .is_some_and(|session| session.state == SessionState::Active)
    });
    changed || before != document.locks.len()
}

fn evict_oldest_terminal_until_room(document: &mut Document) {
    while document.sessions.len() >= MAX_SESSIONS {
        let oldest = document
            .sessions
            .values()
            .filter(|record| record.state != SessionState::Active)
            .min_by_key(|record| {
                (
                    record.terminal_at_utc_s.unwrap_or(i64::MAX),
                    &record.session_id,
                )
            })
            .map(|record| record.session_id.clone());
        let Some(session_id) = oldest else { break };
        document.sessions.remove(&session_id);
    }
}

fn validate_document(document: &Document) -> Result<(), CuError> {
    if document.schema_version != SCHEMA_VERSION {
        return Err(CuError::new(
            "runtime_state_schema",
            "runtime state schema version is unsupported",
        ));
    }
    if document.last_now_utc_s < 0 {
        return Err(CuError::new(
            "runtime_state_corrupt",
            "runtime state clock is invalid",
        ));
    }
    if document.sessions.len() > MAX_SESSIONS || document.locks.len() > MAX_LOCKS {
        return Err(CuError::new(
            "runtime_state_corrupt",
            "runtime state record ceiling exceeded",
        ));
    }
    for (id, record) in &document.sessions {
        if id != &record.session_id
            || !valid_uuid_v4(&record.session_id)
            || validate_label(&record.label).is_err()
            || !valid_digest(&record.lease_sha256)
            || record.created_at_utc_s < 0
            || record.expires_at_utc_s <= record.created_at_utc_s
            || matches!(record.state, SessionState::Active) != record.terminal_at_utc_s.is_none()
            || record
                .terminal_at_utc_s
                .is_some_and(|at| at < record.created_at_utc_s)
        {
            return Err(CuError::new(
                "runtime_state_corrupt",
                "runtime session record is invalid",
            ));
        }
    }
    for (target, record) in &document.locks {
        let owner = document.sessions.get(&record.session_id);
        if target != &record.target
            || validate_target(&record.target).is_err()
            || !valid_uuid_v4(&record.lock_id)
            || owner.is_none()
            || record.expires_at_utc_s < 0
            || record.acquired_at_utc_s < 0
            || record.expires_at_utc_s <= record.acquired_at_utc_s
            || owner.is_some_and(|session| record.expires_at_utc_s > session.expires_at_utc_s)
            || owner.is_some_and(|session| session.state != SessionState::Active)
        {
            return Err(CuError::new(
                "runtime_state_corrupt",
                "runtime lock record is invalid",
            ));
        }
    }
    Ok(())
}

fn session_status(record: &SessionRecord) -> SessionStatus {
    SessionStatus {
        session_id: record.session_id.clone(),
        label: record.label.clone(),
        state: record.state,
        created_at_utc_s: record.created_at_utc_s,
        expires_at_utc_s: record.expires_at_utc_s,
    }
}
fn lock_status(record: &LockRecord) -> LockStatus {
    LockStatus {
        lock_id: record.lock_id.clone(),
        target: record.target.clone(),
        session_id: record.session_id.clone(),
        acquired_at_utc_s: record.acquired_at_utc_s,
        expires_at_utc_s: record.expires_at_utc_s,
    }
}

fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let longest = left.len().max(right.len());
    for index in 0..longest {
        difference |= usize::from(*left.get(index).unwrap_or(&0) ^ *right.get(index).unwrap_or(&0));
    }
    difference == 0
}

fn random_uuid_v4() -> Result<String, CuError> {
    let mut bytes = secure_random_array::<16>()
        .map_err(|_| CuError::new("runtime_entropy_unavailable", "OS CSPRNG is unavailable"))?;
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

fn valid_uuid_v4(value: &str) -> bool {
    value.len() == 36
        && value.as_bytes().get(8) == Some(&b'-')
        && value.as_bytes().get(13) == Some(&b'-')
        && value.as_bytes().get(18) == Some(&b'-')
        && value.as_bytes().get(23) == Some(&b'-')
        && value.as_bytes().get(14) == Some(&b'4')
        && matches!(value.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23)
                || byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
        })
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPath(PathBuf);
    impl TestPath {
        fn new(name: &str) -> Self {
            let root = std::fs::canonicalize(std::env::temp_dir())
                .expect("resolve test temp root")
                .join(format!("agenterm-cu-runtime-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            Self(root.join("runtime.json"))
        }
    }
    impl Drop for TestPath {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    fn open(name: &str) -> (TestPath, RuntimeCoordinator) {
        let path = TestPath::new(name);
        let coordinator = RuntimeCoordinator::open_at(&path.0).unwrap();
        (path, coordinator)
    }

    #[test]
    fn status_counts_are_effective_and_never_publish_or_sweep() {
        let (path, coordinator) = open("status-counts");
        assert_eq!(
            coordinator.status_counts(100).unwrap(),
            RuntimeStatusCounts {
                active_sessions: 0,
                active_locks: 0,
            }
        );
        assert!(!path.0.exists());

        let started = coordinator.session_start(Some("court"), 10, 100).unwrap();
        coordinator
            .lock_acquire(&started.session_id, &started.lease, "desktop:1", 5, 100)
            .unwrap();
        assert_eq!(
            coordinator.status_counts(101).unwrap(),
            RuntimeStatusCounts {
                active_sessions: 1,
                active_locks: 1,
            }
        );
        let before = fs::read(&path.0).unwrap();
        assert_eq!(
            coordinator.status_counts(111).unwrap(),
            RuntimeStatusCounts {
                active_sessions: 0,
                active_locks: 0,
            }
        );
        assert_eq!(fs::read(&path.0).unwrap(), before);
    }

    #[test]
    fn plaintext_lease_never_reaches_disk_and_renewal_is_bounded() {
        let (path, coordinator) = open("lease-not-on-disk");
        let started = coordinator
            .session_start(Some("lease test"), 10, 100)
            .unwrap();
        let persisted = fs::read_to_string(&path.0).unwrap();
        assert!(!persisted.contains(&started.lease));
        assert!(persisted.contains("lease_sha256"));
        assert_eq!(
            coordinator
                .session_status(&started.session_id, 100)
                .unwrap()
                .label,
            "lease test"
        );
        assert_eq!(
            coordinator
                .session_verify(&started.session_id, &started.lease, 100)
                .unwrap()
                .state,
            SessionState::Active
        );
        assert_eq!(
            coordinator
                .session_verify(&started.session_id, "wrong", 100)
                .unwrap_err()
                .code,
            "runtime_session_lease_invalid"
        );
        let renewed = coordinator
            .session_renew(&started.session_id, &started.lease, 20, 105)
            .unwrap();
        assert_eq!(renewed.expires_at_utc_s, 125);
        assert_eq!(
            coordinator
                .session_renew(&started.session_id, "wrong", 10, 106)
                .unwrap_err()
                .code,
            "runtime_session_lease_invalid"
        );
    }

    #[test]
    fn lock_conflict_is_idempotent_for_its_owner_and_released_on_session_expiry() {
        let (_path, coordinator) = open("locks");
        let first = coordinator.session_start(Some("first"), 20, 100).unwrap();
        let second = coordinator.session_start(Some("second"), 20, 100).unwrap();
        let lock = coordinator
            .lock_acquire(&first.session_id, &first.lease, "test:one", 10, 101)
            .unwrap();
        let renewed = coordinator
            .lock_acquire(&first.session_id, &first.lease, "test:one", 15, 102)
            .unwrap();
        assert!(!lock.idempotent);
        assert!(renewed.idempotent);
        assert_eq!(lock.lock.lock_id, renewed.lock.lock_id);
        assert_eq!(lock.lock.acquired_at_utc_s, renewed.lock.acquired_at_utc_s);
        assert_eq!(
            coordinator
                .lock_acquire(&second.session_id, &second.lease, "test:one", 5, 103)
                .unwrap_err()
                .code,
            "runtime_lock_conflict"
        );
        assert!(coordinator.lock_list(120).unwrap().is_empty());
        let expired = coordinator.session_status(&first.session_id, 120).unwrap();
        assert_eq!(expired.state, SessionState::Expired);
        assert_eq!(
            coordinator
                .session_renew(&first.session_id, &first.lease, 1, 120)
                .unwrap_err()
                .code,
            "runtime_session_expired"
        );
    }

    #[test]
    fn corrupt_clock_and_sidecar_contention_fail_typed() {
        let (path, coordinator) = open("corrupt-clock-lock");
        fs::write(&path.0, b"{").unwrap();
        assert_eq!(
            RuntimeCoordinator::open_at(&path.0).unwrap_err().code,
            "runtime_state_corrupt"
        );
        fs::remove_file(&path.0).unwrap();
        let started = coordinator.session_start(Some("clock"), 10, 10).unwrap();
        coordinator.session_status(&started.session_id, 11).unwrap();
        assert_eq!(
            coordinator.session_list(10).unwrap_err().code,
            "runtime_clock_rollback"
        );
        let held = PathLock::try_acquire(coordinator.lock_path().unwrap().as_path()).unwrap();
        assert_eq!(
            coordinator
                .session_status(&started.session_id, 11)
                .unwrap_err()
                .code,
            "runtime_lock_contended"
        );
        drop(held);
    }

    #[test]
    fn refresh_fence_is_one_stable_admission_domain() {
        let (path, coordinator) = open("refresh-fence");
        let held = coordinator.acquire_refresh_fence().unwrap();
        let error = match coordinator.acquire_refresh_fence() {
            Ok(_) => panic!("second refresh fence acquisition must contend"),
            Err(error) => error,
        };
        assert_eq!(error.code, "runtime_refresh_busy");
        let fence_path = path
            .0
            .parent()
            .unwrap()
            .join(".agenterm-runtime-refresh.lock");
        assert!(fence_path.is_file());
        drop(held);
        let reacquired = coordinator.acquire_refresh_fence().unwrap();
        assert!(fence_path.is_file());
        drop(reacquired);
    }

    #[test]
    fn targets_are_closed_shape_and_lock_release_checks_owner() {
        let (_path, coordinator) = open("targets");
        let first = coordinator.session_start(Some("first"), 30, 1).unwrap();
        let second = coordinator.session_start(Some("second"), 30, 1).unwrap();
        assert_eq!(
            coordinator
                .lock_acquire(&first.session_id, &first.lease, "not-a-target", 1, 2)
                .unwrap_err()
                .code,
            "runtime_lock_target_invalid"
        );
        let lock = coordinator
            .lock_acquire(&first.session_id, &first.lease, "ns:name", 1, 2)
            .unwrap();
        assert_eq!(
            coordinator
                .lock_release(&lock.lock.lock_id, &second.lease, 2)
                .unwrap_err()
                .code,
            "runtime_session_lease_invalid"
        );
        let released = coordinator
            .lock_release(&lock.lock.lock_id, &first.lease, 2)
            .unwrap();
        assert_eq!(released.lock_id, lock.lock.lock_id);
    }

    #[test]
    fn end_preserves_terminal_status_and_releases_all_owned_locks() {
        let (_path, coordinator) = open("terminal-history");
        let session = coordinator
            .session_start(Some("retired worker"), 30, 1)
            .unwrap();
        coordinator
            .lock_acquire(&session.session_id, &session.lease, "ns:first", 5, 2)
            .unwrap();
        coordinator
            .lock_acquire(&session.session_id, &session.lease, "ns:second", 5, 2)
            .unwrap();
        let ended = coordinator
            .session_end(&session.session_id, &session.lease, 3)
            .unwrap();
        assert_eq!(ended.session.label, "retired worker");
        assert_eq!(ended.session.state, SessionState::Ended);
        assert_eq!(ended.released_locks, 2);
        assert!(coordinator.lock_list(3).unwrap().is_empty());
        let repeated = coordinator
            .session_end(&session.session_id, &session.lease, 3)
            .unwrap();
        assert_eq!(repeated.session, ended.session);
        assert_eq!(repeated.released_locks, 0);
        assert_eq!(
            coordinator
                .session_end(&session.session_id, "wrong-lease", 3)
                .unwrap_err()
                .code,
            "runtime_session_lease_invalid"
        );
        assert_eq!(
            coordinator
                .session_renew(&session.session_id, &session.lease, 1, 3)
                .unwrap_err()
                .code,
            "runtime_session_ended"
        );
    }

    #[test]
    fn session_gate_is_stable_contended_and_released_without_deletion() {
        let (_path, coordinator) = open("session-gate");
        let session = coordinator.session_start(None, 30, 1).unwrap();
        let first = coordinator
            .acquire_session_gate(&session.session_id)
            .unwrap();
        let error = match coordinator.acquire_session_gate(&session.session_id) {
            Ok(_) => panic!("second session gate unexpectedly acquired"),
            Err(error) => error,
        };
        assert_eq!(error.code, "runtime_session_busy");
        drop(first);
        let second = coordinator
            .acquire_session_gate(&session.session_id)
            .unwrap();
        drop(second);
        let parent = coordinator.path().parent().unwrap();
        assert!(parent.read_dir().unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".agenterm-session-gate-")
        }));
    }

    #[test]
    fn default_and_invalid_labels_are_closed() {
        let (_path, coordinator) = open("labels");
        let defaulted = coordinator.session_start(None, 10, 1).unwrap();
        assert_eq!(defaulted.label, DEFAULT_SESSION_LABEL);
        assert_eq!(
            coordinator
                .session_start(Some("bad\nlabel"), 10, 1)
                .unwrap_err()
                .code,
            "runtime_session_label_invalid"
        );
    }
}
