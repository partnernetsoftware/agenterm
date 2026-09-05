use super::{
    HostFreeMemorySemantics, HostResourceSnapshotError, HostResourceSnapshotErrorKind,
    NativeSnapshot, available_load, checked_hostname, memory_from_native,
};

pub(super) fn snapshot() -> Result<NativeSnapshot, HostResourceSnapshotError> {
    let hostname = hostname()?;
    let uptime_milliseconds = uptime_milliseconds()?;
    let load_average = load_average()?;
    let processor_model = processor_model()?;
    let free_physical_bytes = mem_free_bytes()?;
    let memory = memory_from_native(free_physical_bytes, HostFreeMemorySemantics::LinuxMemFree)?;
    Ok(NativeSnapshot {
        hostname,
        uptime_milliseconds,
        load_average,
        processor_model,
        memory,
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
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &raw mut time) } != 0 {
        return Err(query_error(
            HostResourceSnapshotErrorKind::UptimeQuery,
            "clock_gettime(CLOCK_BOOTTIME)",
        ));
    }
    if time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "CLOCK_BOOTTIME returned invalid timespec",
        ));
    }
    let seconds = u64::try_from(time.tv_sec).map_err(|_| overflow("uptime seconds"))?;
    seconds
        .checked_mul(1000)
        .and_then(|value| value.checked_add(u64::try_from(time.tv_nsec).ok()? / 1_000_000))
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
    let contents = std::fs::read_to_string("/proc/cpuinfo").map_err(|error| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::ProcessorQuery,
            format!("read /proc/cpuinfo: {error}"),
        )
    })?;
    Ok(parse_processor_model(&contents).unwrap_or_else(|| "unknown".to_owned()))
}

fn parse_processor_model(contents: &str) -> Option<String> {
    for key in ["model name", "Hardware", "Processor"] {
        if let Some(value) = contents.lines().find_map(|line| {
            let (candidate, value) = line.split_once(':')?;
            (candidate.trim() == key)
                .then(|| value.trim())
                .filter(|value| !value.is_empty())
        }) {
            return Some(value.chars().take(1024).collect());
        }
    }
    None
}

fn mem_free_bytes() -> Result<u64, HostResourceSnapshotError> {
    let contents = std::fs::read_to_string("/proc/meminfo").map_err(|error| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::MemoryQuery,
            format!("read /proc/meminfo: {error}"),
        )
    })?;
    let line = contents
        .lines()
        .find(|line| line.starts_with("MemFree:"))
        .ok_or_else(|| {
            HostResourceSnapshotError::new(
                HostResourceSnapshotErrorKind::InvalidNativeValue,
                "/proc/meminfo has no MemFree",
            )
        })?;
    let mut fields = line.split_ascii_whitespace();
    if fields.next() != Some("MemFree:") {
        unreachable!();
    }
    let kibibytes = fields.next().and_then(|value| value.parse::<u64>().ok());
    if fields.next() != Some("kB") || fields.next().is_some() {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "invalid MemFree shape",
        ));
    }
    kibibytes
        .and_then(|value| value.checked_mul(1024))
        .ok_or_else(|| overflow("MemFree bytes"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_model_parser_has_bounded_fallbacks() {
        assert_eq!(
            parse_processor_model("processor: 0\nmodel name : Example CPU\n").as_deref(),
            Some("Example CPU")
        );
        assert_eq!(
            parse_processor_model("Hardware: Board CPU\n").as_deref(),
            Some("Board CPU")
        );
        assert_eq!(parse_processor_model("processor: 0\n"), None);
    }
}
