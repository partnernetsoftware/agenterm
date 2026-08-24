//! The tree both variants share.
//!
//! Shared is the point: experiment constraint 1 says one implementation
//! packaged twice, with only the value-representation layer forked. Nothing in
//! this file mentions a tag, a payload or a NaN.

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub funcs: Vec<FuncDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncDecl {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let(String, Expr),
    Assign(String, Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    Return(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A JavaScript Number. One number type, IEEE-754 double, per ECMA-262
    /// 6.1.6.1 -- the corpus only ever uses integer-valued ones, but the
    /// representation has to be able to hold any of them (criterion 2).
    Num(f64),
    Str(String),
    Var(String),
    Call(String, Vec<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}
