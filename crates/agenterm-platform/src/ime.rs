//! Selected IME capability and platform-neutral composition state machine.

use serde::{Deserialize, Serialize};

pub use crate::contract::ime::{ImeAction, ImeComposition, ImeEvent, ImeStatus};
use crate::{CapabilityStatus, selected};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImeEnvSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gtk_im_module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qt_im_module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xmodifiers: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImeHostObservation {
    pub provider: String,
    pub framework: String,
    pub name: String,
    pub available: bool,
    pub open: bool,
    pub native_mode: bool,
    pub full_shape: bool,
    pub label: String,
    pub env: ImeEnvSnapshot,
    pub session_bus_available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImeObserveUnsupported {
    pub reason: String,
    pub env: ImeEnvSnapshot,
    pub probed_frameworks: Vec<String>,
    pub session_bus_available: bool,
    pub required_mechanism: String,
    pub alternatives: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImeObserveResult {
    Ok(ImeHostObservation),
    Unsupported(ImeObserveUnsupported),
}

impl ImeHostObservation {
    pub(crate) fn from_status(
        provider: impl Into<String>,
        framework: impl Into<String>,
        status: ImeStatus,
        env: ImeEnvSnapshot,
        session_bus_available: bool,
    ) -> Self {
        Self {
            provider: provider.into(),
            framework: framework.into(),
            name: status.name.clone(),
            available: status.available,
            open: status.open,
            native_mode: status.native_mode,
            full_shape: status.full_shape,
            label: status.label(),
            env,
            session_bus_available,
        }
    }
}

/// Poll the session input-method framework when the host exposes one.
#[must_use]
pub fn observe_host() -> ImeObserveResult {
    selected::ime::observe()
}

pub fn capability_status(display_available: bool) -> CapabilityStatus {
    selected::ime::capability_status(display_available)
}

/// Input method currently backing this thread's focused surface, when the
/// host can report one. `None` means "unknown", which callers should render
/// as absence rather than as a disabled IME.
#[must_use]
pub fn status() -> Option<ImeStatus> {
    selected::ime::status()
}

/// Current in-progress composition reported by the host, when one is active.
/// Windows adapters maintain this while WM_IME_* messages are processed;
/// winit hosts report the same data as ImeEvent::Preedit and answer
/// None here.
#[must_use]
pub fn composition() -> Option<ImeComposition> {
    selected::ime::composition()
}

/// Anchor the IME composition and candidate windows to a client-area point.
///
/// `x` and `y` are client coordinates of the active caret (for a terminal,
/// the cursor cell's top-left corner). Native adapters convert to screen
/// coordinates internally. Platforms whose input-method events arrive through
/// a different mechanism (winit on macOS/Linux) treat this as a no-op.
///
/// The terminal grid is not a native editable control, so without this call
/// the OS has no caret to anchor the candidate window to and the candidate
/// bar appears at an arbitrary position.
pub fn set_anchor_position(x: i32, y: i32) {
    selected::ime::set_anchor_position(x, y)
}

pub fn classify_event(event: ImeEvent, anchor_available: bool) -> ImeAction {
    match event {
        ImeEvent::Enabled => ImeAction::None,
        ImeEvent::Preedit { text, cursor } if anchor_available => {
            ImeAction::UpdatePreedit { text, cursor }
        }
        ImeEvent::Preedit { .. } | ImeEvent::Disabled => ImeAction::ClearPreedit,
        ImeEvent::Commit(text) => {
            if text.is_empty() || text.chars().any(char::is_control) {
                ImeAction::ClearPreedit
            } else {
                ImeAction::CommitText(text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preedit_requires_an_editable_anchor() {
        let event = ImeEvent::Preedit {
            text: "ni".to_owned(),
            cursor: Some((0, 2)),
        };
        assert_eq!(
            classify_event(event.clone(), true),
            ImeAction::UpdatePreedit {
                text: "ni".to_owned(),
                cursor: Some((0, 2)),
            }
        );
        assert_eq!(classify_event(event, false), ImeAction::ClearPreedit);
    }

    #[test]
    fn empty_commits_are_not_text() {
        assert_eq!(
            classify_event(ImeEvent::Commit(String::new()), true),
            ImeAction::ClearPreedit
        );
        assert_eq!(
            classify_event(ImeEvent::Commit("你好".to_owned()), true),
            ImeAction::CommitText("你好".to_owned())
        );
    }
}
