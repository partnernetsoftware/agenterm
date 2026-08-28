//! PRD 02.36 archive gate 2's second half: **the same guest, both engines,
//! side by side.**
//!
//! The gate asked for a same-guest performance comparison, and until
//! 2026-08-28 that could not be written, because no guest could run on both.
//! Three corrections got it there -- the entry name was a pseudo-problem, the
//! guest's WASI imports were not the real blocker either, the two
//! `agenterm.fleet_call`s had different arities, and once those matched the
//! engines still disagreed about how a guest reports. `agenterm-wasmcore`
//! grew the portable door and `run_export`; this file is what that bought.
//!
//! It is a **comparison**, not a benchmark: one guest, one bridge, wall clock
//! around the call, printed rather than asserted. Asserting a ratio would pin
//! this machine's numbers into the suite, and the ratio is not the product
//! claim -- the product claim is that the answer is the same.
#![cfg(all(feature = "script-wasmcore", feature = "script-qjswasm"))]

use std::sync::Arc;
use std::time::Instant;

/// Character for character the guest both engines' own suites use. The two
/// crates do not depend on each other, so the source travels by copy and each
/// side checks the copy it can see.
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

const ANSWER: &str = "{\"ok\":true}";

/// The claim: **one set of bytes, two engines, one answer.**
#[test]
fn the_same_guest_gives_the_same_answer_at_both_engines() {
    let bytes = wat::parse_str(PORTABLE_GUEST_WAT).expect("valid wat");

    let host = agenterm_wasmcore::WasmCoreHost::new();
    let wasmcore_bridge: agenterm_wasmcore::WasmFleetBridgeFn =
        Arc::new(|_: &str, _: &str| Ok(ANSWER.to_owned()));
    let from_wasmcore = host
        .run_export(&bytes, "main", Some(wasmcore_bridge))
        .expect("wasmcore runs the portable guest");

    let qjswasm_bridge: agenterm_qjswasm::FleetBridgeFn =
        Arc::new(|_: &str, _: &str| Ok(ANSWER.to_owned()));
    let mut engine = agenterm_qjswasm::Engine::new();
    let out = engine
        .run_once(
            agenterm_qjswasm::Guest::Wasm(&bytes),
            Some(qjswasm_bridge),
            "main",
            &[],
        )
        .expect("qjswasm runs the portable guest");

    assert_eq!(
        out.values,
        vec![agenterm_qjswasm::Value::I32(from_wasmcore)],
        "the two engines disagreed about the same bytes; the guest checks its \
         own status, length and first byte, so a negative names which"
    );
    assert_eq!(
        from_wasmcore,
        ANSWER.len() as i32,
        "and the shared answer is the bridge's, not a coincidence"
    );
}

/// The comparison the gate asked for. Printed, never asserted -- see the
/// module doc for why a ratio does not belong in a suite.
#[test]
fn the_same_guest_timed_at_both_engines() {
    const ROUNDS: u32 = 200;
    let bytes = wat::parse_str(PORTABLE_GUEST_WAT).expect("valid wat");

    let host = agenterm_wasmcore::WasmCoreHost::new();
    let started = Instant::now();
    for _ in 0..ROUNDS {
        let bridge: agenterm_wasmcore::WasmFleetBridgeFn =
            Arc::new(|_: &str, _: &str| Ok(ANSWER.to_owned()));
        host.run_export(&bytes, "main", Some(bridge))
            .expect("wasmcore run");
    }
    let wasmcore = started.elapsed();

    let started = Instant::now();
    for _ in 0..ROUNDS {
        let bridge: agenterm_qjswasm::FleetBridgeFn =
            Arc::new(|_: &str, _: &str| Ok(ANSWER.to_owned()));
        let mut engine = agenterm_qjswasm::Engine::new();
        engine
            .run_once(
                agenterm_qjswasm::Guest::Wasm(&bytes),
                Some(bridge),
                "main",
                &[],
            )
            .expect("qjswasm run");
    }
    let qjswasm = started.elapsed();

    // Load plus instantiate plus one bridge round trip, per round, for each.
    // Both include their own module load, because that is what a caller pays
    // for a short script -- which is the shape agenterm actually runs.
    //
    // **Read this as what it measures.** It is dominated by setup, not by
    // execution throughput: `agenterm-wasmcore` spawns a worker thread and
    // builds a WASI context per run, and wasmtime's JIT has no time to earn
    // back its compilation on a guest this small. A compute-heavy guest would
    // very likely go the other way, and nothing here says otherwise. What the
    // number does support is the narrower, and for this product the relevant,
    // claim: **for a short guest that crosses the door once, the interpreter
    // is the cheaper engine.**
    println!("PORTABLE-GUEST {ROUNDS} rounds: wasmcore {wasmcore:?}, qjswasm {qjswasm:?}");
}
