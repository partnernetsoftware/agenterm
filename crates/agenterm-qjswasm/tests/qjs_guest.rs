//! The `.qjs` half of this crate's face, now that the compiler lives upstream.
//!
//! The compiler's own acceptance suite -- what the subset lowers, what it
//! rejects, and the exact wording of every capability diagnostic -- moved to
//! `tinyvm-qjs` with the compiler, where the code under test is. Duplicating it
//! here would be a copy that rots.
//!
//! What is still agenterm's to prove is the seam: that a `.qjs` guest goes all
//! the way through *this* engine -- compile, load gate, slot, call, cost
//! counters -- and that a compile failure is its own error class with the
//! diagnostic intact rather than a generic load rejection. That is the part a
//! version bump upstream could break, and the part no upstream test covers.

use agenterm_qjswasm::{
    Budget, Engine, Guest, GuestKind, QjswasmError, Value, compile_qjs, guest_kind_for_path,
    validate_wasm,
};

fn engine() -> Engine {
    Engine::new()
}

#[test]
fn a_qjs_guest_runs_end_to_end_through_a_slot() {
    let mut eng = engine();
    let out = eng
        .run_once(Guest::Qjs("$0*2+2"), None, "main", &[Value::I32(20)])
        .expect("a `.qjs` guest must compile, load and run");
    assert_eq!(out.values, vec![Value::I32(42)]);
    // The cost counters are the engine's, not the compiler's: a compiled guest
    // must be measured on exactly the same terms as a hand-written one.
    assert!(out.steps > 0, "a compiled guest reported zero steps");
    assert!(out.stdout.is_empty());
    assert!(!out.truncated_stdout);
}

#[test]
fn a_qjs_slot_is_persistent_like_any_other() {
    let mut eng = engine();
    let slot = eng.spawn(Guest::Qjs("$0+$1"), None).expect("spawn");
    for (a, b, want) in [(1, 2, 3), (40, 2, 42), (-5, 5, 0)] {
        let out = eng
            .call(slot, "main", &[Value::I32(a), Value::I32(b)])
            .expect("call");
        assert_eq!(out.values, vec![Value::I32(want)]);
    }
    assert_eq!(eng.live_slots(), 1);
    eng.kill(slot);
    assert_eq!(eng.live_slots(), 0);
}

/// A source the compiler will not lower is [`QjswasmError::Compile`] -- not a
/// load rejection, and not a trap.
///
/// Five failure classes exist so a caller can tell them apart without matching
/// on strings, and this is the one boundary where the distinction is easiest to
/// lose: everything downstream of the compiler also reports failures about
/// bytes, so a compile error that arrived as `Load` would look plausible and be
/// wrong about who to talk to.
#[test]
fn a_source_outside_the_subset_is_a_compile_error_not_a_load_error() {
    let mut eng = engine();
    let err = eng
        .spawn(Guest::Qjs("let x = 1"), None)
        .expect_err("`let` is not in the subset yet");
    match &err {
        QjswasmError::Compile(e) => {
            // The diagnostic must survive the trip: it speaks for the engine,
            // and it says where. Its exact wording is the compiler's contract
            // and is locked upstream.
            assert!(
                e.message.starts_with("this engine "),
                "diagnostic blames the script: {e}"
            );
            assert!(e.to_string().contains("at byte"), "{e}");
        }
        other => panic!("expected a compile error, got {other:?}"),
    }
    assert!(
        err.to_string().starts_with("compiling .qjs:"),
        "{err}, which does not say the compile step failed"
    );
    assert_eq!(
        eng.live_slots(),
        0,
        "a rejected source must not take a slot"
    );
}

/// A `.qjs` guest that fails at *run* time is a trap, not a compile error.
///
/// The other side of the same boundary: `1/0` is a perfectly compilable
/// expression whose i32 division traps, and reporting that as a compile failure
/// would tell the author to fix their syntax.
#[test]
fn a_runtime_fault_in_a_compiled_guest_is_a_trap() {
    let mut eng = engine();
    let err = eng
        .run_once(Guest::Qjs("$0/0"), None, "main", &[Value::I32(1)])
        .expect_err("integer division by zero traps");
    assert!(
        matches!(err, QjswasmError::Trap(_)),
        "expected a trap from a compiled guest, got {err:?}"
    );
}

/// What the compiler emits clears *this* crate's load gate, under this crate's
/// budget -- not merely tinyvm's defaults.
///
/// `Budget` is agenterm's policy and can be tightened independently of the
/// compiler. A guest that only ever loads under `Limits::default()` would be
/// evidence about upstream's dials, not about the ones this engine ships.
#[test]
fn compiled_bytes_clear_this_crates_load_gate() {
    for source in ["0", "$0*($1+$2)-$3%$4/$5", "-(-(-1))", "((1+2)*(3-4))/5%6"] {
        let bytes = compile_qjs(source).unwrap_or_else(|e| panic!("{source:?}: {e}"));
        assert_eq!(&bytes[..4], b"\0asm", "{source:?} is not a wasm module");
        validate_wasm(&bytes)
            .unwrap_or_else(|e| panic!("{source:?} rejected at the load gate: {e}"));
    }
    // And under a deliberately stingy budget, since that is the point of having
    // one: a module is checked against the limits it will actually run under.
    let tight = Budget {
        limits: tinyvm::Limits {
            max_memory_pages: 1,
            max_table_elems: 0,
            ..tinyvm::Limits::default()
        },
        ..Budget::default()
    };
    let bytes = compile_qjs("$0+1").unwrap();
    agenterm_qjswasm::validate_wasm_with(&bytes, &tight)
        .expect("an expression declares no memory and no table");
}

/// Extension routing, which is this crate's and not the compiler's: `.qjs`
/// reaches the compiler, `.wasm` skips it, and nothing else is claimed.
#[test]
fn extension_routing_covers_exactly_two_extensions() {
    assert_eq!(guest_kind_for_path("scripts/x.qjs"), Some(GuestKind::Qjs));
    assert_eq!(guest_kind_for_path("scripts/x.wasm"), Some(GuestKind::Wasm));
    for path in ["scripts/x.js", "scripts/x.mjs", "scripts/x", "x.qjs.txt"] {
        assert_eq!(
            guest_kind_for_path(path),
            None,
            "{path} must not route here"
        );
    }
}
