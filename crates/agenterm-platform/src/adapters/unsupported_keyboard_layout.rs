//! Non-Linux answer for keyboard layout observation.

use crate::keyboard_layout::{
    KeyboardLayoutObserveResult, KeyboardLayoutObserveUnsupported,
};

pub(crate) fn observe() -> KeyboardLayoutObserveResult {
    KeyboardLayoutObserveResult::Unsupported(KeyboardLayoutObserveUnsupported {
        reason: "keyboard layout observation is only implemented on Linux X11".into(),
        required_mechanism: "linux-x11-xkb".into(),
        alternatives: vec![
            "run keyboard-layout on a Linux host with DISPLAY and an X11 session".into(),
        ],
    })
}
