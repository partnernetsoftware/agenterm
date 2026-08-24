//! M0 acceptance for the `.qjs` -> `.wasm` compiler (lane C).
//!
//! Two things are under test, and the second is the one that matters: the
//! numbers a compiled expression produces, and the fact that the bytes
//! producing them clear tinyvm's *load gate*. Passing our own encoder's idea of
//! wasm back through our own encoder would prove nothing, so every fixture goes
//! through `WasmModule::from_bytes_with` + `instantiate` + `invoke_by_name` --
//! the same door a hand-written `.wasm` guest comes through.

use agenterm_qjswasm::compile_qjs;
use tinyvm::{Limits, Val, WasmModule};

/// tinyvm is deliberately fmt-free, so `WasmError` has no `Debug` and
/// `Result::unwrap` will not take it. Unwrap through its `message()` instead.
fn ok<T>(result: Result<T, tinyvm::WasmError>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(e) => panic!("{what}: {}", e.message()),
    }
}

/// Compile, clear the load gate, instantiate, and call `main`. Panics with the
/// diagnostic on a compile failure so a broken fixture reads as itself.
fn eval(source: &str, args: &[i32]) -> i32 {
    let bytes = compile_qjs(source).unwrap_or_else(|e| panic!("compiling {source:?}: {e}"));
    let module = WasmModule::from_bytes_with(&bytes, Limits::default())
        .unwrap_or_else(|e| panic!("tinyvm load gate rejected {source:?}: {}", e.message()));
    let mut instance = module
        .instantiate()
        .unwrap_or_else(|e| panic!("instantiating {source:?}: {}", e.message()));
    let vals: Vec<Val> = args.iter().copied().map(Val::I32).collect();
    let out = instance
        .invoke_by_name("main", &vals)
        .unwrap_or_else(|e| panic!("calling main of {source:?}: {}", e.message()));
    match out.as_slice() {
        [Val::I32(n)] => *n,
        other => panic!(
            "expected one i32 from {source:?}, got {} values",
            other.len()
        ),
    }
}

/// The diagnostic text for a source the M0 subset does not cover.
fn reject(source: &str) -> String {
    match compile_qjs(source) {
        Ok(bytes) => panic!(
            "{source:?} compiled to {} bytes; expected a rejection",
            bytes.len()
        ),
        Err(e) => e.message,
    }
}

/// How many i32 parameters the exported `main` declares.
fn arity(source: &str) -> usize {
    let bytes = compile_qjs(source).unwrap();
    let module = ok(
        WasmModule::from_bytes_with(&bytes, Limits::default()),
        "load gate",
    );
    let instance = ok(module.instantiate(), "instantiate");
    ok(instance.exported_function_handle("main"), "main handle")
        .expect("main is exported")
        .parameter_count()
}

// -- arithmetic --------------------------------------------------------------

#[test]
fn integer_literals_and_addition() {
    assert_eq!(eval("1+2", &[]), 3);
    assert_eq!(eval("0", &[]), 0);
    assert_eq!(eval("2147483647", &[]), i32::MAX);
}

#[test]
fn arguments_are_i32_parameters() {
    assert_eq!(eval("$0*2", &[21]), 42);
    assert_eq!(eval("$0+$1", &[40, 2]), 42);
}

#[test]
fn parameter_count_is_the_highest_argument_index_plus_one() {
    assert_eq!(arity("1+2"), 0);
    assert_eq!(arity("$0"), 1);
    assert_eq!(arity("$2"), 3);
    assert_eq!(arity("$1+$1"), 2);
    assert_eq!(eval("$2", &[7, 8, 9]), 9);
}

#[test]
fn multiplicative_binds_tighter_than_additive() {
    assert_eq!(eval("1+2*3", &[]), 7);
    assert_eq!(eval("2*3+4*5", &[]), 26);
    assert_eq!(eval("10-6/2", &[]), 7);
    assert_eq!(eval("1+7%4", &[]), 4);
}

#[test]
fn binary_operators_are_left_associative() {
    assert_eq!(eval("10-3-4", &[]), 3);
    assert_eq!(eval("100/5/2", &[]), 10);
    assert_eq!(eval("20-1+2", &[]), 21);
    assert_eq!(eval("7%5%3", &[]), 2);
}

#[test]
fn parentheses_override_precedence() {
    assert_eq!(eval("(1+2)*3", &[]), 9);
    assert_eq!(eval("10-(3-4)", &[]), 11);
    assert_eq!(eval("((((7))))", &[]), 7);
    assert_eq!(eval("2*(3+(4*5))", &[]), 46);
}

#[test]
fn unary_minus() {
    assert_eq!(eval("-5", &[]), -5);
    assert_eq!(eval("-5+3", &[]), -2);
    assert_eq!(eval("- -5", &[]), 5);
    assert_eq!(eval("-(2+3)*4", &[]), -20);
    assert_eq!(eval("3*-4", &[]), -12);
    assert_eq!(eval("-$0", &[9]), -9);
    // The most negative i32 has no positive counterpart, so it is only
    // reachable through the unary minus. It must still be reachable.
    assert_eq!(eval("-2147483648", &[]), i32::MIN);
}

#[test]
fn truncating_integer_division() {
    // Not JS division: see `div_and_rem_by_zero_trap`. `/` is `i32.div_s`,
    // which truncates toward zero.
    assert_eq!(eval("7/2", &[]), 3);
    assert_eq!(eval("-7/2", &[]), -3);
    assert_eq!(eval("-7%2", &[]), -1);
}

#[test]
fn whitespace_and_comments_are_not_significant() {
    assert_eq!(eval("  1  +  2  ", &[]), 3);
    assert_eq!(eval("1 + /* two */ 2", &[]), 3);
    assert_eq!(eval("1 + 2 // trailing\n", &[]), 3);
    assert_eq!(eval("1+2;", &[]), 3);
}

// -- the capability boundary -------------------------------------------------
//
// Every rejection must say what the *engine* cannot do yet, never that the
// script is wrong. These assertions are the lock on that wording (PRD 36).

/// Every diagnostic for an out-of-subset construct is phrased as a capability
/// boundary and names the construct.
fn assert_capability_boundary(source: &str, names: &str) {
    let message = reject(source);
    assert!(
        message.starts_with("this engine does not support "),
        "{source:?} gave {message:?}, which does not name an engine capability boundary"
    );
    assert!(
        message.ends_with(" yet"),
        "{source:?} gave {message:?}, which does not read as a boundary that moves"
    );
    assert!(
        message.contains(names),
        "{source:?} gave {message:?}, which does not name {names:?}"
    );
}

#[test]
fn function_declarations_are_not_supported_yet() {
    assert_capability_boundary("function f(){}", "function");
}

#[test]
fn string_literals_are_not_supported_yet() {
    assert_capability_boundary("\"hello\"", "string literal");
    assert_capability_boundary("1 + 'x'", "string literal");
    assert_capability_boundary("`t`", "template literal");
}

#[test]
fn declarations_are_not_supported_yet() {
    assert_capability_boundary("let x = 1", "let");
    assert_capability_boundary("const x = 1", "const");
    assert_capability_boundary("var x = 1", "var");
}

#[test]
fn identifiers_are_not_supported_yet() {
    assert_capability_boundary("x + 1", "variable");
    assert_capability_boundary("foo()", "variable");
}

#[test]
fn non_integer_numbers_are_not_supported_yet() {
    assert_capability_boundary("1.5", "fractional");
    assert_capability_boundary("1e3", "exponent");
    assert_capability_boundary("0x10", "hexadecimal");
    assert_capability_boundary("1n", "BigInt");
}

#[test]
fn wider_syntax_is_not_supported_yet() {
    assert_capability_boundary("[1,2]", "array");
    assert_capability_boundary("{}", "block");
    assert_capability_boundary("1 < 2", "comparison");
    assert_capability_boundary("1 & 2", "bitwise");
    assert_capability_boundary("1 ? 2 : 3", "conditional");
    assert_capability_boundary("1; 2", "statement");
    assert_capability_boundary("2 ** 3", "exponentiation");
}

#[test]
fn integers_outside_the_signed_32_bit_range_are_not_supported_yet() {
    assert_capability_boundary("2147483648", "32-bit");
    assert_capability_boundary("-2147483649", "32-bit");
    assert_capability_boundary("99999999999999999999999", "32-bit");
}

#[test]
fn an_incomplete_expression_says_what_is_missing_without_blaming_the_script() {
    for source in ["", "   ", "1 +", "(1+2", "-"] {
        let message = reject(source);
        let lowered = message.to_lowercase();
        assert!(
            !lowered.contains("syntax error") && !lowered.contains("invalid"),
            "{source:?} gave {message:?}, which is the vague wording PRD 36 forbids"
        );
        assert!(
            message.starts_with("this engine "),
            "{source:?} gave {message:?}, which does not speak for the engine"
        );
    }
}

#[test]
fn a_diagnostic_points_at_the_construct_it_names() {
    let e = compile_qjs("1 + \"two\"").unwrap_err();
    assert_eq!(e.offset, 4, "offset should be the opening quote");
    assert!(e.to_string().contains("at byte 4"), "{e}");
}

// -- the divergence we chose -------------------------------------------------

#[test]
fn div_and_rem_by_zero_trap() {
    // DOCUMENTED JS DIVERGENCE, locked here on purpose.
    //
    // JavaScript has one number type, so `1/0` is `Infinity` and `1%0` is
    // `NaN`. M0 has only i32, so neither value exists to return. The engine
    // therefore keeps wasm's `i32.div_s`/`i32.rem_s` behaviour: the call traps.
    // Revisit when floats land (M3) -- see the comment in `src/lower/emit.rs`.
    for source in ["1/0", "1%0", "$0/0"] {
        let bytes =
            compile_qjs(source).expect("division by zero is a runtime matter, not a compile one");
        let module = ok(
            WasmModule::from_bytes_with(&bytes, Limits::default()),
            "the module itself should be well formed",
        );
        let mut instance = ok(module.instantiate(), "instantiate");
        let args = if source.contains("$0") {
            vec![Val::I32(1)]
        } else {
            vec![]
        };
        assert!(
            instance.invoke_by_name("main", &args).is_err(),
            "{source:?} should trap, not return a fabricated number"
        );
    }
}

#[test]
fn signed_division_overflow_traps() {
    // Same bucket: JS `-2147483648 / -1` is 2147483648, which is not an i32.
    // `i32.div_s` traps instead of wrapping to i32::MIN.
    let bytes = compile_qjs("-2147483648 / -1").unwrap();
    let module = ok(
        WasmModule::from_bytes_with(&bytes, Limits::default()),
        "load gate",
    );
    let mut instance = ok(module.instantiate(), "instantiate");
    assert!(instance.invoke_by_name("main", &[]).is_err());
}

// -- the bytes themselves ----------------------------------------------------

#[test]
fn output_is_a_standard_wasm_module() {
    let bytes = compile_qjs("$0+1").unwrap();
    assert_eq!(&bytes[..8], b"\0asm\x01\0\0\0", "magic and version");
}

#[test]
fn every_fixture_clears_the_tinyvm_load_gate() {
    let sources = [
        "0",
        "1+2",
        "-2147483648",
        "$0",
        "$7",
        "$0*($1+$2)-$3%$4/$5",
        "((1+2)*(3-4))/5%6",
        "-(-(-1))",
    ];
    for source in sources {
        let bytes = compile_qjs(source).unwrap_or_else(|e| panic!("{source:?}: {e}"));
        WasmModule::from_bytes_with(&bytes, Limits::default())
            .unwrap_or_else(|e| panic!("{source:?} rejected at the load gate: {}", e.message()));
    }
}

#[test]
fn encoding_is_byte_identical_to_a_reference_assembler() {
    // `wat` is a dev-dependency only. This is the cross-check that our
    // hand-written section/LEB128/opcode encoder produces the canonical shape
    // rather than merely a shape tinyvm happens to accept.
    let ours = compile_qjs("$0*2").unwrap();
    let theirs = wat::parse_str(
        r#"(module
             (type (func (param i32) (result i32)))
             (func (type 0) (param i32) (result i32)
               local.get 0
               i32.const 2
               i32.mul)
             (export "main" (func 0)))"#,
    )
    .unwrap();
    assert_eq!(
        ours, theirs,
        "our encoder diverged from the reference assembler"
    );
}
