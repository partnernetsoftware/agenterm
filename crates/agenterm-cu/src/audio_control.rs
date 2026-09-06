//! Default-output audio observation and approval-bound, at-most-once mutation.

use std::time::{SystemTime, UNIX_EPOCH};

use agenterm_platform::audio::{
    AudioEffect, AudioError, AudioErrorKind, AudioMutationResult, AudioOutputSettings,
    AudioOutputState, AudioRollback,
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

const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_TTL_SECONDS: u64 = 120;
const MIN_TTL_SECONDS: u64 = 1;
const MAX_TTL_SECONDS: u64 = 600;
const REQUEST_BYTES_MAX: usize = 16 * 1024;
const REPLAY_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const CONTRACT_DOMAIN: &[u8] = b"agenterm-cu/audio-contract/v1\0";
const APPROVAL_DOMAIN: &[u8] = b"agenterm-cu/audio-approval/v1\0";
const OUTCOME_CHANGED: &str = "audio_completed_changed";
const OUTCOME_NOOP: &str = "audio_completed_noop";
const OUTCOME_NOT_PERFORMED: &str = "audio_failed_not_performed";
const OUTCOME_ROLLED_BACK: &str = "audio_failed_rolled_back";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioDevice {
    pub uid: String,
    pub name: String,
    pub manufacturer: String,
    pub identity: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioSettings {
    pub volume: u8,
    pub muted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioState {
    pub device: AudioDevice,
    pub settings: AudioSettings,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioStatus {
    pub provider: String,
    pub state: AudioState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AudioOperation {
    #[serde(rename = "audio.set-default-output")]
    SetDefaultOutput,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioPlan {
    pub schema_version: u32,
    pub operation: AudioOperation,
    pub target: AudioDevice,
    pub before: AudioSettings,
    pub after: AudioSettings,
    pub issued_at_utc_ms: u64,
    pub expires_at_utc_ms: u64,
    pub contract_digest: String,
    pub approval_digest: String,
    pub mutation_performed: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioApplyState {
    Completed,
    Replayed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioApplyReply {
    pub state: AudioApplyState,
    pub idempotent: bool,
    pub changed: bool,
    pub verified: bool,
    pub settings: AudioSettings,
    pub contract_digest: String,
    pub approval_digest: String,
}

pub trait AudioProvider {
    fn status(&mut self) -> Result<AudioState, CuError>;
    fn apply(
        &mut self,
        expected: &AudioState,
        desired: AudioSettings,
    ) -> Result<AudioMutationResult, AudioError>;
    fn now_utc_ms(&mut self) -> Result<u64, CuError>;
}

#[derive(Default)]
pub struct NativeAudioProvider;

impl AudioProvider for NativeAudioProvider {
    fn status(&mut self) -> Result<AudioState, CuError> {
        agenterm_platform::audio::query_default_output()
            .map(project_state)
            .map_err(platform_error)
    }

    fn apply(
        &mut self,
        expected: &AudioState,
        desired: AudioSettings,
    ) -> Result<AudioMutationResult, AudioError> {
        let expected = native_state(expected)
            .map_err(|error| AudioError::invalid_native_value(error.message))?;
        agenterm_platform::audio::apply_default_output_settings(
            &expected,
            AudioOutputSettings {
                volume_percent: desired.volume,
                muted: desired.muted,
            },
        )
    }

    fn now_utc_ms(&mut self) -> Result<u64, CuError> {
        now_utc_ms()
    }
}

pub fn status() -> Result<AudioStatus, CuError> {
    status_with_provider(&mut NativeAudioProvider)
}

pub fn status_with_provider(provider: &mut impl AudioProvider) -> Result<AudioStatus, CuError> {
    let state = provider.status()?;
    validate_state(&state)?;
    Ok(AudioStatus {
        provider: "macos-coreaudio".into(),
        state,
    })
}

pub fn plan_volume(volume: u8, ttl_seconds: u64) -> Result<AudioPlan, CuError> {
    plan_with_provider(&mut NativeAudioProvider, Some(volume), None, ttl_seconds)
}

pub fn plan_muted(muted: bool, ttl_seconds: u64) -> Result<AudioPlan, CuError> {
    plan_with_provider(&mut NativeAudioProvider, None, Some(muted), ttl_seconds)
}

pub fn plan_with_provider(
    provider: &mut impl AudioProvider,
    volume: Option<u8>,
    muted: Option<bool>,
    ttl_seconds: u64,
) -> Result<AudioPlan, CuError> {
    if volume.is_some() == muted.is_some() {
        return Err(CuError::new(
            "audio_plan_invalid",
            "audio plan requires exactly one of volume or muted",
        ));
    }
    if !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&ttl_seconds) {
        return Err(CuError::new(
            "audio_plan_ttl_invalid",
            "audio plan TTL must be in 1..=600 seconds",
        ));
    }
    let before_state = status_with_provider(provider)?.state;
    let mut after = before_state.settings;
    if let Some(volume) = volume {
        if volume > 100 {
            return Err(CuError::new(
                "audio_volume_invalid",
                "audio volume must be an integer in 0..=100",
            ));
        }
        after.volume = volume;
    }
    if let Some(muted) = muted {
        after.muted = muted;
    }
    build_plan(provider, before_state, after, ttl_seconds)
}

fn build_plan(
    provider: &mut impl AudioProvider,
    before_state: AudioState,
    after: AudioSettings,
    ttl_seconds: u64,
) -> Result<AudioPlan, CuError> {
    let issued = provider.now_utc_ms()?;
    let expires =
        issued
            .checked_add(ttl_seconds.checked_mul(1_000).ok_or_else(|| {
                CuError::new("audio_plan_ttl_invalid", "audio plan TTL overflows")
            })?)
            .ok_or_else(|| CuError::new("audio_clock_invalid", "audio plan expiry overflows"))?;
    let mut plan = AudioPlan {
        schema_version: SCHEMA_VERSION,
        operation: AudioOperation::SetDefaultOutput,
        target: before_state.device,
        before: before_state.settings,
        after,
        issued_at_utc_ms: issued,
        expires_at_utc_ms: expires,
        contract_digest: String::new(),
        approval_digest: String::new(),
        mutation_performed: false,
    };
    plan.contract_digest = contract_digest(&plan)?;
    plan.approval_digest = approval_digest(&plan)?;
    Ok(plan)
}

pub fn reverse_plan(plan: &AudioPlan) -> Result<AudioPlan, CuError> {
    validate_plan_integrity(plan, &plan.approval_digest)?;
    let mut reverse = plan.clone();
    reverse.before = plan.after;
    reverse.after = plan.before;
    reverse.contract_digest = contract_digest(&reverse)?;
    reverse.approval_digest = approval_digest(&reverse)?;
    Ok(reverse)
}

pub fn encode_request(plan: &AudioPlan) -> Result<String, CuError> {
    let bytes = serde_json::to_vec(plan)
        .map_err(|_| CuError::new("audio_request_invalid", "audio plan cannot be serialized"))?;
    if bytes.len() > REQUEST_BYTES_MAX {
        return Err(CuError::new(
            "audio_request_limit",
            "audio request exceeds its byte ceiling",
        ));
    }
    Ok(crate::managed_job_ipc::base64_encode(&bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_owned())
}

pub fn decode_request(encoded: &str) -> Result<AudioPlan, CuError> {
    if encoded.is_empty()
        || encoded.len() > REQUEST_BYTES_MAX * 2
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CuError::new(
            "audio_request_invalid",
            "audio request is not bounded unpadded base64url",
        ));
    }
    let mut standard = encoded.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let bytes = crate::managed_job_ipc::base64_decode(&standard)
        .map_err(|_| CuError::new("audio_request_invalid", "audio request is malformed"))?;
    if bytes.is_empty() || bytes.len() > REQUEST_BYTES_MAX {
        return Err(CuError::new(
            "audio_request_limit",
            "decoded audio request exceeds its byte ceiling",
        ));
    }
    let canonical = crate::managed_job_ipc::base64_encode(&bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_owned();
    if canonical != encoded {
        return Err(CuError::new(
            "audio_request_invalid",
            "audio request is not canonical unpadded base64url",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        CuError::new(
            "audio_request_invalid",
            "audio request has an invalid shape",
        )
    })
}

pub fn apply(plan: &AudioPlan, approval: &str) -> Result<AudioApplyReply, CuError> {
    let store = IdempotencyStore::open()?;
    apply_with_provider(&mut NativeAudioProvider, &store, plan, approval)
}

pub fn apply_with_provider(
    provider: &mut impl AudioProvider,
    store: &IdempotencyStore,
    plan: &AudioPlan,
    approval: &str,
) -> Result<AudioApplyReply, CuError> {
    let now = provider.now_utc_ms()?;
    validate_plan_integrity(plan, approval)?;
    let canonical = serde_json::to_vec(plan)
        .map_err(|_| CuError::new("audio_plan_invalid", "audio plan cannot be serialized"))?;
    let fingerprint = fingerprint_canonical_request(&canonical)?;
    if let Some(status) = store.lookup(&plan.approval_digest, &fingerprint, to_i64(now)?)? {
        return replay(plan, status);
    }
    if now > plan.expires_at_utc_ms {
        return Err(CuError::new(
            "audio_plan_expired",
            "audio approval expired before reservation",
        ));
    }
    let live = status_with_provider(provider)?.state;
    if live.device.identity != plan.target.identity {
        return Err(CuError::new(
            "audio_device_changed",
            "the default output device changed after planning",
        ));
    }
    if live.device != plan.target || live.settings != plan.before {
        return Err(CuError::new(
            "audio_state_changed",
            "the exact default output state changed after planning",
        ));
    }
    let reservation = match store.reserve(
        &plan.approval_digest,
        &fingerprint,
        REPLAY_RETENTION_MS,
        to_i64(now)?,
    )? {
        ReserveDecision::Fresh(fresh) => fresh,
        ReserveDecision::ReplayFinalized(status) => return replay(plan, status),
        ReserveDecision::Uncertain(_) => return Err(outcome_unknown()),
    };
    if plan.before == plan.after {
        finalize(
            store,
            plan,
            &fingerprint,
            &reservation,
            FinalOutcomeKind::Succeeded,
            OUTCOME_NOOP,
            now,
        )?;
        return Ok(reply(plan, AudioApplyState::Completed, false, false));
    }
    match provider.apply(&live, plan.after) {
        Ok(result) if result.verified && result.after.settings == native_settings(plan.after) => {
            finalize(
                store,
                plan,
                &fingerprint,
                &reservation,
                FinalOutcomeKind::Succeeded,
                OUTCOME_CHANGED,
                now,
            )?;
            Ok(reply(
                plan,
                AudioApplyState::Completed,
                false,
                result.performed,
            ))
        }
        Ok(_) => Err(mark_unknown(
            store,
            plan,
            &fingerprint,
            &reservation,
            now,
            "audio_verification_failed",
        )),
        Err(error) => handle_platform_failure(store, plan, &fingerprint, &reservation, now, error),
    }
}

fn handle_platform_failure(
    store: &IdempotencyStore,
    plan: &AudioPlan,
    fingerprint: &str,
    reservation: &FreshReservation,
    now: u64,
    error: AudioError,
) -> Result<AudioApplyReply, CuError> {
    let known =
        error.effect() == AudioEffect::NotPerformed || error.rollback() == AudioRollback::Verified;
    let code = if error.rollback() == AudioRollback::Verified {
        OUTCOME_ROLLED_BACK
    } else {
        OUTCOME_NOT_PERFORMED
    };
    let typed = platform_error(error);
    if known {
        finalize(
            store,
            plan,
            fingerprint,
            reservation,
            FinalOutcomeKind::Failed,
            code,
            now,
        )?;
        Err(typed.with_detail(serde_json::json!({
            "effect": "not_applied",
            "rollback": if code == OUTCOME_ROLLED_BACK { "verified" } else { "not_needed" },
        })))
    } else {
        Err(mark_unknown(
            store,
            plan,
            fingerprint,
            reservation,
            now,
            &typed.code,
        ))
    }
}

fn validate_plan_integrity(plan: &AudioPlan, approval: &str) -> Result<(), CuError> {
    validate_state(&AudioState {
        device: plan.target.clone(),
        settings: plan.before,
    })?;
    if plan.schema_version != SCHEMA_VERSION
        || plan.operation != AudioOperation::SetDefaultOutput
        || plan.after.volume > 100
        || plan.mutation_performed
        || plan.expires_at_utc_ms <= plan.issued_at_utc_ms
        || plan.expires_at_utc_ms - plan.issued_at_utc_ms > MAX_TTL_SECONDS * 1_000
        || plan.contract_digest != contract_digest(plan)?
        || plan.approval_digest != approval_digest(plan)?
        || approval != plan.approval_digest
    {
        return Err(CuError::new(
            "audio_approval_mismatch",
            "audio plan content does not match its approval digest",
        ));
    }
    Ok(())
}

fn validate_state(state: &AudioState) -> Result<(), CuError> {
    let text = |value: &str| {
        !value.is_empty()
            && value.len() <= 256
            && !value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    };
    if !text(&state.device.uid)
        || !text(&state.device.name)
        || !text(&state.device.manufacturer)
        || state.device.identity.len() != 64
        || !state
            .device
            .identity
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || state.settings.volume > 100
    {
        return Err(CuError::new(
            "audio_provider_shape",
            "audio provider returned an invalid bounded state",
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct Contract<'a> {
    schema_version: u32,
    operation: AudioOperation,
    target: &'a AudioDevice,
    before: AudioSettings,
    after: AudioSettings,
}

#[derive(Serialize)]
struct Approval<'a> {
    contract_digest: &'a str,
    issued_at_utc_ms: u64,
    expires_at_utc_ms: u64,
}

fn contract_digest(plan: &AudioPlan) -> Result<String, CuError> {
    digest(
        CONTRACT_DOMAIN,
        &Contract {
            schema_version: plan.schema_version,
            operation: plan.operation,
            target: &plan.target,
            before: plan.before,
            after: plan.after,
        },
    )
}

fn approval_digest(plan: &AudioPlan) -> Result<String, CuError> {
    digest(
        APPROVAL_DOMAIN,
        &Approval {
            contract_digest: &plan.contract_digest,
            issued_at_utc_ms: plan.issued_at_utc_ms,
            expires_at_utc_ms: plan.expires_at_utc_ms,
        },
    )
}

fn digest(domain: &[u8], value: &impl Serialize) -> Result<String, CuError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| CuError::new("audio_plan_invalid", "audio plan cannot be serialized"))?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    Ok(hex(&hasher.finalize()))
}

fn project_state(value: AudioOutputState) -> AudioState {
    AudioState {
        device: AudioDevice {
            uid: value.device.uid,
            name: value.device.name,
            manufacturer: value.device.manufacturer,
            identity: hex(&value.device.identity),
        },
        settings: AudioSettings {
            volume: value.settings.volume_percent,
            muted: value.settings.muted,
        },
    }
}

fn native_state(value: &AudioState) -> Result<AudioOutputState, CuError> {
    let identity = decode_hex_32(&value.device.identity)?;
    Ok(AudioOutputState {
        device: agenterm_platform::audio::AudioOutputDevice {
            uid: value.device.uid.clone(),
            name: value.device.name.clone(),
            manufacturer: value.device.manufacturer.clone(),
            identity,
        },
        settings: native_settings(value.settings),
    })
}

fn native_settings(value: AudioSettings) -> AudioOutputSettings {
    AudioOutputSettings {
        volume_percent: value.volume,
        muted: value.muted,
    }
}

fn decode_hex_32(value: &str) -> Result<[u8; 32], CuError> {
    if value.len() != 64 {
        return Err(CuError::new(
            "audio_identity_invalid",
            "audio identity is not SHA-256",
        ));
    }
    let mut bytes = [0_u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| CuError::new("audio_identity_invalid", "audio identity is not SHA-256"))?;
    }
    Ok(bytes)
}

fn platform_error(error: AudioError) -> CuError {
    let code = match error.kind() {
        AudioErrorKind::Unsupported => "audio_unsupported",
        AudioErrorKind::InvalidNativeValue => "audio_provider_shape",
        AudioErrorKind::QueryFailed => "audio_query_failed",
        AudioErrorKind::StateChanged => "audio_state_changed",
        AudioErrorKind::DeviceChanged => "audio_device_changed",
        AudioErrorKind::MutationFailed => "audio_mutation_failed",
        AudioErrorKind::VerificationFailed => "audio_verification_failed",
        _ => "audio_provider_failed",
    };
    CuError::new(code, error.detail().to_owned())
}

fn finalize(
    store: &IdempotencyStore,
    plan: &AudioPlan,
    fingerprint: &str,
    reservation: &FreshReservation,
    kind: FinalOutcomeKind,
    code: &str,
    now: u64,
) -> Result<(), CuError> {
    store.finalize(
        &plan.approval_digest,
        fingerprint,
        &reservation.completion_token,
        FinalOutcome::new(kind, code, None)?,
        to_i64(now)?,
    )?;
    Ok(())
}

fn mark_unknown(
    store: &IdempotencyStore,
    plan: &AudioPlan,
    fingerprint: &str,
    reservation: &FreshReservation,
    now: u64,
    cause: &str,
) -> CuError {
    let persisted = store
        .mark_outcome_unknown(
            &plan.approval_digest,
            fingerprint,
            &reservation.completion_token,
            i64::try_from(now).unwrap_or(i64::MAX),
        )
        .is_ok();
    CuError::new(
        "audio_effect_uncertain",
        "audio mutation may have been applied; automatic replay is refused",
    )
    .with_detail(serde_json::json!({
        "effect": "unknown",
        "cause": cause,
        "uncertain_persisted": persisted,
    }))
}

fn replay(plan: &AudioPlan, status: RequestStatus) -> Result<AudioApplyReply, CuError> {
    match status.state {
        RequestState::Reserved | RequestState::OutcomeUnknown => return Err(outcome_unknown()),
        RequestState::Finalized => {}
    }
    let outcome = status.outcome.ok_or_else(|| {
        CuError::new(
            "audio_receipt_invalid",
            "finalized audio receipt has no outcome",
        )
    })?;
    if outcome.kind == FinalOutcomeKind::Failed {
        return Err(CuError::new(
            "audio_previously_failed",
            "this audio approval already has a terminal failed receipt",
        )
        .with_detail(serde_json::json!({ "effect": "not_repeated", "outcome": outcome.code })));
    }
    match outcome.code.as_str() {
        OUTCOME_CHANGED => Ok(reply(plan, AudioApplyState::Replayed, true, true)),
        OUTCOME_NOOP => Ok(reply(plan, AudioApplyState::Replayed, true, false)),
        _ => Err(CuError::new(
            "audio_receipt_invalid",
            "audio receipt has an unknown terminal outcome",
        )),
    }
}

fn reply(
    plan: &AudioPlan,
    state: AudioApplyState,
    idempotent: bool,
    changed: bool,
) -> AudioApplyReply {
    AudioApplyReply {
        state,
        idempotent,
        changed,
        verified: true,
        settings: plan.after,
        contract_digest: plan.contract_digest.clone(),
        approval_digest: plan.approval_digest.clone(),
    }
}

fn outcome_unknown() -> CuError {
    CuError::new(
        "audio_effect_uncertain",
        "this audio approval may already have been applied; automatic replay is refused",
    )
    .with_detail(serde_json::json!({ "effect": "unknown" }))
}

fn now_utc_ms() -> Result<u64, CuError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CuError::new("audio_clock_invalid", "host clock is before the Unix epoch"))?
        .as_millis()
        .try_into()
        .map_err(|_| {
            CuError::new(
                "audio_clock_invalid",
                "host clock does not fit the audio contract",
            )
        })
}

fn to_i64(value: u64) -> Result<i64, CuError> {
    value.try_into().map_err(|_| {
        CuError::new(
            "audio_clock_invalid",
            "audio timestamp exceeds receipt range",
        )
    })
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    struct TestPath(PathBuf);

    impl TestPath {
        fn new(name: &str) -> Self {
            Self(
                std::env::current_dir()
                    .unwrap()
                    .join("target/audio-control-test-state")
                    .join(format!("{name}-{}", std::process::id())),
            )
        }
    }

    impl Drop for TestPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        state: AudioState,
        now: u64,
        applies: usize,
        verified: bool,
    }

    impl AudioProvider for Fixture {
        fn status(&mut self) -> Result<AudioState, CuError> {
            Ok(self.state.clone())
        }

        fn apply(
            &mut self,
            expected: &AudioState,
            desired: AudioSettings,
        ) -> Result<AudioMutationResult, AudioError> {
            assert_eq!(expected, &self.state);
            self.applies += 1;
            let before = native_state(&self.state).unwrap();
            self.state.settings = desired;
            let after = native_state(&self.state).unwrap();
            Ok(AudioMutationResult {
                before,
                after,
                performed: true,
                verified: self.verified,
            })
        }

        fn now_utc_ms(&mut self) -> Result<u64, CuError> {
            Ok(self.now)
        }
    }

    fn state() -> AudioState {
        AudioState {
            device: AudioDevice {
                uid: "fixture-output".into(),
                name: "Fixture Speakers".into(),
                manufacturer: "Example".into(),
                identity: "11".repeat(32),
            },
            settings: AudioSettings {
                volume: 25,
                muted: false,
            },
        }
    }

    fn fixture() -> Fixture {
        Fixture {
            state: state(),
            now: 1_000,
            applies: 0,
            verified: true,
        }
    }

    fn store(name: &str) -> (TestPath, IdempotencyStore) {
        let root = TestPath::new(name);
        let store = IdempotencyStore::open_at(root.0.join("requests.json")).unwrap();
        (root, store)
    }

    #[test]
    fn plan_is_read_only_bounded_and_has_a_closed_reverse() {
        let mut provider = fixture();
        let plan = plan_with_provider(&mut provider, Some(60), None, 120).unwrap();
        assert_eq!(plan.before.volume, 25);
        assert_eq!(plan.after.volume, 60);
        assert_eq!(plan.before.muted, plan.after.muted);
        assert_eq!(plan.expires_at_utc_ms - plan.issued_at_utc_ms, 120_000);
        assert_ne!(plan.contract_digest, plan.approval_digest);
        assert_eq!(provider.applies, 0);
        let encoded = encode_request(&plan).unwrap();
        assert_eq!(decode_request(&encoded).unwrap(), plan);
        let reverse = reverse_plan(&plan).unwrap();
        assert_eq!(reverse.before, plan.after);
        assert_eq!(reverse.after, plan.before);
        assert_ne!(reverse.approval_digest, plan.approval_digest);
    }

    #[test]
    fn apply_is_exact_at_most_once_and_replays_after_approval_expiry() {
        let (_root, store) = store("apply-replay");
        let mut provider = fixture();
        let plan = plan_with_provider(&mut provider, None, Some(true), 120).unwrap();
        let first =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap();
        assert!(first.changed && first.verified && !first.idempotent);
        assert_eq!(provider.applies, 1);
        provider.now = plan.expires_at_utc_ms + 1;
        provider.state.device.identity = "22".repeat(32);
        let replay =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap();
        assert!(replay.idempotent && replay.changed);
        assert_eq!(provider.applies, 1);
    }

    #[test]
    fn tamper_expiry_and_live_drift_refuse_before_effect() {
        let (_root, store) = store("refusals");
        let mut provider = fixture();
        let plan = plan_with_provider(&mut provider, Some(60), None, 120).unwrap();
        let mut tampered = plan.clone();
        tampered.after.volume = 61;
        assert_eq!(
            apply_with_provider(&mut provider, &store, &tampered, &plan.approval_digest)
                .unwrap_err()
                .code,
            "audio_approval_mismatch"
        );
        provider.now = plan.expires_at_utc_ms + 1;
        assert_eq!(
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "audio_plan_expired"
        );
        provider.now = 2_000;
        provider.state.settings.volume = 26;
        assert_eq!(
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest)
                .unwrap_err()
                .code,
            "audio_state_changed"
        );
        assert_eq!(provider.applies, 0);
    }

    #[test]
    fn a_noop_consumes_the_approval_and_replays_without_live_state() {
        let (_root, store) = store("noop-replay");
        let mut provider = fixture();
        let plan = plan_with_provider(&mut provider, Some(25), None, 120).unwrap();
        let first =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap();
        assert!(!first.changed && first.verified && !first.idempotent);
        assert_eq!(provider.applies, 0);
        provider.now = plan.expires_at_utc_ms + 1;
        provider.state.device.identity = "22".repeat(32);
        let replay =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap();
        assert!(replay.idempotent && !replay.changed);
        assert_eq!(provider.applies, 0);
    }

    #[test]
    fn unverifiable_effect_is_durable_unknown_and_never_repeated() {
        let (_root, store) = store("unknown-replay");
        let mut provider = fixture();
        provider.verified = false;
        let plan = plan_with_provider(&mut provider, Some(60), None, 120).unwrap();
        let first =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap_err();
        assert_eq!(first.code, "audio_effect_uncertain");
        assert_eq!(provider.applies, 1);
        provider.verified = true;
        provider.now = plan.expires_at_utc_ms + 1;
        let replay =
            apply_with_provider(&mut provider, &store, &plan, &plan.approval_digest).unwrap_err();
        assert_eq!(replay.code, "audio_effect_uncertain");
        assert_eq!(provider.applies, 1);
    }
}
