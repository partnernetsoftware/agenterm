//! Executable checks for the product-facing syntax claims in `README.md`.
//!
//! The compiler is the authority. This court deliberately enters through
//! `agenterm_qjswasm::compile_qjs`, the same product compile surface used by
//! `check`, and then pins the README wording so documentation cannot silently
//! lag a compiler-pin advance again.

use agenterm_qjswasm::compile_qjs;

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
