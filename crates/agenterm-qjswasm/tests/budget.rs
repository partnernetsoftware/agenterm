//! Budgets, proved by guests that actively try to exceed them.
//!
//! A limit nobody attacks is a comment. Each test here runs a guest whose only
//! purpose is to overrun one specific `tinyvm::Limits` field, and then asserts
//! three things about the result, because any one of them alone is weak
//! evidence:
//!
//! 1. **the attack was stopped** -- the call did not succeed;
//! 2. **it was stopped by the named limit** -- the error is the budget class,
//!    which the face distinguishes from [`QjswasmError::Trap`] precisely so
//!    "too expensive" and "broken" are not the same report; and
//! 3. **the host outlived it** -- a subsequent, unrelated call still succeeds.
//!
//! Point 3 is the one that is easy to skip and impossible to fake: if a hostile
//! guest could abort the process, the test binary would die and there would be
//! no failure report at all, so every "host survives" assertion below is also a
//! liveness check on the whole harness.
//!
//! The adversarial guests live in `fixtures.rs`; the "why is this guest shaped
//! like this" reasoning lives there next to each one.

mod fixtures;

use agenterm_qjswasm::{Budget, Engine, Guest, QjswasmError, Value};

/// Build a budget from tinyvm's defaults with one field overridden, so each
/// test changes exactly the dial it is about and inherits sane values for the
/// other four. Tests that tuned every field by hand would make it impossible to
/// tell which limit actually stopped a guest.
fn budget_with(edit: impl FnOnce(&mut tinyvm::Limits)) -> Budget {
    let mut limits = tinyvm::Limits::default();
    edit(&mut limits);
    Budget {
        limits,
        ..Budget::default()
    }
}

/// An infinite loop is stopped by `max_steps`, and the engine is still usable
/// afterwards.
///
/// `max_steps` is pulled far below tinyvm's 16M default purely so the test is
/// quick: the guest never terminates, so the wait is exactly the budget.
///
/// The witness call is a separate `run_once` on a benign guest *after* the
/// attack, on the same engine. It is not decoration -- "the guest was stopped"
/// and "the host is intact" are different claims, and a host that stopped the
/// guest by poisoning itself would satisfy only the first.
#[test]
fn an_infinite_loop_is_stopped_by_max_steps_and_the_host_survives() {
    let mut eng = Engine::with_budget(budget_with(|l| l.max_steps = 50_000));
    let spinner = fixtures::infinite_loop();

    let err = eng
        .run_once(Guest::Wasm(&spinner), None, "spin", &[])
        .expect_err("an infinite loop must not return");
    assert!(
        matches!(err, QjswasmError::Budget(_)),
        "a step overrun must report the budget class, not a generic trap: {err:?}"
    );

    // run_once reclaims even when the call failed; a leaked slot after a
    // hostile guest is a slow resource leak dressed up as a clean failure.
    assert_eq!(eng.live_slots(), 0);

    let good = fixtures::benign_constant();
    let out = eng
        .run_once(Guest::Wasm(&good), None, "answer", &[])
        .expect("the engine is still usable after a budget kill");
    assert_eq!(out.values, vec![Value::I32(fixtures::BENIGN_ANSWER)]);
    assert!(out.steps > 0, "a real call must report a nonzero cost");
}

/// Unbounded recursion is stopped by `max_call_depth`, and 20,000 pending
/// guest activations do not touch the native stack.
///
/// The depth is set an order of magnitude *above* tinyvm's 512 default on
/// purpose. A 20,000-frame native recursion would overflow a default thread
/// stack and abort the process -- there would be no assertion failure to read,
/// just a dead test binary. So this test's real subject is the claim in
/// tinyvm's own `WASM_MAX_DEPTH` docs, that activations live in a fallibly
/// grown VM vector: passing means the frames were heap, not stack.
///
/// `max_steps` is raised to cover ~5 instructions per level with room to spare,
/// so the guest genuinely reaches the depth ceiling instead of running out of
/// fuel on the way down and reporting the wrong limit.
#[test]
fn unbounded_recursion_is_stopped_by_max_call_depth_not_by_the_native_stack() {
    let mut eng = Engine::with_budget(budget_with(|l| {
        l.max_call_depth = 20_000;
        l.max_steps = 4_000_000;
    }));
    let deep = fixtures::unbounded_recursion();

    let a = eng.spawn(Guest::Wasm(&deep), None).expect("spawn");
    let err = eng
        .call(a, "recurse", &[])
        .expect_err("unbounded recursion must not return");
    assert!(
        matches!(err, QjswasmError::Budget(_)),
        "a depth overrun must report the budget class, not a generic trap: {err:?}"
    );

    // Reaching this line at all is the "no process abort" evidence.
    let good = fixtures::benign_constant();
    let out = eng
        .run_once(Guest::Wasm(&good), None, "answer", &[])
        .expect("the engine is still usable after a depth kill");
    assert_eq!(out.values, vec![Value::I32(fixtures::BENIGN_ANSWER)]);
}

/// `memory.grow` past `max_memory_pages` is refused, growth within it is not.
///
/// The refusal surfaces as the guest receiving `-1`, not as a trap or a host
/// error, because that is what the WebAssembly specification requires of a
/// failed `memory.grow`; tinyvm returns `-1` from the same instruction whether
/// the spec ceiling or [`Budget::limits`] did the refusing. Turning that into
/// an engine-level error would make this crate's guests non-conforming, so the
/// test asserts the *specified* observable and then proves the memory really
/// did not move by reading `memory.size` back.
///
/// The second half walks the memory up to the ceiling one page at a time and
/// shows the very next page is refused. Without it, "grow was refused" is also
/// consistent with `memory.grow` being broken for every input; with it, the
/// page budget is demonstrably the thing holding the line, at exactly the value
/// configured.
#[test]
fn memory_grow_beyond_the_page_budget_is_refused() {
    const PAGE_CEILING: usize = 4;
    let mut eng = Engine::with_budget(budget_with(|l| l.max_memory_pages = PAGE_CEILING));
    let bomb = fixtures::memory_grow_bomb();

    let a = eng.spawn(Guest::Wasm(&bomb), None).expect("spawn");

    let out = eng
        .call(a, "bomb", &[])
        .expect("a refused grow is not a trap");
    assert_eq!(
        out.values,
        vec![Value::I32(-1)],
        "a 4096-page request under a {PAGE_CEILING}-page budget must be refused"
    );
    let out = eng.call(a, "pages", &[]).expect("read memory.size");
    assert_eq!(
        out.values,
        vec![Value::I32(1)],
        "the refused grow still moved the memory"
    );

    // The module starts at one page, so exactly PAGE_CEILING - 1 single-page
    // grows must succeed, each returning the previous size.
    for expected_old in 1..PAGE_CEILING as i32 {
        let out = eng.call(a, "grow_one", &[]).expect("grow within budget");
        assert_eq!(
            out.values,
            vec![Value::I32(expected_old)],
            "growing from {expected_old} pages should be inside a \
             {PAGE_CEILING}-page budget"
        );
    }

    let out = eng
        .call(a, "grow_one", &[])
        .expect("a refused grow is not a trap");
    assert_eq!(
        out.values,
        vec![Value::I32(-1)],
        "the page after the ceiling must be refused"
    );
    let out = eng.call(a, "pages", &[]).expect("read memory.size");
    assert_eq!(out.values, vec![Value::I32(PAGE_CEILING as i32)]);
}

/// A module whose *declared* minimum memory exceeds the page budget is refused
/// before it runs.
///
/// This is the same ceiling as the test above, enforced at the other end of the
/// pipeline, and it is the more important of the two: rejecting an oversized
/// declaration at load time means the host never performs the allocation, so a
/// guest cannot extract a large `malloc` from the process merely by being
/// loaded and then killed. The error must therefore be [`QjswasmError::Load`]
/// -- a *pre-execution* refusal -- and not a trap from a guest that already
/// started.
#[test]
fn a_declared_memory_above_the_page_budget_is_refused_at_load() {
    // The fixture declares `(memory 1)`; a zero-page budget cannot host it.
    let mut eng = Engine::with_budget(budget_with(|l| l.max_memory_pages = 0));
    let bomb = fixtures::memory_grow_bomb();

    let err = eng
        .spawn(Guest::Wasm(&bomb), None)
        .expect_err("a one-page module cannot load under a zero-page budget");
    assert!(
        matches!(err, QjswasmError::Load(_)),
        "an over-budget declaration must be refused at load, not at run: {err:?}"
    );
    assert_eq!(
        eng.live_slots(),
        0,
        "a rejected load must not occupy a slot"
    );
}

/// The same guest, the same entry, the same arguments: refused under a stingy
/// step budget and successful under a generous one.
///
/// This is the test that makes `Budget` a control rather than a decoration.
/// Both halves run identical bytes through identical calls; the *only*
/// difference between them is `max_steps`. If the dial were ignored, either
/// both halves would pass or both would fail, and no assertion about a single
/// budget value could tell the difference.
///
/// The step assertions are inequalities on purpose. The exact instruction count
/// of the loop is an artefact of how `wat` lowers it, so pinning a number here
/// would make an unrelated toolchain update look like an engine regression.
/// What the engine actually owes us is: at least one step per iteration, and
/// never more than the ceiling it advertised.
#[test]
fn the_step_budget_is_a_real_dial_not_a_decoration() {
    const ITERATIONS: i32 = 2_000;
    const STINGY: u64 = 500;
    const GENEROUS: u64 = 1_000_000;

    let burner = fixtures::bounded_burner();

    let mut stingy = Engine::with_budget(budget_with(|l| l.max_steps = STINGY));
    let err = stingy
        .run_once(
            Guest::Wasm(&burner),
            None,
            "burn",
            &[Value::I32(ITERATIONS)],
        )
        .expect_err("a 2000-iteration loop cannot fit in 500 steps");
    assert!(
        matches!(err, QjswasmError::Budget(_)),
        "a step overrun must report the budget class, not a generic trap: {err:?}"
    );

    let mut generous = Engine::with_budget(budget_with(|l| l.max_steps = GENEROUS));
    let out = generous
        .run_once(
            Guest::Wasm(&burner),
            None,
            "burn",
            &[Value::I32(ITERATIONS)],
        )
        .expect("the very same guest fits in a generous budget");
    assert_eq!(out.values, vec![Value::I32(ITERATIONS)]);
    assert!(
        out.steps > ITERATIONS as u64,
        "a {ITERATIONS}-iteration loop reported only {} steps",
        out.steps
    );
    assert!(
        out.steps <= GENEROUS,
        "reported {} steps against a ceiling of {GENEROUS}",
        out.steps
    );
    assert!(
        out.steps > STINGY,
        "the stingy budget of {STINGY} was never actually the binding \
         constraint: the work only costs {} steps",
        out.steps
    );
}
