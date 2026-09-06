//! Exact console-session observation, short-lived lock plans and at-most-once apply.
//!
//! The platform crate owns native inventory and lock-chord delivery. This
//! module owns product policy: current-user scope, canonical approval, exact
//! session revalidation, durable reservation and postcondition read-back.

use std::{
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agenterm_platform::login_session::{
    LoginSessionError, LoginSessionErrorKind, LoginSessionInventory, LoginSessionProvider,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    CuError,
    idempotency_store::{
        FinalOutcome, FinalOutcomeKind, FreshReservation, IdempotencyStore, RequestState,
        RequestStatus, ReserveDecision, fingerprint_canonical_request,
    },
};

pub const DEFAULT_SESSION_LOCK_TTL_SECONDS: u64 = 120;
pub const MIN_SESSION_LOCK_TTL_SECONDS: u64 = 1;
pub const MAX_SESSION_LOCK_TTL_SECONDS: u64 = 600;
const SESSION_LOCK_SCHEMA_VERSION: u32 = 1;
const REPLAY_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const CONTRACT_DIGEST_DOMAIN: &[u8] = b"agenterm-cu/session-lock-contract/v1\0";
const APPROVAL_DIGEST_DOMAIN: &[u8] = b"agenterm-cu/session-lock-approval/v1\0";
const OUTCOME_CHANGED: &str = "session_lock_completed_changed";
const OUTCOME_PRELOCKED: &str = "session_lock_completed_prelocked";
const OUTCOME_DELIVERY_FAILED: &str = "session_lock_delivery_not_performed";
const REQUEST_BYTES_MAX: usize = 16 * 1024;
const READBACK_ATTEMPTS: usize = 80;
const READBACK_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LoginSessionRecord {
    pub identity: String,
    pub uuid: String,
    pub session_id: u64,
    pub security_session_id: u64,
    pub audit_id: u64,
    pub uid: u32,
    pub gid: u32,
    pub username: String,
    pub display_name: String,
    pub on_console: bool,
    pub login_complete: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LoginSessionStatus {
    pub provider: String,
    pub locked: bool,
    pub sessions: Vec<LoginSessionRecord>,
    pub console_session: Option<LoginSessionRecord>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SessionLockOperation {
    #[serde(rename = "session.lock")]
    Lock,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLockTarget {
    pub identity: String,
    pub uuid: String,
    pub session_id: u64,
    pub security_session_id: u64,
    pub audit_id: u64,
    pub uid: u32,
    pub username: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLockState {
    pub locked: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLockPlan {
    pub schema_version: u32,
    pub operation: SessionLockOperation,
    pub target: SessionLockTarget,
    pub before: SessionLockState,
    pub after: SessionLockState,
    pub issued_at_utc_ms: u64,
    pub expires_at_utc_ms: u64,
    pub contract_digest: String,
    pub approval_digest: String,
    pub mutation_performed: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLockApplyState {
    Completed,
    Replayed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLockApplyReply {
    pub state: SessionLockApplyState,
    pub idempotent: bool,
    pub changed: bool,
    pub preexisting: bool,
    pub locked: bool,
    pub verified: bool,
    pub contract_digest: String,
    pub approval_digest: String,
}

#[derive(Debug)]
pub enum SessionLockDeliveryError {
    /// The provider proved that it posted no input event.
    NotPerformed(CuError),
    /// Delivery may have begun; callers must never retry automatically.
    OutcomeUnknown(CuError),
}

pub trait SessionLockProvider {
    fn inventory(&mut self) -> Result<LoginSessionStatus, CuError>;
    fn current_user_id(&mut self) -> Result<u32, CuError>;
    fn deliver_lock(&mut self) -> Result<(), SessionLockDeliveryError>;
    fn now_utc_ms(&mut self) -> Result<u64, CuError>;
    fn wait(&mut self, duration: Duration);
}

#[derive(Default)]
pub struct NativeSessionLockProvider;

impl SessionLockProvider for NativeSessionLockProvider {
    fn inventory(&mut self) -> Result<LoginSessionStatus, CuError> {
        agenterm_platform::login_session::inventory()
            .map(platform_inventory)
            .map_err(platform_error)
    }

    fn current_user_id(&mut self) -> Result<u32, CuError> {
        let identity = agenterm_platform::user_identity::current_user_identity().map_err(|_| {
            CuError::new(
                "login_session_current_user_unavailable",
                "the current operating-system user identity is unavailable",
            )
        })?;
        identity
            .posix_credentials()
            .map(|credentials| credentials.effective_user_id)
            .ok_or_else(|| {
                CuError::new(
                    "login_session_unsupported",
                    "login-session locking currently requires a POSIX current user",
                )
            })
    }

    fn deliver_lock(&mut self) -> Result<(), SessionLockDeliveryError> {
        agenterm_platform::login_session::lock_console().map_err(|error| {
            let typed = platform_error(error);
            if matches!(
                typed.code.as_str(),
                "login_session_input_permission_denied" | "login_session_unsupported"
            ) {
                SessionLockDeliveryError::NotPerformed(typed)
            } else {
                SessionLockDeliveryError::OutcomeUnknown(typed)
            }
        })
    }

    fn now_utc_ms(&mut self) -> Result<u64, CuError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| {
                CuError::new(
                    "login_session_clock_invalid",
                    "host clock is before the Unix epoch",
                )
            })?
            .as_millis()
            .try_into()
            .map_err(|_| {
                CuError::new(
                    "login_session_clock_invalid",
                    "host clock does not fit the session-lock timestamp contract",
                )
            })
    }

    fn wait(&mut self, duration: Duration) {
        thread::sleep(duration);
    }
}

pub fn status() -> Result<LoginSessionStatus, CuError> {
    status_with_provider(&mut NativeSessionLockProvider)
}

pub fn status_with_provider(
    provider: &mut impl SessionLockProvider,
) -> Result<LoginSessionStatus, CuError> {
    let status = provider.inventory()?;
    validate_status(&status)?;
    Ok(status)
}

pub fn plan_lock() -> Result<SessionLockPlan, CuError> {
    plan_lock_with_ttl(DEFAULT_SESSION_LOCK_TTL_SECONDS)
}

pub fn plan_lock_with_ttl(ttl_seconds: u64) -> Result<SessionLockPlan, CuError> {
    plan_lock_with_provider(&mut NativeSessionLockProvider, ttl_seconds)
}

pub fn plan_lock_with_provider(
    provider: &mut impl SessionLockProvider,
    ttl_seconds: u64,
) -> Result<SessionLockPlan, CuError> {
    if !(MIN_SESSION_LOCK_TTL_SECONDS..=MAX_SESSION_LOCK_TTL_SECONDS).contains(&ttl_seconds) {
        return Err(CuError::new(
            "session_lock_ttl_invalid",
            format!(
                "session lock TTL must be in {MIN_SESSION_LOCK_TTL_SECONDS}..={MAX_SESSION_LOCK_TTL_SECONDS} seconds"
            ),
        ));
    }
    let now = provider.now_utc_ms()?;
    let expires = now
        .checked_add(ttl_seconds.checked_mul(1_000).ok_or_else(|| {
            CuError::new("session_lock_ttl_invalid", "session lock TTL overflows")
        })?)
        .ok_or_else(|| {
            CuError::new(
                "login_session_clock_invalid",
                "session lock expiry overflows the host clock",
            )
        })?;
    let status = status_with_provider(provider)?;
    let current_uid = provider.current_user_id()?;
    let console = eligible_console(&status, current_uid)?;
    let target = target(console);
    let before = SessionLockState {
        locked: status.locked,
    };
    let after = SessionLockState { locked: true };
    let contract = ContractProjection {
        schema_version: SESSION_LOCK_SCHEMA_VERSION,
        operation: SessionLockOperation::Lock,
        target: &target,
        before,
        after,
    };
    let contract_digest = digest(CONTRACT_DIGEST_DOMAIN, &contract)?;
    let approval_digest = digest(
        APPROVAL_DIGEST_DOMAIN,
        &ApprovalProjection {
            contract_digest: &contract_digest,
            issued_at_utc_ms: now,
            expires_at_utc_ms: expires,
        },
    )?;
    Ok(SessionLockPlan {
        schema_version: SESSION_LOCK_SCHEMA_VERSION,
        operation: SessionLockOperation::Lock,
        target,
        before,
        after,
        issued_at_utc_ms: now,
        expires_at_utc_ms: expires,
        contract_digest,
        approval_digest,
        mutation_performed: false,
    })
}

pub fn apply_lock(
    plan: &SessionLockPlan,
    approval_digest: &str,
) -> Result<SessionLockApplyReply, CuError> {
    let store = IdempotencyStore::open()?;
    apply_lock_with_provider(
        &mut NativeSessionLockProvider,
        &store,
        plan,
        approval_digest,
    )
}

/// Encode one closed plan in the MCU-compatible unpadded base64url request
/// envelope. The approval digest remains a separate explicit argument.
pub fn encode_lock_request(plan: &SessionLockPlan) -> Result<String, CuError> {
    let bytes = serde_json::to_vec(plan).map_err(|_| {
        CuError::new(
            "session_lock_request_invalid",
            "session lock request could not be serialized",
        )
    })?;
    if bytes.len() > REQUEST_BYTES_MAX {
        return Err(CuError::new(
            "session_lock_request_limit",
            "session lock request exceeds its byte ceiling",
        ));
    }
    Ok(crate::managed_job_ipc::base64_encode(&bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_owned())
}

/// Decode only the closed serialized shape. [`apply_lock`] recomputes both
/// digests, enforces expiry and revalidates the live console identity.
pub fn decode_lock_request(encoded: &str) -> Result<SessionLockPlan, CuError> {
    if encoded.is_empty()
        || encoded.len() > REQUEST_BYTES_MAX * 2
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CuError::new(
            "session_lock_request_invalid",
            "session lock request is not bounded unpadded base64url",
        ));
    }
    let mut standard = encoded.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let bytes = crate::managed_job_ipc::base64_decode(&standard).map_err(|_| {
        CuError::new(
            "session_lock_request_invalid",
            "session lock request base64url is malformed",
        )
    })?;
    if bytes.is_empty() || bytes.len() > REQUEST_BYTES_MAX {
        return Err(CuError::new(
            "session_lock_request_limit",
            "decoded session lock request exceeds its byte ceiling",
        ));
    }
    let canonical = crate::managed_job_ipc::base64_encode(&bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_owned();
    if canonical != encoded {
        return Err(CuError::new(
            "session_lock_request_invalid",
            "session lock request is not canonical unpadded base64url",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        CuError::new(
            "session_lock_request_invalid",
            "session lock request is not the closed plan shape",
        )
    })
}

pub fn apply_lock_with_provider(
    provider: &mut impl SessionLockProvider,
    store: &IdempotencyStore,
    plan: &SessionLockPlan,
    approval_digest: &str,
) -> Result<SessionLockApplyReply, CuError> {
    apply_lock_with_ledger(provider, store, plan, approval_digest)
}

fn apply_lock_with_ledger(
    provider: &mut impl SessionLockProvider,
    ledger: &impl SessionLockLedger,
    plan: &SessionLockPlan,
    approval_digest: &str,
) -> Result<SessionLockApplyReply, CuError> {
    let now = provider.now_utc_ms()?;
    validate_plan_integrity(plan, approval_digest)?;
    let canonical = serde_json::to_vec(plan).map_err(|_| {
        CuError::new(
            "session_lock_plan_serialization_failed",
            "session lock plan could not be serialized canonically",
        )
    })?;
    let fingerprint = fingerprint_canonical_request(&canonical)?;
    if let Some(status) = ledger.lookup(&plan.approval_digest, &fingerprint, to_i64(now)?)? {
        return match status.state {
            RequestState::Finalized => replay(plan, status),
            RequestState::Reserved | RequestState::OutcomeUnknown => Err(outcome_unknown()),
        };
    }
    validate_plan_expiry(plan, now)?;
    let status = status_with_provider(provider)?;
    let current_uid = provider.current_user_id()?;
    let console = eligible_console(&status, current_uid)?;
    if target(console) != plan.target {
        return Err(CuError::new(
            "session_lock_target_changed",
            "the exact console-session identity changed after planning",
        ));
    }
    if plan.before.locked && !status.locked {
        return Err(CuError::new(
            "session_lock_precondition_changed",
            "the console session became unlocked after a locked-state plan",
        ));
    }

    let reservation = match ledger.reserve(
        &plan.approval_digest,
        &fingerprint,
        REPLAY_RETENTION_MS,
        to_i64(now)?,
    )? {
        ReserveDecision::Fresh(fresh) => fresh,
        ReserveDecision::ReplayFinalized(status) => return replay(plan, status),
        ReserveDecision::Uncertain(_) => return Err(outcome_unknown()),
    };

    if status.locked {
        finalize(
            ledger,
            plan,
            &fingerprint,
            &reservation,
            FinalOutcomeKind::Succeeded,
            OUTCOME_PRELOCKED,
            now,
        )?;
        return Ok(completed(plan, false, true));
    }

    match provider.deliver_lock() {
        Ok(()) => {}
        Err(SessionLockDeliveryError::NotPerformed(error)) => {
            if let Err(persist) = finalize(
                ledger,
                plan,
                &fingerprint,
                &reservation,
                FinalOutcomeKind::Failed,
                OUTCOME_DELIVERY_FAILED,
                now,
            ) {
                return Err(mark_unknown_then(
                    ledger,
                    plan,
                    &fingerprint,
                    &reservation,
                    now,
                    persist,
                ));
            }
            return Err(error.with_detail(serde_json::json!({ "effect": "not_performed" })));
        }
        Err(SessionLockDeliveryError::OutcomeUnknown(error)) => {
            return Err(mark_unknown_then(
                ledger,
                plan,
                &fingerprint,
                &reservation,
                now,
                error,
            ));
        }
    }

    let mut verified = false;
    for attempt in 0..=READBACK_ATTEMPTS {
        let after = match status_with_provider(provider) {
            Ok(status) => status,
            Err(error) => {
                return Err(mark_unknown_then(
                    ledger,
                    plan,
                    &fingerprint,
                    &reservation,
                    now,
                    error,
                ));
            }
        };
        let same_session = after
            .console_session
            .as_ref()
            .is_some_and(|session| target(session) == plan.target);
        if !same_session {
            return Err(mark_unknown_then(
                ledger,
                plan,
                &fingerprint,
                &reservation,
                now,
                CuError::new(
                    "session_lock_target_changed",
                    "the exact console session changed after lock delivery",
                ),
            ));
        }
        if after.locked {
            verified = true;
            break;
        }
        if attempt < READBACK_ATTEMPTS {
            provider.wait(READBACK_INTERVAL);
        }
    }
    if !verified {
        return Err(mark_unknown_then(
            ledger,
            plan,
            &fingerprint,
            &reservation,
            now,
            CuError::new(
                "session_lock_readback_failed",
                "lock delivery did not produce the exact locked console-session postcondition",
            ),
        ));
    }
    if let Err(error) = finalize(
        ledger,
        plan,
        &fingerprint,
        &reservation,
        FinalOutcomeKind::Succeeded,
        OUTCOME_CHANGED,
        now,
    ) {
        return Err(mark_unknown_then(
            ledger,
            plan,
            &fingerprint,
            &reservation,
            now,
            error,
        ));
    }
    Ok(completed(plan, true, false))
}

#[derive(Serialize)]
struct ContractProjection<'a> {
    schema_version: u32,
    operation: SessionLockOperation,
    target: &'a SessionLockTarget,
    before: SessionLockState,
    after: SessionLockState,
}

#[derive(Serialize)]
struct ApprovalProjection<'a> {
    contract_digest: &'a str,
    issued_at_utc_ms: u64,
    expires_at_utc_ms: u64,
}

fn validate_plan_integrity(plan: &SessionLockPlan, approval: &str) -> Result<(), CuError> {
    let valid_uuid = plan.target.uuid.len() == 36
        && plan
            .target
            .uuid
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            });
    if plan.schema_version != SESSION_LOCK_SCHEMA_VERSION
        || plan.operation != SessionLockOperation::Lock
        || plan.after != (SessionLockState { locked: true })
        || plan.mutation_performed
        || plan.target.identity.len() != 64
        || !plan
            .target
            .identity
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !valid_uuid
        || plan.target.username.is_empty()
        || plan.target.username.len() > 256
        || plan
            .target
            .username
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    {
        return Err(CuError::new(
            "session_lock_plan_invalid",
            "session lock plan has an invalid closed shape",
        ));
    }
    let lifetime = plan
        .expires_at_utc_ms
        .checked_sub(plan.issued_at_utc_ms)
        .ok_or_else(|| {
            CuError::new(
                "session_lock_plan_invalid",
                "session lock expiry precedes its issue time",
            )
        })?;
    if !(MIN_SESSION_LOCK_TTL_SECONDS * 1_000..=MAX_SESSION_LOCK_TTL_SECONDS * 1_000)
        .contains(&lifetime)
    {
        return Err(CuError::new(
            "session_lock_plan_invalid",
            "session lock lifetime is outside the bounded contract",
        ));
    }
    let contract = ContractProjection {
        schema_version: plan.schema_version,
        operation: plan.operation,
        target: &plan.target,
        before: plan.before,
        after: plan.after,
    };
    let expected_contract = digest(CONTRACT_DIGEST_DOMAIN, &contract)?;
    let expected_approval = digest(
        APPROVAL_DIGEST_DOMAIN,
        &ApprovalProjection {
            contract_digest: &expected_contract,
            issued_at_utc_ms: plan.issued_at_utc_ms,
            expires_at_utc_ms: plan.expires_at_utc_ms,
        },
    )?;
    if plan.contract_digest != expected_contract
        || plan.approval_digest != expected_approval
        || approval != expected_approval
    {
        return Err(CuError::new(
            "session_lock_approval_mismatch",
            "session lock plan content does not match its approval digest",
        ));
    }
    Ok(())
}

fn validate_plan_expiry(plan: &SessionLockPlan, now: u64) -> Result<(), CuError> {
    if now > plan.expires_at_utc_ms {
        return Err(CuError::new(
            "session_lock_plan_expired",
            "session lock approval expired before reservation",
        ));
    }
    Ok(())
}

fn validate_status(status: &LoginSessionStatus) -> Result<(), CuError> {
    if status.provider.is_empty() || status.provider.len() > 128 || status.sessions.len() > 64 {
        return Err(CuError::new(
            "login_session_provider_shape",
            "login-session provider returned an invalid bounded status",
        ));
    }
    for session in &status.sessions {
        validate_record(session)?;
    }
    for (index, session) in status.sessions.iter().enumerate() {
        for other in &status.sessions[index + 1..] {
            if session.session_id == other.session_id || session.identity == other.identity {
                return Err(CuError::new(
                    "login_session_provider_shape",
                    "login-session provider returned duplicate session identity",
                ));
            }
        }
    }
    let consoles: Vec<&LoginSessionRecord> = status
        .sessions
        .iter()
        .filter(|session| session.on_console)
        .collect();
    if consoles.len() > 1 {
        return Err(CuError::new(
            "login_session_console_ambiguous",
            "login-session provider returned more than one console session",
        ));
    }
    if status.console_session.as_ref() != consoles.first().copied() {
        return Err(CuError::new(
            "login_session_provider_shape",
            "login-session provider console projection is inconsistent",
        ));
    }
    Ok(())
}

fn validate_record(session: &LoginSessionRecord) -> Result<(), CuError> {
    let valid_uuid = session.uuid.len() == 36
        && session
            .uuid
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            });
    let valid_identity = session.identity.len() == 64
        && session
            .identity
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit());
    let valid_text = |value: &str, minimum: usize, maximum: usize| {
        (minimum..=maximum).contains(&value.len())
            && !value
                .as_bytes()
                .iter()
                .any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    };
    if !valid_uuid
        || !valid_identity
        || !valid_text(&session.username, 1, 256)
        || !valid_text(&session.display_name, 0, 512)
    {
        return Err(CuError::new(
            "login_session_provider_shape",
            "login-session provider returned a malformed session row",
        ));
    }
    Ok(())
}

fn eligible_console(
    status: &LoginSessionStatus,
    current_uid: u32,
) -> Result<&LoginSessionRecord, CuError> {
    let console = status.console_session.as_ref().ok_or_else(|| {
        CuError::new(
            "login_session_console_missing",
            "there is no current console session",
        )
    })?;
    if !console.login_complete {
        return Err(CuError::new(
            "login_session_login_incomplete",
            "the current console session has not completed login",
        ));
    }
    if console.uid != current_uid {
        return Err(CuError::new(
            "session_lock_wrong_user",
            "the console session does not belong to the current effective user",
        ));
    }
    Ok(console)
}

fn target(session: &LoginSessionRecord) -> SessionLockTarget {
    SessionLockTarget {
        identity: session.identity.clone(),
        uuid: session.uuid.clone(),
        session_id: session.session_id,
        security_session_id: session.security_session_id,
        audit_id: session.audit_id,
        uid: session.uid,
        username: session.username.clone(),
    }
}

fn platform_inventory(inventory: LoginSessionInventory) -> LoginSessionStatus {
    let sessions: Vec<LoginSessionRecord> = inventory
        .sessions
        .iter()
        .map(|session| LoginSessionRecord {
            identity: hex(session.identity.as_bytes()),
            uuid: session.native_uuid.clone(),
            session_id: session.native_session_id,
            security_session_id: session.native_security_session_id,
            audit_id: session.native_audit_id,
            uid: session.user_id,
            gid: session.group_id,
            username: session.username.clone(),
            display_name: session.display_name.clone(),
            on_console: session.on_console,
            login_complete: session.login_complete,
        })
        .collect();
    let console_session = inventory
        .console_session_index
        .and_then(|index| sessions.get(index).cloned());
    LoginSessionStatus {
        provider: provider_name(inventory.provider).into(),
        locked: inventory.locked,
        sessions,
        console_session,
    }
}

fn provider_name(provider: LoginSessionProvider) -> &'static str {
    match provider {
        LoginSessionProvider::MacosIoRegistry => "macos-io-registry",
        _ => "unknown",
    }
}

fn platform_error(error: LoginSessionError) -> CuError {
    let code = match error.kind() {
        LoginSessionErrorKind::Unsupported => "login_session_unsupported",
        LoginSessionErrorKind::ProviderUnavailable => "login_session_provider_unavailable",
        LoginSessionErrorKind::ProviderShape => "login_session_provider_shape",
        LoginSessionErrorKind::AmbiguousConsole => "login_session_console_ambiguous",
        LoginSessionErrorKind::InputPermissionDenied => "login_session_input_permission_denied",
        LoginSessionErrorKind::DeliveryFailed => "session_lock_delivery_unknown",
        _ => "login_session_provider_failed",
    };
    CuError::new(code, error.to_string())
}

fn digest(domain: &[u8], value: &impl Serialize) -> Result<String, CuError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        CuError::new(
            "session_lock_plan_serialization_failed",
            "session lock projection could not be serialized canonically",
        )
    })?;
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    Ok(hex(&digest.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn to_i64(value: u64) -> Result<i64, CuError> {
    value.try_into().map_err(|_| {
        CuError::new(
            "login_session_clock_invalid",
            "session lock timestamp exceeds the receipt clock range",
        )
    })
}

trait SessionLockLedger {
    fn lookup(
        &self,
        request_id: &str,
        fingerprint: &str,
        now: i64,
    ) -> Result<Option<RequestStatus>, CuError>;
    fn reserve(
        &self,
        request_id: &str,
        fingerprint: &str,
        retention_ms: i64,
        now: i64,
    ) -> Result<ReserveDecision, CuError>;
    fn finalize(
        &self,
        request_id: &str,
        fingerprint: &str,
        token: &str,
        outcome: FinalOutcome,
        now: i64,
    ) -> Result<RequestStatus, CuError>;
    fn mark_unknown(
        &self,
        request_id: &str,
        fingerprint: &str,
        token: &str,
        now: i64,
    ) -> Result<RequestStatus, CuError>;
}

impl SessionLockLedger for IdempotencyStore {
    fn lookup(
        &self,
        request_id: &str,
        fingerprint: &str,
        now: i64,
    ) -> Result<Option<RequestStatus>, CuError> {
        self.lookup(request_id, fingerprint, now)
    }

    fn reserve(
        &self,
        request_id: &str,
        fingerprint: &str,
        retention_ms: i64,
        now: i64,
    ) -> Result<ReserveDecision, CuError> {
        self.reserve(request_id, fingerprint, retention_ms, now)
    }

    fn finalize(
        &self,
        request_id: &str,
        fingerprint: &str,
        token: &str,
        outcome: FinalOutcome,
        now: i64,
    ) -> Result<RequestStatus, CuError> {
        self.finalize(request_id, fingerprint, token, outcome, now)
    }

    fn mark_unknown(
        &self,
        request_id: &str,
        fingerprint: &str,
        token: &str,
        now: i64,
    ) -> Result<RequestStatus, CuError> {
        self.mark_outcome_unknown(request_id, fingerprint, token, now)
    }
}

fn finalize(
    ledger: &impl SessionLockLedger,
    plan: &SessionLockPlan,
    fingerprint: &str,
    reservation: &FreshReservation,
    kind: FinalOutcomeKind,
    code: &str,
    now: u64,
) -> Result<(), CuError> {
    ledger.finalize(
        &plan.approval_digest,
        fingerprint,
        &reservation.completion_token,
        FinalOutcome::new(kind, code, None)?,
        to_i64(now)?,
    )?;
    Ok(())
}

fn mark_unknown_then(
    ledger: &impl SessionLockLedger,
    plan: &SessionLockPlan,
    fingerprint: &str,
    reservation: &FreshReservation,
    now: u64,
    cause: CuError,
) -> CuError {
    let persisted = ledger.mark_unknown(
        &plan.approval_digest,
        fingerprint,
        &reservation.completion_token,
        i64::try_from(now).unwrap_or(i64::MAX),
    );
    CuError::new(
        "session_lock_outcome_unknown",
        "session lock may have been delivered; automatic replay is refused",
    )
    .with_detail(serde_json::json!({
        "effect": "unknown",
        "cause": cause.code,
        "uncertain_persisted": persisted.is_ok(),
    }))
}

fn replay(plan: &SessionLockPlan, status: RequestStatus) -> Result<SessionLockApplyReply, CuError> {
    let outcome = status.outcome.ok_or_else(|| {
        CuError::new(
            "session_lock_receipt_invalid",
            "finalized session lock receipt has no outcome",
        )
    })?;
    if outcome.kind == FinalOutcomeKind::Failed {
        return Err(CuError::new(
            "session_lock_previously_failed",
            "this session lock approval already has a terminal failed receipt",
        )
        .with_detail(serde_json::json!({ "effect": "not_repeated" })));
    }
    match outcome.code.as_str() {
        OUTCOME_CHANGED => Ok(replayed(plan, true, false)),
        OUTCOME_PRELOCKED => Ok(replayed(plan, false, true)),
        _ => Err(CuError::new(
            "session_lock_receipt_invalid",
            "session lock receipt has an unknown terminal outcome",
        )),
    }
}

fn completed(plan: &SessionLockPlan, changed: bool, preexisting: bool) -> SessionLockApplyReply {
    reply(
        plan,
        SessionLockApplyState::Completed,
        false,
        changed,
        preexisting,
    )
}

fn replayed(plan: &SessionLockPlan, changed: bool, preexisting: bool) -> SessionLockApplyReply {
    reply(
        plan,
        SessionLockApplyState::Replayed,
        true,
        changed,
        preexisting,
    )
}

fn reply(
    plan: &SessionLockPlan,
    state: SessionLockApplyState,
    idempotent: bool,
    changed: bool,
    preexisting: bool,
) -> SessionLockApplyReply {
    SessionLockApplyReply {
        state,
        idempotent,
        changed,
        preexisting,
        locked: true,
        verified: true,
        contract_digest: plan.contract_digest.clone(),
        approval_digest: plan.approval_digest.clone(),
    }
}

fn outcome_unknown() -> CuError {
    CuError::new(
        "session_lock_outcome_unknown",
        "this session lock approval may already have been delivered; automatic replay is refused",
    )
    .with_detail(serde_json::json!({ "effect": "unknown" }))
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, path::PathBuf};

    use super::*;

    struct TestPath(PathBuf);

    impl TestPath {
        fn new(name: &str) -> Self {
            let path = std::env::current_dir()
                .expect("resolve repository test root")
                .join("target/login-session-test-state")
                .join(format!(
                    "agenterm-cu-login-session-{name}-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            Self(path)
        }
    }

    impl Drop for TestPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct FixtureProvider {
        inventories: VecDeque<Result<LoginSessionStatus, CuError>>,
        fallback: LoginSessionStatus,
        uid: u32,
        now: u64,
        deliveries: usize,
        delivery: Option<Result<(), SessionLockDeliveryError>>,
    }

    impl SessionLockProvider for FixtureProvider {
        fn inventory(&mut self) -> Result<LoginSessionStatus, CuError> {
            self.inventories
                .pop_front()
                .unwrap_or_else(|| Ok(self.fallback.clone()))
        }

        fn current_user_id(&mut self) -> Result<u32, CuError> {
            Ok(self.uid)
        }

        fn deliver_lock(&mut self) -> Result<(), SessionLockDeliveryError> {
            self.deliveries += 1;
            self.delivery.take().unwrap_or(Ok(()))
        }

        fn now_utc_ms(&mut self) -> Result<u64, CuError> {
            Ok(self.now)
        }

        fn wait(&mut self, _duration: Duration) {}
    }

    fn record(id: u64, console: bool) -> LoginSessionRecord {
        LoginSessionRecord {
            identity: format!("{id:064x}"),
            uuid: format!("00000000-0000-4000-8000-{id:012X}"),
            session_id: id,
            security_session_id: 100_000 + id,
            audit_id: 100_000 + id,
            uid: 501,
            gid: 20,
            username: "fixture".into(),
            display_name: "Fixture User".into(),
            on_console: console,
            login_complete: true,
        }
    }

    fn status(locked: bool) -> LoginSessionStatus {
        let session = record(257, true);
        LoginSessionStatus {
            provider: "fixture".into(),
            locked,
            sessions: vec![session.clone()],
            console_session: Some(session),
        }
    }

    fn provider(locked: bool) -> FixtureProvider {
        FixtureProvider {
            inventories: VecDeque::new(),
            fallback: status(locked),
            uid: 501,
            now: 1_000,
            deliveries: 0,
            delivery: None,
        }
    }

    fn plan(provider: &mut FixtureProvider) -> SessionLockPlan {
        plan_lock_with_provider(provider, DEFAULT_SESSION_LOCK_TTL_SECONDS).unwrap()
    }

    fn store(name: &str) -> (TestPath, IdempotencyStore) {
        let root = TestPath::new(name);
        let store = IdempotencyStore::open_at(root.0.join("requests.json")).unwrap();
        (root, store)
    }

    #[test]
    fn plan_is_bounded_read_only_and_uses_separate_digest_domains() {
        let mut fixture = provider(false);
        let plan = plan(&mut fixture);
        assert_eq!(plan.expires_at_utc_ms - plan.issued_at_utc_ms, 120_000);
        assert_ne!(plan.contract_digest, plan.approval_digest);
        assert!(!plan.mutation_performed);
        assert_eq!(fixture.deliveries, 0);
        let encoded = encode_lock_request(&plan).unwrap();
        assert!(
            !encoded
                .bytes()
                .any(|byte| matches!(byte, b'+' | b'/' | b'='))
        );
        assert_eq!(decode_lock_request(&encoded).unwrap(), plan);
        assert_eq!(
            plan_lock_with_provider(&mut fixture, 0).unwrap_err().code,
            "session_lock_ttl_invalid"
        );
        assert_eq!(
            plan_lock_with_provider(&mut fixture, 601).unwrap_err().code,
            "session_lock_ttl_invalid"
        );
    }

    #[test]
    fn tamper_expiry_drift_and_wrong_uid_refuse_before_reservation() {
        let (_root, store) = store("refusals");
        let mut fixture = provider(false);
        let plan = plan(&mut fixture);

        let mut tampered = plan.clone();
        tampered.target.session_id += 1;
        assert_eq!(
            apply_lock_with_provider(&mut fixture, &store, &tampered, &plan.approval_digest)
                .unwrap_err()
                .code,
            "session_lock_approval_mismatch"
        );

        fixture.now = plan.expires_at_utc_ms + 1;
        assert_eq!(
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "session_lock_plan_expired"
        );

        fixture.now = 2_000;
        fixture
            .fallback
            .console_session
            .as_mut()
            .unwrap()
            .session_id += 1;
        fixture.fallback.sessions[0] = fixture.fallback.console_session.clone().unwrap();
        assert_eq!(
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "session_lock_target_changed"
        );

        fixture.fallback = status(false);
        fixture.uid = 502;
        assert_eq!(
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "session_lock_wrong_user"
        );
        assert_eq!(fixture.deliveries, 0);
    }

    #[test]
    fn missing_ambiguous_and_incomplete_console_refuse() {
        let mut missing = provider(false);
        missing.fallback.sessions.clear();
        missing.fallback.console_session = None;
        assert_eq!(
            plan_lock_with_provider(&mut missing, 120).unwrap_err().code,
            "login_session_console_missing"
        );

        let mut ambiguous = provider(false);
        let second = record(258, true);
        ambiguous.fallback.sessions.push(second);
        assert_eq!(
            plan_lock_with_provider(&mut ambiguous, 120)
                .unwrap_err()
                .code,
            "login_session_console_ambiguous"
        );

        let mut incomplete = provider(false);
        incomplete.fallback.sessions[0].login_complete = false;
        incomplete.fallback.console_session = Some(incomplete.fallback.sessions[0].clone());
        assert_eq!(
            plan_lock_with_provider(&mut incomplete, 120)
                .unwrap_err()
                .code,
            "login_session_login_incomplete"
        );
    }

    #[test]
    fn prelocked_consumes_approval_without_delivery_and_replays() {
        let (_root, store) = store("prelocked");
        let mut fixture = provider(true);
        let plan = plan(&mut fixture);
        let first =
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest).unwrap();
        assert!(first.preexisting);
        assert_eq!(fixture.deliveries, 0);
        let replay =
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest).unwrap();
        assert!(replay.idempotent);
        assert_eq!(fixture.deliveries, 0);
    }

    #[test]
    fn completed_lock_replays_without_a_second_effect() {
        let (_root, store) = store("completed");
        let mut fixture = provider(false);
        let plan = plan(&mut fixture);
        fixture.inventories.push_back(Ok(status(false)));
        fixture.inventories.push_back(Ok(status(true)));
        let first =
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest).unwrap();
        assert!(first.changed);
        assert_eq!(fixture.deliveries, 1);
        fixture.inventories.push_back(Ok(status(false)));
        let replay =
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest).unwrap();
        assert!(replay.idempotent);
        assert_eq!(fixture.deliveries, 1);
    }

    #[test]
    fn completed_lock_replays_after_approval_expiry_and_without_live_inventory() {
        let (_root, store) = store("completed-expired");
        let mut fixture = provider(false);
        let plan = plan(&mut fixture);
        fixture.inventories.push_back(Ok(status(false)));
        fixture.inventories.push_back(Ok(status(true)));
        apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest).unwrap();
        assert_eq!(fixture.deliveries, 1);

        fixture.now = plan.expires_at_utc_ms + 1;
        fixture.inventories.push_back(Err(CuError::new(
            "fixture_inventory_failed",
            "fixture inventory is unavailable",
        )));
        let replay =
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest).unwrap();
        assert!(replay.idempotent);
        assert_eq!(fixture.deliveries, 1);
        assert_eq!(
            fixture.inventories.len(),
            1,
            "replay must not query the provider"
        );
    }

    #[test]
    fn known_delivery_failure_is_terminal_and_never_retried() {
        let (_root, store) = store("delivery-failed");
        let mut fixture = provider(false);
        let plan = plan(&mut fixture);
        fixture.delivery = Some(Err(SessionLockDeliveryError::NotPerformed(CuError::new(
            "fixture_delivery_failed",
            "fixture refused delivery",
        ))));
        let error = apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
            .unwrap_err();
        assert_eq!(error.code, "fixture_delivery_failed");
        assert_eq!(fixture.deliveries, 1);
        let error = apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
            .unwrap_err();
        assert_eq!(error.code, "session_lock_previously_failed");
        assert_eq!(fixture.deliveries, 1);
    }

    #[test]
    fn failed_readback_becomes_durable_unknown_and_retry_never_delivers() {
        let (_root, store) = store("readback-unknown");
        let mut fixture = provider(false);
        let plan = plan(&mut fixture);
        fixture.inventories.push_back(Ok(status(false)));
        fixture.inventories.push_back(Ok(status(false)));
        let error = apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
            .unwrap_err();
        assert_eq!(error.code, "session_lock_outcome_unknown");
        assert_eq!(fixture.deliveries, 1);
        fixture.inventories.push_back(Ok(status(false)));
        assert_eq!(
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "session_lock_outcome_unknown"
        );
        assert_eq!(fixture.deliveries, 1);
    }

    #[test]
    fn uncertain_lock_replays_after_expiry_without_live_inventory() {
        let (_root, store) = store("unknown-expired");
        let mut fixture = provider(false);
        let plan = plan(&mut fixture);
        fixture.inventories.push_back(Ok(status(false)));
        fixture.inventories.push_back(Ok(status(false)));
        assert_eq!(
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "session_lock_outcome_unknown"
        );
        assert_eq!(fixture.deliveries, 1);

        fixture.now = plan.expires_at_utc_ms + 1;
        fixture.inventories.push_back(Err(CuError::new(
            "fixture_inventory_failed",
            "fixture inventory is unavailable",
        )));
        assert_eq!(
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "session_lock_outcome_unknown"
        );
        assert_eq!(fixture.deliveries, 1);
        assert_eq!(
            fixture.inventories.len(),
            1,
            "replay must not query the provider"
        );
    }

    struct FailFinalize<'a> {
        store: &'a IdempotencyStore,
    }

    impl SessionLockLedger for FailFinalize<'_> {
        fn lookup(
            &self,
            request_id: &str,
            fingerprint: &str,
            now: i64,
        ) -> Result<Option<RequestStatus>, CuError> {
            self.store.lookup(request_id, fingerprint, now)
        }

        fn reserve(
            &self,
            request_id: &str,
            fingerprint: &str,
            retention_ms: i64,
            now: i64,
        ) -> Result<ReserveDecision, CuError> {
            self.store
                .reserve(request_id, fingerprint, retention_ms, now)
        }

        fn finalize(
            &self,
            _request_id: &str,
            _fingerprint: &str,
            _token: &str,
            _outcome: FinalOutcome,
            _now: i64,
        ) -> Result<RequestStatus, CuError> {
            Err(CuError::new(
                "fixture_finalize_failed",
                "fixture finalization failed",
            ))
        }

        fn mark_unknown(
            &self,
            request_id: &str,
            fingerprint: &str,
            token: &str,
            now: i64,
        ) -> Result<RequestStatus, CuError> {
            self.store
                .mark_outcome_unknown(request_id, fingerprint, token, now)
        }
    }

    #[test]
    fn completed_effect_with_failed_persistence_is_marked_unknown() {
        let (_root, store) = store("persist-unknown");
        let ledger = FailFinalize { store: &store };
        let mut fixture = provider(false);
        let plan = plan(&mut fixture);
        fixture.inventories.push_back(Ok(status(false)));
        fixture.inventories.push_back(Ok(status(true)));
        let error = apply_lock_with_ledger(&mut fixture, &ledger, &plan, &plan.approval_digest)
            .unwrap_err();
        assert_eq!(error.code, "session_lock_outcome_unknown");
        assert_eq!(error.detail.as_ref().unwrap()["uncertain_persisted"], true);
        assert_eq!(fixture.deliveries, 1);

        fixture.inventories.push_back(Ok(status(false)));
        assert_eq!(
            apply_lock_with_provider(&mut fixture, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "session_lock_outcome_unknown"
        );
        assert_eq!(fixture.deliveries, 1);
    }
}
