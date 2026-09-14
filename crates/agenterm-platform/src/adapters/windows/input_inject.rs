//! Windows input injection (user32 FFI): pointer + Unicode keyboard.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6,
    VK_F7, VK_F8, VK_F9, VK_F10, VK_F11, VK_F12, VK_LEFT, VK_LWIN, VK_MENU, VK_RETURN, VK_RIGHT,
    VK_SHIFT, VK_SPACE, VK_TAB, VK_UP, mouse_event,
};
use windows_sys::Win32::{
    Foundation::POINT,
    UI::WindowsAndMessaging::{
        GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN, SetCursorPos,
    },
};

use crate::CapabilityStatus;
use crate::contract::input_inject::{
    InputInjectError, MAX_POINTER_DRAG_STEPS, PointerButton, PointerPosition,
    validate_pointer_scroll,
};

/// One Windows wheel detent. `windows-sys` does not expose the WinUser.h
/// `WHEEL_DELTA` macro as a Rust constant.
const WHEEL_DELTA: i32 = 120;

pub(crate) fn capability_status() -> CapabilityStatus {
    CapabilityStatus::Available
}

pub(crate) fn pointer_move(position: PointerPosition) -> Result<(), InputInjectError> {
    unsafe {
        if SetCursorPos(position.x, position.y) == 0 {
            return Err(InputInjectError::failed(
                "set_cursor_failed",
                "SetCursorPos returned 0",
            ));
        }
    }
    Ok(())
}

pub(crate) fn pointer_position() -> Result<PointerPosition, InputInjectError> {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return Err(InputInjectError::failed(
            "get_cursor_failed",
            "GetCursorPos returned 0",
        ));
    }
    Ok(PointerPosition {
        x: point.x,
        y: point.y,
    })
}

pub(crate) fn pointer_grab() -> Result<(PointerPosition, u8, u8), InputInjectError> {
    Err(InputInjectError::Unsupported {
        reason:
            "pointer-grab is not mapped on Windows; use pointer-position to observe coordinates"
                .into(),
    })
}

pub(crate) fn pointer_ungrab() -> Result<(PointerPosition, u8), InputInjectError> {
    Err(InputInjectError::Unsupported {
        reason:
            "pointer-ungrab is not mapped on Windows; use pointer-position to observe coordinates"
                .into(),
    })
}

pub(crate) fn pointer_scroll(dx: i32, dy: i32) -> Result<(), InputInjectError> {
    validate_pointer_scroll(dx, dy)?;
    send_batch(&wheel_inputs(dx, dy))
}

pub(crate) fn pointer_scroll_at(
    _position: PointerPosition,
    _dx: i32,
    _dy: i32,
) -> Result<(), InputInjectError> {
    Err(InputInjectError::Unsupported {
        reason: "pointer scroll at position is not wired on Windows yet".into(),
    })
}

fn wheel_inputs(dx: i32, dy: i32) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(2);
    if dy != 0 {
        inputs.push(mouse_wheel_input(MOUSEEVENTF_WHEEL, dy));
    }
    if dx != 0 {
        // Windows defines a positive horizontal wheel delta as right. The
        // portable contract follows the established MCU/Quartz convention:
        // positive dx is left, so invert only at this adapter boundary.
        inputs.push(mouse_wheel_input(MOUSEEVENTF_HWHEEL, -dx));
    }
    inputs
}

fn mouse_wheel_input(flags: u32, detents: i32) -> INPUT {
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_MOUSE;
    input.Anonymous.mi = MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: detents.saturating_mul(WHEEL_DELTA) as u32,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    input
}

pub(crate) fn pointer_click(
    position: PointerPosition,
    button: PointerButton,
    clicks: u32,
) -> Result<(), InputInjectError> {
    let flags = match button {
        PointerButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        PointerButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        PointerButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    unsafe {
        if SetCursorPos(position.x, position.y) == 0 {
            return Err(InputInjectError::failed(
                "set_cursor_failed",
                "SetCursorPos returned 0",
            ));
        }
        for _ in 0..clicks.max(1) {
            mouse_event(flags.0, 0, 0, 0, 0);
            mouse_event(flags.1, 0, 0, 0, 0);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VirtualDesktop {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn virtual_desktop() -> Result<VirtualDesktop, InputInjectError> {
    let desktop = VirtualDesktop {
        x: unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) },
        y: unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) },
        width: unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) },
        height: unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) },
    };
    if desktop.width <= 0 || desktop.height <= 0 {
        return Err(InputInjectError::failed(
            "virtual_desktop_unavailable",
            format!(
                "GetSystemMetrics returned invalid virtual desktop {}x{}",
                desktop.width, desktop.height
            ),
        ));
    }
    Ok(desktop)
}

fn normalize_axis(value: i32, origin: i32, extent: i32) -> i32 {
    if extent <= 1 {
        return 0;
    }
    let offset = i64::from(value).saturating_sub(i64::from(origin));
    let last = i64::from(extent - 1);
    (offset.clamp(0, last) * 65_535 / last) as i32
}

fn drag_points(from: PointerPosition, to: PointerPosition, steps: u32) -> Vec<PointerPosition> {
    let steps = i64::from(steps.max(1));
    let mut points = Vec::with_capacity(steps as usize);
    for index in 1..=steps {
        if index == steps {
            points.push(to);
            continue;
        }
        let interpolate = |start: i32, end: i32| {
            let (start, end) = (i64::from(start), i64::from(end));
            (start + (end - start) * index / steps) as i32
        };
        points.push(PointerPosition {
            x: interpolate(from.x, to.x),
            y: interpolate(from.y, to.y),
        });
    }
    points
}

fn absolute_mouse_input(position: PointerPosition, desktop: VirtualDesktop, flags: u32) -> INPUT {
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_MOUSE;
    input.Anonymous.mi = MOUSEINPUT {
        dx: normalize_axis(position.x, desktop.x, desktop.width),
        dy: normalize_axis(position.y, desktop.y, desktop.height),
        mouseData: 0,
        dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        time: 0,
        dwExtraInfo: 0,
    };
    input
}

fn drag_inputs(
    from: PointerPosition,
    to: PointerPosition,
    button: PointerButton,
    steps: u32,
    desktop: VirtualDesktop,
) -> Vec<INPUT> {
    let (down, up) = match button {
        PointerButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        PointerButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        PointerButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    let mut inputs = Vec::with_capacity(steps as usize + 3);
    inputs.push(absolute_mouse_input(from, desktop, MOUSEEVENTF_MOVE));
    inputs.push(absolute_mouse_input(from, desktop, down));
    for point in drag_points(from, to, steps) {
        inputs.push(absolute_mouse_input(point, desktop, MOUSEEVENTF_MOVE));
    }
    inputs.push(absolute_mouse_input(to, desktop, up));
    inputs
}

/// Deliver one bounded global-pointer drag through a single ordered
/// `SendInput` batch. Absolute coordinates are mapped over the full virtual
/// desktop, so monitors to the left or above the primary display retain their
/// signed screen positions.
pub(crate) fn pointer_drag(
    from: PointerPosition,
    to: PointerPosition,
    button: PointerButton,
    steps: u32,
) -> Result<(), InputInjectError> {
    if steps == 0 || steps > MAX_POINTER_DRAG_STEPS {
        return Err(InputInjectError::failed(
            "invalid_input",
            format!("steps must be 1..={MAX_POINTER_DRAG_STEPS}, got {steps}"),
        ));
    }
    let desktop = virtual_desktop()?;
    let inputs = drag_inputs(from, to, button, steps, desktop);
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        // A short batch may have posted the button-down but not its paired up.
        // An extra up is harmless if the down was never posted and prevents a
        // failed injection from leaving the user's button logically held.
        let release = *inputs.last().expect("drag always includes a release");
        let _ = unsafe { SendInput(1, &release, std::mem::size_of::<INPUT>() as i32) };
        return Err(InputInjectError::failed(
            "send_input_partial",
            format!("SendInput sent {sent}/{} drag inputs", inputs.len()),
        ));
    }
    Ok(())
}

pub(crate) fn type_text(text: &str) -> Result<(), InputInjectError> {
    let inputs = type_text_inputs(text);
    send_batch(&inputs)
}

/// Build the keyboard INPUT sequence for `text` as Unicode events
/// (KEYEVENTF_UNICODE via `wScan`). A VK-mode down/up would treat each
/// character code as a *virtual key*: the lowercase letter codes 0x61..0x7A
/// collide with the VK_NUMPAD1..VK_NUMPAD9 / VK_DECIMAL range, so
/// TranslateMessage would map them through the keyboard layout to the wrong
/// WM_CHAR (e.g. 'a' -> '1'). Unicode mode delivers the character verbatim;
/// non-BMP characters are expanded to their UTF-16 surrogate pair, one
/// key-down/key-up pair per code unit.
fn type_text_inputs(text: &str) -> Vec<INPUT> {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(text.chars().count() * 2);
    for ch in text.chars() {
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        for &unit in &*units {
            inputs.push(key_input(unit, KEYEVENTF_UNICODE));
            inputs.push(key_input(unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }
    }
    inputs
}

pub(crate) fn send_keys(shortcut: &str) -> Result<(), InputInjectError> {
    // Parse "ctrl+alt+key" style shortcuts; modifiers map to VK, the final key
    // may be a named key or a single character sent as Unicode.
    let parts: Vec<&str> = shortcut.split('+').map(|s| s.trim()).collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return Err(InputInjectError::failed(
            "bad_shortcut",
            format!("cannot parse shortcut '{shortcut}'"),
        ));
    }
    let mut down: Vec<INPUT> = Vec::new();
    let mut up: Vec<INPUT> = Vec::new();

    for part in &parts[..parts.len() - 1] {
        let vk = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => VK_CONTROL,
            "shift" => VK_SHIFT,
            "alt" => VK_MENU,
            "win" => VK_LWIN,
            _ => {
                return Err(InputInjectError::failed(
                    "unknown_modifier",
                    format!("unknown modifier '{part}'"),
                ));
            }
        };
        down.push(key_input(vk, 0));
        up.push(key_input(vk, KEYEVENTF_KEYUP));
    }

    let key = parts[parts.len() - 1];
    let lower = key.to_ascii_lowercase();
    match named_vk(&lower) {
        Some(vk) => {
            down.push(key_input(vk, 0));
            up.push(key_input(vk, KEYEVENTF_KEYUP));
        }
        None => {
            let chars: Vec<char> = key.chars().collect();
            if chars.len() != 1 {
                return Err(InputInjectError::failed(
                    "unknown_key",
                    format!("unknown key '{key}'"),
                ));
            }
            let code = virtual_key_for_character(chars[0]).ok_or_else(|| {
                InputInjectError::failed(
                    "unknown_key",
                    format!(
                        "character '{}' has no layout-independent physical key; use type_text for Unicode text",
                        chars[0]
                    ),
                )
            })?;
            down.push(key_input(code, 0));
            up.push(key_input(code, KEYEVENTF_KEYUP));
        }
    }

    let mut all = down;
    all.extend(up);
    send_batch(&all)
}

fn virtual_key_for_character(character: char) -> Option<u16> {
    if character.is_ascii_alphabetic() {
        Some(character.to_ascii_uppercase() as u16)
    } else if character.is_ascii_digit() {
        Some(character as u16)
    } else {
        None
    }
}

fn named_vk(lower: &str) -> Option<u16> {
    match lower {
        "enter" | "return" => Some(VK_RETURN),
        "tab" => Some(VK_TAB),
        "esc" | "escape" => Some(VK_ESCAPE),
        "space" => Some(VK_SPACE),
        "backspace" => Some(VK_BACK),
        "delete" => Some(VK_DELETE),
        "up" => Some(VK_UP),
        "down" => Some(VK_DOWN),
        "left" => Some(VK_LEFT),
        "right" => Some(VK_RIGHT),
        "f1" => Some(VK_F1),
        "f2" => Some(VK_F2),
        "f3" => Some(VK_F3),
        "f4" => Some(VK_F4),
        "f5" => Some(VK_F5),
        "f6" => Some(VK_F6),
        "f7" => Some(VK_F7),
        "f8" => Some(VK_F8),
        "f9" => Some(VK_F9),
        "f10" => Some(VK_F10),
        "f11" => Some(VK_F11),
        "f12" => Some(VK_F12),
        _ => None,
    }
}

/// Build a keyboard INPUT; `flags` includes KEYEVENTF_KEYUP when releasing.
///
/// Unicode mode (KEYEVENTF_UNICODE) delivers arbitrary characters via `wScan`;
/// VK mode delivers named keys via `wVk`. The two modes are mutually exclusive.
fn key_input(wvk_or_scan: u16, flags: u32) -> INPUT {
    let unicode = flags & KEYEVENTF_UNICODE != 0;
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT {
        wVk: if unicode { 0 } else { wvk_or_scan },
        wScan: if unicode { wvk_or_scan } else { 0 },
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    input
}

fn send_batch(inputs: &[INPUT]) -> Result<(), InputInjectError> {
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        return Err(InputInjectError::failed(
            "send_input_partial",
            format!("SendInput sent {sent}/{} inputs", inputs.len()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_pointer_scroll_is_refused_before_send_input() {
        for (dx, dy) in [(0, 0), (101, 0), (0, -101), (i32::MIN, 0)] {
            assert!(matches!(
                pointer_scroll(dx, dy),
                Err(InputInjectError::Failed { ref code, .. }) if code == "invalid_input"
            ));
        }
    }

    #[test]
    fn wheel_inputs_preserve_signed_detents_without_motion_flags() {
        for (flags, detents) in [(MOUSEEVENTF_WHEEL, -3), (MOUSEEVENTF_HWHEEL, 4)] {
            let input = mouse_wheel_input(flags, detents);
            assert_eq!(input.r#type, INPUT_MOUSE);
            let mouse = unsafe { input.Anonymous.mi };
            assert_eq!(mouse.dx, 0);
            assert_eq!(mouse.dy, 0);
            assert_eq!(mouse.dwFlags, flags);
            assert_eq!(
                i32::from_ne_bytes(mouse.mouseData.to_ne_bytes()),
                detents * WHEEL_DELTA
            );
        }
        let horizontal = wheel_inputs(2, 0);
        assert_eq!(horizontal.len(), 1);
        let mouse = unsafe { horizontal[0].Anonymous.mi };
        assert_eq!(mouse.dwFlags, MOUSEEVENTF_HWHEEL);
        assert_eq!(
            i32::from_ne_bytes(mouse.mouseData.to_ne_bytes()),
            -2 * WHEEL_DELTA,
            "portable positive dx means left, while Windows positive means right"
        );
    }

    #[test]
    fn absolute_drag_maps_the_full_signed_virtual_desktop() {
        let desktop = VirtualDesktop {
            x: -1920,
            y: -1080,
            width: 3840,
            height: 2160,
        };
        assert_eq!(normalize_axis(-1920, desktop.x, desktop.width), 0);
        assert_eq!(normalize_axis(1919, desktop.x, desktop.width), 65_535);
        assert_eq!(normalize_axis(-1080, desktop.y, desktop.height), 0);
        assert_eq!(normalize_axis(1079, desktop.y, desktop.height), 65_535);
        assert_eq!(normalize_axis(i32::MIN, desktop.x, desktop.width), 0);
        assert_eq!(normalize_axis(i32::MAX, desktop.x, desktop.width), 65_535);
    }

    #[test]
    fn drag_batch_is_one_bounded_move_down_moves_up_sequence() {
        let desktop = VirtualDesktop {
            x: -100,
            y: -50,
            width: 401,
            height: 201,
        };
        let from = PointerPosition { x: -100, y: -50 };
        let to = PointerPosition { x: 300, y: 150 };
        let inputs = drag_inputs(from, to, PointerButton::Left, 4, desktop);
        assert_eq!(inputs.len(), 7, "move + down + four moves + up");
        let mice: Vec<MOUSEINPUT> = inputs
            .iter()
            .map(|input| unsafe { input.Anonymous.mi })
            .collect();
        let absolute = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        assert_eq!(mice[0].dwFlags, absolute | MOUSEEVENTF_MOVE);
        assert_eq!(mice[1].dwFlags, absolute | MOUSEEVENTF_LEFTDOWN);
        for mouse in &mice[2..6] {
            assert_eq!(mouse.dwFlags, absolute | MOUSEEVENTF_MOVE);
        }
        assert_eq!(mice[6].dwFlags, absolute | MOUSEEVENTF_LEFTUP);
        assert_eq!((mice[0].dx, mice[0].dy), (0, 0));
        assert_eq!((mice[5].dx, mice[5].dy), (65_535, 65_535));
        assert_eq!((mice[6].dx, mice[6].dy), (65_535, 65_535));
    }

    #[test]
    fn drag_points_are_bounded_exact_and_overflow_free() {
        let from = PointerPosition {
            x: i32::MIN,
            y: i32::MAX,
        };
        let to = PointerPosition {
            x: i32::MAX,
            y: i32::MIN,
        };
        let points = drag_points(from, to, MAX_POINTER_DRAG_STEPS);
        assert_eq!(points.len(), MAX_POINTER_DRAG_STEPS as usize);
        assert_eq!(points.last(), Some(&to));
        assert!(
            points
                .windows(2)
                .all(|pair| { pair[0].x <= pair[1].x && pair[0].y >= pair[1].y })
        );
    }

    #[test]
    fn physical_ascii_keys_map_to_windows_virtual_keys() {
        assert_eq!(virtual_key_for_character('n'), Some(u16::from(b'N')));
        assert_eq!(virtual_key_for_character('N'), Some(u16::from(b'N')));
        assert_eq!(virtual_key_for_character('5'), Some(u16::from(b'5')));
        assert_eq!(virtual_key_for_character('?'), None);
        assert_eq!(virtual_key_for_character('中'), None);
    }

    /// Milestone 72 regression: `type_text` must inject Unicode events
    /// (wVk = 0, wScan = code unit, KEYEVENTF_UNICODE). Sending the raw
    /// character code as wVk maps lowercase letters onto the VK_NUMPAD1..9 /
    /// VK_DECIMAL range and TranslateMessage turns 'a' into '1'.
    #[test]
    fn type_text_injects_unicode_events_never_vk_events() {
        let inputs = type_text_inputs("a");
        assert_eq!(inputs.len(), 2, "one down + one up per character");
        assert_eq!(inputs[0].r#type, super::INPUT_KEYBOARD);
        assert_eq!(inputs[1].r#type, super::INPUT_KEYBOARD);
        let down = unsafe { inputs[0].Anonymous.ki };
        let up = unsafe { inputs[1].Anonymous.ki };
        assert_eq!(down.wVk, 0, "Unicode mode must not carry a virtual key");
        assert_eq!(down.wScan, u16::from(b'a'), "the code unit rides in wScan");
        assert_eq!(down.dwFlags & KEYEVENTF_UNICODE, KEYEVENTF_UNICODE);
        assert_eq!(up.dwFlags & KEYEVENTF_UNICODE, KEYEVENTF_UNICODE);
        assert_eq!(up.dwFlags & KEYEVENTF_KEYUP, KEYEVENTF_KEYUP);
    }

    /// A non-BMP character must be expanded to its UTF-16 surrogate pair,
    /// not truncated by a plain `char as u16` cast.
    #[test]
    fn type_text_expands_surrogate_pairs() {
        let inputs = type_text_inputs("\u{1F600}"); // U+1F600 GRINNING FACE
        assert_eq!(inputs.len(), 4, "two code units x down+up");
        let mut scans: Vec<u16> = inputs
            .iter()
            .filter(|i| {
                let ki = unsafe { i.Anonymous.ki };
                ki.dwFlags & KEYEVENTF_UNICODE != 0 && ki.dwFlags & KEYEVENTF_KEYUP == 0
            })
            .map(|i| unsafe { i.Anonymous.ki.wScan })
            .collect();
        let mut expected = "\u{1F600}".encode_utf16();
        let e1 = expected.next().expect("high surrogate");
        let e2 = expected.next().expect("low surrogate");
        let got = scans.as_mut_slice();
        assert_eq!(
            (got[0], got[1]),
            (e1, e2),
            "surrogate pair must be injected in order"
        );
    }
}
