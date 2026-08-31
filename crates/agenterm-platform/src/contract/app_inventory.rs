//! Platform-neutral installed-application inventory and launch.
//!
//! This is the discovery half of the app-lifecycle family: which
//! applications exist on this host, whether or not any of them is running.
//! The running half is `window_enumerate`, which can only see applications
//! that have a window.

use std::borrow::Cow;

/// One application the host has installed.
///
/// `name` is what a person would call it and `path` is what a launcher
/// needs. No bundle identifier: reading one means parsing a plist (macOS)
/// or a desktop entry (Linux) for every candidate, and the name plus the
/// path already address the application unambiguously.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InstalledApp {
    pub name: String,
    pub path: String,
}

/// Most applications one listing reports. A host with more than this is
/// not a host a name-matching caller can search usefully, and the listing
/// says it truncated rather than pretending to be complete.
pub const MAX_INSTALLED_APPS: usize = 2_048;

/// Longest path this accepts for a launch, a bound rather than a policy.
pub const MAX_APP_PATH_BYTES: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AppInventoryError {
    Unsupported {
        reason: Cow<'static, str>,
    },
    Failed {
        code: Cow<'static, str>,
        message: String,
    },
}

impl AppInventoryError {
    #[allow(dead_code)]
    pub(crate) fn failed(code: &'static str, message: impl ToString) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.to_string(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Unsupported { reason } => format!("app inventory unsupported: {reason}"),
            Self::Failed { message, .. } => message.clone(),
        }
    }
}

impl std::fmt::Display for AppInventoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

/// The listing, sorted by name and de-duplicated by path, with a flag for
/// whether the bound cut it short.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct InstalledApps {
    pub apps: Vec<InstalledApp>,
    pub truncated: bool,
}
