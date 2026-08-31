//! Window enumeration facade (portable entry point).
//!
//! Consumers (e.g. `agenterm-cu`) call these functions; the OS-level
//! mechanism lives in the platform adapter selected by `crate::selected`.

use crate::CapabilityStatus;
pub use crate::contract::window_enumerate::{
    ScreenInfo, WindowBounds, WindowEnumerateError, WindowInfo, WindowStacking,
};

pub fn capability_status() -> CapabilityStatus {
    crate::selected::window_enumerate::capability_status()
}

pub fn enumerate_top_level() -> Result<Vec<WindowInfo>, WindowEnumerateError> {
    crate::selected::window_enumerate::enumerate_top_level()
}

pub fn list_screens() -> Result<Vec<ScreenInfo>, WindowEnumerateError> {
    crate::selected::window_enumerate::list_screens()
}

/// Front-to-back stacking for the same windows `enumerate_top_level`
/// returns, with how much of each the windows in front cover.
///
/// A backend that cannot report a real stacking order answers
/// `Unsupported` instead of passing its enumeration order off as one --
/// creation order is indistinguishable from stacking order right up to the
/// moment it is wrong.
pub fn stacking() -> Result<Vec<WindowStacking>, WindowEnumerateError> {
    crate::selected::window_enumerate::stacking()
}
