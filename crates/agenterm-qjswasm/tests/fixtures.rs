//! Adversarial guest programs, shared by `tests/isolation.rs` and
//! `tests/budget.rs` via `mod fixtures;`.
//!
//! Every guest here is authored as `.wat` text and compiled to real wasm bytes
//! by the `wat` dev-dependency, so what the tests feed the engine is the same
//! standard `\0asm` module a hostile author would ship -- not a hand-picked
//! byte pattern that happens to trip one code path. tinyvm's own suite uses the
//! same crate the same way; `wat` never reaches the product, whose encoder
//! lives in `src/lower/`.
//!
//! The shapes below are deliberate, and the "why" is not obvious from the
//! source, so each guest documents which limit it is aimed at and which
//! *neighbouring* limit it is carefully shaped to avoid. An adversarial fixture
//! that trips the wrong guard is worse than no fixture: it makes a green test
//! that proves nothing about the limit named in the test's own title.
#![allow(dead_code)]

/// Compile `.wat` text to wasm bytes, or fail loudly.
///
/// A malformed fixture is a bug in this file, never a finding about the engine
/// under test, so it panics here rather than travelling into a test as bytes
/// the loader will reject for the wrong reason.
fn wasm(wat_source: &str) -> Vec<u8> {
    wat::parse_str(wat_source).expect("fixture .wat must compile; this is a bug in fixtures.rs")
}

/// Burns instruction steps forever. Aimed at `Limits::max_steps`.
///
/// An empty `loop` + `br` back to its own label is the smallest program with no
/// exit: there is no branch out, no memory touched, no call made, so the *only*
/// thing that can stop it is the step counter. That exclusivity is the point --
/// if this guest ever terminates by any other means, the engine's step budget
/// is not what stopped it.
pub fn infinite_loop() -> Vec<u8> {
    wasm(
        r#"
        (module
          (func (export "spin")
            (loop $forever
              (br $forever))))
        "#,
    )
}

/// Recurses with no base case. Aimed at `Limits::max_call_depth`.
///
/// Two deliberate shape choices:
///
/// 1. The recursive call is **not** in tail position -- its result is consumed
///    by an `i32.add` afterwards. A tail call could in principle be turned into
///    a jump, which would burn steps at constant depth and quietly test
///    `max_steps` under a test named for `max_call_depth`. Keeping a live
///    consumer after the call forces a real, retained activation per level.
/// 2. Each frame holds one parameter and a two-value operand stack, so the
///    per-frame cost in *activation slots* is tiny. `max_activation_slots`
///    defaults to `1 << 20` against a `max_call_depth` of 512, so with a frame
///    this small the depth guard is reached first and the neighbouring
///    "call stack" guard stays out of the way.
///
/// The recursion is unbounded on purpose: a base case would let the guest
/// decide when to stop, and the whole claim under test is that the *host*
/// decides.
pub fn unbounded_recursion() -> Vec<u8> {
    wasm(
        r#"
        (module
          (func $descend (param i32) (result i32)
            (i32.add
              (call $descend (i32.add (local.get 0) (i32.const 1)))
              (i32.const 1)))
          (func (export "recurse") (result i32)
            (call $descend (i32.const 0))))
        "#,
    )
}

/// Reads one word past the end of its own linear memory. Aimed at producing a
/// genuine execution trap (not a budget stop, not a load-time rejection).
///
/// The memory is declared `1 1` -- minimum *and* maximum one page -- so the
/// guest can never grow its way into legality and address `65536` is
/// permanently four bytes off the end of a 64 KiB page. The access is a plain
/// `i32.load` of a constant address: no arithmetic to get wrong, no dependence
/// on a budget, so the failure class is unambiguous and reproducible on every
/// run.
pub fn out_of_bounds_load() -> Vec<u8> {
    wasm(
        r#"
        (module
          (memory 1 1)
          (func (export "peek") (result i32)
            (i32.load (i32.const 65536))))
        "#,
    )
}

/// Asks for 4096 more pages (256 MiB) than it owns. Aimed at
/// `Limits::max_memory_pages`.
///
/// 4096 pages is far above the tinyvm default of 256 and astronomically above
/// the handful of pages the budget test grants, so the request cannot
/// accidentally succeed. It is still comfortably *below* the wasm 32-bit
/// maximum of 65536 pages, which matters: the module declares no maximum of its
/// own, so if the request also broke the spec ceiling we could not tell whether
/// the host budget refused it or the format did.
///
/// Three exports, because "refused" is a claim about three observable facts:
/// `bomb` returns the refusal, `pages` shows the memory did not move, and
/// `grow_one` shows a modest growth still works -- i.e. the budget refused this
/// request rather than `memory.grow` being broken outright.
pub fn memory_grow_bomb() -> Vec<u8> {
    wasm(
        r#"
        (module
          (memory 1)
          (func (export "bomb") (result i32)
            (memory.grow (i32.const 4096)))
          (func (export "grow_one") (result i32)
            (memory.grow (i32.const 1)))
          (func (export "pages") (result i32)
            (memory.size)))
        "#,
    )
}

/// The healthy control: returns a constant, touches nothing.
///
/// Every "the host is still alive" claim needs a witness that can only pass if
/// the engine still works, and can fail for no other reason. This guest has no
/// memory, no imports, no loop and no call, so a failure here is never about
/// the witness.
pub fn benign_constant() -> Vec<u8> {
    wasm(
        r#"
        (module
          (func (export "answer") (result i32)
            (i32.const 42)))
        "#,
    )
}

/// The i32 constant [`benign_constant`] returns.
pub const BENIGN_ANSWER: i32 = 42;

/// Writes and reads one i32 at address 0 of its own linear memory.
///
/// Both isolation slots load these *identical bytes*, which is what makes the
/// test meaningful: same module, same address, same instructions -- so if slot
/// B reads back the value slot A stored, the only possible explanation is a
/// shared linear memory. A guest that wrote to per-slot addresses would prove
/// nothing.
pub fn memory_scribbler() -> Vec<u8> {
    wasm(
        r#"
        (module
          (memory 1)
          (func (export "store") (param i32)
            (i32.store (i32.const 0) (local.get 0)))
          (func (export "load") (result i32)
            (i32.load (i32.const 0))))
        "#,
    )
}

/// Spins a *bounded* loop `n` times and returns `n`. Aimed at making a step
/// budget measurable rather than merely fatal.
///
/// The other guests here prove a budget can stop a program. This one proves the
/// budget is a real dial: pick `n` so the loop costs more than a stingy
/// `max_steps` and less than a generous one, and the same bytes must fail under
/// the first and succeed under the second. It is also the witness for budget
/// *freshness* -- a call that costs a large fraction of `max_steps` can only
/// succeed if this call received a full budget of its own.
///
/// Cost is deliberately linear and modest (a fixed handful of instructions per
/// iteration), so tests can bound it with inequalities. They must not assert an
/// exact step count: the per-iteration instruction count is an artefact of how
/// `wat` lowers this loop, not a contract of the engine.
pub fn bounded_burner() -> Vec<u8> {
    wasm(
        r#"
        (module
          (func (export "burn") (param $n i32) (result i32)
            (local $i i32)
            (block $done
              (loop $again
                (br_if $done (i32.ge_s (local.get $i) (local.get $n)))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $again)))
            (local.get $i)))
        "#,
    )
}

/// Calls `agenterm.fleet_call` once and returns the door's status code.
///
/// Used to prove a bridge is bound *per slot*: two slots load these same bytes
/// with two different closures, and ringing one must not move the other's
/// counter. Only `fleet_call` is imported -- the guest never fetches the result
/// -- because the question here is "whose closure ran?", and importing the
/// whole door would drag the two-trip copy protocol into an isolation test that
/// has nothing to say about it.
///
/// The operation name and params are placed in data segments at fixed
/// addresses, so the pointers handed to the door are in-bounds and boring; a
/// hostile pointer is `tests/host_door.rs`'s subject, not this file's.
pub fn bridge_ringer() -> Vec<u8> {
    wasm(
        r#"
        (module
          (import "agenterm" "fleet_call"
            (func $fleet_call (param i32 i32 i32 i32) (result i32)))
          (memory 1)
          (data (i32.const 0) "ping")
          (data (i32.const 16) "{}")
          (func (export "ring") (result i32)
            (call $fleet_call
              (i32.const 0) (i32.const 4)
              (i32.const 16) (i32.const 2))))
        "#,
    )
}

/// The operation name [`bridge_ringer`] passes to the bridge.
pub const RINGER_OP: &str = "ping";
/// The params JSON [`bridge_ringer`] passes to the bridge.
pub const RINGER_PARAMS: &str = "{}";
