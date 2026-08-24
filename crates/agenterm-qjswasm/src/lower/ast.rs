//! The syntax tree the parser produces and the lowering consumes.
//!
//! Deliberately a tree and not a byte stream. A single-pass parser that emitted
//! wasm as it went would be shorter today and unusable at M1: constant folding,
//! scope resolution, and control flow all need to look at a node more than
//! once. The tree is the seam that lets those passes exist.
//!
//! M0 nodes carry no source spans. Every diagnostic this milestone can produce
//! is raised during lexing or parsing, where the token offset is in hand; the
//! first lowering-time diagnostic (M1, an unresolved name) is what should add
//! them, rather than carrying unread fields until then.

/// A whole compilation unit. M0 accepts exactly one expression; the struct
/// exists so M1 can grow a statement list without changing the pipeline shape.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Program {
    pub(crate) expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expr {
    /// A signed 32-bit integer. The parser has already applied any leading
    /// minus and checked the range, so lowering cannot see an unrepresentable
    /// literal.
    Int(i32),
    /// `$N` -- the Nth argument of this call.
    Arg(u32),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnaryOp {
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}
