use crate::host_open::{HostOpenError, HostOpenErrorKind, HostOpenOptions, HostOpenReceipt};
use windows_sys::Win32::UI::{
    Shell::ShellExecuteW,
    WindowsAndMessaging::{SW_SHOWNOACTIVATE, SW_SHOWNORMAL},
};

pub(crate) fn open(
    target: &str,
    options: HostOpenOptions<'_>,
) -> Result<HostOpenReceipt, HostOpenError> {
    let target_wide = target.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let application_wide = options.application.map(|application| {
        application
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>()
    });
    let parameters_wide = options
        .application
        .map(|_| explicit_application_parameters(target));
    let file = application_wide
        .as_ref()
        .map_or(target_wide.as_ptr(), |application| application.as_ptr());
    let parameters = parameters_wide
        .as_ref()
        .map_or(std::ptr::null(), |parameters| parameters.as_ptr());
    // SAFETY: every pointer is either null as allowed by ShellExecuteW or a
    // retained NUL-terminated UTF-16 buffer for the duration of the call. An
    // explicit application receives TARGET as exactly one Windows argv item.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            file,
            parameters,
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

fn explicit_application_parameters(target: &str) -> Vec<u16> {
    crate::process_conventions::windows_command_line(&[target.to_owned()])
        .expect("host-open validation rejected NUL before Windows argument encoding")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn explicit_application_target_is_one_windows_argument() {
        let target = r#"C:\fixture path\trailing\"#;
        let wide = super::explicit_application_parameters(target);
        assert_eq!(wide.last(), Some(&0));
        let encoded = String::from_utf16(&wide[..wide.len() - 1]).expect("UTF-16 parameters");
        assert_eq!(encoded, r#""C:\fixture path\trailing\\""#);
    }
}
