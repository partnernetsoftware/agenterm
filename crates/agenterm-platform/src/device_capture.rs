//! Platform-neutral contract for capturing a frame from an attached device.
//!
//! Device discovery crosses independent host subsystems. On macOS, for
//! example, usbmux can prove that an iOS device is attached and paired while
//! CoreMediaIO/AVFoundation still publishes no capture source. Those facts
//! must not be collapsed into a guess about device trust or lock state.
//!
//! This module deliberately contains no native calls. An adapter observes each
//! subsystem, feeds the facts to [`select_device`], and only then opens the
//! selected stream. This makes the diagnostic precedence shared and testable.

use std::{borrow::Cow, time::Duration};

/// Host Camera authorization, reported even when no device source is visible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum HostCameraAuthorization {
    Authorized,
    Denied,
    Restricted,
    NotDetermined,
}

/// What the independent usbmux probe observed.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "status", rename_all = "snake_case"))]
pub enum UsbmuxObservation {
    /// The host probe itself could not run or answer reliably.
    Unavailable { message: String },
    /// Counts come from usbmux and say nothing about DAL publication.
    Inventory {
        connected_devices: usize,
        paired_devices: usize,
    },
}

/// One capture source published by the host capture stack.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeviceCaptureSource {
    pub name: String,
    pub uid: String,
}

/// What the host capture inventory observed after requesting device sources.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "status", rename_all = "snake_case"))]
pub enum DalObservation {
    /// Inventory enumeration failed. This differs from a successful empty list.
    Failed { message: String },
    /// Successfully enumerated capture sources, possibly empty.
    Inventory { sources: Vec<DeviceCaptureSource> },
}

/// One immutable preflight snapshot from all independent discovery channels.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeviceCaptureEvidence {
    pub host_camera_authorization: HostCameraAuthorization,
    pub usbmux: UsbmuxObservation,
    pub dal: DalObservation,
}

/// Inventory returned to callers. In particular, an empty `sources` list still
/// carries the host Camera authorization that explains whether discovery was
/// allowed to run.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeviceCaptureInventory {
    pub host_camera_authorization: HostCameraAuthorization,
    pub usbmux: UsbmuxObservation,
    pub sources: Vec<DeviceCaptureSource>,
}

impl DeviceCaptureEvidence {
    /// Preserve host-side state in `--list` style output instead of returning a
    /// context-free empty array.
    pub fn inventory(&self) -> Result<DeviceCaptureInventory, DeviceCaptureError> {
        let sources = match &self.dal {
            DalObservation::Inventory { sources } => sources.clone(),
            DalObservation::Failed { message } => {
                return Err(DeviceCaptureError::new(
                    DeviceCaptureErrorKind::DalInventoryFailed,
                    message.clone(),
                    None,
                ));
            }
        };
        Ok(DeviceCaptureInventory {
            host_camera_authorization: self.host_camera_authorization,
            usbmux: self.usbmux.clone(),
            sources,
        })
    }
}

/// Stable machine-readable failure taxonomy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum DeviceCaptureErrorKind {
    Unsupported,
    HostTccDenied,
    HostTccRestricted,
    HostTccConsentRequired,
    UsbmuxUnavailable,
    DeviceNotConnected,
    DeviceNotPaired,
    DalInventoryFailed,
    DeviceSourceNotPublished,
    DeviceNotFound,
    DeviceAmbiguous,
    DeviceStreamOpenFailed,
    DeviceInputRefused,
    DeviceOutputRefused,
    DeviceStreamStartFailed,
    DeviceFrameTimeout,
    DeviceLocked,
    DeviceNotTrusted,
    DeviceFrameEncodeFailed,
}

impl DeviceCaptureErrorKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "device_capture_unsupported",
            Self::HostTccDenied => "host_tcc_denied",
            Self::HostTccRestricted => "host_tcc_restricted",
            Self::HostTccConsentRequired => "host_tcc_consent_required",
            Self::UsbmuxUnavailable => "usbmux_unavailable",
            Self::DeviceNotConnected => "device_not_connected",
            Self::DeviceNotPaired => "device_not_paired",
            Self::DalInventoryFailed => "dal_inventory_failed",
            Self::DeviceSourceNotPublished => "device_source_not_published",
            Self::DeviceNotFound => "device_not_found",
            Self::DeviceAmbiguous => "device_ambiguous",
            Self::DeviceStreamOpenFailed => "device_stream_open_failed",
            Self::DeviceInputRefused => "device_input_refused",
            Self::DeviceOutputRefused => "device_output_refused",
            Self::DeviceStreamStartFailed => "device_stream_start_failed",
            Self::DeviceFrameTimeout => "device_frame_timeout",
            Self::DeviceLocked => "device_locked",
            Self::DeviceNotTrusted => "device_not_trusted",
            Self::DeviceFrameEncodeFailed => "device_frame_encode_failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeviceCaptureError {
    pub kind: DeviceCaptureErrorKind,
    pub message: String,
    pub fix: Option<Cow<'static, str>>,
}

impl DeviceCaptureError {
    pub fn new(
        kind: DeviceCaptureErrorKind,
        message: impl Into<String>,
        fix: Option<Cow<'static, str>>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            fix,
        }
    }

    pub const fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl std::fmt::Display for DeviceCaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DeviceCaptureError {}

/// A source that passed preflight selection. Its private field prevents stream
/// failures from being classified before a concrete DAL source exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedDevice(DeviceCaptureSource);

impl SelectedDevice {
    pub const fn source(&self) -> &DeviceCaptureSource {
        &self.0
    }
}

/// Select a concrete DAL source after classifying host and transport evidence.
pub fn select_device(
    evidence: &DeviceCaptureEvidence,
    selector: Option<&str>,
) -> Result<SelectedDevice, DeviceCaptureError> {
    match evidence.host_camera_authorization {
        HostCameraAuthorization::Denied => {
            return Err(DeviceCaptureError::new(
                DeviceCaptureErrorKind::HostTccDenied,
                "host Camera access is denied, so capture sources cannot be discovered",
                Some(Cow::Borrowed(
                    "grant Camera access to the stable installed application identity",
                )),
            ));
        }
        HostCameraAuthorization::Restricted => {
            return Err(DeviceCaptureError::new(
                DeviceCaptureErrorKind::HostTccRestricted,
                "host Camera access is restricted, so capture sources cannot be discovered",
                Some(Cow::Borrowed(
                    "remove the host Camera restriction or use an authorized host",
                )),
            ));
        }
        HostCameraAuthorization::NotDetermined => {
            return Err(DeviceCaptureError::new(
                DeviceCaptureErrorKind::HostTccConsentRequired,
                "host Camera consent has not been decided",
                Some(Cow::Borrowed(
                    "answer the host Camera consent prompt for the stable installed application",
                )),
            ));
        }
        HostCameraAuthorization::Authorized => {}
    }

    match &evidence.usbmux {
        UsbmuxObservation::Unavailable { message } => {
            return Err(DeviceCaptureError::new(
                DeviceCaptureErrorKind::UsbmuxUnavailable,
                message.clone(),
                None,
            ));
        }
        UsbmuxObservation::Inventory {
            connected_devices: 0,
            ..
        } => {
            return Err(DeviceCaptureError::new(
                DeviceCaptureErrorKind::DeviceNotConnected,
                "usbmux reports no attached device",
                Some(Cow::Borrowed(
                    "connect the device and verify that the cable carries data",
                )),
            ));
        }
        UsbmuxObservation::Inventory {
            paired_devices: 0, ..
        } => {
            return Err(DeviceCaptureError::new(
                DeviceCaptureErrorKind::DeviceNotPaired,
                "usbmux reports attached devices but no host pair record",
                Some(Cow::Borrowed(
                    "complete device pairing with this host, then retry",
                )),
            ));
        }
        UsbmuxObservation::Inventory { .. } => {}
    }

    let sources = match &evidence.dal {
        DalObservation::Failed { message } => {
            return Err(DeviceCaptureError::new(
                DeviceCaptureErrorKind::DalInventoryFailed,
                message.clone(),
                None,
            ));
        }
        DalObservation::Inventory { sources } if sources.is_empty() => {
            return Err(DeviceCaptureError::new(
                DeviceCaptureErrorKind::DeviceSourceNotPublished,
                "the host capture stack publishes no device capture source",
                Some(Cow::Borrowed(
                    "establish an authorized host capture session and retry source discovery",
                )),
            ));
        }
        DalObservation::Inventory { sources } => sources,
    };

    let selected = if let Some(selector) = selector {
        sources
            .iter()
            .find(|source| source.uid == selector || source.name == selector)
            .ok_or_else(|| {
                DeviceCaptureError::new(
                    DeviceCaptureErrorKind::DeviceNotFound,
                    format!("no published capture source matches {selector:?}"),
                    Some(Cow::Borrowed(
                        "list published sources and use an exact name or uid",
                    )),
                )
            })?
    } else if sources.len() == 1 {
        &sources[0]
    } else {
        return Err(DeviceCaptureError::new(
            DeviceCaptureErrorKind::DeviceAmbiguous,
            "more than one device capture source is published",
            Some(Cow::Borrowed("select one source by exact name or uid")),
        ));
    };

    Ok(SelectedDevice(selected.clone()))
}

/// Failure observed after a concrete DAL source has been selected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceStreamFailure {
    OpenFailed {
        message: String,
    },
    InputRefused {
        message: String,
    },
    OutputRefused {
        message: String,
    },
    StartFailed {
        message: String,
    },
    FrameTimeout,
    /// Use only when the native channel explicitly reports a locked device;
    /// absence of a frame is not such evidence.
    ExplicitDeviceLocked,
    /// Use only when the native channel explicitly reports rejected trust;
    /// usbmux pairing or an empty DAL inventory is not such evidence.
    ExplicitDeviceNotTrusted,
    EncodeFailed {
        message: String,
    },
}

pub fn classify_stream_failure(
    _selected: &SelectedDevice,
    failure: DeviceStreamFailure,
) -> DeviceCaptureError {
    let (kind, message, fix) = match failure {
        DeviceStreamFailure::OpenFailed { message } => (
            DeviceCaptureErrorKind::DeviceStreamOpenFailed,
            message,
            Some(Cow::Borrowed(
                "close another capture owner if present, then retry",
            )),
        ),
        DeviceStreamFailure::InputRefused { message } => (
            DeviceCaptureErrorKind::DeviceInputRefused,
            message,
            Some(Cow::Borrowed(
                "close another capture owner if present, then retry",
            )),
        ),
        DeviceStreamFailure::OutputRefused { message } => {
            (DeviceCaptureErrorKind::DeviceOutputRefused, message, None)
        }
        DeviceStreamFailure::StartFailed { message } => (
            DeviceCaptureErrorKind::DeviceStreamStartFailed,
            message,
            None,
        ),
        DeviceStreamFailure::FrameTimeout => (
            DeviceCaptureErrorKind::DeviceFrameTimeout,
            "the selected capture source emitted no frame before the deadline".to_owned(),
            Some(Cow::Borrowed(
                "check the device display and competing capture sessions; the timeout alone does not prove either cause",
            )),
        ),
        DeviceStreamFailure::ExplicitDeviceLocked => (
            DeviceCaptureErrorKind::DeviceLocked,
            "the selected device explicitly reported that it is locked".to_owned(),
            Some(Cow::Borrowed("wake and unlock the selected device")),
        ),
        DeviceStreamFailure::ExplicitDeviceNotTrusted => (
            DeviceCaptureErrorKind::DeviceNotTrusted,
            "the selected device explicitly rejected host trust".to_owned(),
            Some(Cow::Borrowed(
                "complete the device trust exchange with this host",
            )),
        ),
        DeviceStreamFailure::EncodeFailed { message } => (
            DeviceCaptureErrorKind::DeviceFrameEncodeFailed,
            message,
            None,
        ),
    };
    DeviceCaptureError::new(kind, message, fix)
}

/// Encoded frame bytes stay inside the process boundary; product callers own
/// atomic, no-overwrite publication and never print the bytes to stdout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceCaptureFrame {
    pub encoded: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub source: DeviceCaptureSource,
}

/// Native adapter boundary. Implementations observe first, then receive only a
/// source selected by the shared classifier.
pub trait DeviceCaptureBackend {
    fn observe(&self) -> DeviceCaptureEvidence;

    fn capture_selected(
        &self,
        selected: &SelectedDevice,
        timeout: Duration,
    ) -> Result<DeviceCaptureFrame, DeviceStreamFailure>;
}

/// Construct the selected native backend without exposing its Objective-C ABI
/// to product crates. The product feature is currently macOS-only; other hosts
/// must advertise an explicit platform limitation rather than fabricate an
/// empty inventory.
#[cfg(all(feature = "device-capture", target_os = "macos"))]
pub fn native_backend() -> impl DeviceCaptureBackend {
    crate::selected::device_capture::MacosDeviceCaptureBackend
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> DeviceCaptureSource {
        DeviceCaptureSource {
            name: "Fixture Phone".to_owned(),
            uid: "fixture-device-1".to_owned(),
        }
    }

    fn evidence(
        auth: HostCameraAuthorization,
        connected: usize,
        paired: usize,
        sources: Vec<DeviceCaptureSource>,
    ) -> DeviceCaptureEvidence {
        DeviceCaptureEvidence {
            host_camera_authorization: auth,
            usbmux: UsbmuxObservation::Inventory {
                connected_devices: connected,
                paired_devices: paired,
            },
            dal: DalObservation::Inventory { sources },
        }
    }

    #[test]
    fn empty_inventory_with_denied_tcc_points_to_host_not_device() {
        let facts = evidence(HostCameraAuthorization::Denied, 1, 1, vec![]);
        let error = select_device(&facts, None).expect_err("host TCC must fail first");
        assert_eq!(error.code(), "host_tcc_denied");
        assert_ne!(error.kind, DeviceCaptureErrorKind::DeviceNotTrusted);
        assert_ne!(error.kind, DeviceCaptureErrorKind::DeviceLocked);

        let inventory = facts
            .inventory()
            .expect("list preserves an empty inventory");
        assert!(inventory.sources.is_empty());
        assert_eq!(
            inventory.host_camera_authorization,
            HostCameraAuthorization::Denied
        );
    }

    #[test]
    fn empty_inventory_after_authorized_preflight_is_source_not_published() {
        let facts = evidence(HostCameraAuthorization::Authorized, 1, 1, vec![]);
        let error = select_device(&facts, None).expect_err("no DAL source");
        assert_eq!(error.code(), "device_source_not_published");
        assert_ne!(error.kind, DeviceCaptureErrorKind::DeviceNotTrusted);
        assert_ne!(error.kind, DeviceCaptureErrorKind::DeviceLocked);
    }

    #[test]
    fn undecided_and_restricted_camera_states_remain_typed() {
        for (authorization, expected) in [
            (
                HostCameraAuthorization::NotDetermined,
                "host_tcc_consent_required",
            ),
            (HostCameraAuthorization::Restricted, "host_tcc_restricted"),
        ] {
            let facts = evidence(authorization, 1, 1, vec![]);
            assert_eq!(
                select_device(&facts, None)
                    .expect_err("host authorization blocks discovery")
                    .code(),
                expected
            );
        }
    }

    #[test]
    fn probe_and_dal_inventory_failures_are_not_empty_inventory() {
        let usbmux_failed = DeviceCaptureEvidence {
            host_camera_authorization: HostCameraAuthorization::Authorized,
            usbmux: UsbmuxObservation::Unavailable {
                message: "fixture usbmux failure".to_owned(),
            },
            dal: DalObservation::Inventory { sources: vec![] },
        };
        assert_eq!(
            select_device(&usbmux_failed, None)
                .expect_err("usbmux probe failed")
                .code(),
            "usbmux_unavailable"
        );

        let dal_failed = DeviceCaptureEvidence {
            host_camera_authorization: HostCameraAuthorization::Authorized,
            usbmux: UsbmuxObservation::Inventory {
                connected_devices: 1,
                paired_devices: 1,
            },
            dal: DalObservation::Failed {
                message: "fixture DAL failure".to_owned(),
            },
        };
        assert_eq!(
            select_device(&dal_failed, None)
                .expect_err("DAL inventory failed")
                .code(),
            "dal_inventory_failed"
        );
        assert_eq!(
            dal_failed
                .inventory()
                .expect_err("list must not turn a failed inventory into count zero")
                .code(),
            "dal_inventory_failed"
        );
    }

    #[test]
    fn transport_failures_remain_distinct_from_capture_inventory() {
        let disconnected = evidence(HostCameraAuthorization::Authorized, 0, 0, vec![]);
        assert_eq!(
            select_device(&disconnected, None)
                .expect_err("disconnected")
                .code(),
            "device_not_connected"
        );

        let unpaired = evidence(HostCameraAuthorization::Authorized, 1, 0, vec![]);
        assert_eq!(
            select_device(&unpaired, None).expect_err("unpaired").code(),
            "device_not_paired"
        );
    }

    #[test]
    fn frame_timeout_does_not_guess_lock_or_trust() {
        let selected = select_device(
            &evidence(HostCameraAuthorization::Authorized, 1, 1, vec![source()]),
            None,
        )
        .expect("selected source");
        let error = classify_stream_failure(&selected, DeviceStreamFailure::FrameTimeout);
        assert_eq!(error.code(), "device_frame_timeout");
        assert!(!error.message.contains("locked"));
        assert!(!error.message.contains("trust"));
    }

    #[test]
    fn device_specific_diagnostics_require_selected_source_and_explicit_signal() {
        let selected = select_device(
            &evidence(HostCameraAuthorization::Authorized, 1, 1, vec![source()]),
            Some("fixture-device-1"),
        )
        .expect("selected source");
        assert_eq!(selected.source().uid, "fixture-device-1");
        assert_eq!(
            classify_stream_failure(&selected, DeviceStreamFailure::ExplicitDeviceLocked).code(),
            "device_locked"
        );
        assert_eq!(
            classify_stream_failure(&selected, DeviceStreamFailure::ExplicitDeviceNotTrusted)
                .code(),
            "device_not_trusted"
        );
    }

    #[test]
    fn output_refusal_has_its_own_stage() {
        let selected = select_device(
            &evidence(HostCameraAuthorization::Authorized, 1, 1, vec![source()]),
            None,
        )
        .expect("selected source");
        let error = classify_stream_failure(
            &selected,
            DeviceStreamFailure::OutputRefused {
                message: "fixture output refusal".to_owned(),
            },
        );
        assert_eq!(error.code(), "device_output_refused");
    }
}
