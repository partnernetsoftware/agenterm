//! Executable checks for the product-facing syntax claims in `README.md`.
//!
//! The compiler is the authority. This court deliberately enters through
//! `agenterm_qjswasm::compile_qjs`, the same product compile surface used by
//! `check`, and then pins the README wording so documentation cannot silently
//! lag a compiler-pin advance again.

use agenterm_qjswasm::{compile_qjs, compile_qjs_with_modules};

const README: &str = include_str!("../README.md");

fn rejection_section() -> &'static str {
    README
        .split_once("**明确拒绝**")
        .expect("README keeps the syntax rejection heading")
        .1
        .split_once("**运行期缺口")
        .expect("README keeps the runtime-gap heading after syntax rejections")
        .0
}

#[test]
fn break_and_continue_compile_through_the_product_entry() {
    let source = r#"
let seen = 0;
for (let index = 0; index < 5; index = index + 1) {
  if (index === 1) { continue; }
  seen = seen + 1;
  if (seen === 3) { break; }
}
return seen;
"#;
    let wasm = compile_qjs(source).expect("break and continue inside a loop compile");
    assert_eq!(&wasm[..4], b"\0asm");
    assert!(
        !rejection_section().contains("`break`/`continue`"),
        "README must not list syntax accepted by the product compiler as rejected"
    );
}

#[test]
fn nullish_coalescing_compiles_and_is_not_listed_as_rejected() {
    compile_qjs("return null ?? 1;")
        .expect("nullish coalescing compiles through the product entry");
    assert!(
        !rejection_section().contains("`??`"),
        "README must not list nullish coalescing as rejected"
    );
}

#[test]
fn numeric_separators_remain_a_named_and_documented_rejection() {
    let error = compile_qjs("return 1_000;")
        .expect_err("numeric separators remain outside the current product subset");
    assert!(
        error.to_string().contains("numeric separators"),
        "the compile refusal must name the unsupported literal form: {error}"
    );
    assert!(
        rejection_section().contains("数字分隔符（`1_000`）"),
        "README explicit rejection summary must include the rejected separator form"
    );
}

#[test]
fn namespace_imports_compile_and_are_not_disclaimed_as_a_whole() {
    let resolve = |specifier: &str| match specifier {
        "lib/value" => Some("export const answer = 42;".to_owned()),
        _ => None,
    };
    compile_qjs_with_modules(
        "import * as value from \"lib/value\"; return value.answer;",
        &resolve,
    )
    .expect("namespace import compiles through the product module entry");
    for source in [
        "import value from \"lib/value\"; return value;",
        "import { answer } from \"lib/value\"; return answer;",
        "return import(\"lib/value\");",
    ] {
        let error = compile_qjs_with_modules(source, &resolve)
            .expect_err("the documented import form remains unsupported");
        assert!(
            error.to_string().contains("import"),
            "unsupported import form must name its boundary: {error}"
        );
    }

    let documented = rejection_section();
    assert!(
        documented.contains("default / named / dynamic"),
        "README must narrow the import refusal to the forms the compiler lacks"
    );
    assert!(
        documented.contains("`import * as` 已在支持表"),
        "README must not disclaim the namespace-import form the product resolver uses"
    );
}

#[test]
fn representative_unimplemented_statements_remain_named_rejections() {
    for (name, keyword, source) in [
        ("class", "class", "class Example {} return 0;"),
        (
            "switch",
            "switch",
            "switch (1) { case 1: return 1; } return 0;",
        ),
        (
            "for-in",
            "in",
            "for (let key in {a: 1}) { return key; } return '';",
        ),
        (
            "do-while",
            "do",
            "let n = 0; do { n = n + 1; } while (n < 2); return n;",
        ),
    ] {
        let error = compile_qjs(source).expect_err("documented syntax remains rejected");
        let diagnostic = error.to_string().to_ascii_lowercase();
        assert!(
            diagnostic.contains("does not support") && diagnostic.contains(keyword),
            "{name} must fail with its named unsupported diagnostic, got: {error}"
        );
    }

    let documented = rejection_section();
    for spelling in ["`class`", "`switch`", "`for…in`", "`do`/`while`"] {
        assert!(
            documented.contains(spelling),
            "README rejection table lost compiler-owned spelling {spelling}"
        );
    }
}

#[test]
fn loop_control_outside_a_loop_is_still_rejected_by_context() {
    for source in ["break;", "continue;"] {
        let error = compile_qjs(source).expect_err("loop control needs an enclosing loop");
        let diagnostic = error.to_string().to_ascii_lowercase();
        assert!(
            diagnostic.contains("loop") || diagnostic.contains("outside"),
            "context refusal must name the missing loop, got: {error}"
        );
    }
}

#[test]
fn the_current_pin_records_the_missing_for_of_binding_misreport() {
    compile_qjs("for (const value of [1, 2]) { print(value); }")
        .expect("the product compiler supports declaration-form for-of");

    let error = compile_qjs("for (const of values) { }")
        .expect_err("a declaration-form for-of header needs a binding name");
    assert_eq!(
        error.to_string(),
        "this engine does not support the `of` keyword yet (at byte 11)",
        "when the upstream diagnostic is repaired, retire this known-misreport pin and update README"
    );
    assert!(
        rejection_section().contains("`for (const of values) { }`"),
        "README must disclose the current pin's false capability attribution"
    );
}

#[test]
fn the_current_pin_records_the_const_for_in_initializer_misreport() {
    let error = compile_qjs("for (const key in {alpha: 1}) { print(key); }")
        .expect_err("for-in remains unsupported at the current pin");
    assert_eq!(
        error.to_string(),
        "this engine needs a value for the `const` binding `key`; a `const` can never be assigned one later (at byte 11)",
        "when the upstream diagnostic reaches the unsupported in keyword, retire this misreport pin and update README"
    );
    let documented = rejection_section();
    assert!(
        documented.contains("`for…in`") && documented.contains("`9ac2598` 产品入口复测仍拒绝"),
        "README must bind the current for-in refusal to the revision actually exercised"
    );
}
