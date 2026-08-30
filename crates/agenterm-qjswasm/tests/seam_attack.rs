//! Adversarial attack on the engine seam: the `Value::Js` face, the per-slot
//! `Convention`, and the claim that a `.qjs` String is resolved into host-owned
//! data before the slot that owns its bytes dies.
//!
//! This file is a *hunt*, not a feature suite. `tests/qjs_guest.rs` proves the
//! seam works on the paths its authors had in mind; everything here was written
//! to make it fail. Two kinds of test live in it, and the difference matters:
//!
//! * **Green tests** are attacks the seam survived. Each one is an assertion
//!   that a specific hostile shape -- a NaN payload, an empty string, a 1 MiB
//!   string, a killed slot, a trap -- behaves correctly today, so a later
//!   change that breaks it is caught here rather than in a caller.
//! * **`FINDING` tests are the hunt's catch.** Each was written to assert the
//!   behaviour the crate's own documented doctrine says it should have, and
//!   each failed when it was written. All eight now pass, because the defect
//!   they named was fixed; the header on each records what it used to do, so
//!   the fix cannot be quietly undone without a test going red. The last to be
//!   closed -- [`finding_4_running_out_of_pages_is_now_reported_as_a_budget`]
//!   -- could not be fixed in this repository at all: it was `#[ignore]`d
//!   rather than inverted, because asserting the wrong behaviour so it passes
//!   would turn a bug report into a lock on the bug. Its header names the
//!   upstream change that closed it, tinyvm `f8adef8`.
//!
//! Every `FINDING` header states: reproducer, what was observed, what was
//! expected, and why the expectation is the crate's own and not this file's
//! taste.
//!
//! # The attack that could not be expressed, and how it can be now
//!
//! `Convention::JsV1` used to be reachable through exactly one door --
//! `Engine::spawn` from `Guest::Qjs` -- so the only bytes that ever reached
//! `Slot::read_guest_string` were bytes the trusted compiler had produced
//! moments earlier, and the five refusal arms guarding a hostile string pointer
//! had no reachable caller. `Guest::CompiledQjs` closes that: it presents
//! arbitrary bytes under the V1 convention, which is what a compile-to-disk
//! artifact needs anyway. See
//! [`finding_8_the_hostile_pointer_defence_is_unreachable_from_the_face`] and
//! the four `a_hostile_*` attacks it made possible.

use agenterm_qjswasm::{Budget, Engine, Guest, JsValue, QjswasmError, Value, compile_qjs};

fn num(x: f64) -> Value {
    Value::Js(JsValue::Number(x))
}

fn wasm(source: &str) -> Vec<u8> {
    wat::parse_str(source).expect("fixture .wat must compile; that is a bug in this file")
}

/// A guest that prints and then returns a value the engine face cannot carry,
/// next to one that prints and returns a value it can. The pair is what makes
/// [`finding_6_output_is_discarded_when_the_face_refuses_the_return_value`]
/// a measurement rather than a claim: the same print, two return types.
fn prints_then_returns() -> Vec<u8> {
    wasm(
        r#"
        (module
          (import "agenterm" "print" (func $print (param i32 i32)))
          (memory 1)
          (data (i32.const 0) "twenty-three bytes here")
          (func (export "ok") (result i32)
            (call $print (i32.const 0) (i32.const 23))
            (i32.const 1))
          (func (export "bad") (result funcref)
            (call $print (i32.const 0) (i32.const 23))
            (ref.null func))
          (func (export "quiet") (result i32) (i32.const 0)))
        "#,
    )
}

// ---------------------------------------------------------------------------
// Attacks the seam survived.
// ---------------------------------------------------------------------------

/// Every f64 the IEEE-754 domain has awkward cases for, in and out, compared on
/// *bits* rather than on `==`.
///
/// `assert_eq!` on `f64` is the wrong instrument for this attack and would have
/// hidden two of these: `-0.0 == 0.0` is true, so a lost sign bit passes, and
/// `NaN != NaN`, so a canonicalised NaN payload cannot be compared at all. The
/// V1 payload is `i64.reinterpret_f64` (`tinyvm-qjs` `repr.rs`), which is
/// bit-preserving in principle -- this is the test that it is bit-preserving in
/// fact, across the host encode, the wasm locals, and the host decode.
#[test]
fn f64_boundaries_survive_the_round_trip_bit_for_bit() {
    let cases: [(&str, u64); 13] = [
        ("+0", 0x0000_0000_0000_0000),
        ("-0", 0x8000_0000_0000_0000),
        ("+inf", 0x7ff0_0000_0000_0000),
        ("-inf", 0xfff0_0000_0000_0000),
        ("quiet NaN", 0x7ff8_0000_0000_0000),
        ("NaN with a payload", 0x7ff8_0000_0000_0001),
        ("negative NaN with a payload", 0xfff8_dead_beef_cafe),
        ("signalling NaN", 0x7ff0_0000_0000_0001),
        ("all ones", 0xffff_ffff_ffff_ffff),
        ("f64::MAX", 0x7fef_ffff_ffff_ffff),
        ("smallest normal", 0x0010_0000_0000_0000),
        ("smallest subnormal", 0x0000_0000_0000_0001),
        ("f64::EPSILON", 0x3cb0_0000_0000_0000),
    ];
    for (name, bits) in cases {
        let mut eng = Engine::new();
        let out = eng
            .run_once(Guest::Qjs("$0"), None, "main", &[num(f64::from_bits(bits))])
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        match out.values.as_slice() {
            [Value::Js(JsValue::Number(y))] => assert_eq!(
                y.to_bits(),
                bits,
                "{name}: {:#018x} came back as {:#018x}",
                bits,
                y.to_bits()
            ),
            other => panic!("{name}: expected one JS number, got {other:?}"),
        }
    }
}

/// `-0` produced *inside* the guest keeps its sign across the seam.
///
/// Separate from the round trip above because it exercises a different half:
/// there the sign bit only had to be copied, here it has to be produced by
/// `f64.neg` and then survive boxing. Asserted with `is_sign_negative`, since
/// `assert_eq!(-0.0, 0.0)` succeeds.
#[test]
fn negative_zero_produced_in_the_guest_keeps_its_sign() {
    let mut eng = Engine::new();
    let out = eng
        .run_once(Guest::Qjs("let z = 0; return -z;"), None, "main", &[])
        .expect("negating zero is not a fault");
    match out.values.as_slice() {
        [Value::Js(JsValue::Number(y))] => {
            assert!(y.is_sign_negative(), "-0 came back as +0");
            assert_eq!(y.to_bits(), 0x8000_0000_0000_0000);
        }
        other => panic!("expected one JS number, got {other:?}"),
    }
}

/// The load-bearing claim, attacked at the sizes and shapes most likely to
/// break a pointer-to-owned-data projection.
///
/// The existing suite proves one short ASCII string outlives its slot. These
/// are the neighbours: a string of length zero (whose body slice is empty and
/// whose `at + 4 .. at + 4` range is the one range a naive bounds check gets
/// wrong), an embedded NUL (which a C-style reader would truncate at), an
/// astral character built from a surrogate *pair* (four UTF-8 bytes, and the
/// case a UTF-16-length header would mis-size), and one megabyte (which forces
/// the guest's bump allocator to grow linear memory, so the view the seam reads
/// through must be the grown one and not a stale snapshot).
#[test]
fn strings_of_every_awkward_shape_are_resolved_before_the_slot_dies() {
    let cases: [(&str, &str); 5] = [
        ("empty", "return \"\";"),
        ("embedded NUL", "return \"a\\u0000b\";"),
        (
            "astral, from a surrogate pair",
            "return \"\\uD83D\\uDE00\";",
        ),
        ("non-ASCII", "return \"caf\\u00e9\";"),
        (
            "built at run time from two halves",
            "return \"tab\" + \"s.list\";",
        ),
    ];
    let expected = ["", "a\0b", "\u{1F600}", "café", "tabs.list"];
    for ((name, source), want) in cases.iter().zip(expected) {
        let mut eng = Engine::new();
        let out = eng
            .run_once(Guest::Qjs(source), None, "main", &[])
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(eng.live_slots(), 0, "{name}: run_once left a slot behind");
        assert_eq!(
            out.values,
            vec![Value::Js(JsValue::Str(want.to_string()))],
            "{name}"
        );
    }
}

/// One megabyte of text, resolved out of a linear memory the guest grew during
/// the call, and read after the slot is gone.
///
/// This is the size at which a stale memory view stops being a theory: the
/// literal pool fits in the module's declared minimum, but the final
/// concatenation's destination is sixteen pages past it, so `__alloc` runs
/// `memory.grow`. If `Slot::read_guest_string` resolved through a view captured
/// at instantiation the tail of this string would be out of bounds.
#[test]
fn a_megabyte_string_from_grown_memory_survives_the_slot() {
    let budget = Budget {
        limits: tinyvm::Limits {
            max_steps: 1_000_000_000,
            ..tinyvm::Limits::default()
        },
        ..Budget::default()
    };
    let mut eng = Engine::with_budget(budget);
    let source = "let s = \"0123456789abcdef\"; let i = 0; \
                  while (i < 16) { s = s + s; i = i + 1; } return s;";
    let out = eng
        .run_once(Guest::Qjs(source), None, "main", &[])
        .expect("a megabyte of concatenation is within this budget");
    assert_eq!(eng.live_slots(), 0);
    match out.values.as_slice() {
        [Value::Js(JsValue::Str(s))] => {
            assert_eq!(s.len(), 1 << 20);
            assert!(s.starts_with("0123456789abcdef"));
            assert!(s.ends_with("0123456789abcdef"));
        }
        other => panic!("expected one JS string, got {other:?}"),
    }
}

/// A string reached through a *second* call on a live slot, with the first
/// call's string still held.
///
/// The bump allocator has no free and no collector, so the second call's
/// allocation lands past the first's. If the seam ever forwarded a pointer
/// instead of resolving, this is where it would show: the first value would
/// still parse, but as whatever the second call left at that address.
#[test]
fn a_string_from_an_earlier_call_is_not_disturbed_by_a_later_one() {
    let mut eng = Engine::new();
    let slot = eng
        .spawn(
            Guest::Qjs(
                "if ($0 < 1) { return \"first-call-string\"; } return \"second\" + \"-call\";",
            ),
            None,
        )
        .expect("spawn");
    let first = eng.call(slot, "main", &[num(0.0)]).expect("first call");
    let second = eng.call(slot, "main", &[num(5.0)]).expect("second call");
    assert_eq!(
        first.values,
        vec![Value::Js(JsValue::Str("first-call-string".into()))]
    );
    assert_eq!(
        second.values,
        vec![Value::Js(JsValue::Str("second-call".into()))]
    );
    eng.kill(slot);
    // Held across the slot's death, which is the whole point of resolving.
    assert_eq!(
        first.values,
        vec![Value::Js(JsValue::Str("first-call-string".into()))]
    );
}

/// A string literal the seam could not resolve is refused by the *compiler*,
/// before any slot exists.
///
/// An unpaired surrogate is the one JavaScript string that has no UTF-8
/// encoding, so it is the one input that could reach
/// `String::from_utf8` (`src/slot.rs:163`) with bytes that are not text. It
/// does not: the compiler names it a subset boundary. That closes the
/// non-UTF-8 attack for `.qjs` sources -- and is the reason the corresponding
/// arm in `read_guest_string` has no reachable caller.
#[test]
fn a_string_with_no_utf8_encoding_is_refused_at_compile_time() {
    let mut eng = Engine::new();
    let err = eng
        .spawn(Guest::Qjs("return \"\\uD800\";"), None)
        .expect_err("an unpaired surrogate has no UTF-8 encoding");
    match &err {
        QjswasmError::Compile(e) => assert!(
            e.message.contains("unpaired surrogates"),
            "expected a capability diagnostic about surrogates, got {e}"
        ),
        other => panic!("expected a compile error, got {other:?}"),
    }
    assert_eq!(eng.live_slots(), 0);
}

/// A `JsValue::Str` argument is refused with a typed error, does not panic, and
/// does not disturb the slot.
///
/// The refusal itself is already covered upstream in `tests/qjs_guest.rs`. What
/// is attacked here is what the refusal *costs*: that it happens before the
/// guest is entered, and that the slot is still usable afterwards -- a refusal
/// that half-ran the entry point, or that left the argument encoder's state
/// behind, would be worse than a panic because it would be silent.
#[test]
fn a_refused_string_argument_costs_the_slot_nothing() {
    let mut eng = Engine::new();
    let slot = eng.spawn(Guest::Qjs("$0"), None).expect("spawn");
    let err = eng
        .call(slot, "main", &[Value::Js(JsValue::Str("x".into()))])
        .expect_err("a string argument has nowhere to live in the guest yet");
    assert!(
        matches!(err, QjswasmError::UnsupportedValue(_)),
        "got {err:?}"
    );
    // Mixed lists are refused on the same terms, not partially encoded.
    let err = eng
        .call(
            slot,
            "main",
            &[num(1.0), Value::Js(JsValue::Str("x".into()))],
        )
        .expect_err("one bad argument refuses the whole list");
    assert!(
        matches!(err, QjswasmError::UnsupportedValue(_)),
        "got {err:?}"
    );
    assert_eq!(eng.live_slots(), 1, "a refusal must not retire the slot");
    let out = eng.call(slot, "main", &[num(7.0)]).expect("still callable");
    assert_eq!(out.values, vec![num(7.0)]);
}

/// A killed slot is [`QjswasmError::NoSuchSlot`], twice over, and killing twice
/// is not a panic.
///
/// Ids are never recycled -- `Engine::next_id` only increments and `slots` is
/// never compacted -- so a stale id can never come to mean a *different* slot
/// within one engine. That is the property this locks. (It does not hold across
/// two engines; see
/// [`finding_5_a_slot_id_is_not_bound_to_the_engine_that_minted_it`].)
#[test]
fn a_killed_slot_is_a_typed_refusal_and_its_id_is_never_recycled() {
    let mut eng = Engine::new();
    let first = eng.spawn(Guest::Qjs("return 1;"), None).expect("spawn");
    eng.kill(first);
    eng.kill(first);
    assert_eq!(eng.live_slots(), 0);
    match eng.call(first, "main", &[]) {
        Err(QjswasmError::NoSuchSlot(id)) => assert_eq!(id, first),
        other => panic!("expected NoSuchSlot, got {:?}", other.map(|o| o.values)),
    }
    let second = eng.spawn(Guest::Qjs("return 2;"), None).expect("spawn");
    assert_ne!(second, first, "a fresh slot must not reuse a dead id");
    assert_eq!(
        eng.call(second, "main", &[]).expect("call").values,
        vec![num(2.0)]
    );
    match eng.call(first, "main", &[]) {
        Err(QjswasmError::NoSuchSlot(_)) => {}
        other => panic!(
            "the dead id came back to life: {:?}",
            other.map(|o| o.values)
        ),
    }
}

/// A trap leaves the slot live and callable, and `run_once` reclaims on *every*
/// failure path, not only the successful one.
///
/// `run_once` calls `kill` before it inspects the result (`src/lib.rs`), so the
/// reclaim is unconditional by construction -- but "by construction" is exactly
/// the kind of claim that survives a refactor as a comment and not as a fact.
/// Three different failure classes are run through it here.
///
/// The trapping guest is a hand-written `unreachable`. It used to be `"2" * 2`,
/// a String-to-Number coercion, which the `f21f0f2` bump implemented (ba143c5);
/// then a call on a non-function, named at a4e12fd; then `"" + {}`, named at
/// the bump after it. Every stop a `.qjs` program can reach now has a name, so
/// the nameless trap has to be written by hand. What is under test is the
/// accounting, not the fault.
#[test]
fn live_slots_accounting_holds_across_traps_and_every_run_once_failure() {
    let bytes = wasm(r#"(module (func (export "main") (unreachable)))"#);
    let mut eng = Engine::new();
    let slot = eng.spawn(Guest::Wasm(&bytes), None).expect("spawn");
    let err = eng
        .call(slot, "main", &[])
        .expect_err("an unreachable traps");
    assert!(matches!(err, QjswasmError::Trap(_)), "got {err:?}");
    assert_eq!(eng.live_slots(), 1, "a trap must not retire the slot");
    assert!(
        eng.call(slot, "main", &[]).is_err(),
        "the same fault must be reportable again"
    );
    assert_eq!(eng.live_slots(), 1);
    eng.kill(slot);

    for (what, guest, entry, args) in [
        ("a trap", "let f = 1; return f();", "main", vec![]),
        ("a wrong argument count", "$0+$1", "main", vec![]),
        ("a missing export", "1", "not_an_export", vec![]),
        (
            "a refused argument",
            "$0",
            "main",
            vec![Value::Js(JsValue::Str("x".into()))],
        ),
    ] {
        let before = eng.live_slots();
        let result = eng.run_once(Guest::Qjs(guest), None, entry, &args);
        assert!(result.is_err(), "{what}: expected a failure");
        assert_eq!(
            eng.live_slots(),
            before,
            "{what}: run_once did not reclaim its slot"
        );
    }
    assert_eq!(eng.live_slots(), 0);
}

/// Output buffered by a call that trapped does not leak into the next call.
///
/// The slot survives a trap, so the door's pending stdout survives with it
/// unless it is drained. If it were not, the *next* call's `Outcome` would
/// carry another call's output -- attributed to the wrong invocation, which is
/// worse than losing it.
#[test]
fn stdout_from_a_trapping_call_does_not_reappear_in_the_next_one() {
    let bytes = wasm(
        r#"
        (module
          (import "agenterm" "print" (func $print (param i32 i32)))
          (memory 1)
          (data (i32.const 0) "before-the-trap")
          (func (export "boom")
            (call $print (i32.const 0) (i32.const 15))
            (unreachable))
          (func (export "quiet") (result i32) (i32.const 1)))
        "#,
    );
    let mut eng = Engine::new();
    let slot = eng.spawn(Guest::Wasm(&bytes), None).expect("spawn");
    assert!(eng.call(slot, "boom", &[]).is_err(), "boom must trap");
    let out = eng.call(slot, "quiet", &[]).expect("quiet call");
    assert_eq!(
        out.stdout, "",
        "the trapping call's output reappeared in a later Outcome"
    );
}

// ---------------------------------------------------------------------------
// Findings. Each asserts what the crate's own doctrine says should happen, and
// each failed when it was written. All of them pass now; the headers keep the
// reproducer and what it used to do.
// ---------------------------------------------------------------------------

/// FINDING 1 -- a wrong argument *count* is reported as a guest trap.
///
/// **Reproducer:** `Engine::spawn(Guest::Qjs("$0+$1"))`, then `call(slot,
/// "main", &[])` -- or one argument, or three.
///
/// **Observed:** `Err(Trap("function"))`, for every count except the exact one.
/// The same holds for a hand-written guest: `(param i32)` called with no
/// arguments is `Trap("function")` too.
///
/// **Expected, and now the behaviour:** [`QjswasmError::Signature`], naming the
/// expected and the given count, raised before the guest is entered.
///
/// **Why that is this crate's own expectation, not this file's taste:**
/// `QjswasmError::UnsupportedValue`'s own documentation says it is "deliberately
/// its own class rather than a `Trap`: nothing went wrong inside the guest, so
/// reporting a trap would blame it for a limitation of *this* face." And
/// `tests/qjs_guest.rs` states the exact failure mode as the reason its
/// convention check exists: "Without this, a `Value::I32(20)` handed to a
/// `.qjs` entry point would be a wasm arity mismatch reported as a trap,
/// blaming a guest that did nothing wrong." The convention half of that mistake
/// was guarded. The count half was not.
///
/// **Aggravating:** the face offers no way to ask. `Engine` exposes no arity,
/// while `tinyvm::WasmInstance::exported_function_handle` exposes
/// `parameter_count` / `parameter_type` exactly -- see
/// [`the_exact_signature_the_seam_declines_to_consult_is_available`]. A caller
/// must therefore guess, and a wrong guess is reported as the guest's fault.
/// The guess is still required -- `Engine` still exposes no arity -- but a
/// wrong one is now the caller's answer to read rather than the guest's
/// obituary.
#[test]
fn finding_1_a_wrong_argument_count_is_blamed_on_the_guest() {
    let mut eng = Engine::new();
    let slot = eng.spawn(Guest::Qjs("$0+$1"), None).expect("spawn");
    for args in [vec![], vec![num(1.0)], vec![num(1.0), num(2.0), num(3.0)]] {
        let n = args.len();
        let err = eng
            .call(slot, "main", &args)
            .expect_err("this is the wrong number of arguments");
        assert!(
            matches!(err, QjswasmError::Signature(_)),
            "{n} arguments to a two-argument entry: expected a typed refusal, got {err:?}"
        );
        // Counted in JavaScript arguments, not in the wasm words that carry
        // them: a `.qjs` caller never chose two words per value.
        let text = err.to_string();
        assert!(
            text.contains("2 JavaScript argument(s)") && text.contains(&format!("{n} given")),
            "the refusal does not name both counts: {text}"
        );
    }
    // The refusal costs the slot nothing: the right count still works.
    assert_eq!(
        eng.call(slot, "main", &[num(1.0), num(2.0)])
            .expect("the right count")
            .values,
        vec![num(3.0)]
    );

    let bytes = wasm("(module (func (export \"id\") (param i32) (result i32) local.get 0))");
    let hand_written = eng.spawn(Guest::Wasm(&bytes), None).expect("spawn");
    let err = eng
        .call(hand_written, "id", &[])
        .expect_err("this is the wrong number of arguments");
    assert!(
        matches!(err, QjswasmError::Signature(_)),
        "expected a typed refusal, got {err:?}"
    );
    assert!(
        err.to_string().contains("1 wasm parameter(s), 0 given"),
        "the refusal does not name both counts: {err}"
    );
}

/// FINDING 2 -- a wrong argument *type* is not refused, and can succeed.
///
/// **Reproducer:** a hand-written export `(param i32) (result i32)` that just
/// returns its parameter, called with `Value::I64(1 << 32)`.
///
/// **Observed:** `Ok([I64(4294967296)])`. No refusal, no trap, and a result
/// whose type contradicts the export's declared `(result i32)`. The same export
/// called with `Value::F64(1.5)` returns `Ok([F64(1.5)])`. When the guest
/// actually *uses* the value -- `i32.add`, `i32.load` -- it becomes
/// `Trap("expected i32 on stack, got")`, so whether a host-side type error is
/// reported at all depends on whether the guest happens to touch the argument,
/// and when it is reported it is again reported as the guest's fault.
///
/// **Expected, and now the behaviour:** [`QjswasmError::Signature`] at the
/// face, before the guest is entered, naming the parameter and the two types.
///
/// **Not memory-unsafe:** this was checked, not assumed. An `I64` or `F64`
/// substituted for an `i32` address is refused at the *use* site by the
/// interpreter's operand type check, and the bounds check still holds for a
/// genuine `I32` address (`load8(I32(65536))` on a one-page memory is
/// `Trap("memory access [")`). The defect is a typing hole at the boundary and
/// a misattributed error class, not an escape.
#[test]
fn finding_2_a_mistyped_argument_is_not_refused_at_the_face() {
    let bytes = wasm(
        r#"
        (module
          (func (export "id") (param i32) (result i32) (local.get 0))
          (func (export "id64") (param i64) (result i64) (local.get 0)))
        "#,
    );
    let mut eng = Engine::new();
    let slot = eng.spawn(Guest::Wasm(&bytes), None).expect("spawn");

    let err = eng
        .call(slot, "id", &[Value::I64(1 << 32)])
        .expect_err("an i64 is not an i32");
    assert!(
        matches!(err, QjswasmError::Signature(_)),
        "expected a typed refusal, got {err:?}"
    );
    assert!(
        err.to_string()
            .contains("`id` parameter 0: the signature declares i32, the caller gave i64"),
        "the refusal does not name the parameter and both types: {err}"
    );

    let err = eng
        .call(slot, "id64", &[Value::I32(7)])
        .expect_err("an i32 is not an i64");
    assert!(
        matches!(err, QjswasmError::Signature(_)),
        "expected a typed refusal, got {err:?}"
    );
    // Float for integer, the case the core would never notice at all: `id`
    // never touches its parameter, so this used to be `Ok([F64(1.5)])` -- a
    // result contradicting the export's own `(result i32)`.
    let err = eng
        .call(slot, "id", &[Value::F64(1.5)])
        .expect_err("an f64 is not an i32");
    assert!(
        matches!(err, QjswasmError::Signature(_)),
        "expected a typed refusal, got {err:?}"
    );
    // And the correct call still works, on the same slot, afterwards.
    assert_eq!(
        eng.call(slot, "id", &[Value::I32(7)])
            .expect("the declared type")
            .values,
        vec![Value::I32(7)]
    );
}

/// FINDING 3 -- an export that does not exist is reported as a guest trap.
///
/// **Reproducer:** `call(slot, "not_an_export", &[])` on any live slot.
///
/// **Observed:** `Err(Trap("no exported function named"))` -- and the message
/// does not say which name, because tinyvm is `no_std` and its messages are
/// static prefixes.
///
/// **Expected, and now the behaviour:** a class of its own,
/// [`QjswasmError::NoSuchExport`], carrying the entry name the caller asked
/// for.
///
/// **Why:** "this slot has no such export" is the same shape of mistake as
/// "this engine has no such slot", and the latter already has
/// [`QjswasmError::NoSuchSlot`] rather than being folded into `Trap`. The guest
/// did not run, so nothing in it trapped; the caller mistyped a name and gets
/// told their script faulted.
#[test]
fn finding_3_a_missing_export_is_blamed_on_the_guest() {
    let mut eng = Engine::new();
    let slot = eng.spawn(Guest::Qjs("1"), None).expect("spawn");
    let err = eng
        .call(slot, "not_an_export", &[])
        .expect_err("there is no such export");
    assert!(
        matches!(err, QjswasmError::NoSuchExport(_)),
        "expected a typed refusal, got {err:?}"
    );
    assert!(
        err.to_string().contains("not_an_export"),
        "the error does not say which entry point was missing: {err}"
    );
    // The slot survives a mistyped name, as it survived it before.
    assert_eq!(
        eng.call(slot, "main", &[]).expect("still live").values,
        vec![num(1.0)]
    );
}

/// FINDING 4 -- exhausting `max_memory_pages` at run time used to be a
/// `Trap`, never a `Budget`.
///
/// **Reproducer:** a budget with `max_memory_pages: 2`, and a `.qjs` guest that
/// concatenates its way past two pages.
///
/// **Was:** `Err(Trap("unreachable executed"))`.
///
/// **Now, and what the doctrine always required:** `Err(Budget("max_memory_pages"))`.
///
/// **Why it was wrong:** `Budget::limits` is documented as "Core-enforced
/// limits: instruction steps per top-level call, **linear memory pages**,
/// ...", and `slot.rs`'s `ceiling_name` carries a `WasmCeiling::MemoryPages ->
/// "max_memory_pages"` arm for precisely this. `Engine::call`'s own
/// documentation says budget exhaustion "must be answerable without matching on
/// a message string". It was not answerable: the compiler's `__alloc`
/// (`tinyvm-qjs` `runtime.rs`) turned a refused `memory.grow` into
/// `unreachable`, so the ceiling arrived at the seam as an ordinary guest trap.
///
/// **Why it was `#[ignore]`d for a while, and what closed it.** The
/// information was destroyed upstream, before the seam could see it, and a
/// host-side heuristic -- "the trap was `unreachable` and memory happens to sit
/// at `max_memory_pages`, so call it a budget" -- would have mislabelled a
/// genuinely broken script, which is the silent misclassification `classify`'s
/// doctrine exists to prevent. The header used to say the fix had to be
/// upstream, and it was: tinyvm `f8adef8` has the allocator record
/// `FAULT_HEAP_EXHAUSTED` in the first word of the guest's own memory before it
/// gives up, and `tinyvm_qjs::guest_fault` reads it back. `slot.rs` consults it
/// on the error path of a `JsV1` slot, so the guest states the reason and the
/// seam repeats it -- no guess anywhere.
///
/// The load-time half was classified all along: a literal pool larger than the
/// budget is `Load("memory page limit")`, which
/// [`the_page_ceiling_is_classified_at_load_time`] locks. Both halves now name
/// the same field.
#[test]
fn finding_4_running_out_of_pages_is_now_reported_as_a_budget() {
    let budget = Budget {
        limits: tinyvm::Limits {
            max_memory_pages: 2,
            max_steps: 100_000_000,
            ..tinyvm::Limits::default()
        },
        ..Budget::default()
    };
    let mut eng = Engine::with_budget(budget);
    let source = "let s = \"0123456789abcdef\"; let i = 0; \
                  while (i < 20) { s = s + s; i = i + 1; } return s;";
    let err = eng
        .run_once(Guest::Qjs(source), None, "main", &[])
        .expect_err("two pages cannot hold this");
    assert!(
        matches!(err, QjswasmError::Budget("max_memory_pages")),
        "expected the page ceiling to be named as a budget, got {err:?}"
    );
}

/// FINDING 5 -- a [`SlotId`] is not bound to the engine that minted it, so one
/// engine's id silently addresses another engine's slot.
///
/// **Reproducer:** two `Engine`s, one `spawn` each; call and kill engine `b`
/// with engine `a`'s id.
///
/// **Observed:** the ids compare equal (`SlotId(0)` both times, since `next_id`
/// starts at 0 per engine). `b.call(a_id, "main", &[])` runs **b's** slot and
/// returns its value. `b.kill(a_id)` destroys **b's** slot: `b.live_slots()`
/// drops to 0 and b's own handle then reports `NoSuchSlot`.
///
/// **Expected, and now the behaviour:** [`QjswasmError::NoSuchSlot`] for the
/// call, and no effect for the kill -- an id from another engine names nothing
/// here. `SlotId` now carries a process-wide engine tag beside the index.
///
/// **Why this is wrong behaviour and not a caller error:** `SlotId` is `Copy`,
/// `Eq` and `Hash`, i.e. built to be stored in collections and passed around,
/// and its documentation says only "Handle to one live slot. Invalid after
/// `Engine::kill`" -- nothing warns that it is engine-relative. The failure is
/// silent in both directions: the wrong guest runs, or the wrong guest is
/// destroyed, and no error is ever produced. A generation or engine tag inside
/// the opaque `u64` would make it `NoSuchSlot`.
#[test]
fn finding_5_a_slot_id_is_not_bound_to_the_engine_that_minted_it() {
    let mut a = Engine::new();
    let mut b = Engine::new();
    let in_a = a.spawn(Guest::Qjs("return 111;"), None).expect("spawn");
    let in_b = b.spawn(Guest::Qjs("return 222;"), None).expect("spawn");

    match b.call(in_a, "main", &[]) {
        Err(QjswasmError::NoSuchSlot(_)) => {}
        other => panic!(
            "engine a's id addressed a slot in engine b: {:?}",
            other.map(|o| o.values)
        ),
    }

    b.kill(in_a);
    assert_eq!(
        b.live_slots(),
        1,
        "engine a's id destroyed a slot in engine b"
    );
    assert_eq!(
        b.call(in_b, "main", &[]).expect("b's own slot").values,
        vec![num(222.0)]
    );
    // And the tag is not merely "engine b is younger": a's own id still works
    // in a, so the tag discriminates rather than invalidating.
    assert_eq!(
        a.call(in_a, "main", &[]).expect("a's own slot").values,
        vec![num(111.0)]
    );
    assert_ne!(in_a, in_b, "two engines still mint the same id");
}

/// FINDING 6 -- output a guest already produced is discarded when the *face*
/// refuses its return value.
///
/// **Reproducer:** a module that calls `agenterm.print` and then returns a
/// `funcref`.
///
/// **Observed:** `Err(UnsupportedValue("export returned a reference or vector
/// value type"))`, and the 23 printed bytes are gone -- `Slot::call` drains the
/// door's stdout into a local and then returns `Err` from `from_val`, so the
/// buffer is emptied and the text dropped. A later call on the same slot sees
/// an empty `stdout`, confirming the bytes are not merely deferred.
///
/// **Expected:** the guest's output is not lost to a failure that is not the
/// guest's.
///
/// **Why:** the drain is deliberate and correct -- `slot.rs` explains that
/// leaving it buffered "would leak it into the *next* call's `Outcome`, which
/// is worse than losing it" -- but that reasoning was written about a *trapping*
/// call. `UnsupportedValue` is the class the crate reserves for "nothing went
/// wrong inside the guest", and here that is literally true: the guest's work,
/// including its output, was fine. Losing it is the one outcome the door's own
/// failure policy rejects everywhere else ("a cut the caller is told about beats
/// a silent drop", `host.rs`).
///
/// **How it was fixed, and why not the obvious way.** The obvious fix -- hand
/// the buffered bytes to the *next* call -- is the behaviour `slot.rs` already
/// rejects by name: it attributes one call's output to another, which is worse
/// than losing it. The fix instead removes the loss rather than relocating it.
/// The export's *declared* result type is checked before the guest is entered
/// (`Slot::check_entry`), so a `(result funcref)` export is refused while its
/// body is still un-run: nothing is printed, so nothing is dropped. That is
/// strictly stronger than the original expectation -- the output does not have
/// to survive the refusal, because it is never produced.
///
/// The residual is now a *stated* cost rather than an emergent one: a call that
/// fails after the guest has run -- a trap, a budget, a malformed V1 pair --
/// still discards what it printed, and that is documented on `Slot::call`.
#[test]
fn finding_6_output_is_discarded_when_the_face_refuses_the_return_value() {
    let bytes = prints_then_returns();
    let mut eng = Engine::new();
    let slot = eng.spawn(Guest::Wasm(&bytes), None).expect("spawn");

    // The control: the same print, a return type the face can carry.
    let ok = eng.call(slot, "ok", &[]).expect("ok call");
    assert_eq!(ok.stdout, "twenty-three bytes here\n");

    let err = eng
        .call(slot, "bad", &[])
        .expect_err("a funcref cannot cross the face");
    assert!(
        matches!(err, QjswasmError::UnsupportedValue(_)),
        "got {err:?}"
    );
    // The refusal happened before `bad` ran, so its print never fired. The
    // proof is the door's buffer: the next call sees nothing, which is both
    // "no output was lost" and "no output leaked into the wrong `Outcome`".
    let after = eng.call(slot, "quiet", &[]).expect("quiet call");
    assert_eq!(
        after.stdout, "",
        "the refused call ran far enough to print, and its output went nowhere"
    );
    // The slot is untouched by the refusal.
    assert_eq!(
        eng.call(slot, "ok", &[]).expect("ok again").stdout,
        "twenty-three bytes here\n"
    );
}

/// FINDING 7 -- the returned-string projection is a host-side buffer that no
/// `Budget` field bounds.
///
/// **Reproducer:** `Budget { max_stdout_bytes: 16, max_bridge_result_bytes: 16,
/// .. }` and a `.qjs` guest that returns a megabyte of text.
///
/// **Observed:** a 1 048 576-byte host `String`, with both host-side caps set to
/// 16 bytes.
///
/// **Expected, and now the behaviour:** a cap of its own,
/// `Budget::max_result_string_bytes`, checked against the declared length
/// before the copy and reported as `Budget("max_result_string_bytes")` rather
/// than truncating -- half a string is worse than a refusal, exactly as for
/// `max_bridge_result_bytes`.
///
/// **Why:** `Budget`'s documentation says "Execution limits live in the tinyvm
/// core; **the two host-side caps bound what the door itself will buffer**" --
/// and the seam's string resolution is a third host-side buffer, allocated by
/// the host, sized by the guest, that neither cap governs. Today it is bounded
/// incidentally: `max_steps` runs out first, at roughly 256 KiB per call under
/// the default budget (measured: 14 doublings succeed at 9.4M steps, 15 is
/// `Budget("max_steps")`). That bound is an artefact of concatenation being
/// O(n) in steps, not a policy -- a future guest that produces a large string
/// cheaply, or a raised `max_steps`, moves the ceiling to
/// `max_memory_pages * 64 KiB` (16 MiB by default) per call, per call, on a
/// persistent slot.
#[test]
fn finding_7_the_returned_string_is_bounded_by_no_host_side_cap() {
    let budget = Budget {
        limits: tinyvm::Limits {
            max_steps: 1_000_000_000,
            ..tinyvm::Limits::default()
        },
        max_stdout_bytes: 16,
        max_bridge_result_bytes: 16,
        max_result_string_bytes: 16,
        // A1.12: host operations are capped like steps; generous here so the
        // seam under test is the one named above.
        max_host_ops: 4096,
        cancel: None,
    };
    let source = "let s = \"0123456789abcdef\"; let i = 0; \
                  while (i < 16) { s = s + s; i = i + 1; } return s;";
    let mut eng = Engine::with_budget(budget.clone());
    let err = eng
        .run_once(Guest::Qjs(source), None, "main", &[])
        .expect_err("a megabyte does not fit a sixteen-byte cap");
    assert!(
        matches!(err, QjswasmError::Budget("max_result_string_bytes")),
        "expected the cap to be named as a budget, got {err:?}"
    );

    // It is a cap and not a ban: exactly at the cap still crosses, so the
    // refusal above is the size and not the mechanism.
    let mut eng = Engine::with_budget(budget);
    let out = eng
        .run_once(
            Guest::Qjs("return \"0123456789abcdef\";"),
            None,
            "main",
            &[],
        )
        .expect("sixteen bytes is within a sixteen-byte cap");
    assert_eq!(
        out.values,
        vec![Value::Js(JsValue::Str("0123456789abcdef".to_string()))]
    );
}

/// FINDING 8 -- the hostile-pointer defence could not be reached from the
/// public face, so the load-bearing claim had no adversarial coverage.
///
/// **Observed:** `Convention::JsV1` was assigned in exactly one place --
/// `Engine::spawn`, from `Guest::Qjs`, whose bytes come from `compile_qjs` two
/// lines earlier. `Slot::read_guest_string` is called only under that
/// convention. Therefore every pointer it ever saw was produced by the trusted
/// compiler in the same call, and all five of its refusal arms -- negative or
/// oversized pointer, header out of bounds, body out of bounds, invalid UTF-8,
/// and no linear memory -- had no reachable caller. The attacks this lane was
/// asked to run against them could not be expressed through `Engine`'s API at
/// all.
///
/// **Fixed by [`Guest::CompiledQjs`]**, which presents arbitrary bytes under
/// the V1 convention. That variant is not a test hook: a `.wasm` artifact
/// compiled from `.qjs` and loaded back later -- a `pack` output, a cache, a
/// guest fetched over the wire -- has exactly this need, and without it the
/// convention is lost on the way through the file system. Making the refusals
/// reachable is the same change. The four `a_hostile_*` attacks below are what
/// it bought; each one used to be inexpressible.
///
/// **What was deliberately *not* changed:** the finding also suggested a
/// signature check at `spawn`, so bytes that "plainly speak V1" are not loaded
/// as plain wasm. That is declined. `(i32, i64, ...) -> (i32, i64)` is an
/// ordinary hand-written wasm type, so a signature check would be a guess, and
/// `Convention`'s own doctrine is that the convention is *recorded* at load
/// time and "can never be re-derived by guessing at a signature". With
/// `CompiledQjs` in the face, loading compiled bytes as `Wasm` is a caller
/// naming a convention, not a trap they can fall into -- and this test pins
/// that reading of the raw pair, so the two variants are demonstrably
/// different rather than accidentally the same.
#[test]
fn finding_8_the_hostile_pointer_defence_is_unreachable_from_the_face() {
    let bytes = compile_qjs("return \"hello\";").expect("compiles");

    // Under the V1 convention the seam resolves the string into host data.
    let mut eng = Engine::new();
    let out = eng
        .run_once(Guest::CompiledQjs(&bytes), None, "main", &[])
        .expect("compiled bytes run under their own convention");
    assert_eq!(
        out.values,
        vec![Value::Js(JsValue::Str("hello".to_string()))]
    );

    // The same bytes named as plain wasm hand over the raw pair -- tag 3 is
    // TAG_STRING and the payload is a guest heap address. Deliberate: the
    // caller said which convention these bytes speak.
    let mut eng = Engine::new();
    let slot = eng.spawn(Guest::Wasm(&bytes), None).expect("also load");
    let out = eng.call(slot, "main", &[]).expect("and run");
    assert!(
        matches!(out.values.as_slice(), [Value::I32(3), Value::I64(_)]),
        "expected the unresolved V1 pair, got {:?}",
        out.values
    );
}

/// A V1 guest whose entry returns `(tag, payload)` exactly as written.
///
/// The whole point of [`Guest::CompiledQjs`] for this file: bytes the compiler
/// did not produce, presented under the convention whose refusal arms need
/// attacking. `pages` and `data` shape the linear memory the payload will be
/// interpreted against.
fn hostile_v1(tag: i32, payload: i64, pages: Option<u32>, data: &str) -> Vec<u8> {
    let memory = match pages {
        Some(n) => format!("(memory {n}) (data (i32.const 0) \"{data}\")"),
        None => String::new(),
    };
    wasm(&format!(
        "(module {memory}
           (func (export \"main\") (result i32 i64)
             (i32.const {tag}) (i64.const {payload})))"
    ))
}

/// A string pointer that is not a guest address at all is a typed `Door`
/// refusal, not a panic and not invented text.
///
/// `0xFFFF_FFFF` decodes through `bits as u32 as i32` to `-1`, which is the one
/// value `usize::try_from` rejects outright -- the first of the five arms.
#[test]
fn a_hostile_string_pointer_outside_the_address_space_is_refused() {
    let bytes = hostile_v1(3, 0xFFFF_FFFF, Some(1), "");
    let mut eng = Engine::new();
    let err = eng
        .run_once(Guest::CompiledQjs(&bytes), None, "main", &[])
        .expect_err("-1 is not a guest address");
    assert!(matches!(err, QjswasmError::Door(_)), "got {err:?}");
    assert!(
        err.to_string().contains("is not a guest address"),
        "the refusal does not say what was wrong: {err}"
    );
}

/// A pointer inside linear memory whose four-byte length header runs off the
/// end is refused rather than read short.
///
/// 65533 on a one-page memory leaves three bytes, so `at + 4` is the first
/// index past the end. This is the boundary a naive `at < len` check gets
/// wrong.
#[test]
fn a_hostile_string_header_straddling_the_end_of_memory_is_refused() {
    let bytes = hostile_v1(3, 65_533, Some(1), "");
    let mut eng = Engine::new();
    let err = eng
        .run_once(Guest::CompiledQjs(&bytes), None, "main", &[])
        .expect_err("the header does not fit");
    assert!(matches!(err, QjswasmError::Door(_)), "got {err:?}");
    assert!(
        err.to_string()
            .contains("string header at 65533 is out of bounds"),
        "the refusal does not name the address: {err}"
    );
}

/// A well-formed header declaring a body larger than linear memory is refused
/// as a broken guest, and the host does not read past the end to fill it.
///
/// It is `Door` and not `Budget("max_result_string_bytes")` even though the
/// declared length also exceeds that cap, because the two say different things
/// to an embedder: a length that does not fit the guest's own memory is not a
/// number anybody should raise. The bounds check therefore runs first.
#[test]
fn a_hostile_string_body_longer_than_memory_is_refused() {
    // Header at 0: little-endian length 0x00FF_FFFF, far past one page.
    let bytes = hostile_v1(3, 0, Some(1), "\\ff\\ff\\ff\\00");
    let mut eng = Engine::new();
    let err = eng
        .run_once(Guest::CompiledQjs(&bytes), None, "main", &[])
        .expect_err("the body does not fit");
    assert!(matches!(err, QjswasmError::Door(_)), "got {err:?}");
    assert!(
        err.to_string()
            .contains("string body at 0 (len 16777215) is out of bounds"),
        "the refusal does not name the address and length: {err}"
    );
}

/// Bytes that are in bounds but are not UTF-8 are refused, not lossily
/// converted.
///
/// The seam is a value projection, not a diagnostic channel: `agenterm.print`
/// replaces bad bytes with U+FFFD on purpose, because losing a message to one
/// malformed byte is worse than showing it. A *returned value* is the opposite
/// case -- silently substituting a replacement character would hand the caller
/// a string the guest never had.
#[test]
fn a_hostile_string_body_that_is_not_utf8_is_refused() {
    let bytes = hostile_v1(3, 0, Some(1), "\\02\\00\\00\\00\\ff\\fe");
    let mut eng = Engine::new();
    let err = eng
        .run_once(Guest::CompiledQjs(&bytes), None, "main", &[])
        .expect_err("0xff 0xfe is not UTF-8");
    assert!(matches!(err, QjswasmError::Door(_)), "got {err:?}");
    assert!(
        err.to_string().contains("not valid UTF-8"),
        "the refusal does not say what was wrong: {err}"
    );
}

/// A guest that claims to return a string while declaring no linear memory at
/// all is refused, and is told apart from a guest whose pointer is merely out
/// of bounds.
///
/// This attack is what showed the seam was reading through
/// `WasmInstance::memory()`, which substitutes an *empty view* for an absent
/// memory (`wasm.rs`: `.unwrap_or(MemoryView(Empty))`). The no-memory arm was
/// therefore dead, and this case came back as "string header at 0 is out of
/// bounds" -- true of the empty view, and wrong about the guest. Reading
/// through `memory_at(0)`, which reports absence as `None`, separates them.
#[test]
fn a_string_from_a_guest_with_no_linear_memory_is_refused() {
    let bytes = hostile_v1(3, 0, None, "");
    let mut eng = Engine::new();
    let err = eng
        .run_once(Guest::CompiledQjs(&bytes), None, "main", &[])
        .expect_err("there is no memory to read from");
    assert!(matches!(err, QjswasmError::Door(_)), "got {err:?}");
    assert!(
        err.to_string().contains("declares no linear memory"),
        "the refusal does not say what was wrong: {err}"
    );
}

/// A tag the V1 representation does not define is a `Door` refusal naming the
/// convention, not a value guessed from the payload.
#[test]
fn a_hostile_v1_tag_is_refused_without_inventing_a_value() {
    let bytes = hostile_v1(99, 0, Some(1), "");
    let mut eng = Engine::new();
    let err = eng
        .run_once(Guest::CompiledQjs(&bytes), None, "main", &[])
        .expect_err("99 is not a V1 tag");
    assert!(matches!(err, QjswasmError::Door(_)), "got {err:?}");
    assert!(
        err.to_string().contains("unknown tag 99"),
        "the refusal does not name the tag: {err}"
    );
}

// ---------------------------------------------------------------------------
// Supporting measurements cited by the findings above. These pass.
// ---------------------------------------------------------------------------

/// The signature information a typed refusal would need is available, so
/// findings 1, 2 and 8 are actionable rather than aspirational.
///
/// `tinyvm::WasmInstance::exported_function_handle` gives the exact declared
/// type of every parameter and result. A compiled `.qjs` `main` for `$0+$1`
/// comes back as `(i32, i64, i32, i64) -> (i32, i64)`: two wasm words per
/// JavaScript argument, exactly the V1 pair, which is both the arity the seam
/// could check and the fingerprint a `spawn`-time convention check could use.
#[test]
fn the_exact_signature_the_seam_declines_to_consult_is_available() {
    let bytes = wasm("(module (func (export \"id\") (param i32) (result i32) local.get 0))");
    let module =
        tinyvm::WasmModule::from_bytes_with(&bytes, tinyvm::Limits::default()).expect("decodes");
    let instance = module.instantiate().expect("instantiates");
    let handle = instance
        .exported_function_handle("id")
        .ok()
        .flatten()
        .expect("the export has a handle");
    assert_eq!(handle.parameter_count(), 1);
    assert!(
        handle.parameter_type(0) == Some(tinyvm::ValueType::I32),
        "the declared parameter type is not i32"
    );
    assert_eq!(handle.result_count(), 1);

    let compiled = compile_qjs("$0+$1").expect("compiles");
    let module =
        tinyvm::WasmModule::from_bytes_with(&compiled, tinyvm::Limits::default()).expect("decodes");
    let instance = module.instantiate().expect("instantiates");
    let main = instance
        .exported_function_handle("main")
        .ok()
        .flatten()
        .expect("main has a handle");
    let params: Vec<bool> = (0..main.parameter_count())
        .map(|i| main.parameter_type(i) == Some(tinyvm::ValueType::I32))
        .collect();
    let results: Vec<bool> = (0..main.result_count())
        .map(|i| main.result_type(i) == Some(tinyvm::ValueType::I32))
        .collect();
    // (i32, i64) per JavaScript value: two arguments in, one value out.
    assert_eq!(params, vec![true, false, true, false]);
    assert_eq!(results, vec![true, false]);
}

/// The page ceiling *is* classified when it is hit at load time, which is what
/// makes finding 4 a run-time gap specifically rather than a missing feature.
#[test]
fn the_page_ceiling_is_classified_at_load_time() {
    let mut source = String::from("return \"");
    source.push_str(&"x".repeat(70_000));
    source.push_str("\";");
    let budget = Budget {
        limits: tinyvm::Limits {
            max_memory_pages: 1,
            ..tinyvm::Limits::default()
        },
        ..Budget::default()
    };
    let mut eng = Engine::with_budget(budget);
    let err = eng
        .spawn(Guest::Qjs(&source), None)
        .expect_err("a literal pool larger than one page cannot load");
    assert!(
        matches!(err, QjswasmError::Load(_)),
        "expected a load rejection, got {err:?}"
    );
    assert_eq!(eng.live_slots(), 0);
}

/// A JavaScript argument of the wrong *JavaScript* type is the guest's business,
/// and the guest gets it right -- so finding 2 is about the wasm convention
/// only, not about `JsValue` dispatch.
///
/// `ToNumber(undefined)` is `NaN`, `ToNumber(null)` is `+0`, `ToNumber(true)` is
/// `1`. All three arrive correctly through the seam and are coerced by the
/// guest's own runtime, which is exactly right: these are values the language
/// has, not values the face cannot carry.
#[test]
fn a_javascript_argument_of_an_unexpected_type_is_coerced_by_the_guest_not_the_seam() {
    let mut eng = Engine::new();
    let slot = eng.spawn(Guest::Qjs("$0*2"), None).expect("spawn");
    let cases = [
        (JsValue::Undefined, None),
        (JsValue::Null, Some(0.0)),
        (JsValue::Bool(true), Some(2.0)),
        (JsValue::Bool(false), Some(0.0)),
    ];
    for (arg, want) in cases {
        let out = eng
            .call(slot, "main", &[Value::Js(arg.clone())])
            .unwrap_or_else(|e| panic!("{arg:?}: {e}"));
        match (out.values.as_slice(), want) {
            ([Value::Js(JsValue::Number(x))], None) => {
                assert!(x.is_nan(), "{arg:?} should coerce to NaN, got {x}")
            }
            ([Value::Js(JsValue::Number(x))], Some(w)) => assert_eq!(*x, w, "{arg:?}"),
            (other, _) => panic!("{arg:?}: expected one JS number, got {other:?}"),
        }
    }
}
