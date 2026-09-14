use std::{
    mem::{size_of, zeroed},
    ptr::null_mut,
    time::Duration,
};

use windows_sys::Win32::UI::{
    Shell::{
        NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
        Shell_NotifyIconW,
    },
    WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, IDI_APPLICATION, LoadIconW,
    },
};

use crate::host_notification::{
    HostNotificationError, HostNotificationErrorKind, HostNotificationOptions,
    HostNotificationReceipt,
};

// NOTIFYICONDATAW reserves the final UTF-16 code unit in each array for NUL.
// Validate in encoded units before creating the message-only window so CU can
// truthfully classify an oversized request as not performed.
const INFO_TITLE_UTF16_CAPACITY: usize = 64;
const INFO_BODY_UTF16_CAPACITY: usize = 256;

pub(crate) fn notify(
    title: &str,
    body: &str,
    options: HostNotificationOptions<'_>,
) -> Result<HostNotificationReceipt, HostNotificationError> {
    if options.subtitle.is_some() || options.sound {
        return Err(HostNotificationError::new(
            HostNotificationErrorKind::Unsupported,
            "Windows host notification does not claim subtitle or sound semantics",
        ));
    }
    if !options.actions.is_empty() {
        return Err(HostNotificationError::new(
            HostNotificationErrorKind::Unsupported,
            "Windows host notification does not claim action-button semantics",
        ));
    }
    validate_native_text("title", title, INFO_TITLE_UTF16_CAPACITY)?;
    validate_native_text("body", body, INFO_BODY_UTF16_CAPACITY)?;
    let class = "STATIC\0".encode_utf16().collect::<Vec<_>>();
    // SAFETY: the built-in STATIC class is NUL-terminated, all optional
    // pointers are null, and the returned HWND remains owned until destroy.
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            null_mut(),
            null_mut(),
        )
    };
    if window.is_null() {
        return Err(HostNotificationError::new(
            HostNotificationErrorKind::Native,
            format!(
                "notification message window creation failed: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    let result = dispatch(window, title, body);
    // SAFETY: `window` is the live handle created above and is destroyed once.
    unsafe { DestroyWindow(window) };
    result
}

fn dispatch(
    window: windows_sys::Win32::Foundation::HWND,
    title: &str,
    body: &str,
) -> Result<HostNotificationReceipt, HostNotificationError> {
    let mut icon: NOTIFYICONDATAW = unsafe { zeroed() };
    icon.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    icon.hWnd = window;
    icon.uID = 1;
    icon.uFlags = NIF_ICON | NIF_TIP | NIF_INFO;
    // SAFETY: IDI_APPLICATION is a predefined shared resource identifier.
    icon.hIcon = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) };
    copy_wide(&mut icon.szTip, "AgenTerm");
    copy_wide(&mut icon.szInfoTitle, title);
    copy_wide(&mut icon.szInfo, body);
    icon.dwInfoFlags = NIIF_INFO;
    icon.Anonymous.uTimeout = 3000;
    // SAFETY: NOTIFYICONDATAW points to initialized storage that remains live
    // for the synchronous call; the message window is valid.
    if unsafe { Shell_NotifyIconW(NIM_ADD, &icon) } == 0 {
        return Err(HostNotificationError::new(
            HostNotificationErrorKind::Rejected,
            format!(
                "Shell_NotifyIconW rejected the request: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    std::thread::sleep(Duration::from_secs(4));
    // SAFETY: removes only the icon identified by the same live HWND/uID.
    unsafe { Shell_NotifyIconW(NIM_DELETE, &icon) };
    Ok(HostNotificationReceipt {
        provider: "windows-notify-icon",
        accepted: true,
    })
}

fn copy_wide<const N: usize>(destination: &mut [u16; N], value: &str) {
    debug_assert!(value.encode_utf16().count() < N);
    for (slot, unit) in destination
        .iter_mut()
        .take(N.saturating_sub(1))
        .zip(value.encode_utf16())
    {
        *slot = unit;
    }
}

fn validate_native_text(
    field: &str,
    value: &str,
    capacity: usize,
) -> Result<(), HostNotificationError> {
    let encoded_units = value.encode_utf16().count();
    if encoded_units >= capacity {
        return Err(HostNotificationError::new(
            HostNotificationErrorKind::InvalidInput,
            format!(
                "Windows notification {field} must fit in at most {} UTF-16 code units",
                capacity - 1
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_text_capacity_reserves_nul_without_splitting_surrogates() {
        assert!(validate_native_text("title", &"x".repeat(63), 64).is_ok());
        assert_eq!(
            validate_native_text("title", &"x".repeat(64), 64)
                .unwrap_err()
                .kind(),
            HostNotificationErrorKind::InvalidInput
        );
        assert!(validate_native_text("title", &"\u{1f642}".repeat(31), 64).is_ok());
        assert_eq!(
            validate_native_text("title", &"\u{1f642}".repeat(32), 64)
                .unwrap_err()
                .kind(),
            HostNotificationErrorKind::InvalidInput
        );
    }

    #[test]
    fn notification_fields_use_their_native_capacities() {
        assert!(validate_native_text("title", &"t".repeat(63), INFO_TITLE_UTF16_CAPACITY).is_ok());
        assert_eq!(
            validate_native_text("title", &"t".repeat(64), INFO_TITLE_UTF16_CAPACITY)
                .unwrap_err()
                .kind(),
            HostNotificationErrorKind::InvalidInput
        );
        assert!(validate_native_text("body", &"b".repeat(255), INFO_BODY_UTF16_CAPACITY).is_ok());
        assert_eq!(
            validate_native_text("body", &"b".repeat(256), INFO_BODY_UTF16_CAPACITY)
                .unwrap_err()
                .kind(),
            HostNotificationErrorKind::InvalidInput
        );
    }

    #[test]
    fn oversized_native_text_is_refused_before_dispatch() {
        assert_eq!(
            notify(
                &"t".repeat(INFO_TITLE_UTF16_CAPACITY),
                "body",
                HostNotificationOptions::default(),
            )
            .unwrap_err()
            .kind(),
            HostNotificationErrorKind::InvalidInput
        );
        assert_eq!(
            notify(
                "title",
                &"b".repeat(INFO_BODY_UTF16_CAPACITY),
                HostNotificationOptions::default(),
            )
            .unwrap_err()
            .kind(),
            HostNotificationErrorKind::InvalidInput
        );
    }
}
