//! Tokens -> [`Program`]. Shared by both variants.
//!
//! Precedence climbing over a binding-power table, the same shape the upstream
//! compiler uses, because adding a level should be one row rather than one
//! near-identical function.

use crate::ast::{BinOp, Expr, FuncDecl, Program, Stmt, UnOp};
use crate::lex::{Kw, Tok};

pub fn parse(tokens: &[Tok]) -> Result<Program, String> {
    let mut p = Parser { t: tokens, i: 0 };
    let mut funcs = Vec::new();
    while p.peek() != &Tok::Eof {
        funcs.push(p.func_decl()?);
    }
    if funcs.is_empty() {
        return Err("a program needs at least one function declaration".to_string());
    }
    Ok(Program { funcs })
}

struct Parser<'a> {
    t: &'a [Tok],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        self.t.get(self.i).unwrap_or(&Tok::Eof)
    }
    fn next(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.i < self.t.len() {
            self.i += 1;
        }
        t
    }
    fn eat(&mut self, want: &Tok) -> Result<(), String> {
        if self.peek() == want {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected {want:?}, found {:?}", self.peek()))
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        match self.next() {
            Tok::Ident(n) => Ok(n),
            other => Err(format!("expected a name, found {other:?}")),
        }
    }

    fn func_decl(&mut self) -> Result<FuncDecl, String> {
        self.eat(&Tok::Kw(Kw::Function))?;
        let name = self.ident()?;
        self.eat(&Tok::LParen)?;
        let mut params = Vec::new();
        while self.peek() != &Tok::RParen {
            params.push(self.ident()?);
            if self.peek() == &Tok::Comma {
                self.i += 1;
            }
        }
        self.eat(&Tok::RParen)?;
        let body = self.block()?;
        Ok(FuncDecl { name, params, body })
    }

    fn block(&mut self) -> Result<Vec<Stmt>, String> {
        self.eat(&Tok::LBrace)?;
        let mut out = Vec::new();
        while self.peek() != &Tok::RBrace {
            if self.peek() == &Tok::Eof {
                return Err("unterminated block".to_string());
            }
            out.push(self.stmt()?);
        }
        self.eat(&Tok::RBrace)?;
        Ok(out)
    }

    fn stmt(&mut self) -> Result<Stmt, String> {
        match self.peek().clone() {
            Tok::Kw(Kw::Let) => {
                self.i += 1;
                let name = self.ident()?;
                self.eat(&Tok::Assign)?;
                let e = self.expr(0)?;
                self.eat(&Tok::Semi)?;
                Ok(Stmt::Let(name, e))
            }
            Tok::Kw(Kw::Return) => {
                self.i += 1;
                let e = self.expr(0)?;
                self.eat(&Tok::Semi)?;
                Ok(Stmt::Return(e))
            }
            Tok::Kw(Kw::If) => {
                self.i += 1;
                self.eat(&Tok::LParen)?;
                let cond = self.expr(0)?;
                self.eat(&Tok::RParen)?;
                let then = self.block()?;
                let otherwise = if self.peek() == &Tok::Kw(Kw::Else) {
                    self.i += 1;
                    self.block()?
                } else {
                    Vec::new()
                };
                Ok(Stmt::If(cond, then, otherwise))
            }
            Tok::Kw(Kw::While) => {
                self.i += 1;
                self.eat(&Tok::LParen)?;
                let cond = self.expr(0)?;
                self.eat(&Tok::RParen)?;
                let body = self.block()?;
                Ok(Stmt::While(cond, body))
            }
            Tok::Ident(name) => {
                self.i += 1;
                self.eat(&Tok::Assign)?;
                let e = self.expr(0)?;
                self.eat(&Tok::Semi)?;
                Ok(Stmt::Assign(name, e))
            }
            other => Err(format!("expected a statement, found {other:?}")),
        }
    }

    fn expr(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.prefix()?;
        while let Some((op, lbp, rbp)) = infix(self.peek()) {
            if lbp < min_bp {
                break;
            }
            self.i += 1;
            let rhs = self.expr(rbp)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn prefix(&mut self) -> Result<Expr, String> {
        match self.next() {
            Tok::Num(v) => Ok(Expr::Num(v)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Minus => Ok(Expr::Unary(UnOp::Neg, Box::new(self.expr(30)?))),
            Tok::LParen => {
                let e = self.expr(0)?;
                self.eat(&Tok::RParen)?;
                Ok(e)
            }
            Tok::Ident(name) => {
                if self.peek() == &Tok::LParen {
                    self.i += 1;
                    let mut args = Vec::new();
                    while self.peek() != &Tok::RParen {
                        args.push(self.expr(0)?);
                        if self.peek() == &Tok::Comma {
                            self.i += 1;
                        }
                    }
                    self.eat(&Tok::RParen)?;
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            other => Err(format!("expected an operand, found {other:?}")),
        }
    }
}

/// `(operator, left power, right power)`. `right = left + 1` makes a level
/// left associative.
fn infix(t: &Tok) -> Option<(BinOp, u8, u8)> {
    Some(match t {
        Tok::EqEq => (BinOp::Eq, 5, 6),
        Tok::BangEq => (BinOp::Ne, 5, 6),
        Tok::Lt => (BinOp::Lt, 10, 11),
        Tok::Le => (BinOp::Le, 10, 11),
        Tok::Gt => (BinOp::Gt, 10, 11),
        Tok::Ge => (BinOp::Ge, 10, 11),
        Tok::Plus => (BinOp::Add, 20, 21),
        Tok::Minus => (BinOp::Sub, 20, 21),
        Tok::Star => (BinOp::Mul, 25, 26),
        Tok::Slash => (BinOp::Div, 25, 26),
        _ => return None,
    })
}
