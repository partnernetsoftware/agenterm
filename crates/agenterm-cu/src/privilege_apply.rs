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
        FinalOutcome, FinalOutcomeKind, FreshReservation, IdempotencyStore, ReserveDecision,
        fingerprint_canonical_request,
    },
    privilege_plan::{ProcessPriorityPlan, validate_process_priority_plan},
};

pub const PRIVILEGE_APPLY_PROTOCOL_VERSION: u32 = 1;
pub const PRIVILEGE_PROVIDER_CONTRACT_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
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
#[serde(deny_unknown_fields)]
pub struct PrivilegeApplyRequestV1 {
    pub protocol_version: u32,
    pub request_id: String,
    pub plan: ProcessPriorityPlan,
    pub origin: PrivilegeOriginV1,
    pub client: PrivilegeClientV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivilegeApplyState {
    Refused,
    ConsentCanceled,
    ConsentUnavailable,
    Expired,
    PreconditionChanged,
    Reserved,
    Completed,
    Failed,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivilegeApplyReplyV1 {
    pub protocol_version: u32,
    pub request_id: String,
    pub state: PrivilegeApplyState,
    pub contract_digest: String,
    pub approval_digest: String,
    pub mutation_attempted: bool,
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_identity_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_principal_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
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

/// Provider-owned durable replay gate. The path must live in storage protected
/// by the native provider identity, never in ordinary ACU's request store.
#[derive(Clone, Debug)]
pub struct PrivilegeProviderLedger {
    namespace: PrivilegeProviderNamespace,
    store: IdempotencyStore,
}

pub struct PrivilegeProviderReservation {
    provider_key: String,
    fingerprint: String,
    fresh: FreshReservation,
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
    Fresh(PrivilegeProviderReservation),
    ReplayFinalized {
        outcome_code: String,
        receipt_sha256: Option<String>,
    },
    OutcomeUnknown,
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

    /// Reserve only after native consent and provider-side precondition checks.
    pub fn reserve_after_consent(
        &self,
        request: &PrivilegeApplyRequestV1,
        origin_principal_digest: &str,
        now_utc_ms: i64,
    ) -> Result<PrivilegeProviderReserveDecision, CuError> {
        validate_digest(
            origin_principal_digest,
            "privilege_origin_principal_invalid",
        )?;
        let now_for_plan = u64::try_from(now_utc_ms).map_err(|_| {
            CuError::new(
                "privilege_provider_clock_invalid",
                "provider clock must be non-negative UTC milliseconds",
            )
        })?;
        validate_apply_request(request, now_for_plan)?;
        let canonical = serde_json::to_vec(request).map_err(|_| {
            CuError::new(
                "privilege_request_invalid",
                "privilege apply request could not be serialized canonically",
            )
        })?;
        let fingerprint = fingerprint_canonical_request(&canonical)?;
        let provider_key =
            provider_key(self.namespace, origin_principal_digest, &request.request_id);
        match self.store.reserve(
            &provider_key,
            &fingerprint,
            PROVIDER_REPLAY_RETENTION_MS,
            now_utc_ms,
        )? {
            ReserveDecision::Fresh(fresh) => Ok(PrivilegeProviderReserveDecision::Fresh(
                PrivilegeProviderReservation {
                    provider_key,
                    fingerprint,
                    fresh,
                },
            )),
            ReserveDecision::ReplayFinalized(status) => {
                let outcome = status.outcome.ok_or_else(|| {
                    CuError::new(
                        "privilege_provider_state_invalid",
                        "finalized provider reservation has no outcome",
                    )
                })?;
                Ok(PrivilegeProviderReserveDecision::ReplayFinalized {
                    outcome_code: outcome.code,
                    receipt_sha256: outcome.receipt_sha256,
                })
            }
            ReserveDecision::Uncertain(_) => Ok(PrivilegeProviderReserveDecision::OutcomeUnknown),
        }
    }

    pub fn finalize(
        &self,
        reservation: &PrivilegeProviderReservation,
        succeeded: bool,
        outcome_code: &str,
        receipt_sha256: Option<String>,
        now_utc_ms: i64,
    ) -> Result<(), CuError> {
        let kind = if succeeded {
            FinalOutcomeKind::Succeeded
        } else {
            FinalOutcomeKind::Failed
        };
        self.store.finalize(
            &reservation.provider_key,
            &reservation.fingerprint,
            &reservation.fresh.completion_token,
            FinalOutcome::new(kind, outcome_code, receipt_sha256)?,
            now_utc_ms,
        )?;
        Ok(())
    }

    pub fn mark_outcome_unknown(
        &self,
        reservation: &PrivilegeProviderReservation,
        now_utc_ms: i64,
    ) -> Result<(), CuError> {
        self.store.mark_outcome_unknown(
            &reservation.provider_key,
            &reservation.fingerprint,
            &reservation.fresh.completion_token,
            now_utc_ms,
        )?;
        Ok(())
    }
}

/// Parse and validate the untrusted wire bytes before any consent interaction.
///
/// The returned fingerprint binds the entire canonical request. It is safe to
/// persist as provider idempotency metadata; the request body is not.
pub fn parse_apply_request(
    bytes: &[u8],
    now_utc_ms: u64,
) -> Result<(PrivilegeApplyRequestV1, String), CuError> {
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
    validate_apply_request(&request, now_utc_ms)?;
    let canonical = serde_json::to_vec(&request).map_err(|_| {
        CuError::new(
            "privilege_request_invalid",
            "privilege apply request could not be serialized canonically",
        )
    })?;
    Ok((request, fingerprint_canonical_request(&canonical)?))
}

fn validate_apply_request(
    request: &PrivilegeApplyRequestV1,
    now_utc_ms: u64,
) -> Result<(), CuError> {
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
    validate_process_priority_plan(&request.plan, now_utc_ms)?;
    Ok(())
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
    use crate::privilege_plan::process_priority_plan;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn request(now: u64) -> PrivilegeApplyRequestV1 {
        let pid = std::process::id();
        let nice = agenterm_platform::process_metrics::nice(pid).unwrap();
        PrivilegeApplyRequestV1 {
            protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: "request-01".into(),
            plan: process_priority_plan(pid, nice, 120, now).unwrap(),
            origin: PrivilegeOriginV1 {
                session_id: "session-01".into(),
                target_scope: PrivilegeTargetScope::Current,
            },
            client: PrivilegeClientV1 {
                contract_version: PRIVILEGE_PROVIDER_CONTRACT_VERSION,
            },
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn closed_request_round_trips_with_stable_fingerprint() {
        let request = request(1_000);
        let bytes = serde_json::to_vec(&request).unwrap();
        let (parsed, first) = parse_apply_request(&bytes, 1_001).unwrap();
        let (_, second) = parse_apply_request(&bytes, 1_001).unwrap();
        assert_eq!(parsed, request);
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unknown_fields_tampering_and_expiry_fail_before_provider_dispatch() {
        let request = request(1_000);
        let mut value = serde_json::to_value(&request).unwrap();
        value["password"] = serde_json::json!("must-never-enter-this-protocol");
        assert_eq!(
            parse_apply_request(&serde_json::to_vec(&value).unwrap(), 1_001)
                .unwrap_err()
                .code,
            "privilege_request_invalid"
        );

        let mut tampered = request.clone();
        tampered.plan.after.nice = if tampered.plan.after.nice == 20 {
            19
        } else {
            tampered.plan.after.nice + 1
        };
        assert_eq!(
            parse_apply_request(&serde_json::to_vec(&tampered).unwrap(), 1_001)
                .unwrap_err()
                .code,
            "privilege_plan_digest_mismatch"
        );
        assert_eq!(
            parse_apply_request(
                &serde_json::to_vec(&request).unwrap(),
                request.plan.expires_at_utc_ms + 1,
            )
            .unwrap_err()
            .code,
            "privilege_plan_expired"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn identifiers_are_bounded_and_do_not_accept_whitespace() {
        let mut request = request(1_000);
        request.request_id = "bad request".into();
        assert_eq!(
            parse_apply_request(&serde_json::to_vec(&request).unwrap(), 1_001)
                .unwrap_err()
                .code,
            "privilege_request_id_invalid"
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
        let request = request(1_000);
        let principal = sha256_hex(b"fixture-principal");

        let first = match ledger
            .reserve_after_consent(&request, &principal, 1_001)
            .unwrap()
        {
            PrivilegeProviderReserveDecision::Fresh(reservation) => reservation,
            other => panic!("expected fresh reservation, got {other:?}"),
        };
        assert!(matches!(
            ledger
                .reserve_after_consent(&request, &principal, 1_002)
                .unwrap(),
            PrivilegeProviderReserveDecision::OutcomeUnknown
        ));
        ledger.mark_outcome_unknown(&first, 1_003).unwrap();
        assert!(matches!(
            ledger
                .reserve_after_consent(&request, &principal, 1_004)
                .unwrap(),
            PrivilegeProviderReserveDecision::OutcomeUnknown
        ));

        let mut completed_request = request.clone();
        completed_request.request_id = "request-02".into();
        let completed = match ledger
            .reserve_after_consent(&completed_request, &principal, 1_005)
            .unwrap()
        {
            PrivilegeProviderReserveDecision::Fresh(reservation) => reservation,
            other => panic!("expected fresh reservation, got {other:?}"),
        };
        let receipt = sha256_hex(b"fixture-receipt");
        ledger
            .finalize(&completed, true, "completed", Some(receipt.clone()), 1_006)
            .unwrap();
        match ledger
            .reserve_after_consent(&completed_request, &principal, 1_007)
            .unwrap()
        {
            PrivilegeProviderReserveDecision::ReplayFinalized {
                outcome_code,
                receipt_sha256,
            } => {
                assert_eq!(outcome_code, "completed");
                assert_eq!(receipt_sha256.as_deref(), Some(receipt.as_str()));
            }
            other => panic!("expected finalized replay, got {other:?}"),
        }

        let mut changed = completed_request.clone();
        changed.origin.session_id = "session-02".into();
        assert_eq!(
            ledger
                .reserve_after_consent(&changed, &principal, 1_008)
                .unwrap_err()
                .code,
            "request_id_conflict"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
