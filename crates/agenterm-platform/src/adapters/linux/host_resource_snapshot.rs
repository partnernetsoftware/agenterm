use crate::contract::host_memory::HostMemoryErrorKind;

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
    let contents = std::fs::read_to_string("/proc/uptime").map_err(|error| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::UptimeQuery,
            format!("read /proc/uptime: {error}"),
        )
    })?;
    let seconds_text = contents.split_whitespace().next().ok_or_else(|| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "/proc/uptime missing uptime field",
        )
    })?;
    parse_uptime_seconds(seconds_text)
}

fn parse_uptime_seconds(seconds_text: &str) -> Result<u64, HostResourceSnapshotError> {
    let seconds = seconds_text.parse::<f64>().map_err(|_| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "/proc/uptime uptime field is not a number",
        )
    })?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "/proc/uptime uptime field is out of range",
        ));
    }
    let millis = (seconds * 1000.0).round();
    if millis < 0.0 || millis > u64::MAX as f64 {
        return Err(overflow("uptime milliseconds"));
    }
    Ok(millis as u64)
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
    let meminfo = crate::selected::host_memory::read_meminfo().map_err(memory_query_error)?;
    crate::selected::host_memory::meminfo_kibibytes(&meminfo, "MemFree:").map_err(|error| {
        HostResourceSnapshotError::new(
            match error.kind() {
                HostMemoryErrorKind::InvalidValue => {
                    HostResourceSnapshotErrorKind::InvalidNativeValue
                }
                HostMemoryErrorKind::Overflow => HostResourceSnapshotErrorKind::Overflow,
                _ => HostResourceSnapshotErrorKind::MemoryQuery,
            },
            error.to_string(),
        )
    })
}

fn memory_query_error(
    error: crate::contract::host_memory::HostMemoryError,
) -> HostResourceSnapshotError {
    HostResourceSnapshotError::new(
        HostResourceSnapshotErrorKind::MemoryQuery,
        error.to_string(),
    )
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
    fn uptime_parser_rejects_invalid_proc_uptime() {
        assert_eq!(
            parse_uptime_seconds("not-a-number").unwrap_err().kind(),
            HostResourceSnapshotErrorKind::InvalidNativeValue
        );
        assert_eq!(
            parse_uptime_seconds("").unwrap_err().kind(),
            HostResourceSnapshotErrorKind::InvalidNativeValue
        );
        assert_eq!(parse_uptime_seconds("12.5").unwrap(), 12_500);
    }

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
