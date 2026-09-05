use super::{
    HostFreeMemorySemantics, HostResourceSnapshotError, HostResourceSnapshotErrorKind,
    NativeSnapshot, available_load, checked_hostname, memory_from_native,
};

pub(super) fn snapshot() -> Result<NativeSnapshot, HostResourceSnapshotError> {
    Ok(NativeSnapshot {
        hostname: hostname()?,
        uptime_milliseconds: uptime_milliseconds()?,
        load_average: load_average()?,
        processor_model: processor_model()?,
        memory: memory_from_native(
            free_physical_bytes()?,
            HostFreeMemorySemantics::MacosFreePages,
        )?,
    })
}

fn hostname() -> Result<String, HostResourceSnapshotError> {
    let mut bytes = [0_u8; 1024];
    if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err(query_error(
            HostResourceSnapshotErrorKind::HostnameQuery,
            "gethostname",
        ));
    }
    let length = bytes.iter().position(|byte| *byte == 0).ok_or_else(|| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "gethostname returned no terminator",
        )
    })?;
    checked_hostname(&bytes[..length])
}

fn uptime_milliseconds() -> Result<u64, HostResourceSnapshotError> {
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
        return Err(query_error(
            HostResourceSnapshotErrorKind::UptimeQuery,
            "sysctl kern.boottime",
        ));
    }
    if length != std::mem::size_of::<libc::timeval>()
        || boot.tv_sec < 0
        || !(0..1_000_000).contains(&boot.tv_usec)
    {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "kern.boottime returned invalid timeval",
        ));
    }
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    if now < boot.tv_sec {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "boot time is later than current time",
        ));
    }
    u64::try_from(now - boot.tv_sec)
        .map_err(|_| overflow("uptime seconds"))?
        .checked_mul(1000)
        .ok_or_else(|| overflow("uptime milliseconds"))
}

fn load_average() -> Result<super::HostLoadAverage, HostResourceSnapshotError> {
    let mut values = [0.0_f64; 3];
    if unsafe { libc::getloadavg(values.as_mut_ptr(), values.len() as libc::c_int) } != 3 {
        return Err(query_error(
            HostResourceSnapshotErrorKind::LoadAverageQuery,
            "getloadavg",
        ));
    }
    available_load(values)
}

fn processor_model() -> Result<String, HostResourceSnapshotError> {
    let mut last_error = None;
    for name in [c"machdep.cpu.brand_string", c"hw.model"] {
        match sysctl_string(name) {
            Ok(value) if !value.is_empty() => return Ok(value),
            Ok(_) => {
                last_error = Some(HostResourceSnapshotError::new(
                    HostResourceSnapshotErrorKind::InvalidNativeValue,
                    "processor model is empty",
                ));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("processor model sources are nonempty"))
}

fn sysctl_string(name: &std::ffi::CStr) -> Result<String, HostResourceSnapshotError> {
    let mut length = 0_usize;
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || length == 0
        || length > 4096
    {
        return Err(query_error(
            HostResourceSnapshotErrorKind::ProcessorQuery,
            "query processor model length",
        ));
    }
    let mut bytes = vec![0_u8; length];
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(query_error(
            HostResourceSnapshotErrorKind::ProcessorQuery,
            "query processor model",
        ));
    }
    bytes.truncate(length);
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    String::from_utf8(bytes).map_err(|_| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "processor model is not UTF-8",
        )
    })
}

fn free_physical_bytes() -> Result<u64, HostResourceSnapshotError> {
    unsafe extern "C" {
        fn mach_host_self() -> libc::mach_port_t;
    }
    let mut statistics = unsafe { std::mem::zeroed::<libc::vm_statistics64_data_t>() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    let status = unsafe {
        libc::host_statistics64(
            mach_host_self(),
            libc::HOST_VM_INFO64,
            (&raw mut statistics).cast(),
            &mut count,
        )
    };
    if status != libc::KERN_SUCCESS {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::MemoryQuery,
            format!("host_statistics64 returned {status}"),
        ));
    }
    let page_size = crate::host_memory::facts()
        .map_err(|error| {
            HostResourceSnapshotError::new(
                HostResourceSnapshotErrorKind::MemoryQuery,
                error.to_string(),
            )
        })?
        .page_size
        .get() as u64;
    u64::from(statistics.free_count)
        .checked_mul(page_size)
        .ok_or_else(|| overflow("free physical bytes"))
}

fn query_error(kind: HostResourceSnapshotErrorKind, operation: &str) -> HostResourceSnapshotError {
    HostResourceSnapshotError::new(
        kind,
        format!("{operation}: {}", std::io::Error::last_os_error()),
    )
}

fn overflow(field: &str) -> HostResourceSnapshotError {
    HostResourceSnapshotError::new(
        HostResourceSnapshotErrorKind::Overflow,
        format!("{field} overflowed"),
    )
}
