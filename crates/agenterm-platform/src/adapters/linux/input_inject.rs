//! Linux X11 input injection through the XTest extension.

use std::env;
use std::time::Duration;

use x11rb::{
    CURRENT_TIME,
    connection::{Connection, RequestConnection},
    errors::ConnectionError,
    protocol::xproto::{
        BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, KEY_PRESS_EVENT,
        KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
    },
    protocol::xtest::{ConnectionExt as _, X11_EXTENSION_NAME},
    rust_connection::RustConnection,
};

use crate::CapabilityStatus;
use crate::contract::input_inject::{
    InputInjectError, MAX_POINTER_DRAG_STEPS, PointerButton, PointerPosition,
    validate_pointer_scroll,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionKind {
    X11,
    Wayland,
    Unavailable,
}

fn classify_session(
    session_type: Option<&str>,
    wayland_display: Option<&str>,
    x11_display: Option<&str>,
) -> SessionKind {
    if session_type == Some("wayland") || wayland_display.is_some_and(|value| !value.is_empty()) {
        SessionKind::Wayland
    } else if session_type == Some("x11") || x11_display.is_some_and(|value| !value.is_empty()) {
        SessionKind::X11
    } else {
        SessionKind::Unavailable
    }
}

fn session_kind() -> SessionKind {
    classify_session(
        env::var("XDG_SESSION_TYPE").ok().as_deref(),
        env::var("WAYLAND_DISPLAY").ok().as_deref(),
        env::var("DISPLAY").ok().as_deref(),
    )
}

fn x11_screen_available(display: Option<&str>) -> bool {
    display.is_some_and(|value| !value.is_empty())
}

fn x11_display_available() -> bool {
    x11_screen_available(env::var("DISPLAY").ok().as_deref())
}

struct Context {
    connection: RustConnection,
    root: u32,
}

fn failed(message: impl ToString) -> InputInjectError {
    InputInjectError::failed("input_inject_failed", message)
}

fn open_x11_root() -> Result<Context, InputInjectError> {
    let (connection, screen) = x11rb::connect(None)
        .map_err(|error| failed(format!("X11 display could not be opened: {error}")))?;
    let root = connection
        .setup()
        .roots
        .get(screen)
        .ok_or_else(|| failed("configured X11 screen does not exist"))?
        .root;
    Ok(Context { connection, root })
}

fn connect() -> Result<Context, InputInjectError> {
    match session_kind() {
        SessionKind::X11 => {}
        SessionKind::Wayland => {
            return Err(InputInjectError::Unsupported {
                reason: "input-inject requires X11; Wayland has no XTest injection".into(),
            });
        }
        SessionKind::Unavailable => {
            return Err(InputInjectError::Unsupported {
                reason: "input-inject requires an X11 display".into(),
            });
        }
    }
    open_x11_root()
}

/// Read-only pointer observation may use an XWayland display even when the
/// session is classified as Wayland. Injection keeps the stricter gate.
fn connect_for_observe() -> Result<Context, InputInjectError> {
    if !x11_display_available() {
        return Err(InputInjectError::Unsupported {
            reason: "pointer-position requires an X11 display".into(),
        });
    }
    open_x11_root()
}

fn require_xtest(context: &Context) -> Result<(), InputInjectError> {
    match context.connection.extension_information(X11_EXTENSION_NAME) {
        Ok(Some(_)) => Ok(()),
        Ok(None) | Err(ConnectionError::UnsupportedExtension) => {
            Err(InputInjectError::Unsupported {
                reason: "X11 server does not provide the XTest extension".into(),
            })
        }
        Err(error) => Err(failed(format!("XTest extension query failed: {error}"))),
    }
}

fn xtest_input(
    context: &Context,
    event_type: u8,
    detail: u8,
    root_x: i16,
    root_y: i16,
) -> Result<(), InputInjectError> {
    require_xtest(context)?;
    context
        .connection
        .xtest_fake_input(
            event_type,
            detail,
            CURRENT_TIME,
            context.root,
            root_x,
            root_y,
            0,
        )
        .map_err(|_| failed("XTest input request could not be sent"))?
        .check()
        .map_err(|_| failed("X11 server rejected XTest input"))?;
    context
        .connection
        .flush()
        .map_err(|_| failed("XTest input request could not be flushed"))
}

fn button_detail(button: PointerButton) -> u8 {
    match button {
        PointerButton::Left => 1,
        PointerButton::Right => 3,
        PointerButton::Middle => 2,
    }
}

fn keysym_for_token(token: &str) -> Option<u32> {
    match token.to_ascii_lowercase().as_str() {
        "backspace" => Some(0xff08),
        "tab" => Some(0xff09),
        "enter" | "return" => Some(0xff0d),
        "escape" | "esc" => Some(0xff1b),
        "space" => Some(0x0020),
        "home" => Some(0xff50),
        "left" => Some(0xff51),
        "up" => Some(0xff52),
        "right" => Some(0xff53),
        "down" => Some(0xff54),
        "delete" | "del" => Some(0xffff),
        "f1" => Some(0xffbe),
        "f2" => Some(0xffbf),
        "f3" => Some(0xffc0),
        "f4" => Some(0xffc1),
        "f5" => Some(0xffc2),
        "f6" => Some(0xffc3),
        "f7" => Some(0xffc4),
        "f8" => Some(0xffc5),
        "f9" => Some(0xffc6),
        "f10" => Some(0xffc7),
        "f11" => Some(0xffc8),
        "f12" => Some(0xffc9),
        _ => token.chars().next().map(keysym_for_char),
    }
}

fn keysym_for_char(ch: char) -> u32 {
    let scalar = u32::from(ch);
    if scalar <= 0xff {
        scalar
    } else {
        // X11's conventional Unicode keysym encoding. The key-map lookup
        // below still refuses characters the active layout cannot produce;
        // it must never truncate a Unicode scalar to an unrelated byte.
        0x0100_0000 | scalar
    }
}

fn modifier_keysym(token: &str) -> Option<u32> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(0xffe3),
        "shift" => Some(0xffe1),
        "alt" => Some(0xffe9),
        "meta" | "super" | "win" => Some(0xffeb),
        _ => None,
    }
}

fn keycode(context: &Context, requested: u32) -> Result<u8, InputInjectError> {
    let setup = context.connection.setup();
    let first = setup.min_keycode;
    let count = setup
        .max_keycode
        .checked_sub(first)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| failed("X11 keyboard mapping is invalid"))?;
    let reply = context
        .connection
        .get_keyboard_mapping(first, count)
        .map_err(|_| failed("X11 keyboard-map request could not be sent"))?
        .reply()
        .map_err(|_| failed("X11 keyboard-map request failed"))?;
    let per_keycode = usize::from(reply.keysyms_per_keycode);
    if per_keycode == 0 {
        return Err(failed("X11 keyboard mapping is empty"));
    }
    reply
        .keysyms
        .chunks(per_keycode)
        .position(|symbols| symbols.contains(&requested))
        .and_then(|offset| u8::try_from(offset).ok())
        .and_then(|offset| first.checked_add(offset))
        .ok_or_else(|| {
            failed(format!(
                "requested key has no X11 keycode for keysym {requested:#x}"
            ))
        })
}

fn press_key(context: &Context, keycode: u8) -> Result<(), InputInjectError> {
    xtest_input(context, KEY_PRESS_EVENT, keycode, 0, 0)
}

fn release_key(context: &Context, keycode: u8) -> Result<(), InputInjectError> {
    xtest_input(context, KEY_RELEASE_EVENT, keycode, 0, 0)
}

pub(crate) fn capability_status() -> CapabilityStatus {
    match session_kind() {
        SessionKind::X11 => CapabilityStatus::Available,
        SessionKind::Wayland => CapabilityStatus::Unsupported {
            reason: "input-inject requires X11".into(),
        },
        SessionKind::Unavailable => CapabilityStatus::Unsupported {
            reason: "input-inject requires DISPLAY".into(),
        },
    }
}

pub(crate) fn pointer_move(position: PointerPosition) -> Result<(), InputInjectError> {
    let context = connect()?;
    let x = i16::try_from(position.x)
        .map_err(|_| failed("pointer x coordinate is outside the X11 range"))?;
    let y = i16::try_from(position.y)
        .map_err(|_| failed("pointer y coordinate is outside the X11 range"))?;
    xtest_input(&context, MOTION_NOTIFY_EVENT, 0, x, y)
}

pub(crate) fn pointer_position() -> Result<PointerPosition, InputInjectError> {
    let context = connect_for_observe()?;
    let reply = context
        .connection
        .query_pointer(context.root)
        .map_err(|_| failed("X11 pointer query could not be sent"))?
        .reply()
        .map_err(|_| failed("X11 pointer query failed"))?;
    Ok(PointerPosition {
        x: i32::from(reply.root_x),
        y: i32::from(reply.root_y),
    })
}

pub(crate) fn pointer_scroll(dx: i32, dy: i32) -> Result<(), InputInjectError> {
    validate_pointer_scroll(dx, dy)?;
    let context = connect()?;
    // X11's conventional wheel buttons: 4/5 are vertical up/down and 6/7
    // are horizontal left/right. Posting button pairs at (0, 0) retains the
    // current pointer location; unlike MOTION_NOTIFY no coordinate is applied.
    for button in wheel_buttons(dx, dy) {
        xtest_input(&context, BUTTON_PRESS_EVENT, button, 0, 0)?;
        xtest_input(&context, BUTTON_RELEASE_EVENT, button, 0, 0)?;
    }
    Ok(())
}

/// Post signed wheel detents at `position` in absolute screen coordinates.
/// The physical pointer is warped to the target only for delivery and then
/// restored so callers can scroll a named AT-SPI region without leaving the
/// cursor displaced.
pub(crate) fn pointer_scroll_at(
    position: PointerPosition,
    dx: i32,
    dy: i32,
) -> Result<(), InputInjectError> {
    validate_pointer_scroll(dx, dy)?;
    let context = connect()?;
    let reply = context
        .connection
        .query_pointer(context.root)
        .map_err(|_| failed("X11 pointer query could not be sent"))?
        .reply()
        .map_err(|_| failed("X11 pointer query failed"))?;
    let home_x = reply.root_x;
    let home_y = reply.root_y;
    let x = i16::try_from(position.x)
        .map_err(|_| failed("pointer x coordinate is outside the X11 range"))?;
    let y = i16::try_from(position.y)
        .map_err(|_| failed("pointer y coordinate is outside the X11 range"))?;
    deliver_wheel_at(
        &wheel_buttons(dx, dy),
        (x, y),
        (home_x, home_y),
        |event_type, detail, event_x, event_y| {
            xtest_input(&context, event_type, detail, event_x, event_y)
        },
    )
}

fn deliver_wheel_at(
    buttons: &[u8],
    target: (i16, i16),
    home: (i16, i16),
    mut post: impl FnMut(u8, u8, i16, i16) -> Result<(), InputInjectError>,
) -> Result<(), InputInjectError> {
    let delivery = (|| {
        post(MOTION_NOTIFY_EVENT, 0, target.0, target.1)?;
        for &button in buttons {
            post(BUTTON_PRESS_EVENT, button, 0, 0)?;
            if let Err(error) = post(BUTTON_RELEASE_EVENT, button, 0, 0) {
                // A failed release is still followed by one best-effort
                // release before returning the original typed failure.
                let _ = post(BUTTON_RELEASE_EVENT, button, 0, 0);
                return Err(error);
            }
        }
        Ok(())
    })();
    // Restoration is a finally-style obligation: a failed wheel event must
    // not leave the real pointer parked over the addressed node.
    let restored = post(MOTION_NOTIFY_EVENT, 0, home.0, home.1);
    match delivery {
        Err(error) => Err(error),
        Ok(()) => restored,
    }
}

fn wheel_buttons(dx: i32, dy: i32) -> Vec<u8> {
    let mut buttons = Vec::with_capacity((dx.unsigned_abs() + dy.unsigned_abs()) as usize);
    buttons.extend(std::iter::repeat_n(
        if dy > 0 { 4 } else { 5 },
        dy.unsigned_abs() as usize,
    ));
    buttons.extend(std::iter::repeat_n(
        if dx > 0 { 6 } else { 7 },
        dx.unsigned_abs() as usize,
    ));
    buttons
}

pub(crate) fn pointer_click(
    position: PointerPosition,
    button: PointerButton,
    clicks: u32,
) -> Result<(), InputInjectError> {
    let context = connect()?;
    let x = i16::try_from(position.x)
        .map_err(|_| failed("pointer x coordinate is outside the X11 range"))?;
    let y = i16::try_from(position.y)
        .map_err(|_| failed("pointer y coordinate is outside the X11 range"))?;
    let detail = button_detail(button);
    let repeats = clicks.max(1);
    for index in 0..repeats {
        xtest_input(&context, MOTION_NOTIFY_EVENT, 0, x, y)?;
        xtest_input(&context, BUTTON_PRESS_EVENT, detail, 0, 0)?;
        xtest_input(&context, BUTTON_RELEASE_EVENT, detail, 0, 0)?;
        if index + 1 < repeats {
            std::thread::sleep(Duration::from_millis(40));
        }
    }
    Ok(())
}

fn drag_points(from: PointerPosition, to: PointerPosition, steps: u32) -> Vec<(i32, i32)> {
    let steps = i64::from(steps.max(1));
    let mut out = Vec::with_capacity(steps as usize);
    for i in 1..=steps {
        if i == steps {
            out.push((to.x, to.y));
            continue;
        }
        let lerp = |a: i32, b: i32| -> i32 {
            let (a, b) = (i64::from(a), i64::from(b));
            (a + (b - a) * i / steps) as i32
        };
        out.push((lerp(from.x, to.x), lerp(from.y, to.y)));
    }
    out
}

pub(crate) fn pointer_drag(
    from: PointerPosition,
    to: PointerPosition,
    button: PointerButton,
    steps: u32,
) -> Result<(), InputInjectError> {
    if steps == 0 || steps > MAX_POINTER_DRAG_STEPS {
        return Err(InputInjectError::Failed {
            code: "invalid_input".into(),
            message: format!("steps must be 1..={MAX_POINTER_DRAG_STEPS}, got {steps}"),
        });
    }
    let context = connect()?;
    let detail = button_detail(button);
    let from_x = i16::try_from(from.x)
        .map_err(|_| failed("pointer x coordinate is outside the X11 range"))?;
    let from_y = i16::try_from(from.y)
        .map_err(|_| failed("pointer y coordinate is outside the X11 range"))?;
    let to_x =
        i16::try_from(to.x).map_err(|_| failed("pointer x coordinate is outside the X11 range"))?;
    let to_y =
        i16::try_from(to.y).map_err(|_| failed("pointer y coordinate is outside the X11 range"))?;
    xtest_input(&context, MOTION_NOTIFY_EVENT, 0, from_x, from_y)?;
    xtest_input(&context, BUTTON_PRESS_EVENT, detail, 0, 0)?;
    for (x, y) in drag_points(from, to, steps) {
        let x = i16::try_from(x)
            .map_err(|_| failed("pointer x coordinate is outside the X11 range"))?;
        let y = i16::try_from(y)
            .map_err(|_| failed("pointer y coordinate is outside the X11 range"))?;
        xtest_input(&context, MOTION_NOTIFY_EVENT, 0, x, y)?;
    }
    xtest_input(&context, MOTION_NOTIFY_EVENT, 0, to_x, to_y)?;
    xtest_input(&context, BUTTON_RELEASE_EVENT, detail, 0, 0)?;
    Ok(())
}

pub(crate) fn type_text(text: &str) -> Result<(), InputInjectError> {
    let context = connect()?;
    for ch in text.chars() {
        let keysym = keysym_for_char(ch);
        let keycode = keycode(&context, keysym)?;
        press_key(&context, keycode)?;
        release_key(&context, keycode)?;
    }
    Ok(())
}

pub(crate) fn send_keys(shortcut: &str) -> Result<(), InputInjectError> {
    let context = connect()?;
    let parts: Vec<&str> = shortcut.split('+').map(str::trim).collect();
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        return Err(failed(format!("cannot parse shortcut '{shortcut}'")));
    }
    let mut modifier_codes = Vec::new();
    for part in &parts[..parts.len() - 1] {
        let keysym = modifier_keysym(part).ok_or_else(|| {
            failed(format!(
                "unknown modifier '{part}' in shortcut '{shortcut}'"
            ))
        })?;
        modifier_codes.push(keycode(&context, keysym)?);
    }
    let final_token = parts[parts.len() - 1];
    let final_keysym = keysym_for_token(final_token).ok_or_else(|| {
        failed(format!(
            "unknown key '{final_token}' in shortcut '{shortcut}'"
        ))
    })?;
    let final_code = keycode(&context, final_keysym)?;
    for code in &modifier_codes {
        press_key(&context, *code)?;
    }
    press_key(&context, final_code)?;
    release_key(&context, final_code)?;
    for code in modifier_codes.iter().rev() {
        release_key(&context, *code)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_tokens_map_to_expected_keysyms() {
        assert_eq!(keysym_for_token("enter"), Some(0xff0d));
        assert_eq!(modifier_keysym("ctrl"), Some(0xffe3));
        assert_eq!(keysym_for_token("a"), Some(b'a'.into()));
        assert_eq!(keysym_for_token("é"), Some(0x00e9));
        assert_eq!(keysym_for_token("中"), Some(0x0100_4e2d));
        assert_ne!(keysym_for_token("中"), Some(0x2d));
    }

    #[test]
    fn invalid_pointer_scroll_is_refused_before_opening_a_display() {
        for (dx, dy) in [(0, 0), (101, 0), (0, -101), (i32::MIN, 0)] {
            assert!(matches!(
                pointer_scroll(dx, dy),
                Err(InputInjectError::Failed { ref code, .. }) if code == "invalid_input"
            ));
        }
    }

    #[test]
    fn signed_scroll_axes_map_to_x11_wheel_buttons() {
        assert_eq!(wheel_buttons(2, -3), [5, 5, 5, 6, 6]);
        assert_eq!(wheel_buttons(-1, 1), [4, 7]);
    }

    #[test]
    fn wayland_session_does_not_imply_missing_x11_display() {
        // A Wayland session may still publish DISPLAY=:N for XWayland. Injection
        // stays gated; pointer observation consults the display directly.
        assert_eq!(
            classify_session(Some("wayland"), Some("wayland-0"), Some(":2")),
            SessionKind::Wayland
        );
        assert!(x11_screen_available(Some(":2")));
        assert!(!x11_screen_available(Some("")));
        assert!(!x11_screen_available(None));
    }

    #[test]
    fn failed_wheel_release_is_retried_and_pointer_is_restored() {
        let mut calls = Vec::new();
        let mut release_attempts = 0;
        let result = deliver_wheel_at(&[5], (30, 40), (10, 20), |event, detail, x, y| {
            calls.push((event, detail, x, y));
            if event == BUTTON_RELEASE_EVENT {
                release_attempts += 1;
                if release_attempts == 1 {
                    return Err(failed("synthetic release failure"));
                }
            }
            Ok(())
        });
        assert!(matches!(
            result,
            Err(InputInjectError::Failed { message, .. }) if message == "synthetic release failure"
        ));
        assert_eq!(
            calls,
            [
                (MOTION_NOTIFY_EVENT, 0, 30, 40),
                (BUTTON_PRESS_EVENT, 5, 0, 0),
                (BUTTON_RELEASE_EVENT, 5, 0, 0),
                (BUTTON_RELEASE_EVENT, 5, 0, 0),
                (MOTION_NOTIFY_EVENT, 0, 10, 20),
            ]
        );
    }

    #[test]
    fn failed_wheel_press_still_restores_pointer_and_preserves_delivery_error() {
        let mut calls = Vec::new();
        let result = deliver_wheel_at(&[5], (30, 40), (10, 20), |event, detail, x, y| {
            calls.push((event, detail, x, y));
            if event == BUTTON_PRESS_EVENT {
                return Err(failed("synthetic press failure"));
            }
            Ok(())
        });
        assert!(matches!(
            result,
            Err(InputInjectError::Failed { message, .. }) if message == "synthetic press failure"
        ));
        assert_eq!(
            calls,
            [
                (MOTION_NOTIFY_EVENT, 0, 30, 40),
                (BUTTON_PRESS_EVENT, 5, 0, 0),
                (MOTION_NOTIFY_EVENT, 0, 10, 20),
            ]
        );
    }

    #[test]
    fn restoration_failure_is_reported_after_successful_wheel_delivery() {
        let mut calls = Vec::new();
        let result = deliver_wheel_at(&[5], (30, 40), (10, 20), |event, detail, x, y| {
            calls.push((event, detail, x, y));
            if event == MOTION_NOTIFY_EVENT && (x, y) == (10, 20) {
                return Err(failed("synthetic restoration failure"));
            }
            Ok(())
        });
        assert!(matches!(
            result,
            Err(InputInjectError::Failed { message, .. }) if message == "synthetic restoration failure"
        ));
        assert_eq!(
            calls,
            [
                (MOTION_NOTIFY_EVENT, 0, 30, 40),
                (BUTTON_PRESS_EVENT, 5, 0, 0),
                (BUTTON_RELEASE_EVENT, 5, 0, 0),
                (MOTION_NOTIFY_EVENT, 0, 10, 20),
            ]
        );
    }
}
