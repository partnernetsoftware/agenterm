use crate::host_boot_identity::{
    HostBootIdentityError, HostBootIdentityErrorKind, HostBootIdentityFacts,
};

pub(crate) fn query_facts() -> Result<HostBootIdentityFacts, HostBootIdentityError> {
    Ok(HostBootIdentityFacts {
        boot_id: read_boot_id()?,
        machine_id: read_machine_id()?,
        uptime_milliseconds: uptime_milliseconds()?,
    })
}

pub(crate) fn query_material() -> Result<Vec<u8>, HostBootIdentityError> {
    Ok(read_boot_id()?.into_bytes())
}

fn read_boot_id() -> Result<String, HostBootIdentityError> {
    let value = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(|error| {
        HostBootIdentityError::new(
            HostBootIdentityErrorKind::Query,
            format!("read Linux boot_id: {error}"),
        )
    })?;
    let value = value.trim();
    if !boot_id_shape(value) {
        return Err(HostBootIdentityError::new(
            HostBootIdentityErrorKind::InvalidNativeValue,
            "Linux boot_id had an invalid UUID shape",
        ));
    }
    Ok(value.to_owned())
}

fn read_machine_id() -> Result<String, HostBootIdentityError> {
    let value = std::fs::read_to_string("/etc/machine-id").map_err(|error| {
        HostBootIdentityError::new(
            HostBootIdentityErrorKind::Query,
            format!("read Linux machine-id: {error}"),
        )
    })?;
    let value = value.trim();
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(HostBootIdentityError::new(
            HostBootIdentityErrorKind::InvalidNativeValue,
            "Linux machine-id had an invalid shape",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn uptime_milliseconds() -> Result<u64, HostBootIdentityError> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &raw mut time) } != 0 {
        return Err(HostBootIdentityError::new(
            HostBootIdentityErrorKind::Query,
            format!(
                "clock_gettime(CLOCK_BOOTTIME): {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    if time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) {
        return Err(HostBootIdentityError::new(
            HostBootIdentityErrorKind::InvalidNativeValue,
            "CLOCK_BOOTTIME returned invalid timespec",
        ));
    }
    let seconds = u64::try_from(time.tv_sec).map_err(|_| {
        HostBootIdentityError::new(
            HostBootIdentityErrorKind::InvalidNativeValue,
            "uptime seconds overflow",
        )
    })?;
    seconds
        .checked_mul(1000)
        .and_then(|value| {
            value.checked_add(
                u64::try_from(time.tv_nsec)
                    .ok()?
                    .checked_div(1_000_000)?,
            )
        })
        .ok_or_else(|| {
            HostBootIdentityError::new(
                HostBootIdentityErrorKind::InvalidNativeValue,
                "uptime milliseconds overflow",
            )
        })
}

fn boot_id_shape(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_boot_facts_are_stable_and_well_formed() {
        let first = query_facts().expect("boot facts");
        let second = query_facts().expect("repeated boot facts");
        assert_eq!(first.boot_id, second.boot_id);
        assert_eq!(first.machine_id, second.machine_id);
        assert!(boot_id_shape(&first.boot_id));
        assert_eq!(first.machine_id.len(), 32);
        assert!(first
            .machine_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()));
        assert!(second.uptime_milliseconds >= first.uptime_milliseconds);
    }
}
