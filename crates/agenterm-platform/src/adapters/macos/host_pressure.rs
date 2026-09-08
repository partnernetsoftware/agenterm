use std::mem::size_of;

use super::contract::{
    HostPressureError, HostPressureErrorKind, HostPressureSignal, HostPressureSnapshot,
    HostPressureUnavailableReason,
};

const VM_MEMORY_PRESSURE_NAME: &[u8] = b"vm.memory_pressure\0";

pub(super) fn snapshot() -> Result<HostPressureSnapshot, HostPressureError> {
    let memory = vm_memory_pressure()?;
    let unavailable = HostPressureSignal::Unavailable {
        reason: HostPressureUnavailableReason::NotPublishedByPlatform,
    };

    Ok(HostPressureSnapshot {
        memory,
        cpu: unavailable,
        io: unavailable,
    })
}

fn vm_memory_pressure() -> Result<HostPressureSignal, HostPressureError> {
    let mut raw_level = 0_i32;
    let mut output_len = size_of::<i32>();
    // SAFETY: the name is NUL-terminated, `raw_level` is writable for exactly
    // `output_len` bytes, and both pointers remain valid for the synchronous call.
    let return_code = unsafe {
        libc::sysctlbyname(
            VM_MEMORY_PRESSURE_NAME.as_ptr().cast(),
            (&mut raw_level as *mut i32).cast(),
            &mut output_len,
            std::ptr::null_mut(),
            0,
        )
    };
    let errno = (return_code != 0)
        .then(|| std::io::Error::last_os_error().raw_os_error())
        .flatten();

    checked_vm_memory_pressure(return_code, errno, output_len, raw_level)
}

fn checked_vm_memory_pressure(
    return_code: libc::c_int,
    errno: Option<i32>,
    output_len: usize,
    raw_level: i32,
) -> Result<HostPressureSignal, HostPressureError> {
    if return_code != 0 {
        let kind = if errno == Some(libc::ENOENT) || errno == Some(libc::ENOTSUP) {
            HostPressureErrorKind::ProviderUnavailable
        } else {
            HostPressureErrorKind::NativeQuery
        };
        let detail = match errno {
            Some(code) => format!("sysctlbyname(vm.memory_pressure) failed with errno {code}"),
            None => "sysctlbyname(vm.memory_pressure) failed without errno".to_owned(),
        };
        return Err(HostPressureError::new(kind, detail));
    }

    if output_len != size_of::<i32>() {
        return Err(HostPressureError::new(
            HostPressureErrorKind::MalformedNativeData,
            format!(
                "sysctlbyname(vm.memory_pressure) returned {output_len} bytes; expected {}",
                size_of::<i32>()
            ),
        ));
    }

    Ok(HostPressureSignal::MacosVmPressure { raw_level })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_i32_result_preserves_native_raw_level() {
        assert_eq!(
            checked_vm_memory_pressure(0, None, size_of::<i32>(), -7).unwrap(),
            HostPressureSignal::MacosVmPressure { raw_level: -7 }
        );
    }

    #[test]
    fn successful_query_requires_exact_i32_length() {
        for output_len in [0, size_of::<i32>() - 1, size_of::<i32>() + 1] {
            assert_eq!(
                checked_vm_memory_pressure(0, None, output_len, 1)
                    .unwrap_err()
                    .kind(),
                HostPressureErrorKind::MalformedNativeData
            );
        }
    }

    #[test]
    fn missing_or_unsupported_provider_is_typed_unavailable() {
        for errno in [libc::ENOENT, libc::ENOTSUP] {
            assert_eq!(
                checked_vm_memory_pressure(-1, Some(errno), size_of::<i32>(), 0)
                    .unwrap_err()
                    .kind(),
                HostPressureErrorKind::ProviderUnavailable
            );
        }
    }

    #[test]
    fn other_sysctl_failures_are_typed_native_query_errors() {
        for errno in [Some(libc::EIO), None] {
            assert_eq!(
                checked_vm_memory_pressure(-1, errno, size_of::<i32>(), 0)
                    .unwrap_err()
                    .kind(),
                HostPressureErrorKind::NativeQuery
            );
        }
    }
}
