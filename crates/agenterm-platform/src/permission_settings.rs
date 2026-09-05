//! Typed inspection and dispatch for host permission settings.
//!
//! Opening a settings pane is not consent. The receipt reports only native
//! dispatcher acceptance; callers must re-read status after the user changes it.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PermissionKind {
    Accessibility,
    ScreenCapture,
}

impl PermissionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accessibility => "accessibility",
            Self::ScreenCapture => "screen-capture",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PermissionState {
    Granted,
    Denied,
    Unknown,
    NotApplicable,
    ProviderSpecific,
}

impl PermissionState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Denied => "denied",
            Self::Unknown => "unknown",
            Self::NotApplicable => "not-applicable",
            Self::ProviderSpecific => "provider-specific",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionStatus {
    pub permission: PermissionKind,
    pub state: PermissionState,
    pub provider: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionOpenReceipt {
    pub permission: PermissionKind,
    pub before: PermissionState,
    pub provider: &'static str,
    pub accepted: bool,
    pub already_granted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PermissionSettingsErrorKind {
    NotApplicable,
    Unsupported,
    LauncherUnavailable,
    Rejected,
    TimedOut,
    Native,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionSettingsError {
    kind: PermissionSettingsErrorKind,
    message: String,
}

impl PermissionSettingsError {
    pub(crate) fn new(kind: PermissionSettingsErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> PermissionSettingsErrorKind {
        self.kind
    }
}

impl fmt::Display for PermissionSettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PermissionSettingsError {}

pub fn status(permission: PermissionKind) -> Result<PermissionStatus, PermissionSettingsError> {
    crate::selected::permission_settings::status(permission)
}

pub fn open(permission: PermissionKind) -> Result<PermissionOpenReceipt, PermissionSettingsError> {
    crate::selected::permission_settings::open(permission)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_names_are_stable() {
        assert_eq!(PermissionKind::Accessibility.as_str(), "accessibility");
        assert_eq!(PermissionKind::ScreenCapture.as_str(), "screen-capture");
        assert_eq!(PermissionState::NotApplicable.as_str(), "not-applicable");
        assert_eq!(
            PermissionState::ProviderSpecific.as_str(),
            "provider-specific"
        );
    }
}
