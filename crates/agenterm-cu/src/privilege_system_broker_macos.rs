//! launchd-activated root privilege broker for macOS.
//!
//! One kernel-authenticated stream carries the exact request, the broker's
//! replay decision, and (only for a fresh request) one bounded Authorization
//! Services external form. A second connection can never substitute the proof.

use std::time::Duration;

use agenterm_platform::{
    privilege_authorization::{
        PrivilegeAuthorizationDecision, PrivilegeAuthorizationErrorKind,
        verify_macos_authorization_proof,
    },
    system_broker::{
        SystemBrokerError, SystemBrokerErrorCode, SystemBrokerListener, SystemBrokerStream,
    },
};

use crate::{
    CuError,
    privilege_apply::{PrivilegeApplyRequestV1, PrivilegePlanV1, parse_apply_request},
    privilege_broker::{NativeConsentDecision, process_authenticated_request_observed},
    privilege_broker_wire::{
        MacosBrokerConsent, MacosServerDisposition, read_macos_broker_consent, read_macos_request,
        write_macos_server_disposition, write_reply,
    },
    privilege_provider_macos::authority_for_peer,
};

const IDLE_EXIT: Duration = Duration::from_secs(30);
const CONNECTION_DEADLINE: Duration = Duration::from_secs(120);

pub(crate) fn run_launchd() -> i32 {
    match run_launchd_result() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{}", error.code);
            3
        }
    }
}

fn run_launchd_result() -> Result<(), CuError> {
    let listener = SystemBrokerListener::from_launchd_activation().map_err(platform_error)?;
    loop {
        let mut stream = match listener.accept(IDLE_EXIT) {
            Ok(stream) => stream,
            Err(error) if error.code() == SystemBrokerErrorCode::AcceptTimeout => return Ok(()),
            Err(error) => return Err(platform_error(error)),
        };
        if let Err(error) = process_connection(&mut stream) {
            // Never log request or proof bytes. One malformed client must not
            // terminate the launchd-owned provider service.
            eprintln!("{}", error.code);
        }
    }
}

fn process_connection(stream: &mut SystemBrokerStream) -> Result<(), CuError> {
    stream
        .set_io_timeout(CONNECTION_DEADLINE)
        .map_err(platform_error)?;
    let request = read_macos_request(&mut *stream)?;
    let validated = parse_apply_request(&request)?;
    require_process_signal(validated.request())?;
    require_live_peer(stream, "admission")?;
    let authority = authority_for_peer(stream.peer())?;
    let now_utc_ms = current_time_ms()?;
    let mut challenged = false;
    let reply = process_authenticated_request_observed(
        &authority,
        &request,
        now_utc_ms,
        |_| {
            challenged = true;
            write_macos_server_disposition(&mut *stream, MacosServerDisposition::ConsentRequired)?;
            match read_macos_broker_consent(&mut *stream)? {
                MacosBrokerConsent::Proof(proof) => {
                    require_live_peer(stream, "before proof verification")?;
                    match verify_macos_authorization_proof(proof) {
                        Ok(PrivilegeAuthorizationDecision::Authorized) => {
                            require_live_peer(stream, "after proof verification")?;
                            Ok(NativeConsentDecision::Authorized)
                        }
                        Ok(_) => Ok(NativeConsentDecision::Refused {
                            error_code: "privilege_consent_protocol_failed".into(),
                        }),
                        Err(error) => Ok(NativeConsentDecision::Refused {
                            error_code: consent_error_code(error.kind()).into(),
                        }),
                    }
                }
                MacosBrokerConsent::Canceled => Ok(NativeConsentDecision::Canceled),
                MacosBrokerConsent::Denied => Ok(NativeConsentDecision::Refused {
                    error_code: "privilege_consent_denied".into(),
                }),
                MacosBrokerConsent::TimedOut => Ok(NativeConsentDecision::Refused {
                    error_code: "privilege_consent_timeout".into(),
                }),
                MacosBrokerConsent::Failed => Ok(NativeConsentDecision::Refused {
                    error_code: "privilege_consent_protocol_failed".into(),
                }),
            }
        },
        |_| Ok(()),
    )?;
    if !challenged {
        write_macos_server_disposition(&mut *stream, MacosServerDisposition::ReplyReady)?;
    }
    write_reply(&mut *stream, &reply)?;
    stream.shutdown_write().map_err(platform_error)
}

fn require_process_signal(request: &PrivilegeApplyRequestV1) -> Result<(), CuError> {
    match &request.plan {
        PrivilegePlanV1::ProcessSignal(_) => Ok(()),
        PrivilegePlanV1::ProcessPriority(_) => Err(CuError::new(
            "privilege_operation_unsupported",
            "the macOS provider right authorizes only process.signal",
        )),
    }
}

fn require_live_peer(stream: &SystemBrokerStream, phase: &'static str) -> Result<(), CuError> {
    match stream.peer_is_alive().map_err(platform_error)? {
        true => Ok(()),
        false => Err(CuError::new(
            "privilege_origin_stale",
            format!("the authenticated macOS privilege requester exited during {phase}"),
        )),
    }
}

fn current_time_ms() -> Result<i64, CuError> {
    let milliseconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| {
            CuError::new(
                "privilege_provider_clock_invalid",
                "provider clock is before the Unix epoch",
            )
        })?
        .as_millis();
    i64::try_from(milliseconds).map_err(|_| {
        CuError::new(
            "privilege_provider_clock_invalid",
            "provider clock exceeds the supported millisecond range",
        )
    })
}

fn consent_error_code(kind: PrivilegeAuthorizationErrorKind) -> &'static str {
    match kind {
        PrivilegeAuthorizationErrorKind::InvalidProof
        | PrivilegeAuthorizationErrorKind::NotAuthorized => "privilege_consent_proof_invalid",
        PrivilegeAuthorizationErrorKind::TimedOut => "privilege_consent_timeout",
        PrivilegeAuthorizationErrorKind::AuthorizationCanceled => "privilege_consent_canceled",
        PrivilegeAuthorizationErrorKind::InvalidPeer
        | PrivilegeAuthorizationErrorKind::StalePeer => "privilege_origin_invalid",
        _ => "privilege_consent_protocol_failed",
    }
}

fn platform_error(error: SystemBrokerError) -> CuError {
    CuError::new(
        "privilege_broker_transport_failed",
        format!("macOS system privilege broker transport failed: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        command::ProcessSignalKind,
        privilege_apply::{
            PRIVILEGE_APPLY_PROTOCOL_VERSION, PRIVILEGE_PROVIDER_CONTRACT_VERSION,
            PrivilegeAuthorizationV1, PrivilegeClientV1, PrivilegeOriginV1, PrivilegeTargetScope,
        },
        privilege_plan::{process_priority_plan, process_signal_plan},
    };

    #[test]
    fn proof_failures_remain_distinct_from_user_cancellation() {
        assert_eq!(
            consent_error_code(PrivilegeAuthorizationErrorKind::InvalidProof),
            "privilege_consent_proof_invalid"
        );
        assert_eq!(
            consent_error_code(PrivilegeAuthorizationErrorKind::AuthorizationCanceled),
            "privilege_consent_canceled"
        );
        assert_eq!(
            consent_error_code(PrivilegeAuthorizationErrorKind::TimedOut),
            "privilege_consent_timeout"
        );
    }

    #[test]
    fn operation_scoped_right_refuses_priority_before_consent() {
        let priority = process_priority_plan(std::process::id(), 0, 60, 1_000_000).unwrap();
        let request = request_with_plan(PrivilegePlanV1::ProcessPriority(priority));
        let error = require_process_signal(&request).unwrap_err();
        assert_eq!(error.code, "privilege_operation_unsupported");

        let signal = process_signal_plan(
            std::process::id(),
            ProcessSignalKind::Stop,
            false,
            false,
            1_000,
            1,
            60,
            1_000_000,
        )
        .unwrap();
        let request = request_with_plan(PrivilegePlanV1::ProcessSignal(signal));
        require_process_signal(&request).unwrap();
    }

    fn request_with_plan(plan: PrivilegePlanV1) -> PrivilegeApplyRequestV1 {
        PrivilegeApplyRequestV1 {
            protocol_version: PRIVILEGE_APPLY_PROTOCOL_VERSION,
            request_id: "macos-right-scope-fixture".into(),
            plan,
            authorization: PrivilegeAuthorizationV1::OneShotNativeConsent,
            origin: PrivilegeOriginV1 {
                session_id: "macos-right-scope-session".into(),
                target_scope: PrivilegeTargetScope::Current,
            },
            client: PrivilegeClientV1 {
                contract_version: PRIVILEGE_PROVIDER_CONTRACT_VERSION,
            },
        }
    }
}
