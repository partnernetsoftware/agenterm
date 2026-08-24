//! A wasm-shaped module, still in Rust types.
//!
//! This sits between the tree and the bytes on purpose. Lowering decides *what
//! wasm* to build; [`super::encode`] decides *what bytes* that wasm is. Keeping
//! them apart is what lets the encoder be strict on its own terms -- canonical
//! section order, minimal LEB128, exact expression termination -- without the
//! lowering having to know any of it, and what lets M1's control flow be tested
//! as instruction sequences before a single byte is written.
//!
//! The vocabulary is only as wide as M0 needs. Every milestone adds variants;
//! none of them changes this file's role.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValType {
    I32,
}

/// A function signature. Both vectors are the wasm ones, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FuncType {
    pub(crate) params: Vec<ValType>,
    pub(crate) results: Vec<ValType>,
}

/// One instruction. Named after the wasm opcode it becomes, not after the
/// JavaScript operator it came from -- the mapping between those two stops
/// being one-to-one as soon as JS numbers are real numbers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ins {
    I32Const(i32),
    LocalGet(u32),
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32RemS,
}

/// A defined function: its type, its declared locals beyond the parameters as
/// run-length `(count, type)` groups, and its body.
///
/// The body does *not* carry the terminating `end`. That byte is part of how an
/// expression is encoded, not a choice the lowering makes, so it belongs to the
/// encoder -- and keeping it there means no lowering pass can forget it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Func {
    pub(crate) type_index: u32,
    pub(crate) locals: Vec<(u32, ValType)>,
    pub(crate) body: Vec<Ins>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportKind {
    Func,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Export {
    pub(crate) name: String,
    pub(crate) kind: ExportKind,
    pub(crate) index: u32,
}

/// A whole module. M0 has no imports, memory, tables, globals, or start
/// function, so those sections have no fields here yet rather than empty ones:
/// an empty vector would be a section the encoder must decide whether to emit,
/// and a decision no caller can influence is not worth having.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Module {
    pub(crate) types: Vec<FuncType>,
    pub(crate) funcs: Vec<Func>,
    pub(crate) exports: Vec<Export>,
}
