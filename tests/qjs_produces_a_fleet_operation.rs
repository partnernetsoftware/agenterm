//! The acceptance evidence for the `.qjs` engine: a script written in the
//! product's own scripting language reaches the host door and produces a Fleet
//! operation the catalog accepts.
//!
//! This exists because the claim was demonstrated once, by hand, and reported —
//! and a demonstration that is not a test does not survive the week. Everything
//! else about this engine (73 tests in `agenterm-qjswasm`, the compiler's own
//! suite upstream) proves that parts work. This proves the product sentence:
//! **a `.qjs` script can drive the fleet.**
//!
//! It lives in the root crate rather than beside the engine because the engine
//! must not depend on `agenterm` — the dependency runs the other way — and this
//! assertion needs both `agenterm::operations::OPERATION_CATALOG` and
//! `agenterm_qjswasm::Engine` in one process.
#![cfg(feature = "script-qjswasm")]

use std::sync::{Arc, Mutex};

use agenterm::operations::{OPERATION_CATALOG, OperationSpec};
use agenterm_qjswasm::{Engine, FleetBridgeFn, Guest, JsValue, Value};

/// What the bridge was actually asked to do, captured from inside the door.
#[derive(Default)]
struct Captured {
    calls: Vec<(String, String)>,
}

fn capturing_bridge(seen: Arc<Mutex<Captured>>, reply: &'static str) -> FleetBridgeFn {
    Arc::new(move |operation_id: &str, params_json: &str| {
        seen.lock()
            .expect("bridge capture")
            .calls
            .push((operation_id.to_string(), params_json.to_string()));
        Ok(reply.to_string())
    })
}

fn operation(id: &str) -> &'static OperationSpec {
    OPERATION_CATALOG
        .iter()
        .find(|spec| spec.id == id)
        .unwrap_or_else(|| panic!("{id} is not in OPERATION_CATALOG"))
}

/// Assert an emitted payload against the catalog the broker validates with:
/// every key is declared, every required parameter is present, and every value
/// matches its declared type. This is deliberately a re-derivation from
/// `OperationSpec` rather than a call into the broker's private validator —
/// what matters is that the script satisfies the *declaration*, and the
/// declaration is public.
fn assert_payload_conforms(spec: &OperationSpec, params_json: &str) {
    let value: serde_json::Value =
        serde_json::from_str(params_json).expect("the script must emit valid JSON");
    let object = value
        .as_object()
        .expect("Fleet parameters are always a JSON object");

    for key in object.keys() {
        assert!(
            spec.parameters.iter().any(|p| p.name == key),
            "{} does not accept parameter {key}",
            spec.id
        );
    }
    for parameter in spec.parameters {
        let Some(supplied) = object.get(parameter.name) else {
            assert!(
                !parameter.required,
                "{} requires parameter {}",
                spec.id, parameter.name
            );
            continue;
        };
        let matches = match parameter.value_type {
            "string" | "session_name" => supplied.as_str().is_some(),
            "stable_tab_id" => supplied
                .as_str()
                .is_some_and(|tab| tab.starts_with('@') && tab.len() > 1),
            "uint32" | "uint64" => supplied.as_u64().is_some(),
            "integer" => supplied.as_i64().is_some(),
            "number" => supplied.as_f64().is_some(),
            other => panic!("unknown value_type {other:?} for {}", parameter.name),
        };
        assert!(
            matches,
            "{} parameter {} must be {}, got {supplied}",
            spec.id, parameter.name, parameter.value_type
        );
    }
}

/// The product sentence, end to end.
///
/// The script is shaped the way a real binding is: a general `call` helper that
/// every operation wrapper goes through, and one wrapper built on it. That is
/// the structure of `scripts/qjs/lib/fleet.js`, so this is a reduced form of
/// the file that gates retiring the previous engine — not a synthetic probe.
#[test]
fn a_qjs_script_drives_a_real_fleet_operation() {
    let seen = Arc::new(Mutex::new(Captured::default()));
    let bridge = capturing_bridge(Arc::clone(&seen), r#"{"ok":true}"#);

    // Note what this deliberately does NOT use: object literals, property
    // access, `?:` and `JSON.stringify` are all still refused by the compiler,
    // so the payload is assembled by string concatenation. When those land this
    // script should be rewritten to look like `fleet.js` proper, and the
    // assertions below should not need to change.
    let script = r#"
        function call(op, params) {
            if (fleet_call(op, params) === 0) {
                return fleet_result();
            }
            return "";
        }
        function set_note(tab, note) {
            return call("tabs.set-note", "{\"tab\":\"" + tab + "\",\"note\":\"" + note + "\"}");
        }
        set_note("@1", "written from .qjs")
    "#;

    let mut engine = Engine::new();
    let outcome = engine
        .run_once(Guest::Qjs(script), Some(bridge), "main", &[])
        .expect("a .qjs script must reach the host door");

    let captured = seen.lock().expect("bridge capture");
    assert_eq!(
        captured.calls.len(),
        1,
        "the script must call the door exactly once, got {:?}",
        captured.calls
    );
    let (operation_id, params_json) = &captured.calls[0];

    // The operation the script named must be one the product actually declares.
    let spec = operation(operation_id);
    assert_payload_conforms(spec, params_json);

    // And the payload must be the one the script meant, not merely a conformant one.
    let params: serde_json::Value = serde_json::from_str(params_json).expect("valid JSON");
    assert_eq!(params["tab"], "@1");
    assert_eq!(params["note"], "written from .qjs");

    // The bridge's answer has to come back through the two-pass door as a real
    // host-owned JS string -- the slot is dead by the time `run_once` returns,
    // so anything still pointing into its linear memory would be dangling.
    assert_eq!(
        outcome.values.first(),
        Some(&Value::Js(JsValue::Str(r#"{"ok":true}"#.to_string()))),
        "the bridge's reply must survive the slot it was read into"
    );
}

/// A script that names no host function must not import one. The door is a
/// capability, and a capability nobody asked for should not appear in the
/// artifact — otherwise "what can this guest reach?" stops being answerable by
/// reading its imports.
#[test]
fn a_script_that_asks_for_nothing_reaches_nothing() {
    let seen = Arc::new(Mutex::new(Captured::default()));
    let bridge = capturing_bridge(Arc::clone(&seen), "{}");

    let mut engine = Engine::new();
    let outcome = engine
        .run_once(Guest::Qjs("1 + 2"), Some(bridge), "main", &[])
        .expect("a script with no host use still runs");

    assert_eq!(
        outcome.values.first(),
        Some(&Value::Js(JsValue::Number(3.0)))
    );
    assert!(
        seen.lock().expect("bridge capture").calls.is_empty(),
        "a script that mentions no host name must not call the bridge"
    );
}

/// Every operation the catalog declares with only string parameters is
/// expressible by a `.qjs` script today, because strings are all the language
/// can build. This pins the reach of the current subset: when Number-to-String
/// lands, the numeric operations join them and this count should be revisited
/// rather than silently drifting.
#[test]
fn the_reachable_share_of_the_catalog_is_known() {
    let mut no_parameters = 0usize;
    let mut string_only = 0usize;
    let mut needs_a_number = 0usize;

    for spec in OPERATION_CATALOG {
        if spec.parameters.is_empty() {
            no_parameters += 1;
            continue;
        }
        let numeric = spec.parameters.iter().any(|parameter| {
            matches!(
                parameter.value_type,
                "uint32" | "uint64" | "integer" | "number"
            ) && parameter.required
        });
        if numeric {
            needs_a_number += 1;
        } else {
            string_only += 1;
        }
    }

    let reachable = no_parameters + string_only;
    assert_eq!(
        reachable + needs_a_number,
        OPERATION_CATALOG.len(),
        "every operation must fall into exactly one bucket"
    );
    assert!(
        needs_a_number > 0,
        "if this reaches zero the language gained numeric payloads and this test should \
         become an equality against the whole catalog"
    );
    assert!(
        reachable >= 60,
        "the string-expressible share of the catalog shrank to {reachable}; an operation \
         gained a required numeric parameter and is now unreachable from .qjs"
    );
}
