//! Product-neutral default-output audio state and mutation results.

/// Maximum UTF-8 byte length of a native device identity or display field.
pub const AUDIO_DEVICE_FIELD_MAX_BYTES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioOutputDevice {
    /// The provider-stable CoreAudio device UID. It is not a path or open handle.
    pub uid: String,
    pub name: String,
    pub manufacturer: String,
    /// Domain-separated SHA-256 of `uid`, suitable for equality and stale checks.
    pub identity: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioOutputSettings {
    pub volume_percent: u8,
    pub muted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioOutputState {
    pub device: AudioOutputDevice,
    pub settings: AudioOutputSettings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioMutationResult {
    pub before: AudioOutputState,
    pub after: AudioOutputState,
    pub performed: bool,
    pub verified: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioEffect {
    NotPerformed,
    PossiblyApplied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioRollback {
    NotNeeded,
    Verified,
    Failed,
    SkippedDeviceChanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AudioErrorKind {
    Unsupported,
    InvalidNativeValue,
    QueryFailed,
    StateChanged,
    DeviceChanged,
    MutationFailed,
    VerificationFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioError {
    kind: AudioErrorKind,
    detail: String,
    effect: AudioEffect,
    rollback: AudioRollback,
    observed: Option<Box<AudioOutputState>>,
}

impl AudioError {
    /// Constructs a boundary error for a value that cannot be represented by
    /// this typed contract.
    pub fn invalid_native_value(detail: impl Into<String>) -> Self {
        Self::new(AudioErrorKind::InvalidNativeValue, detail)
    }

    pub(crate) fn new(kind: AudioErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            effect: AudioEffect::NotPerformed,
            rollback: AudioRollback::NotNeeded,
            observed: None,
        }
    }

    pub(crate) fn after_effect(
        mut self,
        rollback: AudioRollback,
        observed: Option<AudioOutputState>,
    ) -> Self {
        self.effect = AudioEffect::PossiblyApplied;
        self.rollback = rollback;
        self.observed = observed.map(Box::new);
        self
    }

    pub const fn kind(&self) -> AudioErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub const fn effect(&self) -> AudioEffect {
        self.effect
    }

    pub const fn rollback(&self) -> AudioRollback {
        self.rollback
    }

    pub fn observed(&self) -> Option<&AudioOutputState> {
        self.observed.as_deref()
    }
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "audio {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for AudioError {}
