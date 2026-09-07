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

/// Largest signed wheel-detent magnitude accepted on either axis by one
/// `pointer_scroll` call.
///
/// This is deliberately a per-axis bound: a diagonal request may post both
/// axes, but neither can amplify one call beyond 100 native detents. Validation
/// happens before an adapter opens a display or posts an event.
pub const MAX_POINTER_SCROLL_DETENTS: u32 = 100;

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

pub(crate) fn validate_pointer_scroll(dx: i32, dy: i32) -> Result<(), InputInjectError> {
    if dx == 0 && dy == 0 {
        return Err(InputInjectError::failed(
            "invalid_input",
            "pointer scroll requires at least one non-zero axis",
        ));
    }
    if dx.unsigned_abs() > MAX_POINTER_SCROLL_DETENTS
        || dy.unsigned_abs() > MAX_POINTER_SCROLL_DETENTS
    {
        return Err(InputInjectError::failed(
            "invalid_input",
            format!(
                "pointer scroll axis magnitude must be <= {MAX_POINTER_SCROLL_DETENTS}, got dx={dx}, dy={dy}"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{InputInjectError, MAX_POINTER_SCROLL_DETENTS, validate_pointer_scroll};

    #[test]
    fn pointer_scroll_contract_rejects_zero_and_unbounded_axes() {
        for (dx, dy) in [
            (0, 0),
            (MAX_POINTER_SCROLL_DETENTS as i32 + 1, 0),
            (-(MAX_POINTER_SCROLL_DETENTS as i32) - 1, 0),
            (0, MAX_POINTER_SCROLL_DETENTS as i32 + 1),
            (0, -(MAX_POINTER_SCROLL_DETENTS as i32) - 1),
            (i32::MIN, 0),
        ] {
            assert!(matches!(
                validate_pointer_scroll(dx, dy),
                Err(InputInjectError::Failed { ref code, .. }) if code == "invalid_input"
            ));
        }
    }

    #[test]
    fn pointer_scroll_contract_accepts_each_axis_and_the_diagonal_bound() {
        assert_eq!(validate_pointer_scroll(1, 0), Ok(()));
        assert_eq!(validate_pointer_scroll(0, -1), Ok(()));
        assert_eq!(
            validate_pointer_scroll(
                MAX_POINTER_SCROLL_DETENTS as i32,
                -(MAX_POINTER_SCROLL_DETENTS as i32),
            ),
            Ok(())
        );
    }
}
