//! Slot isolation, proved against hostile guests rather than polite ones.
//!
//! The crate's central safety claim is "a bad slot can only hurt itself". That
//! is four separate claims about four separate resources, and each one gets its
//! own test here:
//!
//! | resource        | the claim                                             |
//! |-----------------|-------------------------------------------------------|
//! | linear memory   | one slot's writes are invisible to another             |
//! | control flow    | one slot's trap leaves another callable                |
//! | step budget     | one slot's exhaustion does not drain another's budget  |
//! | the host door   | a bridge bound to one slot is unreachable from another |
//!
//! Every test spawns *the same bytes* into both slots wherever it can, because
//! two different programs failing differently is not evidence of isolation --
//! identical programs behaving independently is.
//!
//! The adversarial guests live in `fixtures.rs`; the "why is this guest shaped
//! like this" reasoning lives there next to each one.

mod fixtures;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agenterm_qjswasm::{Budget, Engine, Guest, QjswasmError, Value};

/// A budget with a step ceiling low enough to keep the adversarial tests fast,
/// but high enough that [`fixtures::bounded_burner`] can spend a large,
/// *measurable* fraction of it and still finish. That gap is what makes budget
/// freshness observable in `Outcome::steps` instead of on a wall clock.
const STEP_CEILING: u64 = 200_000;

fn engine() -> Engine {
    Engine::with_budget(Budget {
        limits: tinyvm::Limits {
            max_steps: STEP_CEILING,
            ..tinyvm::Limits::default()
        },
        ..Budget::default()
    })
}

/// Two slots, the same scribbler bytes, the same address 0 -- and each reads
/// back only what it wrote.
///
/// The interleaving matters: A writes, B writes, *then* both read. If the two
/// instances shared one linear memory, B's store would have overwritten A's and
/// A's read-back would return B's value. Reading each slot immediately after
/// its own write would pass even on a shared memory, which is why the reads are
/// deliberately deferred until after both writes.
#[test]
fn two_slots_cannot_see_each_others_linear_memory() {
    let bytes = fixtures::memory_scribbler();
    let mut eng = engine();

    let a = eng.spawn(Guest::Wasm(&bytes), None).expect("spawn a");
    let b = eng.spawn(Guest::Wasm(&bytes), None).expect("spawn b");

    eng.call(a, "store", &[Value::I32(0x0A0A_0A0A)])
        .expect("a stores");
    eng.call(b, "store", &[Value::I32(0x0B0B_0B0B)])
        .expect("b stores");

    let a_read = eng.call(a, "load", &[]).expect("a loads");
    let b_read = eng.call(b, "load", &[]).expect("b loads");

    assert_eq!(
        a_read.values,
        vec![Value::I32(0x0A0A_0A0A)],
        "slot A read back a value it never wrote -- linear memory is shared"
    );
    assert_eq!(
        b_read.values,
        vec![Value::I32(0x0B0B_0B0B)],
        "slot B read back a value it never wrote -- linear memory is shared"
    );
    assert_eq!(eng.live_slots(), 2);
}

/// Slot A traps out of bounds; slot B is still callable afterwards.
///
/// The trap must be reported as [`QjswasmError::Trap`] specifically: a trap
/// that arrived as a budget error would mean the engine cannot tell "this guest
/// is broken" from "this guest is expensive", which are different operational
/// problems with different responses.
///
/// Slot B is spawned *before* A traps, so the test also covers the case that
/// matters in production -- an already-live neighbour -- rather than only
/// proving the engine can create a fresh slot after a failure.
#[test]
fn a_trap_in_one_slot_leaves_its_neighbour_callable() {
    let mut eng = engine();
    let bad = fixtures::out_of_bounds_load();
    let good = fixtures::benign_constant();

    let a = eng.spawn(Guest::Wasm(&bad), None).expect("spawn a");
    let b = eng.spawn(Guest::Wasm(&good), None).expect("spawn b");

    let err = eng.call(a, "peek", &[]).expect_err("slot A must trap");
    assert!(
        matches!(err, QjswasmError::Trap(_)),
        "out-of-bounds load should be a Trap, got {err:?}"
    );

    let out = eng
        .call(b, "answer", &[])
        .expect("slot B survives A's trap");
    assert_eq!(out.values, vec![Value::I32(fixtures::BENIGN_ANSWER)]);

    // A trap kills the call, not the slot: A is still a live slot, and calling
    // it again must reach the guest and trap again rather than report
    // NoSuchSlot. Silently retiring a trapped slot would turn one guest bug
    // into a confusing second failure class for its owner.
    assert_eq!(eng.live_slots(), 2);
    let again = eng.call(a, "peek", &[]).expect_err("slot A traps again");
    assert!(matches!(again, QjswasmError::Trap(_)), "got {again:?}");
}

/// Slot A burns its entire step budget to exhaustion; slot B's *next* call
/// still receives a full, fresh one.
///
/// The assertion is on `Outcome::steps`, never on elapsed time. The witness is
/// a bounded burner sized to cost well over half of `STEP_CEILING`: if the two
/// slots shared a step counter -- or if the counter simply were not reset --
/// slot B could not spend that much and return. Slot B is then called a second
/// time and must spend the same amount again, which distinguishes "budgets are
/// per slot" from "budgets are per call", the two claims this crate makes
/// together.
#[test]
fn an_exhausted_budget_in_one_slot_does_not_drain_its_neighbour() {
    let mut eng = engine();
    let spinner = fixtures::infinite_loop();
    let burner = fixtures::bounded_burner();

    // Sized against STEP_CEILING, not against a measured instruction count:
    // the exact per-iteration cost is `wat`'s business, so the test only needs
    // "expensive enough that a stale counter would refuse it, cheap enough that
    // a fresh counter allows it".
    const ITERATIONS: i32 = 10_000;
    let floor = STEP_CEILING / 4;

    let a = eng.spawn(Guest::Wasm(&spinner), None).expect("spawn a");
    let b = eng.spawn(Guest::Wasm(&burner), None).expect("spawn b");

    let err = eng
        .call(a, "spin", &[])
        .expect_err("the infinite loop must be stopped by the step budget");
    assert!(
        matches!(err, QjswasmError::Budget(_)),
        "an infinite loop should exhaust the step budget, got {err:?}"
    );

    let first = eng
        .call(b, "burn", &[Value::I32(ITERATIONS)])
        .expect("slot B still has a budget of its own");
    assert_eq!(first.values, vec![Value::I32(ITERATIONS)]);
    assert!(
        first.steps > floor,
        "slot B's call cost only {} steps; it is too cheap to prove it got a \
         fresh budget (needs > {floor})",
        first.steps
    );
    assert!(
        first.steps <= STEP_CEILING,
        "slot B reported {} steps, above its own ceiling of {STEP_CEILING}",
        first.steps
    );

    let second = eng
        .call(b, "burn", &[Value::I32(ITERATIONS)])
        .expect("every top-level call gets a fresh budget");
    assert_eq!(
        second.steps, first.steps,
        "the same work cost a different number of steps on the second call, so \
         the step counter is carrying state across calls"
    );
}

/// A bridge closure handed to slot A is never invoked by slot B.
///
/// Both slots load identical bytes that ring `agenterm.fleet_call` exactly
/// once, and each gets its own counting closure. Counting is the whole method:
/// a test that only checked slot B's *return value* would pass even if both
/// closures ran, because both return `Ok`. The counters make the question
/// "whose closure executed?" directly observable.
///
/// Slot B is rung first on purpose. If binding were global -- last writer wins,
/// say -- then ringing B after A was installed is exactly the call that would
/// land on A's closure.
#[test]
fn a_bridge_bound_to_one_slot_is_unreachable_from_another() {
    let mut eng = engine();
    let bytes = fixtures::bridge_ringer();

    let a_calls = Arc::new(AtomicUsize::new(0));
    let b_calls = Arc::new(AtomicUsize::new(0));

    let a_bridge = {
        let hits = Arc::clone(&a_calls);
        Arc::new(move |op: &str, params: &str| -> Result<String, String> {
            assert_eq!(op, fixtures::RINGER_OP);
            assert_eq!(params, fixtures::RINGER_PARAMS);
            hits.fetch_add(1, Ordering::SeqCst);
            Ok(r#"{"from":"a"}"#.to_owned())
        })
    };
    let b_bridge = {
        let hits = Arc::clone(&b_calls);
        Arc::new(move |op: &str, params: &str| -> Result<String, String> {
            assert_eq!(op, fixtures::RINGER_OP);
            assert_eq!(params, fixtures::RINGER_PARAMS);
            hits.fetch_add(1, Ordering::SeqCst);
            Ok(r#"{"from":"b"}"#.to_owned())
        })
    };

    let a = eng
        .spawn(Guest::Wasm(&bytes), Some(a_bridge))
        .expect("spawn a");
    let b = eng
        .spawn(Guest::Wasm(&bytes), Some(b_bridge))
        .expect("spawn b");

    let out = eng
        .call(b, "ring", &[])
        .expect("slot B rings its own bridge");
    // 0 = Ok, per the four-item door ABI in plan/goal-agenterm-qjswasm.md.
    assert_eq!(out.values, vec![Value::I32(0)]);
    assert_eq!(
        a_calls.load(Ordering::SeqCst),
        0,
        "slot B's call reached slot A's bridge"
    );
    assert_eq!(b_calls.load(Ordering::SeqCst), 1);

    let out = eng
        .call(a, "ring", &[])
        .expect("slot A rings its own bridge");
    assert_eq!(out.values, vec![Value::I32(0)]);
    assert_eq!(a_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        b_calls.load(Ordering::SeqCst),
        1,
        "slot A's call reached slot B's bridge"
    );
}
