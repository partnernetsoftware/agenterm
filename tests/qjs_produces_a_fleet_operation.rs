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
use agenterm_qjswasm::{Engine, FleetBridgeFn, Guest, JsValue, Value, compile_qjs_with_modules};

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

/// The real `scripts/qjs/lib/fleet.qjs`, read from the repo -- never a copy.
///
/// A copy of a binding tests the copy. `agenterm-qjs`'s `eval_fleet_module`
/// reads `fleet.js` off disk for exactly this reason, and the two engines'
/// facades have to be tested the same way or "equivalent to `fleet.js`" is a
/// claim about two files nobody compared.
fn fleet_binding() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("qjs")
        .join("lib")
        .join("fleet.qjs");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The binding plus a driver, which is how a `.qjs` script uses it today: one
/// compilation unit, library first. There is no module system in the subset
/// yet, so this is concatenation -- the same shape `agenterm-qjs`'s test uses
/// for `fleet.js`, and for the same reason.
/// The library is now **imported**, not pasted.
///
/// This function was `format!("{}\n{driver}", fleet_binding())` until
/// 2026-08-29 -- the only way a script could use `scripts/qjs/lib/fleet.qjs`,
/// and the reason a complete 29-operation binding had no consumers: a `.qjs`
/// file on disk had no way to reach it. That `format!` is what upstream's
/// module milestone was built to replace, and the difference between the two
/// is that this one gives the library a namespace instead of tipping its
/// top-level names into the script's scope.
fn with_binding(driver: &str) -> String {
    format!("import * as lib from \"fleet\";\nconst fleet = lib.fleet;\n{driver}")
}

/// Resolves the one specifier this test uses, from the real file on disk.
///
/// A closure and not a path: the compiler never reads a file, so the product
/// decides what a specifier means. Here it means one name, bound to the same
/// `scripts/qjs/lib/fleet.qjs` the assertions below are about -- read from the
/// repository rather than copied, for the same reason the rest of this file
/// reads it.
fn resolve_fleet(specifier: &str) -> Option<String> {
    (specifier == "fleet").then(fleet_binding)
}

/// Compile the importing script, so the run below can be handed bytes.
///
/// `Engine::run_once(Guest::Qjs(..))` compiles with no resolver, because the
/// engine's guest path has no entry path to resolve against -- a script's
/// source reaches it as text. Wiring that through so `agenterm cli script run
/// FILE.qjs` can import is a separate, named piece of work; until then a
/// caller that wants modules compiles first, which is what this does.
fn compiled(script: &str) -> Vec<u8> {
    compile_qjs_with_modules(script, &resolve_fleet)
        .expect("the importing script compiles against the real fleet binding")
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

/// The product sentence, end to end, through the **real** binding.
///
/// The script is `scripts/qjs/lib/fleet.qjs` itself -- the file that gates
/// retiring `agenterm-qjs` -- with a driver appended. Not a reduced form of it,
/// which is what this test used to be, and not a synthetic probe.
///
/// It could only be a reduced form before: object literals, property access,
/// `?:` and `JSON.stringify` were all refused, so the earlier version assembled
/// its payload by string concatenation and said in a comment that when those
/// landed the script should be rewritten to look like `fleet.js` proper, with
/// the assertions below unchanged. They landed at tinyvm `14a641a`. This is
/// that rewrite, and the assertions are unchanged.
///
/// What the driver returns is load-bearing. `call` parses the broker's answer,
/// so `reply.ok` is only reachable if `JSON.parse` ran on a real host-supplied
/// string and property access worked on the object it produced -- three things
/// the concatenation-era script could not do and could not have noticed
/// breaking.
#[test]
fn a_qjs_script_drives_a_real_fleet_operation() {
    let seen = Arc::new(Mutex::new(Captured::default()));
    let bridge = capturing_bridge(Arc::clone(&seen), r#"{"ok":true}"#);

    let script = with_binding(
        r#"
        const reply = fleet.tabs.set_note("@1", "written from .qjs");
        reply.ok
    "#,
    );

    let mut engine = Engine::new();
    let outcome = engine
        .run_once(
            Guest::CompiledQjs(&compiled(&script)),
            Some(bridge),
            "main",
            &[],
        )
        .expect("a .qjs script must reach the host door through the real binding");

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

    // The bridge's answer came back through the two-pass door, was parsed, and
    // a property was read off the result -- so the reply is not merely a string
    // the script forwarded, and nothing still points into the dead slot's
    // linear memory.
    assert_eq!(
        outcome.values.first(),
        Some(&Value::Js(JsValue::Bool(true))),
        "the binding must parse the bridge's reply, not hand back its text"
    );
}

/// A numeric payload, through the same real binding.
///
/// Separate from the string case because it was separately impossible: until
/// the three ECMA-262 string conversions landed there was no way for a `.qjs`
/// script to put a number on the wire at all, which is what
/// [`the_reachable_share_of_the_catalog_is_known`] used to measure. A binding
/// that can only send strings covers part of a catalog whose specs declare
/// `uint32`, so this asserts the other half of the wire, not a second flavour
/// of the same one.
#[test]
fn a_numeric_fleet_payload_survives_the_trip() {
    let seen = Arc::new(Mutex::new(Captured::default()));
    let bridge = capturing_bridge(Arc::clone(&seen), r#"{"ok":true}"#);

    let script = with_binding(
        r#"
        fleet.ui.tabs.set_width(320);
        1
    "#,
    );

    let mut engine = Engine::new();
    engine
        .run_once(
            Guest::CompiledQjs(&compiled(&script)),
            Some(bridge),
            "main",
            &[],
        )
        .expect("a numeric operation runs");

    let captured = seen.lock().expect("bridge capture");
    let (operation_id, params_json) = &captured.calls[0];
    assert_payload_conforms(operation(operation_id), params_json);

    let params: serde_json::Value = serde_json::from_str(params_json).expect("valid JSON");
    assert_eq!(
        params["width"], 320,
        "the width must arrive as a JSON number, not as the string \"320\""
    );
}

/// A refused operation throws, and the script can catch it.
///
/// This is behavioural parity with `fleet.js`, where the refusal comes out of
/// the host function as an exception. The `.qjs` binding used to return
/// `"ERR " + text` instead, because `throw` was not in the subset -- which
/// meant a script ported from `.js` kept its `try`/`catch`, caught nothing,
/// and let the error travel on as ordinary data. Two engines that disagree
/// about whether a refusal is an exception do not have equivalent bindings,
/// whatever their operation lists look like, so this is part of the archive
/// gate and not a nicety.
#[test]
fn a_refused_operation_throws_and_is_catchable() {
    let refusing: FleetBridgeFn =
        Arc::new(|_op: &str, _params: &str| Err("broker_operation_unknown: nope".to_string()));

    let script = with_binding(
        r#"
        try {
            fleet.ui.hello();
            "not reached"
        } catch (e) {
            e
        }
    "#,
    );

    let mut engine = Engine::new();
    let outcome = engine
        .run_once(
            Guest::CompiledQjs(&compiled(&script)),
            Some(refusing),
            "main",
            &[],
        )
        .expect("a caught refusal is not a failure");

    let Some(Value::Js(JsValue::Str(caught))) = outcome.values.first() else {
        panic!("expected the caught value, got {:?}", outcome.values);
    };
    assert!(
        caught.contains("ui.hello") && caught.contains("broker_operation_unknown"),
        "the thrown value must name the operation and carry the broker's text, got {caught:?}"
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

/// Every operation in the catalog is expressible by a `.qjs` script.
///
/// This used to be a *share*: strings were all the language could build, so
/// operations with a required numeric parameter were out of reach, and the
/// test measured how much of the catalog that left and asserted the shortfall
/// was real. Its own doc said what to do when Number-to-String landed --
/// "the numeric operations join them and this count should be revisited rather
/// than silently drifting". They landed at tinyvm `14a641a`, and
/// [`a_numeric_fleet_payload_survives_the_trip`] measures one going out as a
/// JSON number. So the shortfall is gone and the test becomes the equality it
/// was told to become.
///
/// It keeps its place rather than being deleted. The value_types below are the
/// ones a `.qjs` payload can construct today; an operation declaring a
/// parameter of some *new* type -- an array, a nested object -- would be
/// unreachable again, and this is what would say so instead of the discovery
/// happening in a script someone was trying to write.
#[test]
fn the_reachable_share_of_the_catalog_is_known() {
    let expressible = |value_type: &str| {
        matches!(
            value_type,
            "string"
                | "session_name"
                | "stable_tab_id"
                | "uint32"
                | "uint64"
                | "integer"
                | "number"
                | "bool"
                | "boolean"
        )
    };

    let unreachable: Vec<_> = OPERATION_CATALOG
        .iter()
        .filter(|spec| {
            spec.parameters
                .iter()
                .any(|parameter| !expressible(parameter.value_type))
        })
        .map(|spec| spec.id)
        .collect();

    assert!(
        unreachable.is_empty(),
        "these operations declare a parameter type a .qjs payload cannot build: {unreachable:?}"
    );
}
