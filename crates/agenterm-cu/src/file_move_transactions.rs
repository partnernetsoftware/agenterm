//! Crash-recoverable regular-file move transactions.
//!
//! A move is the hardened copy transaction (private receipt before every
//! effect, ownership marker before data, opened-object identities, atomic
//! same-directory publication) followed by retiring the source into a
//! same-directory backup. Both the replaced-destination backup and the source
//! backup are retained until `finalize`; `rollback` restores the exact recorded
//! objects. Paths alone never confer ownership: every file that recovery may
//! remove or rename is bound to an object identity or a complete marker.
//!
//! Copying instead of renaming keeps one code path for same-volume and
//! cross-volume moves and keeps every phase idempotent for recovery.

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

use crate::{
    CuError,
    file_transactions::{
        FileSnapshot, ObjectIdentity, restore_retired_regular, retire_regular_no_replace,
    },
};

/// Receipt `operation` value that routes a transaction id to this module.
pub const OPERATION: &str = "file.move";
const SCHEMA_VERSION: u32 = 1;
const MAX_RECEIPT_BYTES: u64 = 128 * 1024;
const MAX_MARKER_BYTES: u64 = 256;
const MARKER_PREFIX: &str = "agenterm-cu file.move ownership marker";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileMovePlan {
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
pub enum MoveState {
    Reserved,
    CopyPrepared,
    BackupMoved,
    /// Destination published and read back; the source is not yet retired.
    Installed,
    Completed,
    RollingBack,
    RolledBack,
    Finalizing,
    Finalized,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileMoveReceipt {
    pub schema_version: u32,
    pub operation: String,
    pub transaction_id: String,
    pub state: MoveState,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub replace: bool,
    pub source_snapshot: FileSnapshot,
    pub destination_snapshot: Option<FileSnapshot>,
    pub temporary: PathBuf,
    /// SHA-256 of the random marker published at `temporary` before data is
    /// written there; the marker bytes themselves are never stored.
    pub temporary_marker_sha256: Option<String>,
    pub temporary_identity: Option<ObjectIdentity>,
    pub prepared_snapshot: Option<FileSnapshot>,
    /// Same-directory backup of a replaced destination.
    pub backup: Option<PathBuf>,
    /// Same-directory backup the source is retired into once the destination
    /// is installed. Retained until finalize.
    pub source_backup: PathBuf,
    pub result_snapshot: Option<FileSnapshot>,
    pub destination_durable: Option<bool>,
    pub created_unix_ms: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FileMoveStore {
    directory: PathBuf,
}

/// Crash points inside `apply` that tests use to leave the exact on-disk state
/// an interrupted process would leave. Production always passes [`Self::NONE`].
#[derive(Clone, Copy, Debug, Default)]
struct Interruptions {
    after_marker_published: bool,
    after_install: bool,
}

impl Interruptions {
    const NONE: Self = Self {
        after_marker_published: false,
        after_install: false,
    };
}

impl FileMoveStore {
    /// Opens the same private state directory the copy transaction uses, so
    /// one transaction id namespace and one destination lock namespace serve
    /// both operations.
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

    /// Reads only the `operation` field of a persisted receipt so a caller can
    /// route one transaction id to the owning module without either module
    /// having to accept the other's schema.
    pub fn peek_operation(&self, transaction_id: &str) -> Result<String, CuError> {
        let file = self.open_receipt(transaction_id)?;
        let value: serde_json::Value = serde_json::from_reader(file)
            .map_err(|error| corrupt(format!("invalid receipt: {error}")))?;
        value["operation"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| corrupt("receipt has no operation"))
    }

    pub fn plan(
        &self,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        replace: bool,
    ) -> Result<FileMovePlan, CuError> {
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
        Ok(FileMovePlan {
            schema_version: SCHEMA_VERSION,
            operation: OPERATION.into(),
            source,
            destination,
            replace,
            source_snapshot,
            destination_snapshot,
        })
    }

    pub fn apply(&self, plan: &FileMovePlan) -> Result<FileMoveReceipt, CuError> {
        self.apply_with(plan, Interruptions::NONE)
    }

    fn apply_with(
        &self,
        plan: &FileMovePlan,
        interrupt: Interruptions,
    ) -> Result<FileMoveReceipt, CuError> {
        validate_plan(plan)?;
        let _locks = self.path_locks(&plan.source, &plan.destination)?;
        let fresh = self.plan(&plan.source, &plan.destination, plan.replace)?;
        if &fresh != plan {
            return Err(failure(
                "file_transaction_precondition_changed",
                "source or destination changed since planning",
            ));
        }
        let id = random_id()?;
        let parent = plan.destination.parent().expect("normalized destination");
        let source_parent = plan.source.parent().expect("canonical source");
        let mut receipt = FileMoveReceipt {
            schema_version: SCHEMA_VERSION,
            operation: OPERATION.into(),
            transaction_id: id.clone(),
            state: MoveState::Reserved,
            source: plan.source.clone(),
            destination: plan.destination.clone(),
            replace: plan.replace,
            source_snapshot: plan.source_snapshot.clone(),
            destination_snapshot: plan.destination_snapshot.clone(),
            temporary: parent.join(format!(".agenterm-move-{id}.tmp")),
            temporary_marker_sha256: None,
            temporary_identity: None,
            prepared_snapshot: None,
            backup: plan
                .destination_snapshot
                .as_ref()
                .map(|_| parent.join(format!(".agenterm-move-{id}.backup"))),
            source_backup: source_parent.join(format!(".agenterm-move-{id}.source")),
            result_snapshot: None,
            destination_durable: None,
            created_unix_ms: now_unix_ms()?.to_string(),
            recovery: None,
        };
        match fs::symlink_metadata(&receipt.source_backup) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(failure(
                    "file_transaction_prepare_failed",
                    "source backup path is already occupied",
                ));
            }
            Err(error) => {
                return Err(failure(
                    "file_transaction_inspect_failed",
                    error.to_string(),
                ));
            }
        }
        let marker = ownership_marker(&receipt.transaction_id)?;
        receipt.temporary_marker_sha256 = Some(hex_sha256(&marker));
        self.persist(&receipt)?;

        let result = (|| {
            let staging = marker_staging_path(&receipt);
            let mut temporary = stage_marker(&staging, &marker)?;
            publish_marker(&staging, &receipt.temporary, parent)?;
            if interrupt.after_marker_published {
                return Err(interrupted());
            }
            // The staging handle is the published object: no reopen by path,
            // so nothing can be swapped in between verification and binding.
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
            receipt.state = MoveState::CopyPrepared;
            self.persist(&receipt)?;
            drop(temporary);
            drop(source);

            if let Some(backup) = &receipt.backup {
                ensure_path_snapshot(
                    &receipt.destination,
                    receipt
                        .destination_snapshot
                        .as_ref()
                        .expect("backup snapshot"),
                    "destination",
                )?;
                retire_regular_no_replace(
                    &receipt.destination,
                    backup,
                    receipt
                        .destination_snapshot
                        .as_ref()
                        .expect("backup snapshot"),
                )?;
                receipt.state = MoveState::BackupMoved;
                self.persist(&receipt)?;
            }
            let staged = snapshot_path(&receipt.temporary, "temporary")?;
            if receipt
                .temporary_identity
                .as_ref()
                .is_none_or(|identity| !same_object(&staged, identity))
                || receipt.prepared_snapshot.as_ref() != Some(&staged)
            {
                return Err(ambiguous(
                    "temporary path no longer names the prepared object",
                ));
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
            receipt.state = MoveState::Installed;
            self.persist(&receipt)?;
            if interrupt.after_install {
                return Err(interrupted());
            }

            ensure_path_snapshot(&receipt.source, &receipt.source_snapshot, "source")?;
            retire_regular_no_replace(
                &receipt.source,
                &receipt.source_backup,
                &receipt.source_snapshot,
            )?;
            receipt.state = MoveState::Completed;
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

    pub fn status(&self, transaction_id: &str) -> Result<FileMoveReceipt, CuError> {
        self.read(transaction_id)
    }

    pub fn recover(&self, transaction_id: &str) -> Result<FileMoveReceipt, CuError> {
        let (mut receipt, _locks) = self.locked_receipt(transaction_id)?;
        match receipt.state {
            MoveState::Reserved | MoveState::CopyPrepared | MoveState::BackupMoved => {
                self.recover_reserved(&mut receipt)?;
                receipt.state = MoveState::RolledBack;
                receipt.recovery = Some("pre-completion state restored".into());
            }
            MoveState::Installed => {
                verify_destination_result(&receipt)?;
                verify_backup(&receipt)?;
                verify_source_for_rollback(&receipt)?;
                receipt.state = MoveState::RollingBack;
                self.persist(&receipt)?;
                self.finish_rollback(&receipt)?;
                receipt.state = MoveState::RolledBack;
                receipt.recovery = Some("installed-before-retire state restored".into());
            }
            MoveState::RollingBack => {
                self.finish_rollback(&receipt)?;
                receipt.state = MoveState::RolledBack;
                receipt.recovery = Some("rollback completed after interruption".into());
            }
            MoveState::Finalizing => {
                self.finish_finalize(&receipt)?;
                receipt.state = MoveState::Finalized;
                receipt.recovery = Some("finalize completed after interruption".into());
            }
            MoveState::Completed => {
                return Err(failure(
                    "file_transaction_not_recoverable",
                    "completed transaction requires rollback or finalize",
                ));
            }
            MoveState::RolledBack | MoveState::Finalized => return Ok(receipt),
        }
        self.persist(&receipt)?;
        Ok(receipt)
    }

    pub fn rollback(&self, transaction_id: &str) -> Result<FileMoveReceipt, CuError> {
        let (mut receipt, _locks) = self.locked_receipt(transaction_id)?;
        if receipt.state != MoveState::Completed {
            return Err(invalid_state(&receipt, "rollback"));
        }
        verify_destination_result(&receipt)?;
        verify_backup(&receipt)?;
        verify_source_for_rollback(&receipt)?;
        receipt.state = MoveState::RollingBack;
        self.persist(&receipt)?;
        self.finish_rollback(&receipt)?;
        receipt.state = MoveState::RolledBack;
        self.persist(&receipt)?;
        Ok(receipt)
    }

    pub fn finalize(&self, transaction_id: &str) -> Result<FileMoveReceipt, CuError> {
        let (mut receipt, _locks) = self.locked_receipt(transaction_id)?;
        if receipt.state != MoveState::Completed {
            return Err(invalid_state(&receipt, "finalize"));
        }
        verify_destination_result(&receipt)?;
        verify_backup(&receipt)?;
        ensure_path_snapshot(
            &receipt.source_backup,
            &receipt.source_snapshot,
            "source backup",
        )?;
        ensure_absent(&receipt.source, "source")?;
        receipt.state = MoveState::Finalizing;
        self.persist(&receipt)?;
        self.finish_finalize(&receipt)?;
        receipt.state = MoveState::Finalized;
        self.persist(&receipt)?;
        Ok(receipt)
    }

    fn recover_reserved(&self, receipt: &mut FileMoveReceipt) -> Result<(), CuError> {
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
            verify_source_for_rollback(receipt)?;
            receipt.state = MoveState::RollingBack;
            self.persist(receipt)?;
            return self.finish_rollback(receipt);
        }
        match &receipt.destination_snapshot {
            None if destination.is_none() && backup.is_none() => {}
            Some(old) => restore_retired_regular(
                &receipt.destination,
                receipt.backup.as_ref().expect("backup"),
                old,
            )?,
            _ => {
                return Err(ambiguous(
                    "destination and backup do not uniquely match the durable receipt",
                ));
            }
        }
        remove_owned_temporary(receipt)?;
        Ok(())
    }

    /// Restores the source from its retired backup (if it was retired) and
    /// returns the destination to its recorded prior state. Every step is
    /// idempotent so an interrupted rollback can be resumed.
    fn finish_rollback(&self, receipt: &FileMoveReceipt) -> Result<(), CuError> {
        restore_retired_regular(
            &receipt.source,
            &receipt.source_backup,
            &receipt.source_snapshot,
        )?;

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

    fn finish_finalize(&self, receipt: &FileMoveReceipt) -> Result<(), CuError> {
        verify_destination_result(receipt)?;
        match optional_snapshot(&receipt.source_backup, "source backup")? {
            Some(snapshot) if snapshot == receipt.source_snapshot => {
                fs::remove_file(&receipt.source_backup)
                    .and_then(|()| sync_parent(receipt.source.parent().unwrap()))
                    .map_err(|error| {
                        failure("file_transaction_finalize_failed", error.to_string())
                    })?;
            }
            None => {}
            Some(_) => return Err(changed("source backup")),
        }
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

    fn persist(&self, receipt: &FileMoveReceipt) -> Result<(), CuError> {
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

    fn open_receipt(&self, transaction_id: &str) -> Result<File, CuError> {
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
        Ok(file)
    }

    fn read(&self, transaction_id: &str) -> Result<FileMoveReceipt, CuError> {
        let file = self.open_receipt(transaction_id)?;
        let receipt: FileMoveReceipt = serde_json::from_reader(file)
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

    /// Takes the cross-process locks for both exact paths in one stable order.
    /// The lock names share the copy transaction's destination namespace, so a
    /// move source and a concurrent copy destination on one path exclude each
    /// other.
    fn path_locks(&self, source: &Path, destination: &Path) -> Result<Vec<PathLock>, CuError> {
        let mut digests = [path_digest(source), path_digest(destination)];
        digests.sort();
        digests
            .iter()
            .map(|digest| {
                PathLock::try_acquire(&self.directory.join(format!("destination-{digest}.lock")))
                    .map_err(|error| failure("file_transaction_busy", error.to_string()))
            })
            .collect()
    }

    fn locked_receipt(
        &self,
        transaction_id: &str,
    ) -> Result<(FileMoveReceipt, Vec<PathLock>), CuError> {
        let before = self.read(transaction_id)?;
        let locks = self.path_locks(&before.source, &before.destination)?;
        let after = self.read(transaction_id)?;
        if before.source != after.source || before.destination != after.destination {
            return Err(corrupt(
                "transaction paths changed while acquiring their locks",
            ));
        }
        Ok((after, locks))
    }
}

fn verify_destination_result(receipt: &FileMoveReceipt) -> Result<(), CuError> {
    ensure_path_snapshot(
        &receipt.destination,
        receipt
            .result_snapshot
            .as_ref()
            .ok_or_else(|| corrupt("missing result snapshot"))?,
        "destination",
    )
}

/// The source must be exactly where the receipt state allows: still at its
/// path (not yet retired) or retired into its backup, never both or neither.
fn verify_source_for_rollback(receipt: &FileMoveReceipt) -> Result<(), CuError> {
    let source = optional_snapshot(&receipt.source, "source")?;
    let retired = optional_snapshot(&receipt.source_backup, "source backup")?;
    match (source, retired) {
        (Some(current), None) if current == receipt.source_snapshot => Ok(()),
        (None, Some(saved)) if saved == receipt.source_snapshot => Ok(()),
        (Some(current), Some(saved)) if current == receipt.source_snapshot && current == saved => {
            Ok(())
        }
        _ => Err(changed("source")),
    }
}

fn verify_backup(receipt: &FileMoveReceipt) -> Result<(), CuError> {
    match (&receipt.backup, &receipt.destination_snapshot) {
        (None, None) => Ok(()),
        (Some(path), Some(snapshot)) => ensure_path_snapshot(path, snapshot, "backup"),
        _ => Err(corrupt("backup fields disagree")),
    }
}

fn ensure_absent(path: &Path, label: &str) -> Result<(), CuError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(ambiguous(format!(
            "{label} path is occupied although the receipt recorded it as retired"
        ))),
        Err(error) => Err(failure(
            "file_transaction_inspect_failed",
            error.to_string(),
        )),
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

fn marker_staging_path(receipt: &FileMoveReceipt) -> PathBuf {
    receipt
        .destination
        .parent()
        .expect("normalized destination")
        .join(format!(".agenterm-move-{}.marker", receipt.transaction_id))
}

/// Exclusively creates the marker beside the destination, makes it durable,
/// and returns the read+write handle that the rest of `apply` keeps using.
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

fn remove_owned_temporary(receipt: &FileMoveReceipt) -> Result<(), CuError> {
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

fn remove_marker_owned(receipt: &FileMoveReceipt, path: &Path, label: &str) -> Result<(), CuError> {
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

fn path_digest(path: &Path) -> String {
    let mut hash = Sha256::new();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        hash.update(path.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        for unit in path.as_os_str().encode_wide() {
            hash.update(unit.to_le_bytes());
        }
    }
    let digest = hash.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_plan(plan: &FileMovePlan) -> Result<(), CuError> {
    if plan.schema_version != SCHEMA_VERSION || plan.operation != OPERATION {
        return Err(corrupt("unsupported plan schema"));
    }
    Ok(())
}

fn validate_receipt(receipt: &FileMoveReceipt) -> Result<(), CuError> {
    if receipt.schema_version != SCHEMA_VERSION || receipt.operation != OPERATION {
        return Err(corrupt("unsupported receipt schema"));
    }
    validate_id(&receipt.transaction_id)?;
    let parent = receipt
        .destination
        .parent()
        .ok_or_else(|| corrupt("destination has no parent"))?;
    let source_parent = receipt
        .source
        .parent()
        .ok_or_else(|| corrupt("source has no parent"))?;
    let id = &receipt.transaction_id;
    if receipt.temporary != parent.join(format!(".agenterm-move-{id}.tmp")) {
        return Err(corrupt(
            "temporary path is not derived from the transaction id",
        ));
    }
    if receipt.source_backup != source_parent.join(format!(".agenterm-move-{id}.source")) {
        return Err(corrupt(
            "source backup path is not derived from the transaction id",
        ));
    }
    match (&receipt.backup, &receipt.destination_snapshot) {
        (None, None) => {}
        (Some(backup), Some(_))
            if backup == &parent.join(format!(".agenterm-move-{id}.backup")) => {}
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

fn invalid_state(receipt: &FileMoveReceipt, action: &str) -> CuError {
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

    fn fixture(label: &str) -> (PathBuf, FileMoveStore) {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap_or_else(|_| std::env::temp_dir())
            .join(format!(
                "agenterm-cu-file-move-{label}-{}",
                std::process::id()
            ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store = FileMoveStore::open_at(root.join("state")).unwrap();
        (root, store)
    }

    fn interrupted_apply(
        store: &FileMoveStore,
        source: &Path,
        destination: &Path,
        replace: bool,
        interrupt: Interruptions,
    ) -> FileMoveReceipt {
        let plan = store.plan(source, destination, replace).unwrap();
        let error = store.apply_with(&plan, interrupt).unwrap_err();
        assert_eq!(error.code, "file_transaction_interrupted");
        let id = error.detail.unwrap()["transaction_id"]
            .as_str()
            .unwrap()
            .to_owned();
        store.status(&id).unwrap()
    }

    #[test]
    fn plan_is_mutation_free_and_apply_moves_exact_bytes() {
        let (root, store) = fixture("new-move");
        let other = root.join("elsewhere");
        fs::create_dir_all(&other).unwrap();
        let source = root.join("source");
        let destination = other.join("destination");
        fs::write(&source, b"payload").unwrap();
        let before = snapshot_path(&source, "source").unwrap();
        let plan = store.plan(&source, &destination, false).unwrap();
        assert_eq!(plan.operation, OPERATION);
        assert!(source.exists() && !destination.exists());

        let receipt = store.apply(&plan).unwrap();
        assert_eq!(receipt.state, MoveState::Completed);
        assert!(!source.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"payload");
        assert_eq!(
            receipt.result_snapshot.as_ref().unwrap().sha256,
            before.sha256
        );
        assert_eq!(snapshot_path(&receipt.source_backup, "b").unwrap(), before);
        assert!(!receipt.temporary.exists());
        assert!(receipt.backup.is_none());

        let rolled = store.rollback(&receipt.transaction_id).unwrap();
        assert_eq!(rolled.state, MoveState::RolledBack);
        assert_eq!(snapshot_path(&source, "source").unwrap(), before);
        assert!(!destination.exists());
        assert!(!receipt.source_backup.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_keeps_both_backups_until_finalize() {
        let (root, store) = fixture("replace-finalize");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        assert_eq!(
            store.plan(&source, &destination, false).unwrap_err().code,
            "file_transaction_replace_required"
        );
        let receipt = store
            .apply(&store.plan(&source, &destination, true).unwrap())
            .unwrap();
        let backup = receipt.backup.clone().unwrap();
        assert_eq!(fs::read(&backup).unwrap(), b"old");
        assert!(receipt.source_backup.exists() && !source.exists());

        let finalized = store.finalize(&receipt.transaction_id).unwrap();
        assert_eq!(finalized.state, MoveState::Finalized);
        assert!(!backup.exists() && !receipt.source_backup.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_rollback_restores_both_exact_objects() {
        let (root, store) = fixture("replace-rollback");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        let old_source = snapshot_path(&source, "s").unwrap();
        let old_destination = snapshot_path(&destination, "d").unwrap();
        let receipt = store
            .apply(&store.plan(&source, &destination, true).unwrap())
            .unwrap();
        let rolled = store.rollback(&receipt.transaction_id).unwrap();
        assert_eq!(rolled.state, MoveState::RolledBack);
        assert_eq!(snapshot_path(&source, "s").unwrap(), old_source);
        assert_eq!(snapshot_path(&destination, "d").unwrap(), old_destination);
        assert!(!receipt.backup.unwrap().exists());
        assert!(!receipt.source_backup.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_state_refuses_rollback_and_finalize() {
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
        assert_eq!(fs::read(&destination).unwrap(), b"third party");

        fs::write(&source, b"second").unwrap();
        let receipt = store
            .apply(&store.plan(&source, &destination, true).unwrap())
            .unwrap();
        fs::write(&receipt.source_backup, b"tampered").unwrap();
        assert_eq!(
            store.finalize(&receipt.transaction_id).unwrap_err().code,
            "file_transaction_precondition_changed"
        );
        assert!(receipt.source_backup.exists());
        assert_eq!(
            store.status(&receipt.transaction_id).unwrap().state,
            MoveState::Completed
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn precondition_drift_refuses_apply_before_reservation() {
        let (root, store) = fixture("drift");
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
        assert_eq!(fs::read(&source).unwrap(), b"second");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_path_is_refused() {
        let (root, store) = fixture("same-path");
        let source = root.join("source");
        fs::write(&source, b"x").unwrap();
        assert_eq!(
            store.plan(&source, &source, true).unwrap_err().code,
            "file_transaction_same_path"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_after_install_before_retire_restores_pre_state() {
        let (root, store) = fixture("installed-crash");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"payload").unwrap();
        let before = snapshot_path(&source, "s").unwrap();
        let receipt = interrupted_apply(
            &store,
            &source,
            &destination,
            false,
            Interruptions {
                after_install: true,
                ..Interruptions::default()
            },
        );
        assert_eq!(receipt.state, MoveState::Installed);
        assert!(source.exists() && destination.exists());
        assert!(!receipt.source_backup.exists());

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, MoveState::RolledBack);
        assert!(!destination.exists());
        assert_eq!(snapshot_path(&source, "s").unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_after_retire_before_completed_receipt_restores_source() {
        let (root, store) = fixture("retire-crash");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"payload").unwrap();
        let before = snapshot_path(&source, "s").unwrap();
        let mut receipt = store
            .apply(&store.plan(&source, &destination, false).unwrap())
            .unwrap();
        receipt.state = MoveState::Installed;
        store.persist(&receipt).unwrap();

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, MoveState::RolledBack);
        assert_eq!(snapshot_path(&source, "s").unwrap(), before);
        assert!(!destination.exists() && !receipt.source_backup.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_after_source_backup_link_before_unlink_restores_one_source_name() {
        let (root, store) = fixture("retire-two-names");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"payload").unwrap();
        let before = snapshot_path(&source, "s").unwrap();
        let mut receipt = store
            .apply(&store.plan(&source, &destination, false).unwrap())
            .unwrap();
        fs::hard_link(&receipt.source_backup, &source).unwrap();
        receipt.state = MoveState::Installed;
        store.persist(&receipt).unwrap();

        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, MoveState::RolledBack);
        assert_eq!(snapshot_path(&source, "s").unwrap(), before);
        assert!(!receipt.source_backup.exists());
        assert!(!destination.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_cleans_published_marker_without_identity() {
        let (root, store) = fixture("marker");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"payload").unwrap();
        let receipt = interrupted_apply(
            &store,
            &source,
            &destination,
            false,
            Interruptions {
                after_marker_published: true,
                ..Interruptions::default()
            },
        );
        assert_eq!(receipt.state, MoveState::Reserved);
        assert!(receipt.temporary_identity.is_none());
        assert!(receipt.temporary.exists());
        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, MoveState::RolledBack);
        assert!(!receipt.temporary.exists());
        assert_eq!(fs::read(&source).unwrap(), b"payload");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recover_resumes_interrupted_rollback_and_finalize() {
        let (root, store) = fixture("resume");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        let mut receipt = store
            .apply(&store.plan(&source, &destination, true).unwrap())
            .unwrap();
        receipt.state = MoveState::RollingBack;
        store.persist(&receipt).unwrap();
        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, MoveState::RolledBack);
        assert_eq!(fs::read(&source).unwrap(), b"new");
        assert_eq!(fs::read(&destination).unwrap(), b"old");

        let mut receipt = store
            .apply(&store.plan(&source, &destination, true).unwrap())
            .unwrap();
        receipt.state = MoveState::Finalizing;
        store.persist(&receipt).unwrap();
        fs::remove_file(receipt.backup.as_ref().unwrap()).unwrap();
        let recovered = store.recover(&receipt.transaction_id).unwrap();
        assert_eq!(recovered.state, MoveState::Finalized);
        assert!(!receipt.source_backup.exists() && !source.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"new");
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
    fn staged_handle_is_the_published_object() {
        let (root, _store) = fixture("marker-held");
        let staging = root.join("marker.staged");
        let temporary = root.join("marker.tmp");
        let marker = ownership_marker(&"a".repeat(32)).unwrap();
        let mut held = stage_marker(&staging, &marker).unwrap();
        publish_marker(&staging, &temporary, &root).unwrap();
        assert!(!staging.exists());
        assert!(marker_matches(&mut held, &hex_sha256(&marker)).unwrap());
        let published = snapshot_path(&temporary, "temporary").unwrap();
        assert!(same_object(&published, &identity_of(&held).unwrap()));
        // A swap of the path after publication does not redirect the held handle.
        let intruder = root.join("intruder");
        fs::write(&intruder, b"intruder").unwrap();
        fs::rename(&intruder, &temporary).unwrap();
        held.set_len(0).unwrap();
        held.seek(SeekFrom::Start(0)).unwrap();
        held.write_all(b"owned bytes").unwrap();
        held.sync_all().unwrap();
        assert_eq!(fs::read(&temporary).unwrap(), b"intruder");
        let now = snapshot_path(&temporary, "temporary").unwrap();
        assert!(!same_object(&now, &identity_of(&held).unwrap()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn swapped_temporary_before_publication_is_refused() {
        let (root, store) = fixture("swap-before-publish");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"payload").unwrap();
        let plan = store.plan(&source, &destination, false).unwrap();
        // Occupy the derived temporary name with an unrelated object before the
        // transaction can publish its marker: the hard link must refuse it.
        let receipt_id = {
            let error = store
                .apply_with(
                    &plan,
                    Interruptions {
                        after_marker_published: true,
                        ..Interruptions::default()
                    },
                )
                .unwrap_err();
            error.detail.unwrap()["transaction_id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let receipt = store.status(&receipt_id).unwrap();
        fs::write(&receipt.temporary, b"unrelated").unwrap();
        let error = store.recover(&receipt_id).unwrap_err();
        assert_eq!(error.code, "file_transaction_state_ambiguous");
        assert_eq!(fs::read(&receipt.temporary).unwrap(), b"unrelated");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_lock_shares_the_copy_destination_namespace() {
        let (root, store) = fixture("lock");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"x").unwrap();
        let plan = store.plan(&source, &destination, false).unwrap();
        let digest = path_digest(&source);
        let held =
            PathLock::try_acquire(&store.directory.join(format!("destination-{digest}.lock")))
                .unwrap();
        assert_eq!(
            store.apply(&plan).unwrap_err().code,
            "file_transaction_busy"
        );
        drop(held);
        assert!(store.apply(&plan).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn peek_operation_routes_by_receipt() {
        let (root, store) = fixture("peek");
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"x").unwrap();
        let receipt = store
            .apply(&store.plan(&source, &destination, false).unwrap())
            .unwrap();
        assert_eq!(
            store.peek_operation(&receipt.transaction_id).unwrap(),
            OPERATION
        );
        assert_eq!(
            store.peek_operation(&"0".repeat(32)).unwrap_err().code,
            "file_transaction_not_found"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
