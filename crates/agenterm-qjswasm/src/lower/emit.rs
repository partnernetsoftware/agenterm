//! [`ast::Program`] -> [`ir::Module`].
//!
//! The whole program becomes one exported function named `main`, taking as many
//! `i32` parameters as the highest `$N` the source names, and returning `i32`.
//! Nothing here can fail: the parser has already rejected everything M0 cannot
//! lower, and it rejected it with a diagnostic that named the boundary. Once
//! lowering *can* fail (M1, an unresolved name), this returns a `Result` and
//! the AST grows the spans that diagnostic needs.
//!
//! # A JavaScript divergence we chose, on purpose
//!
//! `/` and `%` lower to `i32.div_s` and `i32.rem_s`, which **trap** when the
//! divisor is zero, and `i32.div_s` also traps on `i32::MIN / -1`. JavaScript
//! has one number type, so `1/0` is `Infinity`, `1%0` is `NaN`, and
//! `-2147483648 / -1` is `2147483648`. M0 has only `i32`: none of those three
//! values exists here, so there is no way to be right, only ways to be wrong.
//!
//! The alternative was to guard every division and yield `0`. That was
//! rejected. A fabricated `0` is a wrong number that flows on into the rest of
//! the expression and is indistinguishable from a real result, which is exactly
//! the silent corruption this whole stack refuses; a trap is loud, arrives as a
//! typed `QjswasmError::Trap`, and costs nothing per division. It is also the
//! divergence that is cheapest to retire: when floats land (M3) these three
//! cases start producing `Infinity`/`NaN`/`2147483648`, and no script can have
//! come to depend on `0` in the meantime, because every such script failed
//! loudly instead of quietly computing the wrong answer.
//!
//! Locked by `div_and_rem_by_zero_trap` and `signed_division_overflow_traps`
//! in `tests/qjs_m0.rs`.

use super::ast::{BinaryOp, Expr, Program, UnaryOp};
use super::ir::{Export, ExportKind, Func, FuncType, Ins, Module, ValType};

/// The name every compiled `.qjs` exports. One entry point, fixed, so the host
/// never has to guess what to call.
pub(crate) const ENTRY: &str = "main";

pub(crate) fn lower(program: &Program) -> Module {
    let arity = argument_count(&program.expr);
    let mut body = Vec::new();
    emit(&program.expr, &mut body);

    Module {
        types: vec![FuncType {
            params: vec![ValType::I32; arity as usize],
            results: vec![ValType::I32],
        }],
        funcs: vec![Func {
            type_index: 0,
            locals: Vec::new(),
            body,
        }],
        exports: vec![Export {
            name: ENTRY.to_string(),
            kind: ExportKind::Func,
            index: 0,
        }],
    }
}

/// How many parameters `main` declares: one past the highest `$N` used, so
/// `$2` alone still means three, and a source naming no argument means zero.
fn argument_count(expr: &Expr) -> u32 {
    match expr {
        Expr::Int(_) => 0,
        Expr::Arg(index) => index + 1,
        Expr::Unary(_, operand) => argument_count(operand),
        Expr::Binary(_, lhs, rhs) => argument_count(lhs).max(argument_count(rhs)),
    }
}

/// Post-order emission: operands first, then the operator. That is the wasm
/// stack discipline, and it is why the tree needs no register allocation.
fn emit(expr: &Expr, out: &mut Vec<Ins>) {
    match expr {
        Expr::Int(value) => out.push(Ins::I32Const(*value)),
        // Parameters occupy the first local indices, in order, so `$N` is
        // local `N` with nothing to look up.
        Expr::Arg(index) => out.push(Ins::LocalGet(*index)),
        // wasm has no integer negation instruction. `0 - x` is the standard
        // encoding and is what a wasm producer would emit.
        Expr::Unary(UnaryOp::Neg, operand) => {
            out.push(Ins::I32Const(0));
            emit(operand, out);
            out.push(Ins::I32Sub);
        }
        Expr::Binary(op, lhs, rhs) => {
            emit(lhs, out);
            emit(rhs, out);
            out.push(match op {
                BinaryOp::Add => Ins::I32Add,
                BinaryOp::Sub => Ins::I32Sub,
                BinaryOp::Mul => Ins::I32Mul,
                BinaryOp::Div => Ins::I32DivS,
                BinaryOp::Rem => Ins::I32RemS,
            });
        }
    }
}
