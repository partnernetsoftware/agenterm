//! macOS input injection through Quartz events.
//!
//! Two facts about this platform shape everything here, and both were
//! measured rather than assumed:
//!
//! 1. **Keys cannot reach an application that is not active.** An accessory
//!    app whose window is ordered front reports `keyWindow = no`, and key
//!    events posted to its pid with `CGEventPostToPid` never arrive at its
//!    `sendEvent:` at all.
//! 2. **Mouse events posted to a pid do arrive, but carry no window.** The
//!    same probe app sees `LeftMouseDown` / `LeftMouseUp` in `sendEvent:`
//!    with the real pointer never moving -- and its button never fires,
//!    because the event has no window for AppKit to route it through
//!    (setting `kCGMouseEventWindowUnderMousePointer` does not change
//!    that).
//!
//! So there is no window-local injection on macOS. What is left is the
//! **global** path: `CGEventPost` on the HID tap, which moves the real
//! cursor and goes to whatever is frontmost. That is a real capability and
//! agents need it (it is what `--to desktop` means), but it is the
//! opposite of the background invariant, so nothing in this file is ever
//! reached by the semantic verbs: `click --node`, `invoke`, `focus` and
//! the rest go through the accessibility tree and never come here.
//!
//! `pointer_position` remains a pure read: creating an event from a null
//! source only samples the current state, and nothing is posted.

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
    fn CGEventCreateMouseEvent(
        source: CgEventSourceRef,
        mouse_type: u32,
        position: CgPoint,
        button: u32,
    ) -> CgEventRef;
    fn CGEventCreateKeyboardEvent(
        source: CgEventSourceRef,
        virtual_key: u16,
        key_down: bool,
    ) -> CgEventRef;
    fn CGEventKeyboardSetUnicodeString(event: CgEventRef, length: usize, string: *const u16);
    fn CGEventSetFlags(event: CgEventRef, flags: u64);
    fn CGEventSetIntegerValueField(event: CgEventRef, field: u32, value: i64);
    fn CGEventPost(tap: u32, event: CgEventRef);
    fn CFRelease(cf: CfTypeRef);
}

/// `kCGHIDEventTap`: the injection point a physical device would use, so
/// the whole system sees the event exactly as it would a real one.
const CG_HID_EVENT_TAP: u32 = 0;
const CG_EVENT_MOUSE_MOVED: u32 = 5;
const CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
const CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
const CG_MOUSE_BUTTON_LEFT: u32 = 0;
const CG_MOUSE_BUTTON_RIGHT: u32 = 1;
const CG_MOUSE_BUTTON_CENTER: u32 = 2;
/// `kCGMouseEventClickState`: 1 for a single click, 2 for a double, and so
/// on. A repeated click that leaves this at 1 is not a double click to the
/// application receiving it.
const CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
/// `CGEventFlags` for the four chord modifiers.
const CG_FLAG_SHIFT: u64 = 0x0002_0000;
const CG_FLAG_CONTROL: u64 = 0x0004_0000;
const CG_FLAG_OPTION: u64 = 0x0008_0000;
const CG_FLAG_COMMAND: u64 = 0x0010_0000;
/// Largest `type_text` payload accepted, mirroring the other adapters.
const MAX_TYPE_TEXT_UTF16: usize = 16 * 1024;
/// Largest number of clicks one call may deliver.
const MAX_CLICKS: u32 = 8;

/// A `CGEventRef` released on drop, so an early return cannot leak it.
struct OwnedEvent(CgEventRef);

impl OwnedEvent {
    fn new(event: CgEventRef, what: &'static str) -> Result<Self, InputInjectError> {
        if event.is_null() {
            return Err(InputInjectError::Failed {
                code: "event_create_failed".into(),
                message: format!("{what} returned null"),
            });
        }
        Ok(Self(event))
    }

    fn as_ptr(&self) -> CgEventRef {
        self.0
    }
}

impl Drop for OwnedEvent {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
            self.0 = std::ptr::null();
        }
    }
}

pub(crate) fn capability_status() -> CapabilityStatus {
    CapabilityStatus::Available
}

/// Move the real cursor. This is the global path by definition: macOS has
/// no window-local pointer injection, so the caller must have asked for a
/// desktop-scoped move (`--to desktop`) to get here.
pub(crate) fn pointer_move(position: PointerPosition) -> Result<(), InputInjectError> {
    let event = OwnedEvent::new(
        unsafe {
            CGEventCreateMouseEvent(
                std::ptr::null(),
                CG_EVENT_MOUSE_MOVED,
                point_of(position),
                CG_MOUSE_BUTTON_LEFT,
            )
        },
        "CGEventCreateMouseEvent(mouseMoved)",
    )?;
    unsafe { CGEventPost(CG_HID_EVENT_TAP, event.as_ptr()) };
    Ok(())
}

fn point_of(position: PointerPosition) -> CgPoint {
    CgPoint {
        x: f64::from(position.x),
        y: f64::from(position.y),
    }
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

/// Press and release at `position`, `clicks` times. The click state field
/// carries the repeat count, so a double click reads as a double click to
/// the application and not as two unrelated singles.
pub(crate) fn pointer_click(
    position: PointerPosition,
    button: PointerButton,
    clicks: u32,
) -> Result<(), InputInjectError> {
    if clicks == 0 || clicks > MAX_CLICKS {
        return Err(InputInjectError::Failed {
            code: "invalid_input".into(),
            message: format!("clicks must be 1..={MAX_CLICKS}, got {clicks}"),
        });
    }
    let (down_type, up_type, cg_button) = match button {
        PointerButton::Left => (
            CG_EVENT_LEFT_MOUSE_DOWN,
            CG_EVENT_LEFT_MOUSE_UP,
            CG_MOUSE_BUTTON_LEFT,
        ),
        PointerButton::Right => (
            CG_EVENT_RIGHT_MOUSE_DOWN,
            CG_EVENT_RIGHT_MOUSE_UP,
            CG_MOUSE_BUTTON_RIGHT,
        ),
        PointerButton::Middle => (
            CG_EVENT_OTHER_MOUSE_DOWN,
            CG_EVENT_OTHER_MOUSE_UP,
            CG_MOUSE_BUTTON_CENTER,
        ),
    };
    let point = point_of(position);
    for click in 1..=clicks {
        for mouse_type in [down_type, up_type] {
            let event = OwnedEvent::new(
                unsafe { CGEventCreateMouseEvent(std::ptr::null(), mouse_type, point, cg_button) },
                "CGEventCreateMouseEvent",
            )?;
            unsafe {
                CGEventSetIntegerValueField(
                    event.as_ptr(),
                    CG_MOUSE_EVENT_CLICK_STATE,
                    i64::from(click),
                );
                CGEventPost(CG_HID_EVENT_TAP, event.as_ptr());
            }
        }
    }
    Ok(())
}

/// Type Unicode text by attaching it to key events rather than by looking
/// up key codes: the text arrives as written whatever the user's layout is,
/// which is the whole point of a text verb as opposed to a chord verb.
pub(crate) fn type_text(text: &str) -> Result<(), InputInjectError> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() > MAX_TYPE_TEXT_UTF16 {
        return Err(InputInjectError::Failed {
            code: "text_limit".into(),
            message: format!("text exceeds {MAX_TYPE_TEXT_UTF16} UTF-16 units"),
        });
    }
    if units.is_empty() {
        return Ok(());
    }
    // One chunk per key-down/up pair. A very long string is split so no
    // single event carries more than the field is meant to hold.
    for chunk in units.chunks(20) {
        for down in [true, false] {
            let event = OwnedEvent::new(
                unsafe { CGEventCreateKeyboardEvent(std::ptr::null(), 0, down) },
                "CGEventCreateKeyboardEvent",
            )?;
            unsafe {
                CGEventKeyboardSetUnicodeString(event.as_ptr(), chunk.len(), chunk.as_ptr());
                CGEventPost(CG_HID_EVENT_TAP, event.as_ptr());
            }
        }
    }
    Ok(())
}

/// One `modifier+key` chord to whatever is frontmost. The key codes are
/// ANSI *physical* positions, so they do not shift with the user's layout;
/// a character with no physical key of its own is refused rather than
/// guessed, the same rule the Windows adapter follows.
pub(crate) fn send_keys(shortcut: &str) -> Result<(), InputInjectError> {
    let (flags, key_code) = parse_chord(shortcut)?;
    for down in [true, false] {
        let event = OwnedEvent::new(
            unsafe { CGEventCreateKeyboardEvent(std::ptr::null(), key_code, down) },
            "CGEventCreateKeyboardEvent",
        )?;
        unsafe {
            CGEventSetFlags(event.as_ptr(), flags);
            CGEventPost(CG_HID_EVENT_TAP, event.as_ptr());
        }
    }
    Ok(())
}

fn parse_chord(shortcut: &str) -> Result<(u64, u16), InputInjectError> {
    let bad = |code: &'static str, detail: String| InputInjectError::Failed {
        code: code.into(),
        message: detail,
    };
    let parts: Vec<&str> = shortcut.split('+').map(str::trim).collect();
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        return Err(bad(
            "bad_shortcut",
            format!("cannot parse shortcut {shortcut:?}"),
        ));
    }
    let mut flags = 0u64;
    for modifier in &parts[..parts.len() - 1] {
        flags |= match modifier.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "win" | "super" => CG_FLAG_COMMAND,
            "ctrl" | "control" => CG_FLAG_CONTROL,
            "alt" | "option" | "opt" => CG_FLAG_OPTION,
            "shift" => CG_FLAG_SHIFT,
            other => {
                return Err(bad(
                    "unknown_modifier",
                    format!("unknown modifier {other:?}"),
                ));
            }
        };
    }
    let key = parts[parts.len() - 1];
    let lower = key.to_ascii_lowercase();
    if let Some(code) = named_key_code(&lower) {
        return Ok((flags, code));
    }
    let mut chars = key.chars();
    let (Some(single), None) = (chars.next(), chars.next()) else {
        return Err(bad("unknown_key", format!("unknown key {key:?}")));
    };
    match character_key_code(single) {
        Some(code) => Ok((flags, code)),
        None => Err(bad(
            "unknown_key",
            format!(
                "character {single:?} has no layout-independent physical key; use type_text for text"
            ),
        )),
    }
}

/// ANSI physical key positions.
fn named_key_code(lower: &str) -> Option<u16> {
    Some(match lower {
        "enter" | "return" => 0x24,
        "tab" => 0x30,
        "space" => 0x31,
        "backspace" => 0x33,
        "esc" | "escape" => 0x35,
        "delete" | "forward-delete" => 0x75,
        "home" => 0x73,
        "end" => 0x77,
        "page-up" | "pageup" => 0x74,
        "page-down" | "pagedown" => 0x79,
        "left" => 0x7b,
        "right" => 0x7c,
        "down" => 0x7d,
        "up" => 0x7e,
        "f1" => 0x7a,
        "f2" => 0x78,
        "f3" => 0x63,
        "f4" => 0x76,
        "f5" => 0x60,
        "f6" => 0x61,
        "f7" => 0x62,
        "f8" => 0x64,
        "f9" => 0x65,
        "f10" => 0x6d,
        "f11" => 0x67,
        "f12" => 0x6f,
        _ => return None,
    })
}

fn character_key_code(character: char) -> Option<u16> {
    Some(match character.to_ascii_lowercase() {
        'a' => 0x00,
        'b' => 0x0b,
        'c' => 0x08,
        'd' => 0x02,
        'e' => 0x0e,
        'f' => 0x03,
        'g' => 0x05,
        'h' => 0x04,
        'i' => 0x22,
        'j' => 0x26,
        'k' => 0x28,
        'l' => 0x25,
        'm' => 0x2e,
        'n' => 0x2d,
        'o' => 0x1f,
        'p' => 0x23,
        'q' => 0x0c,
        'r' => 0x0f,
        's' => 0x01,
        't' => 0x11,
        'u' => 0x20,
        'v' => 0x09,
        'w' => 0x0d,
        'x' => 0x07,
        'y' => 0x10,
        'z' => 0x06,
        '0' => 0x1d,
        '1' => 0x12,
        '2' => 0x13,
        '3' => 0x14,
        '4' => 0x15,
        '5' => 0x17,
        '6' => 0x16,
        '7' => 0x1a,
        '8' => 0x1c,
        '9' => 0x19,
        '-' => 0x1b,
        '=' => 0x18,
        '[' => 0x21,
        ']' => 0x1e,
        '\\' => 0x2a,
        ';' => 0x29,
        '\'' => 0x27,
        ',' => 0x2b,
        '.' => 0x2f,
        '/' => 0x2c,
        '`' => 0x32,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse_to_physical_keys_and_flags() {
        assert_eq!(parse_chord("enter").unwrap(), (0, 0x24));
        assert_eq!(parse_chord("cmd+a").unwrap(), (CG_FLAG_COMMAND, 0x00));
        assert_eq!(
            parse_chord("ctrl+shift+z").unwrap(),
            (CG_FLAG_CONTROL | CG_FLAG_SHIFT, 0x06)
        );
        // A character with no physical key of its own is refused, not
        // guessed: typing it belongs in type_text, which carries Unicode.
        assert!(parse_chord("é").is_err());
        assert!(parse_chord("ctrl+").is_err());
        assert!(parse_chord("hyper+a").is_err());
    }

    #[test]
    fn a_click_count_outside_the_bound_is_refused_before_any_event() {
        let at = PointerPosition { x: 10, y: 10 };
        assert!(pointer_click(at, PointerButton::Left, 0).is_err());
        assert!(pointer_click(at, PointerButton::Left, MAX_CLICKS + 1).is_err());
    }
}
