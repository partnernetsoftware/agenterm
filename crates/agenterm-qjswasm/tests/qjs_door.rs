//! A `.qjs` script reaching the `agenterm.*` door -- end to end, through the
//! real compiler, the real load gate and a real bridge.
//!
//! # What this file locks
//!
//! Until this landed, a compiled `.qjs` guest could compute and could not *do*
//! anything: a free name was a compile-time capability refusal, so the four
//! door functions were reachable only by a hand-written `.wasm` guest. The
//! mechanism that closed it is upstream (`Names::Declared`); what is agenterm's
//! and therefore tested here is the **declaration table**: which host functions
//! exist, what a script may call them, and that the imports they emit are byte
//! for byte the door that already exists.
//!
//! That last point is the one that must not drift. The door is a product
//! contract shared with hand-written guests, locked by
//! `tests/host_door.rs` and `src/host.rs::SIGNATURES`. The compiler unwraps JS
//! values onto it; the door itself learns nothing about JavaScript. So the
//! import table a `.qjs` guest presents is indistinguishable from a
//! hand-written one's, and [`the_emitted_imports_are_exactly_the_existing_door`]
//! asserts exactly that, by decoding the emitted wasm rather than by trusting
//! the declarations.
//!
//! Nothing here asserts "it compiled" where running it was possible.

use std::sync::{Arc, Mutex};

use agenterm_qjswasm::{
    Budget, Engine, FleetBridgeFn, Guest, JsValue, QjswasmError, Value, compile_qjs,
};

// =========================================================================
// Harness
// =========================================================================

/// What the bridge saw, so a test can assert the script's own arguments
/// arrived -- not merely that *something* was called.
#[derive(Default)]
struct Calls(Mutex<Vec<(String, String)>>);

/// A real `FleetBridgeFn`: `answer` decides Ok/Err per operation.
fn bridge(
    calls: &Arc<Calls>,
    answer: impl Fn(&str, &str) -> Result<String, String> + Send + Sync + 'static,
) -> FleetBridgeFn {
    let calls = Arc::clone(calls);
    Arc::new(move |op: &str, params: &str| {
        calls
            .0
            .lock()
            .unwrap()
            .push((op.to_string(), params.to_string()));
        answer(op, params)
    })
}

#[track_caller]
fn run(
    source: &str,
    bridge: Option<FleetBridgeFn>,
) -> Result<agenterm_qjswasm::Outcome, QjswasmError> {
    Engine::new().run_once(Guest::Qjs(source), bridge, "main", &[])
}

#[track_caller]
fn returned_string(source: &str, bridge: Option<FleetBridgeFn>) -> String {
    let out = run(source, bridge).unwrap_or_else(|e| panic!("{source:?}: {e}"));
    match out.values.as_slice() {
        [Value::Js(JsValue::Str(s))] => s.clone(),
        other => panic!("{source:?} returned {other:?}, wanted one JS string"),
    }
}

/// The script every status test runs: call the door, branch on the status
/// code, and fetch the answer in the two-pass shape the door requires.
const BRANCHING: &str = r#"
    let status = fleet_call("some.op", "{}");
    if (status === 0) {
        return "ok:" + fleet_result();
    }
    if (status === 1) {
        return "err:" + fleet_result();
    }
    return "nobridge:" + fleet_result();
"#;

// =========================================================================
// End to end
// =========================================================================

/// The whole point of the task: a `.qjs` script calls the bridge with its own
/// arguments, reads the status, retrieves the answer as a JavaScript string,
/// and hands it back across the engine face.
#[test]
fn a_qjs_script_calls_the_bridge_and_returns_its_answer() {
    let calls = Arc::new(Calls::default());
    let got = returned_string(
        BRANCHING,
        Some(bridge(
            &calls,
            |_op, _params| Ok("{\"tabs\":2}".to_string()),
        )),
    );
    assert_eq!(got, "ok:{\"tabs\":2}");
    // The script's own strings reached the bridge -- both of them, unwrapped
    // from JS values into the raw (ptr, len) pairs the door takes.
    assert_eq!(
        *calls.0.lock().unwrap(),
        vec![("some.op".to_string(), "{}".to_string())]
    );
}

/// Status 1 is an application-level error: a normal result, not a crash, and
/// the bridge's message is retrievable through the same two passes.
#[test]
fn an_application_error_is_status_one_and_its_message_is_readable() {
    let calls = Arc::new(Calls::default());
    let got = returned_string(
        BRANCHING,
        Some(bridge(&calls, |_op, _params| {
            Err("broker_invalid_arguments".to_string())
        })),
    );
    assert_eq!(got, "err:broker_invalid_arguments");
}

/// Status 2 is "no bridge is installed in this slot" -- distinguishable from
/// an error the bridge produced, because a caller can fix one and not the
/// other.
#[test]
fn no_bridge_is_status_two_and_the_door_says_which() {
    let got = returned_string(BRANCHING, None);
    assert_eq!(
        got,
        "nobridge:agenterm: no fleet bridge is installed in this slot"
    );
}

/// `print` from a script lands in `Outcome::stdout`, the same buffer a
/// hand-written guest's `agenterm.print` lands in.
#[test]
fn print_from_a_qjs_script_reaches_the_outcome() {
    let out = run("print(\"hello \"); print(\"door\"); return 1;", None)
        .expect("printing is not a failure");
    assert_eq!(
        out.stdout, "hello \ndoor\n",
        "each print ends its own line, matching the engine this one replaces"
    );
    assert!(!out.truncated_stdout);
    assert_eq!(out.values, vec![Value::Js(JsValue::Number(1.0))]);
}

/// `print` evaluates to `undefined`, because the door returns nothing and the
/// compiler does not invent a value for it.
#[test]
fn print_evaluates_to_undefined() {
    let out = run("return print(\"x\");", None).expect("runs");
    assert_eq!(out.values, vec![Value::Js(JsValue::Undefined)]);
    assert_eq!(out.stdout, "x\n");
}

/// A bridge answer the guest can hold but the *seam* may not copy out is
/// `Budget("max_result_string_bytes")` -- refused, never truncated, and never
/// blamed on the script as a trap.
///
/// The two caps are deliberately set apart here: `max_bridge_result_bytes`
/// bounds what the bridge may push into the slot, and this one bounds what the
/// host copies back out. Leaving the first generous is what makes the second
/// the thing under test.
#[test]
fn a_bridge_answer_over_the_result_string_cap_is_a_budget_refusal() {
    let calls = Arc::new(Calls::default());
    let budget = Budget {
        max_bridge_result_bytes: 1 << 20,
        max_result_string_bytes: 16,
        ..Budget::default()
    };
    let mut eng = Engine::with_budget(budget);
    let err = eng
        .run_once(
            // The status is deliberately dropped rather than concatenated:
            // `Number + String` needs a ToString conversion the engine has not
            // implemented, and reaching it here would test that instead.
            Guest::Qjs("let status = fleet_call(\"big\", \"{}\"); return fleet_result();"),
            Some(bridge(&calls, |_op, _params| Ok("x".repeat(64)))),
            "main",
            &[],
        )
        .expect_err("a 64-byte answer does not fit a 16-byte cap");
    match err {
        QjswasmError::Budget(what) => assert_eq!(what, "max_result_string_bytes"),
        other => panic!("expected a budget refusal, got {other:?}"),
    }
}

/// The other cap, reached from `.qjs`: an answer over
/// `max_bridge_result_bytes` never enters the guest at all. The door replaces
/// it with status 1 and its own bounded message, so the script sees a normal
/// application error rather than half a document.
#[test]
fn a_bridge_answer_over_the_bridge_cap_arrives_as_status_one() {
    let calls = Arc::new(Calls::default());
    let mut eng = Engine::with_budget(Budget {
        max_bridge_result_bytes: 8,
        ..Budget::default()
    });
    let out = eng
        .run_once(
            Guest::Qjs(BRANCHING),
            Some(bridge(&calls, |_op, _params| Ok("x".repeat(64)))),
            "main",
            &[],
        )
        .expect("an over-cap answer is a normal result, not a failure");
    assert_eq!(
        out.values,
        vec![Value::Js(JsValue::Str(
            "err:agenterm: fleet result exceeds the slot's max_bridge_result_bytes".into()
        ))]
    );
}

/// A door call works wherever an expression does -- inside a function, inside
/// a loop -- because the declaration is a mechanism and not a special form.
#[test]
fn a_door_call_is_an_ordinary_expression() {
    let calls = Arc::new(Calls::default());
    let out = run(
        "function ping(op) { return fleet_call(op, \"{}\"); }
         let n = 0;
         for (let i = 0; i < 3; i = i + 1) { n = n + ping(\"tabs.list\"); }
         print(fleet_result());
         return n;",
        Some(bridge(&calls, |_op, _params| Ok("done".to_string()))),
    )
    .expect("runs");
    assert_eq!(out.values, vec![Value::Js(JsValue::Number(0.0))]);
    assert_eq!(out.stdout, "done\n");
    assert_eq!(calls.0.lock().unwrap().len(), 3);
}

// =========================================================================
// The import table is the door, and only what the script asked for
// =========================================================================

/// A script that mentions no host name emits **no** imports, so a guest that
/// cannot reach the door does not oblige anyone to bind one.
#[test]
fn a_script_that_mentions_no_host_name_emits_no_imports() {
    let wasm = compile_qjs("let x = 1; return x + 41;").expect("compiles");
    assert_eq!(imports(&wasm), Vec::<String>::new());

    // And it still runs, unchanged: declaring a door is not a tax on scripts
    // that do not use it.
    let out = run("let x = 1; return x + 41;", None).expect("runs");
    assert_eq!(out.values, vec![Value::Js(JsValue::Number(42.0))]);
}

/// Only the declarations a script mentions become imports, in declaration
/// order -- so an embedder can predict the import table without reading the
/// script.
#[test]
fn only_the_door_functions_a_script_mentions_are_imported() {
    assert_eq!(
        imports(&compile_qjs("print(\"a\"); return 0;").unwrap()),
        vec!["agenterm.print(i32, i32) -> ()"]
    );
    assert_eq!(
        imports(&compile_qjs("return fleet_call(\"o\", \"p\");").unwrap()),
        vec!["agenterm.fleet_call(i32, i32, i32, i32) -> i32"]
    );
    // `fleet_result` alone brings its length pass with it: a byte result is
    // two imports, because a wasm function cannot return a slice.
    assert_eq!(
        imports(&compile_qjs("return fleet_result();").unwrap()),
        vec![
            "agenterm.fleet_result_len() -> i32",
            "agenterm.fleet_result(i32, i32) -> i32",
        ]
    );
}

/// The emitted imports are the existing door, exactly -- decoded out of the
/// wasm rather than read off the declarations.
///
/// This is the cross-repo contract in one assertion (`plan/design-agenterm-qjswasm.md`
/// 6.5): the door does not learn about JS values, the compiler unwraps into
/// it, and a compiled `.qjs` guest therefore presents the same import table a
/// hand-written `.wasm` guest presents. If this ever diverges from
/// `src/host.rs::SIGNATURES`, one of the two has been changed alone.
#[test]
fn the_emitted_imports_are_exactly_the_existing_door() {
    let wasm =
        compile_qjs("print(\"x\"); let s = fleet_call(\"o\", \"p\"); return fleet_result();")
            .expect("compiles");
    assert_eq!(
        imports(&wasm),
        vec![
            "agenterm.print(i32, i32) -> ()",
            "agenterm.fleet_call(i32, i32, i32, i32) -> i32",
            "agenterm.fleet_result_len() -> i32",
            "agenterm.fleet_result(i32, i32) -> i32",
        ]
    );
    // The door itself accepts them: `check_declarations` runs at load time and
    // refuses anything it cannot bind, so a clean spawn is the door agreeing.
    Engine::new()
        .spawn(Guest::CompiledQjs(&wasm), None)
        .expect("the door binds what the compiler emitted");
}

// =========================================================================
// The table is the world
// =========================================================================

/// `check` and `execute` go through one compile entry point, so a script that
/// reaches the door is accepted by both -- and one that does not is refused by
/// both, with the same diagnostic.
#[test]
fn check_and_execute_agree_about_the_door() {
    let reaches = "print(\"x\"); return fleet_call(\"o\", \"p\");";
    compile_qjs(reaches).expect("check accepts a script that calls the door");
    run(reaches, None).expect("execute accepts the same script");

    let does_not = "return fleet_resultt();";
    let check = compile_qjs(does_not).expect_err("check refuses an undeclared name");
    let exec = run(does_not, None).expect_err("execute refuses it too");
    match exec {
        QjswasmError::Compile(e) => assert_eq!(e.message, check.message),
        other => panic!("expected a compile refusal, got {other:?}"),
    }
}

/// An undeclared free name is still refused, and the diagnostic names this
/// engine's capability boundary and lists what there *is* -- never a bare
/// "syntax error", and never a suggestion that the script is malformed.
#[test]
fn a_name_outside_the_door_is_refused_and_the_door_is_listed() {
    let err = compile_qjs("return console_log(\"x\");").expect_err("not a door function");
    assert!(
        err.message.starts_with("this engine "),
        "diagnostic blames the script: {err}"
    );
    assert!(err.message.contains("console_log"), "{err}");
    for offered in ["print", "fleet_call", "fleet_result"] {
        assert!(
            err.message.contains(offered),
            "{err:?} does not tell the reader that {offered} exists"
        );
    }
    // `fleet_result_len` is the length pass of a byte result, not a name a
    // script may write: the two-pass fetch is the compiler's business.
    let err = compile_qjs("return fleet_result_len();").expect_err("not script-visible");
    assert!(err.message.starts_with("this engine "), "{err}");
}

// =========================================================================
// A minimal wasm import decoder, so the assertions above read the bytes
// =========================================================================

/// Decode the emitted module's function imports as `module.field(params) -> results`.
///
/// Hand-rolled rather than borrowed from a library because the point is to
/// read what was *emitted*: a decoder that shares code with the emitter would
/// agree with it by construction.
fn imports(wasm: &[u8]) -> Vec<String> {
    let mut at = 8; // magic + version
    let mut types: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut out = Vec::new();
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let size = uleb(wasm, &mut at) as usize;
        let end = at + size;
        match id {
            1 => {
                let n = uleb(wasm, &mut at);
                for _ in 0..n {
                    assert_eq!(wasm[at], 0x60, "not a function type");
                    at += 1;
                    let params = valtypes(wasm, &mut at);
                    let results = valtypes(wasm, &mut at);
                    types.push((params, results));
                }
            }
            2 => {
                let n = uleb(wasm, &mut at);
                for _ in 0..n {
                    let module = name(wasm, &mut at);
                    let field = name(wasm, &mut at);
                    let kind = wasm[at];
                    at += 1;
                    assert_eq!(kind, 0, "{module}.{field} is not a function import");
                    let (params, results) = &types[uleb(wasm, &mut at) as usize];
                    out.push(format!(
                        "{module}.{field}({}) -> {}",
                        render(params),
                        if results.is_empty() {
                            "()".to_string()
                        } else {
                            render(results)
                        }
                    ));
                }
            }
            _ => {}
        }
        at = end;
    }
    out
}

fn render(types: &[u8]) -> String {
    types
        .iter()
        .map(|t| match t {
            0x7F => "i32",
            0x7E => "i64",
            0x7D => "f32",
            0x7C => "f64",
            _ => "?",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn valtypes(wasm: &[u8], at: &mut usize) -> Vec<u8> {
    let n = uleb(wasm, at);
    let types = wasm[*at..*at + n as usize].to_vec();
    *at += n as usize;
    types
}

fn name(wasm: &[u8], at: &mut usize) -> String {
    let n = uleb(wasm, at) as usize;
    let s = String::from_utf8(wasm[*at..*at + n].to_vec()).expect("utf-8 name");
    *at += n;
    s
}

fn uleb(wasm: &[u8], at: &mut usize) -> u64 {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = wasm[*at];
        *at += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}
