//! Crash-safe public metadata for resident native-device owners.
//!
//! The durable document deliberately contains neither an openable device
//! locator nor plaintext lease authority. A resident owner receives both via
//! its bounded stdin launch document, holds the one native handle, and
//! publishes only identity/state/counter summaries here.

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
const MAX_RECORDS: usize = 1024;
const MAX_TOKEN_BYTES: usize = 512;
const MAX_CODE_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct DeviceLeaseRefreshBlockers {
    pub blocking: usize,
    pub claim_intent: usize,
    pub opening: usize,
    pub active: usize,
    pub owner_lost: usize,
    pub cleanup_uncertain: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceLeaseHandle {
    pub lease_id: String,
    pub generation: u64,
    pub owner_nonce: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceOwnerIdentity {
    pub pid: u32,
    pub start_identity: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceSerialRecord {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: u8,
    pub flow: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum DeviceLeaseState {
    ClaimIntent,
    Opening,
    Active,
    Released,
    Expired,
    OpenFailed { code: String },
    OwnerLost,
    CleanupUncertain { code: String },
}

impl DeviceLeaseState {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Released | Self::Expired | Self::OpenFailed { .. }
        )
    }

    fn blocks_refresh(&self) -> bool {
        matches!(
            self,
            Self::ClaimIntent
                | Self::Opening
                | Self::Active
                | Self::OwnerLost
                | Self::CleanupUncertain { .. }
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceLeaseRecord {
    pub lease_id: String,
    pub generation: u64,
    pub owner_nonce: String,
    pub session_id: String,
    pub runtime_lock_id: String,
    pub device_id: String,
    /// SHA-256 hex only. The plaintext lease is never durable.
    lease_sha256: String,
    pub owner: Option<DeviceOwnerIdentity>,
    pub state: DeviceLeaseState,
    pub exclusive: Option<String>,
    pub serial: Option<DeviceSerialRecord>,
    pub created_at_utc_ms: i64,
    pub updated_at_utc_ms: i64,
    pub expires_at_utc_ms: i64,
    pub terminal_at_utc_ms: Option<i64>,
    pub bytes_read: u64,
    pub bytes_written: u64,
}

impl DeviceLeaseRecord {
    pub(crate) fn handle(&self) -> DeviceLeaseHandle {
        DeviceLeaseHandle {
            lease_id: self.lease_id.clone(),
            generation: self.generation,
            owner_nonce: self.owner_nonce.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct DeviceLeaseStore {
    path: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    last_now_utc_ms: i64,
    leases: BTreeMap<String, DeviceLeaseRecord>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            last_now_utc_ms: 0,
            leases: BTreeMap::new(),
        }
    }
}

impl DeviceLeaseStore {
    pub(crate) fn open() -> Result<Self, CuError> {
        let path = if let Some(path) = std::env::var_os("AGENTERM_CU_DEVICE_LEASE_PATH") {
            PathBuf::from(path)
        } else {
            host_directories()
                .map_err(|_| unavailable())?
                .local_data
                .join("agenterm")
                .join("cu-device-leases.json")
        };
        Self::open_creating_parent(path)
    }

    pub(crate) fn open_at(path: impl Into<PathBuf>) -> Result<Self, CuError> {
        let path = absolutize(path.into())?;
        let parent = explicit_parent(&path)?;
        let metadata = fs::symlink_metadata(parent).map_err(|_| unavailable())?;
        if metadata_is_link_like(&metadata) || !metadata.is_dir() {
            return Err(corrupt("device lease parent must be a direct directory"));
        }
        protect_private_directory(parent).map_err(|_| unavailable())?;
        let store = Self { path };
        let _ = store.read_document()?;
        Ok(store)
    }

    fn open_creating_parent(path: PathBuf) -> Result<Self, CuError> {
        let path = absolutize(path)?;
        let parent = explicit_parent(&path)?;
        fs::create_dir_all(parent).map_err(|_| unavailable())?;
        protect_private_directory(parent).map_err(|_| unavailable())?;
        Self::open_at(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn reserve_claim(
        &self,
        session_id: &str,
        runtime_lock_id: &str,
        device_id: &str,
        lease_sha256: &str,
        expires_at_utc_ms: i64,
        now_utc_ms: i64,
    ) -> Result<DeviceLeaseRecord, CuError> {
        validate_token(session_id, "device_lease_session_invalid")?;
        validate_token(runtime_lock_id, "device_runtime_lock_invalid")?;
        validate_device_id(device_id)?;
        validate_digest(lease_sha256)?;
        validate_live_expiry(now_utc_ms, expires_at_utc_ms)?;
        let lease_id = random_uuid_v4()?;
        let owner_nonce = random_hex::<16>()?;
        self.mutate(now_utc_ms, move |document| {
            evict_terminal_until_room(document);
            if document.leases.len() >= MAX_RECORDS {
                return Err(CuError::new(
                    "device_lease_limit",
                    "device lease registry reached its 1024-record ceiling",
                ));
            }
            if document
                .leases
                .values()
                .any(|record| record.device_id == device_id && !record.state.is_terminal())
            {
                return Err(CuError::new(
                    "device_exclusive_busy",
                    "device already has a nonterminal claim",
                ));
            }
            let record = DeviceLeaseRecord {
                lease_id: lease_id.clone(),
                generation: 1,
                owner_nonce,
                session_id: session_id.to_owned(),
                runtime_lock_id: runtime_lock_id.to_owned(),
                device_id: device_id.to_owned(),
                lease_sha256: lease_sha256.to_owned(),
                owner: None,
                state: DeviceLeaseState::ClaimIntent,
                exclusive: None,
                serial: None,
                created_at_utc_ms: now_utc_ms,
                updated_at_utc_ms: now_utc_ms,
                expires_at_utc_ms,
                terminal_at_utc_ms: None,
                bytes_read: 0,
                bytes_written: 0,
            };
            document.leases.insert(lease_id, record.clone());
            Ok(record)
        })
    }

    pub(crate) fn claim_opening(
        &self,
        handle: &DeviceLeaseHandle,
        owner: DeviceOwnerIdentity,
        now_utc_ms: i64,
    ) -> Result<DeviceLeaseRecord, CuError> {
        validate_owner(&owner)?;
        self.transition(handle, now_utc_ms, |record| {
            if record.state != DeviceLeaseState::ClaimIntent {
                return Err(transition("only claim_intent may become opening"));
            }
            record.owner = Some(owner);
            record.state = DeviceLeaseState::Opening;
            Ok(())
        })
    }

    pub(crate) fn mark_unclaimed_open_failed(
        &self,
        handle: &DeviceLeaseHandle,
        code: &str,
        now_utc_ms: i64,
    ) -> Result<DeviceLeaseRecord, CuError> {
        validate_code(code)?;
        self.transition(handle, now_utc_ms, |record| {
            if record.state != DeviceLeaseState::ClaimIntent || record.owner.is_some() {
                return Err(transition(
                    "only unclaimed intent may fail before owner bind",
                ));
            }
            record.state = DeviceLeaseState::OpenFailed {
                code: code.to_owned(),
            };
            record.terminal_at_utc_ms = Some(now_utc_ms);
            Ok(())
        })
    }

    pub(crate) fn mark_active(
        &self,
        handle: &DeviceLeaseHandle,
        owner: &DeviceOwnerIdentity,
        exclusive: &str,
        serial: Option<DeviceSerialRecord>,
        now_utc_ms: i64,
    ) -> Result<DeviceLeaseRecord, CuError> {
        if exclusive != "kernel" {
            return Err(CuError::new(
                "device_exclusive_unsupported",
                "active device claims require kernel-enforced exclusivity",
            ));
        }
        if let Some(serial) = serial.as_ref() {
            validate_serial(serial)?;
        }
        self.transition(handle, now_utc_ms, move |record| {
            require_owner(record, owner)?;
            if record.state != DeviceLeaseState::Opening {
                return Err(transition("only opening may become active"));
            }
            record.state = DeviceLeaseState::Active;
            record.exclusive = Some(exclusive.to_owned());
            record.serial = serial;
            Ok(())
        })
    }

    pub(crate) fn mark_open_failed(
        &self,
        handle: &DeviceLeaseHandle,
        owner: &DeviceOwnerIdentity,
        code: &str,
        now_utc_ms: i64,
    ) -> Result<DeviceLeaseRecord, CuError> {
        validate_code(code)?;
        self.transition(handle, now_utc_ms, |record| {
            require_owner(record, owner)?;
            if record.state != DeviceLeaseState::Opening {
                return Err(transition("only opening may fail"));
            }
            record.state = DeviceLeaseState::OpenFailed {
                code: code.to_owned(),
            };
            record.terminal_at_utc_ms = Some(now_utc_ms);
            Ok(())
        })
    }

    pub(crate) fn publish_counters(
        &self,
        handle: &DeviceLeaseHandle,
        owner: &DeviceOwnerIdentity,
        bytes_read: u64,
        bytes_written: u64,
        expires_at_utc_ms: i64,
        now_utc_ms: i64,
    ) -> Result<DeviceLeaseRecord, CuError> {
        validate_live_expiry(now_utc_ms, expires_at_utc_ms)?;
        self.transition(handle, now_utc_ms, |record| {
            require_owner(record, owner)?;
            if record.state != DeviceLeaseState::Active {
                return Err(transition("only an active lease may publish owner state"));
            }
            if bytes_read < record.bytes_read || bytes_written < record.bytes_written {
                return Err(transition("device byte counters may not move backward"));
            }
            record.bytes_read = bytes_read;
            record.bytes_written = bytes_written;
            record.expires_at_utc_ms = expires_at_utc_ms;
            Ok(())
        })
    }

    pub(crate) fn mark_terminal(
        &self,
        handle: &DeviceLeaseHandle,
        owner: &DeviceOwnerIdentity,
        state: DeviceLeaseState,
        bytes_read: u64,
        bytes_written: u64,
        now_utc_ms: i64,
    ) -> Result<DeviceLeaseRecord, CuError> {
        if !matches!(
            state,
            DeviceLeaseState::Released
                | DeviceLeaseState::Expired
                | DeviceLeaseState::OwnerLost
                | DeviceLeaseState::CleanupUncertain { .. }
        ) {
            return Err(transition("requested device terminal state is invalid"));
        }
        if let DeviceLeaseState::CleanupUncertain { code } = &state {
            validate_code(code)?;
        }
        self.transition(handle, now_utc_ms, move |record| {
            require_owner(record, owner)?;
            if record.state != DeviceLeaseState::Active {
                return Err(transition("only an active lease may become terminal"));
            }
            if bytes_read < record.bytes_read || bytes_written < record.bytes_written {
                return Err(transition("device byte counters may not move backward"));
            }
            record.bytes_read = bytes_read;
            record.bytes_written = bytes_written;
            record.state = state;
            record.terminal_at_utc_ms = Some(now_utc_ms);
            Ok(())
        })
    }

    pub(crate) fn mark_owner_lost(
        &self,
        handle: &DeviceLeaseHandle,
        owner: &DeviceOwnerIdentity,
        now_utc_ms: i64,
    ) -> Result<DeviceLeaseRecord, CuError> {
        self.transition(handle, now_utc_ms, |record| {
            require_owner(record, owner)?;
            if !matches!(
                record.state,
                DeviceLeaseState::Opening | DeviceLeaseState::Active
            ) {
                return Err(transition("only opening or active may become owner_lost"));
            }
            record.state = DeviceLeaseState::OwnerLost;
            record.terminal_at_utc_ms = Some(now_utc_ms);
            Ok(())
        })
    }

    pub(crate) fn get(&self, lease_id: &str) -> Result<Option<DeviceLeaseRecord>, CuError> {
        validate_uuid_v4(lease_id)?;
        Ok(self
            .read_document()?
            .unwrap_or_default()
            .leases
            .remove(lease_id))
    }

    pub(crate) fn list(&self) -> Result<Vec<DeviceLeaseRecord>, CuError> {
        Ok(self
            .read_document()?
            .unwrap_or_default()
            .leases
            .into_values()
            .collect())
    }

    pub(crate) fn refresh_blockers(&self) -> Result<DeviceLeaseRefreshBlockers, CuError> {
        Ok(blocker_summary(
            self.read_document()?.unwrap_or_default().leases.values(),
        ))
    }

    pub(crate) fn refresh_blockers_read_only() -> Result<DeviceLeaseRefreshBlockers, CuError> {
        let path = if let Some(path) = std::env::var_os("AGENTERM_CU_DEVICE_LEASE_PATH") {
            PathBuf::from(path)
        } else {
            host_directories()
                .map_err(|_| unavailable())?
                .local_data
                .join("agenterm")
                .join("cu-device-leases.json")
        };
        let path = absolutize(path)?;
        let parent = explicit_parent(&path)?;
        let metadata = fs::symlink_metadata(parent).map_err(|_| unavailable())?;
        if metadata_is_link_like(&metadata) || !metadata.is_dir() {
            return Err(corrupt("device lease parent must be a direct directory"));
        }
        let store = Self { path };
        store.refresh_blockers()
    }

    fn transition(
        &self,
        handle: &DeviceLeaseHandle,
        now_utc_ms: i64,
        edit: impl FnOnce(&mut DeviceLeaseRecord) -> Result<(), CuError>,
    ) -> Result<DeviceLeaseRecord, CuError> {
        validate_handle(handle)?;
        self.mutate(now_utc_ms, |document| {
            let record = checked_record_mut(document, handle)?;
            edit(record)?;
            record.updated_at_utc_ms = now_utc_ms;
            Ok(record.clone())
        })
    }

    fn mutate<T>(
        &self,
        now_utc_ms: i64,
        edit: impl FnOnce(&mut Document) -> Result<T, CuError>,
    ) -> Result<T, CuError> {
        validate_time(now_utc_ms)?;
        let _lock = PathLock::try_acquire(&self.lock_path()?).map_err(|error| {
            let code = if error.kind() == LockErrorKind::Contended {
                "device_lease_store_contended"
            } else {
                "device_lease_store_unavailable"
            };
            CuError::new(code, "device lease state lock is unavailable")
        })?;
        let mut document = self.read_document()?.unwrap_or_default();
        if now_utc_ms < document.last_now_utc_ms {
            return Err(CuError::new(
                "device_lease_clock_rollback",
                "device lease clock moved backward from persisted state",
            ));
        }
        let result = edit(&mut document)?;
        document.last_now_utc_ms = now_utc_ms;
        validate_document(&document)?;
        self.write_document(&document)?;
        Ok(result)
    }

    fn lock_path(&self) -> Result<PathBuf, CuError> {
        let file_name = self.path.file_name().ok_or_else(unavailable)?;
        let mut lock = file_name.to_os_string();
        lock.push(".lock");
        Ok(explicit_parent(&self.path)?.join(lock))
    }

    fn read_document(&self) -> Result<Option<Document>, CuError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable()),
        };
        if metadata_is_link_like(&metadata) || !metadata.is_file() {
            return Err(corrupt(
                "device lease state must be one regular, non-link file",
            ));
        }
        if metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(corrupt("device lease state exceeds its byte ceiling"));
        }
        let bytes = fs::read(&self.path).map_err(|_| unavailable())?;
        let document = serde_json::from_slice(&bytes)
            .map_err(|_| corrupt("device lease state is not a valid schema document"))?;
        validate_document(&document)?;
        Ok(Some(document))
    }

    fn write_document(&self, document: &Document) -> Result<(), CuError> {
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && (metadata_is_link_like(&metadata) || !metadata.is_file())
        {
            return Err(corrupt(
                "device lease state target became a link-like or non-file entry",
            ));
        }
        let bytes = serde_json::to_vec(document).map_err(|_| {
            CuError::new(
                "device_lease_store_serialization",
                "device lease state could not be serialized",
            )
        })?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(CuError::new(
                "device_lease_store_limit",
                "device lease state exceeds its byte ceiling",
            ));
        }
        write_private_atomic(&self.path, &bytes).map_err(|_| {
            CuError::new(
                "device_lease_store_publish",
                "device lease state could not be atomically published",
            )
        })
    }
}

fn blocker_summary<'a>(
    records: impl Iterator<Item = &'a DeviceLeaseRecord>,
) -> DeviceLeaseRefreshBlockers {
    let mut result = DeviceLeaseRefreshBlockers::default();
    for record in records.filter(|record| record.state.blocks_refresh()) {
        result.blocking += 1;
        match record.state {
            DeviceLeaseState::ClaimIntent => result.claim_intent += 1,
            DeviceLeaseState::Opening => result.opening += 1,
            DeviceLeaseState::Active => result.active += 1,
            DeviceLeaseState::OwnerLost => result.owner_lost += 1,
            DeviceLeaseState::CleanupUncertain { .. } => result.cleanup_uncertain += 1,
            DeviceLeaseState::Released
            | DeviceLeaseState::Expired
            | DeviceLeaseState::OpenFailed { .. } => unreachable!(),
        }
    }
    result
}

fn checked_record_mut<'a>(
    document: &'a mut Document,
    handle: &DeviceLeaseHandle,
) -> Result<&'a mut DeviceLeaseRecord, CuError> {
    let record = document.leases.get_mut(&handle.lease_id).ok_or_else(|| {
        CuError::new(
            "device_lease_not_found",
            "device lease record does not exist",
        )
    })?;
    if record.generation != handle.generation || record.owner_nonce != handle.owner_nonce {
        return Err(CuError::new(
            "device_lease_identity_changed",
            "device lease generation or owner nonce no longer matches",
        ));
    }
    Ok(record)
}

fn require_owner(record: &DeviceLeaseRecord, owner: &DeviceOwnerIdentity) -> Result<(), CuError> {
    if record.owner.as_ref() == Some(owner) {
        Ok(())
    } else {
        Err(CuError::new(
            "device_owner_changed",
            "resident device owner identity no longer matches",
        ))
    }
}

fn evict_terminal_until_room(document: &mut Document) {
    while document.leases.len() >= MAX_RECORDS {
        let oldest = document
            .leases
            .values()
            .filter(|record| record.state.is_terminal())
            .min_by_key(|record| (record.updated_at_utc_ms, record.lease_id.clone()))
            .map(|record| record.lease_id.clone());
        let Some(oldest) = oldest else { break };
        document.leases.remove(&oldest);
    }
}

fn validate_document(document: &Document) -> Result<(), CuError> {
    if document.schema_version != SCHEMA_VERSION || document.last_now_utc_ms < 0 {
        return Err(corrupt("device lease schema or clock is invalid"));
    }
    if document.leases.len() > MAX_RECORDS {
        return Err(corrupt("device lease state exceeds its record ceiling"));
    }
    for (key, record) in &document.leases {
        validate_record(key, record)?;
        if record.updated_at_utc_ms > document.last_now_utc_ms {
            return Err(corrupt("device lease record exceeds the clock watermark"));
        }
    }
    Ok(())
}

fn validate_record(key: &str, record: &DeviceLeaseRecord) -> Result<(), CuError> {
    validate_handle(&record.handle()).map_err(|_| corrupt("device lease identity is invalid"))?;
    validate_token(&record.session_id, "device_lease_session_invalid")
        .map_err(|_| corrupt("device lease session is invalid"))?;
    validate_token(&record.runtime_lock_id, "device_runtime_lock_invalid")
        .map_err(|_| corrupt("device runtime lock is invalid"))?;
    validate_device_id(&record.device_id)
        .map_err(|_| corrupt("device public identity is invalid"))?;
    validate_digest(&record.lease_sha256).map_err(|_| corrupt("device lease digest is invalid"))?;
    if key != record.lease_id
        || record.created_at_utc_ms < 0
        || record.updated_at_utc_ms < record.created_at_utc_ms
        || record.expires_at_utc_ms < record.created_at_utc_ms
    {
        return Err(corrupt("device lease record timestamps or key are invalid"));
    }
    if let Some(owner) = record.owner.as_ref() {
        validate_owner(owner).map_err(|_| corrupt("device owner identity is invalid"))?;
    }
    if let Some(serial) = record.serial.as_ref() {
        validate_serial(serial).map_err(|_| corrupt("device serial readback is invalid"))?;
    }
    let shape_valid = match &record.state {
        DeviceLeaseState::ClaimIntent => {
            record.owner.is_none()
                && record.exclusive.is_none()
                && record.serial.is_none()
                && record.terminal_at_utc_ms.is_none()
        }
        DeviceLeaseState::Opening => {
            record.owner.is_some()
                && record.exclusive.is_none()
                && record.serial.is_none()
                && record.terminal_at_utc_ms.is_none()
        }
        DeviceLeaseState::Active => {
            record.owner.is_some()
                && record.exclusive.as_deref() == Some("kernel")
                && record.terminal_at_utc_ms.is_none()
        }
        DeviceLeaseState::OpenFailed { code } => {
            validate_code(code).is_ok()
                && record.exclusive.is_none()
                && record.terminal_at_utc_ms == Some(record.updated_at_utc_ms)
        }
        DeviceLeaseState::Released | DeviceLeaseState::Expired => {
            record.owner.is_some()
                && record.exclusive.as_deref() == Some("kernel")
                && record.terminal_at_utc_ms == Some(record.updated_at_utc_ms)
        }
        DeviceLeaseState::OwnerLost => {
            record.owner.is_some()
                && matches!(record.exclusive.as_deref(), None | Some("kernel"))
                && record.terminal_at_utc_ms == Some(record.updated_at_utc_ms)
        }
        DeviceLeaseState::CleanupUncertain { code } => {
            validate_code(code).is_ok()
                && record.owner.is_some()
                && record.exclusive.as_deref() == Some("kernel")
                && record.terminal_at_utc_ms == Some(record.updated_at_utc_ms)
        }
    };
    if !shape_valid {
        return Err(corrupt("device lease transition shape is invalid"));
    }
    Ok(())
}

fn validate_handle(handle: &DeviceLeaseHandle) -> Result<(), CuError> {
    validate_uuid_v4(&handle.lease_id)?;
    if handle.generation == 0 {
        return Err(CuError::new(
            "device_lease_generation_invalid",
            "device lease generation must be nonzero",
        ));
    }
    if handle.owner_nonce.len() != 32 || !is_lower_hex(&handle.owner_nonce) {
        return Err(CuError::new(
            "device_owner_nonce_invalid",
            "device owner nonce must be 32 lowercase hexadecimal bytes",
        ));
    }
    Ok(())
}

fn validate_device_id(value: &str) -> Result<(), CuError> {
    if value.len() == 78 && value.starts_with("agt-device-v1-") && is_lower_hex(&value[14..]) {
        Ok(())
    } else {
        Err(CuError::new(
            "device_id_invalid",
            "device id must be one installation-scoped agt-device-v1 identifier",
        ))
    }
}

fn validate_digest(value: &str) -> Result<(), CuError> {
    if value.len() == 64 && is_lower_hex(value) {
        Ok(())
    } else {
        Err(CuError::new(
            "device_lease_digest_invalid",
            "device lease digest must be 64 lowercase hexadecimal bytes",
        ))
    }
}

fn validate_owner(owner: &DeviceOwnerIdentity) -> Result<(), CuError> {
    if owner.pid == 0 {
        return Err(CuError::new(
            "device_owner_identity_invalid",
            "resident device owner PID must be nonzero",
        ));
    }
    validate_token(&owner.start_identity, "device_owner_identity_invalid")
}

fn validate_serial(serial: &DeviceSerialRecord) -> Result<(), CuError> {
    if serial.baud == 0
        || !(5..=8).contains(&serial.data_bits)
        || !matches!(serial.parity.as_str(), "none" | "even" | "odd")
        || !matches!(serial.stop_bits, 1 | 2)
        || !matches!(serial.flow.as_str(), "none" | "software" | "hardware")
    {
        Err(CuError::new(
            "device_serial_invalid",
            "device serial readback is outside the closed serial contract",
        ))
    } else {
        Ok(())
    }
}

fn validate_live_expiry(now_utc_ms: i64, expires_at_utc_ms: i64) -> Result<(), CuError> {
    validate_time(now_utc_ms)?;
    let remaining = expires_at_utc_ms.checked_sub(now_utc_ms).ok_or_else(|| {
        CuError::new("device_ttl_invalid", "device lease TTL overflows the clock")
    })?;
    if !(1..=86_400_000).contains(&remaining) {
        Err(CuError::new(
            "device_lease_expired",
            "resident device owner may publish only before its monotonic lease deadline",
        ))
    } else {
        Ok(())
    }
}

fn validate_time(value: i64) -> Result<(), CuError> {
    if value < 0 {
        Err(CuError::new(
            "device_lease_clock_invalid",
            "device lease clock must be non-negative",
        ))
    } else {
        Ok(())
    }
}

fn validate_code(value: &str) -> Result<(), CuError> {
    if value.is_empty()
        || value.len() > MAX_CODE_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        Err(CuError::new(
            "device_error_code_invalid",
            "device error code must be a bounded lowercase token",
        ))
    } else {
        Ok(())
    }
}

fn validate_token(value: &str, code: &'static str) -> Result<(), CuError> {
    if value.is_empty() || value.len() > MAX_TOKEN_BYTES || value.chars().any(char::is_control) {
        Err(CuError::new(code, "device lease identity token is invalid"))
    } else {
        Ok(())
    }
}

fn validate_uuid_v4(value: &str) -> Result<(), CuError> {
    let valid = value.len() == 36
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
            "device_lease_id_invalid",
            "device lease id must be a lowercase UUID v4",
        ))
    }
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn absolutize(path: PathBuf) -> Result<PathBuf, CuError> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|_| unavailable())
    }
}

fn explicit_parent(path: &Path) -> Result<&Path, CuError> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| corrupt("device lease state requires an explicit parent"))
}

fn unavailable() -> CuError {
    CuError::new(
        "device_lease_store_unavailable",
        "device lease state or its sidecar lock is unavailable",
    )
}

fn corrupt(message: &'static str) -> CuError {
    CuError::new("device_lease_store_corrupt", message)
}

fn transition(message: &'static str) -> CuError {
    CuError::new("device_lease_transition_invalid", message)
}

fn random_uuid_v4() -> Result<String, CuError> {
    let mut bytes = secure_random_array::<16>().map_err(|_| entropy_error())?;
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
    let bytes = secure_random_array::<N>().map_err(|_| entropy_error())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn entropy_error() -> CuError {
    CuError::new(
        "device_lease_entropy_unavailable",
        "OS CSPRNG is unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct Scratch {
        root: PathBuf,
        path: PathBuf,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "agenterm-device-lease-{label}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let path = root.join("leases.json");
            Self { root, path }
        }

        fn store(&self) -> DeviceLeaseStore {
            DeviceLeaseStore::open_at(&self.path).unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn device_id() -> String {
        format!("agt-device-v1-{}", "a".repeat(64))
    }

    fn digest(secret: &str) -> String {
        Sha256::digest(secret.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn owner() -> DeviceOwnerIdentity {
        DeviceOwnerIdentity {
            pid: 42,
            start_identity: "owner-start-42".to_owned(),
        }
    }

    #[test]
    fn state_machine_reopens_without_plaintext_secret_or_locator() {
        let scratch = Scratch::new("state");
        let store = scratch.store();
        let secret = "lease-plaintext-sentinel";
        let intent = store
            .reserve_claim(
                "session-one",
                "lock-one",
                &device_id(),
                &digest(secret),
                61_000,
                1_000,
            )
            .unwrap();
        let handle = intent.handle();
        let owner = owner();
        store.claim_opening(&handle, owner.clone(), 2_000).unwrap();
        store
            .mark_active(
                &handle,
                &owner,
                "kernel",
                Some(DeviceSerialRecord {
                    baud: 115_200,
                    data_bits: 8,
                    parity: "none".to_owned(),
                    stop_bits: 1,
                    flow: "none".to_owned(),
                }),
                3_000,
            )
            .unwrap();
        store
            .publish_counters(&handle, &owner, 4, 5, 62_000, 4_000)
            .unwrap();
        store
            .mark_terminal(&handle, &owner, DeviceLeaseState::Released, 4, 5, 5_000)
            .unwrap();

        let reopened = scratch.store();
        let record = reopened.get(&intent.lease_id).unwrap().unwrap();
        assert_eq!(record.state, DeviceLeaseState::Released);
        assert_eq!(record.bytes_read, 4);
        assert_eq!(record.bytes_written, 5);
        let disk = fs::read_to_string(&scratch.path).unwrap();
        assert!(!disk.contains(secret));
        assert!(!disk.contains("/dev/"));
        assert!(!disk.contains("COM3"));
        assert!(!disk.contains("payload"));
    }

    #[test]
    fn nonterminal_claim_is_exclusive_and_blocks_refresh() {
        let scratch = Scratch::new("exclusive");
        let store = scratch.store();
        let first = store
            .reserve_claim(
                "session-one",
                "lock-one",
                &device_id(),
                &digest("one"),
                11_000,
                1_000,
            )
            .unwrap();
        assert_eq!(
            store
                .reserve_claim(
                    "session-two",
                    "lock-two",
                    &device_id(),
                    &digest("two"),
                    12_000,
                    2_000
                )
                .unwrap_err()
                .code,
            "device_exclusive_busy"
        );
        assert_eq!(
            store.refresh_blockers().unwrap(),
            DeviceLeaseRefreshBlockers {
                blocking: 1,
                claim_intent: 1,
                ..DeviceLeaseRefreshBlockers::default()
            }
        );
        let owner = owner();
        let handle = first.handle();
        store.claim_opening(&handle, owner.clone(), 3_000).unwrap();
        store
            .mark_active(&handle, &owner, "kernel", None, 4_000)
            .unwrap();
        store
            .mark_terminal(&handle, &owner, DeviceLeaseState::Released, 0, 0, 5_000)
            .unwrap();
        assert_eq!(
            store.refresh_blockers().unwrap(),
            DeviceLeaseRefreshBlockers::default()
        );
        store
            .reserve_claim(
                "session-two",
                "lock-two",
                &device_id(),
                &digest("two"),
                16_000,
                6_000,
            )
            .unwrap();
    }

    #[test]
    fn generation_nonce_owner_and_counter_monotonicity_fail_closed() {
        let scratch = Scratch::new("identity");
        let store = scratch.store();
        let intent = store
            .reserve_claim(
                "session",
                "lock-one",
                &device_id(),
                &digest("lease"),
                20_000,
                1_000,
            )
            .unwrap();
        let handle = intent.handle();
        let owner = owner();
        store.claim_opening(&handle, owner.clone(), 2_000).unwrap();
        store
            .mark_active(&handle, &owner, "kernel", None, 3_000)
            .unwrap();
        store
            .publish_counters(&handle, &owner, 10, 20, 21_000, 4_000)
            .unwrap();
        assert_eq!(
            store
                .publish_counters(&handle, &owner, 9, 20, 22_000, 5_000)
                .unwrap_err()
                .code,
            "device_lease_transition_invalid"
        );
        let mut stale = handle.clone();
        stale.generation += 1;
        assert_eq!(
            store
                .publish_counters(&stale, &owner, 11, 21, 22_000, 5_000)
                .unwrap_err()
                .code,
            "device_lease_identity_changed"
        );
        assert_eq!(
            store
                .publish_counters(
                    &handle,
                    &DeviceOwnerIdentity { pid: 43, ..owner },
                    11,
                    21,
                    22_000,
                    5_000
                )
                .unwrap_err()
                .code,
            "device_owner_changed"
        );
    }

    #[test]
    fn malformed_or_linked_state_is_preserved_and_rejected() {
        let scratch = Scratch::new("corrupt");
        let store = scratch.store();
        fs::write(&scratch.path, br#"{"schema_version":1,"leases":{"#).unwrap();
        assert_eq!(store.list().unwrap_err().code, "device_lease_store_corrupt");

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            fs::remove_file(&scratch.path).unwrap();
            let target = scratch.root.join("target.json");
            let valid_target = br#"{"schema_version":1,"last_now_utc_ms":0,"leases":{}}"#;
            fs::write(&target, valid_target).unwrap();
            symlink(&target, &scratch.path).unwrap();
            assert_eq!(
                store
                    .reserve_claim(
                        "session",
                        "lock-one",
                        &device_id(),
                        &digest("lease"),
                        20_000,
                        1_000
                    )
                    .unwrap_err()
                    .code,
                "device_lease_store_corrupt"
            );
            assert_eq!(fs::read(target).unwrap(), valid_target);
        }
    }
}
