//! Platform-neutral window operation contract.

use std::borrow::Cow;

/// Window visibility/placement states accepted by `window_op::show`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum WindowShowState {
    Hide,
    Show,
    Minimize,
    Maximize,
    Restore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WindowOpError {
    Unsupported {
        reason: Cow<'static, str>,
    },
    Failed {
        code: Cow<'static, str>,
        message: String,
    },
}

impl WindowOpError {
    pub(crate) fn failed(code: &'static str, message: impl ToString) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Absent mechanism" and "the mechanism ran and failed" are two typed
    /// variants, never one stringly-typed error: the ABI maps the first to
    /// `AGT_UNSUPPORTED` and the second to `AGT_FAILED{code}`, so a
    /// `Unsupported` that carried a code (or a `Failed` that meant "not
    /// wired") would be reported to consumers as the wrong outcome.
    #[test]
    fn unsupported_and_failed_stay_distinct_variants() {
        let unsupported = WindowOpError::Unsupported {
            reason: "reading the minimized state is not wired here yet".into(),
        };
        let failed = WindowOpError::failed("window_not_found", "no window 1");
        assert!(matches!(unsupported, WindowOpError::Unsupported { .. }));
        assert!(matches!(
            failed,
            WindowOpError::Failed { ref code, .. } if code == "window_not_found"
        ));
        assert_ne!(unsupported, failed);
    }

    /// Every state the facade accepts is a distinct value: `Minimize` and
    /// `Restore` are the two macOS wired in ABI 1.25 and must not collapse
    /// into `Show`.
    #[test]
    fn show_states_are_distinct() {
        let states = [
            WindowShowState::Hide,
            WindowShowState::Show,
            WindowShowState::Minimize,
            WindowShowState::Maximize,
            WindowShowState::Restore,
        ];
        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                assert_eq!(i == j, a == b, "{a:?} vs {b:?}");
            }
        }
    }
}
