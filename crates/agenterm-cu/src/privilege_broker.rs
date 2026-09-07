//! Transport-neutral privilege broker admission state machine.
//!
//! The platform adapter authenticates the native peer and supplies the native
//! consent callback. This module enforces the important ordering: provider
//! replay lookup first, native consent only for a fresh request, then the
//! second durable reservation and one effect attempt.

use crate::{
    CuError,
    privilege_apply::{PrivilegeApplyReplyV1, PrivilegeApplyRequestV1},
    privilege_provider::{
        FixedProviderAuthority, PreConsentDecision, cancel_before_effect,
        execute_after_native_consent, lookup_before_native_consent, refuse_before_effect,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NativeConsentDecision {
    Authorized,
    Canceled,
    Refused { error_code: String },
}

/// Process one authenticated request. The callback is never invoked for a
/// finalized replay, uncertain reservation, malformed request, or fingerprint
/// conflict; those paths are decided from provider-private state first.
pub(crate) fn process_authenticated_request(
    authority: &FixedProviderAuthority,
    request_bytes: &[u8],
    now_utc_ms: i64,
    consent: impl FnOnce(&PrivilegeApplyRequestV1) -> Result<NativeConsentDecision, CuError>,
) -> Result<PrivilegeApplyReplyV1, CuError> {
    match lookup_before_native_consent(authority, request_bytes, now_utc_ms)? {
        PreConsentDecision::Reply(reply) => Ok(reply),
        PreConsentDecision::Missing(pending) => match consent(pending.request())? {
            NativeConsentDecision::Authorized => {
                execute_after_native_consent(authority, pending, now_utc_ms)
            }
            NativeConsentDecision::Canceled => Ok(cancel_before_effect(pending)),
            NativeConsentDecision::Refused { error_code } => {
                Ok(refuse_before_effect(pending, &error_code))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        process::{Child, Command},
        sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{
        command::ProcessSignalKind,
        privilege_apply::{
            PRIVILEGE_APPLY_PROTOCOL_VERSION, PRIVILEGE_PROVIDER_CONTRACT_VERSION,
            PrivilegeAuthorizationV1, PrivilegeClientV1, PrivilegeOriginV1, PrivilegePlanV1,
            PrivilegeTargetScope,
        },
        privilege_plan::process_signal_plan,
    };

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn fixture_root() -> PathBuf {
        std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "agenterm-cu-broker-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
    }

    fn request(child: &Child, request_id: &str) -> Vec<u8> {
        let plan = process_signal_plan(
            child.id(),
            ProcessSignalKind::Stop,
            false,
            false,
            5_000,
            16,
            120,
            1_000,
        )
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

    fn terminate(mut child: Child) {
        let reference =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(
                child.id(),
            )
            .unwrap();
        let _ = reference.set_suspended(false);
        reference
            .terminate(agenterm_platform::process_control::TerminationMode::Forceful)
            .unwrap();
        child.wait().unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn finalized_replay_never_reopens_native_consent() {
        let root = fixture_root();
        let authority = FixedProviderAuthority::fixture(root.clone());
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let bytes = request(&child, "broker-replay");
        let calls = AtomicUsize::new(0);

        let first = process_authenticated_request(&authority, &bytes, 1_001, |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(NativeConsentDecision::Authorized)
        })
        .unwrap();
        assert!(matches!(first, PrivilegeApplyReplyV1::Completed { .. }));
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        let replay = process_authenticated_request(&authority, &bytes, 1_002, |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(NativeConsentDecision::Authorized)
        })
        .unwrap();
        assert_eq!(first, replay);
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        let reference =
            agenterm_platform::process_reference::ProcessReference::open_for_termination(
                child.id(),
            )
            .unwrap();
        reference.set_suspended(false).unwrap();
        terminate(child);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn canceled_consent_creates_no_reservation_and_can_retry() {
        let root = fixture_root();
        let authority = FixedProviderAuthority::fixture(root.clone());
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let bytes = request(&child, "broker-cancel");

        let refused = process_authenticated_request(&authority, &bytes, 1_001, |_| {
            Ok(NativeConsentDecision::Refused {
                error_code: "privilege_consent_denied".into(),
            })
        })
        .unwrap();
        assert!(matches!(
            refused,
            PrivilegeApplyReplyV1::Refused { ref error_code, .. }
                if error_code == "privilege_consent_denied"
        ));

        let canceled = process_authenticated_request(&authority, &bytes, 1_002, |_| {
            Ok(NativeConsentDecision::Canceled)
        })
        .unwrap();
        assert!(matches!(
            canceled,
            PrivilegeApplyReplyV1::ConsentCanceled { .. }
        ));

        let retried = process_authenticated_request(&authority, &bytes, 1_003, |_| {
            Ok(NativeConsentDecision::Authorized)
        })
        .unwrap();
        assert!(matches!(retried, PrivilegeApplyReplyV1::Completed { .. }));

        terminate(child);
        std::fs::remove_dir_all(root).unwrap();
    }
}
