//! Typed, shell-free dispatch of one path or URL to a host application.

use std::fmt;

const MAX_TARGET_BYTES: usize = 32 * 1024;
const MAX_APP_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HostOpenErrorKind {
    InvalidInput,
    Unsupported,
    LauncherUnavailable,
    Rejected,
    TimedOut,
    Native,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostOpenError {
    kind: HostOpenErrorKind,
    message: String,
}

impl HostOpenError {
    pub(crate) fn new(kind: HostOpenErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> HostOpenErrorKind {
        self.kind
    }
}

impl fmt::Display for HostOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HostOpenError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostOpenOptions<'a> {
    pub application: Option<&'a str>,
    pub background: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostOpenReceipt {
    pub provider: &'static str,
    /// The native dispatcher accepted the request. This does not prove that
    /// the selected application rendered or consumed the target.
    pub accepted: bool,
}

pub fn open(target: &str, options: HostOpenOptions<'_>) -> Result<HostOpenReceipt, HostOpenError> {
    validate(target, options.application)?;
    crate::selected::host_open::open(target, options)
}

fn validate(target: &str, application: Option<&str>) -> Result<(), HostOpenError> {
    if target.is_empty()
        || target.len() > MAX_TARGET_BYTES
        || target.as_bytes().contains(&0)
        || target.starts_with('-')
    {
        return Err(HostOpenError::new(
            HostOpenErrorKind::InvalidInput,
            "host-open target must be 1..=32768 UTF-8 bytes, contain no NUL, and not begin with '-'",
        ));
    }
    if let Some(application) = application
        && (application.is_empty()
            || application.len() > MAX_APP_BYTES
            || application.as_bytes().contains(&0)
            || application.starts_with('-'))
    {
        return Err(HostOpenError::new(
            HostOpenErrorKind::InvalidInput,
            "host-open application must be 1..=512 UTF-8 bytes, contain no NUL, and not begin with '-'",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_option_like_nul_and_oversized_inputs_before_native_dispatch() {
        for target in ["", "-option", "bad\0target"] {
            assert_eq!(
                open(target, HostOpenOptions::default())
                    .expect_err("invalid target")
                    .kind(),
                HostOpenErrorKind::InvalidInput
            );
        }
        assert_eq!(
            open(
                "https://example.invalid",
                HostOpenOptions {
                    application: Some("-application"),
                    background: false,
                },
            )
            .expect_err("invalid application")
            .kind(),
            HostOpenErrorKind::InvalidInput
        );
        let oversized = "x".repeat(MAX_TARGET_BYTES + 1);
        assert_eq!(
            open(&oversized, HostOpenOptions::default())
                .expect_err("oversized target")
                .kind(),
            HostOpenErrorKind::InvalidInput
        );
    }
}
