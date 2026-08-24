//! The slot mechanism: load once, call many times, reclaim, and never panic on
//! a stale handle.
//!
//! These are the M0 "槽机制" acceptance tests from
//! `plan/design-agenterm-qjswasm.md` §9. Guests are hand-authored `.wat` so the
//! instruction count a test reasons about is visible in the test itself rather
//! than hidden inside a compiler that does not exist yet.

use agenterm_qjswasm::{Budget, Engine, Guest, QjswasmError, Value};

/// Two trivial exports plus one that is deliberately absent, so "call an export
/// that is not there" is tested against a module that *does* export things --
/// otherwise a lookup bug that always fails would still pass the test.
const ARITHMETIC_WAT: &str = r#"
(module
  (func (export "answer") (result i32)
    i32.const 42)
  (func (export "add") (param i32 i32) (result i32)
    local.get 0
    local.get 1
    i32.add))
"#;

/// A counted loop. The step cost is a deterministic function of the argument,
/// which is what makes "did this call get a fresh budget?" measurable rather
/// than a matter of opinion.
const BURN_WAT: &str = r#"
(module
  (func (export "burn") (param $n i32) (result i32)
    (local $acc i32)
    (block $done
      (loop $again
        (br_if $done (i32.eqz (local.get $n)))
        (local.set $n (i32.sub (local.get $n) (i32.const 1)))
        (local.set $acc (i32.add (local.get $acc) (i32.const 1)))
        (br $again)))
    (local.get $acc)))
"#;

fn wasm(wat_source: &str) -> Vec<u8> {
    wat::parse_str(wat_source).expect("test guest must be valid wat")
}

#[test]
fn spawn_then_call_returns_the_export_result() {
    let bytes = wasm(ARITHMETIC_WAT);
    let mut engine = Engine::new();
    let slot = engine
        .spawn(Guest::Wasm(&bytes), None)
        .expect("a valid module must load");
    assert_eq!(engine.live_slots(), 1);

    let out = engine.call(slot, "answer", &[]).expect("answer must run");
    assert_eq!(out.values, vec![Value::I32(42)]);

    let out = engine
        .call(slot, "add", &[Value::I32(40), Value::I32(2)])
        .expect("add must run");
    assert_eq!(out.values, vec![Value::I32(42)]);

    // Cost is reported, not guessed: a call that executed instructions must not
    // report zero, or the budget plumbing is decorative.
    assert!(out.steps > 0, "steps must be observed, got {}", out.steps);
}

#[test]
fn each_call_on_one_slot_gets_a_fresh_step_budget() {
    let bytes = wasm(BURN_WAT);

    // First, measure one call's true cost under a budget generous enough not to
    // interfere.
    let mut probe = Engine::new();
    let slot = probe.spawn(Guest::Wasm(&bytes), None).expect("load");
    let measured = probe
        .call(slot, "burn", &[Value::I32(500)])
        .expect("burn must fit the default budget")
        .steps;
    assert!(measured > 500, "the loop must actually cost steps");

    // Sanity-check that `max_steps` is enforced at all, so the two-call
    // assertion below cannot pass merely because the limit is ignored.
    let mut too_tight = Engine::with_budget(Budget {
        limits: tinyvm::Limits {
            max_steps: measured - 1,
            ..tinyvm::Limits::default()
        },
        ..Budget::default()
    });
    let slot = too_tight.spawn(Guest::Wasm(&bytes), None).expect("load");
    let err = too_tight
        .call(slot, "burn", &[Value::I32(500)])
        .unwrap_err();
    assert!(
        matches!(err, QjswasmError::Budget("max_steps")),
        "a one-step-short budget must be reported as exhaustion of that dial, \
         got {err:?}"
    );

    // Now run the same call twice under a budget that fits exactly one of them.
    // If `max_steps` were cumulative across calls on a persistent instance, the
    // second call would trap here. This is the adversarial form of the
    // assertion; comparing the two `steps` figures alone would pass even on an
    // engine that never reset the counter but reported per-call deltas.
    let budget = Budget {
        limits: tinyvm::Limits {
            max_steps: measured,
            ..tinyvm::Limits::default()
        },
        ..Budget::default()
    };
    let mut engine = Engine::with_budget(budget);
    let slot = engine.spawn(Guest::Wasm(&bytes), None).expect("load");

    let first = engine
        .call(slot, "burn", &[Value::I32(500)])
        .expect("call 1 must fit its own budget");
    let second = engine
        .call(slot, "burn", &[Value::I32(500)])
        .expect("call 2 must receive a fresh budget, not call 1's remainder");

    assert_eq!(first.values, vec![Value::I32(500)]);
    assert_eq!(second.values, vec![Value::I32(500)]);
    assert_eq!(
        first.steps, second.steps,
        "identical work must cost identical steps; a cumulative counter would \
         report {} then roughly double",
        first.steps
    );
}

#[test]
fn calling_a_killed_slot_reports_no_such_slot() {
    let bytes = wasm(ARITHMETIC_WAT);
    let mut engine = Engine::new();
    let slot = engine.spawn(Guest::Wasm(&bytes), None).expect("load");
    engine.call(slot, "answer", &[]).expect("live slot works");

    engine.kill(slot);
    assert_eq!(engine.live_slots(), 0);

    // The point of the test is that this is an ordinary `Err`, not a panic and
    // not a call into a reused slot index.
    let err = engine.call(slot, "answer", &[]).unwrap_err();
    assert!(
        matches!(err, QjswasmError::NoSuchSlot(id) if id == slot),
        "expected NoSuchSlot, got {err:?}"
    );

    // Killing twice is idempotent, not a panic.
    engine.kill(slot);
    assert_eq!(engine.live_slots(), 0);
}

#[test]
fn run_once_leaves_no_live_slot() {
    let bytes = wasm(ARITHMETIC_WAT);
    let mut engine = Engine::new();

    let out = engine
        .run_once(Guest::Wasm(&bytes), None, "answer", &[])
        .expect("run_once must run the entry");
    assert_eq!(out.values, vec![Value::I32(42)]);
    assert_eq!(engine.live_slots(), 0);

    // A failing entry must reclaim the slot too, or a script that traps leaks a
    // slot per invocation.
    let err = engine
        .run_once(Guest::Wasm(&bytes), None, "not_exported", &[])
        .unwrap_err();
    assert!(matches!(err, QjswasmError::Trap(_)), "got {err:?}");
    assert_eq!(engine.live_slots(), 0);
}

#[test]
fn calling_an_absent_export_is_a_typed_error() {
    let bytes = wasm(ARITHMETIC_WAT);
    let mut engine = Engine::new();
    let slot = engine.spawn(Guest::Wasm(&bytes), None).expect("load");

    let err = engine.call(slot, "no_such_export", &[]).unwrap_err();
    assert!(
        matches!(err, QjswasmError::Trap(_)),
        "an absent export must be a typed error, got {err:?}"
    );

    // The slot survives: naming the wrong export is a caller mistake, not a
    // reason to burn the guest.
    let out = engine.call(slot, "answer", &[]).expect("slot still live");
    assert_eq!(out.values, vec![Value::I32(42)]);
}
