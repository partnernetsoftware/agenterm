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
        execute_after_native_consent_observed, lookup_before_native_consent, refuse_before_effect,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NativeConsentDecision {
    Authorized,
    Canceled,
    Refused { error_code: String },
}

/// Broker-owned milestones used by native courts and protected operational
/// metrics.  Observers run before the named boundary; refusing to persist an
/// event fails closed before consent or effect rather than fabricating proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrivilegeBrokerEvent {
    RequestAccepted,
    ReplayFinalized,
    ReplayOutcomeUnknown,
    RequestConflict,
    NativeConsentStarted,
    EffectAttemptStarted,
    ReplyCompleted,
    ReplyConsentCanceled,
    ReplyRefused,
    ReplyFailedBeforeEffect,
    ReplyFailedAfterEffect,
    ReplyOutcomeUnknown,
    #[cfg(target_os = "linux")]
    ReplyWriteFailed,
}

/// Process one authenticated request. The callback is never invoked for a
/// finalized replay, uncertain reservation, malformed request, or fingerprint
/// conflict; those paths are decided from provider-private state first.
#[cfg(test)]
pub(crate) fn process_authenticated_request(
    authority: &FixedProviderAuthority,
    request_bytes: &[u8],
    now_utc_ms: i64,
    consent: impl FnOnce(&PrivilegeApplyRequestV1) -> Result<NativeConsentDecision, CuError>,
) -> Result<PrivilegeApplyReplyV1, CuError> {
    process_authenticated_request_observed(
        authority,
        request_bytes,
        now_utc_ms,
        consent,
        |_| Ok(()),
    )
}

pub(crate) fn process_authenticated_request_observed(
    authority: &FixedProviderAuthority,
    request_bytes: &[u8],
    now_utc_ms: i64,
    consent: impl FnOnce(&PrivilegeApplyRequestV1) -> Result<NativeConsentDecision, CuError>,
    mut observe: impl FnMut(PrivilegeBrokerEvent) -> Result<(), CuError>,
) -> Result<PrivilegeApplyReplyV1, CuError> {
    observe(PrivilegeBrokerEvent::RequestAccepted)?;
    let decision = match lookup_before_native_consent(authority, request_bytes, now_utc_ms) {
        Ok(decision) => decision,
        Err(error) => {
            if error.code == "request_id_conflict" {
                observe(PrivilegeBrokerEvent::RequestConflict)?;
            }
            return Err(error);
        }
    };
    let reply = match decision {
        PreConsentDecision::Reply(reply) => {
            match &reply {
                PrivilegeApplyReplyV1::OutcomeUnknown { .. } => {
                    observe(PrivilegeBrokerEvent::ReplayOutcomeUnknown)?;
                }
                _ => observe(PrivilegeBrokerEvent::ReplayFinalized)?,
            }
            reply
        }
        PreConsentDecision::Missing(pending) => {
            observe(PrivilegeBrokerEvent::NativeConsentStarted)?;
            match consent(pending.request())? {
                NativeConsentDecision::Authorized => execute_after_native_consent_observed(
                    authority,
                    pending,
                    now_utc_ms,
                    &mut observe,
                )?,
                NativeConsentDecision::Canceled => cancel_before_effect(pending),
                NativeConsentDecision::Refused { error_code } => {
                    refuse_before_effect(pending, &error_code)
                }
            }
        }
    };
    observe(match &reply {
        PrivilegeApplyReplyV1::Completed { .. } => PrivilegeBrokerEvent::ReplyCompleted,
        PrivilegeApplyReplyV1::ConsentCanceled { .. } => PrivilegeBrokerEvent::ReplyConsentCanceled,
        PrivilegeApplyReplyV1::Refused { .. } => PrivilegeBrokerEvent::ReplyRefused,
        PrivilegeApplyReplyV1::FailedBeforeEffect { .. } => {
            PrivilegeBrokerEvent::ReplyFailedBeforeEffect
        }
        PrivilegeApplyReplyV1::FailedAfterEffect { .. } => {
            PrivilegeBrokerEvent::ReplyFailedAfterEffect
        }
        PrivilegeApplyReplyV1::OutcomeUnknown { .. } => PrivilegeBrokerEvent::ReplyOutcomeUnknown,
    })?;
    Ok(reply)
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

        let mut first_events = Vec::new();
        let first = process_authenticated_request_observed(
            &authority,
            &bytes,
            1_001,
            |_| {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(NativeConsentDecision::Authorized)
            },
            |event| {
                first_events.push(event);
                Ok(())
            },
        )
        .unwrap();
        assert!(matches!(first, PrivilegeApplyReplyV1::Completed { .. }));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            first_events,
            [
                PrivilegeBrokerEvent::RequestAccepted,
                PrivilegeBrokerEvent::NativeConsentStarted,
                PrivilegeBrokerEvent::EffectAttemptStarted,
                PrivilegeBrokerEvent::ReplyCompleted,
            ]
        );

        let mut replay_events = Vec::new();
        let replay = process_authenticated_request_observed(
            &authority,
            &bytes,
            1_002,
            |_| {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(NativeConsentDecision::Authorized)
            },
            |event| {
                replay_events.push(event);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(first, replay);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            replay_events,
            [
                PrivilegeBrokerEvent::RequestAccepted,
                PrivilegeBrokerEvent::ReplayFinalized,
                PrivilegeBrokerEvent::ReplyCompleted,
            ]
        );

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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn observer_failure_before_consent_or_effect_fails_closed() {
        let root = fixture_root();
        let authority = FixedProviderAuthority::fixture(root.clone());
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let bytes = request(&child, "broker-observer-failure");
        let consent_calls = AtomicUsize::new(0);

        let error = process_authenticated_request_observed(
            &authority,
            &bytes,
            1_001,
            |_| {
                consent_calls.fetch_add(1, Ordering::Relaxed);
                Ok(NativeConsentDecision::Authorized)
            },
            |event| {
                if event == PrivilegeBrokerEvent::NativeConsentStarted {
                    return Err(CuError::new("fixture_counter_failed", "fixture"));
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "fixture_counter_failed");
        assert_eq!(consent_calls.load(Ordering::Relaxed), 0);

        let error = process_authenticated_request_observed(
            &authority,
            &bytes,
            1_002,
            |_| {
                consent_calls.fetch_add(1, Ordering::Relaxed);
                Ok(NativeConsentDecision::Authorized)
            },
            |event| {
                if event == PrivilegeBrokerEvent::EffectAttemptStarted {
                    return Err(CuError::new("fixture_counter_failed", "fixture"));
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "fixture_counter_failed");
        assert_eq!(consent_calls.load(Ordering::Relaxed), 1);
        assert!(!agenterm_platform::process_metrics::is_stopped(child.id()).unwrap());

        let replay = process_authenticated_request(&authority, &bytes, 1_003, |_| {
            panic!("an uncertain reservation must not reopen native consent")
        })
        .unwrap();
        assert!(matches!(
            replay,
            PrivilegeApplyReplyV1::OutcomeUnknown { .. }
        ));

        terminate(child);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn conflicting_request_is_counted_before_consent_and_never_mutates() {
        let root = fixture_root();
        let authority = FixedProviderAuthority::fixture(root.clone());
        let first_child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let second_child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let first = request(&first_child, "broker-conflict");
        let conflicting = request(&second_child, "broker-conflict");

        process_authenticated_request(&authority, &first, 1_001, |_| {
            Ok(NativeConsentDecision::Authorized)
        })
        .unwrap();
        let mut events = Vec::new();
        let error = process_authenticated_request_observed(
            &authority,
            &conflicting,
            1_002,
            |_| panic!("a conflicting fingerprint must not request consent"),
            |event| {
                events.push(event);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "request_id_conflict");
        assert_eq!(
            events,
            [
                PrivilegeBrokerEvent::RequestAccepted,
                PrivilegeBrokerEvent::RequestConflict,
            ]
        );
        assert!(!agenterm_platform::process_metrics::is_stopped(second_child.id()).unwrap());

        terminate(first_child);
        terminate(second_child);
        std::fs::remove_dir_all(root).unwrap();
    }
}
