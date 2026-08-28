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
    assert_eq!(outcome.stdout, "pong\n");
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
    assert_eq!(outcome.stdout, "no such op\n");
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
    assert_eq!(
        outcome.stdout, "0123456789",
        "the newline is budgeted too, so a full buffer cuts it"
    );
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
    assert_eq!(out_b.stdout, "b\n");
    assert_eq!(
        a_calls.load(Ordering::SeqCst),
        0,
        "slot b must not reach slot a's bridge"
    );

    let out_a = engine.call(a, "main", &[]).expect("slot a runs");
    assert_eq!(
        out_a.stdout, "from-a\n",
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
    assert_eq!(outcome.stdout, "hi\n");
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

/// `validate_wasm` and `spawn` give the *same* answer about an import nobody
/// can bind, and both name it.
///
/// The pair is the point. `validate_wasm` is the `.wasm` half of a `check`, and
/// it used to decode only: a `wasi_snapshot_preview1` guest validated clean and
/// then died at run time on `Trap("call to unbound imported function")`, which
/// names no import and blames a guest that was built correctly against a
/// different host. A gate that passes what the runner cannot run is the worst
/// shape a gate can have, and PRD 36 requires "rejected before it could run"
/// and "trapped while running" to be tellable apart. Asserting only one of the
/// two halves would leave them free to drift apart again.
#[test]
fn check_and_execute_agree_that_an_unbindable_import_is_refused_at_load() {
    let wasm = wat::parse_str(
        r#"(module
            (import "wasi_snapshot_preview1" "fd_write"
                (func $fd_write (param i32 i32 i32 i32) (result i32)))
            (memory 1)
            (func (export "main") (result i32)
              (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 0))))"#,
    )
    .expect("valid wat");

    let checked = agenterm_qjswasm::validate_wasm(&wasm)
        .expect_err("a check must not pass a guest that cannot run");
    let mut engine = Engine::new();
    let spawned = engine
        .spawn(Guest::Wasm(&wasm), None)
        .expect_err("and the runner must refuse it too");

    for error in [&checked, &spawned] {
        assert!(
            matches!(error, QjswasmError::Door(_)),
            "expected a load-time Door refusal, got {error:?}"
        );
        let text = error.to_string();
        assert!(
            text.contains("wasi_snapshot_preview1") && text.contains("fd_write"),
            "the refusal must name the import nobody can bind: {text}"
        );
    }
    assert_eq!(engine.live_slots(), 0);
}

/// **The two `agenterm.*` doors are two different doors**, and this is the
/// measurement that says so.
///
/// PRD 02.36's archive gate 2 for `agenterm-wasmcore` asks whether one `.wasm`
/// guest could be routed to one engine. Its recorded blocker was "the guest is
/// Rust `std` on `wasm32-wasip1`, so it imports WASI -- rewrite it as `no_std`
/// importing only `agenterm.*`". That is **not sufficient**, and this test is
/// why: a guest importing *only* `agenterm.*` is still refused, because
/// `agenterm-wasmcore`'s `fleet_call` takes **six** arguments and this door's
/// takes four.
///
/// The direction of portability is one-way, and the reason is structural
/// rather than a matter of taste. wasmcore's convention has the host write the
/// answer through out-parameters, which requires the host to call back into
/// the guest's `wasmcore_alloc`. tinyvm's typed host callback holds `&mut` on
/// guest memory for its whole duration, so it **cannot** re-enter the guest --
/// see this crate's `src/host.rs` header. So this engine cannot grow the
/// six-argument form; wasmtime, which can do either, could adopt the two-pass
/// one.
///
/// That made gate 2's migration cost "rewrite wasmcore's door", not "rewrite
/// the guest" -- and that is what happened, which is why the portable guest
/// below exists and why the crate could be archived on 2026-08-28.
///
/// The test outlives the engine it was written against. What it pins is not a
/// fact about wasmcore but a fact about **this** door: a six-argument
/// `fleet_call` is refused, and the `&mut` reason above says it always will
/// be. Any future host that wants to share guests with this one has to take
/// the two-pass shape.
#[test]
fn a_wasmcore_shaped_guest_is_refused_for_its_door_signature_not_for_wasi() {
    let mut engine = Engine::new();

    // Imports only `agenterm.*`, exports `memory`, `_start` and the allocator
    // wasmcore's convention needs -- i.e. exactly what "make the guest
    // `no_std` importing only `agenterm.*`" would produce.
    let wasmcore_shaped = wat::parse_str(
        r#"(module
            (import "agenterm" "fleet_call"
                (func $f (param i32 i32 i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "wasmcore_alloc") (param i32) (result i32) i32.const 0)
            (func (export "_start"))
        )"#,
    )
    .expect("valid wat");
    let refusal = engine
        .spawn(Guest::Wasm(&wasmcore_shaped), None)
        .expect_err("the six-argument door is not this door");
    let text = refusal.to_string();
    assert!(
        text.contains("fleet_call") && text.contains("signature"),
        "the refusal must name the door and the signature, not something vague: {text}"
    );
    assert!(
        text.contains("4 i32 parameter"),
        "and it must say what this door does take, so the author can act: {text}"
    );
    // The point of the test: it is **not** a WASI complaint.
    assert!(
        !text.contains("wasi"),
        "a `no_std` guest has no WASI problem left; the door shape is the          blocker, and a diagnostic naming WASI here would send the reader to          fix the wrong thing: {text}"
    );

    // The same guest with this door's four-argument shape loads.
    let ours = wat::parse_str(
        r#"(module
            (import "agenterm" "fleet_call"
                (func $f (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "_start"))
        )"#,
    )
    .expect("valid wat");
    engine
        .spawn(Guest::Wasm(&ours), None)
        .expect("this door's own shape loads");
}

/// The other engine's **portable** door imports load here unchanged.
///
/// The companion to
/// [`a_wasmcore_shaped_guest_is_refused_for_its_door_signature_not_for_wasi`],
/// which is what this looked like before. `agenterm-wasmcore` grew a two-pass
/// door on 2026-08-28 and gave it the portable name: its
/// `agenterm.fleet_call` is now the same four-argument first pass as this
/// one's, with the same `fleet_result_len` and `fleet_result` behind it. Its
/// original one-call convention kept its behaviour and took the name
/// `fleet_call_into`, which says what it does -- the host writes *into* the
/// guest.
///
/// The import block below is copied from
/// `agenterm-wasmcore/tests/portable_door.rs` and is not adapted in any way.
/// That is the whole assertion: **one import block, two engines.**
///
/// What still keeps one guest from running on both is no longer the door. It
/// is how a guest *reports*: this engine calls a named export and takes its
/// returned value, while wasmcore calls `_start` and reads none, so those
/// guests reach for WASI's `proc_exit` -- which this engine refuses. That is
/// the last item on PRD 02.36's gate 2, and it is a smaller one than the door
/// was.
#[test]
fn the_other_engines_portable_door_imports_load_here_unchanged() {
    let portable = wat::parse_str(
        r#"(module
            (import "agenterm" "fleet_call"
                (func $begin (param i32 i32 i32 i32) (result i32)))
            (import "agenterm" "fleet_result_len" (func $len (result i32)))
            (import "agenterm" "fleet_result" (func $get (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "main") (result i32) (call $len))
        )"#,
    )
    .expect("valid wat");
    let mut engine = Engine::new();
    engine
        .spawn(Guest::Wasm(&portable), None)
        .expect("the portable door is this door");
}

/// **A guest written to nobody's engine in particular runs here.**
///
/// These bytes were built to answer PRD 02.36's archive gate 2: could one
/// guest run unchanged on this engine *and* on `agenterm-wasmcore`? They did,
/// which is part of why that crate could be archived on 2026-08-28 -- the door
/// it defined was not lost with it. Getting there took three corrections.
///
/// The blocker was first thought to be the entry name (`_start`), then the
/// guest's `std`-ness and its WASI imports, then the two doors' differing
/// `fleet_call` arities. Each was measured and each was wrong or incomplete
/// until the last, and closing that left one more: **how a guest reports.**
/// `agenterm-wasmcore` called `_start` and read no returned value, so its
/// guests used WASI's `proc_exit`, which this engine refuses; it grew
/// `run_export` so a guest can return a number from a named function instead.
///
/// The source below is `agenterm-wasmcore/tests/portable_door.rs`'s
/// `PORTABLE_GUEST_WAT`, character for character as of the archival. A second
/// test used to read that file and assert the two copies had not drifted;
/// there is no second copy now, so the guarantee this test carries is weaker
/// and worth naming: it pins that the door still accepts a guest written to
/// the *portable* convention rather than to this engine's conveniences. If a
/// third engine ever appears, that drift check comes back with it.
const PORTABLE_GUEST_WAT: &str = r#"(module
    (import "agenterm" "fleet_call"
        (func $begin (param i32 i32 i32 i32) (result i32)))
    (import "agenterm" "fleet_result_len" (func $len (result i32)))
    (import "agenterm" "fleet_result" (func $get (param i32 i32) (result i32)))
    (memory (export "memory") 1)
    (data (i32.const 0) "demo.echo")
    (data (i32.const 16) "{}")
    (func (export "main") (result i32)
        (local $n i32)
        (if (i32.ne
                (call $begin (i32.const 0) (i32.const 9) (i32.const 16) (i32.const 2))
                (i32.const 0))
            (then (return (i32.const -1))))
        (local.set $n (call $len))
        (if (i32.ne (call $get (i32.const 256) (i32.const 512)) (local.get $n))
            (then (return (i32.const -2))))
        (if (i32.ne (i32.load8_u (i32.const 256)) (i32.const 123))
            (then (return (i32.const -3))))
        (local.get $n)
    )
)"#;

#[test]
fn a_guest_written_to_the_portable_convention_runs_here() {
    let bytes = wat::parse_str(PORTABLE_GUEST_WAT).expect("valid wat");
    let bridge: FleetBridgeFn = Arc::new(|op: &str, params: &str| {
        assert_eq!(op, "demo.echo");
        assert_eq!(params, "{}");
        Ok("{\"ok\":true}".to_owned())
    });
    let mut engine = Engine::new();
    let out = engine
        .run_once(Guest::Wasm(&bytes), Some(bridge), "main", &[])
        .expect("the portable guest runs");
    assert_eq!(
        out.values,
        vec![Value::I32(11)],
        "the portable guest returns the length of the bridge's reply; a \
         negative names which of its own checks failed"
    );
}
