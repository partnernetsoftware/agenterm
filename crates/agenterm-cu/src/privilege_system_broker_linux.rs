//! Linux systemd-activated root privilege broker.

use std::time::Duration;

use agenterm_platform::{
    privilege_authorization::{
        PrivilegeAuthorizationDecision, PrivilegeAuthorizationErrorKind, authorize_user_initiated,
    },
    system_broker::{SystemBrokerErrorCode, SystemBrokerListener, SystemBrokerStream},
};

use crate::{
    CuError,
    privilege_broker::{
        NativeConsentDecision, PrivilegeBrokerEvent, process_authenticated_request_observed,
    },
    privilege_broker_metrics::PrivilegeBrokerMetricsStore,
    privilege_broker_wire::{read_request, write_reply},
    privilege_provider_linux::authority_for_uid,
};

const IDLE_EXIT: Duration = Duration::from_secs(30);
const CONNECTION_DEADLINE: Duration = Duration::from_secs(120);

pub(crate) fn run_systemd() -> i32 {
    match run_systemd_result() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{}", error.code);
            3
        }
    }
}

fn run_systemd_result() -> Result<(), CuError> {
    let listener = SystemBrokerListener::from_systemd_activation().map_err(platform_error)?;
    loop {
        let mut stream = match listener.accept(IDLE_EXIT) {
            Ok(stream) => stream,
            Err(error) if error.code() == SystemBrokerErrorCode::AcceptTimeout => return Ok(()),
            Err(error) => return Err(platform_error(error)),
        };
        if let Err(error) = process_connection(&mut stream) {
            // A malformed or disconnected client must not terminate the
            // system-activated authority service or expose request contents.
            eprintln!("{}", error.code);
        }
    }
}

fn process_connection(stream: &mut SystemBrokerStream) -> Result<(), CuError> {
    stream
        .set_io_timeout(CONNECTION_DEADLINE)
        .map_err(platform_error)?;
    let request = read_request(&mut *stream)?;
    if !stream.peer_is_alive().map_err(platform_error)? {
        return Err(CuError::new(
            "privilege_origin_stale",
            "the authenticated privilege requester exited before admission",
        ));
    }
    let authority = authority_for_uid(stream.peer().effective_user_id)?;
    let metrics = PrivilegeBrokerMetricsStore::fixed();
    let now_utc_ms = current_time_ms()?;
    let reply = process_authenticated_request_observed(
        &authority,
        &request,
        now_utc_ms,
        |_| match authorize_user_initiated(stream, CONNECTION_DEADLINE) {
            Ok(PrivilegeAuthorizationDecision::Authorized) => Ok(NativeConsentDecision::Authorized),
            Ok(PrivilegeAuthorizationDecision::Denied) => Ok(NativeConsentDecision::Refused {
                error_code: "privilege_consent_denied".into(),
            }),
            Ok(PrivilegeAuthorizationDecision::Dismissed) => Ok(NativeConsentDecision::Canceled),
            Ok(PrivilegeAuthorizationDecision::Challenge) => Ok(NativeConsentDecision::Refused {
                error_code: "privilege_consent_unavailable".into(),
            }),
            Ok(_) => Ok(NativeConsentDecision::Refused {
                error_code: "privilege_consent_protocol_failed".into(),
            }),
            Err(error) => Ok(NativeConsentDecision::Refused {
                error_code: consent_error_code(error.kind()).into(),
            }),
        },
        |event| metrics.record(event),
    )?;
    if let Err(error) = write_reply(&mut *stream, &reply) {
        metrics.record(PrivilegeBrokerEvent::ReplyWriteFailed)?;
        return Err(error);
    }
    stream.shutdown_write().map_err(platform_error)
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

fn platform_error(error: agenterm_platform::system_broker::SystemBrokerError) -> CuError {
    CuError::new(
        "privilege_broker_transport_failed",
        format!("Linux system privilege broker transport failed: {error}"),
    )
}

fn consent_error_code(kind: PrivilegeAuthorizationErrorKind) -> &'static str {
    match kind {
        PrivilegeAuthorizationErrorKind::TimedOut => "privilege_consent_timeout",
        PrivilegeAuthorizationErrorKind::CancellationFailed => {
            "privilege_consent_cancellation_failed"
        }
        PrivilegeAuthorizationErrorKind::StalePeer => "privilege_origin_stale",
        PrivilegeAuthorizationErrorKind::SystemBusUnavailable => {
            "privilege_consent_agent_unavailable"
        }
        PrivilegeAuthorizationErrorKind::InvalidPeer => "privilege_origin_invalid",
        PrivilegeAuthorizationErrorKind::TransportFailed
        | PrivilegeAuthorizationErrorKind::InvalidNativeResult => {
            "privilege_consent_protocol_failed"
        }
        _ => "privilege_consent_protocol_failed",
    }
}
