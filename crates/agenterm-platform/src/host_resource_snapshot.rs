//! Current-host CPU, memory, identity and uptime observation.
//!
//! Process inventory is deliberately not composed here: callers that need a
//! process count should combine this snapshot with the independently typed
//! `process::list` result rather than making this feature depend on process
//! inspection.

#[path = "contract/host_resource_snapshot.rs"]
mod contract;

pub use contract::{
    HostFreeMemorySemantics, HostLoadAverage, HostLoadAverageAvailability, HostResourceMemory,
    HostResourceSnapshot, HostResourceSnapshotError, HostResourceSnapshotErrorKind,
};

#[cfg(target_os = "linux")]
#[path = "adapters/linux/host_resource_snapshot.rs"]
mod adapter;
#[cfg(target_os = "macos")]
#[path = "adapters/macos/host_resource_snapshot.rs"]
mod adapter;
#[cfg(windows)]
#[path = "adapters/windows/host_resource_snapshot.rs"]
mod adapter;

pub fn snapshot() -> Result<HostResourceSnapshot, HostResourceSnapshotError> {
    let native = adapter::snapshot()?;
    let topology = crate::processor_topology::facts().map_err(|error| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::ProcessorQuery,
            error.to_string(),
        )
    })?;
    Ok(HostResourceSnapshot {
        platform: crate::platform_kind(),
        architecture: std::env::consts::ARCH,
        hostname: native.hostname,
        uptime_milliseconds: native.uptime_milliseconds,
        load_average: native.load_average,
        logical_processors: topology.system_logical_processors.get(),
        processor_model: native.processor_model,
        memory: native.memory,
    })
}

pub(super) struct NativeSnapshot {
    hostname: String,
    uptime_milliseconds: u64,
    load_average: HostLoadAverage,
    processor_model: String,
    memory: HostResourceMemory,
}

fn memory_from_native(
    free_physical_bytes: u64,
    semantics: HostFreeMemorySemantics,
) -> Result<HostResourceMemory, HostResourceSnapshotError> {
    let facts = crate::host_memory::facts().map_err(|error| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::MemoryQuery,
            error.to_string(),
        )
    })?;
    let available = crate::host_memory::availability().map_err(|error| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::MemoryQuery,
            error.to_string(),
        )
    })?;
    contract::checked_memory(
        facts.physical_bytes.get(),
        free_physical_bytes,
        available.available_physical_bytes,
        semantics,
        available.semantics,
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn available_load(values: [f64; 3]) -> Result<HostLoadAverage, HostResourceSnapshotError> {
    contract::checked_load_average(values, HostLoadAverageAvailability::Available)
}

#[cfg(windows)]
fn unavailable_windows_load() -> HostLoadAverage {
    contract::checked_load_average([0.0; 3], HostLoadAverageAvailability::UnavailableOnWindows)
        .expect("constant unavailable load is valid")
}

fn checked_hostname(bytes: &[u8]) -> Result<String, HostResourceSnapshotError> {
    let hostname = std::str::from_utf8(bytes).map_err(|_| {
        HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "hostname is not UTF-8",
        )
    })?;
    if hostname.is_empty() || hostname.len() > 1024 || hostname.chars().any(char::is_control) {
        return Err(HostResourceSnapshotError::new(
            HostResourceSnapshotErrorKind::InvalidNativeValue,
            "hostname is empty, oversized, or contains control characters",
        ));
    }
    Ok(hostname.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_host_snapshot_is_coherent() {
        let value = snapshot().expect("current host resource snapshot");
        assert!(!value.hostname.is_empty());
        assert!(value.logical_processors > 0);
        assert!(!value.processor_model.is_empty());
        // Free and available are separately sampled native observations; each
        // is bounded by total, but memory activity may cross them between calls.
        assert!(value.memory.free_physical_bytes <= value.memory.total_physical_bytes);
        assert!(value.memory.available_physical_bytes <= value.memory.total_physical_bytes);
    }

    #[test]
    fn hostname_validation_is_closed() {
        assert_eq!(
            checked_hostname(b"").unwrap_err().kind(),
            HostResourceSnapshotErrorKind::InvalidNativeValue
        );
        assert_eq!(
            checked_hostname(&[0xff]).unwrap_err().kind(),
            HostResourceSnapshotErrorKind::InvalidNativeValue
        );
        assert!(checked_hostname(b"station").is_ok());
    }
}
