//! macOS input: only the **read-only pointer position** is wired
//! (`CGEventCreate(NULL)` + `CGEventGetLocation`, no event is posted).
//! Injection (move / click / type / keys) stays typed `Unsupported`, and the
//! capability status says so: a caller that asks "can this host inject
//! input?" must not read "yes" from a host that can only observe the
//! pointer. `agt_input_pointer_position` calls `pointer_position` directly
//! and maps the typed `Unsupported` of other hosts, so this read does not
//! depend on the capability being `Available`.

#![cfg(target_os = "macos")]

use std::ffi::c_void;

use crate::CapabilityStatus;
use crate::contract::input_inject::{InputInjectError, PointerButton, PointerPosition};

type CfTypeRef = *const c_void;
type CgEventRef = *const c_void;
type CgEventSourceRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CgPoint {
    x: f64,
    y: f64,
}

// One `#[link]` per framework on a single extern block; clippy reads the
// repeated attribute name as a copy-paste slip (same false positive as
// foreign_windows.rs).
#[allow(clippy::duplicated_attributes)]
#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: CgEventSourceRef) -> CgEventRef;
    fn CGEventGetLocation(event: CgEventRef) -> CgPoint;
    fn CFRelease(cf: CfTypeRef);
}

const INJECT_REASON: &str =
    "input injection is not wired on macOS (pointer-position is read-only and available)";

pub(crate) fn capability_status() -> CapabilityStatus {
    CapabilityStatus::Unsupported {
        reason: INJECT_REASON.into(),
    }
}

pub(crate) fn pointer_move(_position: PointerPosition) -> Result<(), InputInjectError> {
    Err(InputInjectError::Unsupported {
        reason: INJECT_REASON.into(),
    })
}

/// The real pointer's location in the global Quartz space (top-origin of
/// the main display, the same space `CGWindowListCopyWindowInfo` bounds
/// use). Creating an event from a null source only *samples* the current
/// state; nothing is posted, so this read can never move the pointer.
pub(crate) fn pointer_position() -> Result<PointerPosition, InputInjectError> {
    let event = unsafe { CGEventCreate(std::ptr::null()) };
    if event.is_null() {
        return Err(InputInjectError::Failed {
            code: "pointer_position_failed".into(),
            message: "CGEventCreate(NULL) returned null".to_owned(),
        });
    }
    let point = unsafe { CGEventGetLocation(event) };
    unsafe { CFRelease(event) };
    if !point.x.is_finite() || !point.y.is_finite() {
        return Err(InputInjectError::Failed {
            code: "pointer_position_failed".into(),
            message: "CGEventGetLocation returned a non-finite point".to_owned(),
        });
    }
    Ok(PointerPosition {
        x: point.x.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32,
        y: point.y.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32,
    })
}

pub(crate) fn pointer_click(
    _position: PointerPosition,
    _button: PointerButton,
    _clicks: u32,
) -> Result<(), InputInjectError> {
    Err(InputInjectError::Unsupported {
        reason: INJECT_REASON.into(),
    })
}

pub(crate) fn type_text(_text: &str) -> Result<(), InputInjectError> {
    Err(InputInjectError::Unsupported {
        reason: INJECT_REASON.into(),
    })
}

pub(crate) fn send_keys(_shortcut: &str) -> Result<(), InputInjectError> {
    Err(InputInjectError::Unsupported {
        reason: INJECT_REASON.into(),
    })
}
