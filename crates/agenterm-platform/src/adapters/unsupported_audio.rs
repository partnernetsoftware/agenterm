use crate::audio::{AudioError, AudioErrorKind, AudioOutputSettings, NativeAudioState};

pub(crate) fn query() -> Result<NativeAudioState, AudioError> {
    Err(unsupported())
}

pub(crate) fn set(_: &NativeAudioState, _: AudioOutputSettings) -> Result<(), AudioError> {
    Err(unsupported())
}

fn unsupported() -> AudioError {
    AudioError::new(
        AudioErrorKind::Unsupported,
        "default-output volume and mute control are unsupported on this host",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_hosts_fail_truthfully_without_an_effect() {
        let error = query().unwrap_err();
        assert_eq!(error.kind(), AudioErrorKind::Unsupported);
        assert_eq!(error.effect(), crate::audio::AudioEffect::NotPerformed);
    }
}
