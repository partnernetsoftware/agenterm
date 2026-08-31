//! Platform-neutral Unicode clipboard outcomes.
//!
//! The embedding product supplies payload policy such as its maximum retained
//! read size. Native adapters only report whether the operation is unsupported
//! or failed with a stable machine-readable code.

use std::borrow::Cow;

/// What the clipboard is *carrying*, by type name, without reading any of
/// it.
///
/// An agent that can only paste text still needs to know an image or a
/// file reference is on the clipboard -- otherwise an empty text read
/// looks like an empty clipboard. The names are the host's own spelling
/// (macOS UTIs, X11 TARGETS atoms, Windows clipboard format names), not a
/// normalized vocabulary: a caller matching on them is matching on what
/// the platform actually said.
///
/// This reports names only. Reading arbitrary rich content is a separate
/// question with its own policy, and knowing what is there is the half an
/// agent needs to choose its next move.
pub const MAX_CLIPBOARD_TYPES: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ClipboardError {
    Unsupported {
        reason: Cow<'static, str>,
    },
    Failed {
        code: Cow<'static, str>,
        message: String,
    },
}

impl ClipboardError {
    pub fn unsupported(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::Unsupported {
            reason: reason.into(),
        }
    }

    pub fn failed(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Unsupported { reason } => format!("clipboard unsupported: {reason}"),
            Self::Failed { message, .. } => message.clone(),
        }
    }

    pub fn to_capability_status(&self) -> crate::CapabilityStatus {
        match self {
            Self::Unsupported { reason } => crate::CapabilityStatus::Unsupported {
                reason: reason.clone(),
            },
            Self::Failed { code, message } => crate::CapabilityStatus::Failed {
                code: code.clone(),
                message: message.clone(),
            },
        }
    }
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { reason } => write!(formatter, "clipboard unsupported: {reason}"),
            Self::Failed { message, .. } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ClipboardError {}

pub type ClipboardResult<T> = Result<T, ClipboardError>;

#[cfg(test)]
mod tests {
    use super::ClipboardError;

    #[test]
    fn unsupported_and_failed_remain_distinct() {
        assert_eq!(
            ClipboardError::unsupported("headless-display").to_string(),
            "clipboard unsupported: headless-display"
        );
        assert_eq!(
            ClipboardError::failed("clipboard_timeout", "deadline elapsed").to_string(),
            "deadline elapsed"
        );
    }
}
