//! Connection-local lifecycle for future ACU mutation calls over MCP.
//!
//! This module owns no tool descriptor and invokes no provider. It only decides
//! admission, dispatch ownership, cancellation, completion, and EOF behavior.

const MAX_JSON_RPC_ID_BYTES: usize = 128;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
const MAX_SESSION_ID_BYTES: usize = 128;
const MAX_SESSION_LEASE_BYTES: usize = 512;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum JsonRpcRequestId {
    Integer(i64),
    Unsigned(u64),
    Text(String),
}

impl JsonRpcRequestId {
    pub(crate) fn text(value: impl Into<String>) -> Result<Self, IdentityError> {
        let value = value.into();
        if value.len() > MAX_JSON_RPC_ID_BYTES {
            return Err(IdentityError::InvalidJsonRpcRequestId);
        }
        Ok(Self::Text(value))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct IdempotencyKey(String);

impl IdempotencyKey {
    pub(crate) fn new(value: impl Into<String>) -> Result<Self, IdentityError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_IDEMPOTENCY_KEY_BYTES
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(IdentityError::InvalidIdempotencyKey);
        }
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IdentityError {
    InvalidJsonRpcRequestId,
    InvalidIdempotencyKey,
    InvalidSessionId,
    InvalidSessionLease,
}

pub(crate) struct MutationRequest<T> {
    json_rpc_id: JsonRpcRequestId,
    idempotency_key: IdempotencyKey,
    command: T,
}

impl<T> MutationRequest<T> {
    pub(crate) fn new(
        json_rpc_id: JsonRpcRequestId,
        idempotency_key: IdempotencyKey,
        command: T,
    ) -> Self {
        Self {
            json_rpc_id,
            idempotency_key,
            command,
        }
    }
}

/// In-memory bearer material. Deliberately has no `Debug` or `Display`.
struct PrivateSessionLease(Vec<u8>);

impl PrivateSessionLease {
    fn new(bytes: Vec<u8>) -> Result<Self, IdentityError> {
        if bytes.is_empty() || bytes.len() > MAX_SESSION_LEASE_BYTES {
            return Err(IdentityError::InvalidSessionLease);
        }
        Ok(Self(bytes))
    }

    fn expose_to<R>(&self, consumer: impl FnOnce(&[u8]) -> R) -> R {
        consumer(&self.0)
    }
}

impl Drop for PrivateSessionLease {
    fn drop(&mut self) {
        // This shortens ordinary in-process retention but is only best-effort:
        // Rust does not guarantee that an optimizing compiler preserves it.
        self.0.fill(0);
    }
}

struct SessionIdentity(String);

impl SessionIdentity {
    fn new(value: String) -> Result<Self, IdentityError> {
        if value.is_empty() || value.len() > MAX_SESSION_ID_BYTES {
            return Err(IdentityError::InvalidSessionId);
        }
        Ok(Self(value))
    }
}

struct Dispatched<T> {
    request: MutationRequest<T>,
    cancellation_requested: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionPhase {
    Open,
    EofDraining,
    SessionEnding(SessionEndPhase),
    Ended,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionEndPhase {
    Ready,
    Dispatched,
}

/// Holds at most one provider-dispatched call and one queued call.
pub(crate) struct ConnectionMutationState<T> {
    phase: ConnectionPhase,
    session_id: Option<SessionIdentity>,
    lease: Option<PrivateSessionLease>,
    dispatched: Option<Dispatched<T>>,
    queued: Option<MutationRequest<T>>,
    session_end_record: Option<SessionEndRecord>,
}

impl<T> ConnectionMutationState<T> {
    pub(crate) fn new(
        session_id: impl Into<String>,
        session_lease: Vec<u8>,
    ) -> Result<Self, IdentityError> {
        Ok(Self {
            phase: ConnectionPhase::Open,
            session_id: Some(SessionIdentity::new(session_id.into())?),
            lease: Some(PrivateSessionLease::new(session_lease)?),
            dispatched: None,
            queued: None,
            session_end_record: None,
        })
    }

    pub(crate) fn submit(&mut self, request: MutationRequest<T>) -> SubmitResult {
        if self.phase != ConnectionPhase::Open {
            return SubmitResult::RejectedAfterEof;
        }
        if self.contains_json_rpc_id(&request.json_rpc_id) {
            return SubmitResult::DuplicateJsonRpcId;
        }
        if self.contains_idempotency_key(&request.idempotency_key) {
            return SubmitResult::DuplicateIdempotencyKey;
        }
        if self.queued.is_some() {
            return SubmitResult::Busy;
        }
        self.queued = Some(request);
        SubmitResult::Queued
    }

    /// Moves the sole queued request across the provider-dispatch boundary.
    /// No provider callback is made here.
    pub(crate) fn begin_dispatch(&mut self) -> Option<DispatchView<'_, T>> {
        if self.phase != ConnectionPhase::Open || self.dispatched.is_some() {
            return None;
        }
        self.dispatched = self.queued.take().map(|request| Dispatched {
            request,
            cancellation_requested: false,
        });
        let dispatched = self.dispatched.as_ref()?;
        Some(DispatchView {
            request: &dispatched.request,
            session_id: self
                .session_id
                .as_ref()
                .expect("open connection retains identity"),
            lease: self.lease.as_ref().expect("open connection retains lease"),
        })
    }

    pub(crate) fn cancel(&mut self, id: &JsonRpcRequestId) -> CancelResult {
        if self
            .queued
            .as_ref()
            .is_some_and(|queued| &queued.json_rpc_id == id)
        {
            let request = self.queued.take().expect("queued request was present");
            return CancelResult::QueuedCancelled(CancelledCall::from(request));
        }
        if let Some(dispatched) = self
            .dispatched
            .as_mut()
            .filter(|dispatched| &dispatched.request.json_rpc_id == id)
        {
            let first = !dispatched.cancellation_requested;
            dispatched.cancellation_requested = true;
            return if first {
                CancelResult::DispatchedCancellationRecorded
            } else {
                CancelResult::DispatchedCancellationAlreadyRecorded
            };
        }
        CancelResult::NotActive
    }

    pub(crate) fn complete<R>(
        &mut self,
        id: &JsonRpcRequestId,
        completion: ProviderCompletion<R>,
    ) -> Result<CompletionDisposition<R>, CompletionError> {
        let Some(dispatched) = self.dispatched.as_ref() else {
            return Err(CompletionError::NoDispatchedCall);
        };
        if &dispatched.request.json_rpc_id != id {
            return Err(CompletionError::RequestIdentityMismatch);
        }
        let dispatched = self.dispatched.take().expect("dispatched call was present");
        let completed = CompletedCall {
            json_rpc_id: dispatched.request.json_rpc_id,
            idempotency_key: dispatched.request.idempotency_key,
            cancellation_requested: dispatched.cancellation_requested,
            outcome: completion.into_outcome(),
        };
        let suppress_output = self.phase != ConnectionPhase::Open;
        self.enter_session_ending_if_drained();
        Ok(if suppress_output {
            CompletionDisposition::SuppressAfterEof(completed)
        } else {
            CompletionDisposition::Emit(completed)
        })
    }

    pub(crate) fn receive_eof(&mut self) -> EofDisposition {
        match self.phase {
            ConnectionPhase::Open => self.phase = ConnectionPhase::EofDraining,
            ConnectionPhase::EofDraining => {}
            ConnectionPhase::SessionEnding(_) | ConnectionPhase::Ended => {
                return EofDisposition {
                    cancelled_queued: None,
                    wait_for_dispatched: false,
                };
            }
        }
        let cancelled_queued = self.queued.take().map(CancelledCall::from);
        let wait_for_dispatched = self.dispatched.is_some();
        self.enter_session_ending_if_drained();
        EofDisposition {
            cancelled_queued,
            wait_for_dispatched,
        }
    }

    pub(crate) fn is_ended(&self) -> bool {
        self.phase == ConnectionPhase::Ended
    }

    pub(crate) fn is_session_ending(&self) -> bool {
        matches!(self.phase, ConnectionPhase::SessionEnding(_))
    }

    pub(crate) fn begin_session_end(
        &mut self,
    ) -> Result<SessionEndDispatchView<'_>, SessionEndError> {
        match self.phase {
            ConnectionPhase::SessionEnding(SessionEndPhase::Ready) => {
                self.phase = ConnectionPhase::SessionEnding(SessionEndPhase::Dispatched);
            }
            ConnectionPhase::SessionEnding(SessionEndPhase::Dispatched) => {
                return Err(SessionEndError::AlreadyDispatched);
            }
            _ => return Err(SessionEndError::NotReady),
        }
        Ok(SessionEndDispatchView {
            session_id: self
                .session_id
                .as_ref()
                .expect("session-ending connection retains identity"),
            lease: self
                .lease
                .as_ref()
                .expect("session-ending connection retains lease"),
        })
    }

    /// Completes the internal session-end dispatch. This API never produces an
    /// MCP response disposition, including when cleanup is authoritative.
    pub(crate) fn finish_session_end(
        &mut self,
        completion: SessionEndCompletion,
    ) -> Result<SessionEndRecord, SessionEndError> {
        if self.phase != ConnectionPhase::SessionEnding(SessionEndPhase::Dispatched) {
            return Err(SessionEndError::NotDispatched);
        }
        let outcome = match completion {
            SessionEndCompletion::Authoritative => SessionEndOutcome::Authoritative,
            SessionEndCompletion::LostAfterDispatch => SessionEndOutcome::OutcomeUnknown,
        };
        self.phase = ConnectionPhase::Ended;
        self.session_id.take();
        self.lease.take();
        let record = SessionEndRecord { outcome };
        self.session_end_record = Some(record);
        Ok(record)
    }

    pub(crate) fn session_end_record(&self) -> Option<SessionEndRecord> {
        self.session_end_record
    }

    fn enter_session_ending_if_drained(&mut self) {
        if self.phase == ConnectionPhase::EofDraining && self.dispatched.is_none() {
            self.phase = ConnectionPhase::SessionEnding(SessionEndPhase::Ready);
        }
    }

    fn contains_json_rpc_id(&self, id: &JsonRpcRequestId) -> bool {
        self.dispatched
            .as_ref()
            .is_some_and(|call| &call.request.json_rpc_id == id)
            || self
                .queued
                .as_ref()
                .is_some_and(|call| &call.json_rpc_id == id)
    }

    fn contains_idempotency_key(&self, key: &IdempotencyKey) -> bool {
        self.dispatched
            .as_ref()
            .is_some_and(|call| &call.request.idempotency_key == key)
            || self
                .queued
                .as_ref()
                .is_some_and(|call| &call.idempotency_key == key)
    }
}

pub(crate) struct DispatchView<'a, T> {
    request: &'a MutationRequest<T>,
    session_id: &'a SessionIdentity,
    lease: &'a PrivateSessionLease,
}

impl<T> DispatchView<'_, T> {
    pub(crate) fn json_rpc_id(&self) -> &JsonRpcRequestId {
        &self.request.json_rpc_id
    }

    pub(crate) fn idempotency_key(&self) -> &IdempotencyKey {
        &self.request.idempotency_key
    }

    pub(crate) fn command(&self) -> &T {
        &self.request.command
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session_id.0
    }

    pub(crate) fn with_session_lease<R>(&self, consumer: impl FnOnce(&[u8]) -> R) -> R {
        self.lease.expose_to(consumer)
    }
}

/// Borrowed inputs for the single internal session-end provider dispatch.
/// The bearer material remains behind the private wrapper and is never
/// formatted by this module.
pub(crate) struct SessionEndDispatchView<'a> {
    session_id: &'a SessionIdentity,
    lease: &'a PrivateSessionLease,
}

impl SessionEndDispatchView<'_> {
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id.0
    }

    pub(crate) fn with_session_lease<R>(&self, consumer: impl FnOnce(&[u8]) -> R) -> R {
        self.lease.expose_to(consumer)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionEndCompletion {
    Authoritative,
    LostAfterDispatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionEndOutcome {
    Authoritative,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SessionEndRecord {
    pub(crate) outcome: SessionEndOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionEndError {
    NotReady,
    AlreadyDispatched,
    NotDispatched,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SubmitResult {
    Queued,
    Busy,
    DuplicateJsonRpcId,
    DuplicateIdempotencyKey,
    RejectedAfterEof,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum CancelResult {
    QueuedCancelled(CancelledCall),
    DispatchedCancellationRecorded,
    DispatchedCancellationAlreadyRecorded,
    NotActive,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct CancelledCall {
    pub(crate) json_rpc_id: JsonRpcRequestId,
    pub(crate) idempotency_key: IdempotencyKey,
    pub(crate) counts: EffectCounts,
}

impl<T> From<MutationRequest<T>> for CancelledCall {
    fn from(request: MutationRequest<T>) -> Self {
        Self {
            json_rpc_id: request.json_rpc_id,
            idempotency_key: request.idempotency_key,
            counts: EffectCounts::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EffectCounts {
    pub(crate) provider_calls: CountKnowledge,
    pub(crate) reservations: CountKnowledge,
    pub(crate) effects: CountKnowledge,
}

impl EffectCounts {
    const ZERO: Self = Self {
        provider_calls: CountKnowledge::Known(0),
        reservations: CountKnowledge::Known(0),
        effects: CountKnowledge::Known(0),
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CountKnowledge {
    Known(u8),
    Unknown,
}

pub(crate) enum ProviderCompletion<R> {
    Authoritative {
        reply: R,
        reservation_created: bool,
        effect_attempted: bool,
    },
    LostAfterDispatch,
}

impl<R> ProviderCompletion<R> {
    fn into_outcome(self) -> CompletionOutcome<R> {
        match self {
            Self::Authoritative {
                reply,
                reservation_created,
                effect_attempted,
            } => CompletionOutcome::Authoritative {
                reply,
                counts: EffectCounts {
                    provider_calls: CountKnowledge::Known(1),
                    reservations: CountKnowledge::Known(u8::from(reservation_created)),
                    effects: CountKnowledge::Known(u8::from(effect_attempted)),
                },
            },
            Self::LostAfterDispatch => CompletionOutcome::OutcomeUnknown {
                counts: EffectCounts {
                    provider_calls: CountKnowledge::Known(1),
                    reservations: CountKnowledge::Unknown,
                    effects: CountKnowledge::Unknown,
                },
            },
        }
    }
}

pub(crate) struct CompletedCall<R> {
    pub(crate) json_rpc_id: JsonRpcRequestId,
    pub(crate) idempotency_key: IdempotencyKey,
    pub(crate) cancellation_requested: bool,
    pub(crate) outcome: CompletionOutcome<R>,
}

pub(crate) enum CompletionOutcome<R> {
    Authoritative { reply: R, counts: EffectCounts },
    OutcomeUnknown { counts: EffectCounts },
}

pub(crate) enum CompletionDisposition<R> {
    Emit(CompletedCall<R>),
    SuppressAfterEof(CompletedCall<R>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompletionError {
    NoDispatchedCall,
    RequestIdentityMismatch,
}

pub(crate) struct EofDisposition {
    pub(crate) cancelled_queued: Option<CancelledCall>,
    pub(crate) wait_for_dispatched: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(value: i64) -> JsonRpcRequestId {
        JsonRpcRequestId::Integer(value)
    }

    fn request(id: i64, key: &str, command: &'static str) -> MutationRequest<&'static str> {
        MutationRequest::new(
            rpc(id),
            IdempotencyKey::new(key).expect("valid idempotency key"),
            command,
        )
    }

    fn state() -> ConnectionMutationState<&'static str> {
        ConnectionMutationState::new("session-1", b"private-session-lease".to_vec())
            .expect("valid session")
    }

    #[test]
    fn connection_holds_exactly_one_dispatched_and_one_queued() {
        let mut state = state();
        assert_eq!(
            state.submit(request(1, "effect-a", "first")),
            SubmitResult::Queued
        );
        let dispatch = state.begin_dispatch().expect("first dispatch");
        assert_eq!(dispatch.command(), &"first");
        assert_eq!(
            state.submit(request(2, "effect-b", "second")),
            SubmitResult::Queued
        );
        assert_eq!(
            state.submit(request(3, "effect-c", "third")),
            SubmitResult::Busy
        );
        assert!(state.begin_dispatch().is_none());
    }

    #[test]
    fn transport_request_id_and_public_idempotency_key_are_distinct() {
        let mut state = state();
        assert_eq!(
            state.submit(request(7, "public-effect-key", "command")),
            SubmitResult::Queued
        );
        let dispatch = state.begin_dispatch().expect("dispatch");
        assert_eq!(dispatch.json_rpc_id(), &rpc(7));
        assert_eq!(dispatch.idempotency_key().as_str(), "public-effect-key");
        assert_eq!(dispatch.session_id(), "session-1");
        assert!(dispatch.with_session_lease(|lease| lease == b"private-session-lease"));
    }

    #[test]
    fn unsigned_json_rpc_ids_cover_the_full_u64_range() {
        let mut state = state();
        let id = JsonRpcRequestId::Unsigned(u64::MAX);
        assert_eq!(
            state.submit(MutationRequest::new(
                id.clone(),
                IdempotencyKey::new("effect-u64").expect("valid key"),
                "command",
            )),
            SubmitResult::Queued
        );
        assert_eq!(state.begin_dispatch().expect("dispatch").json_rpc_id(), &id);
    }

    #[test]
    fn duplicate_transport_and_effect_identities_are_rejected_independently() {
        let mut state = state();
        assert_eq!(
            state.submit(request(1, "effect-a", "first")),
            SubmitResult::Queued
        );
        assert_eq!(
            state.submit(request(1, "effect-b", "same transport id")),
            SubmitResult::DuplicateJsonRpcId
        );
        assert_eq!(
            state.submit(request(2, "effect-a", "same effect id")),
            SubmitResult::DuplicateIdempotencyKey
        );
    }

    #[test]
    fn queued_cancellation_wins_with_no_provider_reservation_or_effect() {
        let mut state = state();
        assert_eq!(
            state.submit(request(1, "effect-a", "command")),
            SubmitResult::Queued
        );
        let CancelResult::QueuedCancelled(cancelled) = state.cancel(&rpc(1)) else {
            panic!("queued cancellation must win");
        };
        assert_eq!(cancelled.counts, EffectCounts::ZERO);
        assert!(state.begin_dispatch().is_none());
    }

    #[test]
    fn dispatched_cancellation_is_a_note_not_a_completion() {
        let mut state = state();
        state.submit(request(1, "effect-a", "command"));
        state.begin_dispatch().expect("dispatch");
        assert_eq!(
            state.cancel(&rpc(1)),
            CancelResult::DispatchedCancellationRecorded
        );
        let CompletionDisposition::Emit(completed) = state
            .complete(
                &rpc(1),
                ProviderCompletion::Authoritative {
                    reply: "committed",
                    reservation_created: true,
                    effect_attempted: true,
                },
            )
            .expect("completion")
        else {
            panic!("open connection emits completion");
        };
        assert_eq!(completed.json_rpc_id, rpc(1));
        assert_eq!(completed.idempotency_key.as_str(), "effect-a");
        assert!(completed.cancellation_requested);
        let CompletionOutcome::Authoritative { reply, counts } = completed.outcome else {
            panic!("authoritative reply must win");
        };
        assert_eq!(reply, "committed");
        assert_eq!(counts.provider_calls, CountKnowledge::Known(1));
        assert_eq!(counts.reservations, CountKnowledge::Known(1));
        assert_eq!(counts.effects, CountKnowledge::Known(1));
    }

    #[test]
    fn provider_loss_after_dispatch_is_outcome_unknown() {
        let mut state = state();
        state.submit(request(1, "effect-a", "command"));
        state.begin_dispatch().expect("dispatch");
        let CompletionDisposition::Emit(completed) = state
            .complete::<()>(&rpc(1), ProviderCompletion::LostAfterDispatch)
            .expect("completion")
        else {
            panic!("open connection emits completion");
        };
        let CompletionOutcome::OutcomeUnknown { counts } = completed.outcome else {
            panic!("provider loss must be uncertain");
        };
        assert_eq!(counts.provider_calls, CountKnowledge::Known(1));
        assert_eq!(counts.reservations, CountKnowledge::Unknown);
        assert_eq!(counts.effects, CountKnowledge::Unknown);
    }

    #[test]
    fn eof_rejects_new_work_cancels_queue_waits_for_dispatch_and_suppresses_output() {
        let mut state = state();
        state.submit(request(1, "effect-a", "first"));
        state.begin_dispatch().expect("dispatch");
        state.submit(request(2, "effect-b", "second"));

        let eof = state.receive_eof();
        assert!(eof.wait_for_dispatched);
        let cancelled = eof.cancelled_queued.expect("queued call cancelled");
        assert_eq!(cancelled.json_rpc_id, rpc(2));
        assert_eq!(cancelled.counts, EffectCounts::ZERO);
        assert_eq!(
            state.submit(request(3, "effect-c", "third")),
            SubmitResult::RejectedAfterEof
        );
        assert!(!state.is_ended());

        let disposition = state
            .complete(
                &rpc(1),
                ProviderCompletion::Authoritative {
                    reply: "authoritative",
                    reservation_created: true,
                    effect_attempted: true,
                },
            )
            .expect("dispatched work drains");
        assert!(matches!(
            disposition,
            CompletionDisposition::SuppressAfterEof(_)
        ));
        assert!(state.is_session_ending());
        assert!(!state.is_ended());

        let end = state.begin_session_end().expect("session-end dispatch");
        assert_eq!(end.session_id(), "session-1");
        assert!(end.with_session_lease(|lease| lease == b"private-session-lease"));
        assert_eq!(
            state.begin_session_end().err(),
            Some(SessionEndError::AlreadyDispatched)
        );
        let record = state
            .finish_session_end(SessionEndCompletion::Authoritative)
            .expect("authoritative session end");
        assert_eq!(record.outcome, SessionEndOutcome::Authoritative);
        assert!(state.is_ended());
    }

    #[test]
    fn eof_without_calls_requires_exactly_one_session_end() {
        let mut state = state();
        let eof = state.receive_eof();
        assert!(!eof.wait_for_dispatched);
        assert!(eof.cancelled_queued.is_none());
        assert!(state.is_session_ending());
        assert!(!state.is_ended());

        let end = state.begin_session_end().expect("session-end dispatch");
        assert_eq!(end.session_id(), "session-1");
        assert_eq!(
            state.begin_session_end().err(),
            Some(SessionEndError::AlreadyDispatched)
        );
        state
            .finish_session_end(SessionEndCompletion::Authoritative)
            .expect("session-end completion");
        assert!(state.is_ended());
        assert_eq!(
            state.begin_session_end().err(),
            Some(SessionEndError::NotReady)
        );
        assert_eq!(
            state
                .finish_session_end(SessionEndCompletion::Authoritative)
                .err(),
            Some(SessionEndError::NotDispatched)
        );
    }

    #[test]
    fn lost_session_end_is_recorded_as_outcome_unknown_without_an_output_frame() {
        let mut state = state();
        state.receive_eof();
        state.begin_session_end().expect("session-end dispatch");
        let record = state
            .finish_session_end(SessionEndCompletion::LostAfterDispatch)
            .expect("uncertain session-end completion");
        assert_eq!(record.outcome, SessionEndOutcome::OutcomeUnknown);
        assert_eq!(state.session_end_record(), Some(record));
        assert!(state.is_ended());
    }

    #[test]
    fn completion_must_match_the_active_json_rpc_request() {
        let mut state = state();
        state.submit(request(1, "effect-a", "command"));
        state.begin_dispatch().expect("dispatch");
        assert!(matches!(
            state.complete::<()>(&rpc(2), ProviderCompletion::LostAfterDispatch),
            Err(CompletionError::RequestIdentityMismatch)
        ));
    }

    #[test]
    fn identity_and_lease_bounds_fail_closed() {
        assert_eq!(
            JsonRpcRequestId::text("x".repeat(MAX_JSON_RPC_ID_BYTES + 1)),
            Err(IdentityError::InvalidJsonRpcRequestId)
        );
        assert_eq!(
            JsonRpcRequestId::text(""),
            Ok(JsonRpcRequestId::Text(String::new()))
        );
        assert!(IdempotencyKey::new("AZaz09_.:-").is_ok());
        for invalid in ["", "has space", "slash/not-allowed", "非ascii"] {
            assert_eq!(
                IdempotencyKey::new(invalid),
                Err(IdentityError::InvalidIdempotencyKey)
            );
        }
        assert!(IdempotencyKey::new("x".repeat(MAX_IDEMPOTENCY_KEY_BYTES)).is_ok());
        assert_eq!(
            IdempotencyKey::new("x".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1)),
            Err(IdentityError::InvalidIdempotencyKey)
        );
        assert!(matches!(
            ConnectionMutationState::<()>::new("session-1", Vec::new()),
            Err(IdentityError::InvalidSessionLease)
        ));
        assert!(matches!(
            ConnectionMutationState::<()>::new("", b"lease".to_vec()),
            Err(IdentityError::InvalidSessionId)
        ));
        assert!(matches!(
            ConnectionMutationState::<()>::new(
                "x".repeat(MAX_SESSION_ID_BYTES + 1),
                b"lease".to_vec()
            ),
            Err(IdentityError::InvalidSessionId)
        ));
    }
}
