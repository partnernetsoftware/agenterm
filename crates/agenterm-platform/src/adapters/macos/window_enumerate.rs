//! macOS top-level window enumeration via CGWindowList.

#![cfg(target_os = "macos")]

use crate::CapabilityStatus;
use crate::contract::window_enumerate::{
    ScreenInfo, WindowBounds, WindowEnumerateError, WindowInfo, WindowStacking,
    stacking_from_front_to_back,
};

use crate::selected::macos_foreign_windows as foreign_windows;

pub(crate) fn capability_status() -> CapabilityStatus {
    foreign_windows::capability_status()
}

pub(crate) fn enumerate_top_level() -> Result<Vec<WindowInfo>, WindowEnumerateError> {
    foreign_windows::enumerate_top_level()
}

pub(crate) fn list_screens() -> Result<Vec<ScreenInfo>, WindowEnumerateError> {
    foreign_windows::list_screens()
}

/// `CGWindowListCopyWindowInfo` returns on-screen windows **front to
/// back**, so the enumeration order is the stacking order and the index is
/// the z-index. No extra system call is needed.
pub(crate) fn stacking() -> Result<Vec<WindowStacking>, WindowEnumerateError> {
    let ordered: Vec<(isize, WindowBounds)> = foreign_windows::enumerate_top_level()?
        .into_iter()
        .map(|window| (window.handle, window.bounds))
        .collect();
    Ok(stacking_from_front_to_back(&ordered))
}
