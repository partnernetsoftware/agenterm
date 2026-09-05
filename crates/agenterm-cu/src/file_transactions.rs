//! Crash-recoverable regular-file copy transactions.
//!
//! The receipt is written before each externally visible phase.  Paths alone
//! never confer ownership: every file that recovery may remove or rename is
//! bound to an opened-object identity and a complete content snapshot.

use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use agenterm_platform::{
    entropy::secure_random_array,
    file_identity::file_identity,
    filesystem::{
        host_directories, private_create_new_options, protect_private_directory, sync_parent,
        write_private_atomic,
    },
    filesystem_open::{ExistingEntryType, open_existing_path},
    filesystem_publish::publish_file,
    locking::PathLock,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::CuError;

const SCHEMA_VERSION: u32 = 1;
const MAX_RECEIPT_BYTES: u64 = 128 * 1024;
/// Upper bound of a published ownership marker. A candidate temporary larger
/// than this can never be an unbound marker file of this transaction.
const MAX_MARKER_BYTES: u64 = 256;
const MARKER_PREFIX: &str = "agenterm-cu file.copy ownership marker";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileSnapshot {
    pub filesystem_id: String,
    pub object_id: String,
    pub size_bytes: String,
    pub modified_unix_ns: String,
    pub readonly: bool,
    pub unix_mode: Option<u32>,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileCopyPlan {
    pub schema_version: u32,
    pub operation: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub replace: bool,
    pub source_snapshot: FileSnapshot,
    pub destination_snapshot: Option<FileSnapshot>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionState {
    Reserved,
    CopyPrepared,
    BackupMoved,
    Completed,
    RollingBack,
    RolledBack,
    Finalizing,
    Finalized,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransactionReceipt {
    pub schema_version: u32,
    pub operation: String,
    pub transaction_id: String,
    pub state: TransactionState,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub replace: bool,
    pub source_snapshot: FileSnapshot,
    pub destination_snapshot: Option<FileSnapshot>,
    pub temporary: PathBuf,
    /// SHA-256 of the random ownership marker that is atomically published at
    /// the temporary path before any data is written there. The marker bytes
    /// are never stored or returned; recovery uses this digest to prove that a
    /// temporary without a persisted object identity belongs to this
    /// transaction instead of deleting by name.
    #[serde(default)]
    pub temporary_marker_sha256: Option<String>,
    pub temporary_identity: Option<ObjectIdentity>,
    pub prepared_snapshot: Option<FileSnapshot>,
    pub backup: Option<PathBuf>,
    pub result_snapshot: Option<FileSnapshot>,
    pub destination_durable: Option<bool>,
    pub created_unix_ms: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectIdentity {
    pub filesystem_id: String,
    pub object_id: String,
}

#[derive(Clone, Debug)]
pub struct FileTransactionStore {
    directory: PathBuf,
}

/// Crash points inside `apply` that tests use to leave the exact on-disk state
/// an interrupted process would leave. Production always passes [`Self::NONE`].
#[derive(Clone, Copy, Debug, Default)]
struct Interruptions {
    after_marker_staged: bool,
    after_marker_published: bool,
}

impl Interruptions {
    const NONE: Self = Self {
        after_marker_staged: false,
        after_marker_published: false,
    };
}

impl FileTransactionStore {
    pub fn open() -> Result<Self, CuError> {
        if let Some(path) = std::env::var_os("AGENTERM_CU_FILE_TRANSACTION_DIR") {
            return Self::open_at(path);
        }
        let directories = host_directories()
            .map_err(|error| failure("file_transaction_state_unavailable", error.to_string()))?;
        Self::open_at(
            directories
                .local_data
                .join("agenterm")
                .join("cu-file-transactions"),
        )
    }

    pub fn open_at(path: impl Into<PathBuf>) -> Result<Self, CuError> {
        let directory = path.into();
        fs::create_dir_all(&directory)
            .and_then(|()| protect_private_directory(&directory))
            .map_err(|error| failure("file_transaction_state_unavailable", error.to_string()))?;
        Ok(Self { directory })
    }

    pub fn plan(
        &self,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        replace: bool,
    ) -> Result<FileCopyPlan, CuError> {
        let source = canonical_regular(source.as_ref(), "source")?;
        let destination = exact_destination(destination.as_ref())?;
        if source == destination {
            return Err(failure(
                "file_transaction_same_path",
                "source and destination resolve to the same path",
            ));
        }
        let source_snapshot = snapshot_path(&source, "source")?;
        let destination_snapshot = optional_snapshot(&destination, "destination")?;
        if destination_snapshot.is_some() && !replace {
            return Err(failure(
                "file_transaction_replace_required",
                "destination exists; explicit --replace is required",
            ));
        }
        Ok(FileCopyPlan {
            schema_version: SCHEMA_VERSION,
            operation: "file.copy".into(),
            source,
            destination,
            replace,
            source_snapshot,
            destination_snapshot,
        })
    }

    pub fn apply(&self, plan: &FileCopyPlan) -> Result<FileTransactionReceipt, CuError> {
        self.apply_with(plan, Interruptions::NONE)
    }

    fn apply_with(
        &self,
        plan: &FileCopyPlan,
        interrupt: Interruptions,
    ) -> Result<FileTransactionReceipt, CuError> {
        validate_plan(plan)?;
        let _lock = self.destination_lock(&plan.destination)?;
        let fresh = self.plan(&plan.source, &plan.destination, plan.replace)?;
        if &fresh != plan {
            return Err(failure(
                "file_transaction_precondition_changed",
                "source or destination changed since planning",
            ));
        }
        let id = random_id()?;
        let parent = plan.destination.parent().expect("normalized destination");
        let temporary = parent.join(format!(".agenterm-copy-{id}.tmp"));
        let backup = plan
            .destination_snapshot
            .as_ref()
            .map(|_| parent.join(format!(".agenterm-copy-{id}.backup")));
        let mut receipt = FileTransactionReceipt {
            schema_version: SCHEMA_VERSION,
            operation: "file.copy".into(),
            transaction_id: id,
            state: TransactionState::Reserved,
            source: plan.source.clone(),
            destination: plan.destination.clone(),
            replace: plan.replace,
            source_snapshot: plan.source_snapshot.clone(),
            destination_snapshot: plan.destination_snapshot.clone(),
            temporary,
            temporary_marker_sha256: None,
            temporary_identity: None,
            prepared_snapshot: None,
            backup,
            result_snapshot: None,
            destination_durable: None,
            created_unix_ms: now_unix_ms()?.to_string(),
            recovery: None,
        };
        let marker = ownership_marker(&receipt.transaction_id)?;
        receipt.temporary_marker_sha256 = Some(hex_sha256(&marker));
        self.persist(&receipt)?;

        let result = (|| {
            let staging = marker_staging_path(&receipt);
            let mut temporary = stage_marker(&staging, &marker)?;
            if interrupt.after_marker_staged {
                return Err(interrupted());
            }
            publish_marker(&staging, &receipt.temporary, parent)?;
            if interrupt.after_marker_published {
                return Err(interrupted());
            }
            receipt.temporary_identity = Some(identity_of(&temporary)?);
            self.persist(&receipt)?;
            temporary
                .set_len(0)
                .and_then(|()| temporary.seek(SeekFrom::Start(0)).map(|_| ()))
                .map_err(|error| failure("file_transaction_prepare_failed", error.to_string()))?;

            let mut source = open_regular(&receipt.source, "source")?;
            ensure_snapshot(&mut source, &receipt.source_snapshot, "source")?;
            std::io::copy(&mut source, &mut temporary)
                .and_then(|_| temporary.set_permissions(source.metadata()?.permissions()))
                .and_then(|()| temporary.sync_all())
                .map_err(|error| failure("file_transaction_copy_failed", error.to_string()))?;
            let prepared = snapshot_opened(&mut temporary)?;
            if !same_content(&prepared, &receipt.source_snapshot) {
                return Err(failure(
                    "file_transaction_checksum_mismatch",
                    "prepared copy does not match the source",
                ));
            }
            receipt.prepared_snapshot = Some(prepared);
            receipt.state = TransactionState::CopyPrepared;
            self.persist(&receipt)?;
            drop(temporary);

            if let Some(backup) = &receipt.backup {
                ensure_path_snapshot(
                    &receipt.destination,
                    receipt
                        .destination_snapshot
                        .as_ref()
                        .expect("backup snapshot"),
                    "destination",
                )?;
                fs::rename(&receipt.destination, backup)
                    .and_then(|()| sync_parent(parent))
                    .map_err(|error| {
                        failure("file_transaction_backup_failed", error.to_string())
                    })?;
                receipt.state = TransactionState::BackupMoved;
                self.persist(&receipt)?;
            }
            let destination_durable = match publish_file(&receipt.temporary, &receipt.destination) {
                Ok(_) => true,
                Err(error) if error.published() => sync_parent(parent).is_ok(),
                Err(error) => {
                    return Err(
                        failure("file_transaction_publish_failed", error.to_string())
                            .with_detail(serde_json::json!({ "published": false })),
                    );
                }
            };
            let result = snapshot_path(&receipt.destination, "destination")?;
            if !same_copy_result(&result, &receipt.source_snapshot) {
                return Err(failure(
                    "file_transaction_readback_failed",
                    "installed destination does not match the source",
                ));
            }
            receipt.result_snapshot = Some(result);
            receipt.destination_durable = Some(destination_durable);
            receipt.state = TransactionState::Completed;
            self.persist(&receipt)?;
            Ok(receipt.clone())
        })();
        result.map_err(|error: CuError| {
            error.with_detail(serde_json::json!({
                "transaction_id": receipt.transaction_id,
                "recovery_required": true
            }))
        })
    }

    pub fn status(&self, transaction_id: &str) -> Result<FileTransactionReceipt, CuError> {
        self.read(transaction_id)
    }

    pub fn recover(&self, transaction_id: &str) -> Result<FileTransactionReceipt, CuError> {
        let (mut receipt, _lock) = self.locked_receipt(transaction_id)?;
        match receipt.state {
            TransactionState::Reserved
            | TransactionState::CopyPrepared
            | TransactionState::BackupMoved => {
                self.recover_reserved(&mut receipt)?;
                receipt.state = TransactionState::RolledBack;
                receipt.recovery = Some("pre-completion state restored".into());
            }
            TransactionState::RollingBack => {
                self.finish_rollback(&receipt)?;
                receipt.state = TransactionState::RolledBack;
                receipt.recovery = Some("rollback completed after interruption".into());
            }
            TransactionState::Finalizing => {
                self.finish_finalize(&receipt)?;
                receipt.state = TransactionState::Finalized;
                receipt.recovery = Some("finalize completed after interruption".into());
            }
            TransactionState::Completed => {
                return Err(failure(
                    "file_transaction_not_recoverable",
                    "completed transaction requires rollback or finalize",
                ));
            }
            TransactionState::RolledBack | TransactionState::Finalized => return Ok(receipt),
        }
        self.persist(&receipt)?;
        Ok(receipt)
    }

    pub fn rollback(&self, transaction_id: &str) -> Result<FileTransactionReceipt, CuError> {
        let (mut receipt, _lock) = self.locked_receipt(transaction_id)?;
        if receipt.state != TransactionState::Completed {
            return Err(invalid_state(&receipt, "rollback"));
        }
        ensure_path_snapshot(
            &receipt.destination,
            receipt
                .result_snapshot
                .as_ref()
                .ok_or_else(|| corrupt("missing result snapshot"))?,
            "destination",
        )?;
        verify_backup(&receipt)?;
        receipt.state = TransactionState::RollingBack;
        self.persist(&receipt)?;
        self.finish_rollback(&receipt)?;
        receipt.state = TransactionState::RolledBack;
        self.persist(&receipt)?;
        Ok(receipt)
    }

    pub fn finalize(&self, transaction_id: &str) -> Result<FileTransactionReceipt, CuError> {
        let (mut receipt, _lock) = self.locked_receipt(transaction_id)?;
        if receipt.state != TransactionState::Completed {
            return Err(invalid_state(&receipt, "finalize"));
        }
        ensure_path_snapshot(
            &receipt.destination,
            receipt
                .result_snapshot
                .as_ref()
                .ok_or_else(|| corrupt("missing result snapshot"))?,
            "destination",
        )?;
        verify_backup(&receipt)?;
        receipt.state = TransactionState::Finalizing;
        self.persist(&receipt)?;
        self.finish_finalize(&receipt)?;
        receipt.state = TransactionState::Finalized;
        self.persist(&receipt)?;
        Ok(receipt)
    }

    fn recover_reserved(&self, receipt: &mut FileTransactionReceipt) -> Result<(), CuError> {
        let destination = optional_snapshot(&receipt.destination, "destination")?;
        let backup = match &receipt.backup {
            Some(path) => optional_snapshot(path, "backup")?,
            None => None,
        };
        let installed = destination.as_ref().is_some_and(|snapshot| {
            same_content(snapshot, &receipt.source_snapshot)
                && receipt
                    .temporary_identity
                    .as_ref()
                    .is_some_and(|identity| same_object(snapshot, identity))
        });
        if installed {
            receipt.result_snapshot = destination;
            verify_backup(receipt)?;
            receipt.state = TransactionState::RollingBack;
            self.persist(receipt)?;
            return self.finish_rollback(receipt);
        }
        match (
            &receipt.destination_snapshot,
            destination.as_ref(),
            backup.as_ref(),
        ) {
            (None, None, None) => {}
            (Some(old), Some(current), None) if current == old => {}
            (Some(old), None, Some(saved)) if saved == old => {
                publish_file(
                    receipt.backup.as_ref().expect("backup"),
                    &receipt.destination,
                )
                .map_err(|error| failure("file_transaction_recovery_failed", error.to_string()))?;
            }
            _ => {
                return Err(failure(
                    "file_transaction_state_ambiguous",
                    "destination and backup do not uniquely match the durable receipt",
                ));
            }
        }
        remove_owned_temporary(receipt)?;
        Ok(())
    }

    fn finish_rollback(&self, receipt: &FileTransactionReceipt) -> Result<(), CuError> {
        let current = optional_snapshot(&receipt.destination, "destination")?;
        match &receipt.destination_snapshot {
            None => {
                if let Some(current) = current {
                    let result = receipt
                        .result_snapshot
                        .as_ref()
                        .ok_or_else(|| corrupt("missing result snapshot"))?;
                    if &current != result {
                        return Err(changed("destination"));
                    }
                    fs::remove_file(&receipt.destination)
                        .and_then(|()| sync_parent(receipt.destination.parent().unwrap()))
                        .map_err(|error| {
                            failure("file_transaction_rollback_failed", error.to_string())
                        })?;
                }
            }
            Some(old) => {
                if current.as_ref() == Some(old)
                    && receipt.backup.as_ref().is_some_and(|p| !p.exists())
                {
                    return Ok(());
                }
                if current.as_ref() != receipt.result_snapshot.as_ref() {
                    return Err(changed("destination"));
                }
                let backup = receipt
                    .backup
                    .as_ref()
                    .ok_or_else(|| corrupt("missing backup path"))?;
                ensure_path_snapshot(backup, old, "backup")?;
                publish_file(backup, &receipt.destination).map_err(|error| {
                    failure("file_transaction_rollback_failed", error.to_string())
                })?;
            }
        }
        Ok(())
    }

    fn finish_finalize(&self, receipt: &FileTransactionReceipt) -> Result<(), CuError> {
        ensure_path_snapshot(
            &receipt.destination,
            receipt
                .result_snapshot
                .as_ref()
                .ok_or_else(|| corrupt("missing result snapshot"))?,
            "destination",
        )?;
        if let (Some(backup), Some(old)) = (&receipt.backup, &receipt.destination_snapshot) {
            match optional_snapshot(backup, "backup")? {
                Some(snapshot) if &snapshot == old => fs::remove_file(backup)
                    .and_then(|()| sync_parent(receipt.destination.parent().unwrap()))
                    .map_err(|error| {
                        failure("file_transaction_finalize_failed", error.to_string())
                    })?,
                None => {}
                Some(_) => return Err(changed("backup")),
            }
        }
        Ok(())
    }

    fn persist(&self, receipt: &FileTransactionReceipt) -> Result<(), CuError> {
        validate_receipt(receipt)?;
        let mut bytes = serde_json::to_vec(receipt)
            .map_err(|error| failure("file_transaction_state_failed", error.to_string()))?;
        if bytes.len() as u64 > MAX_RECEIPT_BYTES {
            return Err(corrupt("receipt exceeds its byte bound"));
        }
        bytes.push(b'\n');
        write_private_atomic(&self.receipt_path(&receipt.transaction_id)?, &bytes)
            .map_err(|error| failure("file_transaction_state_failed", error.to_string()))
    }

    fn read(&self, transaction_id: &str) -> Result<FileTransactionReceipt, CuError> {
        let path = self.receipt_path(transaction_id)?;
        let file = open_existing_path(&path, ExistingEntryType::File)
            .map_err(|error| failure("file_transaction_not_found", error.to_string()))?;
        if file
            .metadata()
            .map(|m| m.len())
            .unwrap_or(MAX_RECEIPT_BYTES + 1)
            > MAX_RECEIPT_BYTES
        {
            return Err(corrupt("receipt exceeds its byte bound"));
        }
        let receipt: FileTransactionReceipt = serde_json::from_reader(file)
            .map_err(|error| corrupt(format!("invalid receipt: {error}")))?;
        validate_receipt(&receipt)?;
        if receipt.transaction_id != transaction_id {
            return Err(corrupt("receipt transaction id mismatch"));
        }
        Ok(receipt)
    }

    fn receipt_path(&self, id: &str) -> Result<PathBuf, CuError> {
        validate_id(id)?;
        Ok(self.directory.join(format!("{id}.json")))
    }

    fn destination_lock(&self, destination: &Path) -> Result<PathLock, CuError> {
        let digest = destination_digest(destination);
        PathLock::try_acquire(&self.directory.join(format!("destination-{digest}.lock")))
            .map_err(|error| failure("file_transaction_busy", error.to_string()))
    }

    fn locked_receipt(
        &self,
        transaction_id: &str,
    ) -> Result<(FileTransactionReceipt, PathLock), CuError> {
        let before = self.read(transaction_id)?;
        let lock = self.destination_lock(&before.destination)?;
        let after = self.read(transaction_id)?;
        if before.destination != after.destination {
            return Err(corrupt(
                "destination changed while acquiring its transaction lock",
            ));
        }
        Ok((after, lock))
    }
}

fn exact_destination(path: &Path) -> Result<PathBuf, CuError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| failure("file_transaction_path_invalid", error.to_string()))?
            .join(path)
    };
    let name = absolute.file_name().ok_or_else(|| {
        failure(
            "file_transaction_path_invalid",
            "destination requires a file name",
        )
    })?;
    let parent = absolute.parent().ok_or_else(|| {
        failure(
            "file_transaction_path_invalid",
            "destination requires a parent",
        )
    })?;
    let parent = fs::canonicalize(parent)
        .map_err(|error| failure("file_transaction_path_invalid", error.to_string()))?;
    Ok(parent.join(name))
}

fn canonical_regular(path: &Path, label: &str) -> Result<PathBuf, CuError> {
    let entry = fs::symlink_metadata(path)
        .map_err(|error| failure("file_transaction_path_invalid", format!("{label}: {error}")))?;
    if entry.file_type().is_symlink() || !entry.file_type().is_file() {
        return Err(failure(
            "file_transaction_entry_invalid",
            format!("{label} must be a non-symlink regular file"),
        ));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| failure("file_transaction_path_invalid", format!("{label}: {error}")))?;
    let _ = open_regular(&canonical, label)?;
    Ok(canonical)
}

fn open_regular(path: &Path, label: &str) -> Result<File, CuError> {
    open_existing_path(path, ExistingEntryType::File).map_err(|error| {
        failure(
            "file_transaction_entry_invalid",
            format!("{label} must be a non-symlink regular file: {error}"),
        )
    })
}

fn snapshot_path(path: &Path, label: &str) -> Result<FileSnapshot, CuError> {
    let mut file = open_regular(path, label)?;
    snapshot_opened(&mut file)
}

fn optional_snapshot(path: &Path, label: &str) -> Result<Option<FileSnapshot>, CuError> {
    match fs::symlink_metadata(path) {
        Ok(_) => snapshot_path(path, label).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(
            "file_transaction_inspect_failed",
            error.to_string(),
        )),
    }
}

fn snapshot_opened(file: &mut File) -> Result<FileSnapshot, CuError> {
    let identity = file_identity(file)
        .map_err(|error| failure("file_transaction_identity_failed", error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| failure("file_transaction_inspect_failed", error.to_string()))?;
    let modified = metadata
        .modified()
        .and_then(|time| {
            time.duration_since(UNIX_EPOCH)
                .map_err(std::io::Error::other)
        })
        .map_err(|error| failure("file_transaction_inspect_failed", error.to_string()))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| failure("file_transaction_read_failed", error.to_string()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| failure("file_transaction_read_failed", error.to_string()))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| failure("file_transaction_read_failed", error.to_string()))?;
    let digest = hash.finalize();
    Ok(FileSnapshot {
        filesystem_id: identity.filesystem_id.to_string(),
        object_id: identity.object_id.to_string(),
        size_bytes: metadata.len().to_string(),
        modified_unix_ns: (modified.as_secs() as u128 * 1_000_000_000
            + modified.subsec_nanos() as u128)
            .to_string(),
        readonly: metadata.permissions().readonly(),
        unix_mode: unix_mode(&metadata),
        sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    })
}

fn ensure_snapshot(file: &mut File, expected: &FileSnapshot, label: &str) -> Result<(), CuError> {
    if &snapshot_opened(file)? == expected {
        Ok(())
    } else {
        Err(changed(label))
    }
}

fn ensure_path_snapshot(path: &Path, expected: &FileSnapshot, label: &str) -> Result<(), CuError> {
    if &snapshot_path(path, label)? == expected {
        Ok(())
    } else {
        Err(changed(label))
    }
}

fn same_content(left: &FileSnapshot, right: &FileSnapshot) -> bool {
    left.size_bytes == right.size_bytes && left.sha256 == right.sha256
}

fn same_copy_result(left: &FileSnapshot, right: &FileSnapshot) -> bool {
    same_content(left, right)
        && left.readonly == right.readonly
        && left.unix_mode == right.unix_mode
}

#[cfg(unix)]
fn unix_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    Some(metadata.mode())
}

#[cfg(windows)]
fn unix_mode(_metadata: &fs::Metadata) -> Option<u32> {
    None
}

fn same_object(snapshot: &FileSnapshot, identity: &ObjectIdentity) -> bool {
    snapshot.filesystem_id == identity.filesystem_id && snapshot.object_id == identity.object_id
}

fn identity_of(file: &File) -> Result<ObjectIdentity, CuError> {
    let identity = file_identity(file)
        .map_err(|error| failure("file_transaction_identity_failed", error.to_string()))?;
    Ok(ObjectIdentity {
        filesystem_id: identity.filesystem_id.to_string(),
        object_id: identity.object_id.to_string(),
    })
}

fn ownership_marker(transaction_id: &str) -> Result<Vec<u8>, CuError> {
    let nonce = secure_random_array::<32>()
        .map_err(|error| failure("file_transaction_entropy_failed", error.to_string()))?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{MARKER_PREFIX} {transaction_id} {nonce}\n").into_bytes())
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn marker_staging_path(receipt: &FileTransactionReceipt) -> PathBuf {
    receipt
        .destination
        .parent()
        .expect("normalized destination")
        .join(format!(".agenterm-copy-{}.marker", receipt.transaction_id))
}

/// Exclusively creates the marker beside the destination and makes it durable
/// before it is published at the temporary path.
fn stage_marker(staging: &Path, marker: &[u8]) -> Result<File, CuError> {
    let mut file = private_create_new_options()
        .read(true)
        .write(true)
        .open(staging)
        .map_err(|error| failure("file_transaction_prepare_failed", error.to_string()))?;
    file.write_all(marker)
        .and_then(|()| file.sync_all())
        .map_err(|error| failure("file_transaction_prepare_failed", error.to_string()))?;
    Ok(file)
}

/// Publishes the complete marker at the temporary path with a same-directory
/// hard link. Link creation is atomic and refuses an occupied destination, so
/// there is no check-then-rename window that could replace an unrelated file.
/// A crash may leave both names, but recovery proves and removes each by the
/// same persisted marker digest.
fn publish_marker(staging: &Path, temporary: &Path, parent: &Path) -> Result<(), CuError> {
    fs::hard_link(staging, temporary)
        .and_then(|()| fs::remove_file(staging))
        .and_then(|()| sync_parent(parent))
        .map_err(|error| failure("file_transaction_prepare_failed", error.to_string()))
}

/// True only when the opened file is small enough to be a marker and its
/// complete content hashes to the persisted marker digest.
fn marker_matches(file: &mut File, digest_hex: &str) -> Result<bool, CuError> {
    let length = file
        .metadata()
        .map_err(|error| failure("file_transaction_inspect_failed", error.to_string()))?
        .len();
    if length > MAX_MARKER_BYTES {
        return Ok(false);
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.take(MAX_MARKER_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| failure("file_transaction_read_failed", error.to_string()))?;
    Ok(bytes.len() as u64 <= MAX_MARKER_BYTES && hex_sha256(&bytes) == digest_hex)
}

fn remove_owned_temporary(receipt: &FileTransactionReceipt) -> Result<(), CuError> {
    remove_marker_owned(receipt, &marker_staging_path(receipt), "marker staging")?;
    let Some(expected) = &receipt.temporary_identity else {
        return remove_marker_owned(receipt, &receipt.temporary, "temporary");
    };
    let Some(snapshot) = optional_snapshot(&receipt.temporary, "temporary")? else {
        return Ok(());
    };
    if !same_object(&snapshot, expected) {
        return Err(changed("temporary"));
    }
    fs::remove_file(&receipt.temporary)
        .and_then(|()| sync_parent(receipt.destination.parent().unwrap()))
        .map_err(|error| failure("file_transaction_recovery_failed", error.to_string()))
}

/// Removes a transaction file that never reached a persisted object identity,
/// but only when its complete content is this transaction's marker. Anything
/// else at that derived path is preserved and reported as ambiguous.
fn remove_marker_owned(
    receipt: &FileTransactionReceipt,
    path: &Path,
    label: &str,
) -> Result<(), CuError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(failure(
                "file_transaction_inspect_failed",
                error.to_string(),
            ));
        }
    }
    let Some(digest) = &receipt.temporary_marker_sha256 else {
        return Err(ambiguous(format!(
            "{label} exists without a durable ownership identity or marker"
        )));
    };
    let mut file = open_regular(path, label).map_err(|_| {
        ambiguous(format!(
            "{label} is not a regular file that this transaction can own"
        ))
    })?;
    if !marker_matches(&mut file, digest)? {
        return Err(ambiguous(format!(
            "{label} does not carry this transaction's ownership marker"
        )));
    }
    drop(file);
    fs::remove_file(path)
        .and_then(|()| sync_parent(receipt.destination.parent().unwrap()))
        .map_err(|error| failure("file_transaction_recovery_failed", error.to_string()))
}

fn verify_backup(receipt: &FileTransactionReceipt) -> Result<(), CuError> {
    match (&receipt.backup, &receipt.destination_snapshot) {
        (None, None) => Ok(()),
        (Some(path), Some(snapshot)) => ensure_path_snapshot(path, snapshot, "backup"),
        _ => Err(corrupt("backup fields disagree")),
    }
}

fn destination_digest(destination: &Path) -> String {
    let mut hash = Sha256::new();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        hash.update(destination.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        for unit in destination.as_os_str().encode_wide() {
            hash.update(unit.to_le_bytes());
        }
    }
    let digest = hash.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_plan(plan: &FileCopyPlan) -> Result<(), CuError> {
    if plan.schema_version != SCHEMA_VERSION || plan.operation != "file.copy" {
        return Err(corrupt("unsupported plan schema"));
    }
    Ok(())
}

fn validate_receipt(receipt: &FileTransactionReceipt) -> Result<(), CuError> {
    if receipt.schema_version != SCHEMA_VERSION || receipt.operation != "file.copy" {
        return Err(corrupt("unsupported receipt schema"));
    }
    validate_id(&receipt.transaction_id)?;
    let parent = receipt
        .destination
        .parent()
        .ok_or_else(|| corrupt("destination has no parent"))?;
    let expected_temporary = parent.join(format!(".agenterm-copy-{}.tmp", receipt.transaction_id));
    let expected_backup = parent.join(format!(".agenterm-copy-{}.backup", receipt.transaction_id));
    if receipt.temporary != expected_temporary {
        return Err(corrupt(
            "temporary path is not derived from the transaction id",
        ));
    }
    match (&receipt.backup, &receipt.destination_snapshot) {
        (None, None) => {}
        (Some(backup), Some(_)) if backup == &expected_backup => {}
        _ => return Err(corrupt("backup path and destination snapshot disagree")),
    }
    if receipt
        .temporary_marker_sha256
        .as_ref()
        .is_some_and(|digest| {
            digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(corrupt("temporary marker digest is malformed"));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), CuError> {
    if id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(failure(
            "file_transaction_id_invalid",
            "transaction id must be 32 hexadecimal characters",
        ))
    }
}

fn random_id() -> Result<String, CuError> {
    let bytes = secure_random_array::<16>()
        .map_err(|error| failure("file_transaction_entropy_failed", error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn now_unix_ms() -> Result<u128, CuError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .map_err(|error| failure("file_transaction_clock_failed", error.to_string()))
}

fn invalid_state(receipt: &FileTransactionReceipt, action: &str) -> CuError {
    failure(
        "file_transaction_state_invalid",
        format!("cannot {action} transaction in state {:?}", receipt.state),
    )
}

fn changed(label: &str) -> CuError {
    failure(
        "file_transaction_precondition_changed",
        format!("{label} no longer matches the durable transaction snapshot"),
    )
}

fn corrupt(message: impl Into<String>) -> CuError {
    failure("file_transaction_state_corrupt", message)
}

fn ambiguous(message: impl Into<String>) -> CuError {
    failure("file_transaction_state_ambiguous", message)
}

fn interrupted() -> CuError {
    failure(
        "file_transaction_interrupted",
        "apply stopped at a test-only crash point",
    )
}

fn failure(code: &'static str, message: impl Into<String>) -> CuError {
    CuError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(label: &str) -> (PathBuf, FileTransactionStore) {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap_or_else(|_| std::env::temp_dir())
            .join(format!(
                "agenterm-cu-file-transaction-{label}-{}",
                std::process::id()
            ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = FileTransactionStore::open_at(root.join("state")).unwrap();
        (root, store)
    }

    #[test]
    fn new_copy_can_roll_back() {
        let (root, store) = fixture("new-rollback");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new value").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let plan = store.plan(&source, &destination, false).unwrap();
        assert!(!destination.exists());
        let receipt = store.apply(&plan).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new value");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }
        let rolled = store.rollback(&receipt.transaction_id).unwrap();
        assert_eq!(rolled.state, TransactionState::RolledBack);
        assert!(!destination.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_keeps_backup_until_finalize() {
        let (root, store) = fixture("replace-finalize");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        let receipt = store
            .apply(&store.plan(&source, &destination, true).unwrap())
            .unwrap();
        assert!(receipt.backup.as_ref().unwrap().exists());
        let finalized = store.finalize(&receipt.transaction_id).unwrap();
        assert_eq!(finalized.state, TransactionState::Finalized);
        assert!(!receipt.backup.as_ref().unwrap().exists());
        assert_eq!(fs::read(destination).unwrap(), b"new");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_rollback_restores_exact_old_object() {
        let (root, store) = fixture("replace-rollback");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        let old = snapshot_path(&destination, "destination").unwrap();
        let receipt = store
            .apply(&store.plan(&source, &destination, true).unwrap())
            .unwrap();
        let rolled = store.rollback(&receipt.transaction_id).unwrap();
        assert_eq!(rolled.state, TransactionState::RolledBack);
        assert_eq!(snapshot_path(&destination, "destination").unwrap(), old);
        assert_eq!(fs::read(destination).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_destination_refuses_rollback() {
        let (root, store) = fixture("changed");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        let receipt = store
            .apply(&store.plan(&source, &destination, false).unwrap())
            .unwrap();
        fs::write(&destination, b"third party").unwrap();
        assert_eq!(
            store.rollback(&receipt.transaction_id).unwrap_err().code,
            "file_transaction_precondition_changed"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_source_refuses_apply_before_reservation() {
        let (root, store) = fixture("source-drift");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"first").unwrap();
        let plan = store.plan(&source, &destination, false).unwrap();
        fs::write(&source, b"second").unwrap();
        assert_eq!(
            store.apply(&plan).unwrap_err().code,
            "file_transaction_precondition_changed"
        );
        assert!(fs::read_dir(&store.directory).unwrap().all(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_none_or(|ext| ext != "json")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_restores_backup_moved_before_phase_receipt() {
        let (root, store) = fixture("recover-backup");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        let mut receipt = store
            .apply(&store.plan(&source, &destination, true).unwrap())
            .unwrap();
        fs::rename(&destination, &receipt.temporary).unwrap();
        receipt.state = TransactionState::CopyPrepared;
        receipt.result_snapshot = None;
        store.persist(&receipt).unwrap();

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, TransactionState::RolledBack);
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        assert!(!receipt.temporary.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_detects_install_before_completed_receipt() {
        let (root, store) = fixture("recover-installed");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        let mut receipt = store
            .apply(&store.plan(&source, &destination, false).unwrap())
            .unwrap();
        receipt.state = TransactionState::CopyPrepared;
        receipt.result_snapshot = None;
        store.persist(&receipt).unwrap();

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, TransactionState::RolledBack);
        assert!(!destination.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_destination_uses_one_cross_process_lock_key() {
        let (root, store) = fixture("lock-key");
        let destination = root.join("destination");
        let _held = store.destination_lock(&destination).unwrap();
        let digest = destination_digest(&destination);
        let error = match PathLock::try_acquire(
            &store.directory.join(format!("destination-{digest}.lock")),
        ) {
            Ok(_) => panic!("same destination bypassed the held lock"),
            Err(error) => error,
        };
        assert_eq!(
            error.kind(),
            agenterm_platform::locking::LockErrorKind::Contended
        );
        drop(_held);
        fs::remove_dir_all(root).unwrap();
    }
    fn interrupted_apply(
        store: &FileTransactionStore,
        source: &Path,
        destination: &Path,
        interrupt: Interruptions,
    ) -> FileTransactionReceipt {
        let plan = store.plan(source, destination, false).unwrap();
        let error = store.apply_with(&plan, interrupt).unwrap_err();
        assert_eq!(error.code, "file_transaction_interrupted");
        let id = error.detail.unwrap()["transaction_id"]
            .as_str()
            .unwrap()
            .to_owned();
        store.status(&id).unwrap()
    }

    fn replaced_fixture(label: &str) -> (PathBuf, FileTransactionStore, FileTransactionReceipt) {
        let (root, store) = fixture(label);
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        let receipt = store
            .apply(&store.plan(&source, &destination, true).unwrap())
            .unwrap();
        (root, store, receipt)
    }

    #[test]
    fn recover_cleans_published_marker_without_persisted_identity() {
        let (root, store) = fixture("marker-published");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        let receipt = interrupted_apply(
            &store,
            &source,
            &destination,
            Interruptions {
                after_marker_published: true,
                ..Interruptions::default()
            },
        );
        assert_eq!(receipt.state, TransactionState::Reserved);
        assert!(receipt.temporary_identity.is_none());
        let digest = receipt.temporary_marker_sha256.clone().unwrap();
        let marker = fs::read(&receipt.temporary).unwrap();
        assert!(marker.starts_with(MARKER_PREFIX.as_bytes()));
        assert_eq!(hex_sha256(&marker), digest);
        assert!(!marker_staging_path(&receipt).exists());
        assert!(!destination.exists());

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, TransactionState::RolledBack);
        assert!(!receipt.temporary.exists());
        assert!(!destination.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn marker_publication_refuses_an_occupied_temporary_without_replacement() {
        let (root, _store) = fixture("marker-occupied");
        let staging = root.join("marker.staged");
        let temporary = root.join("marker.tmp");
        fs::write(&staging, b"owned-marker").unwrap();
        fs::write(&temporary, b"unrelated-object").unwrap();
        let error = publish_marker(&staging, &temporary, &root).expect_err("occupied temporary");
        assert_eq!(error.code, "file_transaction_prepare_failed");
        assert_eq!(fs::read(&temporary).unwrap(), b"unrelated-object");
        assert_eq!(fs::read(&staging).unwrap(), b"owned-marker");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_cleans_staged_marker_before_publication() {
        let (root, store) = fixture("marker-staged");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        let receipt = interrupted_apply(
            &store,
            &source,
            &destination,
            Interruptions {
                after_marker_staged: true,
                ..Interruptions::default()
            },
        );
        let staging = marker_staging_path(&receipt);
        assert!(staging.exists());
        assert!(!receipt.temporary.exists());

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, TransactionState::RolledBack);
        assert!(!staging.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn altered_marker_is_ambiguous_and_preserved() {
        let (root, store) = fixture("marker-altered");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        let receipt = interrupted_apply(
            &store,
            &source,
            &destination,
            Interruptions {
                after_marker_published: true,
                ..Interruptions::default()
            },
        );
        fs::write(&receipt.temporary, b"foreign content at the derived path").unwrap();

        let error = store.recover(&receipt.transaction_id).unwrap_err();
        assert_eq!(error.code, "file_transaction_state_ambiguous");
        assert_eq!(
            fs::read(&receipt.temporary).unwrap(),
            b"foreign content at the derived path"
        );
        assert_eq!(
            store.status(&receipt.transaction_id).unwrap().state,
            TransactionState::Reserved
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_without_temporary_after_marker_phase_is_normal() {
        let (root, store) = fixture("marker-missing");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        let receipt = interrupted_apply(
            &store,
            &source,
            &destination,
            Interruptions {
                after_marker_published: true,
                ..Interruptions::default()
            },
        );
        fs::remove_file(&receipt.temporary).unwrap();

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, TransactionState::RolledBack);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_receipt_binds_temporary_identity_and_marker_digest() {
        let (root, store) = fixture("marker-completed");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        let receipt = store
            .apply(&store.plan(&source, &destination, false).unwrap())
            .unwrap();
        assert!(receipt.temporary_identity.is_some());
        assert!(receipt.temporary_marker_sha256.is_some());
        assert!(!receipt.temporary.exists());
        assert!(!marker_staging_path(&receipt).exists());
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_resumes_interrupted_rollback() {
        let (root, store, mut receipt) = replaced_fixture("resume-rollback");
        let destination = receipt.destination.clone();
        let backup = receipt.backup.clone().unwrap();
        receipt.state = TransactionState::RollingBack;
        store.persist(&receipt).unwrap();

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, TransactionState::RolledBack);
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        assert!(!backup.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_accepts_rollback_that_already_restored_the_destination() {
        let (root, store, mut receipt) = replaced_fixture("resume-rollback-done");
        let destination = receipt.destination.clone();
        let backup = receipt.backup.clone().unwrap();
        receipt.state = TransactionState::RollingBack;
        store.persist(&receipt).unwrap();
        fs::rename(&backup, &destination).unwrap();

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, TransactionState::RolledBack);
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_resumes_interrupted_finalize() {
        let (root, store, mut receipt) = replaced_fixture("resume-finalize");
        let destination = receipt.destination.clone();
        let backup = receipt.backup.clone().unwrap();
        receipt.state = TransactionState::Finalizing;
        store.persist(&receipt).unwrap();

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, TransactionState::Finalized);
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!backup.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_accepts_finalize_that_already_removed_the_backup() {
        let (root, store, mut receipt) = replaced_fixture("resume-finalize-done");
        let destination = receipt.destination.clone();
        let backup = receipt.backup.clone().unwrap();
        receipt.state = TransactionState::Finalizing;
        store.persist(&receipt).unwrap();
        fs::remove_file(&backup).unwrap();

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, TransactionState::Finalized);
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        fs::remove_dir_all(root).unwrap();
    }
}
