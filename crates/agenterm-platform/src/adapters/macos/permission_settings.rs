//! macOS TCC status and exact System Settings pane dispatch.

use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::permission_settings::{
    PermissionKind, PermissionOpenReceipt, PermissionSettingsError, PermissionSettingsErrorKind,
    PermissionState, PermissionStatus,
};

const ACCESSIBILITY_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
const SCREEN_CAPTURE_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

pub(crate) fn status(
    permission: PermissionKind,
) -> Result<PermissionStatus, PermissionSettingsError> {
    let granted = match permission {
        PermissionKind::Accessibility => {
            // SAFETY: no pointers; this only reads this process' TCC decision.
            unsafe { AXIsProcessTrusted() != 0 }
        }
        PermissionKind::ScreenCapture => {
            // SAFETY: no pointers or prompt; this only preflights TCC state.
            unsafe { CGPreflightScreenCaptureAccess() }
        }
    };
    Ok(PermissionStatus {
        permission,
        state: if granted {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        },
        provider: "macos-tcc",
    })
}

pub(crate) fn open(
    permission: PermissionKind,
) -> Result<PermissionOpenReceipt, PermissionSettingsError> {
    let before = status(permission)?.state;
    if before == PermissionState::Granted {
        return Ok(PermissionOpenReceipt {
            permission,
            before,
            provider: "macos-tcc",
            accepted: false,
            already_granted: true,
        });
    }
    let url = match permission {
        PermissionKind::Accessibility => ACCESSIBILITY_URL,
        PermissionKind::ScreenCapture => SCREEN_CAPTURE_URL,
    };
    let mut child = Command::new("/usr/bin/open")
        .arg(url)
        .spawn()
        .map_err(|error| {
            let kind = if error.kind() == std::io::ErrorKind::NotFound {
                PermissionSettingsErrorKind::LauncherUnavailable
            } else {
                PermissionSettingsErrorKind::Native
            };
            PermissionSettingsError::new(kind, format!("macOS settings dispatch failed: {error}"))
        })?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return Ok(PermissionOpenReceipt {
                    permission,
                    before,
                    provider: "macos-system-settings",
                    accepted: true,
                    already_granted: false,
                });
            }
            Ok(Some(status)) => {
                return Err(PermissionSettingsError::new(
                    PermissionSettingsErrorKind::Rejected,
                    format!("macOS settings dispatcher rejected the request with {status}"),
                ));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                return Err(PermissionSettingsError::new(
                    PermissionSettingsErrorKind::TimedOut,
                    "macOS settings dispatcher did not finish within 10 seconds",
                ));
            }
            Err(error) => {
                return Err(PermissionSettingsError::new(
                    PermissionSettingsErrorKind::Native,
                    format!("macOS settings dispatcher status failed: {error}"),
                ));
            }
        }
    }
}
