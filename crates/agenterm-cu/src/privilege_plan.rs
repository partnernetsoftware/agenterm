//! Read-only, canonical plans for closed privileged operations.
//!
//! Planning never invokes a broker, native consent surface, shell, or mutation.
//! A later provider must validate the complete plan and its digest again.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::CuError;

pub const DEFAULT_PLAN_TTL_SECONDS: u64 = 120;
pub const MIN_PLAN_TTL_SECONDS: u64 = 1;
pub const MAX_PLAN_TTL_SECONDS: u64 = 600;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PrivilegeOperation {
    #[serde(rename = "process.set-priority")]
    ProcessSetPriority,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessPriorityTarget {
    pub pid: u32,
    pub start_identity: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessPriorityState {
    pub nice: i32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessPriorityPlan {
    pub schema_version: u32,
    pub operation: PrivilegeOperation,
    pub target: ProcessPriorityTarget,
    pub before: ProcessPriorityState,
    pub after: ProcessPriorityState,
    pub issued_at_utc_ms: u64,
    pub expires_at_utc_ms: u64,
    /// Stable for the exact operation, target identity, before and after state.
    pub contract_digest: String,
    /// Binds the complete expiring plan, including both timestamps.
    pub approval_digest: String,
    pub mutation_performed: bool,
}

pub fn process_priority_plan_now(
    pid: u32,
    desired_nice: i32,
    ttl_seconds: u64,
) -> Result<serde_json::Value, CuError> {
    let now_utc_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            CuError::new(
                "privilege_plan_clock_invalid",
                "host clock is before the Unix epoch",
            )
        })?
        .as_millis()
        .try_into()
        .map_err(|_| {
            CuError::new(
                "privilege_plan_clock_invalid",
                "host clock does not fit the plan timestamp contract",
            )
        })?;
    serde_json::to_value(process_priority_plan(
        pid,
        desired_nice,
        ttl_seconds,
        now_utc_ms,
    )?)
    .map_err(|_| {
        CuError::new(
            "privilege_plan_serialization_failed",
            "privilege plan could not be serialized",
        )
    })
}

#[derive(Serialize)]
struct ContractProjection<'a> {
    schema_version: u32,
    operation: PrivilegeOperation,
    target: &'a ProcessPriorityTarget,
    before: ProcessPriorityState,
    after: ProcessPriorityState,
}

#[derive(Serialize)]
struct ApprovalProjection<'a> {
    contract: ContractProjection<'a>,
    issued_at_utc_ms: u64,
    expires_at_utc_ms: u64,
}

pub fn process_priority_plan(
    pid: u32,
    desired_nice: i32,
    ttl_seconds: u64,
    now_utc_ms: u64,
) -> Result<ProcessPriorityPlan, CuError> {
    if pid == 0 {
        return Err(CuError::new(
            "privilege_target_invalid",
            "process.set-priority pid must be greater than zero",
        ));
    }
    if !(-20..=20).contains(&desired_nice) {
        return Err(CuError::new(
            "privilege_parameter_invalid",
            "process.set-priority nice must be in -20..=20",
        ));
    }
    if !(MIN_PLAN_TTL_SECONDS..=MAX_PLAN_TTL_SECONDS).contains(&ttl_seconds) {
        return Err(CuError::new(
            "privilege_plan_ttl_invalid",
            format!(
                "privilege plan TTL must be in {MIN_PLAN_TTL_SECONDS}..={MAX_PLAN_TTL_SECONDS} seconds"
            ),
        ));
    }
    let ttl_ms = ttl_seconds.checked_mul(1_000).ok_or_else(|| {
        CuError::new("privilege_plan_ttl_invalid", "privilege plan TTL overflows")
    })?;
    let expires_at_utc_ms = now_utc_ms.checked_add(ttl_ms).ok_or_else(|| {
        CuError::new(
            "privilege_plan_clock_invalid",
            "privilege plan expiry overflows the host clock",
        )
    })?;

    let before_identity = live_start_identity(pid)?;
    let before_nice = read_nice(pid)?;
    let after_nice = read_nice(pid)?;
    let after_identity = live_start_identity(pid)?;
    if before_identity != after_identity || before_nice != after_nice {
        return Err(CuError::new(
            "privilege_target_changed",
            "process identity or priority changed while the plan was prepared",
        ));
    }

    let target = ProcessPriorityTarget {
        pid,
        start_identity: before_identity,
    };
    let before = ProcessPriorityState { nice: before_nice };
    let after = ProcessPriorityState { nice: desired_nice };
    let contract = ContractProjection {
        schema_version: 1,
        operation: PrivilegeOperation::ProcessSetPriority,
        target: &target,
        before,
        after,
    };
    let contract_digest = digest_json(&contract)?;
    let approval_digest = digest_json(&ApprovalProjection {
        contract,
        issued_at_utc_ms: now_utc_ms,
        expires_at_utc_ms,
    })?;
    Ok(ProcessPriorityPlan {
        schema_version: 1,
        operation: PrivilegeOperation::ProcessSetPriority,
        target,
        before,
        after,
        issued_at_utc_ms: now_utc_ms,
        expires_at_utc_ms,
        contract_digest,
        approval_digest,
        mutation_performed: false,
    })
}

/// Validate the closed plan without treating its digest as user consent.
///
/// A privileged provider must call this after its native consent succeeds,
/// then call [`revalidate_process_priority_precondition`] immediately before
/// reserving and attempting the effect.
pub fn validate_process_priority_plan(
    plan: &ProcessPriorityPlan,
    now_utc_ms: u64,
) -> Result<(), CuError> {
    if plan.schema_version != 1
        || plan.operation != PrivilegeOperation::ProcessSetPriority
        || plan.target.pid == 0
        || !(-20..=20).contains(&plan.before.nice)
        || !(-20..=20).contains(&plan.after.nice)
        || plan.target.start_identity.is_empty()
        || plan.target.start_identity.len() > 256
        || plan.mutation_performed
    {
        return Err(CuError::new(
            "privilege_plan_invalid",
            "privilege plan has an invalid closed shape",
        ));
    }
    let ttl_ms = plan
        .expires_at_utc_ms
        .checked_sub(plan.issued_at_utc_ms)
        .ok_or_else(|| {
            CuError::new(
                "privilege_plan_invalid",
                "privilege plan expiry precedes its issue time",
            )
        })?;
    if !(MIN_PLAN_TTL_SECONDS * 1_000..=MAX_PLAN_TTL_SECONDS * 1_000).contains(&ttl_ms) {
        return Err(CuError::new(
            "privilege_plan_invalid",
            "privilege plan lifetime is outside the bounded contract",
        ));
    }
    if now_utc_ms > plan.expires_at_utc_ms {
        return Err(CuError::new(
            "privilege_plan_expired",
            "privilege plan expired before provider reservation",
        ));
    }
    let contract = ContractProjection {
        schema_version: plan.schema_version,
        operation: plan.operation,
        target: &plan.target,
        before: plan.before,
        after: plan.after,
    };
    if digest_json(&contract)? != plan.contract_digest
        || digest_json(&ApprovalProjection {
            contract,
            issued_at_utc_ms: plan.issued_at_utc_ms,
            expires_at_utc_ms: plan.expires_at_utc_ms,
        })? != plan.approval_digest
    {
        return Err(CuError::new(
            "privilege_plan_digest_mismatch",
            "privilege plan content does not match its canonical digests",
        ));
    }
    Ok(())
}

/// Re-read the exact target identity and pre-effect state.
///
/// This check is necessary but is not itself an authority to mutate a PID.
/// Unix priority mutation remains unavailable until the provider owns an
/// exact-object primitive rather than reopening a mutable numeric PID.
pub fn revalidate_process_priority_precondition(plan: &ProcessPriorityPlan) -> Result<(), CuError> {
    let identity = live_start_identity(plan.target.pid)?;
    let nice = read_nice(plan.target.pid)?;
    if identity != plan.target.start_identity || nice != plan.before.nice {
        return Err(CuError::new(
            "privilege_precondition_changed",
            "process identity or priority changed after the plan was prepared",
        ));
    }
    Ok(())
}

fn live_start_identity(pid: u32) -> Result<String, CuError> {
    match agenterm_platform::process_observation::observe(pid) {
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: Some(identity),
        } => Ok(identity),
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: None,
        } => Err(CuError::new(
            "privilege_target_identity_unavailable",
            "process is live but its stable start identity is unavailable",
        )),
        agenterm_platform::process_observation::ProcessObservation::Dead { .. } => Err(
            CuError::new("privilege_target_not_found", "process is not live"),
        ),
        agenterm_platform::process_observation::ProcessObservation::Unknown { .. } | _ => {
            Err(CuError::new(
                "privilege_target_unavailable",
                "process identity could not be observed",
            ))
        }
    }
}

fn read_nice(pid: u32) -> Result<i32, CuError> {
    agenterm_platform::process_metrics::nice(pid).map_err(|error| {
        use agenterm_platform::process_metrics::ProcessMetricsErrorKind as Kind;
        let code = match error.kind() {
            Kind::InvalidId => "privilege_target_invalid",
            Kind::NotFound => "privilege_target_not_found",
            Kind::Unsupported => "privilege_operation_unsupported",
            _ => "privilege_target_unavailable",
        };
        CuError::new(code, "process priority could not be observed")
            .with_detail(serde_json::json!({ "kind": format!("{:?}", error.kind()) }))
    })
}

fn digest_json(value: &impl Serialize) -> Result<String, CuError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        CuError::new(
            "privilege_plan_serialization_failed",
            "privilege plan could not be serialized canonically",
        )
    })?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn plan_is_read_only_identity_bound_and_has_two_digest_scopes() {
        let pid = std::process::id();
        let current = agenterm_platform::process_metrics::nice(pid).unwrap();
        let first = process_priority_plan(pid, current, 120, 1_000).unwrap();
        let repeated = process_priority_plan(pid, current, 120, 1_000).unwrap();
        assert_eq!(first, repeated);
        assert_eq!(first.before.nice, current);
        assert_eq!(first.after.nice, current);
        assert!(!first.mutation_performed);
        assert_eq!(first.contract_digest.len(), 64);
        assert_eq!(first.approval_digest.len(), 64);

        let later = process_priority_plan(pid, current, 120, 2_000).unwrap();
        assert_eq!(later.contract_digest, first.contract_digest);
        assert_ne!(later.approval_digest, first.approval_digest);
    }

    #[test]
    fn plan_rejects_unbounded_inputs_before_observation() {
        assert_eq!(
            process_priority_plan(0, 0, 120, 1).unwrap_err().code,
            "privilege_target_invalid"
        );
        assert_eq!(
            process_priority_plan(1, 21, 120, 1).unwrap_err().code,
            "privilege_parameter_invalid"
        );
        assert_eq!(
            process_priority_plan(1, 0, 0, 1).unwrap_err().code,
            "privilege_plan_ttl_invalid"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn provider_validation_recomputes_digests_and_rechecks_precondition() {
        let pid = std::process::id();
        let nice = agenterm_platform::process_metrics::nice(pid).unwrap();
        let plan = process_priority_plan(pid, nice, 120, 1_000).unwrap();
        validate_process_priority_plan(&plan, 1_001).unwrap();
        revalidate_process_priority_precondition(&plan).unwrap();

        let mut tampered = plan.clone();
        tampered.after.nice = if nice == 20 { 19 } else { nice + 1 };
        assert_eq!(
            validate_process_priority_plan(&tampered, 1_001)
                .unwrap_err()
                .code,
            "privilege_plan_digest_mismatch"
        );
        assert_eq!(
            validate_process_priority_plan(&plan, plan.expires_at_utc_ms + 1)
                .unwrap_err()
                .code,
            "privilege_plan_expired"
        );
    }
}
