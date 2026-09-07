//! Fixed-provider transaction coordinator for privileged ACU effects.
//!
//! Platform launchers establish the native trust boundary (Authorization
//! Services, polkit, or UAC) and then construct [`FixedProviderAuthority`].
//! This module owns everything after that boundary: replay lookup, exact
//! native-object preparation, write-ahead reservation, one effect attempt,
//! immutable receipts and terminal replay. It never accepts a provider state
//! path or an asserted principal from the untrusted request bytes.

use std::{
    fs,
    path::{Path, PathBuf},
};

use agenterm_platform::{
    entropy::secure_random_array, filesystem::protect_private_directory,
    filesystem_publish::write_path_atomic_no_clobber,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CuError,
    privilege_apply::{
        AuthenticatedPrivilegePeer, NativeAuthorizationProof, PrivilegeApplyReplyV1,
        PrivilegeAuthorizationV1, PrivilegeProviderFinalOutcome, PrivilegeProviderLedger,
        PrivilegeProviderLookupDecision, PrivilegeProviderNamespace,
        PrivilegeProviderReserveDecision, PrivilegeSignalEffectOutcome, bind_authorized_prepared,
        execute_reserved_signal, parse_apply_request, prepare_privilege_effect,
    },
};

const RECEIPT_SCHEMA: u32 = 1;
const ATTEMPT_SCHEMA: u32 = 2;
const ATTEMPT_KEY_DOMAIN: &[u8] = b"agenterm-cu/privilege-provider-attempt/v1\0";

/// Authority proven by an OS-specific, fixed-identity provider launcher.
///
/// Construction is crate-private on purpose: ordinary CLI input cannot turn a
/// path, uid, SID, audit token or digest string into provider authority.
#[derive(Debug)]
pub(crate) struct FixedProviderAuthority {
    namespace: PrivilegeProviderNamespace,
    state_root: PathBuf,
    peer: AuthenticatedPrivilegePeer,
}

impl FixedProviderAuthority {
    pub(crate) fn from_native_boundary(
        namespace: PrivilegeProviderNamespace,
        state_root: PathBuf,
        principal_digest: String,
        provider_identity_digest: String,
    ) -> Result<Self, CuError> {
        Ok(Self {
            namespace,
            state_root,
            peer: AuthenticatedPrivilegePeer::from_native_provider(
                principal_digest,
                provider_identity_digest,
            )?,
        })
    }

    #[cfg(test)]
    fn fixture(state_root: PathBuf) -> Self {
        Self::fixture_with_provider(state_root, digest(b"fixture-provider"))
    }

    #[cfg(test)]
    fn fixture_with_provider(state_root: PathBuf, provider_identity_digest: String) -> Self {
        Self::from_native_boundary(
            PrivilegeProviderNamespace::Fixture,
            state_root,
            digest(b"fixture-principal"),
            provider_identity_digest,
        )
        .expect("fixture authority")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct AttemptRecord {
    schema: u32,
    receipt_id: String,
    request_fingerprint: String,
    principal_digest: String,
    provider_identity_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderReceipt {
    schema: u32,
    receipt_id: String,
    request_id: String,
    contract_digest: String,
    approval_digest: String,
    provider_identity_digest: String,
    origin_principal_digest: String,
    outcome_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    verification: Option<crate::privilege_apply::PrivilegeVerificationV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence: Option<serde_json::Value>,
}

/// Run exactly one already-consented request inside a fixed provider.
///
/// The platform launcher must complete native consent before constructing the
/// authority. This coordinator accepts only one-shot consent until a native
/// delegated-grant verifier is implemented by the same fixed provider.
pub(crate) fn execute_one_shot(
    authority: &FixedProviderAuthority,
    request_bytes: &[u8],
    now_utc_ms: i64,
) -> Result<PrivilegeApplyReplyV1, CuError> {
    prepare_state_root(&authority.state_root)?;
    let validated = parse_apply_request(request_bytes)?;
    let request = validated.request();
    if request.authorization != PrivilegeAuthorizationV1::OneShotNativeConsent {
        return Ok(refused(request, "privilege_delegated_grant_unavailable"));
    }
    let ledger = PrivilegeProviderLedger::open_at(
        authority.state_root.join("replay.json"),
        authority.namespace,
    )?;
    match ledger.lookup_before_consent(&validated, &authority.peer, now_utc_ms)? {
        PrivilegeProviderLookupDecision::ReplayFinalized {
            outcome_code,
            receipt_id,
            receipt_sha256,
        } => {
            return replay_finalized(
                authority,
                request,
                &outcome_code,
                receipt_id.as_deref(),
                receipt_sha256.as_deref(),
            );
        }
        PrivilegeProviderLookupDecision::OutcomeUnknown => {
            let attempt = load_attempt(authority, validated.fingerprint())?;
            return Ok(outcome_unknown(request, &attempt));
        }
        PrivilegeProviderLookupDecision::Missing => {}
    }

    let prepared = match prepare_privilege_effect(&validated) {
        Ok(prepared) => prepared,
        Err(error) => return Ok(failed_before(request, &error.code)),
    };
    let attempt = load_or_create_attempt(authority, validated.fingerprint())?;
    let authorized = bind_authorized_prepared(
        validated.clone(),
        authority.peer.clone(),
        NativeAuthorizationProof::from_native_provider(
            PrivilegeAuthorizationV1::OneShotNativeConsent,
        ),
        prepared,
    )?;
    let mut execution = match ledger.reserve_authorized(authorized, now_utc_ms)? {
        PrivilegeProviderReserveDecision::Fresh(execution) => execution,
        PrivilegeProviderReserveDecision::ReplayFinalized {
            outcome_code,
            receipt_id,
            receipt_sha256,
        } => {
            let reply = replay_finalized(
                authority,
                request,
                &outcome_code,
                receipt_id.as_deref(),
                receipt_sha256.as_deref(),
            );
            remove_attempt(authority, validated.fingerprint());
            return reply;
        }
        PrivilegeProviderReserveDecision::OutcomeUnknown => {
            return Ok(outcome_unknown(request, &attempt));
        }
    };

    match execute_reserved_signal(&mut execution, &authority.state_root) {
        PrivilegeSignalEffectOutcome::Completed { evidence, verified } => {
            let verification = match verified {
                Some(true) => crate::privilege_apply::PrivilegeVerificationV1::Verified,
                Some(false) => crate::privilege_apply::PrivilegeVerificationV1::Unverified,
                None => crate::privilege_apply::PrivilegeVerificationV1::NotApplicable,
            };
            let receipt = receipt(
                authority,
                request,
                &attempt.receipt_id,
                "completed",
                Some(verification),
                None,
                Some(evidence),
            );
            let receipt_sha256 = match publish_receipt(authority, &receipt) {
                Ok(digest) => digest,
                Err(_) => {
                    let _ = ledger.mark_outcome_unknown(execution, now_utc_ms);
                    return Ok(outcome_unknown(request, &attempt));
                }
            };
            if ledger
                .finalize(
                    execution,
                    PrivilegeProviderFinalOutcome::Completed {
                        outcome_code: "completed".into(),
                        receipt_id: attempt.receipt_id.clone(),
                        receipt_sha256: receipt_sha256.clone(),
                    },
                    now_utc_ms,
                )
                .is_err()
            {
                return Ok(outcome_unknown(request, &attempt));
            }
            remove_attempt(authority, validated.fingerprint());
            Ok(completed_reply(request, &receipt, receipt_sha256))
        }
        PrivilegeSignalEffectOutcome::FailedBeforeEffect(error) => {
            let code = error.code.clone();
            if ledger
                .finalize(
                    execution,
                    PrivilegeProviderFinalOutcome::FailedBeforeEffect {
                        outcome_code: code.clone(),
                    },
                    now_utc_ms,
                )
                .is_err()
            {
                return Ok(outcome_unknown(request, &attempt));
            }
            remove_attempt(authority, validated.fingerprint());
            Ok(failed_before(request, &code))
        }
        PrivilegeSignalEffectOutcome::FailedAfterEffect {
            error,
            outcome_unknown: effect_unknown,
        } => {
            let receipt = receipt(
                authority,
                request,
                &attempt.receipt_id,
                if effect_unknown {
                    "outcome_unknown"
                } else {
                    &error.code
                },
                None,
                Some(error.code.clone()),
                error.detail.clone(),
            );
            let receipt_sha256 = publish_receipt(authority, &receipt).ok();
            if effect_unknown || receipt_sha256.is_none() {
                let _ = ledger.mark_outcome_unknown(execution, now_utc_ms);
                return Ok(outcome_unknown(request, &attempt));
            }
            let receipt_sha256 = receipt_sha256.expect("checked above");
            if ledger
                .finalize(
                    execution,
                    PrivilegeProviderFinalOutcome::FailedAfterEffect {
                        outcome_code: error.code.clone(),
                        receipt_id: attempt.receipt_id.clone(),
                        receipt_sha256: receipt_sha256.clone(),
                    },
                    now_utc_ms,
                )
                .is_err()
            {
                return Ok(outcome_unknown(request, &attempt));
            }
            remove_attempt(authority, validated.fingerprint());
            Ok(failed_after_reply(request, &receipt, receipt_sha256))
        }
    }
}

fn prepare_state_root(root: &Path) -> Result<(), CuError> {
    fs::create_dir_all(root)
        .and_then(|()| protect_private_directory(root))
        .map_err(|error| {
            CuError::new(
                "privilege_provider_state_unavailable",
                format!("provider-owned state root is unavailable: {error}"),
            )
        })?;
    for child in ["attempts", "receipts"] {
        let path = root.join(child);
        fs::create_dir(&path)
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(error)
                }
            })
            .map_err(provider_state_error)?;
        protect_private_directory(&path).map_err(provider_state_error)?;
    }
    Ok(())
}

fn load_or_create_attempt(
    authority: &FixedProviderAuthority,
    fingerprint: &str,
) -> Result<AttemptRecord, CuError> {
    let key = digest_parts(
        ATTEMPT_KEY_DOMAIN,
        &[authority.peer.principal_digest(), fingerprint],
    );
    let path = authority
        .state_root
        .join("attempts")
        .join(format!("{key}.json"));
    if path.exists() {
        return read_attempt(&path, fingerprint, authority.peer.principal_digest());
    }
    let candidate = AttemptRecord {
        schema: ATTEMPT_SCHEMA,
        receipt_id: uuid_v4()?,
        request_fingerprint: fingerprint.to_owned(),
        principal_digest: authority.peer.principal_digest().to_owned(),
        provider_identity_digest: authority.peer.provider_identity_digest().to_owned(),
    };
    let bytes = serde_json::to_vec(&candidate).map_err(provider_state_error)?;
    match write_path_atomic_no_clobber(&path, |temporary| fs::write(temporary, &bytes)) {
        Ok(()) => Ok(candidate),
        Err(error) if path.exists() => {
            let _ = error;
            read_attempt(&path, fingerprint, authority.peer.principal_digest())
        }
        Err(error) => Err(provider_state_error(error)),
    }
}

fn load_attempt(
    authority: &FixedProviderAuthority,
    fingerprint: &str,
) -> Result<AttemptRecord, CuError> {
    let path = attempt_path(authority, fingerprint);
    if !path.exists() {
        return Err(CuError::new(
            "privilege_provider_state_invalid",
            "uncertain provider reservation has no durable attempt identity",
        ));
    }
    read_attempt(&path, fingerprint, authority.peer.principal_digest())
}

fn remove_attempt(authority: &FixedProviderAuthority, fingerprint: &str) {
    let _ = fs::remove_file(attempt_path(authority, fingerprint));
}

fn attempt_path(authority: &FixedProviderAuthority, fingerprint: &str) -> PathBuf {
    let key = digest_parts(
        ATTEMPT_KEY_DOMAIN,
        &[authority.peer.principal_digest(), fingerprint],
    );
    authority
        .state_root
        .join("attempts")
        .join(format!("{key}.json"))
}

fn read_attempt(path: &Path, fingerprint: &str, principal: &str) -> Result<AttemptRecord, CuError> {
    let bytes = fs::read(path).map_err(provider_state_error)?;
    let attempt: AttemptRecord = serde_json::from_slice(&bytes).map_err(provider_state_error)?;
    if attempt.schema != ATTEMPT_SCHEMA
        || attempt.request_fingerprint != fingerprint
        || attempt.principal_digest != principal
        || !valid_uuid_v4(&attempt.receipt_id)
        || !valid_digest(&attempt.provider_identity_digest)
    {
        return Err(CuError::new(
            "privilege_provider_state_invalid",
            "provider attempt identity is malformed or belongs to another request",
        ));
    }
    Ok(attempt)
}

fn publish_receipt(
    authority: &FixedProviderAuthority,
    receipt: &ProviderReceipt,
) -> Result<String, CuError> {
    let bytes = serde_json::to_vec(receipt).map_err(provider_state_error)?;
    let path = receipt_path(authority, &receipt.receipt_id);
    match write_path_atomic_no_clobber(&path, |temporary| fs::write(temporary, &bytes)) {
        Ok(()) => Ok(receipt_digest(&bytes)),
        Err(_) if path.exists() => {
            let existing = fs::read(&path).map_err(provider_state_error)?;
            let parsed: ProviderReceipt =
                serde_json::from_slice(&existing).map_err(provider_state_error)?;
            if parsed != *receipt {
                return Err(CuError::new(
                    "privilege_provider_receipt_conflict",
                    "immutable provider receipt name is already bound to another request",
                ));
            }
            Ok(receipt_digest(&existing))
        }
        Err(error) => Err(provider_state_error(error)),
    }
}

fn replay_finalized(
    authority: &FixedProviderAuthority,
    request: &crate::privilege_apply::PrivilegeApplyRequestV1,
    outcome_code: &str,
    receipt_id: Option<&str>,
    expected_sha256: Option<&str>,
) -> Result<PrivilegeApplyReplyV1, CuError> {
    let Some(receipt_id) = receipt_id else {
        return Ok(failed_before(request, outcome_code));
    };
    let bytes = fs::read(receipt_path(authority, receipt_id)).map_err(provider_state_error)?;
    let actual = receipt_digest(&bytes);
    if expected_sha256 != Some(actual.as_str()) {
        return Err(CuError::new(
            "privilege_provider_receipt_invalid",
            "provider replay receipt digest does not match its durable reservation",
        ));
    }
    let receipt: ProviderReceipt = serde_json::from_slice(&bytes).map_err(provider_state_error)?;
    if receipt.schema != RECEIPT_SCHEMA
        || receipt.receipt_id != receipt_id
        || receipt.outcome_code != outcome_code
        || receipt.request_id != request.request_id
        || receipt.contract_digest != request.plan_contract_digest()
        || receipt.approval_digest != request.plan_approval_digest()
        || receipt.origin_principal_digest != authority.peer.principal_digest()
        || !valid_digest(&receipt.provider_identity_digest)
        || (outcome_code != "completed" && receipt.error_code.as_deref() != Some(outcome_code))
    {
        return Err(CuError::new(
            "privilege_provider_receipt_invalid",
            "provider replay receipt identity does not match the current request and peer",
        ));
    }
    if receipt.outcome_code == "completed" {
        return Ok(completed_reply(request, &receipt, actual));
    }
    Ok(failed_after_reply(request, &receipt, actual))
}

fn receipt(
    authority: &FixedProviderAuthority,
    request: &crate::privilege_apply::PrivilegeApplyRequestV1,
    receipt_id: &str,
    outcome_code: &str,
    verification: Option<crate::privilege_apply::PrivilegeVerificationV1>,
    error_code: Option<String>,
    evidence: Option<serde_json::Value>,
) -> ProviderReceipt {
    ProviderReceipt {
        schema: RECEIPT_SCHEMA,
        receipt_id: receipt_id.to_owned(),
        request_id: request.request_id.clone(),
        contract_digest: request.plan_contract_digest().to_owned(),
        approval_digest: request.plan_approval_digest().to_owned(),
        provider_identity_digest: authority.peer.provider_identity_digest().to_owned(),
        origin_principal_digest: authority.peer.principal_digest().to_owned(),
        outcome_code: outcome_code.to_owned(),
        verification,
        error_code,
        evidence,
    }
}

fn receipt_path(authority: &FixedProviderAuthority, receipt_id: &str) -> PathBuf {
    authority
        .state_root
        .join("receipts")
        .join(format!("{receipt_id}.json"))
}

fn completed_reply(
    request: &crate::privilege_apply::PrivilegeApplyRequestV1,
    receipt: &ProviderReceipt,
    receipt_sha256: String,
) -> PrivilegeApplyReplyV1 {
    PrivilegeApplyReplyV1::Completed {
        protocol_version: crate::privilege_apply::PRIVILEGE_APPLY_PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        contract_digest: receipt.contract_digest.clone(),
        approval_digest: receipt.approval_digest.clone(),
        receipt_id: receipt.receipt_id.clone(),
        receipt_sha256,
        provider_identity_digest: receipt.provider_identity_digest.clone(),
        origin_principal_digest: receipt.origin_principal_digest.clone(),
        verification: receipt
            .verification
            .unwrap_or(crate::privilege_apply::PrivilegeVerificationV1::Unverified),
    }
}

fn failed_before(
    request: &crate::privilege_apply::PrivilegeApplyRequestV1,
    error_code: &str,
) -> PrivilegeApplyReplyV1 {
    PrivilegeApplyReplyV1::FailedBeforeEffect {
        protocol_version: crate::privilege_apply::PRIVILEGE_APPLY_PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        contract_digest: request.plan_contract_digest().to_owned(),
        approval_digest: request.plan_approval_digest().to_owned(),
        error_code: error_code.to_owned(),
    }
}

fn failed_after_reply(
    request: &crate::privilege_apply::PrivilegeApplyRequestV1,
    receipt: &ProviderReceipt,
    receipt_sha256: String,
) -> PrivilegeApplyReplyV1 {
    PrivilegeApplyReplyV1::FailedAfterEffect {
        protocol_version: crate::privilege_apply::PRIVILEGE_APPLY_PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        contract_digest: receipt.contract_digest.clone(),
        approval_digest: receipt.approval_digest.clone(),
        error_code: receipt
            .error_code
            .clone()
            .unwrap_or_else(|| "privilege_effect_failed".into()),
        receipt_id: receipt.receipt_id.clone(),
        receipt_sha256,
        provider_identity_digest: receipt.provider_identity_digest.clone(),
        origin_principal_digest: receipt.origin_principal_digest.clone(),
    }
}

fn outcome_unknown(
    request: &crate::privilege_apply::PrivilegeApplyRequestV1,
    attempt: &AttemptRecord,
) -> PrivilegeApplyReplyV1 {
    PrivilegeApplyReplyV1::OutcomeUnknown {
        protocol_version: crate::privilege_apply::PRIVILEGE_APPLY_PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        contract_digest: request.plan_contract_digest().to_owned(),
        approval_digest: request.plan_approval_digest().to_owned(),
        receipt_id: attempt.receipt_id.clone(),
        provider_identity_digest: attempt.provider_identity_digest.clone(),
        origin_principal_digest: attempt.principal_digest.clone(),
    }
}

fn refused(
    request: &crate::privilege_apply::PrivilegeApplyRequestV1,
    error_code: &str,
) -> PrivilegeApplyReplyV1 {
    PrivilegeApplyReplyV1::Refused {
        protocol_version: crate::privilege_apply::PRIVILEGE_APPLY_PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        contract_digest: request.plan_contract_digest().to_owned(),
        approval_digest: request.plan_approval_digest().to_owned(),
        error_code: error_code.to_owned(),
    }
}

fn uuid_v4() -> Result<String, CuError> {
    let mut bytes = secure_random_array::<16>().map_err(|_| {
        CuError::new(
            "privilege_provider_entropy_unavailable",
            "provider could not obtain a cryptographically random receipt id",
        )
    })?;
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

fn valid_uuid_v4(value: &str) -> bool {
    value.len() == 36
        && value.as_bytes().get(8) == Some(&b'-')
        && value.as_bytes().get(13) == Some(&b'-')
        && value.as_bytes().get(18) == Some(&b'-')
        && value.as_bytes().get(23) == Some(&b'-')
        && value.as_bytes().get(14) == Some(&b'4')
        && matches!(value.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23)
                || byte.is_ascii_digit()
                || (b'a'..=b'f').contains(&byte)
        })
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn receipt_digest(bytes: &[u8]) -> String {
    digest(bytes)
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn digest_parts(domain: &[u8], parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    digest(&hasher.finalize())
}

fn provider_state_error(error: impl std::fmt::Display) -> CuError {
    CuError::new(
        "privilege_provider_state_unavailable",
        format!("provider-owned state could not be persisted: {error}"),
    )
}

trait RequestPlanDigests {
    fn plan_contract_digest(&self) -> &str;
    fn plan_approval_digest(&self) -> &str;
}

impl RequestPlanDigests for crate::privilege_apply::PrivilegeApplyRequestV1 {
    fn plan_contract_digest(&self) -> &str {
        match &self.plan {
            crate::privilege_apply::PrivilegePlanV1::ProcessPriority(plan) => &plan.contract_digest,
            crate::privilege_apply::PrivilegePlanV1::ProcessSignal(plan) => &plan.contract_digest,
        }
    }

    fn plan_approval_digest(&self) -> &str {
        match &self.plan {
            crate::privilege_apply::PrivilegePlanV1::ProcessPriority(plan) => &plan.approval_digest,
            crate::privilege_apply::PrivilegePlanV1::ProcessSignal(plan) => &plan.approval_digest,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        process::{Child, Command},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::{
        command::ProcessSignalKind,
        privilege_apply::{
            PRIVILEGE_APPLY_PROTOCOL_VERSION, PRIVILEGE_PROVIDER_CONTRACT_VERSION,
            PrivilegeApplyRequestV1, PrivilegeClientV1, PrivilegeOriginV1, PrivilegePlanV1,
            PrivilegeTargetScope,
        },
        privilege_plan::process_signal_plan,
    };

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn fixture_root() -> PathBuf {
        std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "agenterm-cu-provider-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
    }

    fn request(child: &Child, request_id: &str, signal: ProcessSignalKind) -> Vec<u8> {
        let plan = process_signal_plan(child.id(), signal, false, false, 5_000, 16, 120, 1_000)
            .expect("signal plan");
        serde_json::to_vec(&PrivilegeApplyRequestV1 {
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
        })
        .expect("request bytes")
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn fixed_provider_executes_once_and_replays_the_immutable_receipt() {
        let root = fixture_root();
        let authority = FixedProviderAuthority::fixture(root.clone());
        let original_provider_identity = authority.peer.provider_identity_digest().to_owned();
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let bytes = request(&child, "provider-once", ProcessSignalKind::Stop);
        let first = execute_one_shot(&authority, &bytes, 1_001).unwrap();
        let PrivilegeApplyReplyV1::Completed {
            receipt_id,
            receipt_sha256,
            verification,
            ..
        } = first
        else {
            panic!("expected completed provider reply")
        };
        assert_eq!(
            verification,
            crate::privilege_apply::PrivilegeVerificationV1::Verified
        );
        assert!(valid_uuid_v4(&receipt_id));
        assert!(agenterm_platform::process_metrics::is_stopped(child.id()).unwrap());
        let upgraded = FixedProviderAuthority::fixture_with_provider(
            root.clone(),
            digest(b"fixture-provider-upgraded"),
        );
        let second = execute_one_shot(&upgraded, &bytes, 1_002).unwrap();
        let PrivilegeApplyReplyV1::Completed {
            receipt_id: replay_id,
            receipt_sha256: replay_sha,
            provider_identity_digest: replay_provider_identity,
            ..
        } = second
        else {
            panic!("expected replayed completion")
        };
        assert_eq!(replay_id, receipt_id);
        assert_eq!(replay_sha, receipt_sha256);
        assert_eq!(replay_provider_identity, original_provider_identity);
        assert_ne!(
            replay_provider_identity,
            upgraded.peer.provider_identity_digest(),
            "provider upgrades must replay the old immutable receipt without repeating the effect"
        );

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
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn provider_refuses_delegated_authority_before_effect() {
        let root = fixture_root();
        let authority = FixedProviderAuthority::fixture(root.clone());
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let mut request: PrivilegeApplyRequestV1 = serde_json::from_slice(&request(
            &child,
            "provider-delegated",
            ProcessSignalKind::Stop,
        ))
        .unwrap();
        request.authorization = PrivilegeAuthorizationV1::DelegatedGrant {
            grant_id: "fixture-grant".into(),
        };
        let reply =
            execute_one_shot(&authority, &serde_json::to_vec(&request).unwrap(), 1_001).unwrap();
        assert!(matches!(
            reply,
            PrivilegeApplyReplyV1::Refused { ref error_code, .. }
                if error_code == "privilege_delegated_grant_unavailable"
        ));
        let reference =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(
                child.id(),
            )
            .unwrap();
        reference
            .terminate(agenterm_platform::process_control::TerminationMode::Forceful)
            .unwrap();
        child.wait().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uuid_and_attempt_identity_are_bounded_and_stable() {
        let root = fixture_root();
        let authority = FixedProviderAuthority::fixture(root.clone());
        prepare_state_root(&root).unwrap();
        let first = load_or_create_attempt(&authority, &"a".repeat(64)).unwrap();
        let upgraded = FixedProviderAuthority::fixture_with_provider(
            root.clone(),
            digest(b"fixture-provider-upgraded"),
        );
        let second = load_or_create_attempt(&upgraded, &"a".repeat(64)).unwrap();
        assert_eq!(first, second);
        assert!(valid_uuid_v4(&first.receipt_id));
        assert_eq!(
            first.provider_identity_digest,
            authority.peer.provider_identity_digest()
        );
        assert_ne!(
            first.provider_identity_digest,
            upgraded.peer.provider_identity_digest()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
