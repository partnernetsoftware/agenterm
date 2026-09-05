//! macOS AVFoundation/CoreMediaIO adapter for wired-device frame capture.
//!
//! The Objective-C translation unit owns framework objects and allocations;
//! this module validates and copies every value before releasing that native
//! storage. `observe` never requests Camera permission and capture receives
//! only an exact DAL uid selected by the shared contract.

use std::{ffi::CString, time::Duration};

use crate::device_capture::{
    DalObservation, DeviceCaptureBackend, DeviceCaptureEvidence, DeviceCaptureFrame,
    DeviceCaptureSource, DeviceStreamFailure, HostCameraAuthorization, SelectedDevice,
    UsbmuxObservation,
};

const TEXT: usize = 512;
const NAME: usize = 256;
const UID: usize = 512;
const MAX_DEVICE_SOURCES: usize = 64;
const MAX_PNG_BYTES: usize = 64 * 1024 * 1024;

const CAMERA_AUTHORIZED: i32 = 0;
const CAMERA_DENIED: i32 = 1;
const CAMERA_RESTRICTED: i32 = 2;
const CAMERA_NOT_DETERMINED: i32 = 3;

const OBSERVATION_INVENTORY: i32 = 0;

const STREAM_OK: i32 = 0;
const STREAM_OPEN_FAILED: i32 = 1;
const STREAM_INPUT_REFUSED: i32 = 2;
const STREAM_OUTPUT_REFUSED: i32 = 3;
const STREAM_START_FAILED: i32 = 4;
const STREAM_FRAME_TIMEOUT: i32 = 5;
const STREAM_EXPLICIT_LOCKED: i32 = 6;
const STREAM_EXPLICIT_NOT_TRUSTED: i32 = 7;
const STREAM_ENCODE_FAILED: i32 = 8;

#[repr(C)]
#[derive(Clone, Copy)]
struct RawSource {
    name: [core::ffi::c_char; NAME],
    uid: [core::ffi::c_char; UID],
}

#[repr(C)]
struct RawEvidence {
    camera_authorization: i32,
    usbmux_status: i32,
    connected_devices: usize,
    paired_devices: usize,
    usbmux_message: [core::ffi::c_char; TEXT],
    dal_status: i32,
    sources: *mut RawSource,
    source_count: usize,
    dal_message: [core::ffi::c_char; TEXT],
}

#[repr(C)]
struct RawFrame {
    png: *mut u8,
    png_len: usize,
    width: u32,
    height: u32,
    status: i32,
    message: [core::ffi::c_char; TEXT],
}

unsafe extern "C" {
    fn agenterm_device_capture_observe(out: *mut RawEvidence);
    fn agenterm_device_capture_evidence_free(out: *mut RawEvidence);
    fn agenterm_device_capture_selected(
        uid: *const core::ffi::c_char,
        timeout_ms: u64,
        out: *mut RawFrame,
    );
    fn agenterm_device_capture_frame_free(out: *mut RawFrame);
}

struct EvidenceOwner(RawEvidence);

impl Drop for EvidenceOwner {
    fn drop(&mut self) {
        // SAFETY: the native observe call initialized this exact value and its
        // free function accepts both allocated and empty source storage.
        unsafe { agenterm_device_capture_evidence_free(&mut self.0) };
    }
}

struct FrameOwner(RawFrame);

impl Drop for FrameOwner {
    fn drop(&mut self) {
        // SAFETY: the native capture call initialized this exact value and its
        // free function accepts both successful and failed frames.
        unsafe { agenterm_device_capture_frame_free(&mut self.0) };
    }
}

/// Native macOS implementation. The value owns no persistent native state and
/// may be constructed on any thread; each call owns and tears down its session.
#[derive(Clone, Copy, Debug, Default)]
pub struct MacosDeviceCaptureBackend;

impl DeviceCaptureBackend for MacosDeviceCaptureBackend {
    fn observe(&self) -> DeviceCaptureEvidence {
        let mut raw = core::mem::MaybeUninit::<RawEvidence>::uninit();
        // SAFETY: native observe promises to initialize every field for a
        // non-null output pointer, including on subsystem failures.
        unsafe { agenterm_device_capture_observe(raw.as_mut_ptr()) };
        // SAFETY: established by the native function contract above.
        let owner = EvidenceOwner(unsafe { raw.assume_init() });
        decode_evidence(&owner.0)
    }

    fn capture_selected(
        &self,
        selected: &SelectedDevice,
        timeout: Duration,
    ) -> Result<DeviceCaptureFrame, DeviceStreamFailure> {
        let uid = CString::new(selected.source().uid.as_bytes()).map_err(|_| {
            DeviceStreamFailure::OpenFailed {
                message: "the selected DAL source uid contains a NUL byte".to_owned(),
            }
        })?;
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let mut raw = core::mem::MaybeUninit::<RawFrame>::uninit();
        // SAFETY: `uid` remains alive through the call, the output pointer is
        // valid, and native capture initializes it on every return path.
        unsafe { agenterm_device_capture_selected(uid.as_ptr(), timeout_ms, raw.as_mut_ptr()) };
        // SAFETY: established by the native function contract above.
        let owner = FrameOwner(unsafe { raw.assume_init() });
        decode_frame(&owner.0, selected)
    }
}

fn decode_evidence(raw: &RawEvidence) -> DeviceCaptureEvidence {
    let host_camera_authorization = match raw.camera_authorization {
        CAMERA_AUTHORIZED => HostCameraAuthorization::Authorized,
        CAMERA_DENIED => HostCameraAuthorization::Denied,
        CAMERA_RESTRICTED => HostCameraAuthorization::Restricted,
        CAMERA_NOT_DETERMINED => HostCameraAuthorization::NotDetermined,
        _ => HostCameraAuthorization::NotDetermined,
    };

    let usbmux = if raw.usbmux_status == OBSERVATION_INVENTORY {
        UsbmuxObservation::Inventory {
            connected_devices: raw.connected_devices,
            paired_devices: raw.paired_devices,
        }
    } else {
        UsbmuxObservation::Unavailable {
            message: native_message(&raw.usbmux_message, "usbmux observation failed"),
        }
    };

    let dal = if raw.dal_status != OBSERVATION_INVENTORY {
        DalObservation::Failed {
            message: native_message(&raw.dal_message, "DAL device inventory failed"),
        }
    } else if raw.source_count == 0 {
        // This is a successful empty inventory. In particular, do not infer
        // device lock or trust from a zero count.
        DalObservation::Inventory {
            sources: Vec::new(),
        }
    } else if raw.source_count > MAX_DEVICE_SOURCES || raw.sources.is_null() {
        DalObservation::Failed {
            message: "native DAL inventory exceeded its validated bounds".to_owned(),
        }
    } else {
        // SAFETY: count is bounded above and the native allocation holds that
        // many initialized fixed-size RawSource values until EvidenceOwner drops.
        let sources = unsafe { core::slice::from_raw_parts(raw.sources, raw.source_count) }
            .iter()
            .map(|source| DeviceCaptureSource {
                name: c_text(&source.name),
                uid: c_text(&source.uid),
            })
            .collect();
        DalObservation::Inventory { sources }
    };

    DeviceCaptureEvidence {
        host_camera_authorization,
        usbmux,
        dal,
    }
}

fn decode_frame(
    raw: &RawFrame,
    selected: &SelectedDevice,
) -> Result<DeviceCaptureFrame, DeviceStreamFailure> {
    let message = || native_message(&raw.message, "native device capture failed");
    match raw.status {
        STREAM_OK => {
            if raw.png.is_null()
                || raw.png_len == 0
                || raw.png_len > MAX_PNG_BYTES
                || raw.width == 0
                || raw.height == 0
            {
                return Err(DeviceStreamFailure::EncodeFailed {
                    message: "native capture returned an invalid or oversized PNG frame".to_owned(),
                });
            }
            // SAFETY: the native allocation remains owned by FrameOwner during
            // this bounded copy and is at least `png_len` bytes by ABI contract.
            let encoded = unsafe { core::slice::from_raw_parts(raw.png, raw.png_len) }.to_vec();
            Ok(DeviceCaptureFrame {
                encoded,
                width: raw.width,
                height: raw.height,
                source: selected.source().clone(),
            })
        }
        STREAM_OPEN_FAILED => Err(DeviceStreamFailure::OpenFailed { message: message() }),
        STREAM_INPUT_REFUSED => Err(DeviceStreamFailure::InputRefused { message: message() }),
        STREAM_OUTPUT_REFUSED => Err(DeviceStreamFailure::OutputRefused { message: message() }),
        STREAM_START_FAILED => Err(DeviceStreamFailure::StartFailed { message: message() }),
        STREAM_FRAME_TIMEOUT => Err(DeviceStreamFailure::FrameTimeout),
        // These are accepted only as explicit native statuses. The present
        // Objective-C adapter emits neither from inventory absence or timeout.
        STREAM_EXPLICIT_LOCKED => Err(DeviceStreamFailure::ExplicitDeviceLocked),
        STREAM_EXPLICIT_NOT_TRUSTED => Err(DeviceStreamFailure::ExplicitDeviceNotTrusted),
        STREAM_ENCODE_FAILED => Err(DeviceStreamFailure::EncodeFailed { message: message() }),
        _ => Err(DeviceStreamFailure::EncodeFailed {
            message: "native capture returned an unknown status".to_owned(),
        }),
    }
}

fn native_message(slot: &[core::ffi::c_char], fallback: &str) -> String {
    let value = c_text(slot);
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}

fn c_text(slot: &[core::ffi::c_char]) -> String {
    let bytes: Vec<u8> = slot
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_capture::{DeviceCaptureErrorKind, select_device};

    fn empty_raw_evidence() -> RawEvidence {
        RawEvidence {
            camera_authorization: CAMERA_AUTHORIZED,
            usbmux_status: OBSERVATION_INVENTORY,
            connected_devices: 0,
            paired_devices: 0,
            usbmux_message: [0; TEXT],
            dal_status: OBSERVATION_INVENTORY,
            sources: core::ptr::null_mut(),
            source_count: 0,
            dal_message: [0; TEXT],
        }
    }

    #[test]
    fn successful_empty_native_inventory_does_not_invent_device_state() {
        let evidence = decode_evidence(&empty_raw_evidence());
        assert_eq!(evidence.dal, DalObservation::Inventory { sources: vec![] });
        let error = select_device(&evidence, None).expect_err("no attached device");
        assert_eq!(error.kind, DeviceCaptureErrorKind::DeviceNotConnected);
        assert_ne!(error.kind, DeviceCaptureErrorKind::DeviceLocked);
        assert_ne!(error.kind, DeviceCaptureErrorKind::DeviceNotTrusted);
    }

    #[test]
    fn undecided_tcc_still_carries_both_other_observations() {
        let mut raw = empty_raw_evidence();
        raw.camera_authorization = CAMERA_NOT_DETERMINED;
        raw.connected_devices = 1;
        raw.paired_devices = 1;
        let evidence = decode_evidence(&raw);
        assert_eq!(
            evidence.host_camera_authorization,
            HostCameraAuthorization::NotDetermined
        );
        assert_eq!(
            evidence.usbmux,
            UsbmuxObservation::Inventory {
                connected_devices: 1,
                paired_devices: 1
            }
        );
        assert_eq!(evidence.dal, DalObservation::Inventory { sources: vec![] });
    }

    #[test]
    fn timeout_status_remains_timeout_without_lock_or_trust_guess() {
        let raw = RawFrame {
            png: core::ptr::null_mut(),
            png_len: 0,
            width: 0,
            height: 0,
            status: STREAM_FRAME_TIMEOUT,
            message: [0; TEXT],
        };
        let selected = select_device(
            &DeviceCaptureEvidence {
                host_camera_authorization: HostCameraAuthorization::Authorized,
                usbmux: UsbmuxObservation::Inventory {
                    connected_devices: 1,
                    paired_devices: 1,
                },
                dal: DalObservation::Inventory {
                    sources: vec![DeviceCaptureSource {
                        name: "Fixture Phone".to_owned(),
                        uid: "fixture-device".to_owned(),
                    }],
                },
            },
            None,
        )
        .expect("fixture source selected");
        assert_eq!(
            decode_frame(&raw, &selected).expect_err("fixture timeout"),
            DeviceStreamFailure::FrameTimeout
        );
    }

    #[test]
    fn oversized_success_is_rejected_before_reading_native_bytes() {
        let raw = RawFrame {
            png: core::ptr::dangling_mut(),
            png_len: MAX_PNG_BYTES + 1,
            width: 1,
            height: 1,
            status: STREAM_OK,
            message: [0; TEXT],
        };
        let selected = select_device(
            &DeviceCaptureEvidence {
                host_camera_authorization: HostCameraAuthorization::Authorized,
                usbmux: UsbmuxObservation::Inventory {
                    connected_devices: 1,
                    paired_devices: 1,
                },
                dal: DalObservation::Inventory {
                    sources: vec![DeviceCaptureSource {
                        name: "Fixture Phone".to_owned(),
                        uid: "fixture-device".to_owned(),
                    }],
                },
            },
            None,
        )
        .expect("fixture source selected");
        assert!(matches!(
            decode_frame(&raw, &selected),
            Err(DeviceStreamFailure::EncodeFailed { .. })
        ));
    }
}
