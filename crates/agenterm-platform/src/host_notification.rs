//! Typed, shell-free dispatch of one bounded desktop notification.

use std::fmt;

const MAX_TITLE_BYTES: usize = 1024;
const MAX_BODY_BYTES: usize = 16 * 1024;
const MAX_SUBTITLE_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HostNotificationErrorKind {
    InvalidInput,
    Unsupported,
    DispatcherUnavailable,
    Rejected,
    TimedOut,
    Native,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostNotificationError {
    kind: HostNotificationErrorKind,
    message: String,
}

impl HostNotificationError {
    pub(crate) fn new(kind: HostNotificationErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> HostNotificationErrorKind {
        self.kind
    }
}

impl fmt::Display for HostNotificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HostNotificationError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostNotificationOptions<'a> {
    pub subtitle: Option<&'a str>,
    pub sound: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostNotificationReceipt {
    pub provider: &'static str,
    /// The native dispatcher accepted the notification. User presentation or
    /// attention is intentionally not claimed.
    pub accepted: bool,
}

pub fn notify(
    title: &str,
    body: &str,
    options: HostNotificationOptions<'_>,
) -> Result<HostNotificationReceipt, HostNotificationError> {
    validate("title", title, 1, MAX_TITLE_BYTES)?;
    validate("body", body, 0, MAX_BODY_BYTES)?;
    if let Some(subtitle) = options.subtitle {
        validate("subtitle", subtitle, 1, MAX_SUBTITLE_BYTES)?;
    }
    crate::selected::host_notification::notify(title, body, options)
}

fn validate(
    field: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), HostNotificationError> {
    if value.len() < minimum || value.len() > maximum || value.as_bytes().contains(&0) {
        return Err(HostNotificationError::new(
            HostNotificationErrorKind::InvalidInput,
            format!("notification {field} must be {minimum}..={maximum} UTF-8 bytes without NUL"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_title_nul_and_oversized_body_before_dispatch() {
        assert_eq!(
            notify("", "", HostNotificationOptions::default())
                .unwrap_err()
                .kind(),
            HostNotificationErrorKind::InvalidInput
        );
        assert_eq!(
            notify("title", "bad\0body", HostNotificationOptions::default())
                .unwrap_err()
                .kind(),
            HostNotificationErrorKind::InvalidInput
        );
        let body = "x".repeat(MAX_BODY_BYTES + 1);
        assert_eq!(
            notify("title", &body, HostNotificationOptions::default())
                .unwrap_err()
                .kind(),
            HostNotificationErrorKind::InvalidInput
        );
    }
}
