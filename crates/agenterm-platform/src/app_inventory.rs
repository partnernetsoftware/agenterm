//! Installed-application inventory facade (portable entry point).

use crate::contract::app_inventory::{AppInventoryError, InstalledApps};

/// Every application this host has installed, running or not.
///
/// The counterpart to `window_enumerate`, which can only see applications
/// that currently have a window. A host with no notion of an installed
/// application answers `Unsupported`.
pub fn list_installed() -> Result<InstalledApps, AppInventoryError> {
    crate::selected::app_inventory::list_installed()
}

/// Ask the host to start the application at `path`.
///
/// **No pid comes back, and that is the platform's answer rather than an
/// omission**: every host route here hands the new process to a launcher
/// service (macOS LaunchServices, a desktop-entry `Exec=`, the shell
/// association), which owns it. A caller that needs the pid finds it the
/// way a person would -- by looking for the window that appears.
pub fn launch(path: &str) -> Result<(), AppInventoryError> {
    crate::selected::app_inventory::launch(path)
}
