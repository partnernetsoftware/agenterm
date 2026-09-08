//! Input injection facade (portable entry point).

use crate::CapabilityStatus;
pub use crate::contract::input_inject::{
    InputInjectError, MAX_POINTER_DRAG_STEPS, MAX_POINTER_SCROLL_DETENTS, PointerButton,
    PointerPosition,
};

pub fn capability_status() -> CapabilityStatus {
    crate::selected::input_inject::capability_status()
}

pub fn pointer_move(position: PointerPosition) -> Result<(), InputInjectError> {
    crate::selected::input_inject::pointer_move(position)
}

/// Reads the current pointer position in absolute target-session screen
/// coordinates without injecting an event.
pub fn pointer_position() -> Result<PointerPosition, InputInjectError> {
    crate::selected::input_inject::pointer_position()
}

/// Post signed wheel detents at the real pointer's current desktop location
/// without moving the pointer. Positive `dx` scrolls left and positive `dy`
/// scrolls up; negative values scroll right/down. At least one axis must be
/// non-zero and each magnitude is bounded
/// by [`MAX_POINTER_SCROLL_DETENTS`].
pub fn pointer_scroll(dx: i32, dy: i32) -> Result<(), InputInjectError> {
    crate::contract::input_inject::validate_pointer_scroll(dx, dy)?;
    crate::selected::input_inject::pointer_scroll(dx, dy)
}

/// Post signed wheel detents at absolute screen `position`. Linux restores
/// the physical pointer after delivery; other hosts answer typed
/// `Unsupported`.
pub fn pointer_scroll_at(
    position: PointerPosition,
    dx: i32,
    dy: i32,
) -> Result<(), InputInjectError> {
    crate::contract::input_inject::validate_pointer_scroll(dx, dy)?;
    crate::selected::input_inject::pointer_scroll_at(position, dx, dy)
}

pub fn pointer_click(
    position: PointerPosition,
    button: PointerButton,
    clicks: u32,
) -> Result<(), InputInjectError> {
    crate::selected::input_inject::pointer_click(position, button, clicks)
}

/// Press `button` at `from`, deliver `steps` intermediate drag moves toward
/// `to`, and release at `to`. `steps` must be `1..=MAX_POINTER_DRAG_STEPS`;
/// an adapter that implements the drag refuses anything else before posting
/// a single event, so a rejected request leaves the pointer untouched.
pub fn pointer_drag(
    from: PointerPosition,
    to: PointerPosition,
    button: PointerButton,
    steps: u32,
) -> Result<(), InputInjectError> {
    crate::selected::input_inject::pointer_drag(from, to, button, steps)
}

/// Types `text` into the focused control using Unicode key events.
pub fn type_text(text: &str) -> Result<(), InputInjectError> {
    crate::selected::input_inject::type_text(text)
}

/// Sends a hotkey such as `ctrl+s`, `alt+f4` or `enter`.
pub fn send_keys(shortcut: &str) -> Result<(), InputInjectError> {
    crate::selected::input_inject::send_keys(shortcut)
}
