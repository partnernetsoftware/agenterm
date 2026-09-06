use crate::host_boot_identity::{HostBootIdentityError, HostBootIdentityErrorKind};

pub(super) fn query_material() -> Result<Vec<u8>, HostBootIdentityError> {
    let value = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|error| {
        HostBootIdentityError::new(
            HostBootIdentityErrorKind::Query,
            format!("read Linux boot_id: {error}"),
        )
    })?;
    let value = value.trim();
    if value.len() != 36
        || !value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err(HostBootIdentityError::new(
            HostBootIdentityErrorKind::InvalidNativeValue,
            "Linux boot_id had an invalid UUID shape",
        ));
    }
    Ok(value.as_bytes().to_vec())
}
