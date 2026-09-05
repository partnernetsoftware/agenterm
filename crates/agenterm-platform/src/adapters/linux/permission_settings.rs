use crate::permission_settings::{
    PermissionKind, PermissionOpenReceipt, PermissionSettingsError, PermissionSettingsErrorKind,
    PermissionState, PermissionStatus,
};

pub(crate) fn status(
    permission: PermissionKind,
) -> Result<PermissionStatus, PermissionSettingsError> {
    Ok(PermissionStatus {
        permission,
        state: PermissionState::NotApplicable,
        provider: "linux-no-per-app-consent",
    })
}

pub(crate) fn open(
    _permission: PermissionKind,
) -> Result<PermissionOpenReceipt, PermissionSettingsError> {
    Err(PermissionSettingsError::new(
        PermissionSettingsErrorKind::NotApplicable,
        "Linux has no equivalent per-application Accessibility or Screen Capture consent pane",
    ))
}
