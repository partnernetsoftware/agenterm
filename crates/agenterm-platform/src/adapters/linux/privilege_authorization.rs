//! Linux polkit adapter for the fixed system privilege broker.

use std::{
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::time::{MissedTickBehavior, timeout};
use zbus::{Connection, Proxy, zvariant::Value};

use crate::{
    privilege_authorization::{
        PRIVILEGE_ACTION_ID, PrivilegeAuthorizationDecision, PrivilegeAuthorizationError,
        PrivilegeAuthorizationErrorKind, PrivilegeAuthorizationResult,
    },
    system_broker::{SystemBrokerPeerFacts, SystemBrokerStream},
};

const POLKIT_DESTINATION: &str = "org.freedesktop.PolicyKit1";
const POLKIT_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const POLKIT_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";
const ALLOW_USER_INTERACTION: u32 = 1;
const MAX_DETAIL_ENTRIES: usize = 32;
const MAX_DETAIL_FIELD_BYTES: usize = 256;
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
const PEER_LIVENESS_INTERVAL: Duration = Duration::from_millis(25);

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
    Peer,
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
    async fn check(
        &self,
        peer: &SystemBrokerPeerFacts,
        cancellation_id: &str,
        deadline: Duration,
    ) -> Result<RawAuthorizationResult, TransportError>;

    async fn cancel(&self, cancellation_id: &str, deadline: Duration)
    -> Result<(), TransportError>;
}

trait PeerLiveness {
    fn is_alive(&self) -> Result<bool, TransportError>;
}

impl PeerLiveness for SystemBrokerStream {
    fn is_alive(&self) -> Result<bool, TransportError> {
        self.peer_is_alive().map_err(|error| TransportError {
            kind: TransportErrorKind::Peer,
            message: format!("cannot inspect retained system broker peer: {error}"),
        })
    }
}

struct ZbusTransport {
    connection: Connection,
}

pub(super) fn authorize(
    peer: &SystemBrokerStream,
    deadline: Duration,
) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
    validate_peer(peer.peer())?;
    let started = Instant::now();
    let transport = ZbusTransport::connect(deadline)?;
    let remaining = deadline.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(PrivilegeAuthorizationError::new(
            PrivilegeAuthorizationErrorKind::TimedOut,
            "polkit authorization deadline expired while connecting to the system bus",
        ));
    }
    runtime()?.block_on(authorize_with_transport(
        peer.peer(),
        remaining,
        &transport,
        peer,
        PEER_LIVENESS_INTERVAL,
    ))
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
    async fn check(
        &self,
        peer: &SystemBrokerPeerFacts,
        cancellation_id: &str,
        deadline: Duration,
    ) -> Result<RawAuthorizationResult, TransportError> {
        let result = timeout(deadline, async {
            let proxy = authority_proxy(&self.connection).await?;
            let mut subject_details = HashMap::with_capacity(3);
            subject_details.insert("pid", Value::from(peer.process_id));
            subject_details.insert("start-time", Value::from(peer.start_ticks));
            subject_details.insert(
                "uid",
                Value::from(
                    i32::try_from(peer.effective_user_id).map_err(|_| TransportError {
                        kind: TransportErrorKind::Call,
                        message: "system broker peer uid does not fit the polkit wire type".into(),
                    })?,
                ),
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
    }

    async fn cancel(
        &self,
        cancellation_id: &str,
        deadline: Duration,
    ) -> Result<(), TransportError> {
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

async fn authorize_with_transport(
    peer: &SystemBrokerPeerFacts,
    deadline: Duration,
    transport: &impl AuthorizationTransport,
    liveness: &impl PeerLiveness,
    liveness_interval: Duration,
) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
    validate_peer(peer)?;
    let cancellation_id = next_cancellation_id();
    let check = transport.check(peer, &cancellation_id, deadline);
    tokio::pin!(check);
    let first_probe = tokio::time::Instant::now() + liveness_interval;
    let mut probes = tokio::time::interval_at(first_probe, liveness_interval);
    probes.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let result = loop {
        tokio::select! {
            result = &mut check => break result,
            _ = probes.tick() => match liveness.is_alive() {
                Ok(true) => {}
                Ok(false) => {
                    cancel_pending_check(
                        transport,
                        &cancellation_id,
                        deadline,
                        "caller exited while polkit authorization was pending",
                    ).await?;
                    return Err(PrivilegeAuthorizationError::new(
                        PrivilegeAuthorizationErrorKind::StalePeer,
                        "system broker peer exited while polkit authorization was pending",
                    ));
                }
                Err(error) => {
                    cancel_pending_check(
                        transport,
                        &cancellation_id,
                        deadline,
                        "caller liveness became unavailable while polkit authorization was pending",
                    ).await?;
                    return Err(map_transport_error(error));
                }
            }
        }
    };
    match result {
        Ok(result) => classify_result(result),
        Err(error) if error.kind == TransportErrorKind::TimedOut => {
            cancel_pending_check(
                transport,
                &cancellation_id,
                deadline,
                "polkit authorization timed out",
            )
            .await?;
            Err(PrivilegeAuthorizationError::new(
                PrivilegeAuthorizationErrorKind::TimedOut,
                error.message,
            ))
        }
        Err(error) => Err(map_transport_error(error)),
    }
}

async fn cancel_pending_check(
    transport: &impl AuthorizationTransport,
    cancellation_id: &str,
    deadline: Duration,
    cause: &str,
) -> PrivilegeAuthorizationResult<()> {
    transport
        .cancel(cancellation_id, deadline.min(CANCEL_TIMEOUT))
        .await
        .map_err(|error| {
            PrivilegeAuthorizationError::new(
                PrivilegeAuthorizationErrorKind::CancellationFailed,
                format!(
                    "{cause} and the check could not be canceled: {}",
                    error.message
                ),
            )
        })
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
        TransportErrorKind::Peer => PrivilegeAuthorizationErrorKind::InvalidPeer,
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
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering as AtomicOrdering},
        },
    };

    struct FakeTransport {
        result: Mutex<Option<Result<RawAuthorizationResult, TransportError>>>,
        check_delay: Option<Duration>,
        cancel_result: Mutex<Option<Result<(), TransportError>>>,
        cancellation_id: Mutex<Option<String>>,
        cancel_id: Mutex<Option<String>>,
        calls: AtomicUsize,
        cancel_calls: AtomicUsize,
    }

    impl AuthorizationTransport for FakeTransport {
        async fn check(
            &self,
            _peer: &SystemBrokerPeerFacts,
            cancellation_id: &str,
            _deadline: Duration,
        ) -> Result<RawAuthorizationResult, TransportError> {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            *self.cancellation_id.lock().expect("cancellation id lock") =
                Some(cancellation_id.into());
            match self.check_delay {
                Some(delay) => tokio::time::sleep(delay).await,
                None => std::future::pending().await,
            }
            self.result
                .lock()
                .expect("check result lock")
                .take()
                .expect("one check result")
        }

        async fn cancel(
            &self,
            cancellation_id: &str,
            _deadline: Duration,
        ) -> Result<(), TransportError> {
            self.cancel_calls.fetch_add(1, AtomicOrdering::Relaxed);
            *self.cancel_id.lock().expect("cancel id lock") = Some(cancellation_id.into());
            self.cancel_result
                .lock()
                .expect("cancel result lock")
                .take()
                .expect("one cancel result")
        }
    }

    struct FakeLiveness {
        results: Mutex<VecDeque<Result<bool, TransportError>>>,
        calls: AtomicUsize,
    }

    impl PeerLiveness for FakeLiveness {
        fn is_alive(&self) -> Result<bool, TransportError> {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            self.results
                .lock()
                .expect("liveness result lock")
                .pop_front()
                .unwrap_or(Ok(true))
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
            result: Mutex::new(Some(result)),
            check_delay: Some(Duration::ZERO),
            cancel_result: Mutex::new(Some(Ok(()))),
            cancellation_id: Mutex::new(None),
            cancel_id: Mutex::new(None),
            calls: AtomicUsize::new(0),
            cancel_calls: AtomicUsize::new(0),
        }
    }

    fn live() -> FakeLiveness {
        FakeLiveness {
            results: Mutex::new(VecDeque::new()),
            calls: AtomicUsize::new(0),
        }
    }

    fn authorize_fake(
        transport: &FakeTransport,
        liveness: &FakeLiveness,
    ) -> PrivilegeAuthorizationResult<PrivilegeAuthorizationDecision> {
        runtime()
            .expect("test runtime")
            .block_on(authorize_with_transport(
                &peer(),
                Duration::from_millis(50),
                transport,
                liveness,
                Duration::from_millis(1),
            ))
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
            let transport = fake(Ok(raw));
            assert_eq!(
                authorize_fake(&transport, &live()).expect("valid decision"),
                expected
            );
            assert_eq!(transport.calls.load(AtomicOrdering::Relaxed), 1);
            assert_eq!(transport.cancel_calls.load(AtomicOrdering::Relaxed), 0);
        }
    }

    #[test]
    fn rejects_contradictory_or_unbounded_native_results() {
        let contradictory = fake(Ok(reply(true, true, &[])));
        assert_eq!(
            authorize_fake(&contradictory, &live())
                .expect_err("contradictory result")
                .kind(),
            PrivilegeAuthorizationErrorKind::InvalidNativeResult
        );

        let details = (0..=MAX_DETAIL_ENTRIES)
            .map(|index| (format!("k{index}"), "v".to_owned()))
            .collect();
        let unbounded = fake(Ok(RawAuthorizationResult {
            authorized: false,
            challenge: false,
            details,
        }));
        assert_eq!(
            authorize_fake(&unbounded, &live())
                .expect_err("unbounded result")
                .kind(),
            PrivilegeAuthorizationErrorKind::InvalidNativeResult
        );
    }

    #[test]
    fn timeout_cancels_on_the_same_transport_identity() {
        let transport = fake(Err(TransportError {
            kind: TransportErrorKind::TimedOut,
            message: "fixture timeout".into(),
        }));
        assert_eq!(
            authorize_fake(&transport, &live())
                .expect_err("timed out")
                .kind(),
            PrivilegeAuthorizationErrorKind::TimedOut
        );
        assert_eq!(transport.cancel_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(
            *transport.cancel_id.lock().expect("cancel id lock"),
            *transport
                .cancellation_id
                .lock()
                .expect("cancellation id lock")
        );
    }

    #[test]
    fn timeout_with_failed_cancel_is_unknown_not_plain_timeout() {
        let transport = fake(Err(TransportError {
            kind: TransportErrorKind::TimedOut,
            message: "fixture timeout".into(),
        }));
        *transport.cancel_result.lock().expect("cancel result lock") = Some(Err(TransportError {
            kind: TransportErrorKind::Cancel,
            message: "fixture cancel failure".into(),
        }));
        assert_eq!(
            authorize_fake(&transport, &live())
                .expect_err("cancel failed")
                .kind(),
            PrivilegeAuthorizationErrorKind::CancellationFailed
        );
        assert_eq!(transport.cancel_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(
            *transport.cancel_id.lock().expect("cancel id lock"),
            *transport
                .cancellation_id
                .lock()
                .expect("cancellation id lock")
        );
    }

    #[test]
    fn caller_death_cancels_once_and_cannot_return_effect_authority() {
        let mut transport = fake(Ok(reply(true, false, &[])));
        transport.check_delay = None;
        let liveness = FakeLiveness {
            results: Mutex::new(VecDeque::from([Ok(false)])),
            calls: AtomicUsize::new(0),
        };

        assert_eq!(
            authorize_fake(&transport, &liveness)
                .expect_err("dead caller must not receive effect authority")
                .kind(),
            PrivilegeAuthorizationErrorKind::StalePeer
        );
        assert_eq!(transport.calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(transport.cancel_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(liveness.calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(
            *transport.cancel_id.lock().expect("cancel id lock"),
            *transport
                .cancellation_id
                .lock()
                .expect("cancellation id lock")
        );
    }

    #[test]
    fn live_caller_can_approve_without_cancellation() {
        let mut transport = fake(Ok(reply(true, false, &[])));
        transport.check_delay = Some(Duration::from_millis(5));
        let liveness = live();

        assert_eq!(
            authorize_fake(&transport, &liveness).expect("live caller approval"),
            PrivilegeAuthorizationDecision::Authorized
        );
        assert_eq!(transport.cancel_calls.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn polkit_cancel_remains_typed_without_a_second_cancel_request() {
        let transport = fake(Err(TransportError {
            kind: TransportErrorKind::AuthorizationCanceled,
            message: "fixture user cancellation".into(),
        }));

        assert_eq!(
            authorize_fake(&transport, &live())
                .expect_err("user cancellation")
                .kind(),
            PrivilegeAuthorizationErrorKind::AuthorizationCanceled
        );
        assert_eq!(transport.cancel_calls.load(AtomicOrdering::Relaxed), 0);
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
