//! Tokens -> [`ast::Program`].
//!
//! Precedence climbing over a binding-power table rather than one function per
//! precedence level. The two are equivalent for the five operators M0 has; they
//! stop being equivalent at the fourteen JavaScript actually defines, where the
//! function-per-level shape becomes fourteen near-identical functions. Adding a
//! level here is one row in [`infix`].

use super::CompileError;
use super::ast::{BinaryOp, Expr, Program, UnaryOp};
use super::diag::{malformed, unsupported};
use super::lex::{OUT_OF_I32_RANGE, TOO_MANY_ARGUMENTS, Token, TokenKind};

/// Binding power of the unary minus. Above every infix level, so `-2 * 3` is
/// `(-2) * 3` and not `-(2 * 3)` -- indistinguishable here, but not once `**`
/// (right associative, binds tighter than unary) arrives.
const PREFIX_BP: u8 = 30;

/// The most arguments one call may name. `$N` becomes a wasm parameter, so an
/// unbounded index would let a two-character source demand a huge signature.
const MAX_ARGS: u64 = 64;

pub(crate) fn parse(tokens: &[Token]) -> Result<Program, CompileError> {
    let mut parser = Parser { tokens, pos: 0 };
    parser.program()
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl Parser<'_> {
    /// The token under the cursor. `tokenize` always terminates the stream
    /// with `Eof` and [`advance`](Self::advance) never steps past it, so the
    /// cursor is a fixed point at the end and this cannot run off.
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn advance(&mut self) {
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
    }

    fn program(&mut self) -> Result<Program, CompileError> {
        let first = self.peek();
        if first.kind == TokenKind::Eof {
            return Err(malformed(
                "needs an expression to compile; this source is empty",
                first.offset,
            ));
        }
        let expr = self.expression(0)?;
        while self.peek().kind == TokenKind::Semi {
            self.advance();
        }
        let tail = self.peek();
        match &tail.kind {
            TokenKind::Eof => Ok(Program { expr }),
            TokenKind::Unsupported(phrase) => Err(unsupported(phrase, tail.offset)),
            TokenKind::RParen => Err(malformed(
                "found a `)` here with no `(` to match it",
                tail.offset,
            )),
            // A second expression where the source should have ended. M0
            // compiles one expression, so this is a capability boundary and
            // not a mistake: `1; 2` is perfectly good JavaScript.
            _ => Err(unsupported("multiple statements", tail.offset)),
        }
    }

    /// One expression, consuming infix operators whose left binding power is at
    /// least `min_bp`.
    fn expression(&mut self, min_bp: u8) -> Result<Expr, CompileError> {
        let mut lhs = self.prefix()?;
        while let Some((op, lbp, rbp)) = infix(&self.peek().kind) {
            if lbp < min_bp {
                break;
            }
            self.advance();
            let rhs = self.expression(rbp)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// A literal, an argument, a parenthesised group, or a unary minus.
    fn prefix(&mut self) -> Result<Expr, CompileError> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Int(magnitude) => {
                self.advance();
                int_literal(magnitude, false, token.offset)
            }
            TokenKind::Arg(index) => {
                self.advance();
                if index >= MAX_ARGS {
                    return Err(unsupported(TOO_MANY_ARGUMENTS, token.offset));
                }
                Ok(Expr::Arg(index as u32))
            }
            TokenKind::Minus => {
                self.advance();
                // Fold a minus directly onto a literal. Not an optimisation:
                // `i32::MIN` has no positive counterpart, so `-2147483648` is
                // only representable if the sign reaches the literal.
                let next = self.peek().clone();
                if let TokenKind::Int(magnitude) = next.kind {
                    self.advance();
                    return int_literal(magnitude, true, next.offset);
                }
                let operand = self.expression(PREFIX_BP)?;
                Ok(Expr::Unary(UnaryOp::Neg, Box::new(operand)))
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.expression(0)?;
                let close = self.peek().clone();
                match close.kind {
                    TokenKind::RParen => {
                        self.advance();
                        Ok(inner)
                    }
                    TokenKind::Unsupported(phrase) => Err(unsupported(&phrase, close.offset)),
                    TokenKind::Eof => Err(malformed(
                        &format!(
                            "needs a `)` to close the group opened at byte {}; the source ends first",
                            token.offset
                        ),
                        close.offset,
                    )),
                    other => Err(malformed(
                        &format!(
                            "needs a `)` to close the group opened at byte {}, and found {} instead",
                            token.offset,
                            other.name()
                        ),
                        close.offset,
                    )),
                }
            }
            TokenKind::Unsupported(phrase) => Err(unsupported(&phrase, token.offset)),
            TokenKind::Eof => Err(malformed(
                "needs an operand here; the source ends first",
                token.offset,
            )),
            other => Err(malformed(
                &format!("needs an operand here, and found {} instead", other.name()),
                token.offset,
            )),
        }
    }
}

/// Range-check a decimal magnitude and apply the sign. `negative` widens the
/// accepted range by exactly one, which is the whole reason the sign is folded
/// here instead of lowered as an operator.
fn int_literal(magnitude: u64, negative: bool, offset: usize) -> Result<Expr, CompileError> {
    let limit = if negative {
        i32::MIN.unsigned_abs() as u64
    } else {
        i32::MAX as u64
    };
    if magnitude > limit {
        return Err(unsupported(OUT_OF_I32_RANGE, offset));
    }
    let value = if negative {
        (magnitude as i64).wrapping_neg() as i32
    } else {
        magnitude as i32
    };
    Ok(Expr::Int(value))
}

/// The binding-power table: `(operator, left power, right power)`.
///
/// `right = left + 1` is what makes a level left associative -- the loop in
/// [`Parser::expression`] refuses to re-enter at the same power. A future
/// right-associative level (`**`, assignment) uses `right = left` instead.
fn infix(kind: &TokenKind) -> Option<(BinaryOp, u8, u8)> {
    match kind {
        TokenKind::Plus => Some((BinaryOp::Add, 10, 11)),
        TokenKind::Minus => Some((BinaryOp::Sub, 10, 11)),
        TokenKind::Star => Some((BinaryOp::Mul, 20, 21)),
        TokenKind::Slash => Some((BinaryOp::Div, 20, 21)),
        TokenKind::Percent => Some((BinaryOp::Rem, 20, 21)),
        _ => None,
    }
}
