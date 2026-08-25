//! The claim all three fleet bindings make in their own header comments, as a
//! test: *a script that calls `fleet.tabs.set_note(...)` produces the identical
//! Fleet operation regardless of which engine ran it.*
//!
//! Until now that sentence was checked one layer too high. `script_fleet_facade_parity`
//! compares the binding *files* -- same function paths, same operation ids --
//! which catches a rename or a dropped wrapper and cannot catch anything about
//! what the engines actually do with them. Two files that agree on
//! `tabs.set-note` can still emit different params, disagree about whether a
//! refusal is an exception, or hand a caller a different shape of answer.
//!
//! This is the gate-2 evidence for retiring `agenterm-qjs` (PRD 02.36): before
//! a production call site moves from one engine to the other, the two have to
//! be shown to agree on the wire, and the disagreements that remain have to be
//! named rather than discovered by whoever moves first.
//!
//! # What "the same script" means here
//!
//! It cannot mean the same bytes. The two engines have different entry
//! conventions -- `agenterm-qjs` calls a top-level `entry()` and reaches the
//! host through `__host.fleet_call`; `agenterm-qjswasm` runs the file and takes
//! its completion value, and its door is free functions. So each case is one
//! *body*, wrapped in whichever idiom its engine uses, appended to whichever
//! binding file belongs to it -- and both binding files are read off disk, so
//! this tests the shipped bindings and not a copy of them.
#![cfg(all(feature = "script-qjs", feature = "script-qjswasm"))]

use std::sync::{Arc, Mutex};

use agenterm::operations::{OPERATION_CATALOG, OperationSpec};

/// Every `(operation_id, params_json)` an engine sent, in order.
type Calls = Vec<(String, String)>;

/// What one engine did with a case: the operations it emitted, and the value
/// the script evaluated to, projected to JSON so the two engines' different
/// value types can be compared at all.
#[derive(Debug, PartialEq)]
struct Ran {
    calls: Calls,
    value: Option<serde_json::Value>,
}

fn read(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The reply a fake broker gives, chosen per case: a real answer for that
/// operation is what makes the *result* half of the comparison mean anything.
struct Broker {
    reply: Result<String, String>,
}

// ---- engine one: agenterm-qjs (rquickjs) ---------------------------------

fn run_on_qjs(body: &str, broker: &Broker) -> Result<Ran, String> {
    let calls: Arc<Mutex<Calls>> = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    let reply = broker.reply.clone();
    let host = agenterm_qjs::QjsHostFunctions {
        fleet_call: Some(Arc::new(move |op: &str, params: &str| {
            seen.lock()
                .expect("capture")
                .push((op.to_string(), params.to_string()));
            reply.clone()
        })),
        ..Default::default()
    };

    let source = format!(
        "{}\nfunction entry() {{\n{body}\n}}",
        read("scripts/qjs/lib/fleet.js")
    );
    let outcome = agenterm_qjs::eval_entry_with_host(&source, "equivalence.js", &host)
        .map_err(|e| e.to_string())?;

    let calls = calls.lock().expect("capture").clone();
    Ok(Ran {
        calls,
        value: outcome.value,
    })
}

// ---- engine two: agenterm-qjswasm (tinyvm) -------------------------------

fn run_on_qjswasm(body: &str, broker: &Broker) -> Result<Ran, String> {
    use agenterm_qjswasm::{Engine, FleetBridgeFn, Guest, JsValue, Value};

    let calls: Arc<Mutex<Calls>> = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    let reply = broker.reply.clone();
    let bridge: FleetBridgeFn = Arc::new(move |op: &str, params: &str| {
        seen.lock()
            .expect("capture")
            .push((op.to_string(), params.to_string()));
        reply.clone()
    });

    let source = format!("{}\n{body}", read("scripts/qjs/lib/fleet.qjs"));
    let mut engine = Engine::new();
    let outcome = engine
        .run_once(Guest::Qjs(&source), Some(bridge), "main", &[])
        .map_err(|e| e.to_string())?;

    // Project the completion value onto JSON, the shape `agenterm-qjs` reports
    // its own result in. An Object or a function has no projection today and is
    // not something these cases return; anything else here is a real difference
    // and must not be silently flattened to `None`.
    let value = match outcome.values.first() {
        None | Some(Value::Js(JsValue::Undefined)) => None,
        Some(Value::Js(JsValue::Null)) => Some(serde_json::Value::Null),
        Some(Value::Js(JsValue::Bool(b))) => Some(serde_json::Value::Bool(*b)),
        Some(Value::Js(JsValue::Number(x))) => Some(
            serde_json::Number::from_f64(*x)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
        ),
        Some(Value::Js(JsValue::Str(s))) => Some(serde_json::Value::String(s.clone())),
        other => panic!("this case returned a value with no JSON projection: {other:?}"),
    };

    let calls = calls.lock().expect("capture").clone();
    Ok(Ran { calls, value })
}

// ---- the comparison ------------------------------------------------------

fn operation(id: &str) -> &'static OperationSpec {
    OPERATION_CATALOG
        .iter()
        .find(|spec| spec.id == id)
        .unwrap_or_else(|| panic!("{id} is not in OPERATION_CATALOG"))
}

/// Compare numbers by value, not by JSON spelling.
///
/// The first run of this file failed on `1920` vs `1920.0`, and the difference
/// was this test's, not the engines'. A JavaScript Number is always a double;
/// `agenterm-qjs` reports its result through `JSON.stringify`, which writes an
/// integral double without a fraction, and the projection above wrote one with.
/// The *product* faces agree -- `script_engine.rs::number_as_json` applies
/// exactly ECMA-262's rule to the tinyvm side for exactly this reason -- so
/// asserting on `serde_json::Number`'s representation was asserting on
/// something neither engine promises and neither caller sees.
///
/// Rendering every number as `f64` here says so rather than papering over it:
/// this file is about the wire and the value, and JSON number spelling is
/// settled one layer up.
fn by_value(value: &Option<serde_json::Value>) -> Option<serde_json::Value> {
    fn walk(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Number(n) => n
                .as_f64()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(walk).collect())
            }
            serde_json::Value::Object(fields) => serde_json::Value::Object(
                fields.iter().map(|(k, v)| (k.clone(), walk(v))).collect(),
            ),
            other => other.clone(),
        }
    }
    value.as_ref().map(walk)
}

/// Run one body on both engines and require they agree, returning what they
/// both did so a case can assert more about it.
#[track_caller]
fn agree(body: &str, broker: &Broker) -> Ran {
    let qjs = run_on_qjs(body, broker).expect("the rquickjs engine runs the case");
    let qjswasm = run_on_qjswasm(body, broker).expect("the tinyvm engine runs the case");
    assert_eq!(
        qjs.calls, qjswasm.calls,
        "the two engines sent different Fleet operations for this script:\n{body}"
    );
    assert_eq!(
        by_value(&qjs.value),
        by_value(&qjswasm.value),
        "the two engines returned different values for this script:\n{body}"
    );
    // Whatever they agreed on still has to be something the broker accepts --
    // two engines can agree on a payload the catalog refuses.
    for (id, _) in &qjs.calls {
        operation(id);
    }
    qjs
}

/// A string payload, through both bindings.
#[test]
fn both_engines_emit_the_same_operation_for_a_string_payload() {
    let broker = Broker {
        reply: Ok(r#"{"ok":true}"#.to_string()),
    };
    let ran = agree(
        r#"const reply = fleet.tabs.set_note("@1", "written by both");
           return reply.ok;"#,
        &broker,
    );
    assert_eq!(ran.calls.len(), 1);
    let (id, params) = &ran.calls[0];
    assert_eq!(id, "tabs.set-note");
    let params: serde_json::Value = serde_json::from_str(params).expect("valid JSON");
    assert_eq!(params["tab"], "@1");
    assert_eq!(params["note"], "written by both");
    assert_eq!(ran.value, Some(serde_json::Value::Bool(true)));
}

/// A numeric payload. Separate because it was separately impossible on the
/// tinyvm side until the ECMA-262 Number-to-String conversions landed, and
/// because "arrives as a JSON number, not the string `320`" is exactly the kind
/// of difference a file-level parity check cannot see.
#[test]
fn both_engines_emit_the_same_operation_for_a_numeric_payload() {
    let broker = Broker {
        reply: Ok(r#"{"ok":true}"#.to_string()),
    };
    let ran = agree(
        r#"fleet.ui.tabs.set_width(320);
           return 1;"#,
        &broker,
    );
    let (id, params) = &ran.calls[0];
    assert_eq!(id, "ui.tabs.set-width");
    let params: serde_json::Value = serde_json::from_str(params).expect("valid JSON");
    assert_eq!(params["width"], 320);
}

/// A no-parameter operation, and the `{}` both bindings must send for one.
#[test]
fn both_engines_send_an_empty_object_for_a_nullary_operation() {
    let broker = Broker {
        reply: Ok(r#"{"width":1920,"height":1080}"#.to_string()),
    };
    let ran = agree(
        r#"const snap = fleet.ui.snapshot();
           return snap.width;"#,
        &broker,
    );
    assert_eq!(
        ran.calls,
        vec![("ui.snapshot".to_string(), "{}".to_string())]
    );
    assert_eq!(by_value(&ran.value), Some(serde_json::json!(1920.0)));
}

/// A refusal is an exception on both engines, and the script catches it on
/// both.
///
/// This is the one that had to change to be true. The `.qjs` binding used to
/// return `"ERR " + text`, so a script ported from `.js` kept its `try`/`catch`
/// and caught nothing -- the two engines disagreed about whether a rejected
/// operation is an error at all, which is the difference most likely to be
/// found in production rather than in a test.
#[test]
fn both_engines_make_a_refusal_catchable() {
    let broker = Broker {
        reply: Err("broker_operation_unknown: nope".to_string()),
    };
    let ran = agree(
        r#"try {
             fleet.ui.hello();
             return "not reached";
           } catch (e) {
             return "caught";
           }"#,
        &broker,
    );
    assert_eq!(ran.value, Some(serde_json::json!("caught")));
}

/// **A named divergence, not a passing case.**
///
/// `tabs.list` answers with a JSON *array*. Both bindings wrap the parse in
/// `try`/`catch` and fall back to the raw text, so on `agenterm-qjs` the caller
/// gets an array and on `agenterm-qjswasm` it gets a String -- the tinyvm
/// engine has no Array type yet and `JSON.parse` refuses one by name rather
/// than approximating it (`tinyvm-qjs`'s README says so at the `JSON.parse("[1]")`
/// boundary, and states this exact downstream consequence).
///
/// It is pinned here rather than left out because a gap nobody wrote down is a
/// gap somebody rediscovers. When arrays land upstream this test fails, and the
/// right fix is to move the case into [`both_engines_emit_the_same_operation_for_a_string_payload`]'s
/// company as an ordinary agreement -- not to widen the assertion below.
#[test]
fn an_array_answer_is_the_one_place_the_two_engines_still_differ() {
    let broker = Broker {
        reply: Ok(r#"[{"id":"tab1"}]"#.to_string()),
    };
    let body = r#"const tabs = fleet.tabs.list();
                  return typeof tabs;"#;

    let qjs = run_on_qjs(body, &broker).expect("rquickjs runs it");
    let qjswasm = run_on_qjswasm(body, &broker).expect("tinyvm runs it");

    assert_eq!(
        qjs.calls, qjswasm.calls,
        "the two engines must still send the identical operation; only the \
         answer's shape differs"
    );
    assert_eq!(
        qjs.value,
        Some(serde_json::json!("object")),
        "rquickjs parses the array, and an array is an object"
    );
    assert_eq!(
        qjswasm.value,
        Some(serde_json::json!("string")),
        "tinyvm has no Array type, so the binding's catch hands back the raw \
         text -- if this is no longer a string, arrays landed and this test \
         should become an ordinary agreement case"
    );
}
