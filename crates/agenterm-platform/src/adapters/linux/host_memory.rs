use std::io::Read;

use crate::contract::host_memory::{
    HostMemoryAvailability, HostMemoryAvailabilitySemantics, HostMemoryError, HostMemoryErrorKind,
    HostMemoryFacts, checked_availability, checked_facts,
};

const MEMINFO_BYTE_CEILING: usize = 64 * 1024;

pub(crate) fn facts() -> Result<HostMemoryFacts, HostMemoryError> {
    let page_size = positive_sysconf(libc::_SC_PAGESIZE, "page size")?;
    let meminfo = read_meminfo()?;
    let physical_bytes = meminfo_kibibytes(&meminfo, "MemTotal:")?;
    checked_facts(page_size, page_size, physical_bytes)
}

pub(crate) fn availability() -> Result<HostMemoryAvailability, HostMemoryError> {
    let meminfo = read_meminfo()?;
    let available_physical_bytes = meminfo_kibibytes(&meminfo, "MemAvailable:")?;
    let total_physical_bytes = meminfo_kibibytes(&meminfo, "MemTotal:")?;
    checked_availability(
        available_physical_bytes,
        total_physical_bytes,
        HostMemoryAvailabilitySemantics::LinuxMemAvailable,
    )
}

pub(crate) fn observed() -> Result<(HostMemoryFacts, HostMemoryAvailability), HostMemoryError> {
    let page_size = positive_sysconf(libc::_SC_PAGESIZE, "page size")?;
    let meminfo = read_meminfo()?;
    let physical_bytes = meminfo_kibibytes(&meminfo, "MemTotal:")?;
    let available_physical_bytes = meminfo_kibibytes(&meminfo, "MemAvailable:")?;
    let facts = checked_facts(page_size, page_size, physical_bytes)?;
    let availability = checked_availability(
        available_physical_bytes,
        physical_bytes,
        HostMemoryAvailabilitySemantics::LinuxMemAvailable,
    )?;
    Ok((facts, availability))
}

pub(crate) fn read_meminfo() -> Result<String, HostMemoryError> {
    let file = std::fs::File::open("/proc/meminfo").map_err(|error| {
        HostMemoryError::new(
            HostMemoryErrorKind::Query,
            format!("open /proc/meminfo: {error}"),
        )
    })?;
    let mut bytes = Vec::with_capacity(MEMINFO_BYTE_CEILING.min(4096));
    file.take((MEMINFO_BYTE_CEILING + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            HostMemoryError::new(
                HostMemoryErrorKind::Query,
                format!("read /proc/meminfo: {error}"),
            )
        })?;
    if bytes.len() > MEMINFO_BYTE_CEILING {
        return Err(HostMemoryError::new(
            HostMemoryErrorKind::Query,
            "/proc/meminfo exceeds 64 KiB",
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        HostMemoryError::new(
            HostMemoryErrorKind::InvalidValue,
            "/proc/meminfo is not UTF-8",
        )
    })
}

pub(crate) fn meminfo_kibibytes(meminfo: &str, key: &str) -> Result<u64, HostMemoryError> {
    let line = meminfo
        .lines()
        .find(|line| line.starts_with(key))
        .ok_or_else(|| {
            HostMemoryError::new(
                HostMemoryErrorKind::InvalidValue,
                format!("/proc/meminfo does not contain {key}"),
            )
        })?;
    let mut fields = line.split_ascii_whitespace();
    let parsed_key = fields.next();
    let kibibytes = fields.next().and_then(|value| value.parse::<u64>().ok());
    let unit = fields.next();
    if parsed_key != Some(key) || unit != Some("kB") || fields.next().is_some() {
        return Err(HostMemoryError::new(
            HostMemoryErrorKind::InvalidValue,
            format!("invalid {key} line: {line}"),
        ));
    }
    kibibytes
        .ok_or_else(|| {
            HostMemoryError::new(
                HostMemoryErrorKind::InvalidValue,
                format!("invalid {key} value: {line}"),
            )
        })?
        .checked_mul(1024)
        .ok_or_else(|| {
            HostMemoryError::new(
                HostMemoryErrorKind::Overflow,
                format!("{key} kibibytes overflowed u64 bytes"),
            )
        })
}

fn positive_sysconf(key: libc::c_int, name: &str) -> Result<u64, HostMemoryError> {
    let value = unsafe { libc::sysconf(key) };
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            HostMemoryError::new(
                HostMemoryErrorKind::InvalidValue,
                format!("host reported invalid {name}: {value}"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_parser_requires_the_kernel_unit_and_shape() {
        let meminfo = "MemTotal: 10 kB\nMemAvailable: 7 kB\nMemFree: 3 kB\n";
        assert_eq!(meminfo_kibibytes(meminfo, "MemTotal:").unwrap(), 10 * 1024);
        assert_eq!(
            meminfo_kibibytes(meminfo, "MemAvailable:").unwrap(),
            7 * 1024
        );
        assert_eq!(meminfo_kibibytes(meminfo, "MemFree:").unwrap(), 3 * 1024);
        for (key, input) in [
            ("MemAvailable:", "MemTotal: 10 kB\n"),
            ("MemAvailable:", "MemAvailable: nope kB\n"),
            ("MemAvailable:", "MemAvailable: 7 MB\n"),
            ("MemAvailable:", "MemAvailable: 7 kB extra\n"),
            ("MemTotal:", "MemAvailable: 7 kB\n"),
            ("MemFree:", "MemFree: 7 MB\n"),
        ] {
            assert_eq!(
                meminfo_kibibytes(input, key).unwrap_err().kind(),
                HostMemoryErrorKind::InvalidValue
            );
        }
    }

    #[test]
    fn availability_uses_one_meminfo_total_and_available_pair() {
        let value = availability_from_meminfo("MemTotal: 10 kB\nMemAvailable: 7 kB\n").unwrap();
        assert_eq!(value.available_physical_bytes, 7 * 1024);
        assert_eq!(
            value.semantics,
            HostMemoryAvailabilitySemantics::LinuxMemAvailable
        );
    }

    fn availability_from_meminfo(meminfo: &str) -> Result<HostMemoryAvailability, HostMemoryError> {
        let available_physical_bytes = meminfo_kibibytes(meminfo, "MemAvailable:")?;
        let total_physical_bytes = meminfo_kibibytes(meminfo, "MemTotal:")?;
        checked_availability(
            available_physical_bytes,
            total_physical_bytes,
            HostMemoryAvailabilitySemantics::LinuxMemAvailable,
        )
    }
}
