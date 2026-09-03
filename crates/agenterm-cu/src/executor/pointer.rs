//! `pointer-move` / `pointer-position`: absolute-screen pointer verbs.

use super::*;

pub(super) fn pointer_move(x: i32, y: i32) -> Result<serde_json::Value, CuError> {
    pointer_move_with(x, y, |x, y| {
        mechanism::input_inject::pointer_move(x, y).map_err(map_mechanism_err)
    })
}

pub(super) fn pointer_position() -> Result<serde_json::Value, CuError> {
    pointer_position_with(|| mechanism::input_inject::pointer_position().map_err(map_mechanism_err))
}

pub(super) fn pointer_position_with(
    observe_once: impl FnOnce() -> Result<(i32, i32), CuError>,
) -> Result<serde_json::Value, CuError> {
    let (x, y) = observe_once()?;
    Ok(serde_json::json!({
        "effect": "observed",
        "addressing": "absolute-screen-coordinates",
        "coords": [x, y],
        "mechanism": "libagenterm",
    }))
}

pub(super) fn pointer_move_with(
    x: i32,
    y: i32,
    move_once: impl FnOnce(i32, i32) -> Result<(), CuError>,
) -> Result<serde_json::Value, CuError> {
    move_once(x, y)?;
    Ok(serde_json::json!({
        "effect": "committed",
        "addressing": "absolute-screen-coordinates",
        "coords": [x, y],
        "mechanism": "libagenterm",
        "button_effect": "none",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_move_calls_only_move_once_and_returns_bounded_typed_reply() {
        let mut calls = Vec::new();
        let reply = pointer_move_with(-320, 1440, |x, y| {
            calls.push((x, y));
            Ok(())
        })
        .expect("pointer move");
        assert_eq!(calls, [(-320, 1440)]);
        assert_eq!(reply["effect"], "committed");
        assert_eq!(reply["coords"], serde_json::json!([-320, 1440]));
        assert_eq!(reply["button_effect"], "none");
        assert_eq!(reply.as_object().expect("object").len(), 5);
    }

    #[test]
    fn pointer_position_observes_once_without_injection() {
        let mut calls = 0;
        let reply = pointer_position_with(|| {
            calls += 1;
            Ok((-17, 2048))
        })
        .expect("pointer position");
        assert_eq!(calls, 1);
        assert_eq!(reply["effect"], "observed");
        assert_eq!(reply["coords"], serde_json::json!([-17, 2048]));
        assert_eq!(reply.as_object().expect("object").len(), 4);
    }

    #[test]
    fn pointer_move_requires_actuate_and_refusal_moves_nothing() {
        let command = Command::PointerMove {
            target: TargetRef::Current,
            x: 10,
            y: 20,
        };
        let reply = Executor::new(Authorization::new(Default::default())).execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.expect("typed refusal").code, "refused");
    }
}
