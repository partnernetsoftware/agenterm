//! Attacking the `.qjs` -> `agenterm.*` door: hostile scripts, hostile
//! bridges, and the budget edges where the two-pass string fetch lives.
//!
//! `tests/qjs_door.rs` proves the door works. This file tries to break it. The
//! division matters because the two answer different questions: that one asks
//! "does a script reach the bridge", this one asks "what does the engine say
//! when the script, the bridge, or the budget misbehaves" -- and the required
//! answer is always the same shape. A typed refusal is correct. A panic, a
//! wrong value, a fabricated string, or a diagnostic that blames the wrong
//! party is not.
//!
//! # What is asserted, and what was found
//!
//! Most of this file locks behaviour that was already right, so it stays
//! right. Five tests started life differently: they carried a `FINDING` block
//! asserting what the engine *did* while saying what it *should* do. Three of
//! those defects are now fixed in `src/**` -- an exhausted guest heap is a
//! named budget, a panicking bridge is a contained `Door` failure, and `check`
//! puts the compiler's own output through the load gate -- and their tests now
//! assert the fixed behaviour, keeping the reproducer that found it. The two
//! that remain (`finding_3`, `finding_5`) are upstream in `tinyvm-qjs`; their
//! blocks say what to change them to when upstream closes them.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agenterm_qjswasm::{
    Budget, Engine, FleetBridgeFn, Guest, JsValue, Outcome, QjswasmError, Value, compile_qjs,
};

// =========================================================================
// Harness
// =========================================================================

/// A bridge plus the `(op, params)` pairs it saw, so a test can prove the
/// script's own bytes arrived -- not merely that something was called.
struct Bridge {
    call: FleetBridgeFn,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl Bridge {
    fn new(answer: impl Fn(&str, &str) -> Result<String, String> + Send + Sync + 'static) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        Self {
            call: Arc::new(move |op: &str, params: &str| {
                log.lock()
                    .unwrap()
                    .push((op.to_string(), params.to_string()));
                answer(op, params)
            }),
            seen,
        }
    }

    /// Every call, in order.
    fn seen(&self) -> Vec<(String, String)> {
        self.seen.lock().unwrap().clone()
    }
}

/// A bridge that answers every call with the same text.
fn answering(text: &str) -> Bridge {
    let text = text.to_string();
    Bridge::new(move |_op, _params| Ok(text.clone()))
}

fn run(source: &str, bridge: Option<FleetBridgeFn>) -> Result<Outcome, QjswasmError> {
    run_with(Budget::default(), source, bridge)
}

fn run_with(
    budget: Budget,
    source: &str,
    bridge: Option<FleetBridgeFn>,
) -> Result<Outcome, QjswasmError> {
    Engine::with_budget(budget).run_once(Guest::Qjs(source), bridge, "main", &[])
}

/// The one JavaScript string a script returned.
#[track_caller]
fn string_from(source: &str, bridge: Option<FleetBridgeFn>) -> String {
    let out = run(source, bridge).unwrap_or_else(|e| panic!("{source:?}: {e}"));
    match out.values.as_slice() {
        [Value::Js(JsValue::Str(s))] => s.clone(),
        other => panic!("{source:?} returned {other:?}, wanted one JS string"),
    }
}

/// The compile diagnostic a script is refused with.
#[track_caller]
fn refused(source: &str) -> String {
    match run(source, None) {
        Err(QjswasmError::Compile(e)) => e.message,
        other => panic!("{source:?} was not refused at compile time: {other:?}"),
    }
}

/// A budget whose only unusual field is `max_memory_pages`.
fn pages(n: usize) -> tinyvm::Limits {
    tinyvm::Limits {
        max_memory_pages: n,
        ..tinyvm::Limits::default()
    }
}

/// `fleet_call` then `fleet_result`, the shape almost every attack below uses.
const CALL_THEN_FETCH: &str = "let status = fleet_call(\"o\", \"p\"); return fleet_result();";

// =========================================================================
// Calling a door function wrongly
// =========================================================================

/// Every arity mistake is settled before the script runs, and the diagnostic
/// carries both numbers -- what the door takes and what the call passed --
/// because a reader who only learns that something is wrong has to go looking
/// for the table.
#[test]
fn a_wrong_argument_count_is_refused_at_compile_time_with_both_numbers() {
    for (source, wants, gives) in [
        ("print(); return 0;", 1, 0),
        ("print(\"a\", \"b\"); return 0;", 1, 2),
        ("return fleet_call(\"o\");", 2, 1),
        ("return fleet_call(\"o\", \"p\", \"q\");", 2, 3),
        ("return fleet_result(1);", 0, 1),
    ] {
        let message = refused(source);
        assert!(
            message.contains(&format!("with {wants} argument(s)"))
                && message.contains(&format!("passes {gives}")),
            "{source:?}: {message}"
        );
        assert!(
            message.starts_with("this engine "),
            "the diagnostic must name the engine's boundary, not the script: {message}"
        );
    }
}

/// A value whose type the compiler can settle without running anything is
/// refused there, naming the argument position and the declared type. This is
/// the half of the type policy a script author can act on.
#[test]
fn a_statically_settled_wrong_type_is_refused_at_compile_time() {
    for (source, got) in [
        ("print(1); return 0;", "a Number"),
        ("print(true); return 0;", "a Boolean"),
        ("print(null); return 0;", "Null"),
        ("print(undefined); return 0;", "Undefined"),
        ("print(-1); return 0;", "a Number"),
        ("return fleet_call(\"o\", 2);", "a Number"),
    ] {
        let message = refused(source);
        assert!(
            message.contains(&format!("cannot pass {got}")) && message.contains("a String"),
            "{source:?}: {message}"
        );
        assert!(message.starts_with("this engine "), "{message}");
    }
}

/// FINDING 5 (low, legibility) -- a runtime type mismatch at the door is a
/// bare `unreachable`.
///
/// ```text
/// reproducer  let x = 1; print(x); return 0;
/// observed    Trap("unreachable executed")
/// expected    a fault that names `print`, the argument, and the declared type
/// ```
///
/// Nothing *wrong* happens: the host is not handed a fabricated pointer, and
/// the failure is a typed `QjswasmError::Trap`, which is the correct class --
/// the guest did run. What is missing is any way to tell this trap from a
/// division by zero. Compare the message the same mistake gets one line
/// earlier, in [`a_statically_settled_wrong_type_is_refused_at_compile_time`].
///
/// The cause was upstream and deliberate: `unbox_string` -> `require_tag`
/// emitted a bare `unreachable`, "the runtime half of the policy whose
/// compile-time half is `static_type`". Fixed upstream in tinyvm 1012da1:
/// the guest records `"<host>#<n>"` (the sixth fault code) before the trap,
/// and this crate reads it back as [`QjswasmError::HostArgument`]. This is
/// the assertion the old test said to write when that day came.
#[test]
fn finding_5_a_runtime_type_mismatch_names_the_door_and_the_argument() {
    for (source, host) in [
        ("let x = 1; print(x); return 0;", "print"),
        ("let x = null; print(x); return 0;", "print"),
        ("let x = 1; return fleet_call(x, \"p\");", "fleet_call"),
    ] {
        match run(source, Some(answering("never").call)) {
            Err(QjswasmError::HostArgument(Some((named, position)))) => {
                assert_eq!(named, host, "{source:?}");
                assert_eq!(position, 1, "{source:?}");
            }
            other => panic!("{source:?}: expected the door and argument named, got {other:?}"),
        }
    }
}

/// A door name the script does not define is refused, and the door is listed.
/// A door name the script *does* define shadows it -- checked next.
#[test]
fn a_near_miss_name_is_refused_and_the_whole_door_is_listed() {
    let message = refused("return fleet_calll(\"o\", \"p\");");
    assert!(message.contains("fleet_calll"), "{message}");
    for offered in ["print", "fleet_call", "fleet_result"] {
        assert!(message.contains(offered), "{message}");
    }
}

/// A script may define its own `print`. The door does not win over a binding
/// the script wrote, in any of the three shapes a binding comes in.
///
/// This is the difference between a declaration table and a set of reserved
/// words, and it is worth a test because getting it backwards would be silent:
/// a script that defined `print` to collect output would instead be shipping
/// its callers' text to the host.
#[test]
fn a_script_binding_shadows_the_door_rather_than_the_other_way_round() {
    assert_eq!(
        string_from(
            "function print(x) { return x; } return print(\"mine\");",
            None
        ),
        "mine"
    );
    assert_eq!(
        string_from("let print = \"mine\"; return print;", None),
        "mine"
    );
    assert_eq!(
        string_from(
            "function f(fleet_result) { return fleet_result; } return f(\"mine\");",
            None
        ),
        "mine"
    );
    // And the shadowing script imports nothing: the door is not merely
    // out-voted at the call site, it is absent from the module.
    assert!(
        compile_qjs("function print(x) { return x; } return print(\"mine\");")
            .expect("compiles")
            .windows(8)
            .all(|w| w != b"agenterm"),
        "a script that shadows `print` still imports the door"
    );
}

/// FINDING 3 (medium) -- mentioning a zero-argument door function calls it.
///
/// ```text
/// reproducer  return typeof fleet_result;
/// observed    "string"        (the door was called; its answer was typeof'd)
/// expected    "function"      (ECMA-262 13.5.3 on a callable binding)
///
/// reproducer  let f = fleet_result; return f;
/// observed    ""              (the door was called at the mention)
/// expected    a diagnostic, or a function value
/// ```
///
/// The rule behind it is upstream's, in `tinyvm-qjs` `src/emit.rs:857`: "A bare
/// host name is a zero-argument call, as it is at M0." Under `Names::Declared`
/// that rule stops being harmless, because the declarations now have arities.
/// It shows up twice in agenterm's table:
///
/// - `fleet_result` takes no arguments, so a bare mention *is* a well-formed
///   call and silently performs one.
/// - `print` takes one, so the same mention is a compile error -- and the
///   error describes a call the script never wrote ("with 1 argument(s), and
///   this call passes 0"), which reads as a bug in the script.
///
/// Both are the same defect seen from two arities, and the second is the one
/// that will waste an author's afternoon.
///
/// When it is fixed: `typeof fleet_result` becomes `"function"` or a
/// diagnostic, and the `print` case gets a diagnostic about referencing a host
/// function rather than an argument count.
#[test]
fn finding_3_a_bare_door_name_is_a_call() {
    assert_eq!(
        string_from("return typeof fleet_result;", None),
        "string",
        "FIXED if this is now `function` -- update this test",
    );
    let bridge = answering("called!");
    let out = run("let f = fleet_result; return f;", Some(bridge.call)).expect("runs");
    assert_eq!(out.values, vec![Value::Js(JsValue::Str(String::new()))]);

    // The same shape at arity 1 is a compile error whose text describes a call
    // the script never wrote.
    let message = refused("return typeof print;");
    assert!(
        message.contains("this call passes 0"),
        "FIXED if the diagnostic no longer invents a call: {message}"
    );
}

// =========================================================================
// What crosses the door as a string
// =========================================================================

/// An empty string is a string. All four positions take one, and none of them
/// mistakes it for absence.
#[test]
fn empty_strings_cross_the_door_in_both_directions() {
    let bridge = Bridge::new(|op, params| Ok(format!("[{op}|{params}]")));
    let out = run(
        "print(\"\"); let status = fleet_call(\"\", \"\"); return fleet_result();",
        Some(Arc::clone(&bridge.call)),
    )
    .expect("runs");
    assert_eq!(out.stdout, "\n", "an empty print still ends a line");
    assert_eq!(out.values, vec![Value::Js(JsValue::Str("[|]".into()))]);
    assert_eq!(bridge.seen(), vec![(String::new(), String::new())]);

    // A bridge that answers with nothing is not "no answer".
    assert_eq!(string_from(CALL_THEN_FETCH, Some(answering("").call)), "");
}

/// `print("")` under a zero-byte cap writes nothing and is *not* flagged
/// truncated: the flag means "you lost bytes", and no bytes were lost.
#[test]
fn an_empty_print_is_not_a_truncation() {
    // One byte of room, which is exactly what an empty print needs: `print`
    // ends a line, so the newline is the only byte it writes. A zero budget
    // would make even that a truncation, which is a true but different fact --
    // asserted separately below so neither claim hides the other.
    let budget = Budget {
        max_stdout_bytes: 1,
        ..Budget::default()
    };
    let out = run_with(budget.clone(), "print(\"\"); return 0;", None).expect("runs");
    assert_eq!((out.stdout.as_str(), out.truncated_stdout), ("\n", false));

    let out = run_with(budget, "print(\"abc\"); return 0;", None).expect("runs");
    assert_eq!((out.stdout.as_str(), out.truncated_stdout), ("a", true));

    // At zero the newline itself is the lost byte, and the flag says so.
    let none_at_all = Budget {
        max_stdout_bytes: 0,
        ..Budget::default()
    };
    let out = run_with(none_at_all, "print(\"\"); return 0;", None).expect("runs");
    assert_eq!((out.stdout.as_str(), out.truncated_stdout), ("", true));
}

/// A NUL is an ordinary byte of a JavaScript string, and the door carries
/// `(ptr, len)` rather than a C string, so it must survive with the length
/// intact rather than cutting the value short.
#[test]
fn a_nul_byte_crosses_the_door_without_truncating_anything() {
    let bridge = Bridge::new(|op, _params| Ok(op.to_string()));
    let out = run(
        "print(\"a\\u0000b\"); let s = fleet_call(\"n\\u0000ul\", \"\"); return fleet_result();",
        Some(Arc::clone(&bridge.call)),
    )
    .expect("runs");
    assert_eq!(out.stdout, "a\0b\n");
    assert_eq!(bridge.seen(), vec![("n\0ul".to_string(), String::new())]);
    assert_eq!(out.values, vec![Value::Js(JsValue::Str("n\0ul".into()))]);
}

/// A `.qjs` guest cannot present invalid UTF-8 at the door, so `host.rs`'s
/// `NOT_UTF8` status-1 path is unreachable from this language and remains a
/// hand-written-guest concern only.
///
/// The engine closes the two routes that would produce it: an unpaired
/// surrogate is refused in the lexer, and `\xNN` is a code point, not a byte.
#[test]
fn a_qjs_script_cannot_hand_the_door_invalid_utf8() {
    for source in [
        "print(\"a\\ud800b\"); return 0;",
        "return fleet_call(\"\\udfff\", \"p\");",
    ] {
        let message = refused(source);
        assert!(
            message.contains("unpaired surrogates"),
            "{source:?}: {message}"
        );
    }
    // `\xff` is U+00FF, which is two well-formed UTF-8 bytes, and a surrogate
    // *pair* is one astral code point.
    let bridge = Bridge::new(|op, _params| Ok(op.to_string()));
    let out = run(
        "print(\"a\\xffb\"); return fleet_call(\"\\ud83d\\ude00\", \"p\");",
        Some(Arc::clone(&bridge.call)),
    )
    .expect("runs");
    assert_eq!(out.stdout, "a\u{ff}b\n");
    assert_eq!(
        bridge.seen(),
        vec![("\u{1f600}".to_string(), "p".to_string())]
    );
}

/// A string built at run time lives on the bump heap, a literal lives in the
/// data segment, and the door has to read both the same way -- the record's
/// four-byte header is the engine's business and must never reach the bridge.
#[test]
fn heap_built_and_literal_strings_arrive_identically() {
    let bridge = Bridge::new(|_op, _params| Ok("x".to_string()));
    run(
        "let s = fleet_call(\"a\" + \"b\", \"c\" + \"d\");
         let t = fleet_call(\"ab\", \"cd\");
         return 0;",
        Some(Arc::clone(&bridge.call)),
    )
    .expect("runs");
    let seen = bridge.seen();
    assert_eq!(
        seen[0], seen[1],
        "a heap string and a literal must look alike"
    );
    assert_eq!(seen[0], ("ab".to_string(), "cd".to_string()));
}

/// A hundred kilobytes through `print` and through `fleet_call`, byte-exact.
/// Big enough that a length taken from the wrong place, or a copy bounded by
/// the wrong number, would not survive.
#[test]
fn a_hundred_kilobyte_argument_arrives_whole() {
    let big = "y".repeat(100_000);
    let bridge = Bridge::new(|op, _params| Ok(op.len().to_string()));
    let out = run(
        &format!("print(\"{big}\"); let s = fleet_call(\"{big}\", \"p\"); return fleet_result();"),
        Some(Arc::clone(&bridge.call)),
    )
    .expect("runs");
    assert_eq!(
        out.stdout.len(),
        100_001,
        "100 KiB of payload plus print's newline"
    );
    assert!(out.stdout.trim_end_matches('\n').bytes().all(|b| b == b'y'));
    assert_eq!(bridge.seen()[0].0.len(), 100_000);
    assert_eq!(out.values, vec![Value::Js(JsValue::Str("100000".into()))]);
}

/// Arguments are evaluated left to right, once each, before any of them is
/// unwrapped onto a raw parameter. The `Bytes` fetch pushes its raw parameters
/// twice, so "evaluate first, unwrap later" is the only order that does not
/// repeat a side effect.
#[test]
fn door_arguments_are_evaluated_left_to_right_exactly_once() {
    let bridge = Bridge::new(|op, params| Ok(format!("{op}/{params}")));
    let out = run(
        "function tag(t) { print(t); return t; }
         let s = fleet_call(tag(\"first\"), tag(\"second\"));
         return fleet_result();",
        Some(Arc::clone(&bridge.call)),
    )
    .expect("runs");
    assert_eq!(
        out.stdout, "first\nsecond\n",
        "each print ends its own line, as it does on the engine this replaces"
    );
    assert_eq!(
        bridge.seen(),
        vec![("first".to_string(), "second".to_string())]
    );
    assert_eq!(
        out.values,
        vec![Value::Js(JsValue::Str("first/second".into()))]
    );
}

// =========================================================================
// The pending buffer
// =========================================================================

/// Fetching before any call is the empty string, not a fault and not stale
/// bytes -- and it costs a real host round trip, so the length pass has to
/// answer 0 rather than leaving the guest to allocate a garbage length.
#[test]
fn fetching_before_any_call_is_the_empty_string() {
    assert_eq!(string_from("return fleet_result();", None), "");
    assert_eq!(
        string_from("return fleet_result();", Some(answering("unused").call)),
        "",
        "the bridge is not consulted by a fetch"
    );
}

/// Fetching twice yields the same bytes twice. The pending buffer survives
/// collection by design, and the second fetch is a second allocation on the
/// guest's heap -- so this also proves the two-pass shape is re-entrant
/// against itself.
#[test]
fn fetching_twice_yields_the_same_answer_twice() {
    let bridge = answering("ab");
    assert_eq!(
        string_from(
            "let s = fleet_call(\"o\", \"p\"); return fleet_result() + fleet_result();",
            Some(bridge.call)
        ),
        "abab"
    );
}

/// Two fetches inside one argument list, feeding the door its own answer.
/// Both must see the same pending bytes, and the first argument's pointer must
/// still be good after the second fetch has allocated (and possibly grown
/// memory) behind it.
#[test]
fn the_door_can_be_fed_its_own_answer() {
    let bridge = Bridge::new(|op, params| Ok(format!("[{op}|{params}]")));
    let out = run(
        "let seed = fleet_call(\"seed\", \"P\");
         let s = fleet_call(fleet_result(), fleet_result());
         return fleet_result();",
        Some(Arc::clone(&bridge.call)),
    )
    .expect("runs");
    assert_eq!(
        bridge.seen(),
        vec![
            ("seed".to_string(), "P".to_string()),
            ("[seed|P]".to_string(), "[seed|P]".to_string()),
        ]
    );
    assert_eq!(
        out.values,
        vec![Value::Js(JsValue::Str("[[seed|P]|[seed|P]]".into()))]
    );
}

/// One slot's pending answer is invisible to another slot in the same engine,
/// including a slot with no bridge at all.
#[test]
fn the_pending_buffer_is_per_slot() {
    let mut engine = Engine::new();
    let a = engine
        .spawn(
            Guest::Qjs("let s = fleet_call(\"A\", \"p\"); return fleet_result();"),
            Some(Bridge::new(|op, _p| Ok(format!("from-{op}"))).call),
        )
        .expect("spawns");
    let b = engine
        .spawn(Guest::Qjs("return fleet_result();"), None)
        .expect("spawns");

    let first = engine.call(a, "main", &[]).expect("A runs");
    assert_eq!(first.values, vec![Value::Js(JsValue::Str("from-A".into()))]);
    let other = engine.call(b, "main", &[]).expect("B runs");
    assert_eq!(
        other.values,
        vec![Value::Js(JsValue::Str(String::new()))],
        "B never called the bridge and must not see A's answer"
    );
    let again = engine.call(a, "main", &[]).expect("A runs again");
    assert_eq!(again.values, vec![Value::Js(JsValue::Str("from-A".into()))]);
}

// =========================================================================
// Caps, budgets, and the edges between them
// =========================================================================

/// Both result caps refuse exactly one byte past themselves and accept exactly
/// themselves, and they refuse in their own separate ways: the bridge cap is
/// an application-level status the script can branch on, the seam cap is an
/// engine-level budget the script never sees.
#[test]
fn both_result_caps_refuse_at_the_byte_after_the_cap() {
    // `max_bridge_result_bytes`: the door replaces the answer with its own
    // bounded message, so the script sees status 1.
    for (n, expected) in [
        (4usize, "abcd".to_string()),
        (
            5,
            "agenterm: fleet result exceeds the slot's max_bridge_result_bytes".to_string(),
        ),
    ] {
        let budget = Budget {
            max_bridge_result_bytes: 4,
            ..Budget::default()
        };
        let got = run_with(budget, CALL_THEN_FETCH, Some(answering(&"abcde"[..n]).call))
            .expect("an over-cap answer is a normal result");
        assert_eq!(got.values, vec![Value::Js(JsValue::Str(expected))]);
    }

    // `max_result_string_bytes`: a refusal at the seam, never a prefix.
    for (n, ok) in [(16usize, true), (17, false)] {
        let budget = Budget {
            max_result_string_bytes: 16,
            ..Budget::default()
        };
        let got = run_with(
            budget,
            CALL_THEN_FETCH,
            Some(answering(&"x".repeat(n)).call),
        );
        match (got, ok) {
            (Ok(out), true) => {
                assert_eq!(out.values, vec![Value::Js(JsValue::Str("x".repeat(16)))])
            }
            (Err(QjswasmError::Budget(what)), false) => {
                assert_eq!(what, "max_result_string_bytes")
            }
            (other, _) => panic!("n={n}: {other:?}"),
        }
    }
}

/// The step budget can land anywhere in the two-pass fetch. Whatever it cuts,
/// the call either produces the exact answer or reports
/// `Budget("max_steps")` -- never a short string, never a fabricated tail,
/// never a panic.
///
/// This is the assertion the two-pass design most needs: the fetch is
/// length-then-copy across two host calls with a guest allocation between
/// them, and a budget expiring in the middle is the one way a partially built
/// string could become visible.
#[test]
fn a_step_budget_landing_inside_the_two_pass_fetch_never_yields_half_an_answer() {
    for steps in 1u64..=400 {
        let budget = Budget {
            limits: tinyvm::Limits {
                max_steps: steps,
                ..tinyvm::Limits::default()
            },
            ..Budget::default()
        };
        let got = run_with(
            budget,
            "print(\"p\"); let s = fleet_call(\"o\", \"p\"); return fleet_result();",
            Some(answering("abcd").call),
        );
        match got {
            Ok(out) => assert_eq!(
                out.values,
                vec![Value::Js(JsValue::Str("abcd".into()))],
                "max_steps={steps} produced a value that is not the whole answer"
            ),
            Err(QjswasmError::Budget(what)) => assert_eq!(what, "max_steps"),
            Err(other) => panic!("max_steps={steps}: {other:?}"),
        }
    }
}

/// The same, for a loop that crosses the door repeatedly and accumulates the
/// answers -- so the cut can also land between iterations, and between a
/// bridge call and the fetch that collects it.
#[test]
fn a_step_budget_landing_inside_a_door_loop_never_yields_a_partial_accumulation() {
    for steps in (1u64..=4000).step_by(13) {
        let budget = Budget {
            limits: tinyvm::Limits {
                max_steps: steps,
                ..tinyvm::Limits::default()
            },
            ..Budget::default()
        };
        let got = run_with(
            budget,
            "let acc = \"\";
             for (let i = 0; i < 5; i = i + 1) {
                 let s = fleet_call(\"o\", \"p\");
                 acc = acc + fleet_result();
             }
             return acc;",
            Some(answering("abcd").call),
        );
        match got {
            Ok(out) => assert_eq!(
                out.values,
                vec![Value::Js(JsValue::Str("abcd".repeat(5)))],
                "max_steps={steps}"
            ),
            Err(QjswasmError::Budget(what)) => assert_eq!(what, "max_steps"),
            Err(other) => panic!("max_steps={steps}: {other:?}"),
        }
    }
}

/// Recursion whose every frame crosses the door exhausts the call-depth
/// budget and says so. A host call is an ordinary wasm frame; it neither
/// escapes the depth accounting nor re-enters the guest.
#[test]
fn recursion_through_a_host_call_is_a_call_depth_budget() {
    let out = run(
        "function f(n) { print(\"x\"); if (n === 0) { return 0; } return f(n - 1); }
         return f(1000);",
        None,
    );
    match out {
        Err(QjswasmError::Budget(what)) => assert_eq!(what, "max_call_depth"),
        other => panic!("expected a depth budget, got {other:?}"),
    }
    // Below the ceiling the same shape completes, so the test above is
    // measuring the ceiling and not a broken script.
    let out = run(
        "function f(n) { print(\"x\"); if (n === 0) { return 0; } return f(n - 1); }
         return f(20);",
        None,
    )
    .expect("21 frames fit");
    // Twenty-one frames, each printing one byte and ending its line.
    assert_eq!(out.stdout.len(), 42);
}

/// A call that trapped loses its own output -- a stated cost in `src/slot.rs`
/// -- but must not donate it to the *next* call on the same slot.
#[test]
fn output_from_a_trapped_call_is_not_carried_into_the_next_one() {
    // Call 1: the bridge errors, so `status === 1`, so the script prints a
    // Number at the door and traps. Call 2: the bridge succeeds, no trap.
    let calls = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&calls);
    let bridge: FleetBridgeFn = Arc::new(move |_op: &str, _params: &str| {
        let mut n = counter.lock().unwrap();
        *n += 1;
        if *n == 1 {
            Err("nope".to_string())
        } else {
            Ok("fine".to_string())
        }
    });
    let mut engine = Engine::new();
    let slot = engine
        .spawn(
            Guest::Qjs(
                "print(\"once\");
                 let status = fleet_call(\"o\", \"p\");
                 if (status === 1) { print(status); }
                 return fleet_result();",
            ),
            Some(bridge),
        )
        .expect("spawns");

    assert!(matches!(
        engine.call(slot, "main", &[]),
        Err(QjswasmError::HostArgument(_))
    ));
    let second = engine
        .call(slot, "main", &[])
        .expect("the slot is still live");
    assert_eq!(
        second.stdout, "once\n",
        "the second call's stdout is its own, not both calls'"
    );
    assert_eq!(second.values, vec![Value::Js(JsValue::Str("fine".into()))]);
}

// =========================================================================
// Findings: the guest heap
// =========================================================================

/// A bridge answer the guest's linear memory cannot hold is
/// `Budget("max_memory_pages")` -- the field an embedder would raise -- and
/// not the bare `Trap("unreachable executed")` it used to be.
///
/// ```text
/// reproducer  max_memory_pages = 2 (128 KiB); bridge answers 200 000 bytes;
///             let s = fleet_call("o","p"); return fleet_result();
/// was         Err(Trap("unreachable executed"))     (FINDING 1)
/// now         Err(Budget("max_memory_pages"))
/// ```
///
/// The information used to be destroyed inside the guest: `tinyvm-qjs`'s
/// `__alloc` lowered a refused `memory.grow` to a bare `unreachable`, which
/// reaches the core as an ordinary `WasmFaultClass::Guest` fault
/// indistinguishable from a broken script. The guest now writes
/// `FAULT_HEAP_EXHAUSTED` into the first word of its own memory before it
/// gives up, and `src/slot.rs` reads it back through
/// `tinyvm_qjs::guest_fault` on the error path. No host-side heuristic is
/// involved -- the guest states the reason, the seam repeats it.
///
/// The bridge is the party that sizes this allocation, which is why the case
/// matters: a host capability returning a large-but-in-budget answer must not
/// be reported as the script's own fault.
#[test]
fn a_bridge_answer_too_large_for_the_guest_heap_is_a_page_budget() {
    let budget = Budget {
        limits: pages(2),
        // Deliberately generous, so neither host-side cap is what refuses:
        // 200 000 bytes is inside both, and only the guest's 128 KiB is not.
        max_bridge_result_bytes: 4 << 20,
        max_result_string_bytes: 4 << 20,
        ..Budget::default()
    };
    match run_with(
        budget.clone(),
        CALL_THEN_FETCH,
        Some(answering(&"z".repeat(200_000)).call),
    ) {
        Err(QjswasmError::Budget("max_memory_pages")) => {}
        other => panic!("expected the page budget to be named, got {other:?}"),
    }

    // The same slot size handles an answer that fits, so the refusal above is
    // the heap running out and not the door failing at 128 KiB.
    let got = run_with(budget, CALL_THEN_FETCH, Some(answering("small").call))
        .expect("an answer that fits is fine in the same slot");
    assert_eq!(got.values, vec![Value::Js(JsValue::Str("small".into()))]);
}

/// A script that exhausts the heap on its own -- no bridge, no door -- is the
/// same budget, so the classification is about the guest's allocator and not
/// about who handed it the bytes.
#[test]
fn a_script_that_exhausts_the_heap_by_itself_is_the_same_budget() {
    let budget = Budget {
        limits: tinyvm::Limits {
            max_memory_pages: 2,
            max_steps: 100_000_000,
            ..tinyvm::Limits::default()
        },
        ..Budget::default()
    };
    let source = "let s = \"0123456789abcdef\"; let i = 0; \
                  while (i < 20) { s = s + s; i = i + 1; } return s;";
    match run_with(budget, source, None) {
        Err(QjswasmError::Budget("max_memory_pages")) => {}
        other => panic!("expected the page budget to be named, got {other:?}"),
    }
}

/// A script that is simply broken keeps its own diagnosis: `unreachable` from
/// a runtime type error is still a `Trap`, not a budget. The fault word is
/// what separates them, and it is cleared on the way into every call, so one
/// call's exhaustion cannot mislabel the next call's genuine fault.
#[test]
fn a_broken_script_is_not_relabelled_as_a_budget() {
    let mut engine = Engine::with_budget(Budget {
        limits: pages(2),
        ..Budget::default()
    });
    let slot = engine
        .spawn(
            Guest::Qjs(
                "let x = 1; let s = \"a\"; while (x < 200000) { s = s + s; x = x + x; } return s;",
            ),
            None,
        )
        .expect("spawns");
    // First: exhaust the heap, which is a budget.
    match engine.call(slot, "main", &[]) {
        Err(QjswasmError::Budget("max_memory_pages")) => {}
        other => panic!("expected a page budget first, got {other:?}"),
    }
    // Then a fresh slot whose fault is its own: a number where the door wants
    // a string traps, and must not inherit the label.
    let broken = engine
        .spawn(Guest::Qjs("let x = 1; print(x); return 0;"), None)
        .expect("spawns");
    match engine.call(broken, "main", &[]) {
        Err(QjswasmError::HostArgument(Some((host, 1)))) => assert_eq!(host, "print"),
        other => panic!("expected the script's own fault (print#1), got {other:?}"),
    }
}

/// The same exhaustion on a persistent slot: the bump pointer is advanced
/// *before* the grow is attempted, so once it overshoots, every later
/// allocation in that slot fails however small. That does not recover, and
/// this test is the statement that it does not -- every later call says
/// `Budget("max_memory_pages")` rather than trapping opaquely, and
/// `Engine::call`'s documentation now says a heap-exhausted `.qjs` slot is
/// spent.
///
/// ```text
/// reproducer  default budget (256 pages = 16 MiB); one slot, called 25 times;
///             bridge answers 1 MiB for the first 20 calls, then 4 bytes
/// was         calls 16..25 Trap("unreachable executed")          (FINDING 1b)
/// now         calls 16..25 Budget("max_memory_pages"), including the ones
///             whose answers are four bytes
/// ```
///
/// Note what stayed inside its stated limits the whole way: every answer is
/// under `max_bridge_result_bytes`, every call under `max_steps`, and the
/// module never declares more memory than `max_memory_pages`. There is still
/// no cap on the *cumulative* bytes the door writes into a guest's heap --
/// sixteen well-behaved answers are enough to spend a slot -- which is why
/// the refusal has to name the budget an embedder can act on.
#[test]
fn heap_exhaustion_is_a_budget_on_every_later_call_and_the_slot_does_not_recover() {
    let calls = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&calls);
    let bridge: FleetBridgeFn = Arc::new(move |_op: &str, _params: &str| {
        let mut n = counter.lock().unwrap();
        *n += 1;
        Ok(if *n <= 20 {
            "z".repeat(1 << 20)
        } else {
            "tiny".to_string()
        })
    });
    let mut engine = Engine::with_budget(Budget {
        max_result_string_bytes: 4 << 20,
        ..Budget::default()
    });
    let slot = engine
        .spawn(
            Guest::Qjs("let s = fleet_call(\"o\", \"p\"); let t = fleet_result(); return 1;"),
            Some(bridge),
        )
        .expect("spawns");

    let mut first_failure = None;
    for i in 1..=25 {
        match engine.call(slot, "main", &[]) {
            Ok(_) => assert!(
                first_failure.is_none(),
                "call {i} recovered after a failure"
            ),
            Err(QjswasmError::Budget("max_memory_pages")) => {
                first_failure.get_or_insert(i);
            }
            Err(other) => panic!("call {i}: {other:?}"),
        };
    }
    let at = first_failure.expect("16 MiB of 1 MiB answers must exhaust a 16 MiB guest");
    assert!(
        (10..=20).contains(&at),
        "expected the heap to run out around 16 one-megabyte answers, not at {at}"
    );
    // Calls 21..=25 asked for four bytes and were still refused: the slot
    // never recovers, and `Engine::live_slots` still counts it. Reclaiming it
    // is the caller's move, as it is for any other failure.
    assert_eq!(engine.live_slots(), 1);
}

// =========================================================================
// Findings: the host side
// =========================================================================

/// A panicking bridge is contained at the door: the call fails with
/// `QjswasmError::Door` naming the panic, the unwind does not escape into the
/// embedder, and `run_once` reclaims the slot it promised to reclaim.
///
/// ```text
/// reproducer  run_once with a bridge whose closure panics
/// was         the panic propagated out of run_once; live_slots() == 1  (FINDING 2)
/// now         Err(Door("the fleet bridge panicked ...")); live_slots() == 0
/// ```
///
/// Why a `Door` error and not a status the script can branch on: a panicking
/// bridge is a defect in the *host* capability, not an application-level
/// "no" like `Err(message)`. Handing the guest a status would let a script
/// carry on as though it had been answered, and would hide the bug from the
/// embedder entirely. Why not let it unwind: the guest chooses the `op`
/// string, so a script can steer a bridge into whichever of its paths panics,
/// and under a `panic = "abort"` profile that is a guest-triggered process
/// abort.
///
/// The engine is undamaged either way, which this also asserts: a later slot
/// in the same engine, with the same bridge, runs correctly -- the door's
/// `RefCell` is never left borrowed, because `src/host.rs` calls the bridge
/// before it borrows.
#[test]
fn a_panicking_bridge_is_a_door_error_and_run_once_still_reclaims() {
    let mut engine = Engine::new();
    let bridge: FleetBridgeFn = Arc::new(|op: &str, _params: &str| {
        if op == "boom" {
            panic!("the bridge exploded");
        }
        Ok(format!("<{op}>"))
    });

    // Silence the panic hook: the panic is expected, and its default report on
    // stderr is noise in a test log.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let refusal = engine.run_once(
        Guest::Qjs(
            "print(\"before\"); let s = fleet_call(\"boom\", \"p\"); return fleet_result();",
        ),
        Some(Arc::clone(&bridge)),
        "main",
        &[],
    );
    std::panic::set_hook(previous);

    match refusal {
        Err(QjswasmError::Door(message)) => {
            assert!(
                message.contains("panicked"),
                "the diagnostic does not say a panic happened: {message}"
            );
            assert!(
                message.contains("the bridge exploded"),
                "the panic's own message is lost: {message}"
            );
            assert!(
                message.contains("boom"),
                "the diagnostic does not say which op was being served: {message}"
            );
        }
        other => panic!("expected a contained door failure, got {other:?}"),
    }
    assert_eq!(
        engine.live_slots(),
        0,
        "run_once is documented as spawn, call and reclaim"
    );

    // Nothing else is damaged: the same engine, the same bridge, a new slot.
    let out = engine
        .run_once(
            Guest::Qjs(
                "print(\"after\"); let s = fleet_call(\"ok\", \"p\"); return fleet_result();",
            ),
            Some(bridge),
            "main",
            &[],
        )
        .expect("the engine still works after a bridge panicked");
    assert_eq!(out.values, vec![Value::Js(JsValue::Str("<ok>".into()))]);
    assert_eq!(out.stdout, "after\n");
    assert_eq!(engine.live_slots(), 0);
}

/// A panic with a payload that is neither `&str` nor `String` is still
/// contained and still says a panic happened -- the message just cannot quote
/// it. A door that only handled the two common payloads would let an exotic
/// one through as an unwind.
#[test]
fn a_bridge_panicking_with_an_exotic_payload_is_still_contained() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let refusal = Engine::new().run_once(
        Guest::Qjs(CALL_THEN_FETCH),
        Some(Arc::new(|_op: &str, _params: &str| {
            std::panic::panic_any(7u8);
        })),
        "main",
        &[],
    );
    std::panic::set_hook(previous);

    match refusal {
        Err(QjswasmError::Door(message)) => assert!(
            message.contains("panicked"),
            "the diagnostic does not say a panic happened: {message}"
        ),
        other => panic!("expected a contained door failure, got {other:?}"),
    }
}

/// A slot whose bridge panicked stays live and callable, and the *next* call
/// is answered normally. The containment is per call, not a poisoning of the
/// slot: the panic happened on the host side of the door and touched nothing
/// in the guest's heap.
#[test]
fn a_slot_survives_a_bridge_panic_and_answers_the_next_call() {
    // An `AtomicUsize` rather than a `Mutex`: the bridge panics, and a panic
    // while holding a lock poisons it -- which would make the *test* the thing
    // that fails on the second call.
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let bridge: FleetBridgeFn = Arc::new(move |_op: &str, _params: &str| {
        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 {
            panic!("the bridge exploded");
        }
        Ok(format!("answer {n}"))
    });
    let mut engine = Engine::new();
    let slot = engine
        .spawn(Guest::Qjs(CALL_THEN_FETCH), Some(bridge))
        .expect("spawns");

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let first = engine.call(slot, "main", &[]);
    std::panic::set_hook(previous);
    assert!(
        matches!(first, Err(QjswasmError::Door(_))),
        "expected a contained door failure, got {first:?}"
    );

    let second = engine
        .call(slot, "main", &[])
        .expect("the slot is still live");
    assert_eq!(
        second.values,
        vec![Value::Js(JsValue::Str("answer 2".into()))]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// `check` refuses a `.qjs` script `execute` could not load: the compiler's
/// own output goes through the same load gate its bytes will later have to
/// pass.
///
/// ```text
/// reproducer  a script whose string literals need more pages than the budget
///             allows; here max_memory_pages = 1 and a 100 000-byte literal
/// was         compile_qjs -> Ok;  run_once -> Err(Load("memory page limit"))  (FINDING 4)
/// now         check_qjs_with -> Err(Load("memory page limit")), the same
///             diagnostic execute gives
/// ```
///
/// `compile_qjs` itself is unchanged and still compile-only -- it is the
/// compiler's face, and a caller who wants bytes wants them whatever budget
/// they will run under. What was missing was a *check* that spends the budget
/// the run will spend, and that is [`agenterm_qjswasm::check_qjs_with`].
#[test]
fn check_refuses_a_script_execute_could_not_load() {
    let source = format!("print(\"{}\"); return 0;", "y".repeat(100_000));
    let budget = Budget {
        limits: pages(1),
        ..Budget::default()
    };

    let refusal = agenterm_qjswasm::check_qjs_with(&source, &budget)
        .expect_err("check must refuse what execute cannot load");
    match refusal {
        QjswasmError::Load(e) => assert_eq!(e.message(), "memory page limit"),
        other => panic!("check refused for the wrong reason: {other:?}"),
    }

    let executed = run_with(budget, &source, None).expect_err("execute refuses it too");
    match executed {
        QjswasmError::Load(e) => assert_eq!(e.message(), "memory page limit"),
        other => panic!("execute refused for the wrong reason: {other:?}"),
    }

    // The bytes themselves are fine -- this is a budget, not a broken module,
    // and the same script checks clean against a budget that can hold it.
    let bytes = compile_qjs(&source).expect("the compiler is not what refuses");
    assert!(!bytes.is_empty());
    agenterm_qjswasm::check_qjs(&source).expect("the default budget has room for one page");
}

/// The gate `check` applies is the load gate and nothing more: it never runs
/// the guest, so a script whose *execution* fails still checks clean. A check
/// that ran the script would have side effects, which is the whole reason
/// `check` exists as a separate verb.
#[test]
fn check_does_not_run_the_script() {
    let seen = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&seen);
    let bridge: FleetBridgeFn = Arc::new(move |_op: &str, _params: &str| {
        *counter.lock().unwrap() += 1;
        Ok(String::new())
    });
    let source = "print(\"side effect\"); let s = fleet_call(\"o\", \"p\"); return 1 / 0;";
    agenterm_qjswasm::check_qjs(source).expect("this compiles and loads");
    assert_eq!(*seen.lock().unwrap(), 0, "check must not call the bridge");

    // And the same source executes, so the check was not passing something
    // unrunnable.
    let out = run(source, Some(bridge)).expect("it runs");
    assert_eq!(out.stdout, "side effect\n");
    assert_eq!(*seen.lock().unwrap(), 1);
}
