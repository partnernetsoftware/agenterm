use crate::host_boot_identity::{HostBootIdentityError, HostBootIdentityErrorKind};

pub(super) fn query_material() -> Result<Vec<u8>, HostBootIdentityError> {
    let mut boot = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut length = std::mem::size_of::<libc::timeval>();
    if unsafe {
        libc::sysctlbyname(
            c"kern.boottime".as_ptr(),
            (&raw mut boot).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(HostBootIdentityError::new(
            HostBootIdentityErrorKind::Query,
            format!("sysctl kern.boottime: {}", std::io::Error::last_os_error()),
        ));
    }
    if length != std::mem::size_of::<libc::timeval>()
        || boot.tv_sec < 0
        || !(0..1_000_000).contains(&boot.tv_usec)
    {
        return Err(HostBootIdentityError::new(
            HostBootIdentityErrorKind::InvalidNativeValue,
            "kern.boottime returned an invalid timeval",
        ));
    }
    let mut material = Vec::with_capacity(16);
    material.extend_from_slice(&boot.tv_sec.to_le_bytes());
    material.extend_from_slice(&boot.tv_usec.to_le_bytes());
    Ok(material)
}
