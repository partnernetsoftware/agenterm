//! Explicit unsupported-host adapter for attached-device frame capture.

use std::time::Duration;

use crate::device_capture::{
    DeviceCaptureBackend, DeviceCaptureError, DeviceCaptureErrorKind, DeviceCaptureEvidence,
    DeviceCaptureFrame, DeviceStreamFailure, SelectedDevice,
};

pub(crate) struct UnsupportedDeviceCaptureBackend;

pub(crate) fn native_backend() -> Result<UnsupportedDeviceCaptureBackend, DeviceCaptureError> {
    Err(DeviceCaptureError::new(
        DeviceCaptureErrorKind::Unsupported,
        "attached-device capture is unsupported on this host",
        None,
    ))
}

impl DeviceCaptureBackend for UnsupportedDeviceCaptureBackend {
    fn observe(&self) -> DeviceCaptureEvidence {
        unreachable!("unsupported adapter is never constructed")
    }

    fn capture_selected(
        &self,
        _selected: &SelectedDevice,
        _timeout: Duration,
    ) -> Result<DeviceCaptureFrame, DeviceStreamFailure> {
        Err(DeviceStreamFailure::OpenFailed {
            message: "attached-device capture is unsupported on this host".to_owned(),
        })
    }
}
