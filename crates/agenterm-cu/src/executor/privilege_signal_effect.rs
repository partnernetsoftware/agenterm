//! Provider-side process-signal effect over already retained native objects.
//!
//! The native consent shell owns peer authentication and authorization. This
//! module starts only after the provider replay ledger has durably reserved a
//! fresh request; it never accepts a naked PID as effect authority.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use serde_json::json;

use crate::{
    CuError,
    command::ProcessSignalKind,
    privilege_apply::{
        PreparedPrivilegeEffect, PreparedProcessSignalEffect, PrivilegeSignalEffectOutcome,
    },
    privilege_plan::{
        ProcessSignalScope, revalidate_process_signal_precondition,
        revalidate_process_signal_topology,
    },
    receipt::ReceiptLog,
};

use super::process_signal_recovery::{RecoveryMemberInput, RecoveryStore};

pub(crate) fn execute_prepared_signal(
    prepared: PreparedPrivilegeEffect,
    provider_state_root: &Path,
) -> PrivilegeSignalEffectOutcome {
    let PreparedPrivilegeEffect::ProcessSignal(effect) = prepared;
    if effect.plan.scope == ProcessSignalScope::Single {
        execute_single(effect, provider_state_root)
    } else {
        execute_tree(effect, provider_state_root)
    }
}

fn execute_single(
    effect: PreparedProcessSignalEffect,
    provider_state_root: &Path,
) -> PrivilegeSignalEffectOutcome {
    let mut receipts = match provider_receipts(provider_state_root) {
        Ok(receipts) => receipts,
        Err(error) => return PrivilegeSignalEffectOutcome::FailedBeforeEffect(error),
    };
    let ticket = match receipts.reserve(
        "privilege-process-signal",
        0,
        json!({
            "contract_digest": effect.plan.contract_digest,
            "scope": "single",
            "signal": effect.plan.signal.as_str(),
        }),
    ) {
        Ok(ticket) => ticket,
        Err(error) => return PrivilegeSignalEffectOutcome::FailedBeforeEffect(error),
    };
    if let Err(error) = revalidate_process_signal_precondition(&effect.plan) {
        let _ = receipts.complete(
            &ticket,
            "privilege-process-signal",
            0,
            false,
            json!({ "performed": false, "error_code": error.code }),
        );
        return PrivilegeSignalEffectOutcome::FailedBeforeEffect(error);
    }
    let Some(reference) = effect.references.first() else {
        return PrivilegeSignalEffectOutcome::FailedBeforeEffect(CuError::new(
            "privilege_provider_state_invalid",
            "prepared single-process signal has no retained native object",
        ));
    };
    let started = Instant::now();
    if let Err(error) = deliver(reference, effect.plan.signal) {
        let typed = effect_error(error);
        let _ = receipts.complete(
            &ticket,
            "privilege-process-signal",
            0,
            false,
            json!({ "performed": true, "verified": false, "error_code": typed.code }),
        );
        return PrivilegeSignalEffectOutcome::FailedAfterEffect {
            error: typed,
            outcome_unknown: true,
        };
    }
    let verification = verify_all(&effect, started);
    let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    finish_effect_receipt(receipts, ticket, &effect, verification, elapsed_ms)
}

fn execute_tree(
    effect: PreparedProcessSignalEffect,
    provider_state_root: &Path,
) -> PrivilegeSignalEffectOutcome {
    let mut receipts = match provider_receipts(provider_state_root) {
        Ok(receipts) => receipts,
        Err(error) => return PrivilegeSignalEffectOutcome::FailedBeforeEffect(error),
    };
    let recovery = match RecoveryStore::open_beside_receipt(receipts.path()) {
        Ok(recovery) => recovery,
        Err(error) => return PrivilegeSignalEffectOutcome::FailedBeforeEffect(error),
    };
    if let Err(error) = recovery.recover_pending(&mut receipts) {
        return PrivilegeSignalEffectOutcome::FailedBeforeEffect(error);
    }
    if let Err(error) = revalidate_process_signal_precondition(&effect.plan) {
        return PrivilegeSignalEffectOutcome::FailedBeforeEffect(error);
    }
    let ticket = match receipts.reserve(
        "process-signal-tree",
        0,
        json!({
            "contract_digest": effect.plan.contract_digest,
            "root": {
                "pid": effect.plan.target.pid,
                "start_identity": effect.plan.target.start_identity,
            },
            "signal": effect.plan.signal.as_str(),
            "max_descendants": effect.plan.max_descendants,
        }),
    ) {
        Ok(ticket) => ticket,
        Err(error) => return PrivilegeSignalEffectOutcome::FailedBeforeEffect(error),
    };
    let inputs = effect
        .plan
        .members
        .iter()
        .map(|member| RecoveryMemberInput {
            pid: member.pid,
            depth: member.depth as usize,
            start_identity: &member.start_identity,
            was_stopped: member.before.stopped,
        })
        .collect::<Vec<_>>();
    let transaction_id = match recovery.begin(
        &ticket.id,
        effect.plan.target.pid,
        &effect.plan.target.start_identity,
        effect.plan.signal,
        &inputs,
    ) {
        Ok(id) => id,
        Err(error) => {
            return PrivilegeSignalEffectOutcome::FailedBeforeEffect(close_before_error(
                &mut receipts,
                &ticket,
                error,
            ));
        }
    };
    let mut temporary_effect_started = false;
    if effect.plan.signal != ProcessSignalKind::Continue {
        for (member, reference) in effect.plan.members.iter().zip(&effect.references) {
            if member.before.stopped {
                continue;
            }
            if let Err(error) = recovery.before_freeze(&transaction_id, member.pid) {
                return recover_after_effect(
                    &recovery,
                    &mut receipts,
                    error,
                    temporary_effect_started,
                );
            }
            temporary_effect_started = true;
            if let Err(error) = reference.set_suspended(true) {
                return recover_after_effect(
                    &recovery,
                    &mut receipts,
                    CuError::new("privilege_signal_freeze_failed", error.to_string()),
                    true,
                );
            }
            if let Err(error) = recovery.after_freeze(&transaction_id, member.pid) {
                return recover_after_effect(&recovery, &mut receipts, error, true);
            }
        }
    }
    let member_ids = effect
        .plan
        .members
        .iter()
        .map(|member| member.pid)
        .collect::<Vec<_>>();
    if let Err(error) = recovery.mark_stable(&transaction_id, &member_ids) {
        return recover_after_effect(&recovery, &mut receipts, error, temporary_effect_started);
    }
    if effect.plan.signal != ProcessSignalKind::Continue
        && let Err(error) = revalidate_process_signal_topology(&effect.plan)
    {
        return recover_after_effect(&recovery, &mut receipts, error, temporary_effect_started);
    }

    let started = Instant::now();
    for (member, reference) in effect.plan.members.iter().zip(&effect.references).rev() {
        if let Err(error) = recovery.before_delivery(&transaction_id, member.pid) {
            return recover_after_effect(&recovery, &mut receipts, error, true);
        }
        if let Err(error) = deliver(reference, effect.plan.signal) {
            return recover_after_effect(&recovery, &mut receipts, effect_error(error), true);
        }
        if let Err(error) = recovery.after_delivery(&transaction_id, member.pid) {
            return recover_after_effect(&recovery, &mut receipts, error, true);
        }
    }

    if !matches!(
        effect.plan.signal,
        ProcessSignalKind::Stop | ProcessSignalKind::Kill | ProcessSignalKind::Continue
    ) {
        for (member, reference) in effect.plan.members.iter().zip(&effect.references).rev() {
            if member.before.stopped {
                continue;
            }
            match reference.is_alive() {
                Ok(false) => {
                    if let Err(error) = recovery.released_after_exit(&transaction_id, member.pid) {
                        return recover_after_effect(&recovery, &mut receipts, error, true);
                    }
                }
                Ok(true) => {
                    if let Err(error) = recovery.before_release(&transaction_id, member.pid) {
                        return recover_after_effect(&recovery, &mut receipts, error, true);
                    }
                    if let Err(error) = reference.set_suspended(false) {
                        return recover_after_effect(
                            &recovery,
                            &mut receipts,
                            CuError::new("privilege_signal_release_failed", error.to_string()),
                            true,
                        );
                    }
                    if let Err(error) = recovery.after_release(&transaction_id, member.pid) {
                        return recover_after_effect(&recovery, &mut receipts, error, true);
                    }
                }
                Err(error) => {
                    return recover_after_effect(
                        &recovery,
                        &mut receipts,
                        CuError::new("privilege_signal_readback_failed", error.to_string()),
                        true,
                    );
                }
            }
        }
    }

    let verification = verify_all(&effect, started);
    match verification {
        Ok(verified) => {
            let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            if let Err(error) =
                recovery.finish_effect(&transaction_id, elapsed_ms, verified, &mut receipts)
            {
                return PrivilegeSignalEffectOutcome::FailedAfterEffect {
                    error,
                    outcome_unknown: true,
                };
            }
            completed_outcome(&ticket, &effect, verified, elapsed_ms)
        }
        Err(error) => recover_after_effect(&recovery, &mut receipts, error, true),
    }
}

fn provider_receipts(provider_state_root: &Path) -> Result<ReceiptLog, CuError> {
    ReceiptLog::open_in(
        &provider_state_root.join("process-signal-receipts"),
        crate::TargetRef::Current,
    )
}

fn deliver(
    reference: &agenterm_platform::process_reference::ProcessReference,
    signal: ProcessSignalKind,
) -> std::io::Result<()> {
    match signal {
        ProcessSignalKind::Terminate => {
            reference.terminate(agenterm_platform::process_control::TerminationMode::Graceful)
        }
        ProcessSignalKind::Kill => {
            reference.terminate(agenterm_platform::process_control::TerminationMode::Forceful)
        }
        ProcessSignalKind::Stop => reference.set_suspended(true),
        ProcessSignalKind::Continue => reference.set_suspended(false),
        ProcessSignalKind::Hangup => {
            reference.send_signal(agenterm_platform::process_reference::ProcessSignal::Hangup)
        }
        ProcessSignalKind::Interrupt => {
            reference.send_signal(agenterm_platform::process_reference::ProcessSignal::Interrupt)
        }
        ProcessSignalKind::User1 => {
            reference.send_signal(agenterm_platform::process_reference::ProcessSignal::User1)
        }
        ProcessSignalKind::User2 => {
            reference.send_signal(agenterm_platform::process_reference::ProcessSignal::User2)
        }
    }
}

fn verify_all(
    effect: &PreparedProcessSignalEffect,
    started: Instant,
) -> Result<Option<bool>, CuError> {
    if matches!(
        effect.plan.signal,
        ProcessSignalKind::Hangup
            | ProcessSignalKind::Interrupt
            | ProcessSignalKind::User1
            | ProcessSignalKind::User2
    ) {
        return Ok(None);
    }
    loop {
        let mut all = true;
        for (member, reference) in effect.plan.members.iter().zip(&effect.references) {
            let alive = reference.is_alive().map_err(|error| {
                CuError::new("privilege_signal_readback_failed", error.to_string())
            })?;
            let matches = match effect.plan.signal {
                ProcessSignalKind::Terminate | ProcessSignalKind::Kill => !alive,
                ProcessSignalKind::Stop if alive => process_stopped(member.pid)?,
                ProcessSignalKind::Continue if alive => !process_stopped(member.pid)?,
                ProcessSignalKind::Stop | ProcessSignalKind::Continue => false,
                _ => unreachable!("non-verifiable signals returned above"),
            };
            all &= matches;
        }
        if all {
            return Ok(Some(true));
        }
        if started.elapsed() >= Duration::from_millis(effect.plan.timeout_ms) {
            return Err(CuError::new(
                "privilege_signal_postcondition_failed",
                "the retained process objects did not reach the required postcondition before timeout",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn process_stopped(pid: u32) -> Result<bool, CuError> {
    agenterm_platform::process_metrics::is_stopped(pid)
        .map_err(|error| CuError::new("privilege_signal_readback_failed", error.to_string()))
}

fn finish_effect_receipt(
    mut receipts: ReceiptLog,
    ticket: crate::receipt::ReceiptTicket,
    effect: &PreparedProcessSignalEffect,
    verification: Result<Option<bool>, CuError>,
    elapsed_ms: u64,
) -> PrivilegeSignalEffectOutcome {
    match verification {
        Ok(verified) => {
            if let Err(error) = receipts.complete(
                &ticket,
                "privilege-process-signal",
                0,
                verified.unwrap_or(true),
                json!({ "performed": true, "verified": verified, "elapsed_ms": elapsed_ms }),
            ) {
                return PrivilegeSignalEffectOutcome::FailedAfterEffect {
                    error,
                    outcome_unknown: true,
                };
            }
            completed_outcome(&ticket, effect, verified, elapsed_ms)
        }
        Err(error) => {
            let _ = receipts.complete(
                &ticket,
                "privilege-process-signal",
                0,
                false,
                json!({ "performed": true, "verified": false, "error_code": error.code }),
            );
            PrivilegeSignalEffectOutcome::FailedAfterEffect {
                error,
                outcome_unknown: true,
            }
        }
    }
}

fn completed_outcome(
    ticket: &crate::receipt::ReceiptTicket,
    effect: &PreparedProcessSignalEffect,
    verified: Option<bool>,
    elapsed_ms: u64,
) -> PrivilegeSignalEffectOutcome {
    PrivilegeSignalEffectOutcome::Completed {
        evidence: json!({
            "contract_digest": effect.plan.contract_digest,
            "signal": effect.plan.signal.as_str(),
            "scope": if effect.plan.scope == ProcessSignalScope::Single { "single" } else { "tree" },
            "member_count": effect.references.len(),
            "performed": true,
            "verified": verified,
            "elapsed_ms": elapsed_ms,
            "receipt": ticket.json(),
        }),
        verified,
    }
}

fn close_before_error(
    receipts: &mut ReceiptLog,
    ticket: &crate::receipt::ReceiptTicket,
    error: CuError,
) -> CuError {
    let _ = receipts.complete(
        ticket,
        "process-signal-tree",
        0,
        false,
        json!({ "performed": false, "error_code": error.code }),
    );
    error
}

fn recover_after_effect(
    recovery: &RecoveryStore,
    receipts: &mut ReceiptLog,
    error: CuError,
    effect_started: bool,
) -> PrivilegeSignalEffectOutcome {
    let recovery_result = recovery.recover_pending(receipts);
    let recovery_failed = recovery_result.is_err();
    let error = match recovery_result {
        Ok(_) => error,
        Err(recovery_error) => CuError::new(
            "privilege_signal_recovery_failed",
            format!("{}; recovery: {}", error.message, recovery_error.message),
        ),
    };
    PrivilegeSignalEffectOutcome::FailedAfterEffect {
        error,
        outcome_unknown: effect_started || recovery_failed,
    }
}

fn effect_error(error: std::io::Error) -> CuError {
    CuError::new(
        if error.kind() == std::io::ErrorKind::Unsupported {
            "privilege_signal_unsupported"
        } else {
            "privilege_signal_failed"
        },
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::privilege_apply::{
        PRIVILEGE_APPLY_PROTOCOL_VERSION, PRIVILEGE_PROVIDER_CONTRACT_VERSION,
        PrivilegeApplyRequestV1, PrivilegeAuthorizationV1, PrivilegeClientV1, PrivilegeOriginV1,
        PrivilegePlanV1, PrivilegeTargetScope, parse_apply_request, prepare_privilege_effect,
    };

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn prepared_tree(
        pid: u32,
        signal: ProcessSignalKind,
        force: bool,
        request_id: &str,
    ) -> PreparedPrivilegeEffect {
        let plan = crate::privilege_plan::process_signal_plan(
            pid, signal, force, true, 5_000, 16, 120, 1_000,
        )
        .unwrap();
        let request = PrivilegeApplyRequestV1 {
            protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: request_id.into(),
            plan: PrivilegePlanV1::ProcessSignal(plan),
            authorization: PrivilegeAuthorizationV1::OneShotNativeConsent,
            origin: PrivilegeOriginV1 {
                session_id: "fixture-session".into(),
                target_scope: PrivilegeTargetScope::Current,
            },
            client: PrivilegeClientV1 {
                contract_version: PRIVILEGE_PROVIDER_CONTRACT_VERSION,
            },
        };
        let validated = parse_apply_request(&serde_json::to_vec(&request).unwrap()).unwrap();
        prepare_privilege_effect(&validated).unwrap()
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn retained_tree_stop_continue_and_kill_share_one_recoverable_effect_core() {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "agenterm-cu-provider-tree-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&root);
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30 & sleep 30 & wait"])
            .spawn()
            .expect("spawn provider tree fixture");

        let stopped = execute_prepared_signal(
            prepared_tree(child.id(), ProcessSignalKind::Stop, false, "tree-stop"),
            &root,
        );
        let PrivilegeSignalEffectOutcome::Completed {
            verified: Some(true),
            evidence,
        } = stopped
        else {
            panic!("tree STOP must complete, got {stopped:?}")
        };
        assert!(evidence["member_count"].as_u64().unwrap() >= 3);

        let continued = execute_prepared_signal(
            prepared_tree(
                child.id(),
                ProcessSignalKind::Continue,
                false,
                "tree-continue",
            ),
            &root,
        );
        assert!(matches!(
            continued,
            PrivilegeSignalEffectOutcome::Completed {
                verified: Some(true),
                ..
            }
        ));

        let killed = execute_prepared_signal(
            prepared_tree(child.id(), ProcessSignalKind::Kill, true, "tree-kill"),
            &root,
        );
        assert!(matches!(
            killed,
            PrivilegeSignalEffectOutcome::Completed {
                verified: Some(true),
                ..
            }
        ));
        child.wait().expect("reap provider tree fixture");
        std::fs::remove_dir_all(root).unwrap();
    }
}
