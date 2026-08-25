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
//!
//! Two of those seam facts are new as of the `df8decd` bump and are what most
//! of this file is now about:
//!
//! * **The calling convention changed.** A compiled entry point speaks the V1
//!   representation -- one JavaScript value is a `(tag: i32, payload: i64)`
//!   pair -- so a `.qjs` slot takes and returns [`JsValue`], not raw wasm
//!   numerics. Which convention a slot speaks is settled at load time.
//! * **A returned value is projected, not forwarded.** A String's payload is a
//!   pointer into the slot's linear memory, and `run_once` drops that memory
//!   before its caller sees the result, so the text is read out at the seam.
//!
//! The small corpus in [`the_capability_claims_in_this_crates_own_copy`] is the
//! exception to "language tests live upstream", and deliberately so: this
//! crate's README and PRD 36 make capability claims in agenterm's voice, and
//! the evidence gate says product copy is locked by a test rather than by a
//! reading of someone else's source.

use agenterm_qjswasm::{
    Budget, Engine, Guest, GuestKind, JsValue, QjswasmError, Value, compile_qjs,
    guest_kind_for_path, validate_wasm,
};

fn engine() -> Engine {
    Engine::new()
}

fn number(x: f64) -> Value {
    Value::Js(JsValue::Number(x))
}

/// Run a whole `.qjs` script with no arguments and hand back the one JS value
/// it evaluated to.
#[track_caller]
fn returns(source: &str) -> JsValue {
    let mut eng = engine();
    let out = eng
        .run_once(Guest::Qjs(source), None, "main", &[])
        .unwrap_or_else(|e| panic!("{source:?}: {e}"));
    match out.values.as_slice() {
        [Value::Js(value)] => value.clone(),
        other => panic!("{source:?}: a `.qjs` guest must return one JS value, got {other:?}"),
    }
}

#[test]
fn a_qjs_guest_runs_end_to_end_through_a_slot() {
    let mut eng = engine();
    let out = eng
        .run_once(Guest::Qjs("$0*2+2"), None, "main", &[number(20.0)])
        .expect("a `.qjs` guest must compile, load and run");
    assert_eq!(out.values, vec![number(42.0)]);
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
    for (a, b, want) in [(1.0, 2.0, 3.0), (40.0, 2.0, 42.0), (-5.0, 5.0, 0.0)] {
        let out = eng
            .call(slot, "main", &[number(a), number(b)])
            .expect("call");
        assert_eq!(out.values, vec![number(want)]);
    }
    assert_eq!(eng.live_slots(), 1);
    eng.kill(slot);
    assert_eq!(eng.live_slots(), 0);
}

/// Every ECMA-262 language type the compiler has, crossing the face as itself.
///
/// This is the assertion the M0 seam could not make: with an `i32`-in/`i32`-out
/// entry point there was exactly one thing a `.qjs` guest could return, so the
/// face had nothing to get wrong. Now a guest returns one of five kinds and the
/// projection has to name which -- collapsing them back to a number would throw
/// away the milestone this bump is here to pick up.
#[test]
fn every_kind_of_javascript_value_crosses_the_face() {
    assert_eq!(returns("return 42;"), JsValue::Number(42.0));
    assert_eq!(returns("return \"hello\";"), JsValue::Str("hello".into()));
    assert_eq!(returns("return true;"), JsValue::Bool(true));
    assert_eq!(returns("return false;"), JsValue::Bool(false));
    assert_eq!(returns("return null;"), JsValue::Null);
    assert_eq!(returns("return undefined;"), JsValue::Undefined);
    // A script with no `return` evaluates to its completion value, which is a
    // value like any other and must arrive as one.
    assert_eq!(returns("1 + 1"), JsValue::Number(2.0));
    assert_eq!(returns("let x = 1;"), JsValue::Undefined);
}

/// A returned String is text, read out of the guest's memory before the slot
/// dies.
///
/// The V1 payload for a String is a pointer into *that instance's* linear
/// memory, and [`Engine::run_once`] kills the slot before returning. Forwarding
/// the pointer would therefore hand every `run_once` caller a dangling
/// reference -- not occasionally, but on the common path. So the seam resolves
/// it while the instance is alive, and this test is what says so: the value
/// below is read after the memory it came from has been dropped.
#[test]
fn a_returned_string_is_text_and_outlives_the_slot_it_came_from() {
    let mut eng = engine();
    let out = eng
        .run_once(
            Guest::Qjs("return \"tab\" + \"s.list\";"),
            None,
            "main",
            &[],
        )
        .expect("string concatenation runs");
    assert_eq!(eng.live_slots(), 0, "run_once must not leave a slot behind");
    assert_eq!(
        out.values,
        vec![Value::Js(JsValue::Str("tabs.list".into()))]
    );
    // Escapes and non-ASCII are decoded by the compiler and must survive the
    // trip through linear memory as UTF-8, not as bytes.
    assert_eq!(
        returns("return \"caf\\u00e9\";"),
        JsValue::Str("café".into())
    );
}

/// A source the compiler will not lower is [`QjswasmError::Compile`] -- not a
/// load rejection, and not a trap.
///
/// Five failure classes exist so a caller can tell them apart without matching
/// on strings, and this is the one boundary where the distinction is easiest to
/// lose: everything downstream of the compiler also reports failures about
/// bytes, so a compile error that arrived as `Load` would look plausible and be
/// wrong about who to talk to.
///
/// The sources are chosen to still be outside the subset, and each is measured
/// rather than read off upstream's source. This test is the lock on the
/// "honest boundary" copy in this crate's README and in PRD 36 -- if the subset
/// grows past one of these, the copy has to be rewritten in the same commit
/// that makes it stale.
///
/// It has now caught that once, which is what it is for. The rev bump to
/// `6920c60` -- taken for `Names::Declared`, the mechanism that lets a `.qjs`
/// script reach the door -- also brought upstream's `%` (dd35c44) and `typeof`
/// (c707558), and the two of them were this list's first two entries. Both
/// measured: `return 1 % 2;` is `Number(1.0)` and `return typeof 1;` is
/// `Str("number")`. They are replaced below with constructs measured to be
/// outside the subset at that same rev, and the README's refusal list was
/// corrected in the same commit.
#[test]
fn a_source_outside_the_subset_is_a_compile_error_not_a_load_error() {
    for source in [
        "return 1 ? 2 : 3;",
        "return {};",
        "return `x`;",
        "function outer() { let a = 1; function inner() { return a; } return inner(); }",
    ] {
        let mut eng = engine();
        let err = eng
            .spawn(Guest::Qjs(source), None)
            .expect_err("this source is not in the subset yet");
        match &err {
            QjswasmError::Compile(e) => {
                // The diagnostic must survive the trip: it speaks for the
                // engine, and it says where. Its exact wording is the
                // compiler's contract and is locked upstream.
                assert!(
                    e.message.starts_with("this engine "),
                    "diagnostic blames the script: {e}"
                );
                assert!(e.to_string().contains("at byte"), "{e}");
            }
            other => panic!("{source:?}: expected a compile error, got {other:?}"),
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
}

/// The claims this crate's own README and PRD 36 make about `.qjs`, executed.
///
/// Not a language suite -- that is upstream's, next to the code. This is the
/// documentation lock the evidence gate asks for: every capability agenterm
/// advertises in its own voice is a line here that runs, so the copy cannot
/// drift ahead of the engine the way "M0, integer expressions only" drifted
/// behind it.
#[test]
fn the_capability_claims_in_this_crates_own_copy() {
    // Declarations with real scoping, and a block that does not leak.
    assert_eq!(
        returns("const y = 2; { let y = 3; } return y;"),
        JsValue::Number(2.0)
    );
    assert_eq!(returns("var z = 9; return z;"), JsValue::Number(9.0));
    assert_eq!(returns("let u; u = 5; return u;"), JsValue::Number(5.0));
    // Control flow.
    assert_eq!(
        returns("if (1 < 2) { return 10; } else { return 20; }"),
        JsValue::Number(10.0)
    );
    assert_eq!(
        returns("let i = 0; while (i < 5) { i = i + 1; } return i;"),
        JsValue::Number(5.0)
    );
    assert_eq!(
        returns("let s = 0; for (let k = 0; k < 4; k = k + 1) { s = s + k; } return s;"),
        JsValue::Number(6.0)
    );
    // Functions with parameters, and recursion.
    assert_eq!(
        returns("function add(a, b) { return a + b; } return add(2, 40);"),
        JsValue::Number(42.0)
    );
    assert_eq!(
        returns("function f(n) { if (n < 2) { return n; } return f(n-1) + f(n-2); } return f(10);"),
        JsValue::Number(55.0)
    );
    // The operator ladder, including short-circuit and compound assignment.
    assert_eq!(
        returns("return 1 < 2 && 3 >= 3 || false;"),
        JsValue::Bool(true)
    );
    assert_eq!(
        returns("return true && \"yes\";"),
        JsValue::Str("yes".into())
    );
    assert_eq!(
        returns("let n = 1; n += 2; n *= 3; return n;"),
        JsValue::Number(9.0)
    );
    assert_eq!(
        returns("let m = 0; return m++ + ++m;"),
        JsValue::Number(2.0)
    );
    // ASI: no semicolon anywhere, and the same answer.
    assert_eq!(
        returns("let a = 1\nlet b = 2\nreturn a + b"),
        JsValue::Number(3.0)
    );
}

/// Numbers are ECMA-262 binary64, and the seam reports them as such.
///
/// This assertion is the *opposite* of the one M0 shipped here, which asserted
/// that `$0/0` trapped. That was true of an `i32` division and is now wrong:
/// 6.1.6.1 says a Number is an IEEE-754 double, so `1/0` is `Infinity`, `0/0`
/// is `NaN`, and `2147483647 + 1` does not wrap. This is a deliberate
/// correctness improvement, not a loosened assertion -- the old test locked a
/// limitation, and the limitation is gone.
#[test]
fn arithmetic_is_binary64_so_division_by_zero_is_infinity() {
    let mut eng = engine();
    let out = eng
        .run_once(Guest::Qjs("$0/0"), None, "main", &[number(1.0)])
        .expect("dividing by zero is a Number, not a fault");
    assert_eq!(out.values, vec![number(f64::INFINITY)]);

    // No NaN equals itself, so this one is asserted by predicate.
    assert!(
        matches!(returns("return 0/0;"), JsValue::Number(x) if x.is_nan()),
        "0/0 must be NaN"
    );
    assert_eq!(
        returns("return 2147483647 + 1;"),
        JsValue::Number(2147483648.0)
    );
    // `-x` keeps the sign of a zero, which an integer engine cannot express.
    assert_eq!(
        returns("let z = 0; return 1 / -z;"),
        JsValue::Number(f64::NEG_INFINITY)
    );
}

/// A `.qjs` guest that fails at *run* time is a trap, not a compile error.
///
/// The other side of the same boundary: reporting a run-time fault as a compile
/// failure would tell the author to fix their syntax.
///
/// The source is a String/Number coercion, which upstream lowers to a trap
/// rather than fabricating a value -- ToString of a Number, StringToNumber and
/// String relational comparison are the three ECMA-262 conversions the runtime
/// does not have yet, and it refuses rather than guessing. That is a recorded
/// divergence with a milestone on it, so when those land this test needs a
/// different trapping source; what it is protecting is the classification, not
/// the divergence.
#[test]
fn a_runtime_fault_in_a_compiled_guest_is_a_trap() {
    let mut eng = engine();
    let err = eng
        .run_once(Guest::Qjs("return \"2\" * 2;"), None, "main", &[])
        .expect_err("an unimplemented conversion traps rather than guessing");
    assert!(
        matches!(err, QjswasmError::Trap(_)),
        "expected a trap from a compiled guest, got {err:?}"
    );
}

/// A slot accepts values in the convention it was loaded under, and refuses the
/// other one rather than reinterpreting the bits.
///
/// Both directions are the same mistake seen from either side, and neither is
/// the guest's fault -- which is why both are `UnsupportedValue` and not
/// `Trap`. Without this, a `Value::I32(20)` handed to a `.qjs` entry point
/// would be a wasm arity mismatch reported as a trap, blaming a guest that did
/// nothing wrong.
#[test]
fn a_value_offered_in_the_wrong_convention_is_refused_at_the_face() {
    let mut eng = engine();
    let qjs = eng.spawn(Guest::Qjs("$0+1"), None).expect("spawn");
    let err = eng
        .call(qjs, "main", &[Value::I32(20)])
        .expect_err("a `.qjs` entry point does not take raw wasm numerics");
    assert!(
        matches!(err, QjswasmError::UnsupportedValue(_)),
        "got {err:?}"
    );

    let bytes =
        wat::parse_str("(module (func (export \"id\") (param i32) (result i32) local.get 0))")
            .expect("valid wat");
    let hand_written = eng.spawn(Guest::Wasm(&bytes), None).expect("spawn");
    let err = eng
        .call(hand_written, "id", &[number(1.0)])
        .expect_err("a hand-written module does not take JavaScript values");
    assert!(
        matches!(err, QjswasmError::UnsupportedValue(_)),
        "got {err:?}"
    );

    // A String *argument* is the one JS value the face cannot hand in: it would
    // have to be allocated in the guest's heap, and there is no door onto that
    // allocator. Refused for the same reason and in the same class -- never
    // faked with a pointer that means nothing.
    let err = eng
        .call(qjs, "main", &[Value::Js(JsValue::Str("x".into()))])
        .expect_err("a string argument has nowhere to live in the guest yet");
    assert!(
        matches!(err, QjswasmError::UnsupportedValue(_)),
        "got {err:?}"
    );
}

/// What the compiler emits clears *this* crate's load gate, under this crate's
/// budget -- not merely tinyvm's defaults.
///
/// `Budget` is agenterm's policy and can be tightened independently of the
/// compiler. A guest that only ever loads under `Limits::default()` would be
/// evidence about upstream's dials, not about the ones this engine ships.
///
/// This got sharper with the bump: a compiled module now declares a linear
/// memory (the string literal pool and the bump allocator live in it) and
/// carries an emitted runtime, so "does the product of the compiler fit through
/// the host's limits?" is a real question here for the first time.
#[test]
fn compiled_bytes_clear_this_crates_load_gate() {
    for source in [
        "0",
        "$0*($1+$2)-$3/$4",
        "-(-(-1))",
        "((1+2)*(3-4))/5",
        "return \"a string literal that occupies the pool\";",
        "function f(n) { if (n < 2) { return n; } return f(n-1); } return f(3);",
    ] {
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
    for source in ["$0+1", "return \"s\";"] {
        let bytes = compile_qjs(source).unwrap();
        agenterm_qjswasm::validate_wasm_with(&bytes, &tight)
            .unwrap_or_else(|e| panic!("{source:?} needs more than one page and no table: {e}"));
    }
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

/// Compiling to bytes and loading those bytes back gives the *same* value as
/// running the source directly.
///
/// This is the acceptance criterion the `agenterm-qjs` CLI archive gate names
/// as its step zero (`plan/design-qjs-archive-gate.md` 4.3 and 9). Every
/// compile-once-run-later verb -- `pack`, a build cache, an artifact fetched
/// from anywhere -- is built on it, and before `Guest::CompiledQjs` existed it
/// was false: `Guest::Wasm(&compile_qjs(src))` loaded the same bytes under the
/// wasm convention, so `42` came back as the raw V1 pair `[I32(1),
/// I64(4631107791820423168)]` instead of `Number(42.0)`, and a string came back
/// as a pointer into a linear memory the caller was about to drop.
///
/// A `.wasm` file does not remember where it came from. The convention has to
/// be carried by whoever loads it, which is what the variant is for -- and the
/// third leg below is why: the same bytes named as plain wasm still speak the
/// pair, deliberately, so this is a choice the caller makes rather than a
/// property the engine guesses at.
#[test]
fn a_compiled_artifact_reloaded_gives_the_same_value_as_its_source() {
    for (source, want) in [
        ("return 42;", Value::Js(JsValue::Number(42.0))),
        ("return \"hello\";", Value::Js(JsValue::Str("hello".into()))),
        ("return true;", Value::Js(JsValue::Bool(true))),
        ("return null;", Value::Js(JsValue::Null)),
        ("let unused = 1;", Value::Js(JsValue::Undefined)),
    ] {
        let bytes = compile_qjs(source).unwrap_or_else(|e| panic!("{source:?}: {e}"));

        let direct = engine()
            .run_once(Guest::Qjs(source), None, "main", &[])
            .unwrap_or_else(|e| panic!("{source:?} direct: {e}"));
        let reloaded = engine()
            .run_once(Guest::CompiledQjs(&bytes), None, "main", &[])
            .unwrap_or_else(|e| panic!("{source:?} reloaded: {e}"));

        assert_eq!(direct.values, vec![want.clone()], "{source:?} direct");
        assert_eq!(reloaded.values, direct.values, "{source:?} reloaded");
    }

    // The same bytes under the wasm convention are a different answer, on
    // purpose: `Convention` is recorded, never inferred. If this ever starts
    // matching the two above, the engine has begun guessing at signatures.
    let bytes = compile_qjs("return 42;").expect("compiles");
    let raw = engine()
        .run_once(Guest::Wasm(&bytes), None, "main", &[])
        .expect("compiled bytes are ordinary wasm too");
    assert_eq!(
        raw.values,
        vec![Value::I32(1), Value::I64(42.0f64.to_bits() as i64)],
        "loading compiled bytes as plain wasm must still hand over the raw V1 pair"
    );
}
