use agenterm_platform::{
    PlatformKind,
    host_resource_snapshot::{
        HostLoadAverageAvailability, HostResourceSnapshot, HostResourceSnapshotError,
        HostResourceSnapshotErrorKind,
    },
};
use serde_json::{Value, json};

use crate::reply::CuError;

pub(super) fn resource_status_payload() -> Result<Value, CuError> {
    let snapshot =
        agenterm_platform::host_resource_snapshot::snapshot().map_err(resource_snapshot_error)?;
    let process_count = agenterm_platform::process::list()
        .map_err(|error| {
            CuError::new("process_inventory_failed", error.to_string()).with_detail(json!({
                "kind": format!("{:?}", error.kind()),
            }))
        })?
        .len();
    resource_status_from(snapshot, process_count)
}

fn resource_status_from(
    snapshot: HostResourceSnapshot,
    process_count: usize,
) -> Result<Value, CuError> {
    if process_count == 0 {
        return Err(CuError::new(
            "process_inventory_invalid",
            "the complete native process inventory was unexpectedly empty",
        ));
    }
    let platform = match snapshot.platform {
        PlatformKind::Macos => "darwin",
        PlatformKind::Linux => "linux",
        PlatformKind::Windows => "win32",
        _ => {
            return Err(CuError::new(
                "host_resource_invalid",
                "the platform provider returned an unknown host kind",
            ));
        }
    };
    let architecture = match snapshot.architecture {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    let load_semantics = match snapshot.load_average.availability {
        HostLoadAverageAvailability::Available => "unix-getloadavg",
        HostLoadAverageAvailability::UnavailableOnWindows => "windows-not-available",
    };
    Ok(json!({
        "platform": platform,
        "arch": architecture,
        "hostname": snapshot.hostname,
        "uptimeSeconds": snapshot.uptime_milliseconds as f64 / 1000.0,
        "loadAverage": [
            snapshot.load_average.one_minute,
            snapshot.load_average.five_minutes,
            snapshot.load_average.fifteen_minutes,
        ],
        "loadAverageSemantics": load_semantics,
        "cpu": {
            "logical": snapshot.logical_processors,
            "model": snapshot.processor_model,
        },
        "memory": {
            "totalBytes": snapshot.memory.total_physical_bytes,
            "freeBytes": snapshot.memory.free_physical_bytes,
            "availableBytes": snapshot.memory.available_physical_bytes,
            "freeSemantics": snapshot.memory.free_semantics.as_str(),
            "availabilitySemantics": snapshot.memory.availability_semantics.as_str(),
        },
        "processCount": process_count,
        "atomicSnapshot": false,
    }))
}

fn resource_snapshot_error(error: HostResourceSnapshotError) -> CuError {
    let code = match error.kind() {
        HostResourceSnapshotErrorKind::HostnameQuery => "host_identity_query_failed",
        HostResourceSnapshotErrorKind::UptimeQuery => "host_uptime_query_failed",
        HostResourceSnapshotErrorKind::LoadAverageQuery => "host_load_query_failed",
        HostResourceSnapshotErrorKind::ProcessorQuery => "host_processor_query_failed",
        HostResourceSnapshotErrorKind::MemoryQuery => "host_memory_query_failed",
        HostResourceSnapshotErrorKind::InvalidNativeValue => "host_resource_invalid",
        HostResourceSnapshotErrorKind::Overflow => "host_resource_overflow",
        _ => "host_resource_query_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenterm_platform::host_memory::HostMemoryAvailabilitySemantics;
    use agenterm_platform::host_resource_snapshot::{
        HostFreeMemorySemantics, HostLoadAverage, HostResourceMemory,
    };

    fn snapshot(platform: PlatformKind, architecture: &'static str) -> HostResourceSnapshot {
        HostResourceSnapshot {
            platform,
            architecture,
            hostname: "station".into(),
            uptime_milliseconds: 12_345,
            load_average: HostLoadAverage {
                one_minute: 1.0,
                five_minutes: 0.5,
                fifteen_minutes: 0.25,
                availability: HostLoadAverageAvailability::Available,
            },
            logical_processors: 8,
            processor_model: "Example CPU".into(),
            memory: HostResourceMemory {
                total_physical_bytes: 1024,
                free_physical_bytes: 128,
                available_physical_bytes: 512,
                free_semantics: HostFreeMemorySemantics::MacosFreePages,
                availability_semantics: HostMemoryAvailabilitySemantics::MacosFreeAndInactive,
            },
        }
    }

    #[test]
    fn projects_mcu_shape_without_collapsing_memory_semantics() {
        let value = resource_status_from(snapshot(PlatformKind::Macos, "aarch64"), 42).unwrap();
        assert_eq!(value["platform"], "darwin");
        assert_eq!(value["arch"], "arm64");
        assert_eq!(value["uptimeSeconds"], 12.345);
        assert_eq!(value["memory"]["freeBytes"], 128);
        assert_eq!(value["memory"]["availableBytes"], 512);
        assert_eq!(value["processCount"], 42);
        assert_eq!(value["atomicSnapshot"], false);
    }

    #[test]
    fn windows_load_unavailability_is_explicit_and_empty_inventory_fails() {
        let mut value = snapshot(PlatformKind::Windows, "x86_64");
        value.load_average = HostLoadAverage {
            one_minute: 0.0,
            five_minutes: 0.0,
            fifteen_minutes: 0.0,
            availability: HostLoadAverageAvailability::UnavailableOnWindows,
        };
        let payload = resource_status_from(value.clone(), 1).unwrap();
        assert_eq!(payload["platform"], "win32");
        assert_eq!(payload["arch"], "x64");
        assert_eq!(payload["loadAverage"], json!([0.0, 0.0, 0.0]));
        assert_eq!(payload["loadAverageSemantics"], "windows-not-available");
        assert_eq!(
            resource_status_from(value, 0).unwrap_err().code,
            "process_inventory_invalid"
        );
    }
}
