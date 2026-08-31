//! unix installed-application inventory: not wired in this cut.

use crate::contract::app_inventory::{AppInventoryError, InstalledApps};

/// Listing installed applications here means walking nothing and reading a
/// display name out of each entry, and launching one means nothing.
/// Neither is wired, and a caller is told that rather than handed an empty
/// list -- "no applications installed" and "this host cannot tell you" are
/// very different answers.
pub(crate) fn list_installed() -> Result<InstalledApps, AppInventoryError> {
    Err(AppInventoryError::Unsupported {
        reason: "the installed-application inventory is not wired on this host".into(),
    })
}

pub(crate) fn launch(_path: &str) -> Result<(), AppInventoryError> {
    Err(AppInventoryError::Unsupported {
        reason: "application launch is not wired on this host".into(),
    })
}
