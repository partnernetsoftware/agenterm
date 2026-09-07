//! Read-only, canonical plans for closed privileged operations.
//!
//! Planning never invokes a broker, native consent surface, shell, or mutation.
//! A later provider must validate the complete plan and its digest again.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{CuError, command::ProcessSignalKind};

pub const DEFAULT_PLAN_TTL_SECONDS: u64 = 120;
pub const MIN_PLAN_TTL_SECONDS: u64 = 1;
pub const MAX_PLAN_TTL_SECONDS: u64 = 600;
pub const PROCESS_SIGNAL_TIMEOUT_MS_MAX: u64 = 60_000;
/// MCU's largest closed privileged tree was 128 descendants. Keep the same
/// product ceiling and make the subsequent provider wire prove it can carry
/// the maximum accepted plan instead of silently narrowing this contract.
pub const PROCESS_SIGNAL_TREE_MAX_DESCENDANTS: u32 = 128;
const PROCESS_SIGNAL_SNAPSHOT_ATTEMPTS: usize = 6;
const PROCESS_SIGNAL_SNAPSHOT_SETTLE_MS: u64 = 30;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PrivilegeOperation {
    #[serde(rename = "process.set-priority")]
    ProcessSetPriority,
    #[serde(rename = "process.signal")]
    ProcessSignal,
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

/// Whether the later privileged provider may address only the exact root or
/// the complete bounded descendant set frozen by this plan.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessSignalScope {
    Single,
    Tree,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSignalTarget {
    pub pid: u32,
    pub start_identity: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSignalBeforeState {
    pub stopped: bool,
}

/// One exact member of the frozen process-signal scope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSignalMember {
    pub pid: u32,
    pub depth: u32,
    /// Exact parent inside the approved tree. The root has no in-tree parent.
    pub parent_pid: Option<u32>,
    pub start_identity: String,
    pub before: ProcessSignalBeforeState,
}

/// Read-only intent for a later exact-object privileged signal provider.
///
/// This record deliberately contains no native process handle and confers no
/// effect authority. The provider must validate it, obtain native consent,
/// retain every exact native object, and revalidate the precondition before
/// reserving an effect receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSignalPlan {
    pub schema_version: u32,
    pub operation: PrivilegeOperation,
    pub target: ProcessSignalTarget,
    pub scope: ProcessSignalScope,
    pub signal: ProcessSignalKind,
    pub force: bool,
    pub timeout_ms: u64,
    pub max_descendants: u32,
    pub members: Vec<ProcessSignalMember>,
    pub issued_at_utc_ms: u64,
    pub expires_at_utc_ms: u64,
    /// Stable for the exact signal contract and observed precondition.
    pub contract_digest: String,
    /// Binds the complete expiring plan, including both timestamps.
    pub approval_digest: String,
    pub consent_requested: bool,
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

#[derive(Serialize)]
struct ProcessSignalContractProjection<'a> {
    schema_version: u32,
    operation: PrivilegeOperation,
    target: &'a ProcessSignalTarget,
    scope: ProcessSignalScope,
    signal: ProcessSignalKind,
    force: bool,
    timeout_ms: u64,
    max_descendants: u32,
    members: &'a [ProcessSignalMember],
}

#[derive(Serialize)]
struct ProcessSignalApprovalProjection<'a> {
    contract: ProcessSignalContractProjection<'a>,
    issued_at_utc_ms: u64,
    expires_at_utc_ms: u64,
}

pub fn process_signal_plan_now(
    pid: u32,
    signal: ProcessSignalKind,
    force: bool,
    tree: bool,
    timeout_ms: u64,
    max_descendants: u32,
    ttl_seconds: u64,
) -> Result<serde_json::Value, CuError> {
    let now_utc_ms = now_utc_ms()?;
    serde_json::to_value(process_signal_plan(
        pid,
        signal,
        force,
        tree,
        timeout_ms,
        max_descendants,
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

#[allow(clippy::too_many_arguments)]
pub fn process_signal_plan(
    pid: u32,
    signal: ProcessSignalKind,
    force: bool,
    tree: bool,
    timeout_ms: u64,
    max_descendants: u32,
    ttl_seconds: u64,
    now_utc_ms: u64,
) -> Result<ProcessSignalPlan, CuError> {
    validate_process_signal_inputs(pid, signal, force, timeout_ms, max_descendants, ttl_seconds)?;
    let expires_at_utc_ms = plan_expiry(now_utc_ms, ttl_seconds)?;
    let scope = if tree {
        ProcessSignalScope::Tree
    } else {
        ProcessSignalScope::Single
    };
    let members = observe_stable_signal_members(pid, scope, max_descendants)?;
    build_process_signal_plan(
        signal,
        force,
        timeout_ms,
        max_descendants,
        scope,
        members,
        now_utc_ms,
        expires_at_utc_ms,
    )
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
        || plan.target.pid <= 1
        || !(-20..=20).contains(&plan.before.nice)
        || !(-20..=20).contains(&plan.after.nice)
        || !valid_process_start_identity(&plan.target.start_identity)
        || plan.mutation_performed
    {
        return Err(CuError::new(
            "privilege_plan_invalid",
            "privilege plan has an invalid closed shape",
        ));
    }
    validate_plan_lifetime(plan.issued_at_utc_ms, plan.expires_at_utc_ms, now_utc_ms)?;
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

/// Validate the complete closed signal plan before a provider asks for native
/// consent. A digest authenticates bytes, not authority; successful validation
/// still cannot deliver a signal.
pub fn validate_process_signal_plan(
    plan: &ProcessSignalPlan,
    now_utc_ms: u64,
) -> Result<(), CuError> {
    if plan.schema_version != 1
        || plan.operation != PrivilegeOperation::ProcessSignal
        || plan.target.pid <= 1
        || plan.target.start_identity.is_empty()
        || plan.target.start_identity.len() > 256
        || plan.timeout_ms == 0
        || plan.timeout_ms > PROCESS_SIGNAL_TIMEOUT_MS_MAX
        || !(1..=PROCESS_SIGNAL_TREE_MAX_DESCENDANTS).contains(&plan.max_descendants)
        || (plan.signal == ProcessSignalKind::Kill) != plan.force
        || plan.consent_requested
        || plan.mutation_performed
        || !valid_signal_members(plan)
    {
        return Err(CuError::new(
            "privilege_plan_invalid",
            "process signal plan has an invalid closed shape",
        ));
    }
    validate_plan_lifetime(plan.issued_at_utc_ms, plan.expires_at_utc_ms, now_utc_ms)?;
    let contract = signal_contract_projection(plan);
    if digest_json(&contract)? != plan.contract_digest
        || digest_json(&ProcessSignalApprovalProjection {
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

/// Re-observe the exact root identity, member set, depths and scheduler state.
///
/// This is a read-only precondition check. It does not retain a native process
/// object and therefore cannot authorize or perform the later signal effect.
pub fn revalidate_process_signal_precondition(plan: &ProcessSignalPlan) -> Result<(), CuError> {
    revalidate_process_signal_precondition_with(plan, observe_stable_signal_members)
}

fn revalidate_process_signal_precondition_with(
    plan: &ProcessSignalPlan,
    observe: impl FnOnce(u32, ProcessSignalScope, u32) -> Result<Vec<ProcessSignalMember>, CuError>,
) -> Result<(), CuError> {
    let current = observe(plan.target.pid, plan.scope, plan.max_descendants)?;
    if current != plan.members {
        return Err(CuError::new(
            "privilege_precondition_changed",
            "process identity, tree membership, depth or scheduler state changed after planning",
        ));
    }
    Ok(())
}

fn valid_process_start_identity(value: &str) -> bool {
    fn decimal(value: &str) -> bool {
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
    }

    if let Some(value) = value.strip_prefix("proc-start-ticks:") {
        return decimal(value);
    }
    if let Some(value) = value.strip_prefix("windows-filetime:") {
        return decimal(value);
    }
    value
        .strip_prefix("macos-start-time:")
        .and_then(|value| value.split_once('.'))
        .is_some_and(|(seconds, micros)| decimal(seconds) && decimal(micros))
}

fn validate_process_signal_inputs(
    pid: u32,
    signal: ProcessSignalKind,
    force: bool,
    timeout_ms: u64,
    max_descendants: u32,
    ttl_seconds: u64,
) -> Result<(), CuError> {
    if pid <= 1 {
        return Err(CuError::new(
            "privilege_target_invalid",
            "process.signal pid must be greater than one",
        ));
    }
    if (signal == ProcessSignalKind::Kill) != force {
        return Err(CuError::new(
            "privilege_parameter_invalid",
            "process.signal requires force exactly for SIGKILL",
        ));
    }
    if !(1..=PROCESS_SIGNAL_TIMEOUT_MS_MAX).contains(&timeout_ms) {
        return Err(CuError::new(
            "privilege_parameter_invalid",
            format!("process.signal timeout_ms must be in 1..={PROCESS_SIGNAL_TIMEOUT_MS_MAX}"),
        ));
    }
    if !(1..=PROCESS_SIGNAL_TREE_MAX_DESCENDANTS).contains(&max_descendants) {
        return Err(CuError::new(
            "privilege_parameter_invalid",
            format!(
                "process.signal max_descendants must be in 1..={PROCESS_SIGNAL_TREE_MAX_DESCENDANTS}"
            ),
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
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_process_signal_plan(
    signal: ProcessSignalKind,
    force: bool,
    timeout_ms: u64,
    max_descendants: u32,
    scope: ProcessSignalScope,
    members: Vec<ProcessSignalMember>,
    issued_at_utc_ms: u64,
    expires_at_utc_ms: u64,
) -> Result<ProcessSignalPlan, CuError> {
    let root = members
        .iter()
        .find(|member| member.depth == 0)
        .ok_or_else(|| {
            CuError::new(
                "privilege_target_unavailable",
                "process signal plan has no exact root observation",
            )
        })?;
    let target = ProcessSignalTarget {
        pid: root.pid,
        start_identity: root.start_identity.clone(),
    };
    let mut plan = ProcessSignalPlan {
        schema_version: 1,
        operation: PrivilegeOperation::ProcessSignal,
        target,
        scope,
        signal,
        force,
        timeout_ms,
        max_descendants,
        members,
        issued_at_utc_ms,
        expires_at_utc_ms,
        contract_digest: String::new(),
        approval_digest: String::new(),
        consent_requested: false,
        mutation_performed: false,
    };
    if !valid_signal_members(&plan) {
        return Err(CuError::new(
            "privilege_target_changed",
            "process signal membership was not a complete bounded tree",
        ));
    }
    let contract = signal_contract_projection(&plan);
    plan.contract_digest = digest_json(&contract)?;
    let contract = signal_contract_projection(&plan);
    plan.approval_digest = digest_json(&ProcessSignalApprovalProjection {
        contract,
        issued_at_utc_ms,
        expires_at_utc_ms,
    })?;
    Ok(plan)
}

fn signal_contract_projection(plan: &ProcessSignalPlan) -> ProcessSignalContractProjection<'_> {
    ProcessSignalContractProjection {
        schema_version: plan.schema_version,
        operation: plan.operation,
        target: &plan.target,
        scope: plan.scope,
        signal: plan.signal,
        force: plan.force,
        timeout_ms: plan.timeout_ms,
        max_descendants: plan.max_descendants,
        members: &plan.members,
    }
}

fn valid_signal_members(plan: &ProcessSignalPlan) -> bool {
    if plan.members.is_empty()
        || plan.members.len() > plan.max_descendants.saturating_add(1) as usize
    {
        return false;
    }
    if plan.scope == ProcessSignalScope::Single && plan.members.len() != 1 {
        return false;
    }
    let mut seen = BTreeSet::new();
    let mut previous_key = None;
    for member in &plan.members {
        if member.pid <= 1
            || !valid_process_start_identity(&member.start_identity)
            || !seen.insert(member.pid)
        {
            return false;
        }
        let key = (member.depth, member.pid);
        if previous_key.is_some_and(|previous| previous >= key) {
            return false;
        }
        match (member.depth, member.parent_pid) {
            (0, None) => {}
            (0, Some(_)) | (_, None) => return false,
            (depth, Some(parent_pid)) => {
                if !plan
                    .members
                    .iter()
                    .any(|row| row.pid == parent_pid && row.depth + 1 == depth)
                {
                    return false;
                }
            }
        }
        previous_key = Some(key);
    }
    let root = &plan.members[0];
    root.depth == 0
        && root.pid == plan.target.pid
        && root.start_identity == plan.target.start_identity
        && plan
            .members
            .iter()
            .filter(|member| member.depth == 0)
            .count()
            == 1
}

fn observe_stable_signal_members(
    root_pid: u32,
    scope: ProcessSignalScope,
    max_descendants: u32,
) -> Result<Vec<ProcessSignalMember>, CuError> {
    let mut previous = None;
    for attempt in 0..PROCESS_SIGNAL_SNAPSHOT_ATTEMPTS {
        let current = observe_signal_members_once(root_pid, scope, max_descendants)?;
        if previous.as_ref() == Some(&current) {
            return Ok(current);
        }
        previous = Some(current);
        if attempt + 1 < PROCESS_SIGNAL_SNAPSHOT_ATTEMPTS {
            thread::sleep(Duration::from_millis(PROCESS_SIGNAL_SNAPSHOT_SETTLE_MS));
        }
    }
    Err(CuError::new(
        "privilege_target_changed",
        "process signal scope did not produce two equal bounded observations",
    ))
}

fn observe_signal_members_once(
    root_pid: u32,
    scope: ProcessSignalScope,
    max_descendants: u32,
) -> Result<Vec<ProcessSignalMember>, CuError> {
    let ids = match scope {
        ProcessSignalScope::Single => vec![(root_pid, 0, None)],
        ProcessSignalScope::Tree => signal_tree_ids(root_pid, max_descendants)?,
    };
    let mut members = ids
        .into_iter()
        .map(|(pid, depth, parent_pid)| observe_signal_member(pid, depth, parent_pid))
        .collect::<Result<Vec<_>, _>>()?;
    members.sort_by_key(|member| (member.depth, member.pid));
    Ok(members)
}

fn signal_tree_ids(
    root_pid: u32,
    max_descendants: u32,
) -> Result<Vec<(u32, u32, Option<u32>)>, CuError> {
    let rows = agenterm_platform::process::list().map_err(|error| {
        CuError::new(
            "privilege_target_unavailable",
            "process tree inventory failed",
        )
        .with_detail(serde_json::json!({ "kind": format!("{:?}", error.kind()) }))
    })?;
    if !rows.iter().any(|row| row.id == root_pid) {
        return Err(CuError::new(
            "privilege_target_not_found",
            "the exact root is absent from the process inventory",
        ));
    }
    let mut children = BTreeMap::<u32, Vec<u32>>::new();
    for row in rows {
        children.entry(row.parent_id).or_default().push(row.id);
    }
    for child_ids in children.values_mut() {
        child_ids.sort_unstable();
    }
    let mut seen = BTreeSet::from([root_pid]);
    let mut queue = VecDeque::from([(root_pid, 0u32)]);
    let mut ids = vec![(root_pid, 0u32, None)];
    while let Some((parent, depth)) = queue.pop_front() {
        for child in children.get(&parent).into_iter().flatten() {
            if seen.insert(*child) {
                if seen.len() - 1 > max_descendants as usize {
                    return Err(CuError::new(
                        "privilege_target_too_large",
                        format!("process signal tree exceeds {max_descendants} descendants"),
                    ));
                }
                ids.push((*child, depth + 1, Some(parent)));
                queue.push_back((*child, depth + 1));
            }
        }
    }
    Ok(ids)
}

fn observe_signal_member(
    pid: u32,
    depth: u32,
    parent_pid: Option<u32>,
) -> Result<ProcessSignalMember, CuError> {
    let before_identity = live_start_identity(pid)?;
    let before_stopped = read_stopped(pid)?;
    let after_stopped = read_stopped(pid)?;
    let after_identity = live_start_identity(pid)?;
    if before_identity != after_identity || before_stopped != after_stopped {
        return Err(CuError::new(
            "privilege_target_changed",
            format!("process {pid} changed while its signal precondition was observed"),
        ));
    }
    Ok(ProcessSignalMember {
        pid,
        depth,
        parent_pid,
        start_identity: before_identity,
        before: ProcessSignalBeforeState {
            stopped: before_stopped,
        },
    })
}

fn read_stopped(pid: u32) -> Result<bool, CuError> {
    agenterm_platform::process_metrics::is_stopped(pid).map_err(|error| {
        use agenterm_platform::process_metrics::ProcessMetricsErrorKind as Kind;
        let code = match error.kind() {
            Kind::InvalidId => "privilege_target_invalid",
            Kind::NotFound => "privilege_target_not_found",
            Kind::Unsupported => "privilege_operation_unsupported",
            _ => "privilege_target_unavailable",
        };
        CuError::new(code, "process scheduler state could not be observed")
            .with_detail(serde_json::json!({ "kind": format!("{:?}", error.kind()) }))
    })
}

fn validate_plan_lifetime(
    issued_at_utc_ms: u64,
    expires_at_utc_ms: u64,
    now_utc_ms: u64,
) -> Result<(), CuError> {
    let ttl_ms = expires_at_utc_ms
        .checked_sub(issued_at_utc_ms)
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
    if now_utc_ms < issued_at_utc_ms {
        return Err(CuError::new(
            "privilege_plan_not_yet_valid",
            "privilege plan issue time is later than the provider clock",
        ));
    }
    if now_utc_ms > expires_at_utc_ms {
        return Err(CuError::new(
            "privilege_plan_expired",
            "privilege plan expired before provider reservation",
        ));
    }
    Ok(())
}

fn plan_expiry(now_utc_ms: u64, ttl_seconds: u64) -> Result<u64, CuError> {
    let ttl_ms = ttl_seconds.checked_mul(1_000).ok_or_else(|| {
        CuError::new("privilege_plan_ttl_invalid", "privilege plan TTL overflows")
    })?;
    now_utc_ms.checked_add(ttl_ms).ok_or_else(|| {
        CuError::new(
            "privilege_plan_clock_invalid",
            "privilege plan expiry overflows the host clock",
        )
    })
}

fn now_utc_ms() -> Result<u64, CuError> {
    SystemTime::now()
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
        })
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn signal_plan_is_read_only_identity_bound_and_has_two_digest_scopes() {
        let pid = std::process::id();
        let first = process_signal_plan(
            pid,
            ProcessSignalKind::Terminate,
            false,
            false,
            5_000,
            32,
            120,
            1_000,
        )
        .unwrap();
        let repeated = process_signal_plan(
            pid,
            ProcessSignalKind::Terminate,
            false,
            false,
            5_000,
            32,
            120,
            1_000,
        )
        .unwrap();
        assert_eq!(first, repeated);
        assert_eq!(first.scope, ProcessSignalScope::Single);
        assert_eq!(first.members.len(), 1);
        assert_eq!(first.members[0].pid, pid);
        assert_eq!(first.members[0].start_identity, first.target.start_identity);
        assert!(!first.consent_requested);
        assert!(!first.mutation_performed);

        let later = process_signal_plan(
            pid,
            ProcessSignalKind::Terminate,
            false,
            false,
            5_000,
            32,
            120,
            2_000,
        )
        .unwrap();
        assert_eq!(later.contract_digest, first.contract_digest);
        assert_ne!(later.approval_digest, first.approval_digest);

        let changed_signal = process_signal_plan(
            pid,
            ProcessSignalKind::Kill,
            true,
            false,
            5_000,
            32,
            120,
            1_000,
        )
        .unwrap();
        assert_ne!(changed_signal.contract_digest, first.contract_digest);
        let changed_timeout = process_signal_plan(
            pid,
            ProcessSignalKind::Terminate,
            false,
            false,
            5_001,
            32,
            120,
            1_000,
        )
        .unwrap();
        assert_ne!(changed_timeout.contract_digest, first.contract_digest);
    }

    #[test]
    fn signal_plan_rejects_unbounded_or_incoherent_inputs_before_observation() {
        let cases = [
            process_signal_plan(0, ProcessSignalKind::Terminate, false, false, 1, 1, 1, 1),
            process_signal_plan(1, ProcessSignalKind::Kill, false, false, 1, 1, 1, 1),
            process_signal_plan(2, ProcessSignalKind::Terminate, true, false, 1, 1, 1, 1),
            process_signal_plan(2, ProcessSignalKind::Terminate, false, false, 0, 1, 1, 1),
            process_signal_plan(2, ProcessSignalKind::Terminate, false, true, 1, 0, 1, 1),
        ];
        for result in &cases[..2] {
            assert_eq!(
                result.as_ref().unwrap_err().code,
                "privilege_target_invalid"
            );
        }
        for result in &cases[2..] {
            assert_eq!(
                result.as_ref().unwrap_err().code,
                "privilege_parameter_invalid"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn signal_provider_validation_rejects_expiry_tampering_and_member_drift() {
        let pid = std::process::id();
        let plan = process_signal_plan(
            pid,
            ProcessSignalKind::Terminate,
            false,
            false,
            5_000,
            32,
            120,
            1_000,
        )
        .unwrap();
        validate_process_signal_plan(&plan, 1_001).unwrap();
        revalidate_process_signal_precondition(&plan).unwrap();

        let mut tampered = plan.clone();
        tampered.timeout_ms += 1;
        assert_eq!(
            validate_process_signal_plan(&tampered, 1_001)
                .unwrap_err()
                .code,
            "privilege_plan_digest_mismatch"
        );
        assert_eq!(
            validate_process_signal_plan(&plan, plan.expires_at_utc_ms + 1)
                .unwrap_err()
                .code,
            "privilege_plan_expired"
        );

        let mut changed_members = plan.members.clone();
        changed_members[0].before.stopped = !changed_members[0].before.stopped;
        let error =
            revalidate_process_signal_precondition_with(&plan, |_, _, _| Ok(changed_members))
                .unwrap_err();
        assert_eq!(error.code, "privilege_precondition_changed");

        let mut changed_identity = plan.members.clone();
        changed_identity[0].start_identity.push_str("-changed");
        let error =
            revalidate_process_signal_precondition_with(&plan, |_, _, _| Ok(changed_identity))
                .unwrap_err();
        assert_eq!(error.code, "privilege_precondition_changed");
    }

    #[test]
    fn signal_tree_plan_freezes_bounded_depth_and_exact_member_set() {
        let members = vec![
            ProcessSignalMember {
                pid: 41,
                depth: 0,
                parent_pid: None,
                start_identity: "proc-start-ticks:4100".into(),
                before: ProcessSignalBeforeState { stopped: false },
            },
            ProcessSignalMember {
                pid: 43,
                depth: 1,
                parent_pid: Some(41),
                start_identity: "proc-start-ticks:4300".into(),
                before: ProcessSignalBeforeState { stopped: true },
            },
        ];
        let plan = build_process_signal_plan(
            ProcessSignalKind::User1,
            false,
            5_000,
            1,
            ProcessSignalScope::Tree,
            members.clone(),
            1_000,
            121_000,
        )
        .unwrap();
        validate_process_signal_plan(&plan, 1_001).unwrap();
        assert_eq!(plan.target.pid, 41);
        assert_eq!(plan.target.start_identity, "proc-start-ticks:4100");
        assert_eq!(plan.members, members);
        revalidate_process_signal_precondition_with(&plan, |_, _, _| Ok(members.clone())).unwrap();

        let error = revalidate_process_signal_precondition_with(&plan, |_, _, _| {
            Ok(vec![members[0].clone()])
        })
        .unwrap_err();
        assert_eq!(error.code, "privilege_precondition_changed");

        let too_small = build_process_signal_plan(
            ProcessSignalKind::User1,
            false,
            5_000,
            1,
            ProcessSignalScope::Tree,
            vec![
                members[0].clone(),
                members[1].clone(),
                ProcessSignalMember {
                    pid: 47,
                    depth: 1,
                    parent_pid: Some(41),
                    start_identity: "proc-start-ticks:4700".into(),
                    before: ProcessSignalBeforeState { stopped: false },
                },
            ],
            1_000,
            121_000,
        )
        .unwrap_err();
        assert_eq!(too_small.code, "privilege_target_changed");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn signal_plan_serde_denies_unknown_fields_and_force_is_part_of_kill_contract() {
        let plan = process_signal_plan(
            std::process::id(),
            ProcessSignalKind::Kill,
            true,
            false,
            5_000,
            32,
            120,
            1_000,
        )
        .unwrap();
        validate_process_signal_plan(&plan, 1_001).unwrap();
        let mut value = serde_json::to_value(&plan).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), serde_json::json!(true));
        assert!(serde_json::from_value::<ProcessSignalPlan>(value).is_err());

        let mut invalid = plan;
        invalid.force = false;
        assert_eq!(
            validate_process_signal_plan(&invalid, 1_001)
                .unwrap_err()
                .code,
            "privilege_plan_invalid"
        );

        let mut invalid_identity = invalid;
        invalid_identity.force = true;
        invalid_identity.target.start_identity = "arbitrary\\\"json".into();
        invalid_identity.members[0].start_identity = invalid_identity.target.start_identity.clone();
        assert_eq!(
            validate_process_signal_plan(&invalid_identity, 1_001)
                .unwrap_err()
                .code,
            "privilege_plan_invalid"
        );
    }
}
