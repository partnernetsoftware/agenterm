//! The `agenterm.*` host door, seen from the product face.
//!
//! The door's own mechanism is proved in `src/host.rs`'s unit tests, which
//! drive tinyvm directly (the module is private, so an integration test cannot
//! reach `host::install`). What is only observable from out here is the door
//! *reaching* a caller: a status crossing back as a returned `Value`, printed
//! bytes arriving in `Outcome::stdout`, an out-of-range pointer arriving as a
//! typed `QjswasmError`, and one slot's pending buffer and bridge staying that
//! slot's alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agenterm_qjswasm::{Budget, Engine, FleetBridgeFn, Guest, QjswasmError, Value};

/// The four imports plus a scratch page, written once.
const DOOR_IMPORTS: &str = r#"
    (import "agenterm" "print" (func $print (param i32 i32)))
    (import "agenterm" "fleet_call"
        (func $fleet_call (param i32 i32 i32 i32) (result i32)))
    (import "agenterm" "fleet_result_len" (func $fleet_result_len (result i32)))
    (import "agenterm" "fleet_result" (func $fleet_result (param i32 i32) (result i32)))
"#;

fn guest(rest: &str) -> Vec<u8> {
    wat::parse_str(format!("(module {DOOR_IMPORTS} {rest})")).expect("test guest is valid wat")
}

/// Call the bridge with `"fleet.ping"` and no params, then print the whole
/// answer back and return the status.
fn ping_guest() -> Vec<u8> {
    guest(
        r#"
        (memory 1)
        (data (i32.const 0) "fleet.ping")
        (func (export "main") (result i32)
            (local $status i32)
            (local.set $status (call $fleet_call
                (i32.const 0) (i32.const 10) (i32.const 0) (i32.const 0)))
            (call $print (i32.const 256)
                (call $fleet_result (i32.const 256) (call $fleet_result_len)))
            (local.get $status))
        "#,
    )
}

fn bridge_answering(answer: Result<String, String>) -> FleetBridgeFn {
    Arc::new(move |_op: &str, _params: &str| answer.clone())
}

fn status_of(values: &[Value]) -> i32 {
    match values {
        [Value::I32(status)] => *status,
        other => panic!("expected one i32 status, got {other:?}"),
    }
}

#[test]
fn status_ok_and_the_answer_both_cross_back() {
    let mut engine = Engine::new();
    let outcome = engine
        .run_once(
            Guest::Wasm(&ping_guest()),
            Some(bridge_answering(Ok("pong".to_owned()))),
            "main",
            &[],
        )
        .expect("the guest runs");
    assert_eq!(status_of(&outcome.values), 0);
    assert_eq!(outcome.stdout, "pong");
    assert!(!outcome.truncated_stdout);
}

#[test]
fn a_bridge_error_is_status_1_and_its_message_is_readable() {
    let mut engine = Engine::new();
    let outcome = engine
        .run_once(
            Guest::Wasm(&ping_guest()),
            Some(bridge_answering(Err("no such op".to_owned()))),
            "main",
            &[],
        )
        .expect("an application error is a normal result, not a trap");
    assert_eq!(status_of(&outcome.values), 1);
    assert_eq!(outcome.stdout, "no such op");
}

#[test]
fn no_bridge_is_status_2_with_a_diagnostic_the_guest_can_read() {
    let mut engine = Engine::new();
    let outcome = engine
        .run_once(Guest::Wasm(&ping_guest()), None, "main", &[])
        .expect("the guest runs");
    assert_eq!(status_of(&outcome.values), 2);
    assert!(
        outcome.stdout.contains("bridge"),
        "expected a diagnostic naming the missing capability, got {:?}",
        outcome.stdout
    );
}

#[test]
fn stdout_over_budget_arrives_truncated_and_flagged() {
    let wasm = guest(
        r#"
        (memory 1)
        (data (i32.const 0) "0123456789abcdef")
        (func (export "main") (result i32)
            (call $print (i32.const 0) (i32.const 16))
            (call $print (i32.const 0) (i32.const 16))
            (i32.const 0))
        "#,
    );
    let mut engine = Engine::with_budget(Budget {
        max_stdout_bytes: 10,
        ..Budget::default()
    });
    let outcome = engine
        .run_once(Guest::Wasm(&wasm), None, "main", &[])
        .expect("an over-budget print does not kill the guest");
    assert_eq!(outcome.stdout, "0123456789");
    assert!(outcome.truncated_stdout);
}

/// A refusal, not a prefix: the guest gets a diagnostic it can act on rather
/// than the first `max_bridge_result_bytes` of a document it would then parse.
#[test]
fn an_over_budget_bridge_answer_is_refused_rather_than_cut() {
    let mut engine = Engine::with_budget(Budget {
        max_bridge_result_bytes: 4,
        ..Budget::default()
    });
    let outcome = engine
        .run_once(
            Guest::Wasm(&ping_guest()),
            Some(bridge_answering(Ok("0123456789abcdef".to_owned()))),
            "main",
            &[],
        )
        .expect("the guest runs");
    assert_eq!(status_of(&outcome.values), 1);
    assert!(
        !"0123456789abcdef".starts_with(&outcome.stdout),
        "expected a refusal, got a prefix of the payload: {:?}",
        outcome.stdout
    );
}

#[test]
fn an_out_of_range_door_pointer_fails_the_call_as_a_trap() {
    let wasm = guest(
        r#"
        (memory 1)
        (func (export "main") (result i32)
            (call $fleet_call
                (i32.const 1000000) (i32.const 4) (i32.const 0) (i32.const 0)))
        "#,
    );
    let mut engine = Engine::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&calls);
    let bridge: FleetBridgeFn = Arc::new(move |_op: &str, _params: &str| {
        hits.fetch_add(1, Ordering::SeqCst);
        Ok(String::new())
    });
    let error = engine
        .run_once(Guest::Wasm(&wasm), Some(bridge), "main", &[])
        .expect_err("an out-of-range pointer fails the call");
    assert!(
        matches!(error, QjswasmError::Trap(_)),
        "expected a Trap, got {error:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the door refused before reaching the bridge"
    );
}

/// One pending buffer and one bridge per slot. Two live slots must not see
/// each other's answers, and must not reach each other's bridge.
#[test]
fn the_pending_buffer_and_the_bridge_are_per_slot() {
    let wasm = ping_guest();
    let mut engine = Engine::new();

    let a_calls = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&a_calls);
    let a_bridge: FleetBridgeFn = Arc::new(move |_op: &str, _params: &str| {
        hits.fetch_add(1, Ordering::SeqCst);
        Ok("from-a".to_owned())
    });

    let a = engine
        .spawn(Guest::Wasm(&wasm), Some(a_bridge))
        .expect("slot a loads");
    let b = engine
        .spawn(
            Guest::Wasm(&wasm),
            Some(bridge_answering(Ok("b".to_owned()))),
        )
        .expect("slot b loads");

    let out_b = engine.call(b, "main", &[]).expect("slot b runs");
    assert_eq!(out_b.stdout, "b");
    assert_eq!(
        a_calls.load(Ordering::SeqCst),
        0,
        "slot b must not reach slot a's bridge"
    );

    let out_a = engine.call(a, "main", &[]).expect("slot a runs");
    assert_eq!(
        out_a.stdout, "from-a",
        "slot a keeps its own pending buffer"
    );
    assert_eq!(a_calls.load(Ordering::SeqCst), 1);
}

/// A guest that never talks to the fleet still loads: the door binds only the
/// imports the guest declared.
#[test]
fn a_guest_importing_only_print_still_loads() {
    let wasm = wat::parse_str(
        r#"(module
            (import "agenterm" "print" (func $print (param i32 i32)))
            (memory 1)
            (data (i32.const 0) "hi")
            (func (export "main") (result i32)
                (call $print (i32.const 0) (i32.const 2))
                (i32.const 7)))"#,
    )
    .expect("valid wat");
    let mut engine = Engine::new();
    let outcome = engine
        .run_once(Guest::Wasm(&wasm), None, "main", &[])
        .expect("a partial importer loads and runs");
    assert_eq!(status_of(&outcome.values), 7);
    assert_eq!(outcome.stdout, "hi");
}

/// A guest declaring a door name the ABI does not have is refused at load, in
/// the `Door` class -- not as a mid-run trap on first use.
#[test]
fn an_unknown_door_name_is_refused_at_load() {
    let wasm = wat::parse_str(
        r#"(module
            (import "agenterm" "exec" (func $exec (param i32) (result i32)))
            (memory 1)
            (func (export "main") (result i32) (i32.const 0)))"#,
    )
    .expect("valid wat");
    let mut engine = Engine::new();
    let error = engine
        .spawn(Guest::Wasm(&wasm), None)
        .expect_err("an unknown door name is refused");
    assert!(
        matches!(error, QjswasmError::Door(_)),
        "expected a Door diagnostic, got {error:?}"
    );
}
