//! Linux XKB keyboard layout observation for `keyboard-layout`.

use agenterm_platform::keyboard_layout::{
    KeyboardLayoutObservation, KeyboardLayoutObserveResult, KeyboardLayoutObserveUnsupported,
};
use serde_json::{Value, json};

use crate::reply::CuError;

pub fn status_payload() -> Result<Value, CuError> {
    if std::env::var_os("DISPLAY").is_none() {
        return Err(CuError::new(
            "headless-display",
            "keyboard layout observation requires a graphical desktop session",
        )
        .with_detail(json!({
            "effect": "not_performed",
            "required_mechanism": "x11-display",
            "alternatives": [
                "export DISPLAY to the active X11 session before calling keyboard-layout",
            ],
        })));
    }

    match agenterm_platform::keyboard_layout::observe_host() {
        KeyboardLayoutObserveResult::Ok(observation) => Ok(project_observation(observation)),
        KeyboardLayoutObserveResult::Unsupported(unsupported) => {
            Err(unsupported_error(unsupported))
        }
    }
}

fn project_observation(observation: KeyboardLayoutObservation) -> Value {
    json!({
        "provider": observation.provider,
        "rules": observation.rules,
        "model": observation.model,
        "layout": observation.layout,
        "variant": observation.variant,
        "id": observation.id,
        "name": observation.name,
    })
}

fn unsupported_error(unsupported: KeyboardLayoutObserveUnsupported) -> CuError {
    CuError::new("keyboard_layout_unsupported", unsupported.reason).with_detail(json!({
        "effect": "not_performed",
        "required_mechanism": unsupported.required_mechanism,
        "alternatives": unsupported.alternatives,
    }))
}
