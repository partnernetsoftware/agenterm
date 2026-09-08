//! Selected keyboard-layout observation facade.

pub use crate::contract::keyboard_layout::{
    KeyboardLayoutObservation, KeyboardLayoutObserveResult, KeyboardLayoutObserveUnsupported,
};
use crate::selected;

/// Read the active XKB layout on the selected host when the mechanism exists.
#[must_use]
pub fn observe_host() -> KeyboardLayoutObserveResult {
    selected::keyboard_layout::observe()
}
