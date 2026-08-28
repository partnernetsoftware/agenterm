//! The portable `agenterm.*` door: one guest, two engines.
//!
//! PRD 02.36's archive gate 2 asks whether a `.wasm` guest could be routed to
//! either `agenterm-wasmcore` or `agenterm-qjswasm`. It could not, and the
//! blocker took three rewrites to name correctly. It is not the entry point --
//! either engine calls any export. It is not WASI -- a `no_std` guest has none.
//! It is that the two `agenterm.fleet_call`s had **different arities**: six
//! here, four there.
//!
//! As of 2026-08-28 they no longer do. `agenterm.fleet_call` here is the same
//! four-argument first pass it is there, with the same `fleet_result_len` and
//! `fleet_result` behind it; the one-call convention kept its behaviour and
//! took a name that describes it, `fleet_call_into`. The imports below are
//! byte-for-byte what a guest would write for the other engine.
//!
//! And the impossibility ran one way. The six-argument form has the host write
//! the answer into the guest through its `wasmcore_alloc`, which means the host
//! re-enters the guest; tinyvm's typed host callback holds `&mut` on guest
//! memory for its whole duration and structurally cannot. wasmtime can do
//! either. So the portable shape is the two-pass one, and this crate grew it:
//! `fleet_call_begin` / `fleet_result_len` / `fleet_result`.
//!
//! This file is that door's own test. The cross-engine half -- the same bytes
//! accepted by both -- lives in `agenterm-qjswasm`, which is the crate that can
//! depend on nothing here.

use std::sync::Arc;

use agenterm_wasmcore::{GuestExit, WasmCoreHost};

/// A guest that uses **only** the portable trio and **checks its own
/// answers**, exiting 0 when everything held and a small code naming the first
/// thing that did not.
///
/// The guest asserts rather than reporting raw numbers because WASI confines
/// an exit code to `[0, 126)` -- too narrow to encode a byte count -- and
/// because a guest that checks is a better witness anyway: it proves the
/// values crossed the boundary intact, not merely that a call returned.
///
/// Deliberately no `println!`: the whole question is what a guest must import,
/// so it imports the door and `proc_exit` and nothing else.
fn two_pass_guest() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (import "agenterm" "fleet_call"
                (func $begin (param i32 i32 i32 i32) (result i32)))
            (import "agenterm" "fleet_result_len" (func $len (result i32)))
            (import "agenterm" "fleet_result" (func $get (param i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "demo.echo")
            (data (i32.const 16) "{}")
            (func (export "_start")
                (local $n i32)
                ;; 1: the first pass must say Ok.
                (if (i32.ne
                        (call $begin (i32.const 0) (i32.const 9) (i32.const 16) (i32.const 2))
                        (i32.const 0))
                    (then (call $exit (i32.const 1))))
                ;; 2: `{"ok":true}` is eleven bytes.
                (local.set $n (call $len))
                (if (i32.ne (local.get $n) (i32.const 11))
                    (then (call $exit (i32.const 2))))
                ;; 3: the copy writes exactly that many, where the guest asked.
                (if (i32.ne (call $get (i32.const 256) (i32.const 512)) (local.get $n))
                    (then (call $exit (i32.const 3))))
                ;; 4: and the bytes really are the answer -- first and last.
                (if (i32.ne (i32.load8_u (i32.const 256)) (i32.const 123))
                    (then (call $exit (i32.const 4))))
                (if (i32.ne (i32.load8_u (i32.const 266)) (i32.const 125))
                    (then (call $exit (i32.const 5))))
                (call $exit (i32.const 0))
            )
        )"#,
    )
    .expect("valid wat")
}

#[test]
fn the_two_pass_door_answers_a_guest_that_imports_only_it() {
    let host = WasmCoreHost::new();
    let bridge: agenterm_wasmcore::WasmFleetBridgeFn = Arc::new(|op: &str, params: &str| {
        assert_eq!(op, "demo.echo", "the op id must cross intact");
        assert_eq!(params, "{}", "and so must the params");
        Ok("{\"ok\":true}".to_owned())
    });
    let result = host
        .run_module_from_bytes(&two_pass_guest(), Some(bridge))
        .expect("the guest runs");
    assert_eq!(
        result.exit,
        GuestExit::Exited(0),
        "the guest checks status, length, written count and the bytes \
         themselves; a non-zero code names which of those failed"
    );
}

#[test]
fn a_destination_too_small_is_refused_rather_than_truncated() {
    let host = WasmCoreHost::new();
    let bridge: agenterm_wasmcore::WasmFleetBridgeFn =
        Arc::new(|_: &str, _: &str| Ok("0123456789".to_owned()));
    let guest = wat::parse_str(
        r#"(module
            (import "agenterm" "fleet_call"
                (func $begin (param i32 i32 i32 i32) (result i32)))
            (import "agenterm" "fleet_result_len" (func $len (result i32)))
            (import "agenterm" "fleet_result" (func $get (param i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "x")
            (func (export "_start")
                (drop (call $begin (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 0)))
                ;; Ask for ten bytes in four: must refuse, not truncate.
                (if (i32.ge_s (call $get (i32.const 256) (i32.const 4)) (i32.const 0))
                    (then (call $exit (i32.const 1))))
                ;; And the answer must still be parked, so a guest that asks
                ;; again with enough room still gets it. A refusal that also
                ;; ate the answer would be worse than a truncation.
                (if (i32.ne (call $len) (i32.const 10))
                    (then (call $exit (i32.const 2))))
                (if (i32.ne (call $get (i32.const 256) (i32.const 64)) (i32.const 10))
                    (then (call $exit (i32.const 3))))
                (call $exit (i32.const 0))
            )
        )"#,
    )
    .expect("valid wat");
    let result = host
        .run_module_from_bytes(&guest, Some(bridge))
        .expect("the guest runs");
    assert_eq!(result.exit, GuestExit::Exited(0));
}

#[test]
fn no_bridge_is_a_status_and_not_a_crash() {
    let host = WasmCoreHost::new();
    let guest = wat::parse_str(
        r#"(module
            (import "agenterm" "fleet_call"
                (func $begin (param i32 i32 i32 i32) (result i32)))
            (import "agenterm" "fleet_result_len" (func $len (result i32)))
            (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "x")
            (func (export "_start")
                ;; Status 2 is NoBridge -- the capability is absent, which is
                ;; an answer, not a fault.
                (if (i32.ne
                        (call $begin (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 0))
                        (i32.const 2))
                    (then (call $exit (i32.const 1))))
                ;; And a diagnostic is waiting, so the guest can say why.
                (if (i32.le_s (call $len) (i32.const 0))
                    (then (call $exit (i32.const 2))))
                (call $exit (i32.const 0))
            )
        )"#,
    )
    .expect("valid wat");
    let result = host
        .run_module_from_bytes(&guest, None)
        .expect("the guest runs");
    assert_eq!(result.exit, GuestExit::Exited(0));
}

/// A guest that imports **only** `agenterm.*` and reports by **returning a
/// number from a named export** -- no WASI at all.
///
/// This is the shape gate 2 needs, and the reason it did not exist before is
/// that this crate only called `_start` and read no returned value, so a guest
/// had to reach for `proc_exit`. `WasmCoreHost::run_export` closes that.
///
/// The bytes are reused verbatim by `agenterm-qjswasm`'s
/// `host_door::the_same_guest_bytes_run_here_and_at_wasmcore`, which is the
/// other half of the claim. Kept `pub` for exactly that.
pub const PORTABLE_GUEST_WAT: &str = r#"(module
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

/// **Gate 2's shape, proved on this side.** The same bytes run at
/// `agenterm-qjswasm` too; see the test named in `PORTABLE_GUEST_WAT`.
#[test]
fn a_guest_that_imports_only_the_door_reports_through_a_named_export() {
    let host = WasmCoreHost::new();
    let bridge: agenterm_wasmcore::WasmFleetBridgeFn = Arc::new(|op: &str, params: &str| {
        assert_eq!(op, "demo.echo");
        assert_eq!(params, "{}");
        Ok("{\"ok\":true}".to_owned())
    });
    let bytes = wat::parse_str(PORTABLE_GUEST_WAT).expect("valid wat");
    let answer = host
        .run_export(&bytes, "main", Some(bridge))
        .expect("the guest runs");
    assert_eq!(
        answer, 11,
        "the guest returns the byte length it read back; negatives name which \
         of its own checks failed"
    );
}
