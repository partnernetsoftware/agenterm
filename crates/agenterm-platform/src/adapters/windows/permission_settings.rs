use crate::permission_settings::{
    PermissionKind, PermissionOpenReceipt, PermissionSettingsError, PermissionSettingsErrorKind,
    PermissionState, PermissionStatus,
};

pub(crate) fn status(
    permission: PermissionKind,
) -> Result<PermissionStatus, PermissionSettingsError> {
    Ok(PermissionStatus {
        permission,
        state: PermissionState::ProviderSpecific,
        provider: "windows-provider-specific",
    })
}

pub(crate) fn open(
    _permission: PermissionKind,
) -> Result<PermissionOpenReceipt, PermissionSettingsError> {
    Err(PermissionSettingsError::new(
        PermissionSettingsErrorKind::Unsupported,
        "Windows permission repair is provider-specific; no equivalent generic Accessibility or Screen Capture pane is claimed",
    ))
}
