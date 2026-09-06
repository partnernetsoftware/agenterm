use super::{
    HostFreeMemorySemantics, HostResourceSnapshotError, HostResourceSnapshotErrorKind,
    NativeSnapshot, checked_hostname, memory_from_native, unavailable_windows_load,
};

pub(super) fn snapshot() -> Result<NativeSnapshot, HostResourceSnapshotError> {
    let available = crate::host_memory::availability().map_err(|error| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::MemoryQuery,
            error.to_string(),
        )
    })?;
    Ok(NativeSnapshot {
        hostname: hostname()?,
        uptime_milliseconds: unsafe {
            windows_sys::Win32::System::SystemInformation::GetTickCount64()
        },
        load_average: unavailable_windows_load(),
        processor_model: processor_model()?,
        memory: memory_from_native(
            available.available_physical_bytes,
            HostFreeMemorySemantics::WindowsAvailablePhysical,
        )?,
    })
}

fn hostname() -> Result<String, HostResourceSnapshotError> {
    use windows_sys::Win32::System::SystemInformation::{
        ComputerNameDnsHostname, GetComputerNameExW,
    };

    let mut buffer = [0_u16; 256];
    let mut length = buffer.len() as u32;
    if unsafe {
        GetComputerNameExW(
            ComputerNameDnsHostname,
            buffer.as_mut_ptr(),
            &raw mut length,
        )
    } == 0
    {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::HostnameQuery,
            format!("GetComputerNameExW: {}", std::io::Error::last_os_error()),
        ));
    }
    let length = usize::try_from(length).map_err(|_| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "GetComputerNameExW length overflowed",
        )
    })?;
    if length == 0 || length > buffer.len() {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "GetComputerNameExW returned invalid length",
        ));
    }
    let value = String::from_utf16(&buffer[..length]).map_err(|_| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "computer name is not valid UTF-16",
        )
    })?;
    checked_hostname(value.as_bytes())
}

fn processor_model() -> Result<String, HostResourceSnapshotError> {
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW},
    };

    let subkey = wide("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0");
    let value_name = wide("ProcessorNameString");
    let mut bytes = 0_u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut bytes,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok("unknown".to_owned());
    }
    if status != ERROR_SUCCESS || !(2..=8192).contains(&bytes) || !bytes.is_multiple_of(2) {
        return Err(registry_error(status, "query ProcessorNameString size"));
    }
    let mut buffer = vec![0_u16; bytes as usize / 2];
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &raw mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(registry_error(status, "read ProcessorNameString"));
    }
    let written = bytes as usize / 2;
    if written == 0 || written > buffer.len() {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "ProcessorNameString returned invalid length",
        ));
    }
    buffer.truncate(written);
    if buffer.last() == Some(&0) {
        buffer.pop();
    }
    let model = String::from_utf16(&buffer).map_err(|_| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "ProcessorNameString is not valid UTF-16",
        )
    })?;
    let model = model.trim();
    Ok(if model.is_empty() {
        "unknown".to_owned()
    } else {
        model.chars().take(1024).collect()
    })
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn registry_error(status: u32, operation: &str) -> HostResourceSnapshotError {
    HostResourceSnapshotError::new(
        HostResourceSnapshotErrorKind::ProcessorQuery,
        format!("{operation}: Windows status {status}"),
    )
}
