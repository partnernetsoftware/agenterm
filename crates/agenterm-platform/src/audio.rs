//! Exact current-default-output state and mutation transaction.

#[path = "contract/audio.rs"]
mod contract;

pub use contract::{
    AUDIO_DEVICE_FIELD_MAX_BYTES, AudioEffect, AudioError, AudioErrorKind, AudioMutationResult,
    AudioOutputDevice, AudioOutputSettings, AudioOutputState, AudioRollback,
};

use sha2::{Digest as _, Sha256};

#[cfg(target_os = "macos")]
#[path = "adapters/macos/audio.rs"]
mod native;
#[cfg(target_os = "linux")]
#[path = "adapters/linux/audio.rs"]
mod native;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[path = "adapters/unsupported_audio.rs"]
mod native;

pub(crate) const COREAUDIO_IDENTITY_DOMAIN: &[u8] = b"coreaudio\0";

#[derive(Clone, Debug)]
pub(crate) struct NativeAudioState {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) device_id: u32,
    pub(crate) provider: &'static str,
    pub(crate) identity_domain: &'static [u8],
    pub(crate) uid: String,
    pub(crate) name: String,
    pub(crate) manufacturer: String,
    pub(crate) volume_scalar: f32,
    pub(crate) muted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioObserveUnsupported {
    pub reason: String,
    pub probed_mechanisms: Vec<String>,
    pub session_bus_available: bool,
    pub required_mechanism: String,
    pub alternatives: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioQueryResult {
    Ok(AudioHostStatus),
    Unsupported(AudioObserveUnsupported),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioHostStatus {
    pub provider: String,
    pub state: AudioOutputState,
}

trait AudioProvider {
    fn query(&self) -> Result<NativeAudioState, AudioError>;
    fn set(
        &self,
        expected_device: &NativeAudioState,
        settings: AudioOutputSettings,
    ) -> Result<(), AudioError>;
}

struct SelectedProvider;

impl AudioProvider for SelectedProvider {
    fn query(&self) -> Result<NativeAudioState, AudioError> {
        native::query()
    }

    fn set(
        &self,
        expected_device: &NativeAudioState,
        settings: AudioOutputSettings,
    ) -> Result<(), AudioError> {
        native::set(expected_device, settings)
    }
}

pub fn query_default_output() -> Result<AudioOutputState, AudioError> {
    query_default_output_host().map(|status| status.state)
}

pub fn query_default_output_host() -> Result<AudioHostStatus, AudioError> {
    match query_default_output_result() {
        AudioQueryResult::Ok(status) => Ok(status),
        AudioQueryResult::Unsupported(_) => Err(AudioError::new(
            AudioErrorKind::Unsupported,
            "default-output volume and mute control are unsupported on this host",
        )),
    }
}

pub fn query_default_output_result() -> AudioQueryResult {
    match SelectedProvider.query() {
        Ok(native) => match public_state(&native) {
            Ok(state) => AudioQueryResult::Ok(AudioHostStatus {
                provider: native.provider.into(),
                state,
            }),
            Err(error) => AudioQueryResult::Unsupported(AudioObserveUnsupported {
                reason: error.detail().to_owned(),
                probed_mechanisms: vec![native.provider.into()],
                session_bus_available: false,
                required_mechanism: "linux-default-output".into(),
                alternatives: Vec::new(),
            }),
        },
        Err(error) if error.kind() == AudioErrorKind::Unsupported => {
            #[cfg(target_os = "linux")]
            {
                AudioQueryResult::Unsupported(native::unsupported_detail())
            }
            #[cfg(not(target_os = "linux"))]
            {
                AudioQueryResult::Unsupported(AudioObserveUnsupported {
                    reason: error.detail().to_owned(),
                    probed_mechanisms: Vec::new(),
                    session_bus_available: false,
                    required_mechanism: "coreaudio-default-output".into(),
                    alternatives: Vec::new(),
                })
            }
        }
        Err(error) => AudioQueryResult::Unsupported(AudioObserveUnsupported {
            reason: error.detail().to_owned(),
            probed_mechanisms: Vec::new(),
            session_bus_available: false,
            required_mechanism: "default-output-query".into(),
            alternatives: Vec::new(),
        }),
    }
}

pub fn apply_default_output_settings(
    expected_before: &AudioOutputState,
    desired_after: AudioOutputSettings,
) -> Result<AudioMutationResult, AudioError> {
    apply_with(&SelectedProvider, expected_before, desired_after)
}

fn apply_with(
    provider: &impl AudioProvider,
    expected_before: &AudioOutputState,
    desired_after: AudioOutputSettings,
) -> Result<AudioMutationResult, AudioError> {
    let native_before = provider.query()?;
    let before = public_state(&native_before)?;
    if before.device.identity != expected_before.device.identity {
        return Err(AudioError::new(
            AudioErrorKind::DeviceChanged,
            "the current default output device changed before mutation",
        ));
    }
    if before != *expected_before {
        return Err(AudioError::new(
            AudioErrorKind::StateChanged,
            "the current default output state changed before mutation",
        ));
    }
    if before.settings == desired_after {
        return Ok(AudioMutationResult {
            before: before.clone(),
            after: before,
            performed: false,
            verified: true,
        });
    }
    if let Err(error) = provider.set(&native_before, desired_after) {
        return Err(rollback_after_effect(
            provider,
            &native_before,
            before,
            error,
        ));
    }
    let observed = match provider.query().and_then(|native| public_state(&native)) {
        Ok(state) => state,
        Err(error) => {
            return Err(rollback_after_effect(
                provider,
                &native_before,
                before,
                error,
            ));
        }
    };
    if observed.device.identity == before.device.identity && observed.settings == desired_after {
        return Ok(AudioMutationResult {
            before,
            after: observed,
            performed: true,
            verified: true,
        });
    }
    let kind = if observed.device.identity == before.device.identity {
        AudioErrorKind::VerificationFailed
    } else {
        AudioErrorKind::DeviceChanged
    };
    Err(rollback_after_effect(
        provider,
        &native_before,
        before,
        AudioError::new(kind, "audio mutation did not read back as requested"),
    ))
}

fn rollback_after_effect(
    provider: &impl AudioProvider,
    native_before: &NativeAudioState,
    before: AudioOutputState,
    error: AudioError,
) -> AudioError {
    let observed = provider
        .query()
        .and_then(|native| public_state(&native))
        .ok();
    if observed
        .as_ref()
        .is_some_and(|state| state.device.identity != before.device.identity)
    {
        return error.after_effect(AudioRollback::SkippedDeviceChanged, observed);
    }
    let rollback = if provider.set(native_before, before.settings).is_ok()
        && provider
            .query()
            .and_then(|native| public_state(&native))
            .is_ok_and(|state| state == before)
    {
        AudioRollback::Verified
    } else {
        AudioRollback::Failed
    };
    error.after_effect(rollback, observed)
}

fn public_state(native: &NativeAudioState) -> Result<AudioOutputState, AudioError> {
    for (field, value) in [
        ("uid", native.uid.as_str()),
        ("name", native.name.as_str()),
        ("manufacturer", native.manufacturer.as_str()),
    ] {
        if value.is_empty()
            || value.len() > AUDIO_DEVICE_FIELD_MAX_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(AudioError::new(
                AudioErrorKind::InvalidNativeValue,
                format!("audio provider returned an invalid {field}"),
            ));
        }
    }
    if !native.volume_scalar.is_finite() || !(0.0..=1.0).contains(&native.volume_scalar) {
        return Err(AudioError::new(
            AudioErrorKind::InvalidNativeValue,
            "audio provider returned volume outside 0.0..=1.0",
        ));
    }
    let mut digest = Sha256::new();
    digest.update(native.identity_domain);
    digest.update(native.uid.as_bytes());
    Ok(AudioOutputState {
        device: AudioOutputDevice {
            uid: native.uid.clone(),
            name: native.name.clone(),
            manufacturer: native.manufacturer.clone(),
            identity: digest.finalize().into(),
        },
        settings: AudioOutputSettings {
            volume_percent: (native.volume_scalar * 100.0).round() as u8,
            muted: native.muted,
        },
    })
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque};

    use super::*;

    struct Fake {
        states: RefCell<VecDeque<NativeAudioState>>,
        sets: RefCell<Vec<AudioOutputSettings>>,
        set_fails: bool,
    }

    impl AudioProvider for Fake {
        fn query(&self) -> Result<NativeAudioState, AudioError> {
            self.states
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| AudioError::new(AudioErrorKind::QueryFailed, "fixture exhausted"))
        }

        fn set(
            &self,
            _: &NativeAudioState,
            settings: AudioOutputSettings,
        ) -> Result<(), AudioError> {
            self.sets.borrow_mut().push(settings);
            if self.set_fails {
                Err(AudioError::new(
                    AudioErrorKind::MutationFailed,
                    "fixture failure",
                ))
            } else {
                Ok(())
            }
        }
    }

    fn native(volume: f32, muted: bool) -> NativeAudioState {
        NativeAudioState {
            device_id: 7,
            provider: "macos-coreaudio",
            identity_domain: COREAUDIO_IDENTITY_DOMAIN,
            uid: "fixture-output".into(),
            name: "Fixture Speakers".into(),
            manufacturer: "Example".into(),
            volume_scalar: volume,
            muted,
        }
    }

    #[test]
    fn identity_matches_the_existing_mcu_coreaudio_contract() {
        let state = public_state(&native(0.25, false)).unwrap();
        assert_eq!(
            state.device.identity,
            [
                0x84, 0xa4, 0xbd, 0x43, 0x69, 0x3e, 0xa9, 0xa5, 0x67, 0xf0, 0x07, 0x61, 0xf1, 0x9a,
                0x63, 0x11, 0x84, 0x7d, 0x62, 0x1d, 0x0e, 0x0d, 0xcf, 0xa6, 0x57, 0x48, 0x29, 0xe9,
                0x14, 0x15, 0x5e, 0x67,
            ]
        );
    }
}
