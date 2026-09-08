//! Linux `org.a11y.Status` / AT-SPI bus readiness for `doctor` and `unlock`.

use serde_json::json;

fn unsupported_readiness(reason: String) -> serde_json::Value {
    serde_json::json!({
        "status": "unsupported",
        "platform": crate::mcu_surface::host_os(),
        "reason": reason,
        "alternatives": a11y_bus_alternatives(),
    })
}

fn a11y_bus_alternatives() -> Vec<&'static str> {
    if cfg!(target_os = "linux") {
        vec![
            "export DBUS_SESSION_BUS_ADDRESS and AT_SPI_BUS_ADDRESS for the intended desktop session",
            "run doctor and read checks.a11y_bus.detail.org_a11y_status.flags",
            "run unlock --window HANDLE and read a11y_status_before / a11y_status_after",
        ]
    } else if cfg!(target_os = "macos") {
        vec![
            "run unlock --window HANDLE (AXManualAccessibility poke) and re-read the tree",
            "run permissions status for the macOS accessibility grant",
        ]
    } else if cfg!(target_os = "windows") {
        vec![
            "run unlock --window HANDLE; on Windows the UIA tree walk itself is the Chromium poke",
            "read unlock poked:false with the Windows reason, then compare returned_before/after",
        ]
    } else {
        vec!["run doctor on a supported desktop host"]
    }
}

fn map_platform_readiness(
    result: Result<
        serde_json::Value,
        agenterm_platform::contract::accessibility_tree::AccessibilityTreeError,
    >,
) -> serde_json::Value {
    use agenterm_platform::contract::accessibility_tree::AccessibilityTreeError;
    match result {
        Ok(value) => value,
        Err(AccessibilityTreeError::Unsupported { reason }) => {
            unsupported_readiness(reason.into_owned())
        }
        Err(AccessibilityTreeError::Failed { code, message }) => json!({
            "status": "failed",
            "platform": crate::mcu_surface::host_os(),
            "error": { "code": code, "message": message },
            "alternatives": a11y_bus_alternatives(),
        }),
        Err(_) => json!({
            "status": "failed",
            "platform": crate::mcu_surface::host_os(),
            "error": {
                "code": "a11y_bus_readiness_failed",
                "message": "accessibility readiness probe failed with an unclassified error",
            },
            "alternatives": a11y_bus_alternatives(),
        }),
    }
}

/// Zero-write readiness projection shared by `doctor` and `unlock`.
pub(super) fn a11y_bus_readiness_json() -> serde_json::Value {
    #[cfg(target_os = "linux")]
    {
        map_platform_readiness(agenterm_platform::accessibility_tree::a11y_bus_readiness())
    }
    #[cfg(not(target_os = "linux"))]
    {
        map_platform_readiness(Err(
            agenterm_platform::contract::accessibility_tree::AccessibilityTreeError::Unsupported {
                reason: format!(
                    "org.a11y.Status is a Linux AT-SPI session-bus switch; on {} use unlock/doctor host guidance instead",
                    crate::mcu_surface::host_os()
                )
                .into(),
            },
        ))
    }
}

/// `doctor` check wrapper: informational on Linux, typed unsupported elsewhere.
pub(super) fn doctor_a11y_bus_check() -> serde_json::Value {
    let detail = a11y_bus_readiness_json();
    let status = detail["status"].as_str().unwrap_or("unknown");
    let check_status = match status {
        "ok" | "partial" => "available",
        "unsupported" => "not-applicable",
        _ => "failed",
    };
    serde_json::json!({
        "required": cfg!(target_os = "linux"),
        "status": check_status,
        "detail": detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_hosts_name_alternatives() {
        let value = unsupported_readiness("fixture".into());
        assert_eq!(value["status"], "unsupported");
        assert!(value["alternatives"].as_array().unwrap().len() >= 1);
    }
}
