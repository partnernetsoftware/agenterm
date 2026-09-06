//! Crash recovery for the temporary freeze used by `process-signal --tree`.
//!
//! A transaction is durably published after the root's exact scheduler state
//! is captured and before any member is suspended. Every later freeze and
//! release is write-ahead logged. Recovery may reopen a PID only to compare
//! that exact start identity; it never signals a replacement process.

use std::{
    fs,
    path::{Path, PathBuf},
};

use agenterm_platform::{
    filesystem::{protect_private_directory, sync_parent, write_private_atomic},
    filesystem_open::{ExistingEntryType, open_existing_path},
    locking::PathLock,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    CuError,
    command::ProcessSignalKind,
    receipt::{ReceiptLog, ReceiptTicket},
};

const SCHEMA_VERSION: u32 = 1;
const MAX_TRANSACTION_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TRANSACTIONS: usize = 1_024;

#[derive(Clone, Debug)]
pub(super) struct RecoveryMemberInput<'a> {
    pub pid: u32,
    pub depth: usize,
    pub start_identity: &'a str,
    pub was_stopped: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TransactionPhase {
    Stabilizing,
    Frozen,
    Delivering,
    Recovering,
    RecoveryBlocked,
    EffectTerminal,
    RecoveryTerminal,
    ReceiptClosed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TerminalKind {
    EffectCompleted,
    RecoveryCompleted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeliveryPhase {
    Pending,
    Started,
    Delivered,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum FreezePhase {
    Captured,
    PreservedStopped,
    FreezeIntent,
    FrozenByUs,
    ReleaseIntent,
    Released,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DurableMember {
    pid: u32,
    depth: usize,
    start_identity: String,
    was_stopped: bool,
    freeze: FreezePhase,
    #[serde(default)]
    in_final_tree: bool,
    delivery: DeliveryPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    schema_version: u32,
    transaction_id: String,
    receipt_id: String,
    root_pid: u32,
    root_start_identity: String,
    signal: ProcessSignalKind,
    phase: TransactionPhase,
    members: Vec<DurableMember>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_kind: Option<TerminalKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_effect_verified: Option<bool>,
    #[serde(default)]
    effect_outcome_unknown: bool,
}

pub(super) struct RecoveryStore {
    directory: PathBuf,
    _lock: PathLock,
}

impl RecoveryStore {
    pub(super) fn open_beside_receipt(receipt_path: &Path) -> Result<Self, CuError> {
        let receipt_dir = receipt_path
            .parent()
            .ok_or_else(|| state_error("receipt path has no parent"))?;
        let directory = receipt_dir.join("process-signal-transactions");
        fs::create_dir_all(&directory)
            .and_then(|()| protect_private_directory(&directory))
            .map_err(|error| state_error(error.to_string()))?;
        let lock = PathLock::try_acquire(&directory.join("owner.lock"))
            .map_err(|error| CuError::new("process_tree_recovery_busy", error.to_string()))?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    /// Recover every interrupted transaction before allowing another tree
    /// effect. A corrupt transaction prevents mutation; a torn public receipt
    /// permits exact compensation but prevents the next requested effect.
    pub(super) fn recover_pending(&self, receipts: &mut ReceiptLog) -> Result<Vec<Value>, CuError> {
        let mut transactions = self.read_all()?;
        let mut outcomes = Vec::new();
        for transaction in &mut transactions {
            match transaction.phase {
                TransactionPhase::ReceiptClosed => {
                    self.remove_terminal(&transaction.transaction_id)?;
                    continue;
                }
                TransactionPhase::EffectTerminal | TransactionPhase::RecoveryTerminal => {
                    let outcome = terminal_outcome(transaction)?;
                    self.close_terminal_receipt(transaction, receipts)?;
                    outcomes.push(outcome);
                    continue;
                }
                TransactionPhase::Stabilizing
                | TransactionPhase::Frozen
                | TransactionPhase::Delivering
                | TransactionPhase::Recovering
                | TransactionPhase::RecoveryBlocked => {}
            }
            transaction.phase = TransactionPhase::Recovering;
            self.persist(transaction)?;
            let mut blocked = false;
            let mut rows = Vec::with_capacity(transaction.members.len());
            for index in (0..transaction.members.len()).rev() {
                let (row, effect_unknown) = {
                    let member = &mut transaction.members[index];
                    let result = recover_member(
                        member.pid,
                        &member.start_identity,
                        member.was_stopped,
                        member.freeze,
                    );
                    match result {
                        Ok((outcome, effect_unknown)) => {
                            member.recovery = Some(outcome.to_owned());
                            if !member.was_stopped {
                                member.freeze = FreezePhase::Released;
                            }
                            (
                                json!({
                                    "pid": member.pid,
                                    "depth": member.depth,
                                    "start_identity": member.start_identity,
                                    "outcome": outcome,
                                    "cleanup_verified": true,
                                    "effect_outcome_unknown": effect_unknown,
                                    "freeze_ownership_ambiguous": outcome == "freeze_intent_resumed",
                                }),
                                effect_unknown,
                            )
                        }
                        Err(error) => {
                            blocked = true;
                            member.recovery = Some(error.code.clone());
                            (
                                json!({
                                    "pid": member.pid,
                                    "depth": member.depth,
                                    "start_identity": member.start_identity,
                                    "outcome": "refused",
                                    "cleanup_verified": false,
                                    "error": { "code": error.code, "message": error.message },
                                }),
                                false,
                            )
                        }
                    }
                };
                transaction.effect_outcome_unknown |= effect_unknown;
                rows.push(row);
                self.persist(transaction)?;
            }
            transaction.effect_outcome_unknown |= transaction
                .members
                .iter()
                .any(|member| member.delivery != DeliveryPhase::Pending);
            let outcome = recovery_outcome(transaction, !blocked, rows);
            outcomes.push(outcome.clone());
            transaction.phase = if blocked {
                TransactionPhase::RecoveryBlocked
            } else {
                TransactionPhase::RecoveryTerminal
            };
            transaction.terminal_kind = (!blocked).then_some(TerminalKind::RecoveryCompleted);
            self.persist(transaction)?;
            if blocked {
                return Err(CuError::new(
                    "process_tree_recovery_blocked",
                    "an exact live tree member could not be restored; no new signal was attempted",
                )
                .with_detail(json!({ "recoveries": outcomes })));
            }
            self.close_terminal_receipt(transaction, receipts)?;
        }
        // A torn public receipt always blocks the next effect, including when
        // there was no transaction to repair.
        receipts.list(None, usize::MAX)?;
        Ok(outcomes)
    }

    pub(super) fn begin(
        &self,
        receipt_id: &str,
        root_pid: u32,
        root_start_identity: &str,
        signal: ProcessSignalKind,
        members: &[RecoveryMemberInput<'_>],
    ) -> Result<String, CuError> {
        let id = random_id()?;
        let transaction = Transaction {
            schema_version: SCHEMA_VERSION,
            transaction_id: id.clone(),
            receipt_id: receipt_id.to_owned(),
            root_pid,
            root_start_identity: root_start_identity.to_owned(),
            signal,
            phase: TransactionPhase::Stabilizing,
            members: members
                .iter()
                .map(|member| DurableMember {
                    pid: member.pid,
                    depth: member.depth,
                    start_identity: member.start_identity.to_owned(),
                    was_stopped: member.was_stopped,
                    freeze: if member.was_stopped {
                        FreezePhase::PreservedStopped
                    } else {
                        FreezePhase::Captured
                    },
                    in_final_tree: false,
                    delivery: DeliveryPhase::Pending,
                    recovery: None,
                })
                .collect(),
            terminal_kind: None,
            terminal_elapsed_ms: None,
            terminal_effect_verified: None,
            effect_outcome_unknown: false,
        };
        self.persist(&transaction)?;
        Ok(id)
    }

    pub(super) fn register_members(
        &self,
        id: &str,
        members: &[RecoveryMemberInput<'_>],
    ) -> Result<(), CuError> {
        self.update(id, |transaction| {
            if transaction.phase != TransactionPhase::Stabilizing {
                return Err(corrupt("members may be registered only while stabilizing"));
            }
            for input in members {
                if let Some(existing) = transaction
                    .members
                    .iter_mut()
                    .find(|member| member.pid == input.pid)
                {
                    if existing.start_identity != input.start_identity
                        || existing.was_stopped != input.was_stopped
                    {
                        return Err(corrupt("registered member identity changed"));
                    }
                    existing.depth = input.depth;
                } else {
                    transaction.members.push(DurableMember {
                        pid: input.pid,
                        depth: input.depth,
                        start_identity: input.start_identity.to_owned(),
                        was_stopped: input.was_stopped,
                        freeze: if input.was_stopped {
                            FreezePhase::PreservedStopped
                        } else {
                            FreezePhase::Captured
                        },
                        in_final_tree: false,
                        delivery: DeliveryPhase::Pending,
                        recovery: None,
                    });
                }
            }
            Ok(())
        })
    }

    pub(super) fn before_freeze(&self, id: &str, pid: u32) -> Result<(), CuError> {
        self.set_freeze_phase(id, pid, FreezePhase::Captured, FreezePhase::FreezeIntent)
    }

    pub(super) fn after_freeze(&self, id: &str, pid: u32) -> Result<(), CuError> {
        self.set_freeze_phase(id, pid, FreezePhase::FreezeIntent, FreezePhase::FrozenByUs)
    }

    pub(super) fn before_release(&self, id: &str, pid: u32) -> Result<(), CuError> {
        self.set_freeze_phase(id, pid, FreezePhase::FrozenByUs, FreezePhase::ReleaseIntent)
    }

    pub(super) fn after_release(&self, id: &str, pid: u32) -> Result<(), CuError> {
        self.set_freeze_phase(id, pid, FreezePhase::ReleaseIntent, FreezePhase::Released)
    }

    pub(super) fn released_after_exit(&self, id: &str, pid: u32) -> Result<(), CuError> {
        self.update(id, |transaction| {
            let member = transaction
                .members
                .iter_mut()
                .find(|member| member.pid == pid)
                .ok_or_else(|| corrupt("released member is absent from transaction"))?;
            if !matches!(
                member.freeze,
                FreezePhase::FrozenByUs | FreezePhase::ReleaseIntent
            ) {
                return Err(corrupt("exited member has an invalid release phase"));
            }
            member.freeze = FreezePhase::Released;
            Ok(())
        })
    }

    pub(super) fn released_without_freeze(&self, id: &str, pid: u32) -> Result<(), CuError> {
        self.set_freeze_phase(id, pid, FreezePhase::Captured, FreezePhase::Released)
    }

    fn set_freeze_phase(
        &self,
        id: &str,
        pid: u32,
        expected: FreezePhase,
        next: FreezePhase,
    ) -> Result<(), CuError> {
        self.update(id, |transaction| {
            if transaction.phase != TransactionPhase::Stabilizing {
                return Err(corrupt("freeze state changed outside stabilization"));
            }
            let member = transaction
                .members
                .iter_mut()
                .find(|member| member.pid == pid)
                .ok_or_else(|| corrupt("freeze member is absent from transaction"))?;
            if member.was_stopped || member.freeze != expected {
                return Err(corrupt("freeze member phase is invalid"));
            }
            member.freeze = next;
            Ok(())
        })
    }

    pub(super) fn mark_stable(&self, id: &str, final_ids: &[u32]) -> Result<(), CuError> {
        let final_ids = final_ids
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        self.update(id, |transaction| {
            if transaction.phase != TransactionPhase::Stabilizing {
                return Err(corrupt("tree became stable from an invalid phase"));
            }
            for member in &mut transaction.members {
                member.in_final_tree = final_ids.contains(&member.pid);
                if member.in_final_tree
                    && !matches!(
                        member.freeze,
                        FreezePhase::FrozenByUs
                            | FreezePhase::PreservedStopped
                            | FreezePhase::Captured
                    )
                {
                    return Err(corrupt("final member has an invalid freeze phase"));
                }
                if !member.in_final_tree
                    && !matches!(
                        member.freeze,
                        FreezePhase::Released | FreezePhase::PreservedStopped
                    )
                {
                    return Err(corrupt("removed member was not durably released"));
                }
            }
            if !transaction
                .members
                .iter()
                .any(|member| member.in_final_tree)
            {
                return Err(corrupt("stable tree has no final members"));
            }
            transaction.phase = TransactionPhase::Frozen;
            Ok(())
        })
    }

    pub(super) fn before_delivery(&self, id: &str, pid: u32) -> Result<(), CuError> {
        self.update(id, |transaction| {
            transaction.phase = TransactionPhase::Delivering;
            let member = transaction
                .members
                .iter_mut()
                .find(|member| member.pid == pid)
                .ok_or_else(|| corrupt("delivery member is absent from transaction"))?;
            if !member.in_final_tree || member.delivery != DeliveryPhase::Pending {
                return Err(corrupt("tree signal delivery would be repeated"));
            }
            member.delivery = DeliveryPhase::Started;
            Ok(())
        })
    }

    pub(super) fn after_delivery(&self, id: &str, pid: u32) -> Result<(), CuError> {
        self.update(id, |transaction| {
            let member = transaction
                .members
                .iter_mut()
                .find(|member| member.pid == pid)
                .ok_or_else(|| corrupt("delivery member is absent from transaction"))?;
            if member.delivery != DeliveryPhase::Started {
                return Err(corrupt("tree signal delivery phase is invalid"));
            }
            member.delivery = DeliveryPhase::Delivered;
            Ok(())
        })
    }

    pub(super) fn finish_effect(
        &self,
        id: &str,
        elapsed_ms: u64,
        effect_verified: Option<bool>,
        receipts: &mut ReceiptLog,
    ) -> Result<(), CuError> {
        self.update(id, |transaction| {
            transaction.phase = TransactionPhase::EffectTerminal;
            transaction.terminal_kind = Some(TerminalKind::EffectCompleted);
            transaction.terminal_elapsed_ms = Some(elapsed_ms);
            transaction.terminal_effect_verified = effect_verified;
            Ok(())
        })?;
        let mut transaction = self.read(id)?;
        self.close_terminal_receipt(&mut transaction, receipts)
    }

    pub(super) fn finish_recovery(
        &self,
        id: &str,
        effect_outcome_unknown: bool,
        receipts: &mut ReceiptLog,
    ) -> Result<(), CuError> {
        self.update(id, |transaction| {
            transaction.phase = TransactionPhase::RecoveryTerminal;
            transaction.terminal_kind = Some(TerminalKind::RecoveryCompleted);
            transaction.effect_outcome_unknown |= effect_outcome_unknown
                || transaction
                    .members
                    .iter()
                    .any(|member| member.delivery != DeliveryPhase::Pending);
            Ok(())
        })?;
        let mut transaction = self.read(id)?;
        self.close_terminal_receipt(&mut transaction, receipts)
    }

    fn update(
        &self,
        id: &str,
        mutate: impl FnOnce(&mut Transaction) -> Result<(), CuError>,
    ) -> Result<(), CuError> {
        let mut transaction = self.read(id)?;
        mutate(&mut transaction)?;
        self.persist(&transaction)
    }

    fn read_all(&self) -> Result<Vec<Transaction>, CuError> {
        let mut paths = Vec::new();
        for entry in
            fs::read_dir(&self.directory).map_err(|error| state_error(error.to_string()))?
        {
            let path = entry
                .map_err(|error| state_error(format!("could not enumerate transaction: {error}")))?
                .path();
            if path.extension().and_then(|value| value.to_str()) == Some("json") {
                paths.push(path);
            }
        }
        paths.sort();
        if paths.len() > MAX_TRANSACTIONS {
            return Err(corrupt("too many process signal transactions"));
        }
        paths
            .into_iter()
            .map(|path| self.read_path(&path))
            .collect()
    }

    fn read(&self, id: &str) -> Result<Transaction, CuError> {
        validate_id(id)?;
        self.read_path(&self.directory.join(format!("{id}.json")))
    }

    fn read_path(&self, path: &Path) -> Result<Transaction, CuError> {
        let file = open_existing_path(path, ExistingEntryType::File)
            .map_err(|error| corrupt(format!("could not open transaction: {error}")))?;
        if file
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(MAX_TRANSACTION_BYTES + 1)
            > MAX_TRANSACTION_BYTES
        {
            return Err(corrupt("transaction exceeds its byte bound"));
        }
        let transaction: Transaction = serde_json::from_reader(file)
            .map_err(|error| corrupt(format!("invalid or torn transaction: {error}")))?;
        validate_transaction(&transaction)?;
        if path.file_stem().and_then(|value| value.to_str()) != Some(&transaction.transaction_id) {
            return Err(corrupt("transaction filename does not match its identity"));
        }
        Ok(transaction)
    }

    fn persist(&self, transaction: &Transaction) -> Result<(), CuError> {
        validate_transaction(transaction)?;
        let mut bytes =
            serde_json::to_vec(transaction).map_err(|error| state_error(error.to_string()))?;
        if bytes.len() as u64 > MAX_TRANSACTION_BYTES {
            return Err(corrupt("transaction exceeds its byte bound"));
        }
        bytes.push(b'\n');
        write_private_atomic(
            &self
                .directory
                .join(format!("{}.json", transaction.transaction_id)),
            &bytes,
        )
        .map_err(|error| state_error(error.to_string()))
    }

    fn remove_terminal(&self, id: &str) -> Result<(), CuError> {
        validate_id(id)?;
        fs::remove_file(self.directory.join(format!("{id}.json")))
            .and_then(|()| sync_parent(&self.directory))
            .map_err(|error| state_error(format!("could not retire transaction: {error}")))
    }

    fn close_terminal_receipt(
        &self,
        transaction: &mut Transaction,
        receipts: &mut ReceiptLog,
    ) -> Result<(), CuError> {
        let lines = receipts.list(None, usize::MAX)?.0;
        let kind = transaction
            .terminal_kind
            .ok_or_else(|| corrupt("terminal transaction has no terminal kind"))?;
        let expected_terminal_phase = match kind {
            TerminalKind::EffectCompleted => "completed",
            TerminalKind::RecoveryCompleted => "failed",
        };
        let mut reserved_count = 0_usize;
        let mut existing_reservation = None;
        let mut existing_terminal = None;
        for line in lines.iter().filter(|line| {
            line.get("receipt_id").and_then(Value::as_str) == Some(&transaction.receipt_id)
        }) {
            if line.get("verb").and_then(Value::as_str) != Some("process-signal-tree") {
                return Err(corrupt("public receipt verb does not match transaction"));
            }
            match line.get("phase").and_then(Value::as_str) {
                Some("reserved") => {
                    reserved_count += 1;
                    existing_reservation = Some(line);
                }
                Some("completed" | "failed") if existing_terminal.is_none() => {
                    existing_terminal = Some(line)
                }
                Some("completed" | "failed") => {
                    return Err(corrupt("public receipt has duplicate terminal records"));
                }
                _ => return Err(corrupt("public receipt has an invalid phase")),
            }
        }
        if reserved_count != 1 {
            return Err(corrupt(
                "public receipt must contain exactly one matching reservation",
            ));
        }
        let reservation = existing_reservation.expect("one reservation counted");
        if reservation.pointer("/root/pid").and_then(Value::as_u64)
            != Some(u64::from(transaction.root_pid))
            || reservation
                .pointer("/root/start_identity")
                .and_then(Value::as_str)
                != Some(&transaction.root_start_identity)
            || reservation.get("signal").and_then(Value::as_str)
                != Some(transaction.signal.as_str())
        {
            return Err(corrupt(
                "public receipt reservation does not match transaction",
            ));
        }
        if let Some(line) = existing_terminal {
            if line.get("phase").and_then(Value::as_str) != Some(expected_terminal_phase) {
                return Err(corrupt(
                    "public receipt terminal phase does not match transaction",
                ));
            }
            validate_terminal_receipt(line, transaction, kind)?;
        } else {
            let (verified, body) = match kind {
                TerminalKind::EffectCompleted => (
                    true,
                    json!({
                        "performed": true,
                        "verified": transaction.terminal_effect_verified,
                        "member_count": final_member_count(transaction),
                        "elapsed_ms": transaction.terminal_elapsed_ms,
                    }),
                ),
                TerminalKind::RecoveryCompleted => (
                    false,
                    json!({
                        "performed": transaction.members.iter().any(|member| member.delivery != DeliveryPhase::Pending),
                        "verified": false,
                        "recovery_verified": true,
                        "cleanup_verified": true,
                        "effect_outcome_unknown": transaction.effect_outcome_unknown,
                        "recovery": recovery_outcome(transaction, true, recovery_rows(transaction)),
                    }),
                ),
            };
            receipts.complete(
                &ReceiptTicket {
                    id: transaction.receipt_id.clone(),
                    path: receipts.path().to_owned(),
                },
                "process-signal-tree",
                0,
                verified,
                body,
            )?;
        }
        transaction.phase = TransactionPhase::ReceiptClosed;
        self.persist(transaction)?;
        self.remove_terminal(&transaction.transaction_id)
    }
}

fn validate_terminal_receipt(
    line: &Value,
    transaction: &Transaction,
    kind: TerminalKind,
) -> Result<(), CuError> {
    let valid = match kind {
        TerminalKind::EffectCompleted => {
            line.get("verified").and_then(Value::as_bool) == Some(true)
                && line.get("performed").and_then(Value::as_bool) == Some(true)
                && line.get("member_count").and_then(Value::as_u64)
                    == Some(final_member_count(transaction) as u64)
                && line.get("elapsed_ms").and_then(Value::as_u64) == transaction.terminal_elapsed_ms
        }
        TerminalKind::RecoveryCompleted => {
            line.get("verified").and_then(Value::as_bool) == Some(false)
                && line.get("recovery_verified").and_then(Value::as_bool) == Some(true)
                && line.get("cleanup_verified").and_then(Value::as_bool) == Some(true)
                && line.get("effect_outcome_unknown").and_then(Value::as_bool)
                    == Some(transaction.effect_outcome_unknown)
                && line
                    .pointer("/recovery/transaction_id")
                    .and_then(Value::as_str)
                    == Some(&transaction.transaction_id)
        }
    };
    if valid {
        Ok(())
    } else {
        Err(corrupt(
            "public receipt terminal evidence does not match transaction",
        ))
    }
}

enum RecoveryDisposition {
    Exited,
    Replaced,
}

fn recover_member(
    pid: u32,
    expected_identity: &str,
    was_stopped: bool,
    freeze: FreezePhase,
) -> Result<(&'static str, bool), CuError> {
    match agenterm_platform::process_observation::observe(pid) {
        agenterm_platform::process_observation::ProcessObservation::Dead { .. } => {
            return Ok(("exited", true));
        }
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: Some(identity),
        } if identity != expected_identity => return Ok(("replaced", true)),
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: Some(_),
        } => {}
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: None,
        }
        | agenterm_platform::process_observation::ProcessObservation::Unknown { .. } => {
            return Err(CuError::new(
                "process_tree_recovery_identity_unavailable",
                format!("pid {pid} may still be live but has no stable identity"),
            ));
        }
        _ => {
            return Err(CuError::new(
                "process_tree_recovery_identity_unavailable",
                "unsupported process observation result",
            ));
        }
    }
    let reference =
        match agenterm_platform::process_reference::ProcessReference::open_for_termination(pid) {
            Ok(reference) => reference,
            Err(error) => match agenterm_platform::process_observation::observe(pid) {
                agenterm_platform::process_observation::ProcessObservation::Dead { .. } => {
                    return Ok(("exited", true));
                }
                agenterm_platform::process_observation::ProcessObservation::Live {
                    start_identity: Some(identity),
                } if identity != expected_identity => return Ok(("replaced", true)),
                _ => {
                    return Err(CuError::new(
                        "process_tree_recovery_reference_failed",
                        error.to_string(),
                    ));
                }
            },
        };
    if !reference.is_alive().map_err(|error| {
        CuError::new("process_tree_recovery_reference_failed", error.to_string())
    })? {
        return Ok(("exited", true));
    }
    if let Some(disposition) = observed_exact_identity(pid, expected_identity)? {
        return Ok(match disposition {
            RecoveryDisposition::Exited => ("exited", true),
            RecoveryDisposition::Replaced => ("replaced", true),
        });
    }
    if was_stopped
        || matches!(
            freeze,
            FreezePhase::Captured | FreezePhase::PreservedStopped | FreezePhase::Released
        )
    {
        return Ok((
            if freeze == FreezePhase::Released {
                "released"
            } else {
                "preserved"
            },
            false,
        ));
    }
    let stopped = agenterm_platform::process_metrics::is_stopped(pid)
        .map_err(|error| CuError::new("process_tree_recovery_verify_failed", error.to_string()))?;
    if let Some(disposition) = observed_exact_identity(pid, expected_identity)? {
        return Ok(match disposition {
            RecoveryDisposition::Exited => ("exited", true),
            RecoveryDisposition::Replaced => ("replaced", true),
        });
    }
    if !stopped {
        return Ok((
            if freeze == FreezePhase::FreezeIntent {
                "freeze_intent_not_applied"
            } else {
                "already_released"
            },
            false,
        ));
    }
    reference
        .set_suspended(false)
        .map_err(|error| CuError::new("process_tree_recovery_resume_failed", error.to_string()))?;
    if let Some(disposition) = observed_exact_identity(pid, expected_identity)? {
        return Ok(match disposition {
            RecoveryDisposition::Exited => ("exited", true),
            RecoveryDisposition::Replaced => ("replaced", true),
        });
    }
    if agenterm_platform::process_metrics::is_stopped(pid)
        .map_err(|error| CuError::new("process_tree_recovery_verify_failed", error.to_string()))?
    {
        return Err(CuError::new(
            "process_tree_recovery_verify_failed",
            format!("pid {pid} remained stopped"),
        ));
    }
    if let Some(disposition) = observed_exact_identity(pid, expected_identity)? {
        return Ok(match disposition {
            RecoveryDisposition::Exited => ("exited", true),
            RecoveryDisposition::Replaced => ("replaced", true),
        });
    }
    Ok((
        if freeze == FreezePhase::FreezeIntent {
            "freeze_intent_resumed"
        } else if freeze == FreezePhase::ReleaseIntent {
            "release_intent_resumed"
        } else {
            "resumed"
        },
        freeze == FreezePhase::FreezeIntent,
    ))
}

fn observed_exact_identity(
    pid: u32,
    expected_identity: &str,
) -> Result<Option<RecoveryDisposition>, CuError> {
    match agenterm_platform::process_observation::observe(pid) {
        agenterm_platform::process_observation::ProcessObservation::Dead { .. } => {
            Ok(Some(RecoveryDisposition::Exited))
        }
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: Some(identity),
        } if identity != expected_identity => Ok(Some(RecoveryDisposition::Replaced)),
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: Some(_),
        } => Ok(None),
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: None,
        }
        | agenterm_platform::process_observation::ProcessObservation::Unknown { .. } => {
            Err(CuError::new(
                "process_tree_recovery_identity_unavailable",
                format!("pid {pid} may still be live but has no stable identity"),
            ))
        }
        _ => Err(CuError::new(
            "process_tree_recovery_identity_unavailable",
            "unsupported process observation result",
        )),
    }
}

fn recovery_rows(transaction: &Transaction) -> Vec<Value> {
    transaction
        .members
        .iter()
        .rev()
        .map(|member| {
            let outcome = member.recovery.as_deref().unwrap_or("preserved");
            json!({
                "pid": member.pid,
                "depth": member.depth,
                "start_identity": member.start_identity,
                "outcome": outcome,
                "cleanup_verified": !outcome.starts_with("process_tree_recovery_"),
                "effect_outcome_unknown": matches!(outcome, "exited" | "replaced" | "freeze_intent_resumed"),
                "freeze_ownership_ambiguous": outcome == "freeze_intent_resumed",
            })
        })
        .collect()
}

fn final_member_count(transaction: &Transaction) -> usize {
    transaction
        .members
        .iter()
        .filter(|member| member.in_final_tree)
        .count()
}

fn recovery_outcome(
    transaction: &Transaction,
    cleanup_verified: bool,
    members: Vec<Value>,
) -> Value {
    json!({
        "transaction_id": transaction.transaction_id,
        "receipt_id": transaction.receipt_id,
        "root_pid": transaction.root_pid,
        "root_start_identity": transaction.root_start_identity,
        "signal": transaction.signal.as_str(),
        "outcome": if cleanup_verified { "recovered" } else { "blocked" },
        "cleanup_verified": cleanup_verified,
        "effect_outcome_unknown": transaction.effect_outcome_unknown,
        "freeze_ownership_ambiguous": transaction.members.iter().any(|member| member.recovery.as_deref() == Some("freeze_intent_resumed")),
        "members": members,
    })
}

fn terminal_outcome(transaction: &Transaction) -> Result<Value, CuError> {
    match transaction.terminal_kind {
        Some(TerminalKind::EffectCompleted) => Ok(json!({
            "transaction_id": transaction.transaction_id,
            "receipt_id": transaction.receipt_id,
            "root_pid": transaction.root_pid,
            "root_start_identity": transaction.root_start_identity,
            "signal": transaction.signal.as_str(),
            "outcome": "effect_completed_receipt_repaired",
            "cleanup_verified": true,
            "effect_outcome_unknown": false,
            "members": [],
        })),
        Some(TerminalKind::RecoveryCompleted) => Ok(recovery_outcome(
            transaction,
            true,
            recovery_rows(transaction),
        )),
        None => Err(corrupt("terminal transaction has no terminal kind")),
    }
}

#[cfg(all(test, not(windows)))]
fn live_identity(pid: u32) -> Result<String, CuError> {
    match agenterm_platform::process_observation::observe(pid) {
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: Some(start_identity),
        } => Ok(start_identity),
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: None,
        } => Err(CuError::new(
            "process_tree_recovery_identity_unavailable",
            "live process has no stable start identity",
        )),
        agenterm_platform::process_observation::ProcessObservation::Dead { reason } => {
            Err(CuError::new("process_tree_recovery_member_exited", reason))
        }
        agenterm_platform::process_observation::ProcessObservation::Unknown { reason } => Err(
            CuError::new("process_tree_recovery_identity_unavailable", reason),
        ),
        _ => Err(CuError::new(
            "process_tree_recovery_identity_unavailable",
            "unsupported process observation result",
        )),
    }
}

fn validate_transaction(transaction: &Transaction) -> Result<(), CuError> {
    validate_id(&transaction.transaction_id)?;
    validate_receipt_id(&transaction.receipt_id)?;
    if transaction.schema_version != SCHEMA_VERSION
        || transaction.root_pid <= 1
        || !valid_identity(&transaction.root_start_identity)
        || transaction.members.is_empty()
        || transaction.members.len() > 10_001
        || transaction.members[0].pid != transaction.root_pid
        || transaction.members[0].depth != 0
        || transaction.members[0].start_identity != transaction.root_start_identity
    {
        return Err(corrupt("transaction invariants are invalid"));
    }
    let mut pids = std::collections::BTreeSet::new();
    for member in &transaction.members {
        if member.pid <= 1
            || member.depth > 10_000
            || !valid_identity(&member.start_identity)
            || !pids.insert(member.pid)
            || (member.was_stopped != (member.freeze == FreezePhase::PreservedStopped))
            || (!member.in_final_tree && member.delivery != DeliveryPhase::Pending)
            || member.recovery.as_deref().is_some_and(|value| {
                !matches!(
                    value,
                    "preserved"
                        | "resumed"
                        | "released"
                        | "already_released"
                        | "freeze_intent_not_applied"
                        | "freeze_intent_resumed"
                        | "release_intent_resumed"
                        | "exited"
                        | "replaced"
                        | "process_tree_recovery_identity_unavailable"
                        | "process_tree_recovery_reference_failed"
                        | "process_tree_recovery_resume_failed"
                        | "process_tree_recovery_verify_failed"
                )
            })
        {
            return Err(corrupt("transaction member identity is invalid"));
        }
    }
    let effect_state = matches!(
        transaction.phase,
        TransactionPhase::Frozen | TransactionPhase::Delivering | TransactionPhase::EffectTerminal
    ) || (transaction.phase == TransactionPhase::ReceiptClosed
        && transaction.terminal_kind == Some(TerminalKind::EffectCompleted));
    let recovery_terminal = transaction.phase == TransactionPhase::RecoveryTerminal
        || (transaction.phase == TransactionPhase::ReceiptClosed
            && transaction.terminal_kind == Some(TerminalKind::RecoveryCompleted));
    if (effect_state || recovery_terminal)
        && transaction.members.iter().any(|member| {
            if !member.in_final_tree {
                return !matches!(
                    member.freeze,
                    FreezePhase::PreservedStopped | FreezePhase::Released
                );
            }
            if member.was_stopped {
                return member.freeze != FreezePhase::PreservedStopped;
            }
            if effect_state {
                if transaction.signal == ProcessSignalKind::Continue {
                    member.freeze != FreezePhase::Captured
                } else {
                    member.freeze != FreezePhase::FrozenByUs
                }
            } else if transaction.signal == ProcessSignalKind::Continue {
                !matches!(member.freeze, FreezePhase::Captured | FreezePhase::Released)
            } else {
                !matches!(
                    member.freeze,
                    FreezePhase::FrozenByUs | FreezePhase::Released
                )
            }
        })
    {
        return Err(corrupt("stable transaction has an invalid freeze state"));
    }
    let terminal = matches!(
        transaction.phase,
        TransactionPhase::EffectTerminal
            | TransactionPhase::RecoveryTerminal
            | TransactionPhase::ReceiptClosed
    );
    if terminal != transaction.terminal_kind.is_some()
        || (transaction.terminal_kind == Some(TerminalKind::EffectCompleted))
            != transaction.terminal_elapsed_ms.is_some()
        || (transaction.terminal_kind != Some(TerminalKind::EffectCompleted)
            && transaction.terminal_effect_verified.is_some())
    {
        return Err(corrupt("transaction terminal state is inconsistent"));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), CuError> {
    if id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(corrupt("transaction id must be 32 hexadecimal characters"))
    }
}

fn validate_receipt_id(id: &str) -> Result<(), CuError> {
    let parts = id.split('-').collect::<Vec<_>>();
    if id.len() > 80
        || parts.len() != 3
        || parts.iter().any(|part| part.is_empty())
        || parts
            .iter()
            .any(|part| !part.bytes().all(|byte| byte.is_ascii_digit()))
        || parts
            .iter()
            .any(|part| part.len() > 1 && part.starts_with('0'))
        || parts[0].parse::<u128>().is_err()
        || parts[1].parse::<u32>().ok().is_none_or(|pid| pid == 0)
        || parts[2].parse::<u64>().is_err()
    {
        return Err(corrupt(
            "receipt id does not match the reserved receipt format",
        ));
    }
    Ok(())
}

fn valid_identity(identity: &str) -> bool {
    !identity.is_empty() && identity.len() <= 512 && !identity.chars().any(char::is_control)
}

fn random_id() -> Result<String, CuError> {
    let bytes = agenterm_platform::entropy::secure_random_array::<16>()
        .map_err(|error| state_error(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn state_error(message: impl Into<String>) -> CuError {
    CuError::new("process_tree_recovery_state_unavailable", message)
}

fn corrupt(message: impl Into<String>) -> CuError {
    CuError::new("process_tree_recovery_state_corrupt", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "process-signal-recovery-test-{label}-{}",
                std::process::id()
            ))
    }

    #[cfg(not(windows))]
    fn spawn_recovery_fixture() -> std::process::Child {
        std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .unwrap()
    }

    #[cfg(not(windows))]
    fn wait_stopped(pid: u32, expected: bool) {
        for _ in 0..100 {
            if agenterm_platform::process_metrics::is_stopped(pid).unwrap() == expected {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("pid {pid} stopped state did not become {expected}");
    }

    #[cfg(not(windows))]
    fn reserve_tree_receipt(receipts: &mut ReceiptLog, pid: u32, identity: &str) -> ReceiptTicket {
        receipts
            .reserve(
                "process-signal-tree",
                0,
                json!({
                    "root": { "pid": pid, "start_identity": identity },
                    "signal": "SIGUSR1",
                }),
            )
            .unwrap()
    }

    #[test]
    fn torn_transaction_fails_before_recovery() {
        let root = scratch("torn");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("process-signal-transactions")).unwrap();
        fs::write(
            root.join("process-signal-transactions/00000000000000000000000000000000.json"),
            b"{\n",
        )
        .unwrap();
        let store = RecoveryStore::open_beside_receipt(&root.join("current.jsonl")).unwrap();
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let error = store.recover_pending(&mut receipts).unwrap_err();
        assert_eq!(error.code, "process_tree_recovery_state_corrupt");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn torn_public_receipt_refuses_a_new_tree_effect() {
        let root = scratch("torn-receipt");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("current.jsonl"), b"{\n").unwrap();
        let store = RecoveryStore::open_beside_receipt(&root.join("current.jsonl")).unwrap();
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let error = store.recover_pending(&mut receipts).unwrap_err();
        assert_eq!(error.code, "receipt_corrupt");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transaction_refuses_repeated_delivery() {
        let root = scratch("repeat");
        let _ = fs::remove_dir_all(&root);
        let store = RecoveryStore::open_beside_receipt(&root.join("current.jsonl")).unwrap();
        let id = store
            .begin(
                "1-1-1",
                42,
                "identity",
                ProcessSignalKind::User1,
                &[RecoveryMemberInput {
                    pid: 42,
                    depth: 0,
                    start_identity: "identity",
                    was_stopped: false,
                }],
            )
            .unwrap();
        store.before_freeze(&id, 42).unwrap();
        store.after_freeze(&id, 42).unwrap();
        store.mark_stable(&id, &[42]).unwrap();
        store.before_delivery(&id, 42).unwrap();
        let error = store.before_delivery(&id, 42).unwrap_err();
        assert_eq!(error.code, "process_tree_recovery_state_corrupt");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn recovery_classifies_identity_drift_as_cleanup_safe_without_wedging() {
        let root = scratch("identity-drift");
        let _ = fs::remove_dir_all(&root);
        let pid = std::process::id();
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let ticket = receipts
            .reserve(
                "process-signal-tree",
                0,
                json!({
                    "root": { "pid": pid, "start_identity": "deliberately-wrong-start-identity" },
                    "signal": "SIGUSR1",
                }),
            )
            .unwrap();
        let store = RecoveryStore::open_beside_receipt(receipts.path()).unwrap();
        store
            .begin(
                &ticket.id,
                pid,
                "deliberately-wrong-start-identity",
                ProcessSignalKind::User1,
                &[RecoveryMemberInput {
                    pid,
                    depth: 0,
                    start_identity: "deliberately-wrong-start-identity",
                    was_stopped: false,
                }],
            )
            .unwrap();
        let outcomes = store.recover_pending(&mut receipts).unwrap();
        assert_eq!(outcomes[0]["members"][0]["outcome"], "replaced");
        assert_eq!(outcomes[0]["cleanup_verified"], true);
        assert_eq!(outcomes[0]["effect_outcome_unknown"], true);
        assert!(store.recover_pending(&mut receipts).unwrap().is_empty());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn recovery_validates_but_does_not_mutate_a_preserved_member() {
        let root = scratch("preserved");
        let _ = fs::remove_dir_all(&root);
        let pid = std::process::id();
        let identity = live_identity(pid).unwrap();
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let ticket = receipts
            .reserve(
                "process-signal-tree",
                0,
                json!({
                    "root": { "pid": pid, "start_identity": identity },
                    "signal": "SIGUSR1",
                }),
            )
            .unwrap();
        let store = RecoveryStore::open_beside_receipt(receipts.path()).unwrap();
        store
            .begin(
                &ticket.id,
                pid,
                &identity,
                ProcessSignalKind::User1,
                &[RecoveryMemberInput {
                    pid,
                    depth: 0,
                    start_identity: &identity,
                    was_stopped: true,
                }],
            )
            .unwrap();
        let outcomes = store.recover_pending(&mut receipts).unwrap();
        assert_eq!(outcomes[0]["members"][0]["outcome"], "preserved");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn terminal_phase_repairs_missing_receipt_and_existing_receipt_is_not_duplicated() {
        let root = scratch("terminal-receipt");
        let _ = fs::remove_dir_all(&root);
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let ticket = receipts
            .reserve(
                "process-signal-tree",
                0,
                json!({
                    "root": { "pid": 42, "start_identity": "identity" },
                    "signal": "SIGSTOP",
                }),
            )
            .unwrap();
        let store = RecoveryStore::open_beside_receipt(receipts.path()).unwrap();
        let id = store
            .begin(
                &ticket.id,
                42,
                "identity",
                ProcessSignalKind::Stop,
                &[RecoveryMemberInput {
                    pid: 42,
                    depth: 0,
                    start_identity: "identity",
                    was_stopped: false,
                }],
            )
            .unwrap();
        let mut transaction = store.read(&id).unwrap();
        transaction.members[0].freeze = FreezePhase::FrozenByUs;
        transaction.members[0].in_final_tree = true;
        transaction.members[0].delivery = DeliveryPhase::Delivered;
        transaction.phase = TransactionPhase::EffectTerminal;
        transaction.terminal_kind = Some(TerminalKind::EffectCompleted);
        transaction.terminal_elapsed_ms = Some(7);
        transaction.terminal_effect_verified = Some(true);
        store.persist(&transaction).unwrap();
        store.recover_pending(&mut receipts).unwrap();
        assert_eq!(receipts.list(None, usize::MAX).unwrap().1, 2);
        assert!(!store.directory.join(format!("{id}.json")).exists());

        let ticket = receipts
            .reserve(
                "process-signal-tree",
                0,
                json!({
                    "root": { "pid": 43, "start_identity": "identity-2" },
                    "signal": "SIGSTOP",
                }),
            )
            .unwrap();
        let id = store
            .begin(
                &ticket.id,
                43,
                "identity-2",
                ProcessSignalKind::Stop,
                &[RecoveryMemberInput {
                    pid: 43,
                    depth: 0,
                    start_identity: "identity-2",
                    was_stopped: false,
                }],
            )
            .unwrap();
        let mut transaction = store.read(&id).unwrap();
        transaction.members[0].freeze = FreezePhase::FrozenByUs;
        transaction.members[0].in_final_tree = true;
        transaction.members[0].delivery = DeliveryPhase::Delivered;
        transaction.phase = TransactionPhase::EffectTerminal;
        transaction.terminal_kind = Some(TerminalKind::EffectCompleted);
        transaction.terminal_elapsed_ms = Some(8);
        transaction.terminal_effect_verified = Some(true);
        store.persist(&transaction).unwrap();
        receipts
            .complete(
                &ticket,
                "process-signal-tree",
                0,
                true,
                json!({ "performed": true, "member_count": 1, "elapsed_ms": 8 }),
            )
            .unwrap();
        let before = receipts.list(None, usize::MAX).unwrap().1;
        store.recover_pending(&mut receipts).unwrap();
        assert_eq!(receipts.list(None, usize::MAX).unwrap().1, before);
        assert!(!store.directory.join(format!("{id}.json")).exists());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_identifiers_and_depth_are_closed_before_output() {
        let mut transaction = Transaction {
            schema_version: SCHEMA_VERSION,
            transaction_id: "00000000000000000000000000000000".into(),
            receipt_id: "1-2-3".into(),
            root_pid: 42,
            root_start_identity: "identity".into(),
            signal: ProcessSignalKind::Stop,
            phase: TransactionPhase::Frozen,
            members: vec![DurableMember {
                pid: 42,
                depth: 0,
                start_identity: "identity".into(),
                was_stopped: false,
                freeze: FreezePhase::FrozenByUs,
                in_final_tree: true,
                delivery: DeliveryPhase::Pending,
                recovery: None,
            }],
            terminal_kind: None,
            terminal_elapsed_ms: None,
            terminal_effect_verified: None,
            effect_outcome_unknown: false,
        };
        transaction.transaction_id.replace_range(..1, "A");
        assert!(validate_transaction(&transaction).is_err());
        transaction.transaction_id.replace_range(..1, "0");
        transaction.receipt_id = "not-a-receipt".into();
        assert!(validate_transaction(&transaction).is_err());
        transaction.receipt_id = "1-2-3".into();
        transaction.members[0].start_identity = "bad\nidentity".into();
        assert!(validate_transaction(&transaction).is_err());
        transaction.members[0].start_identity = "identity".into();
        transaction.members[0].depth = 10_001;
        assert!(validate_transaction(&transaction).is_err());
        transaction.members[0].depth = 0;
        transaction.members[0].freeze = FreezePhase::Captured;
        assert!(validate_transaction(&transaction).is_err());
        transaction.signal = ProcessSignalKind::Continue;
        assert!(validate_transaction(&transaction).is_ok());
        transaction.phase = TransactionPhase::Delivering;
        assert!(validate_transaction(&transaction).is_ok());
        transaction.signal = ProcessSignalKind::Stop;
        assert!(validate_transaction(&transaction).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn recovery_before_first_suspend_preserves_a_running_exact_member() {
        let root = scratch("freeze-intent-running");
        let _ = fs::remove_dir_all(&root);
        let mut child = spawn_recovery_fixture();
        let pid = child.id();
        let identity = live_identity(pid).unwrap();
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let ticket = reserve_tree_receipt(&mut receipts, pid, &identity);
        let store = RecoveryStore::open_beside_receipt(receipts.path()).unwrap();
        let id = store
            .begin(
                &ticket.id,
                pid,
                &identity,
                ProcessSignalKind::User1,
                &[RecoveryMemberInput {
                    pid,
                    depth: 0,
                    start_identity: &identity,
                    was_stopped: false,
                }],
            )
            .unwrap();
        store.before_freeze(&id, pid).unwrap();
        let outcomes = store.recover_pending(&mut receipts).unwrap();
        assert_eq!(
            outcomes[0]["members"][0]["outcome"],
            "freeze_intent_not_applied"
        );
        wait_stopped(pid, false);
        child.kill().unwrap();
        child.wait().unwrap();
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn recovery_after_suspend_before_frozen_mark_resumes_with_ambiguous_evidence() {
        let root = scratch("freeze-intent-stopped");
        let _ = fs::remove_dir_all(&root);
        let mut child = spawn_recovery_fixture();
        let pid = child.id();
        let identity = live_identity(pid).unwrap();
        let reference =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(pid)
                .unwrap();
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let ticket = reserve_tree_receipt(&mut receipts, pid, &identity);
        let store = RecoveryStore::open_beside_receipt(receipts.path()).unwrap();
        let id = store
            .begin(
                &ticket.id,
                pid,
                &identity,
                ProcessSignalKind::User1,
                &[RecoveryMemberInput {
                    pid,
                    depth: 0,
                    start_identity: &identity,
                    was_stopped: false,
                }],
            )
            .unwrap();
        store.before_freeze(&id, pid).unwrap();
        reference.set_suspended(true).unwrap();
        wait_stopped(pid, true);
        let outcomes = store.recover_pending(&mut receipts).unwrap();
        assert_eq!(
            outcomes[0]["members"][0]["outcome"],
            "freeze_intent_resumed"
        );
        assert_eq!(outcomes[0]["members"][0]["effect_outcome_unknown"], true);
        wait_stopped(pid, false);
        child.kill().unwrap();
        child.wait().unwrap();
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn recovery_mid_multi_member_freeze_resumes_every_owned_or_intended_stop() {
        let root = scratch("freeze-mid-multi");
        let _ = fs::remove_dir_all(&root);
        let mut first = spawn_recovery_fixture();
        let mut second = spawn_recovery_fixture();
        let first_id = first.id();
        let second_id = second.id();
        let first_identity = live_identity(first_id).unwrap();
        let second_identity = live_identity(second_id).unwrap();
        let first_ref =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(first_id)
                .unwrap();
        let second_ref =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(second_id)
                .unwrap();
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let ticket = reserve_tree_receipt(&mut receipts, first_id, &first_identity);
        let store = RecoveryStore::open_beside_receipt(receipts.path()).unwrap();
        let id = store
            .begin(
                &ticket.id,
                first_id,
                &first_identity,
                ProcessSignalKind::User1,
                &[RecoveryMemberInput {
                    pid: first_id,
                    depth: 0,
                    start_identity: &first_identity,
                    was_stopped: false,
                }],
            )
            .unwrap();
        store
            .register_members(
                &id,
                &[RecoveryMemberInput {
                    pid: second_id,
                    depth: 1,
                    start_identity: &second_identity,
                    was_stopped: false,
                }],
            )
            .unwrap();
        store.before_freeze(&id, first_id).unwrap();
        first_ref.set_suspended(true).unwrap();
        store.after_freeze(&id, first_id).unwrap();
        store.before_freeze(&id, second_id).unwrap();
        second_ref.set_suspended(true).unwrap();
        wait_stopped(first_id, true);
        wait_stopped(second_id, true);
        let outcomes = store.recover_pending(&mut receipts).unwrap();
        assert_eq!(
            outcomes[0]["members"][0]["outcome"],
            "freeze_intent_resumed"
        );
        assert_eq!(outcomes[0]["members"][1]["outcome"], "resumed");
        wait_stopped(first_id, false);
        wait_stopped(second_id, false);
        first.kill().unwrap();
        second.kill().unwrap();
        first.wait().unwrap();
        second.wait().unwrap();
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn recovery_after_removed_member_resume_before_release_mark_is_idempotent() {
        let root = scratch("release-intent-running");
        let _ = fs::remove_dir_all(&root);
        let mut child = spawn_recovery_fixture();
        let pid = child.id();
        let identity = live_identity(pid).unwrap();
        let reference =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(pid)
                .unwrap();
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let ticket = reserve_tree_receipt(&mut receipts, pid, &identity);
        let store = RecoveryStore::open_beside_receipt(receipts.path()).unwrap();
        let id = store
            .begin(
                &ticket.id,
                pid,
                &identity,
                ProcessSignalKind::User1,
                &[RecoveryMemberInput {
                    pid,
                    depth: 0,
                    start_identity: &identity,
                    was_stopped: false,
                }],
            )
            .unwrap();
        store.before_freeze(&id, pid).unwrap();
        reference.set_suspended(true).unwrap();
        store.after_freeze(&id, pid).unwrap();
        store.before_release(&id, pid).unwrap();
        reference.set_suspended(false).unwrap();
        wait_stopped(pid, false);
        let outcomes = store.recover_pending(&mut receipts).unwrap();
        assert_eq!(outcomes[0]["members"][0]["outcome"], "already_released");
        assert!(store.recover_pending(&mut receipts).unwrap().is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_process_signal_has_no_owner_death_environment_seam() {
        assert!(!include_str!("process.rs").contains("PROCESS_SIGNAL_OWNER_DEATH"));
    }

    #[cfg(not(windows))]
    #[test]
    fn exited_exact_member_is_cleanup_safe_and_does_not_wedge_later_calls() {
        let root = scratch("exited");
        let _ = fs::remove_dir_all(&root);
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 1"])
            .spawn()
            .unwrap();
        let pid = child.id();
        let identity = live_identity(pid).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let mut receipts = ReceiptLog::open_in(&root, crate::TargetRef::Current).unwrap();
        let ticket = receipts
            .reserve(
                "process-signal-tree",
                0,
                json!({
                    "root": { "pid": pid, "start_identity": identity },
                    "signal": "SIGUSR1",
                }),
            )
            .unwrap();
        let store = RecoveryStore::open_beside_receipt(receipts.path()).unwrap();
        store
            .begin(
                &ticket.id,
                pid,
                &identity,
                ProcessSignalKind::User1,
                &[RecoveryMemberInput {
                    pid,
                    depth: 0,
                    start_identity: &identity,
                    was_stopped: false,
                }],
            )
            .unwrap();
        let outcomes = store.recover_pending(&mut receipts).unwrap();
        assert_eq!(outcomes[0]["members"][0]["outcome"], "exited");
        assert_eq!(outcomes[0]["cleanup_verified"], true);
        assert_eq!(outcomes[0]["effect_outcome_unknown"], true);
        assert!(store.recover_pending(&mut receipts).unwrap().is_empty());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
