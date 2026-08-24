//! `.qjs` -> `.wasm`, in pure Rust.
//!
//! ```text
//! source  --lex-->  tokens  --parse-->  AST  --emit-->  wasm IR  --encode-->  bytes
//! ```
//!
//! Five stages, five modules, each with one job. That is more structure than
//! M0's arithmetic needs and exactly the structure M1-M5 need: strings, objects
//! and closures each land in one or two of these stages, and none of them wants
//! to be threaded through a single pass that turns characters straight into
//! bytes.
//!
//! # What M0 compiles
//!
//! Decimal integer literals, `+ - * / %`, unary minus, parentheses, and `$N`
//! for the Nth argument of this call. The result is an ordinary wasm module
//! exporting one function named `main`, taking one `i32` parameter per argument
//! the source names and returning `i32`. No imports, no memory, no host door --
//! an expression has nothing to say to the world.
//!
//! Everything else is rejected with a diagnostic that names the engine's
//! boundary rather than blaming the script; see [`diag`]. `/` and `%` diverge
//! from JavaScript on a zero divisor, deliberately and for a documented reason
//! -- see [`emit`].

mod ast;
mod diag;
mod emit;
mod encode;
mod ir;
mod lex;
mod parse;

/// A compile failure that names the engine's capability boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub message: String,
    pub offset: usize,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.offset)
    }
}

impl std::error::Error for CompileError {}

/// Compile `.qjs` source to standard wasm bytes. Compile-only: never executes.
///
/// The bytes are an ordinary module. They go through tinyvm's load gate on the
/// same terms as a hand-written `.wasm` guest, which is the point: there is one
/// engine here, not two pipelines sharing a name.
pub fn compile_qjs(source: &str) -> Result<Vec<u8>, CompileError> {
    let tokens = lex::tokenize(source)?;
    let program = parse::parse(&tokens)?;
    let module = emit::lower(&program);
    Ok(encode::encode(&module))
}
