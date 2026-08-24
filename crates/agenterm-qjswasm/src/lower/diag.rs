//! Compile diagnostics.
//!
//! Every message speaks for the *engine*, never about the author. That is a
//! product requirement (PRD 36), not a style preference: this compiler's subset
//! is deliberately small and grows by real script demand, so the overwhelmingly
//! common rejection is a perfectly good script that is simply ahead of the
//! engine. Telling that author "syntax error" would be a lie.
//!
//! Two constructors, and the distinction between them is the whole point:
//!
//! * [`unsupported`] -- the construct is real JavaScript that this engine does
//!   not lower yet. The wording is fixed ("this engine does not support X
//!   yet") and locked by `tests/qjs_m0.rs`, because that wording is the thing
//!   product documentation is not allowed to drift away from.
//! * [`malformed`] -- the source is structurally incomplete (ends mid
//!   expression, an unclosed group). Free wording, but still narrated from the
//!   engine's side: what it was looking for and did not find.

use super::CompileError;

/// "this engine does not support {construct} yet".
///
/// `construct` is a noun phrase naming the capability, e.g. `"string
/// literals"`, ``"the `let` keyword"``. It must read naturally in that
/// sentence, and it must be specific enough that a reader can tell which part
/// of their script is ahead of the engine.
pub(crate) fn unsupported(construct: &str, offset: usize) -> CompileError {
    CompileError {
        message: format!("this engine does not support {construct} yet"),
        offset,
    }
}

/// "this engine {what}" -- for input the engine cannot finish reading.
///
/// `what` is a verb phrase completing that sentence, e.g. `"needs an operand
/// after the operator here; the source ends first"`.
pub(crate) fn malformed(what: &str, offset: usize) -> CompileError {
    CompileError {
        message: format!("this engine {what}"),
        offset,
    }
}
