//! Product-neutral exclusive serial-device I/O contracts.

use std::{borrow::Cow, fmt};

pub const DEVICE_IO_MAX_BYTES: usize = 64 * 1024;
pub const DEVICE_IO_TIMEOUT_MS_MAX: u64 = 300_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialDataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialParity {
    None,
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialStopBits {
    One,
    Two,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialFlowControl {
    None,
    Software,
    Hardware,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SerialConfiguration {
    pub baud_rate: u32,
    pub data_bits: SerialDataBits,
    pub parity: SerialParity,
    pub stop_bits: SerialStopBits,
    pub flow_control: SerialFlowControl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceExclusiveMode {
    Kernel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceReadState {
    Data,
    WouldBlock,
    EndOfFile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceReadOutcome {
    pub bytes: Vec<u8>,
    pub state: DeviceReadState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceWriteDelivery {
    Complete,
    Partial,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceWriteOutcome {
    pub requested_bytes: usize,
    pub written_bytes: usize,
    pub delivery: DeviceWriteDelivery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DeviceIoErrorKind {
    InvalidArgument,
    NotFound,
    Ambiguous,
    NotClaimable,
    IdentityChanged,
    UnsafeLocator,
    NotCharacterDevice,
    Busy,
    PermissionDenied,
    Unsupported,
    OpenFailed,
    Disconnected,
    SerialApplyFailed,
    SerialReadbackMismatch,
    SerialRestoreFailed,
    ReadFailed,
    WriteFailed,
    ResourceLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceIoError {
    kind: DeviceIoErrorKind,
    code: Cow<'static, str>,
    detail: String,
    known_written_lower_bound: Option<usize>,
    delivery_uncertain: Option<bool>,
    retry_safe: Option<bool>,
}

impl DeviceIoError {
    pub(crate) fn new(
        kind: DeviceIoErrorKind,
        code: impl Into<Cow<'static, str>>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code: code.into(),
            detail: detail.into(),
            known_written_lower_bound: None,
            delivery_uncertain: None,
            retry_safe: None,
        }
    }
    pub(crate) const fn with_write_failure(
        mut self,
        known_written_lower_bound: usize,
        delivery_uncertain: bool,
        retry_safe: bool,
    ) -> Self {
        self.known_written_lower_bound = Some(known_written_lower_bound);
        self.delivery_uncertain = Some(delivery_uncertain);
        self.retry_safe = Some(retry_safe);
        self
    }
    #[must_use]
    pub const fn kind(&self) -> DeviceIoErrorKind {
        self.kind
    }
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
    #[must_use]
    pub const fn known_written_lower_bound(&self) -> Option<usize> {
        self.known_written_lower_bound
    }
    #[must_use]
    pub const fn delivery_uncertain(&self) -> Option<bool> {
        self.delivery_uncertain
    }
    #[must_use]
    pub const fn retry_safe(&self) -> Option<bool> {
        self.retry_safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_failure_metadata_distinguishes_uncertainty_from_retry_safety() {
        let error = DeviceIoError::new(
            DeviceIoErrorKind::WriteFailed,
            "device-write-failed",
            "fixture",
        )
        .with_write_failure(3, false, false);
        assert_eq!(error.known_written_lower_bound(), Some(3));
        assert_eq!(error.delivery_uncertain(), Some(false));
        assert_eq!(error.retry_safe(), Some(false));
    }
}

impl fmt::Display for DeviceIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "device I/O failed ({}): {}", self.code, self.detail)
    }
}
impl std::error::Error for DeviceIoError {}
