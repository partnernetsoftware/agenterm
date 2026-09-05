use crate::host_open::{HostOpenError, HostOpenErrorKind, HostOpenOptions, HostOpenReceipt};
use windows_sys::Win32::UI::{
    Shell::ShellExecuteW,
    WindowsAndMessaging::{SW_SHOWNOACTIVATE, SW_SHOWNORMAL},
};

pub(crate) fn open(
    target: &str,
    options: HostOpenOptions<'_>,
) -> Result<HostOpenReceipt, HostOpenError> {
    if options.application.is_some() {
        return Err(HostOpenError::new(
            HostOpenErrorKind::Unsupported,
            "Windows host-open does not yet claim explicit application selection",
        ));
    }
    let wide = target.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    // SAFETY: every pointer is either null as allowed by ShellExecuteW or a
    // retained NUL-terminated UTF-16 buffer for the duration of the call.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            if options.background {
                SW_SHOWNOACTIVATE
            } else {
                SW_SHOWNORMAL
            },
        )
    };
    let code = result as isize;
    if code <= 32 {
        return Err(HostOpenError::new(
            HostOpenErrorKind::Rejected,
            format!("ShellExecuteW rejected the request with code {code}"),
        ));
    }
    Ok(HostOpenReceipt {
        provider: "windows-shell-execute",
        accepted: true,
    })
}
