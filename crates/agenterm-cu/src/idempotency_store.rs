//! Durable, bounded caller-request idempotency.
//!
//! The caller supplies a stable, non-secret request id and a fingerprint of a
//! canonical request projection. The projection may contain private command
//! data but exists only in memory; only its domain-separated SHA-256 reaches
//! the private store. A fresh reservation returns an opaque completion token;
//! only its SHA-256 digest reaches disk.  A retry with the same request id and
//! fingerprint either replays finalized metadata or reports an uncertain
//! `reserved` / `outcome_unknown` state.  It never authorizes a second effect.
//!
//! Cooperating processes serialize through a stable sibling [`PathLock`].  The
//! replaceable JSON document is never used as the lock inode.  Each mutation
//! re-reads and validates the complete bounded document while holding the
//! sidecar, then publishes through [`write_private_atomic`].

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
use sha2::{Digest, Sha256};

use crate::CuError;

const SCHEMA_VERSION: u32 = 2;
const OLDEST_READABLE_SCHEMA_VERSION: u32 = 1;
const FINGERPRINT_DOMAIN: &[u8] = b"agenterm-cu/idempotency-request/v1\0";
const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 1_024;
const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_CANONICAL_BYTES: usize = 1024 * 1024;
const MAX_OUTCOME_CODE_BYTES: usize = 96;
pub const MIN_RETENTION_TTL_MS: i64 = 1_000;
pub const MAX_RETENTION_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug)]
pub struct IdempotencyStore {
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Reserved,
    Finalized,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalOutcomeKind {
    Succeeded,
    Failed,
}

/// Minimal non-secret public data needed to replay a completed effect without
/// executing it again. Never add bearer authority or private launch material.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum FinalReplay {
    JobSpawn { job_id: String, generation: u64 },
    DeviceClaim { lease_id: String, generation: u64 },
}

/// Bounded, non-secret outcome metadata retained for exact replay.
///
/// Human messages, command output, paths, request bodies and credentials do
/// not belong here.  `code` is a closed-shape machine token and
/// `receipt_sha256`, when present, binds a separately owned receipt without
/// copying that receipt into the idempotency document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FinalOutcome {
    pub kind: FinalOutcomeKind,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<FinalReplay>,
}

impl FinalOutcome {
    pub fn new(
        kind: FinalOutcomeKind,
        code: impl Into<String>,
        receipt_sha256: Option<String>,
    ) -> Result<Self, CuError> {
        let outcome = Self {
            kind,
            code: code.into(),
            receipt_sha256,
            replay: None,
        };
        validate_outcome(&outcome)?;
        Ok(outcome)
    }

    pub fn with_replay(mut self, replay: FinalReplay) -> Result<Self, CuError> {
        self.replay = Some(replay);
        validate_outcome(&self)?;
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RequestStatus {
    pub request_id: String,
    pub fingerprint_sha256: String,
    pub state: RequestState,
    pub created_at_utc_ms: i64,
    pub updated_at_utc_ms: i64,
    pub expires_at_utc_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<FinalOutcome>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct FreshReservation {
    pub status: RequestStatus,
    /// Returned only for a newly created reservation.  Persistent state holds
    /// its SHA-256 digest, never this bearer token.
    pub completion_token: String,
}

impl std::fmt::Debug for FreshReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FreshReservation")
            .field("status", &self.status)
            .field("completion_token", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReserveDecision {
    Fresh(FreshReservation),
    ReplayFinalized(RequestStatus),
    /// The first caller may already have performed the effect.  Retrying must
    /// fail closed instead of dispatching the mutation again.
    Uncertain(RequestStatus),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    last_now_utc_ms: i64,
    records: BTreeMap<String, RequestRecord>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            last_now_utc_ms: 0,
            records: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestRecord {
    request_id: String,
    fingerprint_sha256: String,
    completion_token_sha256: String,
    state: RequestState,
    created_at_utc_ms: i64,
    updated_at_utc_ms: i64,
    expires_at_utc_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<FinalOutcome>,
}

impl IdempotencyStore {
    /// Open the machine-local private store.
    pub fn open() -> Result<Self, CuError> {
        if let Some(path) = std::env::var_os("AGENTERM_CU_IDEMPOTENCY_PATH") {
            return Self::open_at(PathBuf::from(path));
        }
        let directories = host_directories().map_err(|_| unavailable())?;
        Self::open_at(
            directories
                .local_data
                .join("agenterm")
                .join("cu-idempotency.json"),
        )
    }

    /// Open an injected state path.  Missing state means an empty store;
    /// malformed, oversized or link-like state fails closed.
    pub fn open_at(path: impl Into<PathBuf>) -> Result<Self, CuError> {
        let path = path.into();
        let parent = explicit_parent(&path)?;
        fs::create_dir_all(parent).map_err(|_| unavailable())?;
        protect_private_directory(parent).map_err(|_| unavailable())?;
        let store = Self { path };
        let _ = store.read_document()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read an existing request outcome without creating, refreshing or
    /// deleting any record. This is the first step for effect APIs whose
    /// short-lived approval may expire before a caller asks for the durable
    /// terminal receipt again.
    pub fn lookup(
        &self,
        request_id: &str,
        fingerprint_sha256: &str,
        now_utc_ms: i64,
    ) -> Result<Option<RequestStatus>, CuError> {
        validate_request_id(request_id)?;
        validate_digest(fingerprint_sha256, "request_fingerprint_invalid")?;
        validate_now(now_utc_ms)?;

        let lock_path = self.lock_path()?;
        let _lock = PathLock::try_acquire(&lock_path).map_err(|error| {
            let code = if error.kind() == LockErrorKind::Contended {
                "request_store_lock_contended"
            } else {
                "request_store_unavailable"
            };
            CuError::new(code, "request idempotency store lock is unavailable")
        })?;
        let document = self.read_document()?.unwrap_or_default();
        if now_utc_ms < document.last_now_utc_ms {
            return Err(CuError::new(
                "request_store_clock_rollback",
                "request idempotency clock moved backward from persisted state",
            ));
        }
        let Some(record) = document.records.get(request_id) else {
            return Ok(None);
        };
        if record.expires_at_utc_ms <= now_utc_ms {
            return Ok(None);
        }
        ensure_fingerprint(record, fingerprint_sha256)?;
        Ok(Some(public_status(record)))
    }

    /// Durably reserve a caller identity before its external side effect.
    ///
    /// `fingerprint_sha256` should come from [`fingerprint_canonical_request`].
    /// The retention TTL defines the bounded replay window.  Within that
    /// window a same-id retry is never returned as `Fresh`.
    pub fn reserve(
        &self,
        request_id: &str,
        fingerprint_sha256: &str,
        retention_ttl_ms: i64,
        now_utc_ms: i64,
    ) -> Result<ReserveDecision, CuError> {
        validate_request_id(request_id)?;
        validate_digest(fingerprint_sha256, "request_fingerprint_invalid")?;
        validate_now(now_utc_ms)?;
        validate_retention_ttl(retention_ttl_ms)?;

        self.mutate(now_utc_ms, |document| {
            if let Some(record) = document.records.get(request_id) {
                ensure_fingerprint(record, fingerprint_sha256)?;
                let status = public_status(record);
                return Ok(match record.state {
                    RequestState::Finalized => ReserveDecision::ReplayFinalized(status),
                    RequestState::Reserved | RequestState::OutcomeUnknown => {
                        ReserveDecision::Uncertain(status)
                    }
                });
            }
            if document.records.len() >= MAX_RECORDS {
                return Err(CuError::new(
                    "request_store_limit",
                    "request idempotency record limit reached",
                ));
            }

            let completion_token = random_token()?;
            let expires_at_utc_ms = now_utc_ms
                .checked_add(retention_ttl_ms)
                .ok_or_else(|| ttl_invalid("retention deadline overflows UTC milliseconds"))?;
            let record = RequestRecord {
                request_id: request_id.to_owned(),
                fingerprint_sha256: fingerprint_sha256.to_owned(),
                completion_token_sha256: sha256_hex(completion_token.as_bytes()),
                state: RequestState::Reserved,
                created_at_utc_ms: now_utc_ms,
                updated_at_utc_ms: now_utc_ms,
                expires_at_utc_ms,
                outcome: None,
            };
            let status = public_status(&record);
            document.records.insert(request_id.to_owned(), record);
            Ok(ReserveDecision::Fresh(FreshReservation {
                status,
                completion_token,
            }))
        })
    }

    /// Close a reservation with bounded, non-secret outcome metadata.
    ///
    /// Repeating the same finalization is idempotent.  A different outcome for
    /// an already-finalized identity is a typed conflict.
    pub fn finalize(
        &self,
        request_id: &str,
        fingerprint_sha256: &str,
        completion_token: &str,
        outcome: FinalOutcome,
        now_utc_ms: i64,
    ) -> Result<RequestStatus, CuError> {
        validate_request_id(request_id)?;
        validate_digest(fingerprint_sha256, "request_fingerprint_invalid")?;
        validate_completion_token(completion_token)?;
        validate_outcome(&outcome)?;
        validate_now(now_utc_ms)?;

        self.mutate(now_utc_ms, |document| {
            let record = document.records.get_mut(request_id).ok_or_else(|| {
                CuError::new(
                    "request_reservation_not_found",
                    "request reservation is not present within its retention window",
                )
            })?;
            ensure_fingerprint(record, fingerprint_sha256)?;
            ensure_completion_token(record, completion_token)?;
            if record.state == RequestState::Finalized {
                if record.outcome.as_ref() != Some(&outcome) {
                    return Err(CuError::new(
                        "request_outcome_conflict",
                        "request identity was already finalized with a different outcome",
                    ));
                }
                return Ok(public_status(record));
            }
            record.state = RequestState::Finalized;
            record.updated_at_utc_ms = now_utc_ms;
            record.outcome = Some(outcome);
            Ok(public_status(record))
        })
    }

    /// Persist that transport or mechanism completion is not knowable.
    ///
    /// A later same-id reservation remains uncertain.  If the original owner
    /// subsequently obtains authoritative completion evidence, its token may
    /// still transition this record to `finalized`.
    pub fn mark_outcome_unknown(
        &self,
        request_id: &str,
        fingerprint_sha256: &str,
        completion_token: &str,
        now_utc_ms: i64,
    ) -> Result<RequestStatus, CuError> {
        validate_request_id(request_id)?;
        validate_digest(fingerprint_sha256, "request_fingerprint_invalid")?;
        validate_completion_token(completion_token)?;
        validate_now(now_utc_ms)?;

        self.mutate(now_utc_ms, |document| {
            let record = document.records.get_mut(request_id).ok_or_else(|| {
                CuError::new(
                    "request_reservation_not_found",
                    "request reservation is not present within its retention window",
                )
            })?;
            ensure_fingerprint(record, fingerprint_sha256)?;
            ensure_completion_token(record, completion_token)?;
            if record.state == RequestState::Reserved {
                record.state = RequestState::OutcomeUnknown;
                record.updated_at_utc_ms = now_utc_ms;
            }
            Ok(public_status(record))
        })
    }

    fn mutate<T>(
        &self,
        now_utc_ms: i64,
        operation: impl FnOnce(&mut Document) -> Result<T, CuError>,
    ) -> Result<T, CuError> {
        let lock_path = self.lock_path()?;
        let _lock = PathLock::try_acquire(&lock_path).map_err(|error| {
            let code = if error.kind() == LockErrorKind::Contended {
                "request_store_lock_contended"
            } else {
                "request_store_unavailable"
            };
            CuError::new(code, "request idempotency store lock is unavailable")
        })?;
        let mut document = self.read_document()?.unwrap_or_default();
        if now_utc_ms < document.last_now_utc_ms {
            return Err(CuError::new(
                "request_store_clock_rollback",
                "request idempotency clock moved backward from persisted state",
            ));
        }
        document
            .records
            .retain(|_, record| record.expires_at_utc_ms > now_utc_ms);
        let result = operation(&mut document)?;
        document.schema_version = SCHEMA_VERSION;
        document.last_now_utc_ms = now_utc_ms;
        validate_document(&document)?;
        self.write_document(&document)?;
        Ok(result)
    }

    fn lock_path(&self) -> Result<PathBuf, CuError> {
        let file_name = self
            .path
            .file_name()
            .ok_or_else(|| CuError::new("request_store_path_invalid", "state path has no name"))?;
        let mut lock_name = file_name.to_os_string();
        lock_name.push(".lock");
        Ok(explicit_parent(&self.path)?.join(lock_name))
    }

    fn read_document(&self) -> Result<Option<Document>, CuError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => return Err(unavailable()),
        };
        let Some(metadata) = metadata else {
            return Ok(None);
        };
        if metadata_is_link_like(&metadata) || !metadata.is_file() {
            return Err(CuError::new(
                "request_store_corrupt",
                "request idempotency state must be one regular, non-link file",
            ));
        }
        if metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(CuError::new(
                "request_store_corrupt",
                "request idempotency state exceeds its byte ceiling",
            ));
        }
        let raw = fs::read(&self.path).map_err(|_| unavailable())?;
        let document: Document = serde_json::from_slice(&raw).map_err(|_| {
            CuError::new(
                "request_store_corrupt",
                "request idempotency state is not valid schema JSON",
            )
        })?;
        validate_document(&document)?;
        Ok(Some(document))
    }

    fn write_document(&self, document: &Document) -> Result<(), CuError> {
        let bytes = serde_json::to_vec(document).map_err(|_| {
            CuError::new(
                "request_store_serialization",
                "request idempotency state could not be serialized",
            )
        })?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(CuError::new(
                "request_store_limit",
                "request idempotency state exceeds its byte ceiling",
            ));
        }
        write_private_atomic(&self.path, &bytes).map_err(|_| {
            CuError::new(
                "request_store_publish",
                "request idempotency state could not be atomically published",
            )
        })
    }
}

/// Hash one already-canonicalized request projection.
///
/// Length framing and a versioned domain separator prevent accidental reuse
/// as a generic content digest. The projection is never persisted, but bearer
/// credentials must still be excluded: unlike high-entropy command material,
/// low-entropy credentials may be guessable from an unkeyed digest.
pub fn fingerprint_canonical_request(canonical_request: &[u8]) -> Result<String, CuError> {
    if canonical_request.len() > MAX_CANONICAL_BYTES {
        return Err(CuError::new(
            "request_fingerprint_input_limit",
            "canonical request projection exceeds its byte ceiling",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_DOMAIN);
    hasher.update((canonical_request.len() as u64).to_be_bytes());
    hasher.update(canonical_request);
    Ok(hex_digest(&hasher.finalize()))
}

/// Fingerprint a possibly-sensitive command projection under an ephemeral
/// high-entropy bearer secret. The secret itself and its intermediate digest
/// are never persisted, so a leaked private request store cannot be used to
/// confirm guesses about low-entropy command values without also possessing
/// the live session lease.
pub fn fingerprint_canonical_request_with_secret(
    canonical_request: &[u8],
    secret: &[u8],
) -> Result<String, CuError> {
    if secret.len() < 32 || secret.len() > 4_096 {
        return Err(CuError::new(
            "request_fingerprint_secret_invalid",
            "request fingerprint secret must be 32..=4096 bytes",
        ));
    }
    if canonical_request.len() > MAX_CANONICAL_BYTES {
        return Err(CuError::new(
            "request_fingerprint_input_limit",
            "canonical request projection exceeds its byte ceiling",
        ));
    }
    let secret_digest = Sha256::digest(secret);
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_DOMAIN);
    hasher.update(b"secret-bound\0");
    hasher.update(secret_digest);
    hasher.update((canonical_request.len() as u64).to_be_bytes());
    hasher.update(canonical_request);
    Ok(hex_digest(&hasher.finalize()))
}

fn validate_document(document: &Document) -> Result<(), CuError> {
    if !(OLDEST_READABLE_SCHEMA_VERSION..=SCHEMA_VERSION).contains(&document.schema_version)
        || document.last_now_utc_ms < 0
        || document.records.len() > MAX_RECORDS
    {
        return Err(CuError::new(
            "request_store_corrupt",
            "request idempotency state violates its schema or bounds",
        ));
    }
    for (key, record) in &document.records {
        validate_request_id(&record.request_id).map_err(as_corrupt)?;
        if key != &record.request_id {
            return Err(CuError::new(
                "request_store_corrupt",
                "request idempotency map key does not match its record",
            ));
        }
        validate_digest(&record.fingerprint_sha256, "request_store_corrupt").map_err(as_corrupt)?;
        validate_digest(&record.completion_token_sha256, "request_store_corrupt")
            .map_err(as_corrupt)?;
        if record.created_at_utc_ms < 0
            || record.updated_at_utc_ms < record.created_at_utc_ms
            || record.expires_at_utc_ms <= record.created_at_utc_ms
            || record.updated_at_utc_ms >= record.expires_at_utc_ms
        {
            return Err(CuError::new(
                "request_store_corrupt",
                "request idempotency timestamps violate their ordering",
            ));
        }
        match (&record.state, &record.outcome) {
            (RequestState::Finalized, Some(outcome)) => {
                validate_outcome(outcome).map_err(as_corrupt)?;
                if document.schema_version == 1 && outcome.replay.is_some() {
                    return Err(CuError::new(
                        "request_store_corrupt",
                        "schema-one request state cannot contain replay metadata",
                    ));
                }
            }
            (RequestState::Reserved | RequestState::OutcomeUnknown, None) => {}
            _ => {
                return Err(CuError::new(
                    "request_store_corrupt",
                    "request state and outcome metadata disagree",
                ));
            }
        }
    }
    Ok(())
}

fn validate_request_id(value: &str) -> Result<(), CuError> {
    if value.is_empty()
        || value.len() > MAX_REQUEST_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(CuError::new(
            "request_id_invalid",
            "request id must be 1..=128 ASCII token bytes",
        ));
    }
    Ok(())
}

fn validate_completion_token(value: &str) -> Result<(), CuError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CuError::new(
            "request_completion_token_invalid",
            "completion token is not a valid opaque token",
        ));
    }
    Ok(())
}

fn validate_digest(value: &str, code: &'static str) -> Result<(), CuError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CuError::new(
            code,
            "value is not a lowercase SHA-256 digest",
        ));
    }
    Ok(())
}

fn validate_outcome(outcome: &FinalOutcome) -> Result<(), CuError> {
    if outcome.code.is_empty()
        || outcome.code.len() > MAX_OUTCOME_CODE_BYTES
        || !outcome.code.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b':')
        })
    {
        return Err(CuError::new(
            "request_outcome_invalid",
            "outcome code must be a bounded lowercase machine token",
        ));
    }
    if let Some(digest) = &outcome.receipt_sha256 {
        validate_digest(digest, "request_outcome_invalid")?;
    }
    if let Some(replay) = &outcome.replay {
        if outcome.kind != FinalOutcomeKind::Succeeded {
            return Err(CuError::new(
                "request_outcome_invalid",
                "failed outcomes cannot carry successful replay data",
            ));
        }
        match replay {
            FinalReplay::JobSpawn { job_id, generation } => {
                if *generation == 0 || !is_lowercase_uuid_v4(job_id) {
                    return Err(CuError::new(
                        "request_outcome_invalid",
                        "job replay requires a lowercase UUID v4 and nonzero generation",
                    ));
                }
            }
            FinalReplay::DeviceClaim {
                lease_id,
                generation,
            } => {
                if *generation == 0 || !is_lowercase_uuid_v4(lease_id) {
                    return Err(CuError::new(
                        "request_outcome_invalid",
                        "device replay requires a lowercase UUID v4 and nonzero generation",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn is_lowercase_uuid_v4(value: &str) -> bool {
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

fn validate_now(now_utc_ms: i64) -> Result<(), CuError> {
    if now_utc_ms < 0 {
        return Err(CuError::new(
            "request_store_clock_invalid",
            "request idempotency clock must be non-negative UTC milliseconds",
        ));
    }
    Ok(())
}

fn validate_retention_ttl(ttl_ms: i64) -> Result<(), CuError> {
    if !(MIN_RETENTION_TTL_MS..=MAX_RETENTION_TTL_MS).contains(&ttl_ms) {
        return Err(ttl_invalid(
            "retention TTL must be within 1000..=604800000 milliseconds",
        ));
    }
    Ok(())
}

fn ttl_invalid(message: &'static str) -> CuError {
    CuError::new("request_retention_ttl_invalid", message)
}

fn ensure_fingerprint(record: &RequestRecord, fingerprint: &str) -> Result<(), CuError> {
    if constant_time_equal(record.fingerprint_sha256.as_bytes(), fingerprint.as_bytes()) {
        Ok(())
    } else {
        Err(CuError::new(
            "request_id_conflict",
            "request id is already bound to a different canonical fingerprint",
        ))
    }
}

fn ensure_completion_token(record: &RequestRecord, token: &str) -> Result<(), CuError> {
    let digest = sha256_hex(token.as_bytes());
    if constant_time_equal(record.completion_token_sha256.as_bytes(), digest.as_bytes()) {
        Ok(())
    } else {
        Err(CuError::new(
            "request_completion_token_invalid",
            "completion token does not own this reservation",
        ))
    }
}

fn public_status(record: &RequestRecord) -> RequestStatus {
    RequestStatus {
        request_id: record.request_id.clone(),
        fingerprint_sha256: record.fingerprint_sha256.clone(),
        state: record.state,
        created_at_utc_ms: record.created_at_utc_ms,
        updated_at_utc_ms: record.updated_at_utc_ms,
        expires_at_utc_ms: record.expires_at_utc_ms,
        outcome: record.outcome.clone(),
    }
}

fn random_token() -> Result<String, CuError> {
    let bytes = secure_random_array::<32>().map_err(|_| {
        CuError::new(
            "request_store_entropy_unavailable",
            "OS CSPRNG is unavailable",
        )
    })?;
    Ok(hex_digest(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(&Sha256::digest(bytes))
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
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

fn explicit_parent(path: &Path) -> Result<&Path, CuError> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| CuError::new("request_store_path_invalid", "state path needs a parent"))
}

fn unavailable() -> CuError {
    CuError::new(
        "request_store_unavailable",
        "request idempotency state is unavailable",
    )
}

fn as_corrupt(_: CuError) -> CuError {
    CuError::new(
        "request_store_corrupt",
        "request idempotency state violates its schema or bounds",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPath(PathBuf);

    impl TestPath {
        fn new(name: &str) -> Self {
            let root = std::fs::canonicalize(std::env::temp_dir())
                .expect("resolve test temporary root")
                .join(format!(
                    "agenterm-cu-idempotency-{name}-{}",
                    std::process::id()
                ));
            let _ = fs::remove_dir_all(&root);
            Self(root.join("requests.json"))
        }
    }

    impl Drop for TestPath {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().expect("test path parent"));
        }
    }

    fn open(name: &str) -> (TestPath, IdempotencyStore) {
        let path = TestPath::new(name);
        let store = IdempotencyStore::open_at(&path.0).expect("open store");
        (path, store)
    }

    fn fingerprint(value: &str) -> String {
        fingerprint_canonical_request(value.as_bytes()).expect("fingerprint")
    }

    fn succeeded() -> FinalOutcome {
        FinalOutcome::new(FinalOutcomeKind::Succeeded, "completed", None).expect("outcome")
    }

    fn job_replay() -> FinalReplay {
        FinalReplay::JobSpawn {
            job_id: "00000000-0000-4000-8000-000000000001".into(),
            generation: 1,
        }
    }

    #[test]
    fn finalized_request_replays_across_reopen_without_persisting_secrets() {
        let (path, store) = open("final-reopen");
        let canonical = "verb=invoke;target=stable-node;secret=<excluded>";
        let digest = fingerprint(canonical);
        let fresh = match store.reserve("req-1", &digest, 60_000, 1_000).unwrap() {
            ReserveDecision::Fresh(fresh) => fresh,
            other => panic!("expected fresh reservation, got {other:?}"),
        };
        let finalized = store
            .finalize(
                "req-1",
                &digest,
                &fresh.completion_token,
                succeeded(),
                1_001,
            )
            .unwrap();
        assert_eq!(finalized.state, RequestState::Finalized);

        let reopened = IdempotencyStore::open_at(&path.0).unwrap();
        let replay = reopened.reserve("req-1", &digest, 60_000, 1_002).unwrap();
        assert_eq!(replay, ReserveDecision::ReplayFinalized(finalized));

        let persisted = fs::read_to_string(&path.0).unwrap();
        assert!(!persisted.contains(canonical));
        assert!(!persisted.contains(&fresh.completion_token));
        assert!(persisted.contains("completion_token_sha256"));
    }

    #[test]
    fn job_replay_round_trips_and_schema_one_upgrades_on_mutation() {
        let (path, store) = open("job-replay-upgrade");
        let digest = fingerprint("job=<command-sentinel>;lease=<lease-sentinel>");
        let fresh = match store.reserve("req-job", &digest, 60_000, 1_000).unwrap() {
            ReserveDecision::Fresh(fresh) => fresh,
            other => panic!("expected fresh reservation, got {other:?}"),
        };
        let outcome = succeeded().with_replay(job_replay()).unwrap();
        store
            .finalize(
                "req-job",
                &digest,
                &fresh.completion_token,
                outcome.clone(),
                1_001,
            )
            .unwrap();
        let replay = store.reserve("req-job", &digest, 60_000, 1_002).unwrap();
        assert!(matches!(
            replay,
            ReserveDecision::ReplayFinalized(RequestStatus {
                outcome: Some(ref actual),
                ..
            }) if actual == &outcome
        ));

        let schema_two = fs::read_to_string(&path.0).unwrap();
        assert!(!schema_two.contains("command-sentinel"));
        assert!(!schema_two.contains("lease-sentinel"));
        assert!(!schema_two.contains(&fresh.completion_token));
        let schema_one = schema_two.replacen("\"schema_version\":2", "\"schema_version\":1", 1);
        // Schema one predates replay projections, so use a valid old outcome.
        let schema_one = schema_one.replace(
            ",\"replay\":{\"kind\":\"job_spawn\",\"job_id\":\"00000000-0000-4000-8000-000000000001\",\"generation\":1}",
            "",
        );
        fs::write(&path.0, schema_one).unwrap();
        let reopened = IdempotencyStore::open_at(&path.0).expect("read schema one");
        let other = fingerprint("other");
        assert!(matches!(
            reopened
                .reserve("req-other", &other, 60_000, 1_003)
                .unwrap(),
            ReserveDecision::Fresh(_)
        ));
        let upgraded = fs::read_to_string(&path.0).unwrap();
        assert!(upgraded.contains("\"schema_version\":2"));
    }

    #[test]
    fn replay_metadata_rejects_failed_or_invalid_job_identity() {
        assert_eq!(
            FinalOutcome::new(FinalOutcomeKind::Failed, "failed", None)
                .unwrap()
                .with_replay(job_replay())
                .unwrap_err()
                .code,
            "request_outcome_invalid"
        );
        for replay in [
            FinalReplay::JobSpawn {
                job_id: "not-a-job".into(),
                generation: 1,
            },
            FinalReplay::JobSpawn {
                job_id: "00000000-0000-4000-8000-000000000001".into(),
                generation: 0,
            },
        ] {
            assert_eq!(
                succeeded().with_replay(replay).unwrap_err().code,
                "request_outcome_invalid"
            );
        }
    }

    #[test]
    fn reserved_retry_is_uncertain_and_different_fingerprint_conflicts() {
        let (_path, store) = open("reserved-conflict");
        let first = fingerprint("verb=close;window=7");
        let different = fingerprint("verb=close;window=8");
        let fresh = store.reserve("req-close", &first, 60_000, 10).unwrap();
        assert!(matches!(fresh, ReserveDecision::Fresh(_)));

        let retry = store.reserve("req-close", &first, 60_000, 11).unwrap();
        assert!(matches!(
            retry,
            ReserveDecision::Uncertain(RequestStatus {
                state: RequestState::Reserved,
                ..
            })
        ));
        assert_eq!(
            store
                .reserve("req-close", &different, 60_000, 12)
                .unwrap_err()
                .code,
            "request_id_conflict"
        );
    }

    #[test]
    fn explicit_unknown_survives_reopen_and_can_later_be_finalized() {
        let (path, store) = open("unknown");
        let digest = fingerprint("verb=send-keys;window=3;keys=escape");
        let fresh = match store.reserve("req-unknown", &digest, 60_000, 20).unwrap() {
            ReserveDecision::Fresh(fresh) => fresh,
            _ => panic!("fresh reservation expected"),
        };
        let unknown = store
            .mark_outcome_unknown("req-unknown", &digest, &fresh.completion_token, 21)
            .unwrap();
        assert_eq!(unknown.state, RequestState::OutcomeUnknown);

        let reopened = IdempotencyStore::open_at(&path.0).unwrap();
        assert!(matches!(
            reopened
                .reserve("req-unknown", &digest, 60_000, 22)
                .unwrap(),
            ReserveDecision::Uncertain(RequestStatus {
                state: RequestState::OutcomeUnknown,
                ..
            })
        ));
        assert_eq!(
            reopened
                .finalize(
                    "req-unknown",
                    &digest,
                    &fresh.completion_token,
                    succeeded(),
                    23,
                )
                .unwrap()
                .state,
            RequestState::Finalized
        );
    }

    #[test]
    fn expiry_reclaims_capacity_but_clock_rollback_fails_closed() {
        let (_path, store) = open("ttl-clock");
        let digest = fingerprint("verb=focus;window=1");
        let first = match store.reserve("req-ttl", &digest, 1_000, 100).unwrap() {
            ReserveDecision::Fresh(fresh) => fresh,
            _ => panic!("fresh reservation expected"),
        };
        assert!(matches!(
            store.reserve("req-ttl", &digest, 1_000, 1_099).unwrap(),
            ReserveDecision::Uncertain(_)
        ));
        let second = match store.reserve("req-ttl", &digest, 1_000, 1_100).unwrap() {
            ReserveDecision::Fresh(fresh) => fresh,
            _ => panic!("expired identity should be reclaimable"),
        };
        assert_ne!(first.completion_token, second.completion_token);
        assert_eq!(
            store
                .reserve("req-other", &digest, 1_000, 1_099)
                .unwrap_err()
                .code,
            "request_store_clock_rollback"
        );
    }

    #[test]
    fn malformed_state_and_stable_sidecar_contention_are_typed() {
        let (path, store) = open("corrupt-lock");
        fs::write(&path.0, b"{").unwrap();
        assert_eq!(
            IdempotencyStore::open_at(&path.0).unwrap_err().code,
            "request_store_corrupt"
        );
        fs::remove_file(&path.0).unwrap();

        let lock_path = store.lock_path().unwrap();
        let held = PathLock::try_acquire(&lock_path).unwrap();
        let digest = fingerprint("verb=activate;window=2");
        assert_eq!(
            store
                .reserve("req-lock", &digest, 1_000, 1)
                .unwrap_err()
                .code,
            "request_store_lock_contended"
        );
        drop(held);
        assert!(lock_path.exists());
    }

    #[test]
    fn input_and_outcome_metadata_are_closed_and_bounded() {
        assert_eq!(
            fingerprint_canonical_request(&vec![0; MAX_CANONICAL_BYTES + 1])
                .unwrap_err()
                .code,
            "request_fingerprint_input_limit"
        );
        assert_eq!(
            FinalOutcome::new(FinalOutcomeKind::Failed, "Human message!", None)
                .unwrap_err()
                .code,
            "request_outcome_invalid"
        );
        let command = b"send-text:low-entropy-value";
        let first = fingerprint_canonical_request_with_secret(command, &[7; 32]).unwrap();
        let second = fingerprint_canonical_request_with_secret(command, &[8; 32]).unwrap();
        assert_ne!(first, second, "the session secret must bind the digest");
        assert_ne!(first, fingerprint_canonical_request(command).unwrap());
        assert_eq!(
            fingerprint_canonical_request_with_secret(command, b"short")
                .unwrap_err()
                .code,
            "request_fingerprint_secret_invalid"
        );
        let (_path, store) = open("inputs");
        let digest = fingerprint("verb=click;node=button-1");
        assert_eq!(
            store
                .reserve("contains whitespace", &digest, 1_000, 0)
                .unwrap_err()
                .code,
            "request_id_invalid"
        );
        assert_eq!(
            store.reserve("req", &digest, 999, 0).unwrap_err().code,
            "request_retention_ttl_invalid"
        );
    }

    #[test]
    fn schema_validation_rejects_record_and_file_growth() {
        let digest = fingerprint("verb=bounded");
        let mut document = Document::default();
        for index in 0..=MAX_RECORDS {
            let request_id = format!("req-{index}");
            document.records.insert(
                request_id.clone(),
                RequestRecord {
                    request_id,
                    fingerprint_sha256: digest.clone(),
                    completion_token_sha256: digest.clone(),
                    state: RequestState::Reserved,
                    created_at_utc_ms: 1,
                    updated_at_utc_ms: 1,
                    expires_at_utc_ms: 2,
                    outcome: None,
                },
            );
        }
        assert_eq!(
            validate_document(&document).unwrap_err().code,
            "request_store_corrupt"
        );

        let (path, _store) = open("file-bound");
        fs::write(&path.0, vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
        assert_eq!(
            IdempotencyStore::open_at(&path.0).unwrap_err().code,
            "request_store_corrupt"
        );
    }
}
