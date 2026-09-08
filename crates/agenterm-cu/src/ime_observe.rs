//! Linux session input-method observation for `ime-status`.

use agenterm_platform::ime::{ImeHostObservation, ImeObserveResult, ImeObserveUnsupported};
use serde_json::{Value, json};

use crate::reply::CuError;

pub fn status_payload() -> Result<Value, CuError> {
    if std::env::var_os("DISPLAY").is_none() {
        return Err(
            CuError::new(
                "headless-display",
                "ime status requires a graphical desktop session",
            )
            .with_detail(json!({
                "effect": "not_performed",
                "required_mechanism": "x11-display",
                "alternatives": [
                    "export DISPLAY to the active X11 session before calling ime-status",
                ],
            })),
        );
    }

    match agenterm_platform::ime::observe_host() {
        ImeObserveResult::Ok(observation) => Ok(project_observation(observation)),
        ImeObserveResult::Unsupported(unsupported) => {
            Err(unsupported_error(unsupported))
        }
    }
}

fn project_observation(observation: ImeHostObservation) -> Value {
    json!({
        "provider": observation.provider,
        "framework": observation.framework,
        "name": observation.name,
        "available": observation.available,
        "open": observation.open,
        "native_mode": observation.native_mode,
        "full_shape": observation.full_shape,
        "label": observation.label,
        "env": observation.env,
        "session_bus_available": observation.session_bus_available,
    })
}

fn unsupported_error(unsupported: ImeObserveUnsupported) -> CuError {
    CuError::new("ime_unsupported", unsupported.reason).with_detail(json!({
        "effect": "not_performed",
        "required_mechanism": unsupported.required_mechanism,
        "env": unsupported.env,
        "probed_frameworks": unsupported.probed_frameworks,
        "session_bus_available": unsupported.session_bus_available,
        "alternatives": unsupported.alternatives,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_detail_carries_alternatives() {
        let error = unsupported_error(ImeObserveUnsupported {
            reason: "no framework".into(),
            env: agenterm_platform::ime::ImeEnvSnapshot::default(),
            probed_frameworks: vec!["ibus".into()],
            session_bus_available: true,
            required_mechanism: "linux-session-input-method".into(),
            alternatives: vec!["install ibus".into()],
        });
        assert_eq!(error.code, "ime_unsupported");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "not_performed");
        assert!(detail["alternatives"].is_array());
    }
}
