//! Linux polkit adapter for the fixed system privilege broker.

use std::{
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::time::timeout;
use zbus::{Connection, Proxy, zvariant::Value};

use crate::{
    privilege_authorization::{
        PRIVILEGE_ACTION_ID, PrivilegeAuthorizationDecision, PrivilegeAuthorizationError,
        PrivilegeAuthorizationErrorKind, PrivilegeAuthorizationResult,
    },
    system_broker::SystemBrokerPeerFacts,
};

const POLKIT_DESTINATION: &str = "org.freedesktop.PolicyKit1";
const POLKIT_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const POLKIT_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";
const ALLOW_USER_INTERACTION: u32 = 1;
const MAX_DETAIL_ENTRIES: usize = 32;
const MAX_DETAIL_FIELD_BYTES: usize = 256;
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);

static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
static CANCELLATION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct RawAuthorizationResult {
    authorized: bool,
    challenge: bool,
    details: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransportErrorKind {
    Connect,
    Call,
    TimedOut,
    Cancel,
    AuthorizationCanceled,
    CancellationIdConflict,
    NotAuthorized,
    Unsupported,
}

#[derive(Debug)]
struct TransportError {
    kind: TransportErrorKind,
    message: String,
}

trait AuthorizationTransport {
    fn check(
        &mut self,
        peer: &SystemBrokerPeerFacts,
        cancellation_id: &str,
        deadline: Duration,
    ) -> Result<RawAuthorizationResult, TransportError>;

    fn cancel(&mut self, cancellation_id: &str, deadline: Duration) -> Result<(), TransportError>;
}

struct ZbusTransport {
    connection: Connection,
}

pub(super) fn authorize(
    peer: &SystemBrokerPeerFacts,
    deadline: Duration,
) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
    validate_peer(peer)?;
    let started = Instant::now();
    let mut transport = ZbusTransport::connect(deadline)?;
    let remaining = deadline.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::TimedOut,
            "polkit authorization deadline expired while connecting to the system bus",
        ));
    }
    authorize_with_transport(peer, remaining, &mut transport)
}

fn runtime() -> PrivilegeAuthorizationResult<&'static tokio::runtime::Runtime> {
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("cannot create the polkit runtime: {error}"))
        })
        .as_ref()
        .map_err(|message| {
            PrivilegeAuthorizationError::new(
                PrivilegeAuthorizationErrorKind::SystemBusUnavailable,
                message.clone(),
            )
        })
}

impl ZbusTransport {
    fn connect(deadline: Duration) -> PrivilegeAuthorizationResult<Self> {
        runtime()?
            .block_on(async {
                match timeout(deadline, Connection::system()).await {
                    Ok(Ok(connection)) => Ok(connection),
                    Ok(Err(error)) => Err(PrivilegeAuthorizationError::new(
                        PrivilegeAuthorizationErrorKind::SystemBusUnavailable,
                        format!("cannot connect to the system D-Bus for polkit: {error}"),
                    )),
                    Err(_) => Err(PrivilegeAuthorizationError::new(
                        PrivilegeAuthorizationErrorKind::TimedOut,
                        "timed out connecting to the system D-Bus for polkit",
                    )),
                }
            })
            .map(|connection| Self { connection })
    }
}

impl AuthorizationTransport for ZbusTransport {
    fn check(
        &mut self,
        peer: &SystemBrokerPeerFacts,
        cancellation_id: &str,
        deadline: Duration,
    ) -> Result<RawAuthorizationResult, TransportError> {
        runtime().map_err(privilege_to_transport)?.block_on(async {
            let result = timeout(deadline, async {
                let proxy = authority_proxy(&self.connection).await?;
                let mut subject_details = HashMap::with_capacity(3);
                subject_details.insert("pid", Value::from(peer.process_id));
                subject_details.insert("start-time", Value::from(peer.start_ticks));
                subject_details.insert(
                    "uid",
                    Value::from(i32::try_from(peer.effective_user_id).map_err(|_| {
                        TransportError {
                            kind: TransportErrorKind::Call,
                            message: "system broker peer uid does not fit the polkit wire type"
                                .into(),
                        }
                    })?),
                );
                let subject = ("unix-process", subject_details);
                let details: HashMap<&str, &str> = HashMap::new();
                let arguments = (
                    subject,
                    PRIVILEGE_ACTION_ID,
                    details,
                    ALLOW_USER_INTERACTION,
                    cancellation_id,
                );
                proxy
                    .call::<_, _, (bool, bool, HashMap<String, String>)>(
                        "CheckAuthorization",
                        &arguments,
                    )
                    .await
                    .map_err(map_check_error)
            })
            .await
            .map_err(|_| TransportError {
                kind: TransportErrorKind::TimedOut,
                message: "polkit CheckAuthorization exceeded its deadline".into(),
            })??;
            let (authorized, challenge, details) = result;
            Ok(RawAuthorizationResult {
                authorized,
                challenge,
                details,
            })
        })
    }

    fn cancel(&mut self, cancellation_id: &str, deadline: Duration) -> Result<(), TransportError> {
        runtime().map_err(privilege_to_transport)?.block_on(async {
            timeout(deadline, async {
                let proxy = authority_proxy(&self.connection).await?;
                proxy
                    .call::<_, _, ()>("CancelCheckAuthorization", &(cancellation_id,))
                    .await
                    .map_err(map_cancel_error)
            })
            .await
            .map_err(|_| TransportError {
                kind: TransportErrorKind::Cancel,
                message: "polkit cancellation exceeded its deadline".into(),
            })?
        })
    }
}

async fn authority_proxy(connection: &Connection) -> Result<Proxy<'_>, TransportError> {
    Proxy::new(
        connection,
        POLKIT_DESTINATION,
        POLKIT_PATH,
        POLKIT_INTERFACE,
    )
    .await
    .map_err(|error| TransportError {
        kind: TransportErrorKind::Connect,
        message: format!("cannot create the polkit authority proxy: {error}"),
    })
}

fn privilege_to_transport(error: PrivilegeAuthorizationError) -> TransportError {
    TransportError {
        kind: TransportErrorKind::Connect,
        message: error.to_string(),
    }
}

fn method_error_kind(error: &zbus::Error) -> Option<TransportErrorKind> {
    let zbus::Error::MethodError(name, _, _) = error else {
        return None;
    };
    method_error_kind_name(name.as_str())
}

fn method_error_kind_name(name: &str) -> Option<TransportErrorKind> {
    match name {
        "org.freedesktop.PolicyKit1.Error.Cancelled" => {
            Some(TransportErrorKind::AuthorizationCanceled)
        }
        "org.freedesktop.PolicyKit1.Error.CancellationIdNotUnique" => {
            Some(TransportErrorKind::CancellationIdConflict)
        }
        "org.freedesktop.PolicyKit1.Error.NotAuthorized" => Some(TransportErrorKind::NotAuthorized),
        "org.freedesktop.PolicyKit1.Error.NotSupported" => Some(TransportErrorKind::Unsupported),
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner" => Some(TransportErrorKind::Connect),
        "org.freedesktop.PolicyKit1.Error.Failed" => Some(TransportErrorKind::Call),
        _ => None,
    }
}

fn map_check_error(error: zbus::Error) -> TransportError {
    TransportError {
        kind: method_error_kind(&error).unwrap_or(TransportErrorKind::Call),
        message: format!("polkit CheckAuthorization failed: {error}"),
    }
}

fn map_cancel_error(error: zbus::Error) -> TransportError {
    TransportError {
        kind: method_error_kind(&error).unwrap_or(TransportErrorKind::Cancel),
        message: format!("polkit cancellation failed: {error}"),
    }
}

fn authorize_with_transport(
    peer: &SystemBrokerPeerFacts,
    deadline: Duration,
    transport: &mut impl AuthorizationTransport,
) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
    validate_peer(peer)?;
    let cancellation_id = next_cancellation_id();
    match transport.check(peer, &cancellation_id, deadline) {
        Ok(result) => classify_result(result),
        Err(error) if error.kind == TransportErrorKind::TimedOut => {
            transport
                .cancel(&cancellation_id, deadline.min(CANCEL_TIMEOUT))
                .map_err(|cancel_error| {
                    PrivilegeAuthorizationError::new(
                        PrivilegeAuthorizationErrorKind::CancellationFailed,
                        format!(
                            "polkit authorization timed out and could not be canceled: {}",
                            cancel_error.message
                        ),
                    )
                })?;
            Err(PrivilegeAuthorizationError::new(
                PrivilegeAuthorizationErrorKind::TimedOut,
                error.message,
            ))
        }
        Err(error) => Err(map_transport_error(error)),
    }
}

fn validate_peer(peer: &SystemBrokerPeerFacts) -> PrivilegeAuthorizationResult<()> {
    if peer.process_id == 0 || peer.effective_user_id == 0 || peer.start_ticks == 0 {
        return Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::InvalidPeer,
            "polkit subject requires a live ordinary-user pid, uid and process start time",
        ));
    }
    i32::try_from(peer.effective_user_id).map_err(|_| {
        PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::InvalidPeer,
            "system broker peer uid does not fit the polkit unix-process wire type",
        )
    })?;
    Ok(())
}

fn classify_result(
    result: RawAuthorizationResult,
) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
    validate_details(&result.details)?;
    if result.authorized && result.challenge {
        return Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::InvalidNativeResult,
            "polkit returned contradictory authorized and challenge flags",
        ));
    }
    if result.authorized {
        return Ok(PrivilegeAuthorizationDecision::Authorized);
    }
    if result.challenge {
        return Ok(PrivilegeAuthorizationDecision::Challenge);
    }
    match result.details.get("polkit.dismissed") {
        Some(value) if !value.is_empty() => Ok(PrivilegeAuthorizationDecision::Dismissed),
        Some(_) | None => Ok(PrivilegeAuthorizationDecision::Denied),
    }
}

fn validate_details(details: &HashMap<String, String>) -> PrivilegeAuthorizationResult<()> {
    if details.len() > MAX_DETAIL_ENTRIES {
        return Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::InvalidNativeResult,
            "polkit returned too many authorization details",
        ));
    }
    for (key, value) in details {
        if key.is_empty()
            || key.len() > MAX_DETAIL_FIELD_BYTES
            || value.len() > MAX_DETAIL_FIELD_BYTES
            || key.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err(PrivilegeAuthorizationError::new(
                PrivilegeAuthorizationErrorKind::InvalidNativeResult,
                "polkit returned an invalid authorization detail",
            ));
        }
    }
    Ok(())
}

fn next_cancellation_id() -> String {
    let sequence = CANCELLATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("agenterm-cu-{}-{sequence}", std::process::id())
}

fn map_transport_error(error: TransportError) -> PrivilegeAuthorizationError {
    let kind = match error.kind {
        TransportErrorKind::Connect => PrivilegeAuthorizationErrorKind::SystemBusUnavailable,
        TransportErrorKind::Call => PrivilegeAuthorizationErrorKind::TransportFailed,
        TransportErrorKind::TimedOut => PrivilegeAuthorizationErrorKind::TimedOut,
        TransportErrorKind::Cancel => PrivilegeAuthorizationErrorKind::CancellationFailed,
        TransportErrorKind::AuthorizationCanceled => {
            PrivilegeAuthorizationErrorKind::AuthorizationCanceled
        }
        TransportErrorKind::CancellationIdConflict => {
            PrivilegeAuthorizationErrorKind::CancellationIdConflict
        }
        TransportErrorKind::NotAuthorized => PrivilegeAuthorizationErrorKind::NotAuthorized,
        TransportErrorKind::Unsupported => PrivilegeAuthorizationErrorKind::Unsupported,
    };
    PrivilegeAuthorizationError::new(kind, error.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTransport {
        result: Option<Result<RawAuthorizationResult, TransportError>>,
        cancel_result: Result<(), TransportError>,
        cancellation_id: Option<String>,
        cancel_id: Option<String>,
        calls: usize,
    }

    impl AuthorizationTransport for FakeTransport {
        fn check(
            &mut self,
            _peer: &SystemBrokerPeerFacts,
            cancellation_id: &str,
            _deadline: Duration,
        ) -> Result<RawAuthorizationResult, TransportError> {
            self.calls += 1;
            self.cancellation_id = Some(cancellation_id.into());
            self.result.take().expect("one check result")
        }

        fn cancel(
            &mut self,
            cancellation_id: &str,
            _deadline: Duration,
        ) -> Result<(), TransportError> {
            self.cancel_id = Some(cancellation_id.into());
            std::mem::replace(&mut self.cancel_result, Ok(()))
        }
    }

    fn peer() -> SystemBrokerPeerFacts {
        SystemBrokerPeerFacts {
            process_id: 42,
            effective_user_id: 1000,
            effective_group_id: 1000,
            start_ticks: 9001,
        }
    }

    fn reply(
        authorized: bool,
        challenge: bool,
        details: &[(&str, &str)],
    ) -> RawAuthorizationResult {
        RawAuthorizationResult {
            authorized,
            challenge,
            details: details
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }

    fn fake(result: Result<RawAuthorizationResult, TransportError>) -> FakeTransport {
        FakeTransport {
            result: Some(result),
            cancel_result: Ok(()),
            cancellation_id: None,
            cancel_id: None,
            calls: 0,
        }
    }

    #[test]
    fn classifies_all_non_contradictory_polkit_results() {
        let cases = [
            (
                reply(true, false, &[]),
                PrivilegeAuthorizationDecision::Authorized,
            ),
            (
                reply(false, false, &[]),
                PrivilegeAuthorizationDecision::Denied,
            ),
            (
                reply(false, false, &[("polkit.dismissed", "yes")]),
                PrivilegeAuthorizationDecision::Dismissed,
            ),
            (
                reply(false, false, &[("polkit.dismissed", "false")]),
                PrivilegeAuthorizationDecision::Dismissed,
            ),
            (
                reply(false, true, &[]),
                PrivilegeAuthorizationDecision::Challenge,
            ),
        ];
        for (raw, expected) in cases {
            let mut transport = fake(Ok(raw));
            assert_eq!(
                authorize_with_transport(&peer(), Duration::from_secs(1), &mut transport)
                    .expect("valid decision"),
                expected
            );
            assert_eq!(transport.calls, 1);
            assert!(transport.cancel_id.is_none());
        }
    }

    #[test]
    fn rejects_contradictory_or_unbounded_native_results() {
        let mut contradictory = fake(Ok(reply(true, true, &[])));
        assert_eq!(
            authorize_with_transport(&peer(), Duration::from_secs(1), &mut contradictory)
                .expect_err("contradictory result")
                .kind(),
            PrivilegeAuthorizationErrorKind::InvalidNativeResult
        );

        let details = (0..=MAX_DETAIL_ENTRIES)
            .map(|index| (format!("k{index}"), "v".to_owned()))
            .collect();
        let mut unbounded = fake(Ok(RawAuthorizationResult {
            authorized: false,
            challenge: false,
            details,
        }));
        assert_eq!(
            authorize_with_transport(&peer(), Duration::from_secs(1), &mut unbounded)
                .expect_err("unbounded result")
                .kind(),
            PrivilegeAuthorizationErrorKind::InvalidNativeResult
        );
    }

    #[test]
    fn timeout_cancels_on_the_same_transport_identity() {
        let mut transport = fake(Err(TransportError {
            kind: TransportErrorKind::TimedOut,
            message: "fixture timeout".into(),
        }));
        assert_eq!(
            authorize_with_transport(&peer(), Duration::from_secs(1), &mut transport)
                .expect_err("timed out")
                .kind(),
            PrivilegeAuthorizationErrorKind::TimedOut
        );
        assert_eq!(transport.cancel_id, transport.cancellation_id);
    }

    #[test]
    fn timeout_with_failed_cancel_is_unknown_not_plain_timeout() {
        let mut transport = fake(Err(TransportError {
            kind: TransportErrorKind::TimedOut,
            message: "fixture timeout".into(),
        }));
        transport.cancel_result = Err(TransportError {
            kind: TransportErrorKind::Cancel,
            message: "fixture cancel failure".into(),
        });
        assert_eq!(
            authorize_with_transport(&peer(), Duration::from_secs(1), &mut transport)
                .expect_err("cancel failed")
                .kind(),
            PrivilegeAuthorizationErrorKind::CancellationFailed
        );
        assert_eq!(transport.cancel_id, transport.cancellation_id);
    }

    #[test]
    fn maps_named_polkit_and_bus_errors_without_string_guessing() {
        let cases = [
            (
                "org.freedesktop.PolicyKit1.Error.Cancelled",
                TransportErrorKind::AuthorizationCanceled,
            ),
            (
                "org.freedesktop.PolicyKit1.Error.CancellationIdNotUnique",
                TransportErrorKind::CancellationIdConflict,
            ),
            (
                "org.freedesktop.PolicyKit1.Error.NotAuthorized",
                TransportErrorKind::NotAuthorized,
            ),
            (
                "org.freedesktop.PolicyKit1.Error.NotSupported",
                TransportErrorKind::Unsupported,
            ),
            (
                "org.freedesktop.PolicyKit1.Error.Failed",
                TransportErrorKind::Call,
            ),
            (
                "org.freedesktop.DBus.Error.ServiceUnknown",
                TransportErrorKind::Connect,
            ),
            (
                "org.freedesktop.DBus.Error.NameHasNoOwner",
                TransportErrorKind::Connect,
            ),
        ];
        for (name, expected) in cases {
            assert_eq!(method_error_kind_name(name), Some(expected));
        }
        assert_eq!(method_error_kind_name("org.example.Future"), None);
    }
}
