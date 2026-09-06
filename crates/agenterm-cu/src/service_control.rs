//! Owned service observation and approval-bound, at-most-once user mutation.
//!
//! The platform facade owns native provider mechanics. This module owns the
//! product boundary: serializable owned facts, short-lived plans, exact live
//! revalidation, durable reservation before effect, and fail-closed replay.
//! System services remain observable, but their mutation requires the separate
//! privileged-provider protocol and is never dispatched here.

use std::{
    io::Read as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use agenterm_platform::service as native;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    CuError,
    idempotency_store::{
        FinalOutcome, FinalOutcomeKind, FreshReservation, IdempotencyStore, RequestState,
        RequestStatus, ReserveDecision, fingerprint_canonical_request,
    },
};

const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_TTL_SECONDS: u64 = 120;
pub const MIN_TTL_SECONDS: u64 = 1;
pub const MAX_TTL_SECONDS: u64 = 600;
pub const MAX_DEFINITION_BYTES: usize = 1024 * 1024;
const REQUEST_BYTES_MAX: usize = 32 * 1024;
const REPLAY_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const CONTRACT_DOMAIN: &[u8] = b"agenterm-cu/service-contract/v1\0";
const APPROVAL_DOMAIN: &[u8] = b"agenterm-cu/service-approval/v1\0";
const OUTCOME_CHANGED: &str = "service_completed_changed";
const OUTCOME_NOOP: &str = "service_completed_noop";
const OUTCOME_NOT_PERFORMED: &str = "service_failed_not_performed";
const OUTCOME_ROLLED_BACK: &str = "service_failed_rolled_back";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    User,
    System,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceIdentity {
    pub scope: ServiceScope,
    pub provider: String,
    pub provider_scope: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceInstanceIdentity {
    pub provider: String,
    pub opaque: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Missing,
    LoadedInactive,
    Activating,
    Running,
    Deactivating,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSnapshot {
    pub identity: ServiceIdentity,
    pub instance: Option<ServiceInstanceIdentity>,
    pub state: ServiceState,
    pub substate: String,
    pub description: String,
    pub definition: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceList {
    pub scope: ServiceScope,
    pub services: Vec<ServiceSnapshot>,
    pub complete: bool,
    pub visited: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceOperation {
    Start,
    Stop,
    Restart,
    Bootstrap,
    Bootout,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDefinitionBinding {
    pub path: PathBuf,
    pub declared_name: String,
    pub byte_len: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServicePlan {
    pub schema_version: u32,
    pub operation: ServiceOperation,
    pub identity: ServiceIdentity,
    pub before: ServiceSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<ServiceDefinitionBinding>,
    pub issued_at_utc_ms: u64,
    pub expires_at_utc_ms: u64,
    pub contract_digest: String,
    pub approval_digest: String,
    pub mutation_performed: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceApplyState {
    Completed,
    Replayed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceApplyReply {
    pub state: ServiceApplyState,
    pub idempotent: bool,
    pub changed: bool,
    pub verified: bool,
    pub contract_digest: String,
    pub approval_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceMutationEvidence {
    pub before: ServiceSnapshot,
    pub after: ServiceSnapshot,
    pub performed: bool,
    pub verified: bool,
}

#[derive(Debug)]
pub enum ServiceMutationFailure {
    /// The provider proved no effect began.
    NotPerformed(CuError),
    /// The provider proved compensation restored the complete before snapshot.
    RolledBack(CuError),
    /// An effect may remain; automatic replay is permanently refused.
    OutcomeUnknown(CuError),
}

pub trait ServiceProvider {
    fn identity(&mut self, scope: ServiceScope, name: &str) -> Result<ServiceIdentity, CuError>;
    fn list(
        &mut self,
        scope: ServiceScope,
        match_text: Option<&str>,
        max_items: usize,
    ) -> Result<ServiceList, CuError>;
    fn status(&mut self, identity: &ServiceIdentity) -> Result<ServiceSnapshot, CuError>;
    fn definition_identity(&mut self, path: &Path) -> Result<ServiceDefinitionBinding, CuError>;
    fn mutate(
        &mut self,
        operation: ServiceOperation,
        before: &ServiceSnapshot,
        definition: Option<&Path>,
    ) -> Result<ServiceMutationEvidence, ServiceMutationFailure>;
    fn now_utc_ms(&mut self) -> Result<u64, CuError>;
}

#[derive(Default)]
pub struct NativeServiceProvider;

impl ServiceProvider for NativeServiceProvider {
    fn identity(&mut self, scope: ServiceScope, name: &str) -> Result<ServiceIdentity, CuError> {
        native::identity(native_scope(scope), name, native::SERVICE_DEFAULT_DEADLINE)
            .map(project_identity)
            .map_err(platform_error)
    }

    fn list(
        &mut self,
        scope: ServiceScope,
        match_text: Option<&str>,
        max_items: usize,
    ) -> Result<ServiceList, CuError> {
        let result = native::list(
            native_scope(scope),
            native::ServiceListBudget {
                max_items,
                deadline: native::SERVICE_DEFAULT_DEADLINE,
                match_text: match_text.map(str::to_owned),
            },
        )
        .map_err(platform_error)?;
        Ok(ServiceList {
            scope,
            services: result.services.into_iter().map(project_snapshot).collect(),
            complete: result.complete,
            visited: result.visited,
        })
    }

    fn status(&mut self, identity: &ServiceIdentity) -> Result<ServiceSnapshot, CuError> {
        native::status(
            &native_identity(identity)?,
            native::SERVICE_DEFAULT_DEADLINE,
        )
        .map(project_snapshot)
        .map_err(platform_error)
    }

    fn definition_identity(&mut self, path: &Path) -> Result<ServiceDefinitionBinding, CuError> {
        let canonical = std::fs::canonicalize(path).map_err(|_| {
            CuError::new(
                "service_definition_invalid",
                "service definition path cannot be resolved",
            )
        })?;
        let before = definition_identity(&canonical)?;
        let declared_name = native::definition_name(&canonical, native::SERVICE_DEFAULT_DEADLINE)
            .map_err(platform_error)?;
        let after = definition_identity(&canonical)?;
        if before != after {
            return Err(CuError::new(
                "service_definition_changed",
                "service definition changed while its native identity was read",
            ));
        }
        Ok(ServiceDefinitionBinding {
            path: canonical,
            declared_name,
            byte_len: after.byte_len,
            sha256: after.sha256,
        })
    }

    fn mutate(
        &mut self,
        operation: ServiceOperation,
        before: &ServiceSnapshot,
        definition: Option<&Path>,
    ) -> Result<ServiceMutationEvidence, ServiceMutationFailure> {
        let expected_before =
            native_snapshot(before).map_err(ServiceMutationFailure::NotPerformed)?;
        let request = native::ServiceMutationRequest {
            operation: native_operation(operation),
            expected_before,
            definition: definition.map(Path::to_owned),
            deadline: native::SERVICE_DEFAULT_DEADLINE,
        };
        native::mutate(&request)
            .map(|receipt| ServiceMutationEvidence {
                before: project_snapshot(receipt.before),
                after: project_snapshot(receipt.after),
                performed: receipt.performed,
                verified: receipt.verified,
            })
            .map_err(|error| {
                let known_no_effect = error.effect() == native::ServiceEffect::NotPerformed;
                let rolled_back = error.rollback() == native::ServiceRollback::Verified;
                let typed = platform_error(error);
                if known_no_effect {
                    ServiceMutationFailure::NotPerformed(typed)
                } else if rolled_back {
                    ServiceMutationFailure::RolledBack(typed)
                } else {
                    ServiceMutationFailure::OutcomeUnknown(typed)
                }
            })
    }

    fn now_utc_ms(&mut self) -> Result<u64, CuError> {
        now_utc_ms()
    }
}

/// Resolve an old service label into the complete current native provider and
/// authority-domain identity. This performs no status query or mutation.
pub fn identity(scope: ServiceScope, name: &str) -> Result<ServiceIdentity, CuError> {
    identity_with_provider(&mut NativeServiceProvider, scope, name)
}

pub fn identity_with_provider(
    provider: &mut impl ServiceProvider,
    scope: ServiceScope,
    name: &str,
) -> Result<ServiceIdentity, CuError> {
    validate_text(name, false)?;
    let identity = provider.identity(scope, name)?;
    validate_identity(&identity)?;
    if identity.scope != scope || identity.name != name {
        return Err(CuError::new(
            "service_identity_changed",
            "service provider resolved a different scope or name",
        ));
    }
    Ok(identity)
}

pub fn list(
    scope: ServiceScope,
    match_text: Option<&str>,
    max_items: usize,
) -> Result<ServiceList, CuError> {
    list_with_provider(&mut NativeServiceProvider, scope, match_text, max_items)
}

pub fn list_with_provider(
    provider: &mut impl ServiceProvider,
    scope: ServiceScope,
    match_text: Option<&str>,
    max_items: usize,
) -> Result<ServiceList, CuError> {
    if !(1..=native::SERVICE_MAX_ITEMS).contains(&max_items) {
        return Err(CuError::new(
            "service_list_limit_invalid",
            "service max_items must be in 1..=5000",
        ));
    }
    if match_text.is_some_and(|value| {
        value.len() > native::SERVICE_FIELD_MAX_BYTES || value.chars().any(char::is_control)
    }) {
        return Err(CuError::new(
            "service_match_invalid",
            "service match text is oversized or contains control characters",
        ));
    }
    let result = provider.list(scope, match_text, max_items)?;
    validate_list(&result, scope, max_items)?;
    Ok(result)
}

pub fn status(identity: &ServiceIdentity) -> Result<ServiceSnapshot, CuError> {
    status_with_provider(&mut NativeServiceProvider, identity)
}

/// Resolve one provider-qualified identity from its label and return its exact
/// status, without asking callers to guess a launchd/systemd authority domain.
pub fn status_named(scope: ServiceScope, name: &str) -> Result<ServiceSnapshot, CuError> {
    let mut provider = NativeServiceProvider;
    let identity = identity_with_provider(&mut provider, scope, name)?;
    status_with_provider(&mut provider, &identity)
}

/// Read the bounded native identity declared by one service definition without
/// exposing its contents or performing a lifecycle effect.
pub fn definition_binding(path: &Path) -> Result<ServiceDefinitionBinding, CuError> {
    let mut provider = NativeServiceProvider;
    provider.definition_identity(path)
}

pub fn status_with_provider(
    provider: &mut impl ServiceProvider,
    identity: &ServiceIdentity,
) -> Result<ServiceSnapshot, CuError> {
    validate_identity(identity)?;
    let result = provider.status(identity)?;
    validate_snapshot(&result)?;
    if result.identity != *identity {
        return Err(CuError::new(
            "service_identity_changed",
            "service provider returned a different identity",
        ));
    }
    Ok(result)
}

pub fn plan(
    identity: &ServiceIdentity,
    operation: ServiceOperation,
    definition: Option<PathBuf>,
    ttl_seconds: u64,
) -> Result<ServicePlan, CuError> {
    plan_with_provider(
        &mut NativeServiceProvider,
        identity,
        operation,
        definition,
        ttl_seconds,
    )
}

pub fn plan_with_provider(
    provider: &mut impl ServiceProvider,
    identity: &ServiceIdentity,
    operation: ServiceOperation,
    definition: Option<PathBuf>,
    ttl_seconds: u64,
) -> Result<ServicePlan, CuError> {
    validate_identity(identity)?;
    require_user_mutation(identity)?;
    validate_ttl(ttl_seconds)?;
    let before = status_with_provider(provider, identity)?;
    if operation == ServiceOperation::Bootstrap && before.state != ServiceState::Missing {
        return Err(CuError::new(
            "service_already_loaded",
            "bootstrap refuses a service that is already loaded",
        ));
    }
    if operation != ServiceOperation::Bootstrap && before.state == ServiceState::Missing {
        return Err(CuError::new(
            "service_not_loaded",
            "service lifecycle refuses a service that is not loaded",
        ));
    }
    let definition_path = definition;
    if matches!(operation, ServiceOperation::Bootstrap) && definition_path.is_none() {
        return Err(CuError::new(
            "service_definition_required",
            "bootstrap requires an explicit regular definition file",
        ));
    }
    let definition = definition_path
        .as_deref()
        .map(|path| provider.definition_identity(path))
        .transpose()?;
    if definition
        .as_ref()
        .is_some_and(|binding| binding.declared_name != identity.name)
    {
        return Err(CuError::new(
            "service_definition_identity_mismatch",
            "service definition declares a different native service name",
        ));
    }
    let issued_at_utc_ms = provider.now_utc_ms()?;
    let expires_at_utc_ms = issued_at_utc_ms
        .checked_add(ttl_seconds.checked_mul(1_000).ok_or_else(|| {
            CuError::new("service_plan_ttl_invalid", "service plan TTL overflows")
        })?)
        .ok_or_else(|| {
            CuError::new(
                "service_clock_invalid",
                "service plan expiry overflows the host clock",
            )
        })?;
    let mut plan = ServicePlan {
        schema_version: SCHEMA_VERSION,
        operation,
        identity: identity.clone(),
        before,
        definition,
        issued_at_utc_ms,
        expires_at_utc_ms,
        contract_digest: String::new(),
        approval_digest: String::new(),
        mutation_performed: false,
    };
    plan.contract_digest = contract_digest(&plan)?;
    plan.approval_digest = approval_digest(&plan)?;
    Ok(plan)
}

pub fn encode_request(plan: &ServicePlan) -> Result<String, CuError> {
    let bytes = serde_json::to_vec(plan).map_err(|_| {
        CuError::new(
            "service_request_invalid",
            "service plan cannot be serialized",
        )
    })?;
    if bytes.len() > REQUEST_BYTES_MAX {
        return Err(CuError::new(
            "service_request_limit",
            "service request exceeds its byte ceiling",
        ));
    }
    Ok(crate::managed_job_ipc::base64_encode(&bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_owned())
}

pub fn decode_request(encoded: &str) -> Result<ServicePlan, CuError> {
    if encoded.is_empty()
        || encoded.len() > REQUEST_BYTES_MAX * 2
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CuError::new(
            "service_request_invalid",
            "service request is not bounded unpadded base64url",
        ));
    }
    let mut standard = encoded.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let bytes = crate::managed_job_ipc::base64_decode(&standard)
        .map_err(|_| CuError::new("service_request_invalid", "service request is malformed"))?;
    if bytes.is_empty() || bytes.len() > REQUEST_BYTES_MAX {
        return Err(CuError::new(
            "service_request_limit",
            "decoded service request exceeds its byte ceiling",
        ));
    }
    let canonical = crate::managed_job_ipc::base64_encode(&bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_owned();
    if canonical != encoded {
        return Err(CuError::new(
            "service_request_invalid",
            "service request is not canonical unpadded base64url",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        CuError::new(
            "service_request_invalid",
            "service request has an invalid shape",
        )
    })
}

pub fn apply(plan: &ServicePlan, approval_digest: &str) -> Result<ServiceApplyReply, CuError> {
    let store = IdempotencyStore::open()?;
    apply_with_provider(&mut NativeServiceProvider, &store, plan, approval_digest)
}

pub fn apply_with_provider(
    provider: &mut impl ServiceProvider,
    store: &IdempotencyStore,
    plan: &ServicePlan,
    approval_digest: &str,
) -> Result<ServiceApplyReply, CuError> {
    validate_plan_integrity(plan, approval_digest)?;
    require_user_mutation(&plan.identity)?;
    let canonical = serde_json::to_vec(plan).map_err(|_| {
        CuError::new(
            "service_plan_invalid",
            "service plan could not be serialized canonically",
        )
    })?;
    let fingerprint = fingerprint_canonical_request(&canonical)?;
    let now = provider.now_utc_ms()?;

    // Terminal and uncertain receipts outrank the short approval lifetime and
    // all live provider checks. A completed request remains replayable without
    // touching the provider; an uncertain request can never dispatch again.
    if let Some(status) = store.lookup(&plan.approval_digest, &fingerprint, to_i64(now)?)? {
        return replay(plan, status);
    }
    if now >= plan.expires_at_utc_ms {
        return Err(CuError::new(
            "service_plan_expired",
            "service approval expired before reservation",
        ));
    }
    let live = status_with_provider(provider, &plan.identity)?;
    if live != plan.before {
        return Err(CuError::new(
            "service_state_changed",
            "the complete service identity or before snapshot changed after planning",
        ));
    }
    if let Some(expected) = &plan.definition {
        let current = provider.definition_identity(&expected.path)?;
        if current != *expected {
            return Err(CuError::new(
                "service_definition_changed",
                "the service definition bytes changed after planning",
            ));
        }
    }

    let reservation = match store.reserve(
        &plan.approval_digest,
        &fingerprint,
        REPLAY_RETENTION_MS,
        to_i64(now)?,
    )? {
        ReserveDecision::Fresh(fresh) => fresh,
        ReserveDecision::ReplayFinalized(status) => return replay(plan, status),
        ReserveDecision::Uncertain(_) => return Err(outcome_unknown()),
    };

    if operation_already_satisfied(plan.operation, &live) {
        finalize(
            store,
            plan,
            &fingerprint,
            &reservation,
            FinalOutcomeKind::Succeeded,
            OUTCOME_NOOP,
            now,
        )?;
        return Ok(reply(plan, ServiceApplyState::Completed, false, false));
    }

    match provider.mutate(
        plan.operation,
        &live,
        plan.definition.as_ref().map(|value| value.path.as_path()),
    ) {
        Ok(evidence)
            if evidence.before == plan.before
                && evidence.verified
                && postcondition(plan.operation, &plan.before, &evidence.after) =>
        {
            finalize(
                store,
                plan,
                &fingerprint,
                &reservation,
                FinalOutcomeKind::Succeeded,
                OUTCOME_CHANGED,
                now,
            )?;
            Ok(reply(
                plan,
                ServiceApplyState::Completed,
                false,
                evidence.performed,
            ))
        }
        Ok(_) => Err(mark_unknown(
            store,
            plan,
            &fingerprint,
            &reservation,
            now,
            "service_verification_failed",
        )),
        Err(ServiceMutationFailure::NotPerformed(error)) => finalize_failure(
            store,
            plan,
            &fingerprint,
            &reservation,
            OUTCOME_NOT_PERFORMED,
            now,
            error,
            "not_applied",
        ),
        Err(ServiceMutationFailure::RolledBack(error)) => finalize_failure(
            store,
            plan,
            &fingerprint,
            &reservation,
            OUTCOME_ROLLED_BACK,
            now,
            error,
            "verified_rollback",
        ),
        Err(ServiceMutationFailure::OutcomeUnknown(error)) => Err(mark_unknown(
            store,
            plan,
            &fingerprint,
            &reservation,
            now,
            &error.code,
        )),
    }
}

fn validate_plan_integrity(plan: &ServicePlan, approval: &str) -> Result<(), CuError> {
    validate_identity(&plan.identity)?;
    validate_snapshot(&plan.before)?;
    if let Some(definition) = &plan.definition {
        validate_definition_binding(definition)?;
    }
    let lifetime = plan.expires_at_utc_ms.checked_sub(plan.issued_at_utc_ms);
    if plan.schema_version != SCHEMA_VERSION
        || plan.before.identity != plan.identity
        || plan.mutation_performed
        || lifetime.is_none_or(|value| {
            !(MIN_TTL_SECONDS * 1_000..=MAX_TTL_SECONDS * 1_000).contains(&value)
        })
        || matches!(plan.operation, ServiceOperation::Bootstrap) && plan.definition.is_none()
        || plan.contract_digest != contract_digest(plan)?
        || plan.approval_digest != approval_digest(plan)?
        || approval != plan.approval_digest
    {
        return Err(CuError::new(
            "service_approval_mismatch",
            "service plan content does not match its canonical approval",
        ));
    }
    Ok(())
}

fn validate_list(
    list: &ServiceList,
    expected_scope: ServiceScope,
    max_items: usize,
) -> Result<(), CuError> {
    if list.scope != expected_scope
        || list.services.len() > max_items
        || list.visited < list.services.len()
        || list.complete != (list.visited <= max_items)
    {
        return Err(CuError::new(
            "service_provider_shape",
            "service provider returned inconsistent bounded inventory metadata",
        ));
    }
    for service in &list.services {
        validate_snapshot(service)?;
        if service.identity.scope != expected_scope {
            return Err(CuError::new(
                "service_provider_shape",
                "service provider returned an entry from another scope",
            ));
        }
    }
    Ok(())
}

fn validate_snapshot(snapshot: &ServiceSnapshot) -> Result<(), CuError> {
    validate_identity(&snapshot.identity)?;
    if let Some(instance) = &snapshot.instance {
        validate_text(&instance.provider, false)?;
        validate_text(&instance.opaque, false)?;
    }
    validate_text(&snapshot.substate, true)?;
    validate_text(&snapshot.description, true)?;
    Ok(())
}

fn validate_identity(identity: &ServiceIdentity) -> Result<(), CuError> {
    validate_text(&identity.provider, false)?;
    validate_text(&identity.provider_scope, false)?;
    validate_text(&identity.name, false)
}

fn validate_text(value: &str, empty_allowed: bool) -> Result<(), CuError> {
    if (!empty_allowed && value.is_empty())
        || value.len() > native::SERVICE_FIELD_MAX_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(CuError::new(
            "service_provider_shape",
            "service provider returned an empty, oversized, or control-bearing field",
        ));
    }
    Ok(())
}

fn validate_definition_binding(binding: &ServiceDefinitionBinding) -> Result<(), CuError> {
    if binding.path.as_os_str().is_empty()
        || binding.declared_name.is_empty()
        || binding.declared_name.len() > native::SERVICE_FIELD_MAX_BYTES
        || binding.declared_name.chars().any(char::is_control)
        || binding.byte_len > MAX_DEFINITION_BYTES as u64
        || !is_digest(&binding.sha256)
    {
        return Err(CuError::new(
            "service_definition_invalid",
            "service definition binding is empty, oversized, or has an invalid digest",
        ));
    }
    Ok(())
}

fn validate_ttl(ttl_seconds: u64) -> Result<(), CuError> {
    if !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&ttl_seconds) {
        Err(CuError::new(
            "service_plan_ttl_invalid",
            "service plan TTL must be in 1..=600 seconds",
        ))
    } else {
        Ok(())
    }
}

fn require_user_mutation(identity: &ServiceIdentity) -> Result<(), CuError> {
    if identity.scope == ServiceScope::System {
        Err(CuError::new(
            "service_requires_privilege",
            "system service mutation requires the typed privileged provider",
        )
        .with_detail(serde_json::json!({
            "requires_privilege": true,
            "effect": "not_applied",
        })))
    } else {
        Ok(())
    }
}

fn definition_identity(path: &Path) -> Result<ServiceDefinitionBinding, CuError> {
    let mut file = agenterm_platform::filesystem_open::open_existing(
        path,
        agenterm_platform::filesystem_open::ExistingEntryType::File,
    )
    .map_err(|_| {
        CuError::new(
            "service_definition_invalid",
            "service definition must be a readable regular file, not a final symlink",
        )
    })?;
    let metadata = file.metadata().map_err(|_| {
        CuError::new(
            "service_definition_unavailable",
            "service definition metadata is unavailable",
        )
    })?;
    if !metadata.is_file() || metadata.len() > MAX_DEFINITION_BYTES as u64 {
        return Err(CuError::new(
            "service_definition_limit",
            "service definition is not regular or exceeds its byte ceiling",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take((MAX_DEFINITION_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            CuError::new(
                "service_definition_unavailable",
                "service definition could not be read",
            )
        })?;
    if bytes.len() > MAX_DEFINITION_BYTES {
        return Err(CuError::new(
            "service_definition_limit",
            "service definition exceeds its byte ceiling",
        ));
    }
    Ok(ServiceDefinitionBinding {
        path: path.to_owned(),
        declared_name: String::new(),
        byte_len: bytes.len() as u64,
        sha256: hex(&Sha256::digest(&bytes)),
    })
}

#[derive(Serialize)]
struct Contract<'a> {
    schema_version: u32,
    operation: ServiceOperation,
    identity: &'a ServiceIdentity,
    before: &'a ServiceSnapshot,
    definition: &'a Option<ServiceDefinitionBinding>,
}

#[derive(Serialize)]
struct Approval<'a> {
    contract_digest: &'a str,
    issued_at_utc_ms: u64,
    expires_at_utc_ms: u64,
}

fn contract_digest(plan: &ServicePlan) -> Result<String, CuError> {
    digest(
        CONTRACT_DOMAIN,
        &Contract {
            schema_version: plan.schema_version,
            operation: plan.operation,
            identity: &plan.identity,
            before: &plan.before,
            definition: &plan.definition,
        },
    )
}

fn approval_digest(plan: &ServicePlan) -> Result<String, CuError> {
    digest(
        APPROVAL_DOMAIN,
        &Approval {
            contract_digest: &plan.contract_digest,
            issued_at_utc_ms: plan.issued_at_utc_ms,
            expires_at_utc_ms: plan.expires_at_utc_ms,
        },
    )
}

fn digest(domain: &[u8], value: &impl Serialize) -> Result<String, CuError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        CuError::new(
            "service_plan_invalid",
            "service plan projection could not be serialized canonically",
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    Ok(hex(&hasher.finalize()))
}

fn finalize(
    store: &IdempotencyStore,
    plan: &ServicePlan,
    fingerprint: &str,
    reservation: &FreshReservation,
    kind: FinalOutcomeKind,
    code: &str,
    now: u64,
) -> Result<(), CuError> {
    store.finalize(
        &plan.approval_digest,
        fingerprint,
        &reservation.completion_token,
        FinalOutcome::new(kind, code, None)?,
        to_i64(now)?,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finalize_failure(
    store: &IdempotencyStore,
    plan: &ServicePlan,
    fingerprint: &str,
    reservation: &FreshReservation,
    code: &str,
    now: u64,
    error: CuError,
    recovery: &str,
) -> Result<ServiceApplyReply, CuError> {
    finalize(
        store,
        plan,
        fingerprint,
        reservation,
        FinalOutcomeKind::Failed,
        code,
        now,
    )?;
    Err(error.with_detail(serde_json::json!({
        "effect": "not_applied",
        "recovery": recovery,
    })))
}

fn mark_unknown(
    store: &IdempotencyStore,
    plan: &ServicePlan,
    fingerprint: &str,
    reservation: &FreshReservation,
    now: u64,
    cause: &str,
) -> CuError {
    let persisted = store
        .mark_outcome_unknown(
            &plan.approval_digest,
            fingerprint,
            &reservation.completion_token,
            i64::try_from(now).unwrap_or(i64::MAX),
        )
        .is_ok();
    CuError::new(
        "service_effect_uncertain",
        "service mutation may have been applied; automatic replay is refused",
    )
    .with_detail(serde_json::json!({
        "effect": "unknown",
        "cause": cause,
        "uncertain_persisted": persisted,
    }))
}

fn replay(plan: &ServicePlan, status: RequestStatus) -> Result<ServiceApplyReply, CuError> {
    match status.state {
        RequestState::Reserved | RequestState::OutcomeUnknown => return Err(outcome_unknown()),
        RequestState::Finalized => {}
    }
    let outcome = status.outcome.ok_or_else(|| {
        CuError::new(
            "service_receipt_invalid",
            "finalized service receipt has no outcome",
        )
    })?;
    if outcome.kind == FinalOutcomeKind::Failed {
        return Err(CuError::new(
            "service_previously_failed",
            "this service approval already has a terminal failed receipt",
        )
        .with_detail(serde_json::json!({
            "effect": "not_repeated",
            "outcome": outcome.code,
        })));
    }
    match outcome.code.as_str() {
        OUTCOME_CHANGED => Ok(reply(plan, ServiceApplyState::Replayed, true, true)),
        OUTCOME_NOOP => Ok(reply(plan, ServiceApplyState::Replayed, true, false)),
        _ => Err(CuError::new(
            "service_receipt_invalid",
            "service receipt has an unknown terminal outcome",
        )),
    }
}

fn reply(
    plan: &ServicePlan,
    state: ServiceApplyState,
    idempotent: bool,
    changed: bool,
) -> ServiceApplyReply {
    ServiceApplyReply {
        state,
        idempotent,
        changed,
        verified: true,
        contract_digest: plan.contract_digest.clone(),
        approval_digest: plan.approval_digest.clone(),
    }
}

fn outcome_unknown() -> CuError {
    CuError::new(
        "service_effect_uncertain",
        "this service approval may already have been applied; automatic replay is refused",
    )
    .with_detail(serde_json::json!({ "effect": "unknown" }))
}

fn operation_already_satisfied(operation: ServiceOperation, state: &ServiceSnapshot) -> bool {
    matches!(operation, ServiceOperation::Start) && state.state == ServiceState::Running
        || matches!(
            operation,
            ServiceOperation::Stop | ServiceOperation::Bootout
        ) && state.state == ServiceState::Missing
}

fn postcondition(
    operation: ServiceOperation,
    before: &ServiceSnapshot,
    state: &ServiceSnapshot,
) -> bool {
    match operation {
        ServiceOperation::Start => state.state == ServiceState::Running,
        ServiceOperation::Restart => {
            state.state == ServiceState::Running
                && state.instance.is_some()
                && state.instance != before.instance
        }
        ServiceOperation::Stop => !matches!(
            state.state,
            ServiceState::Running | ServiceState::Activating | ServiceState::Deactivating
        ),
        ServiceOperation::Bootstrap => state.state != ServiceState::Missing,
        ServiceOperation::Bootout => state.state == ServiceState::Missing,
    }
}

fn native_scope(scope: ServiceScope) -> native::ServiceScope {
    match scope {
        ServiceScope::User => native::ServiceScope::User,
        ServiceScope::System => native::ServiceScope::System,
    }
}

fn project_scope(scope: native::ServiceScope) -> ServiceScope {
    match scope {
        native::ServiceScope::User => ServiceScope::User,
        native::ServiceScope::System => ServiceScope::System,
    }
}

fn native_provider(value: &str) -> Result<&'static str, CuError> {
    match value {
        "launchd" => Ok("launchd"),
        "systemd" => Ok("systemd"),
        _ => Err(CuError::new(
            "service_provider_invalid",
            "service identity names an unavailable native provider",
        )),
    }
}

fn native_identity(value: &ServiceIdentity) -> Result<native::ServiceIdentity, CuError> {
    Ok(native::ServiceIdentity {
        scope: native_scope(value.scope),
        provider: native_provider(&value.provider)?,
        provider_scope: value.provider_scope.clone(),
        name: value.name.clone(),
    })
}

fn native_snapshot(value: &ServiceSnapshot) -> Result<native::ServiceSnapshot, CuError> {
    Ok(native::ServiceSnapshot {
        identity: native_identity(&value.identity)?,
        instance: value
            .instance
            .as_ref()
            .map(|instance| -> Result<_, CuError> {
                Ok(native::ServiceInstanceIdentity {
                    provider: native_provider(&instance.provider)?,
                    opaque: instance.opaque.clone(),
                })
            })
            .transpose()?,
        state: match value.state {
            ServiceState::Missing => native::ServiceState::Missing,
            ServiceState::LoadedInactive => native::ServiceState::LoadedInactive,
            ServiceState::Activating => native::ServiceState::Activating,
            ServiceState::Running => native::ServiceState::Running,
            ServiceState::Deactivating => native::ServiceState::Deactivating,
            ServiceState::Failed => native::ServiceState::Failed,
            ServiceState::Unknown => native::ServiceState::Unknown,
        },
        substate: value.substate.clone(),
        description: value.description.clone(),
        definition: value.definition.clone(),
    })
}

fn project_snapshot(value: native::ServiceSnapshot) -> ServiceSnapshot {
    ServiceSnapshot {
        identity: project_identity(value.identity),
        instance: value.instance.map(|instance| ServiceInstanceIdentity {
            provider: instance.provider.into(),
            opaque: instance.opaque,
        }),
        state: match value.state {
            native::ServiceState::Missing => ServiceState::Missing,
            native::ServiceState::LoadedInactive => ServiceState::LoadedInactive,
            native::ServiceState::Activating => ServiceState::Activating,
            native::ServiceState::Running => ServiceState::Running,
            native::ServiceState::Deactivating => ServiceState::Deactivating,
            native::ServiceState::Failed => ServiceState::Failed,
            native::ServiceState::Unknown => ServiceState::Unknown,
        },
        substate: value.substate,
        description: value.description,
        definition: value.definition,
    }
}

fn project_identity(value: native::ServiceIdentity) -> ServiceIdentity {
    ServiceIdentity {
        scope: project_scope(value.scope),
        provider: value.provider.into(),
        provider_scope: value.provider_scope,
        name: value.name,
    }
}

fn native_operation(operation: ServiceOperation) -> native::ServiceOperation {
    match operation {
        ServiceOperation::Start => native::ServiceOperation::Start,
        ServiceOperation::Stop => native::ServiceOperation::Stop,
        ServiceOperation::Restart => native::ServiceOperation::Restart,
        ServiceOperation::Bootstrap => native::ServiceOperation::Bootstrap,
        ServiceOperation::Bootout => native::ServiceOperation::Bootout,
    }
}

fn platform_error(error: native::ServiceError) -> CuError {
    let code = match error.kind() {
        native::ServiceErrorKind::Unsupported => "service_unsupported",
        native::ServiceErrorKind::RequiresPrivilege => "service_requires_privilege",
        native::ServiceErrorKind::InvalidRequest => "service_request_invalid",
        native::ServiceErrorKind::InvalidNativeValue => "service_provider_shape",
        native::ServiceErrorKind::InventoryTooLarge => "service_inventory_limit",
        native::ServiceErrorKind::TimedOut => "service_timed_out",
        native::ServiceErrorKind::QueryFailed => "service_query_failed",
        native::ServiceErrorKind::StateChanged => "service_state_changed",
        native::ServiceErrorKind::MutationFailed => "service_mutation_failed",
        native::ServiceErrorKind::VerificationFailed => "service_verification_failed",
        _ => "service_provider_failed",
    };
    CuError::new(code, error.detail().to_owned())
}

fn now_utc_ms() -> Result<u64, CuError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CuError::new("service_clock_invalid", "host clock is before Unix epoch"))?
        .as_millis()
        .try_into()
        .map_err(|_| {
            CuError::new(
                "service_clock_invalid",
                "host clock does not fit the service contract",
            )
        })
}

fn to_i64(value: u64) -> Result<i64, CuError> {
    value.try_into().map_err(|_| {
        CuError::new(
            "service_clock_invalid",
            "service timestamp exceeds receipt range",
        )
    })
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, fs};

    use super::*;

    struct TestPath(PathBuf);

    impl TestPath {
        fn new(name: &str) -> Self {
            let path = std::env::current_dir()
                .unwrap()
                .join("target/service-control-test-state")
                .join(format!("{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestPath {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        state: ServiceSnapshot,
        definition: ServiceDefinitionBinding,
        definition_reads: usize,
        now: u64,
        mutations: usize,
        results: VecDeque<Result<ServiceMutationEvidence, ServiceMutationFailure>>,
    }

    impl ServiceProvider for Fixture {
        fn identity(
            &mut self,
            scope: ServiceScope,
            name: &str,
        ) -> Result<ServiceIdentity, CuError> {
            let mut identity = self.state.identity.clone();
            identity.scope = scope;
            identity.name = name.into();
            Ok(identity)
        }

        fn list(
            &mut self,
            scope: ServiceScope,
            _match_text: Option<&str>,
            _max_items: usize,
        ) -> Result<ServiceList, CuError> {
            Ok(ServiceList {
                scope,
                services: vec![self.state.clone()],
                complete: true,
                visited: 1,
            })
        }

        fn status(&mut self, _identity: &ServiceIdentity) -> Result<ServiceSnapshot, CuError> {
            Ok(self.state.clone())
        }

        fn definition_identity(
            &mut self,
            path: &Path,
        ) -> Result<ServiceDefinitionBinding, CuError> {
            self.definition_reads += 1;
            let mut value = self.definition.clone();
            value.path = path.to_owned();
            Ok(value)
        }

        fn mutate(
            &mut self,
            _operation: ServiceOperation,
            before: &ServiceSnapshot,
            _definition: Option<&Path>,
        ) -> Result<ServiceMutationEvidence, ServiceMutationFailure> {
            assert_eq!(before, &self.state);
            self.mutations += 1;
            self.results.pop_front().expect("fixture mutation result")
        }

        fn now_utc_ms(&mut self) -> Result<u64, CuError> {
            Ok(self.now)
        }
    }

    fn identity(scope: ServiceScope) -> ServiceIdentity {
        ServiceIdentity {
            scope,
            provider: "fixture".into(),
            provider_scope: "fixture-user".into(),
            name: "example.service".into(),
        }
    }

    fn snapshot(scope: ServiceScope, state: ServiceState) -> ServiceSnapshot {
        ServiceSnapshot {
            identity: identity(scope),
            instance: Some(ServiceInstanceIdentity {
                provider: "fixture".into(),
                opaque: "incarnation-1".into(),
            }),
            state,
            substate: "fixture".into(),
            description: "Example".into(),
            definition: Some(PathBuf::from("fixture.service")),
        }
    }

    fn evidence(before: &ServiceSnapshot, after_state: ServiceState) -> ServiceMutationEvidence {
        let mut after = before.clone();
        after.state = after_state;
        ServiceMutationEvidence {
            before: before.clone(),
            after,
            performed: true,
            verified: true,
        }
    }

    fn fixture() -> Fixture {
        let state = snapshot(ServiceScope::User, ServiceState::LoadedInactive);
        Fixture {
            definition: ServiceDefinitionBinding {
                path: PathBuf::from("fixture.service"),
                declared_name: "example.service".into(),
                byte_len: 7,
                sha256: "11".repeat(32),
            },
            definition_reads: 0,
            now: 1_000,
            mutations: 0,
            results: VecDeque::from([Ok(evidence(&state, ServiceState::Running))]),
            state,
        }
    }

    fn store(name: &str) -> (TestPath, IdempotencyStore) {
        let root = TestPath::new(name);
        let store = IdempotencyStore::open_at(root.0.join("requests.json")).unwrap();
        (root, store)
    }

    #[test]
    fn plan_is_read_only_owned_and_binds_complete_state_and_definition() {
        let mut provider = fixture();
        let resolved =
            identity_with_provider(&mut provider, ServiceScope::User, "example.service").unwrap();
        assert_eq!(resolved, identity(ServiceScope::User));
        provider.state.state = ServiceState::Missing;
        let plan = plan_with_provider(
            &mut provider,
            &identity(ServiceScope::User),
            ServiceOperation::Bootstrap,
            Some(PathBuf::from("fixture.service")),
            120,
        )
        .unwrap();
        assert_eq!(provider.mutations, 0);
        assert_eq!(plan.before, provider.state);
        assert_eq!(plan.identity, provider.state.identity);
        assert_eq!(plan.definition.as_ref().unwrap().sha256, "11".repeat(32));
        assert_eq!(plan.expires_at_utc_ms - plan.issued_at_utc_ms, 120_000);
        assert_ne!(plan.contract_digest, plan.approval_digest);
        assert!(!plan.mutation_performed);
        assert_eq!(provider.definition_reads, 1);
        let json = serde_json::to_string(&plan).unwrap();
        assert!(!json.contains("file contents"));
        let encoded = encode_request(&plan).unwrap();
        assert_eq!(decode_request(&encoded).unwrap(), plan);
        assert!(decode_request(&format!("{encoded}=")).is_err());
    }

    #[test]
    fn bootstrap_refuses_loaded_or_mismatched_definition_and_bootout_does_not_read_it() {
        let mut provider = fixture();
        assert_eq!(
            plan_with_provider(
                &mut provider,
                &identity(ServiceScope::User),
                ServiceOperation::Bootstrap,
                Some(PathBuf::from("fixture.service")),
                120,
            )
            .unwrap_err()
            .code,
            "service_already_loaded"
        );
        assert_eq!(provider.definition_reads, 0);

        let bootout = plan_with_provider(
            &mut provider,
            &identity(ServiceScope::User),
            ServiceOperation::Bootout,
            None,
            120,
        )
        .unwrap();
        assert!(bootout.definition.is_none());
        assert_eq!(provider.definition_reads, 0);

        provider.state.state = ServiceState::Missing;
        provider.definition.declared_name = "different.service".into();
        assert_eq!(
            plan_with_provider(
                &mut provider,
                &identity(ServiceScope::User),
                ServiceOperation::Bootstrap,
                Some(PathBuf::from("fixture.service")),
                120,
            )
            .unwrap_err()
            .code,
            "service_definition_identity_mismatch"
        );
        assert_eq!(provider.mutations, 0);
    }

    #[test]
    fn tamper_expiry_and_complete_state_drift_refuse_before_reservation() {
        let (_root, store) = store("refuse");
        let mut provider = fixture();
        let plan = plan_with_provider(
            &mut provider,
            &identity(ServiceScope::User),
            ServiceOperation::Start,
            None,
            120,
        )
        .unwrap();
        let mut tampered = plan.clone();
        tampered.before.description.push('!');
        assert_eq!(
            apply_with_provider(&mut provider, &store, &tampered, &plan.approval_digest)
                .unwrap_err()
                .code,
            "service_approval_mismatch"
        );
        provider.now = plan.expires_at_utc_ms;
        assert_eq!(
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "service_plan_expired"
        );
        provider.now = 2_000;
        provider.state.instance.as_mut().unwrap().opaque = "incarnation-2".into();
        assert_eq!(
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "service_state_changed"
        );
        assert_eq!(provider.mutations, 0);
    }

    #[test]
    fn success_runs_once_and_replays_after_plan_expiry_without_provider_state() {
        let (_root, store) = store("success-replay");
        let mut provider = fixture();
        let plan = plan_with_provider(
            &mut provider,
            &identity(ServiceScope::User),
            ServiceOperation::Start,
            None,
            120,
        )
        .unwrap();
        let first =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap();
        assert!(first.changed && first.verified && !first.idempotent);
        assert_eq!(provider.mutations, 1);
        provider.now = plan.expires_at_utc_ms + 1;
        provider.state.identity.name = "drifted.service".into();
        let replay =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap();
        assert!(replay.idempotent && replay.changed);
        assert_eq!(provider.mutations, 1);
    }

    #[test]
    fn noop_is_finalized_and_never_calls_mutation() {
        let (_root, store) = store("noop");
        let mut provider = fixture();
        provider.state.state = ServiceState::Running;
        let plan = plan_with_provider(
            &mut provider,
            &identity(ServiceScope::User),
            ServiceOperation::Start,
            None,
            120,
        )
        .unwrap();
        let first =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap();
        assert!(!first.changed && first.verified && !first.idempotent);
        let replay =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap();
        assert!(replay.idempotent && !replay.changed);
        assert_eq!(provider.mutations, 0);
    }

    #[test]
    fn uncertain_effect_is_durable_and_never_repeated() {
        let (_root, store) = store("unknown");
        let mut provider = fixture();
        provider.results = VecDeque::from([Err(ServiceMutationFailure::OutcomeUnknown(
            CuError::new("fixture_uncertain", "fixture lost completion"),
        ))]);
        let plan = plan_with_provider(
            &mut provider,
            &identity(ServiceScope::User),
            ServiceOperation::Start,
            None,
            120,
        )
        .unwrap();
        assert_eq!(
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "service_effect_uncertain"
        );
        assert_eq!(provider.mutations, 1);
        provider.now = plan.expires_at_utc_ms + 1;
        assert_eq!(
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "service_effect_uncertain"
        );
        assert_eq!(provider.mutations, 1);
    }

    #[test]
    fn system_mutation_is_a_typed_privilege_refusal() {
        let (_root, store) = store("system-refusal");
        let mut provider = fixture();
        let system = identity(ServiceScope::System);
        assert_eq!(
            plan_with_provider(&mut provider, &system, ServiceOperation::Start, None, 120,)
                .unwrap_err()
                .code,
            "service_requires_privilege"
        );
        let mut plan = plan_with_provider(
            &mut provider,
            &identity(ServiceScope::User),
            ServiceOperation::Start,
            None,
            120,
        )
        .unwrap();
        plan.identity.scope = ServiceScope::System;
        plan.before.identity.scope = ServiceScope::System;
        plan.contract_digest = contract_digest(&plan).unwrap();
        plan.approval_digest = approval_digest(&plan).unwrap();
        let approval = plan.approval_digest.clone();
        assert_eq!(
            apply_with_provider(&mut provider, &store, &plan, &approval)
                .unwrap_err()
                .code,
            "service_requires_privilege"
        );
        assert_eq!(provider.mutations, 0);
    }

    #[test]
    fn definition_drift_refuses_before_mutation() {
        let (_root, store) = store("definition-drift");
        let mut provider = fixture();
        provider.state.state = ServiceState::Missing;
        let plan = plan_with_provider(
            &mut provider,
            &identity(ServiceScope::User),
            ServiceOperation::Bootstrap,
            Some(PathBuf::from("fixture.service")),
            120,
        )
        .unwrap();
        provider.definition.sha256 = "22".repeat(32);
        assert_eq!(
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "service_definition_changed"
        );
        assert_eq!(provider.mutations, 0);
    }

    #[test]
    fn native_definition_reader_rejects_final_symlink_and_oversize() {
        let root = TestPath::new("definition-files");
        let regular = root.0.join("regular.service");
        fs::write(&regular, b"fixture").unwrap();
        let binding = definition_identity(&regular).unwrap();
        assert_eq!(binding.byte_len, 7);
        assert_eq!(binding.sha256, hex(&Sha256::digest(b"fixture")));

        let oversized = root.0.join("oversized.service");
        let file = fs::File::create(&oversized).unwrap();
        file.set_len(MAX_DEFINITION_BYTES as u64 + 1).unwrap();
        assert_eq!(
            definition_identity(&oversized).unwrap_err().code,
            "service_definition_limit"
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&regular, root.0.join("link.service")).unwrap();
            assert_eq!(
                definition_identity(&root.0.join("link.service"))
                    .unwrap_err()
                    .code,
                "service_definition_invalid"
            );
        }
    }

    #[test]
    fn known_no_effect_and_verified_rollback_finalize_failed() {
        for (name, failure) in [
            (
                "not-performed",
                ServiceMutationFailure::NotPerformed(CuError::new("fixture_no", "not run")),
            ),
            (
                "rolled-back",
                ServiceMutationFailure::RolledBack(CuError::new("fixture_rollback", "restored")),
            ),
        ] {
            let (_root, store) = store(name);
            let mut provider = fixture();
            provider.results = VecDeque::from([Err(failure)]);
            let plan = plan_with_provider(
                &mut provider,
                &identity(ServiceScope::User),
                ServiceOperation::Start,
                None,
                120,
            )
            .unwrap();
            let first = apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest)
                .unwrap_err();
            assert!(matches!(
                first.code.as_str(),
                "fixture_no" | "fixture_rollback"
            ));
            let replay = apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest)
                .unwrap_err();
            assert_eq!(replay.code, "service_previously_failed");
            assert_eq!(provider.mutations, 1);
        }
    }
}
