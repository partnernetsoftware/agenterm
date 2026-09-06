use windows_sys::core::GUID;

use crate::host_boot_identity::{HostBootIdentityError, HostBootIdentityErrorKind};

const SYSTEM_BOOT_ENVIRONMENT_INFORMATION_CLASS: i32 = 90;

#[repr(C)]
struct SystemBootEnvironmentInformation {
    boot_identifier: GUID,
    firmware_type: u32,
    boot_flags: u64,
}

pub(crate) fn query_material() -> Result<Vec<u8>, HostBootIdentityError> {
    use windows_sys::Wdk::System::SystemInformation::NtQuerySystemInformation;

    let mut information: SystemBootEnvironmentInformation = unsafe { std::mem::zeroed() };
    let mut returned = 0_u32;
    let status = unsafe {
        NtQuerySystemInformation(
            SYSTEM_BOOT_ENVIRONMENT_INFORMATION_CLASS,
            (&raw mut information).cast(),
            std::mem::size_of::<SystemBootEnvironmentInformation>() as u32,
            &raw mut returned,
        )
    };
    if status < 0 {
        return Err(HostBootIdentityError::new(
            HostBootIdentityErrorKind::Query,
            format!("NtQuerySystemInformation(SystemBootEnvironmentInformation): {status:#x}"),
        ));
    }
    if returned < 16
        || (information.boot_identifier.data1 == 0
            && information.boot_identifier.data2 == 0
            && information.boot_identifier.data3 == 0
            && information.boot_identifier.data4 == [0; 8])
    {
        return Err(HostBootIdentityError::new(
            HostBootIdentityErrorKind::InvalidNativeValue,
            "Windows returned an empty boot identifier",
        ));
    }
    let mut material = Vec::with_capacity(16);
    material.extend_from_slice(&information.boot_identifier.data1.to_le_bytes());
    material.extend_from_slice(&information.boot_identifier.data2.to_le_bytes());
    material.extend_from_slice(&information.boot_identifier.data3.to_le_bytes());
    material.extend_from_slice(&information.boot_identifier.data4);
    Ok(material)
}
