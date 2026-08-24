//! A wasm-shaped module in Rust types, plus the provenance tag that makes the
//! size criteria measurable.
//!
//! Every instruction carries an [`Origin`]. That is the whole reason this file
//! is not just a copy of a wasm IR: criterion 6 asks for size in three tiers,
//! and the innermost tier ("value-representation mechanism code only") is not a
//! section, a function, or a file -- it is a subset of the instructions inside
//! ordinary functions. Tagging at emission time is the only way to report it
//! without dividing across measurement definitions.

/// Who emitted an instruction. Assigned where the instruction is created, never
/// inferred afterwards.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// The value-representation layer: boxing, unboxing, tag tests, tag
    /// construction. Tier L1 is exactly the encoded bytes of these.
    Repr,
    /// The emitted guest runtime that is not the representation itself:
    /// float arithmetic, the bump allocator, the string helpers, control flow
    /// inside those helpers.
    Runtime,
    /// A corpus function's own code: locals, calls, control flow, literals.
    Corpus,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValType {
    I32,
    I64,
    F64,
}

impl ValType {
    pub fn byte(self) -> u8 {
        match self {
            ValType::I32 => 0x7F,
            ValType::I64 => 0x7E,
            ValType::F64 => 0x7C,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockType {
    Empty,
    Value(ValType),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ins {
    // control
    Block(BlockType),
    Loop(BlockType),
    If(BlockType),
    Else,
    End,
    Br(u32),
    BrIf(u32),
    Return,
    Call(u32),
    Unreachable,
    Drop,
    Select,
    // variables
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    GlobalGet(u32),
    GlobalSet(u32),
    // memory (align exponent, offset)
    I32Load(u32, u32),
    I32Load8U(u32, u32),
    I32Store(u32, u32),
    I32Store8(u32, u32),
    MemorySize,
    MemoryGrow,
    // constants
    I32Const(i32),
    I64Const(i64),
    F64Const(f64),
    // i32
    I32Eqz,
    I32Eq,
    I32Ne,
    I32LtU,
    I32GeU,
    I32Add,
    I32Sub,
    I32Mul,
    I32And,
    I32Or,
    I32Shl,
    // i64
    I64Eq,
    I64LtU,
    I64GeU,
    I64Add,
    I64And,
    I64Or,
    I64Shl,
    // f64
    F64Eq,
    F64Ne,
    F64Lt,
    F64Gt,
    F64Le,
    F64Ge,
    F64Neg,
    F64Add,
    F64Sub,
    F64Mul,
    F64Div,
    // conversions
    I32WrapI64,
    I64ExtendI32U,
    F64ConvertI32S,
    F64ReinterpretI64,
    I64ReinterpretF64,
}

/// A run of instructions under construction, each already tagged.
///
/// Three push methods rather than one plus a mode flag: the tag is a claim
/// about who is responsible for a byte, and a mode flag is exactly the thing
/// that goes stale when code is moved.
#[derive(Clone, Default, Debug)]
pub struct Body {
    pub ins: Vec<(Ins, Origin)>,
}

impl Body {
    pub fn new() -> Self {
        Self::default()
    }
    /// Emit as the value-representation layer.
    pub fn r(&mut self, i: Ins) {
        self.ins.push((i, Origin::Repr));
    }
    /// Emit as the guest runtime.
    pub fn t(&mut self, i: Ins) {
        self.ins.push((i, Origin::Runtime));
    }
    /// Emit as corpus code.
    pub fn c(&mut self, i: Ins) {
        self.ins.push((i, Origin::Corpus));
    }
    pub fn append(&mut self, other: Body) {
        self.ins.extend(other.ins);
    }
    pub fn len(&self) -> usize {
        self.ins.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ins.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub type_index: u32,
    /// Declared locals beyond the parameters, as run-length groups.
    pub locals: Vec<(u32, ValType)>,
    pub body: Body,
    /// Whether this function is emitted runtime or compiled corpus source.
    /// Criterion 5 splits its two columns on exactly this.
    pub is_runtime: bool,
}

#[derive(Clone, Debug)]
pub struct Export {
    pub name: String,
    pub func_index: u32,
}

#[derive(Clone, Debug)]
pub struct Global {
    pub ty: ValType,
    pub mutable: bool,
    pub init_i32: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub types: Vec<FuncType>,
    pub funcs: Vec<Func>,
    pub exports: Vec<Export>,
    pub globals: Vec<Global>,
    /// `(min_pages, max_pages)`, or none when the point needs no linear memory.
    pub memory: Option<(u32, u32)>,
    /// One active data segment at this byte offset.
    pub data: Option<(u32, Vec<u8>)>,
}

impl Module {
    pub fn intern_type(&mut self, wanted: FuncType) -> u32 {
        if let Some(i) = self.types.iter().position(|t| *t == wanted) {
            return i as u32;
        }
        self.types.push(wanted);
        (self.types.len() - 1) as u32
    }
}
