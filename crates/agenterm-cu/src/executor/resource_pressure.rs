use agenterm_platform::host_pressure::{
    HostPressureError, HostPressureErrorKind, HostPressureSignal, HostPressureSnapshot,
    LinuxPsiWindow,
};
use serde_json::{Value, json};

use crate::reply::CuError;

pub(super) fn resource_pressure_payload() -> Result<Value, CuError> {
    let snapshot = agenterm_platform::host_pressure::snapshot().map_err(pressure_error)?;
    Ok(resource_pressure_from(snapshot))
}

fn resource_pressure_from(snapshot: HostPressureSnapshot) -> Value {
    json!({
        "provider": provider_name(snapshot),
        "derived": false,
        "atomicSnapshot": false,
        "memory": signal_json(snapshot.memory),
        "cpu": signal_json(snapshot.cpu),
        "io": signal_json(snapshot.io),
    })
}

fn provider_name(snapshot: HostPressureSnapshot) -> &'static str {
    match snapshot.memory {
        HostPressureSignal::LinuxPsi { .. } => "linux-proc-pressure",
        HostPressureSignal::MacosVmPressure { .. } => "macos-vm-memory-pressure",
        HostPressureSignal::WindowsMemoryResourceNotification { .. } => {
            "windows-memory-resource-notification"
        }
        HostPressureSignal::Unavailable { .. } => "unavailable",
    }
}

fn signal_json(signal: HostPressureSignal) -> Value {
    match signal {
        HostPressureSignal::Unavailable { reason } => json!({
            "available": false,
            "semantics": signal.semantics(),
            "reason": reason.as_str(),
        }),
        HostPressureSignal::LinuxPsi { some, full } => json!({
            "available": true,
            "semantics": signal.semantics(),
            "some": psi_window_json(some),
            "full": full.map(psi_window_json),
        }),
        HostPressureSignal::MacosVmPressure { raw_level } => json!({
            "available": true,
            "semantics": signal.semantics(),
            "rawLevel": raw_level,
        }),
        HostPressureSignal::WindowsMemoryResourceNotification {
            low_memory,
            high_memory,
        } => json!({
            "available": true,
            "semantics": signal.semantics(),
            "lowMemory": low_memory,
            "highMemory": high_memory,
        }),
    }
}

fn psi_window_json(window: LinuxPsiWindow) -> Value {
    json!({
        "avg10": window.average_10_seconds,
        "avg60": window.average_60_seconds,
        "avg300": window.average_300_seconds,
        "totalStallMicroseconds": window.total_stall_microseconds.to_string(),
    })
}

fn pressure_error(error: HostPressureError) -> CuError {
    let code = match error.kind() {
        HostPressureErrorKind::ProviderUnavailable => "host_pressure_provider_unavailable",
        HostPressureErrorKind::NativeQuery => "host_pressure_query_failed",
        HostPressureErrorKind::MalformedNativeData => "host_pressure_malformed",
        HostPressureErrorKind::InvalidNativeValue => "host_pressure_invalid",
        _ => "host_pressure_query_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenterm_platform::host_pressure::HostPressureUnavailableReason;

    #[test]
    fn linux_projection_keeps_native_windows_and_wide_total() {
        let some = LinuxPsiWindow {
            average_10_seconds: 1.25,
            average_60_seconds: 2.5,
            average_300_seconds: 3.75,
            total_stall_microseconds: u64::MAX,
        };
        let unavailable = HostPressureSignal::Unavailable {
            reason: HostPressureUnavailableReason::NotPublishedByPlatform,
        };
        let value = resource_pressure_from(HostPressureSnapshot {
            memory: HostPressureSignal::LinuxPsi { some, full: None },
            cpu: HostPressureSignal::LinuxPsi {
                some,
                full: Some(some),
            },
            io: unavailable,
        });
        assert_eq!(value["derived"], false);
        assert_eq!(value["memory"]["some"]["avg10"], 1.25);
        assert_eq!(
            value["memory"]["some"]["totalStallMicroseconds"],
            u64::MAX.to_string()
        );
        assert_eq!(value["io"]["available"], false);
    }

    #[test]
    fn platform_projections_do_not_invent_shared_levels() {
        let unavailable = HostPressureSignal::Unavailable {
            reason: HostPressureUnavailableReason::NotPublishedByPlatform,
        };
        let mac = resource_pressure_from(HostPressureSnapshot {
            memory: HostPressureSignal::MacosVmPressure { raw_level: 7 },
            cpu: unavailable,
            io: unavailable,
        });
        assert_eq!(mac["memory"]["rawLevel"], 7);
        assert!(mac["memory"].get("level").is_none());

        let windows = resource_pressure_from(HostPressureSnapshot {
            memory: HostPressureSignal::WindowsMemoryResourceNotification {
                low_memory: false,
                high_memory: true,
            },
            cpu: unavailable,
            io: unavailable,
        });
        assert_eq!(windows["memory"]["lowMemory"], false);
        assert_eq!(windows["memory"]["highMemory"], true);
    }

    #[test]
    fn unavailable_memory_does_not_masquerade_as_linux() {
        let unavailable = HostPressureSignal::Unavailable {
            reason: HostPressureUnavailableReason::NotPublishedByPlatform,
        };
        let value = resource_pressure_from(HostPressureSnapshot {
            memory: unavailable,
            cpu: unavailable,
            io: unavailable,
        });
        assert_eq!(value["provider"], "unavailable");
    }
}
