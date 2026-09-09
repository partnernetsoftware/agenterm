//! Linux `org.a11y.Status` observation for `screen-reader`.

#[cfg(target_os = "linux")]
use agenterm_platform::contract::accessibility_tree::AccessibilityTreeError;
use serde_json::{Value, json};

use crate::reply::CuError;

#[cfg(target_os = "linux")]
pub fn status_payload() -> Result<Value, CuError> {
    if std::env::var_os("DISPLAY").is_none() {
        return Err(CuError::new(
            "headless-display",
            "screen-reader status requires a graphical desktop session",
        )
        .with_detail(json!({
            "effect": "not_performed",
            "required_mechanism": "x11-display",
            "alternatives": [
                "export DISPLAY to the active X11 session before calling screen-reader",
            ],
        })));
    }

    match agenterm_platform::accessibility_tree::a11y_bus_readiness() {
        Ok(readiness) => project_readiness(readiness),
        Err(AccessibilityTreeError::Unsupported { reason }) => {
            Err(unsupported_error(reason.into_owned()))
        }
        Err(AccessibilityTreeError::Failed { code, message }) => Err(CuError::new(code, message)
            .with_detail(json!({
                "effect": "not_performed",
                "required_mechanism": "linux-atspi-session-bus",
                "alternatives": alternatives(),
            }))),
        Err(_) => Err(CuError::new(
            "screen_reader_unavailable",
            "screen-reader status probe failed with an unclassified error",
        )
        .with_detail(json!({
            "effect": "not_performed",
            "required_mechanism": "linux-atspi-session-bus",
            "alternatives": alternatives(),
        }))),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn status_payload() -> Result<Value, CuError> {
    Err(unsupported_error(
        "screen-reader status is available only through the Linux AT-SPI session bus".to_owned(),
    ))
}

#[cfg(target_os = "linux")]
fn project_readiness(readiness: Value) -> Result<Value, CuError> {
    let status = readiness["status"].as_str().unwrap_or("unknown");
    match status {
        "ok" | "partial" => Ok(json!({
            "platform": readiness["platform"].clone(),
            "status": status,
            "session_bus": readiness["session_bus"].clone(),
            "a11y_bus": readiness["a11y_bus"].clone(),
            "org_a11y_status": readiness["org_a11y_status"].clone(),
        })),
        "unsupported" => Err(unsupported_error(
            readiness["reason"]
                .as_str()
                .unwrap_or("org.a11y.Status is unavailable on this host")
                .to_owned(),
        )),
        _ => Err(CuError::new(
            "screen_reader_unavailable",
            readiness["repair"]
                .as_str()
                .unwrap_or("org.a11y.Status / AT-SPI bus readiness is unavailable")
                .to_owned(),
        )
        .with_detail(json!({
            "effect": "not_performed",
            "required_mechanism": "linux-atspi-session-bus",
            "readiness": readiness,
            "alternatives": alternatives(),
        }))),
    }
}

fn unsupported_error(reason: String) -> CuError {
    CuError::new("screen_reader_unsupported", reason).with_detail(json!({
        "effect": "not_performed",
        "required_os": "linux",
        "required_mechanism": "linux-atspi-session-bus",
        "alternatives": alternatives(),
    }))
}

fn alternatives() -> Vec<&'static str> {
    vec![
        "export DBUS_SESSION_BUS_ADDRESS and AT_SPI_BUS_ADDRESS for the intended desktop session",
        "run screen-reader and read org_a11y_status.flags",
        "run doctor and read checks.a11y_bus.detail.org_a11y_status.flags",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_detail_carries_alternatives() {
        let error = unsupported_error("fixture".into());
        assert_eq!(error.code, "screen_reader_unsupported");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "not_performed");
        assert_eq!(detail["required_os"], "linux");
        assert!(detail["alternatives"].is_array());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_hosts_refuse_without_linking_the_linux_adapter() {
        let error = status_payload().unwrap_err();
        assert_eq!(error.code, "screen_reader_unsupported");
        assert_eq!(error.detail.unwrap()["required_os"], "linux");
    }
}
