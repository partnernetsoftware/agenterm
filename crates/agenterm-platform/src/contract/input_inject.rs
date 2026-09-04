//! Platform-neutral input injection contract.

use std::borrow::Cow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PointerPosition {
    pub x: i32,
    pub y: i32,
}

/// Largest number of intermediate moves one `pointer_drag` may deliver.
///
/// The bound belongs to the contract rather than to one adapter: the ABI
/// validates `steps` against it *before* any platform call, so an
/// out-of-range request never touches the pointer on any host, and every
/// adapter that implements the drag rejects the same range with the same
/// number in its message.
pub const MAX_POINTER_DRAG_STEPS: u32 = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum InputInjectError {
    Unsupported {
        reason: Cow<'static, str>,
    },
    Failed {
        code: Cow<'static, str>,
        message: String,
    },
}

impl InputInjectError {
    /// Built by the Linux and Windows adapters; the macOS one reports its
    /// failures another way, so this looks unused when linting that target.
    #[allow(dead_code)]
    pub(crate) fn failed(code: &'static str, message: impl ToString) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.to_string(),
        }
    }
}
