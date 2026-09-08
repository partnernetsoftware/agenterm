//! Typed host/OS limit errors: `detail.os`, required provider, and honest alternatives.

use crate::reply::CuError;

#[derive(Clone, Copy)]
struct HostLimitDetail {
    group: Option<&'static str>,
    provider: Option<&'static str>,
    required_os: Option<&'static str>,
    mechanism: Option<&'static str>,
    alternatives: &'static [&'static str],
}

fn host_limit_error(
    code: &str,
    message: impl Into<String>,
    detail: HostLimitDetail,
) -> CuError {
    let mut json = serde_json::json!({
        "os": crate::mcu_surface::host_os(),
        "limit": "host",
    });
    if let Some(group) = detail.group {
        json["group"] = serde_json::json!(group);
    }
    if let Some(provider) = detail.provider {
        json["provider"] = serde_json::json!(provider);
    }
    if let Some(required_os) = detail.required_os {
        json["required_os"] = serde_json::json!(required_os);
    }
    if let Some(mechanism) = detail.mechanism {
        json["mechanism"] = serde_json::json!(mechanism);
    }
    if !detail.alternatives.is_empty() {
        json["alternatives"] = serde_json::json!(detail.alternatives);
    }
    CuError::new(code, message).with_detail(json)
}

pub(crate) fn spaces_unsupported() -> CuError {
    host_limit_error(
        "unsupported",
        format!(
            "managed Space inventory is a macOS SkyLight host limit; {} has no equivalent Space model",
            crate::mcu_surface::host_os()
        ),
        HostLimitDetail {
            group: Some("geometry"),
            provider: Some("none"),
            required_os: Some("macos"),
            mechanism: Some("skylight-private-read"),
            alternatives: &[
                "displays (screen enumeration and work areas)",
                "windows / window-place (per-window geometry without Space ids)",
            ],
        },
    )
}

pub(crate) fn audio_unsupported() -> CuError {
    host_limit_error(
        "audio_unsupported",
        format!(
            "default-output volume and mute observation require macOS CoreAudio; {} does not expose that provider",
            crate::mcu_surface::host_os()
        ),
        HostLimitDetail {
            group: Some("system"),
            provider: Some("none"),
            required_os: Some("macos"),
            mechanism: Some("coreaudio-default-output"),
            alternatives: &[],
        },
    )
}

pub(crate) fn login_session_unsupported() -> CuError {
    host_limit_error(
        "login_session_unsupported",
        format!(
            "console login-session inventory and screen-lock delivery require macOS IORegistry integration; {} has no mapped provider",
            crate::mcu_surface::host_os()
        ),
        HostLimitDetail {
            group: Some("system"),
            provider: Some("none"),
            required_os: Some("macos"),
            mechanism: Some("macos-io-registry"),
            alternatives: &[
                "session-list (AgenTerm runtime sessions when agenterm server is running)",
                "resource-status / power-status (host facts; not OS screen lock)",
            ],
        },
    )
}

pub(crate) fn simulator_unsupported(verb: &str) -> CuError {
    let message = match verb {
        "simulator-launch" | "simulator-terminate" | "simulator-boot" => format!(
            "iOS Simulator app lifecycle requires macOS CoreSimulator (simctl); {} cannot run simctl",
            crate::mcu_surface::host_os()
        ),
        _ => format!(
            "iOS Simulator device inventory requires macOS CoreSimulator (simctl); {} cannot run simctl",
            crate::mcu_surface::host_os()
        ),
    };
    host_limit_error(
        "simulator_unsupported",
        message,
        HostLimitDetail {
            group: Some("simulator"),
            provider: Some("none"),
            required_os: Some("macos"),
            mechanism: Some("core-simulator"),
            alternatives: &[
                "ps / process-state (host processes only; not Simulator apps)",
                "host-open (open URLs or documents on the host where mapped)",
            ],
        },
    )
}

pub(crate) fn application_hide_unsupported(reason: impl Into<String>) -> CuError {
    host_limit_error(
        "unsupported",
        reason,
        HostLimitDetail {
            group: Some("process"),
            provider: Some("none"),
            required_os: None,
            mechanism: Some("application-set-hidden"),
            alternatives: &[
                "window-place minimize / unminimize where the host maps native window state",
                "close or app quit when teardown is acceptable",
            ],
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_limit_detail_includes_os_and_alternatives() {
        let error = spaces_unsupported();
        assert_eq!(error.code, "unsupported");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["os"], crate::mcu_surface::host_os());
        assert_eq!(detail["limit"], "host");
        assert_eq!(detail["required_os"], "macos");
        assert!(detail["alternatives"].as_array().is_some_and(|a| !a.is_empty()));
    }

    #[test]
    fn audio_unsupported_is_typed_without_fake_alternatives() {
        let error = audio_unsupported();
        assert_eq!(error.code, "audio_unsupported");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["os"], crate::mcu_surface::host_os());
        assert!(
            detail.get("alternatives").is_none()
                || detail["alternatives"].as_array().is_some_and(|a| a.is_empty())
        );
    }

    #[test]
    fn login_session_message_avoids_display_wrapper_noise() {
        let error = login_session_unsupported();
        assert_eq!(error.code, "login_session_unsupported");
        assert!(!error.message.contains("Unsupported:"));
    }
}
