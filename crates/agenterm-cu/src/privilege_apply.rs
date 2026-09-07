//! Closed wire contract between ordinary ACU and a native privileged provider.
//!
//! This module deliberately carries no password, authentication response, or
//! generic executable request. Native consent is established out of band by
//! Authorization Services, polkit, or UAC. A valid request is therefore still
//! only an intent: the provider must authenticate the peer, obtain consent,
//! revalidate the plan, durably reserve it in provider-owned storage, and own
//! the complete effect/read-back state machine.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CuError,
    idempotency_store::{
        FinalOutcome, FinalOutcomeKind, FinalReplay, FreshReservation, IdempotencyStore,
        RequestState, ReserveDecision, fingerprint_canonical_request,
    },
    privilege_plan::{
        PrivilegeOperation, ProcessPriorityPlan, ProcessSignalPlan,
        revalidate_process_signal_precondition, validate_process_priority_plan,
        validate_process_signal_plan,
    },
};

pub const PRIVILEGE_APPLY_PROTOCOL_VERSION: u32 = 1;
pub const PRIVILEGE_PROVIDER_CONTRACT_VERSION: u32 = 1;
/// One request must carry the maximum valid 129-member signal plan on every
/// target. All native transports must reuse this exact ceiling.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_REPLY_BYTES: usize = 16 * 1024;
const MAX_ID_BYTES: usize = 128;
const PROVIDER_REPLAY_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const PROVIDER_KEY_DOMAIN: &[u8] = b"agenterm-cu/privileged-provider-key/v1\0";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivilegeOriginV1 {
    pub session_id: String,
    pub target_scope: PrivilegeTargetScope,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivilegeTargetScope {
    Current,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivilegeClientV1 {
    pub contract_version: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PrivilegePlanV1 {
    ProcessPriority(ProcessPriorityPlan),
    ProcessSignal(ProcessSignalPlan),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode", deny_unknown_fields)]
pub enum PrivilegeAuthorizationV1 {
    OneShotNativeConsent,
    DelegatedGrant { grant_id: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivilegeApplyRequestV1 {
    pub protocol_version: u32,
    pub request_id: String,
    pub plan: PrivilegePlanV1,
    pub authorization: PrivilegeAuthorizationV1,
    pub origin: PrivilegeOriginV1,
    pub client: PrivilegeClientV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivilegeVerificationV1 {
    Verified,
    NotApplicable,
    Unverified,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum PrivilegeApplyReplyV1 {
    Refused {
        protocol_version: u32,
        request_id: String,
        contract_digest: String,
        approval_digest: String,
        error_code: String,
    },
    ConsentCanceled {
        protocol_version: u32,
        request_id: String,
        contract_digest: String,
        approval_digest: String,
    },
    Completed {
        protocol_version: u32,
        request_id: String,
        contract_digest: String,
        approval_digest: String,
        receipt_id: String,
        receipt_sha256: String,
        provider_identity_digest: String,
        origin_principal_digest: String,
        verification: PrivilegeVerificationV1,
    },
    FailedBeforeEffect {
        protocol_version: u32,
        request_id: String,
        contract_digest: String,
        approval_digest: String,
        error_code: String,
    },
    FailedAfterEffect {
        protocol_version: u32,
        request_id: String,
        contract_digest: String,
        approval_digest: String,
        error_code: String,
        receipt_id: String,
        receipt_sha256: String,
        provider_identity_digest: String,
        origin_principal_digest: String,
    },
    OutcomeUnknown {
        protocol_version: u32,
        request_id: String,
        contract_digest: String,
        approval_digest: String,
        receipt_id: String,
        provider_identity_digest: String,
        origin_principal_digest: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivilegeProviderNamespace {
    MacosAuthorizationServices,
    LinuxPolkit,
    WindowsUac,
    #[cfg(test)]
    Fixture,
}

impl PrivilegeProviderNamespace {
    fn as_str(self) -> &'static str {
        match self {
            Self::MacosAuthorizationServices => "macos-authorization-services-v1",
            Self::LinuxPolkit => "linux-polkit-v1",
            Self::WindowsUac => "windows-uac-v1",
            #[cfg(test)]
            Self::Fixture => "fixture-v1",
        }
    }
}

impl PrivilegePlanV1 {
    fn operation(&self) -> PrivilegeOperation {
        match self {
            Self::ProcessPriority(plan) => plan.operation,
            Self::ProcessSignal(plan) => plan.operation,
        }
    }

    fn contract_digest(&self) -> &str {
        match self {
            Self::ProcessPriority(plan) => &plan.contract_digest,
            Self::ProcessSignal(plan) => &plan.contract_digest,
        }
    }

    fn issued_at_utc_ms(&self) -> u64 {
        match self {
            Self::ProcessPriority(plan) => plan.issued_at_utc_ms,
            Self::ProcessSignal(plan) => plan.issued_at_utc_ms,
        }
    }

    fn validate_at(&self, now_utc_ms: u64) -> Result<(), CuError> {
        match self {
            Self::ProcessPriority(plan) => validate_process_priority_plan(plan, now_utc_ms),
            Self::ProcessSignal(plan) => validate_process_signal_plan(plan, now_utc_ms),
        }
    }

    fn validate_structure(&self) -> Result<(), CuError> {
        self.validate_at(self.issued_at_utc_ms())?;
        match (self, self.operation()) {
            (Self::ProcessPriority(_), PrivilegeOperation::ProcessSetPriority)
            | (Self::ProcessSignal(_), PrivilegeOperation::ProcessSignal) => Ok(()),
            _ => Err(CuError::new(
                "privilege_plan_invalid",
                "privilege plan variant does not match its closed operation",
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ValidatedPrivilegeRequestV1 {
    request: PrivilegeApplyRequestV1,
    fingerprint: String,
}

impl ValidatedPrivilegeRequestV1 {
    pub fn request(&self) -> &PrivilegeApplyRequestV1 {
        &self.request
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// Native peer identity. No wire or public constructor exists: a provider
/// child module may create this only after authenticating its native IPC peer.
#[derive(Clone, Debug)]
pub struct AuthenticatedPrivilegePeer {
    principal_digest: String,
    provider_identity_digest: String,
}

#[cfg(any(target_os = "linux", test))]
impl AuthenticatedPrivilegePeer {
    pub(crate) fn from_native_provider(
        principal_digest: String,
        provider_identity_digest: String,
    ) -> Result<Self, CuError> {
        let peer = Self {
            principal_digest,
            provider_identity_digest,
        };
        validate_authenticated_peer(&peer)?;
        Ok(peer)
    }

    pub(crate) fn principal_digest(&self) -> &str {
        &self.principal_digest
    }

    pub(crate) fn provider_identity_digest(&self) -> &str {
        &self.provider_identity_digest
    }
}

#[derive(Debug)]
pub struct NativeAuthorizationProof {
    authorization: PrivilegeAuthorizationV1,
}

#[cfg(any(target_os = "linux", test))]
impl NativeAuthorizationProof {
    pub(crate) fn from_native_provider(authorization: PrivilegeAuthorizationV1) -> Self {
        Self { authorization }
    }
}

pub struct PreparedProcessSignalEffect {
    pub(crate) plan: ProcessSignalPlan,
    pub(crate) references: Vec<agenterm_platform::process_reference::ProcessReference>,
}

impl std::fmt::Debug for PreparedProcessSignalEffect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedProcessSignalEffect")
            .field("contract_digest", &self.plan.contract_digest)
            .field("member_count", &self.references.len())
            .finish()
    }
}

#[derive(Debug)]
pub enum PreparedPrivilegeEffect {
    ProcessSignal(PreparedProcessSignalEffect),
}

impl PreparedPrivilegeEffect {
    fn contract_digest(&self) -> &str {
        match self {
            Self::ProcessSignal(effect) => &effect.plan.contract_digest,
        }
    }
}

/// Closed proof that native consent/delegated authorization and exact native
/// effect preparation have both completed for this contract.
#[derive(Debug)]
pub struct AuthorizedPreparedRequest {
    pub(crate) validated: ValidatedPrivilegeRequestV1,
    pub(crate) peer: AuthenticatedPrivilegePeer,
    pub(crate) prepared: Option<PreparedPrivilegeEffect>,
}

#[derive(Debug)]
pub enum PrivilegeProviderLookupDecision {
    Missing,
    ReplayFinalized {
        outcome_code: String,
        receipt_id: Option<String>,
        receipt_sha256: Option<String>,
    },
    OutcomeUnknown,
}

/// Provider-owned durable replay gate. The path must live in storage protected
/// by the native provider identity, never in ordinary ACU's request store.
#[derive(Clone, Debug)]
pub struct PrivilegeProviderLedger {
    namespace: PrivilegeProviderNamespace,
    store: IdempotencyStore,
}

struct PrivilegeProviderReservation {
    provider_key: String,
    fingerprint: String,
    fresh: FreshReservation,
}

/// A fresh provider reservation and the exact native objects it authorizes.
/// Keeping both in one non-cloneable value prevents an effect implementation
/// from dropping the retained objects and reopening mutable numeric PIDs.
pub struct PrivilegeProviderExecution {
    reservation: PrivilegeProviderReservation,
    pub(crate) authorized: AuthorizedPreparedRequest,
}

impl std::fmt::Debug for PrivilegeProviderExecution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrivilegeProviderExecution")
            .field("provider_key", &self.reservation.provider_key)
            .field("fingerprint", &self.reservation.fingerprint)
            .field("prepared", &self.authorized.prepared.as_ref())
            .field("completion_token", &"<redacted>")
            .finish()
    }
}

impl PrivilegeProviderExecution {
    pub(crate) fn take_prepared_effect(&mut self) -> Result<PreparedPrivilegeEffect, CuError> {
        self.authorized.prepared.take().ok_or_else(|| {
            CuError::new(
                "privilege_effect_already_attempted",
                "the reserved provider effect was already taken for its single attempt",
            )
        })
    }
}

#[derive(Debug)]
pub enum PrivilegeSignalEffectOutcome {
    Completed {
        evidence: serde_json::Value,
        verified: Option<bool>,
    },
    FailedBeforeEffect(CuError),
    FailedAfterEffect {
        error: CuError,
        outcome_unknown: bool,
    },
}

/// Consume the one prepared effect attempt from a fresh provider reservation.
/// `provider_state_root` must be a directory owned and protected by the fixed
/// native provider identity; ordinary ACU state is not an acceptable path.
pub fn execute_reserved_signal(
    execution: &mut PrivilegeProviderExecution,
    provider_state_root: &std::path::Path,
) -> PrivilegeSignalEffectOutcome {
    match execution.take_prepared_effect() {
        Ok(prepared) => crate::executor::privilege_signal_effect::execute_prepared_signal(
            prepared,
            provider_state_root,
        ),
        Err(error) => PrivilegeSignalEffectOutcome::FailedBeforeEffect(error),
    }
}

impl std::fmt::Debug for PrivilegeProviderReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrivilegeProviderReservation")
            .field("provider_key", &self.provider_key)
            .field("fingerprint", &self.fingerprint)
            .field("completion_token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug)]
pub enum PrivilegeProviderReserveDecision {
    Fresh(Box<PrivilegeProviderExecution>),
    ReplayFinalized {
        outcome_code: String,
        receipt_id: Option<String>,
        receipt_sha256: Option<String>,
    },
    OutcomeUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrivilegeProviderFinalOutcome {
    Completed {
        outcome_code: String,
        receipt_id: String,
        receipt_sha256: String,
    },
    FailedBeforeEffect {
        outcome_code: String,
    },
    FailedAfterEffect {
        outcome_code: String,
        receipt_id: String,
        receipt_sha256: String,
    },
}

impl PrivilegeProviderLedger {
    pub fn open_at(
        path: impl Into<PathBuf>,
        namespace: PrivilegeProviderNamespace,
    ) -> Result<Self, CuError> {
        Ok(Self {
            namespace,
            store: IdempotencyStore::open_at(path)?,
        })
    }

    /// Read durable replay state before opening native consent. An expired plan
    /// can still retrieve an earlier terminal/unknown outcome during retention.
    pub fn lookup_before_consent(
        &self,
        validated: &ValidatedPrivilegeRequestV1,
        peer: &AuthenticatedPrivilegePeer,
        now_utc_ms: i64,
    ) -> Result<PrivilegeProviderLookupDecision, CuError> {
        validate_authenticated_peer(peer)?;
        let provider_key = provider_key(
            self.namespace,
            &peer.principal_digest,
            &validated.request.request_id,
        );
        match self
            .store
            .lookup(&provider_key, &validated.fingerprint, now_utc_ms)?
        {
            None => Ok(PrivilegeProviderLookupDecision::Missing),
            Some(status) => lookup_decision(status),
        }
    }

    /// Atomic second replay check and reservation after native consent and
    /// exact-object preparation. Only the provider's private type-state can
    /// reach this method.
    pub fn reserve_authorized(
        &self,
        authorized: AuthorizedPreparedRequest,
        now_utc_ms: i64,
    ) -> Result<PrivilegeProviderReserveDecision, CuError> {
        validate_authenticated_peer(&authorized.peer)?;
        validate_fresh_request(&authorized.validated.request, now_utc_ms)?;
        let provider_key = provider_key(
            self.namespace,
            &authorized.peer.principal_digest,
            &authorized.validated.request.request_id,
        );
        match self.store.reserve(
            &provider_key,
            &authorized.validated.fingerprint,
            PROVIDER_REPLAY_RETENTION_MS,
            now_utc_ms,
        )? {
            ReserveDecision::Fresh(fresh) => Ok(PrivilegeProviderReserveDecision::Fresh(Box::new(
                PrivilegeProviderExecution {
                    reservation: PrivilegeProviderReservation {
                        provider_key,
                        fingerprint: authorized.validated.fingerprint.clone(),
                        fresh,
                    },
                    authorized,
                },
            ))),
            ReserveDecision::ReplayFinalized(status) => {
                let PrivilegeProviderLookupDecision::ReplayFinalized {
                    outcome_code,
                    receipt_id,
                    receipt_sha256,
                } = lookup_decision(status)?
                else {
                    return Err(CuError::new(
                        "privilege_provider_state_invalid",
                        "finalized reservation did not yield a terminal replay",
                    ));
                };
                Ok(PrivilegeProviderReserveDecision::ReplayFinalized {
                    outcome_code,
                    receipt_id,
                    receipt_sha256,
                })
            }
            ReserveDecision::Uncertain(_) => Ok(PrivilegeProviderReserveDecision::OutcomeUnknown),
        }
    }

    pub fn finalize(
        &self,
        execution: Box<PrivilegeProviderExecution>,
        outcome: PrivilegeProviderFinalOutcome,
        now_utc_ms: i64,
    ) -> Result<(), CuError> {
        let outcome = match outcome {
            PrivilegeProviderFinalOutcome::Completed {
                outcome_code,
                receipt_id,
                receipt_sha256,
            } => FinalOutcome::new(
                FinalOutcomeKind::Succeeded,
                outcome_code,
                Some(receipt_sha256),
            )?
            .with_replay(FinalReplay::PrivilegeApply { receipt_id })?,
            PrivilegeProviderFinalOutcome::FailedBeforeEffect { outcome_code } => {
                FinalOutcome::new(FinalOutcomeKind::Failed, outcome_code, None)?
            }
            PrivilegeProviderFinalOutcome::FailedAfterEffect {
                outcome_code,
                receipt_id,
                receipt_sha256,
            } => FinalOutcome::new(FinalOutcomeKind::Failed, outcome_code, Some(receipt_sha256))?
                .with_replay(FinalReplay::PrivilegeApply { receipt_id })?,
        };
        self.store.finalize(
            &execution.reservation.provider_key,
            &execution.reservation.fingerprint,
            &execution.reservation.fresh.completion_token,
            outcome,
            now_utc_ms,
        )?;
        Ok(())
    }

    pub fn mark_outcome_unknown(
        &self,
        execution: Box<PrivilegeProviderExecution>,
        now_utc_ms: i64,
    ) -> Result<(), CuError> {
        self.store.mark_outcome_unknown(
            &execution.reservation.provider_key,
            &execution.reservation.fingerprint,
            &execution.reservation.fresh.completion_token,
            now_utc_ms,
        )?;
        Ok(())
    }
}

/// Retain every exact native process object and revalidate the complete signal
/// precondition while those objects are still held. Priority mutation remains
/// unavailable until a platform facade can mutate one retained process object;
/// reopening a PID with `setpriority` would violate this contract.
pub fn prepare_privilege_effect(
    validated: &ValidatedPrivilegeRequestV1,
) -> Result<PreparedPrivilegeEffect, CuError> {
    let PrivilegePlanV1::ProcessSignal(plan) = &validated.request.plan else {
        return Err(CuError::new(
            "privilege_effect_unsupported",
            "process.set-priority has no race-free retained-object mutation primitive",
        ));
    };
    if cfg!(windows)
        && (plan.scope != crate::privilege_plan::ProcessSignalScope::Single
            || plan.signal != crate::command::ProcessSignalKind::Kill
            || !plan.force)
    {
        return Err(CuError::new(
            "privilege_effect_unsupported",
            "Windows supports only forceful termination of one retained process object",
        ));
    }

    revalidate_process_signal_precondition(plan)?;
    let mut references = Vec::with_capacity(plan.members.len());
    for member in &plan.members {
        let reference =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(
                member.pid,
            )
            .map_err(|error| CuError::new("privilege_effect_prepare_failed", error.to_string()))?;
        if reference.id() != member.pid
            || !reference.is_alive().map_err(|error| {
                CuError::new("privilege_effect_prepare_failed", error.to_string())
            })?
        {
            return Err(CuError::new(
                "privilege_precondition_changed",
                "an approved process exited while its exact native object was retained",
            ));
        }
        references.push(reference);
    }
    revalidate_process_signal_precondition(plan)?;
    for reference in &references {
        let alive = reference
            .is_alive()
            .map_err(|error| CuError::new("privilege_effect_prepare_failed", error.to_string()))?;
        if !alive {
            return Err(CuError::new(
                "privilege_precondition_changed",
                "an approved process changed during final retained-object validation",
            ));
        }
    }
    Ok(PreparedPrivilegeEffect::ProcessSignal(
        PreparedProcessSignalEffect {
            plan: plan.clone(),
            references,
        },
    ))
}

/// Parse and structurally validate untrusted wire bytes before replay lookup.
/// Freshness is deliberately separate: an expired approval can still retrieve
/// an earlier retained terminal or uncertain provider outcome without opening
/// native consent again.
pub fn parse_apply_request(bytes: &[u8]) -> Result<ValidatedPrivilegeRequestV1, CuError> {
    if bytes.is_empty() || bytes.len() > MAX_REQUEST_BYTES {
        return Err(CuError::new(
            "privilege_request_size_invalid",
            "privilege apply request is empty or exceeds its byte budget",
        ));
    }
    let request: PrivilegeApplyRequestV1 = serde_json::from_slice(bytes).map_err(|_| {
        CuError::new(
            "privilege_request_invalid",
            "privilege apply request is not the closed protocol-v1 shape",
        )
    })?;
    validate_apply_request_structure(&request)?;
    let canonical = serde_json::to_vec(&request).map_err(|_| {
        CuError::new(
            "privilege_request_invalid",
            "privilege apply request could not be serialized canonically",
        )
    })?;
    Ok(ValidatedPrivilegeRequestV1 {
        request,
        fingerprint: fingerprint_canonical_request(&canonical)?,
    })
}

pub fn parse_apply_reply(bytes: &[u8]) -> Result<PrivilegeApplyReplyV1, CuError> {
    if bytes.is_empty() || bytes.len() > MAX_REPLY_BYTES {
        return Err(CuError::new(
            "privilege_reply_size_invalid",
            "privilege apply reply is empty or exceeds its byte budget",
        ));
    }
    let reply: PrivilegeApplyReplyV1 = serde_json::from_slice(bytes).map_err(|_| {
        CuError::new(
            "privilege_reply_invalid",
            "privilege apply reply is not the closed protocol-v1 shape",
        )
    })?;
    validate_apply_reply(&reply)?;
    Ok(reply)
}

fn validate_apply_reply(reply: &PrivilegeApplyReplyV1) -> Result<(), CuError> {
    let (protocol_version, request_id, contract_digest, approval_digest) = match reply {
        PrivilegeApplyReplyV1::Refused {
            protocol_version,
            request_id,
            contract_digest,
            approval_digest,
            error_code,
        }
        | PrivilegeApplyReplyV1::FailedBeforeEffect {
            protocol_version,
            request_id,
            contract_digest,
            approval_digest,
            error_code,
        } => {
            validate_machine_code(error_code)?;
            (
                *protocol_version,
                request_id,
                contract_digest,
                approval_digest,
            )
        }
        PrivilegeApplyReplyV1::ConsentCanceled {
            protocol_version,
            request_id,
            contract_digest,
            approval_digest,
        } => (
            *protocol_version,
            request_id,
            contract_digest,
            approval_digest,
        ),
        PrivilegeApplyReplyV1::Completed {
            protocol_version,
            request_id,
            contract_digest,
            approval_digest,
            receipt_id,
            receipt_sha256,
            provider_identity_digest,
            origin_principal_digest,
            ..
        } => {
            validate_receipt_id(receipt_id)?;
            validate_digest(receipt_sha256, "privilege_receipt_digest_invalid")?;
            validate_digest(
                provider_identity_digest,
                "privilege_provider_identity_invalid",
            )?;
            validate_digest(
                origin_principal_digest,
                "privilege_origin_principal_invalid",
            )?;
            (
                *protocol_version,
                request_id,
                contract_digest,
                approval_digest,
            )
        }
        PrivilegeApplyReplyV1::FailedAfterEffect {
            protocol_version,
            request_id,
            contract_digest,
            approval_digest,
            error_code,
            receipt_id,
            receipt_sha256,
            provider_identity_digest,
            origin_principal_digest,
        } => {
            validate_machine_code(error_code)?;
            validate_receipt_id(receipt_id)?;
            validate_digest(receipt_sha256, "privilege_receipt_digest_invalid")?;
            validate_digest(
                provider_identity_digest,
                "privilege_provider_identity_invalid",
            )?;
            validate_digest(
                origin_principal_digest,
                "privilege_origin_principal_invalid",
            )?;
            (
                *protocol_version,
                request_id,
                contract_digest,
                approval_digest,
            )
        }
        PrivilegeApplyReplyV1::OutcomeUnknown {
            protocol_version,
            request_id,
            contract_digest,
            approval_digest,
            receipt_id,
            provider_identity_digest,
            origin_principal_digest,
        } => {
            validate_receipt_id(receipt_id)?;
            validate_digest(
                provider_identity_digest,
                "privilege_provider_identity_invalid",
            )?;
            validate_digest(
                origin_principal_digest,
                "privilege_origin_principal_invalid",
            )?;
            (
                *protocol_version,
                request_id,
                contract_digest,
                approval_digest,
            )
        }
    };
    if protocol_version != PRIVILEGE_APPLY_PROTOCOL_VERSION {
        return Err(CuError::new(
            "privilege_protocol_unsupported",
            "privilege reply protocol version is unsupported",
        ));
    }
    validate_identifier(request_id, "privilege_request_id_invalid")?;
    validate_digest(contract_digest, "privilege_contract_digest_invalid")?;
    validate_digest(approval_digest, "privilege_approval_digest_invalid")
}

fn validate_apply_request_structure(request: &PrivilegeApplyRequestV1) -> Result<(), CuError> {
    if request.protocol_version != PRIVILEGE_APPLY_PROTOCOL_VERSION
        || request.client.contract_version != PRIVILEGE_PROVIDER_CONTRACT_VERSION
    {
        return Err(CuError::new(
            "privilege_protocol_unsupported",
            "privilege apply protocol or provider contract version is unsupported",
        ));
    }
    validate_identifier(&request.request_id, "privilege_request_id_invalid")?;
    validate_identifier(&request.origin.session_id, "privilege_session_id_invalid")?;
    match &request.authorization {
        PrivilegeAuthorizationV1::OneShotNativeConsent => {}
        PrivilegeAuthorizationV1::DelegatedGrant { grant_id } => {
            validate_identifier(grant_id, "privilege_grant_id_invalid")?;
        }
    }
    request.plan.validate_structure()?;
    Ok(())
}

fn validate_fresh_request(
    request: &PrivilegeApplyRequestV1,
    now_utc_ms: i64,
) -> Result<(), CuError> {
    let now_utc_ms = u64::try_from(now_utc_ms).map_err(|_| {
        CuError::new(
            "privilege_provider_clock_invalid",
            "provider clock must be non-negative UTC milliseconds",
        )
    })?;
    request.plan.validate_at(now_utc_ms)
}

fn validate_authenticated_peer(peer: &AuthenticatedPrivilegePeer) -> Result<(), CuError> {
    validate_digest(&peer.principal_digest, "privilege_origin_principal_invalid")?;
    validate_digest(
        &peer.provider_identity_digest,
        "privilege_provider_identity_invalid",
    )
}

pub fn bind_authorized_prepared(
    validated: ValidatedPrivilegeRequestV1,
    peer: AuthenticatedPrivilegePeer,
    authorization: NativeAuthorizationProof,
    prepared: PreparedPrivilegeEffect,
) -> Result<AuthorizedPreparedRequest, CuError> {
    validate_authenticated_peer(&peer)?;
    if authorization.authorization != validated.request.authorization {
        return Err(CuError::new(
            "privilege_authorization_mismatch",
            "native authorization does not match the requested mode",
        ));
    }
    if prepared.contract_digest() != validated.request.plan.contract_digest() {
        return Err(CuError::new(
            "privilege_prepared_effect_mismatch",
            "prepared native effect does not match the validated contract",
        ));
    }
    Ok(AuthorizedPreparedRequest {
        validated,
        peer,
        prepared: Some(prepared),
    })
}

fn lookup_decision(
    status: crate::idempotency_store::RequestStatus,
) -> Result<PrivilegeProviderLookupDecision, CuError> {
    match status.state {
        RequestState::Reserved | RequestState::OutcomeUnknown => {
            Ok(PrivilegeProviderLookupDecision::OutcomeUnknown)
        }
        RequestState::Finalized => {
            let outcome = status.outcome.ok_or_else(|| {
                CuError::new(
                    "privilege_provider_state_invalid",
                    "finalized provider reservation has no outcome",
                )
            })?;
            let receipt_id = match outcome.replay {
                Some(FinalReplay::PrivilegeApply { receipt_id }) => Some(receipt_id),
                None => None,
                Some(_) => {
                    return Err(CuError::new(
                        "privilege_provider_state_invalid",
                        "provider replay metadata belongs to another effect family",
                    ));
                }
            };
            Ok(PrivilegeProviderLookupDecision::ReplayFinalized {
                outcome_code: outcome.code,
                receipt_id,
                receipt_sha256: outcome.receipt_sha256,
            })
        }
    }
}

fn validate_identifier(value: &str, code: &'static str) -> Result<(), CuError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(CuError::new(
            code,
            "privilege identifier must be bounded printable ASCII without whitespace",
        ));
    }
    Ok(())
}

fn validate_machine_code(value: &str) -> Result<(), CuError> {
    if value.is_empty()
        || value.len() > 96
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b':')
        })
    {
        return Err(CuError::new(
            "privilege_error_code_invalid",
            "privilege error code must be a bounded lowercase machine token",
        ));
    }
    Ok(())
}

fn validate_receipt_id(value: &str) -> Result<(), CuError> {
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
    if !valid {
        return Err(CuError::new(
            "privilege_receipt_id_invalid",
            "privilege receipt id must be a lowercase UUID v4",
        ));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_digest(value: &str, code: &'static str) -> Result<(), CuError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CuError::new(
            code,
            "privilege identity digest must be lowercase SHA-256 hex",
        ));
    }
    Ok(())
}

fn provider_key(
    namespace: PrivilegeProviderNamespace,
    origin_principal_digest: &str,
    request_id: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PROVIDER_KEY_DOMAIN);
    for value in [namespace.as_str(), origin_principal_digest, request_id] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    sha256_hex(&hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use crate::privilege_plan::process_priority_plan;
    use crate::{
        command::ProcessSignalKind,
        privilege_plan::{
            PROCESS_SIGNAL_TREE_MAX_DESCENDANTS, ProcessSignalBeforeState, ProcessSignalMember,
            ProcessSignalScope,
        },
    };

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn request(now: u64) -> PrivilegeApplyRequestV1 {
        let pid = std::process::id();
        let nice = agenterm_platform::process_metrics::nice(pid).unwrap();
        PrivilegeApplyRequestV1 {
            protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: "request-01".into(),
            plan: PrivilegePlanV1::ProcessPriority(
                process_priority_plan(pid, nice, 120, now).unwrap(),
            ),
            authorization: PrivilegeAuthorizationV1::OneShotNativeConsent,
            origin: PrivilegeOriginV1 {
                session_id: "session-01".into(),
                target_scope: PrivilegeTargetScope::Current,
            },
            client: PrivilegeClientV1 {
                contract_version: PRIVILEGE_PROVIDER_CONTRACT_VERSION,
            },
        }
    }

    fn peer() -> AuthenticatedPrivilegePeer {
        AuthenticatedPrivilegePeer {
            principal_digest: sha256_hex(b"fixture-principal"),
            provider_identity_digest: sha256_hex(b"fixture-provider"),
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn signal_request(now: u64, signal: ProcessSignalKind) -> PrivilegeApplyRequestV1 {
        let mut request = request(now);
        request.plan = PrivilegePlanV1::ProcessSignal(
            crate::privilege_plan::process_signal_plan(
                std::process::id(),
                signal,
                false,
                false,
                5_000,
                16,
                120,
                now,
            )
            .unwrap(),
        );
        request
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn authorize(validated: ValidatedPrivilegeRequestV1) -> AuthorizedPreparedRequest {
        let authorization = validated.request.authorization.clone();
        let prepared = prepare_privilege_effect(&validated).unwrap();
        bind_authorized_prepared(
            validated,
            peer(),
            NativeAuthorizationProof { authorization },
            prepared,
        )
        .unwrap()
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn closed_request_round_trips_with_stable_fingerprint() {
        let request = signal_request(1_000, ProcessSignalKind::User1);
        let bytes = serde_json::to_vec(&request).unwrap();
        let first = parse_apply_request(&bytes).unwrap();
        let second = parse_apply_request(&bytes).unwrap();
        assert_eq!(first.request(), &request);
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_eq!(first.fingerprint().len(), 64);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn signal_plan_round_trips_through_the_closed_union() {
        let plan = crate::privilege_plan::process_signal_plan(
            std::process::id(),
            ProcessSignalKind::Terminate,
            false,
            false,
            5_000,
            16,
            120,
            1_000,
        )
        .unwrap();
        let mut request = request(1_000);
        request.plan = PrivilegePlanV1::ProcessSignal(plan.clone());
        let parsed = parse_apply_request(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert!(matches!(
            parsed.request().plan,
            PrivilegePlanV1::ProcessSignal(ref actual) if actual == &plan
        ));
    }

    #[test]
    fn reply_states_cannot_encode_effect_contradictions() {
        let reply = PrivilegeApplyReplyV1::FailedAfterEffect {
            protocol_version: 1,
            request_id: "request-01".into(),
            contract_digest: "a".repeat(64),
            approval_digest: "b".repeat(64),
            error_code: "readback_failed".into(),
            receipt_id: "12345678-1234-4234-8234-123456789abc".into(),
            receipt_sha256: "c".repeat(64),
            provider_identity_digest: "d".repeat(64),
            origin_principal_digest: "e".repeat(64),
        };
        let value = serde_json::to_value(&reply).unwrap();
        assert_eq!(value["state"], "failed_after_effect");
        assert!(value.get("mutation_attempted").is_none());
        assert!(value.get("verified").is_none());
        parse_apply_reply(&serde_json::to_vec(&value).unwrap()).unwrap();

        let mut invalid_code = value.clone();
        invalid_code["error_code"] = serde_json::json!("Human Error");
        assert_eq!(
            parse_apply_reply(&serde_json::to_vec(&invalid_code).unwrap())
                .unwrap_err()
                .code,
            "privilege_error_code_invalid"
        );

        let mut unknown = value;
        unknown["unexpected"] = serde_json::json!(true);
        assert_eq!(
            parse_apply_reply(&serde_json::to_vec(&unknown).unwrap())
                .unwrap_err()
                .code,
            "privilege_reply_invalid"
        );
        assert_eq!(
            parse_apply_reply(&vec![b' '; MAX_REPLY_BYTES + 1])
                .unwrap_err()
                .code,
            "privilege_reply_size_invalid"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn type_state_rejects_wrong_authorization_and_prepared_contract() {
        let signal_request_value = signal_request(1_000, ProcessSignalKind::User1);
        let validated =
            parse_apply_request(&serde_json::to_vec(&signal_request_value).unwrap()).unwrap();
        let prepared = prepare_privilege_effect(&validated).unwrap();
        let wrong_authorization = NativeAuthorizationProof {
            authorization: PrivilegeAuthorizationV1::DelegatedGrant {
                grant_id: "grant-01".into(),
            },
        };
        assert_eq!(
            bind_authorized_prepared(validated, peer(), wrong_authorization, prepared,)
                .unwrap_err()
                .code,
            "privilege_authorization_mismatch"
        );

        let prepared_request = signal_request(1_000, ProcessSignalKind::User1);
        let prepared_validated =
            parse_apply_request(&serde_json::to_vec(&prepared_request).unwrap()).unwrap();
        let prepared = prepare_privilege_effect(&prepared_validated).unwrap();
        let other_request = signal_request(1_000, ProcessSignalKind::User2);
        let other_validated =
            parse_apply_request(&serde_json::to_vec(&other_request).unwrap()).unwrap();
        assert_eq!(
            bind_authorized_prepared(
                other_validated,
                peer(),
                NativeAuthorizationProof {
                    authorization: other_request.authorization.clone(),
                },
                prepared,
            )
            .unwrap_err()
            .code,
            "privilege_prepared_effect_mismatch"
        );

        let priority = request(1_000);
        let validated = parse_apply_request(&serde_json::to_vec(&priority).unwrap()).unwrap();
        assert_eq!(
            prepare_privilege_effect(&validated).unwrap_err().code,
            "privilege_effect_unsupported"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn signal_preparation_retains_exact_objects_and_refuses_scheduler_drift() {
        let request = signal_request(1_000, ProcessSignalKind::User1);
        let validated = parse_apply_request(&serde_json::to_vec(&request).unwrap()).unwrap();
        let prepared = prepare_privilege_effect(&validated).unwrap();
        let PreparedPrivilegeEffect::ProcessSignal(prepared) = prepared;
        assert_eq!(prepared.references.len(), 1);
        assert_eq!(prepared.references[0].id(), std::process::id());
        assert!(prepared.references[0].is_alive().unwrap());

        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn signal preparation fixture");
        let plan = crate::privilege_plan::process_signal_plan(
            child.id(),
            ProcessSignalKind::User1,
            false,
            false,
            5_000,
            16,
            120,
            1_000,
        )
        .unwrap();
        let mut drift_request = request;
        drift_request.request_id = "request-drift".into();
        drift_request.plan = PrivilegePlanV1::ProcessSignal(plan);
        let drift_validated =
            parse_apply_request(&serde_json::to_vec(&drift_request).unwrap()).unwrap();
        let child_reference =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(
                child.id(),
            )
            .unwrap();
        child_reference.set_suspended(true).unwrap();
        assert_eq!(
            prepare_privilege_effect(&drift_validated).unwrap_err().code,
            "privilege_precondition_changed"
        );
        child_reference.set_suspended(false).unwrap();
        child_reference
            .terminate(agenterm_platform::process_control::TerminationMode::Forceful)
            .unwrap();
        child.wait().unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn fresh_provider_execution_attempts_one_retained_object_effect_exactly_once() {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "agenterm-cu-privilege-effect-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&root);
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn provider effect fixture");
        let mut request = request(1_000);
        request.plan = PrivilegePlanV1::ProcessSignal(
            crate::privilege_plan::process_signal_plan(
                child.id(),
                ProcessSignalKind::Stop,
                false,
                false,
                5_000,
                16,
                120,
                1_000,
            )
            .unwrap(),
        );
        let validated = parse_apply_request(&serde_json::to_vec(&request).unwrap()).unwrap();
        let ledger = PrivilegeProviderLedger::open_at(
            root.join("ledger.json"),
            PrivilegeProviderNamespace::Fixture,
        )
        .unwrap();
        let mut execution = match ledger
            .reserve_authorized(authorize(validated), 1_001)
            .unwrap()
        {
            PrivilegeProviderReserveDecision::Fresh(execution) => execution,
            other => panic!("expected fresh provider execution, got {other:?}"),
        };
        match execute_reserved_signal(&mut execution, &root) {
            PrivilegeSignalEffectOutcome::Completed { verified, .. } => {
                assert_eq!(verified, Some(true));
            }
            other => panic!("expected completed retained effect, got {other:?}"),
        }
        assert!(agenterm_platform::process_metrics::is_stopped(child.id()).unwrap());
        match execute_reserved_signal(&mut execution, &root) {
            PrivilegeSignalEffectOutcome::FailedBeforeEffect(error) => {
                assert_eq!(error.code, "privilege_effect_already_attempted");
            }
            other => panic!("second effect attempt must fail closed, got {other:?}"),
        }

        let reference =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(
                child.id(),
            )
            .unwrap();
        reference.set_suspended(false).unwrap();
        reference
            .terminate(agenterm_platform::process_control::TerminationMode::Forceful)
            .unwrap();
        child.wait().unwrap();
        ledger
            .finalize(
                execution,
                PrivilegeProviderFinalOutcome::Completed {
                    outcome_code: "completed".into(),
                    receipt_id: "12345678-1234-4234-8234-123456789abc".into(),
                    receipt_sha256: sha256_hex(b"fixture-effect-receipt"),
                },
                1_002,
            )
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unknown_fields_tampering_and_expiry_fail_before_provider_dispatch() {
        let request = request(1_000);
        let mut value = serde_json::to_value(&request).unwrap();
        value["password"] = serde_json::json!("must-never-enter-this-protocol");
        assert_eq!(
            parse_apply_request(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .code,
            "privilege_request_invalid"
        );

        let mut tampered = request.clone();
        let PrivilegePlanV1::ProcessPriority(plan) = &mut tampered.plan else {
            unreachable!()
        };
        plan.after.nice = if plan.after.nice == 20 {
            19
        } else {
            plan.after.nice + 1
        };
        assert_eq!(
            parse_apply_request(&serde_json::to_vec(&tampered).unwrap())
                .unwrap_err()
                .code,
            "privilege_plan_digest_mismatch"
        );
        let validated = parse_apply_request(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert_eq!(
            validate_fresh_request(&request, 999).unwrap_err().code,
            "privilege_plan_not_yet_valid"
        );
        assert_eq!(
            validate_fresh_request(&request, 121_001).unwrap_err().code,
            "privilege_plan_expired"
        );
        assert_eq!(validated.request(), &request);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn identifiers_are_bounded_and_do_not_accept_whitespace() {
        let mut request = request(1_000);
        request.request_id = "bad request".into();
        assert_eq!(
            parse_apply_request(&serde_json::to_vec(&request).unwrap())
                .unwrap_err()
                .code,
            "privilege_request_id_invalid"
        );
    }

    #[test]
    fn maximum_signal_request_fits_one_shared_wire_ceiling() {
        let identity = "windows-filetime:18446744073709551615".to_owned();
        let mut members = Vec::new();
        for index in 0..=PROCESS_SIGNAL_TREE_MAX_DESCENDANTS {
            members.push(ProcessSignalMember {
                pid: index + 2,
                depth: u32::from(index != 0),
                parent_pid: (index != 0).then_some(2),
                start_identity: identity.clone(),
                before: ProcessSignalBeforeState { stopped: false },
            });
        }
        let request = PrivilegeApplyRequestV1 {
            protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: "r".repeat(MAX_ID_BYTES),
            plan: PrivilegePlanV1::ProcessSignal(ProcessSignalPlan {
                schema_version: 1,
                operation: PrivilegeOperation::ProcessSignal,
                target: crate::privilege_plan::ProcessSignalTarget {
                    pid: 2,
                    start_identity: identity,
                },
                scope: ProcessSignalScope::Tree,
                signal: ProcessSignalKind::Terminate,
                force: false,
                timeout_ms: 60_000,
                max_descendants: PROCESS_SIGNAL_TREE_MAX_DESCENDANTS,
                members,
                issued_at_utc_ms: u64::MAX - 600_000,
                expires_at_utc_ms: u64::MAX,
                contract_digest: "a".repeat(64),
                approval_digest: "b".repeat(64),
                consent_requested: false,
                mutation_performed: false,
            }),
            authorization: PrivilegeAuthorizationV1::DelegatedGrant {
                grant_id: "g".repeat(MAX_ID_BYTES),
            },
            origin: PrivilegeOriginV1 {
                session_id: "s".repeat(MAX_ID_BYTES),
                target_scope: PrivilegeTargetScope::Current,
            },
            client: PrivilegeClientV1 {
                contract_version: PRIVILEGE_PROVIDER_CONTRACT_VERSION,
            },
        };
        let bytes = serde_json::to_vec(&request).unwrap();
        assert_eq!(bytes.len(), 16_607);
        assert!(bytes.len() <= MAX_REQUEST_BYTES);
        assert_eq!(
            parse_apply_request(&vec![b' '; MAX_REQUEST_BYTES + 1])
                .unwrap_err()
                .code,
            "privilege_request_size_invalid"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn provider_ledger_replays_completion_and_never_reopens_uncertain_effects() {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "agenterm-cu-privilege-ledger-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_dir_all(&root);
        let ledger = PrivilegeProviderLedger::open_at(
            root.join("ledger.json"),
            PrivilegeProviderNamespace::Fixture,
        )
        .unwrap();
        let request = signal_request(1_000, ProcessSignalKind::User1);
        let validated = parse_apply_request(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert!(matches!(
            ledger
                .lookup_before_consent(&validated, &peer(), 1_000)
                .unwrap(),
            PrivilegeProviderLookupDecision::Missing
        ));

        let first = match ledger
            .reserve_authorized(authorize(validated), 1_001)
            .unwrap()
        {
            PrivilegeProviderReserveDecision::Fresh(reservation) => reservation,
            other => panic!("expected fresh reservation, got {other:?}"),
        };
        let Some(PreparedPrivilegeEffect::ProcessSignal(prepared)) = &first.authorized.prepared
        else {
            panic!("fresh execution must retain its prepared signal effect")
        };
        assert_eq!(prepared.references.len(), 1);
        assert_eq!(prepared.references[0].id(), std::process::id());
        assert!(prepared.references[0].is_alive().unwrap());
        let validated = parse_apply_request(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert!(matches!(
            ledger
                .lookup_before_consent(&validated, &peer(), 1_002)
                .unwrap(),
            PrivilegeProviderLookupDecision::OutcomeUnknown
        ));
        assert!(matches!(
            ledger
                .reserve_authorized(authorize(validated), 1_002)
                .unwrap(),
            PrivilegeProviderReserveDecision::OutcomeUnknown
        ));
        ledger.mark_outcome_unknown(first, 1_003).unwrap();
        let validated = parse_apply_request(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert!(matches!(
            ledger
                .lookup_before_consent(&validated, &peer(), 1_004)
                .unwrap(),
            PrivilegeProviderLookupDecision::OutcomeUnknown
        ));

        let mut completed_request = request.clone();
        completed_request.request_id = "request-02".into();
        let completed_validated =
            parse_apply_request(&serde_json::to_vec(&completed_request).unwrap()).unwrap();
        let completed = match ledger
            .reserve_authorized(authorize(completed_validated), 1_005)
            .unwrap()
        {
            PrivilegeProviderReserveDecision::Fresh(reservation) => reservation,
            other => panic!("expected fresh reservation, got {other:?}"),
        };
        let receipt = sha256_hex(b"fixture-receipt");
        let receipt_id = "12345678-1234-4234-8234-123456789abc".to_owned();
        ledger
            .finalize(
                completed,
                PrivilegeProviderFinalOutcome::Completed {
                    outcome_code: "completed".into(),
                    receipt_id: receipt_id.clone(),
                    receipt_sha256: receipt.clone(),
                },
                1_006,
            )
            .unwrap();
        let completed_validated =
            parse_apply_request(&serde_json::to_vec(&completed_request).unwrap()).unwrap();
        match ledger
            .lookup_before_consent(&completed_validated, &peer(), 121_001)
            .unwrap()
        {
            PrivilegeProviderLookupDecision::ReplayFinalized {
                outcome_code,
                receipt_id: replay_receipt_id,
                receipt_sha256,
            } => {
                assert_eq!(outcome_code, "completed");
                assert_eq!(replay_receipt_id.as_deref(), Some(receipt_id.as_str()));
                assert_eq!(receipt_sha256.as_deref(), Some(receipt.as_str()));
            }
            other => panic!("expected finalized replay, got {other:?}"),
        }

        let mut failed_request = request.clone();
        failed_request.request_id = "request-03".into();
        let failed_validated =
            parse_apply_request(&serde_json::to_vec(&failed_request).unwrap()).unwrap();
        let failed = match ledger
            .reserve_authorized(authorize(failed_validated), 1_007)
            .unwrap()
        {
            PrivilegeProviderReserveDecision::Fresh(reservation) => reservation,
            other => panic!("expected fresh reservation, got {other:?}"),
        };
        let failed_receipt = sha256_hex(b"fixture-failed-receipt");
        let failed_receipt_id = "12345678-1234-4234-9234-123456789abc".to_owned();
        ledger
            .finalize(
                failed,
                PrivilegeProviderFinalOutcome::FailedAfterEffect {
                    outcome_code: "privilege_effect_failed".into(),
                    receipt_id: failed_receipt_id.clone(),
                    receipt_sha256: failed_receipt.clone(),
                },
                1_008,
            )
            .unwrap();
        let failed_validated =
            parse_apply_request(&serde_json::to_vec(&failed_request).unwrap()).unwrap();
        match ledger
            .lookup_before_consent(&failed_validated, &peer(), 1_009)
            .unwrap()
        {
            PrivilegeProviderLookupDecision::ReplayFinalized {
                outcome_code,
                receipt_id,
                receipt_sha256,
            } => {
                assert_eq!(outcome_code, "privilege_effect_failed");
                assert_eq!(receipt_id.as_deref(), Some(failed_receipt_id.as_str()));
                assert_eq!(receipt_sha256.as_deref(), Some(failed_receipt.as_str()));
            }
            other => panic!("expected failed-after-effect replay, got {other:?}"),
        }

        let mut changed = completed_request.clone();
        changed.origin.session_id = "session-02".into();
        let changed = parse_apply_request(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(
            ledger
                .lookup_before_consent(&changed, &peer(), 121_002)
                .unwrap_err()
                .code,
            "request_id_conflict"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
