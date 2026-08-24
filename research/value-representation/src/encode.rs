//! [`Module`] -> standard `.wasm` bytes, and the byte accounting the size
//! criteria need.
//!
//! Hand-written, per the experiment's constraint 5 (no third-party wasm
//! encoder). The output has to clear tinyvm's load gate, which is strict about
//! canonical section order, minimal LEB128, memarg alignment and the exact
//! `end` that terminates an expression.
//!
//! Instruction encoding here is context free: every immediate is already
//! resolved in the IR, so the encoded length of one instruction never depends
//! on its neighbours. That is what makes [`SizeReport`] exact rather than an
//! estimate -- the per-origin byte totals are sums of real encodings, not a
//! proportional split of a section length.

use crate::ir::{BlockType, FuncType, Global, Ins, Module, Origin};

const HEADER: [u8; 8] = [0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
const END: u8 = 0x0B;
const FUNC_TYPE_TAG: u8 = 0x60;

/// Byte accounting for one encoded module, in the three tiers the measurement
/// discipline requires plus the two columns criterion 5 asks for.
#[derive(Clone, Copy, Debug, Default)]
pub struct SizeReport {
    /// L1: encoded instruction bytes tagged [`Origin::Repr`], every function.
    pub l1_repr_ins: usize,
    /// Encoded instruction bytes tagged [`Origin::Runtime`].
    pub runtime_ins: usize,
    /// Encoded instruction bytes tagged [`Origin::Corpus`].
    pub corpus_ins: usize,
    /// Code-section bytes of runtime functions: size prefix, locals
    /// declaration, every instruction, terminating `end`.
    pub runtime_func_total: usize,
    /// The same for corpus functions.
    pub corpus_func_total: usize,
    /// Memory + global section bytes (present only where the point needs a heap).
    pub heap_decl_bytes: usize,
    /// Data section bytes (string literals; corpus data, not runtime).
    pub data_section_bytes: usize,
    /// L3: the whole `.wasm` file.
    pub l3_total: usize,
    pub runtime_func_count: usize,
    pub corpus_func_count: usize,
    /// L1 bytes that sit inside runtime functions, so L2 does not count them twice.
    pub l1_in_runtime: usize,
    /// L1 bytes spent on constant immediates, and how many constants that was.
    /// NaN-boxing's tag masks are 64-bit, and LEB128 charges by magnitude, so
    /// this is the single largest confound in the size numbers -- reported
    /// separately rather than buried.
    pub l1_const_bytes: usize,
    pub l1_const_count: usize,
}

impl SizeReport {
    /// L2: L1 plus everything else this variant must emit to run the corpus --
    /// the non-representation runtime instructions and the code-section framing
    /// of the runtime functions, plus the memory and global declarations.
    /// What L1 would be if every representation constant were hoisted into a
    /// module global and read with a two-byte `global.get`. Not a build that
    /// exists -- hoisting is an optimisation, and constraint 4 forbids
    /// optimising either side -- but the counterfactual that says whether the
    /// L1 ordering survives the LEB128 confound.
    pub fn l1_constants_hoisted(&self) -> usize {
        self.l1_repr_ins - self.l1_const_bytes + 2 * self.l1_const_count
    }
    pub fn l2(&self) -> usize {
        self.l1_repr_ins + (self.runtime_func_total - self.l1_in_runtime) + self.heap_decl_bytes
    }
}

pub struct Encoded {
    pub bytes: Vec<u8>,
    pub size: SizeReport,
}

pub fn encode(module: &Module) -> Encoded {
    let mut out = HEADER.to_vec();
    let mut size = SizeReport::default();

    section(&mut out, 1, |b| {
        vector(b, &module.types, func_type);
    });
    section(&mut out, 3, |b| {
        vector(b, &module.funcs, |b, f| unsigned(b, f.type_index));
    });
    let mut heap_bytes = 0usize;
    if let Some((min, max)) = module.memory {
        let before = out.len();
        section(&mut out, 5, |b| {
            unsigned(b, 1);
            b.push(0x01);
            unsigned(b, min);
            unsigned(b, max);
        });
        heap_bytes += out.len() - before;
    }
    if !module.globals.is_empty() {
        let before = out.len();
        section(&mut out, 6, |b| {
            vector(b, &module.globals, |b, g: &Global| {
                b.push(g.ty.byte());
                b.push(u8::from(g.mutable));
                b.push(0x41);
                signed_32(b, g.init_i32);
                b.push(END);
            });
        });
        heap_bytes += out.len() - before;
    }
    size.heap_decl_bytes = heap_bytes;

    section(&mut out, 7, |b| {
        vector(b, &module.exports, |b, e| {
            name(b, &e.name);
            b.push(0x00);
            unsigned(b, e.func_index);
        });
    });

    section(&mut out, 10, |b| {
        unsigned(b, module.funcs.len() as u32);
        for f in &module.funcs {
            let mut code = Vec::new();
            vector(&mut code, &f.locals, |c, (n, t)| {
                unsigned(c, *n);
                c.push(t.byte());
            });
            let mut per_origin = [0usize; 3];
            for (ins, origin) in &f.body.ins {
                let before = code.len();
                instruction(&mut code, ins);
                let width = code.len() - before;
                per_origin[*origin as usize] += width;
                if *origin == Origin::Repr
                    && matches!(ins, Ins::I32Const(_) | Ins::I64Const(_) | Ins::F64Const(_))
                {
                    size.l1_const_bytes += width;
                    size.l1_const_count += 1;
                }
            }
            code.push(END);
            let mut entry = Vec::new();
            unsigned(&mut entry, code.len() as u32);
            entry.extend_from_slice(&code);

            size.l1_repr_ins += per_origin[Origin::Repr as usize];
            size.runtime_ins += per_origin[Origin::Runtime as usize];
            size.corpus_ins += per_origin[Origin::Corpus as usize];
            if f.is_runtime {
                size.runtime_func_total += entry.len();
                size.runtime_func_count += 1;
                size.l1_in_runtime += per_origin[Origin::Repr as usize];
            } else {
                size.corpus_func_total += entry.len();
                size.corpus_func_count += 1;
            }
            b.extend_from_slice(&entry);
        }
    });

    if let Some((offset, bytes)) = &module.data {
        let before = out.len();
        section(&mut out, 11, |b| {
            unsigned(b, 1);
            unsigned(b, 0);
            b.push(0x41);
            signed_32(b, *offset as i32);
            b.push(END);
            unsigned(b, bytes.len() as u32);
            b.extend_from_slice(bytes);
        });
        size.data_section_bytes = out.len() - before;
    }

    size.l3_total = out.len();
    Encoded { bytes: out, size }
}

fn section(out: &mut Vec<u8>, id: u8, build: impl FnOnce(&mut Vec<u8>)) {
    let mut body = Vec::new();
    build(&mut body);
    out.push(id);
    unsigned(out, body.len() as u32);
    out.extend_from_slice(&body);
}

fn vector<T>(out: &mut Vec<u8>, items: &[T], mut element: impl FnMut(&mut Vec<u8>, &T)) {
    unsigned(out, items.len() as u32);
    for item in items {
        element(out, item);
    }
}

fn func_type(out: &mut Vec<u8>, ty: &FuncType) {
    out.push(FUNC_TYPE_TAG);
    vector(out, &ty.params, |o, t| o.push(t.byte()));
    vector(out, &ty.results, |o, t| o.push(t.byte()));
}

fn name(out: &mut Vec<u8>, text: &str) {
    unsigned(out, text.len() as u32);
    out.extend_from_slice(text.as_bytes());
}

fn block_type(out: &mut Vec<u8>, ty: BlockType) {
    match ty {
        BlockType::Empty => out.push(0x40),
        BlockType::Value(v) => out.push(v.byte()),
    }
}

fn memarg(out: &mut Vec<u8>, align: u32, offset: u32) {
    unsigned(out, align);
    unsigned(out, offset);
}

fn instruction(out: &mut Vec<u8>, ins: &Ins) {
    match *ins {
        Ins::Unreachable => out.push(0x00),
        Ins::Block(t) => {
            out.push(0x02);
            block_type(out, t);
        }
        Ins::Loop(t) => {
            out.push(0x03);
            block_type(out, t);
        }
        Ins::If(t) => {
            out.push(0x04);
            block_type(out, t);
        }
        Ins::Else => out.push(0x05),
        Ins::End => out.push(0x0B),
        Ins::Br(l) => {
            out.push(0x0C);
            unsigned(out, l);
        }
        Ins::BrIf(l) => {
            out.push(0x0D);
            unsigned(out, l);
        }
        Ins::Return => out.push(0x0F),
        Ins::Call(f) => {
            out.push(0x10);
            unsigned(out, f);
        }
        Ins::Drop => out.push(0x1A),
        Ins::Select => out.push(0x1B),
        Ins::LocalGet(x) => {
            out.push(0x20);
            unsigned(out, x);
        }
        Ins::LocalSet(x) => {
            out.push(0x21);
            unsigned(out, x);
        }
        Ins::LocalTee(x) => {
            out.push(0x22);
            unsigned(out, x);
        }
        Ins::GlobalGet(x) => {
            out.push(0x23);
            unsigned(out, x);
        }
        Ins::GlobalSet(x) => {
            out.push(0x24);
            unsigned(out, x);
        }
        Ins::I32Load(a, o) => {
            out.push(0x28);
            memarg(out, a, o);
        }
        Ins::I32Load8U(a, o) => {
            out.push(0x2D);
            memarg(out, a, o);
        }
        Ins::I32Store(a, o) => {
            out.push(0x36);
            memarg(out, a, o);
        }
        Ins::I32Store8(a, o) => {
            out.push(0x3A);
            memarg(out, a, o);
        }
        Ins::MemorySize => {
            out.push(0x3F);
            out.push(0x00);
        }
        Ins::MemoryGrow => {
            out.push(0x40);
            out.push(0x00);
        }
        Ins::I32Const(v) => {
            out.push(0x41);
            signed_32(out, v);
        }
        Ins::I64Const(v) => {
            out.push(0x42);
            signed_64(out, v);
        }
        Ins::F64Const(v) => {
            out.push(0x44);
            out.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        Ins::I32Eqz => out.push(0x45),
        Ins::I32Eq => out.push(0x46),
        Ins::I32Ne => out.push(0x47),
        Ins::I32LtU => out.push(0x49),
        Ins::I32GeU => out.push(0x4F),
        Ins::I64Eq => out.push(0x51),
        Ins::I64LtU => out.push(0x54),
        Ins::I64GeU => out.push(0x5A),
        Ins::F64Eq => out.push(0x61),
        Ins::F64Ne => out.push(0x62),
        Ins::F64Lt => out.push(0x63),
        Ins::F64Gt => out.push(0x64),
        Ins::F64Le => out.push(0x65),
        Ins::F64Ge => out.push(0x66),
        Ins::I32Add => out.push(0x6A),
        Ins::I32Sub => out.push(0x6B),
        Ins::I32Mul => out.push(0x6C),
        Ins::I32And => out.push(0x71),
        Ins::I32Or => out.push(0x72),
        Ins::I32Shl => out.push(0x74),
        Ins::I64Add => out.push(0x7C),
        Ins::I64And => out.push(0x83),
        Ins::I64Or => out.push(0x84),
        Ins::I64Shl => out.push(0x86),
        Ins::F64Neg => out.push(0x9A),
        Ins::F64Add => out.push(0xA0),
        Ins::F64Sub => out.push(0xA1),
        Ins::F64Mul => out.push(0xA2),
        Ins::F64Div => out.push(0xA3),
        Ins::I32WrapI64 => out.push(0xA7),
        Ins::I64ExtendI32U => out.push(0xAD),
        Ins::F64ConvertI32S => out.push(0xB7),
        Ins::I64ReinterpretF64 => out.push(0xBD),
        Ins::F64ReinterpretI64 => out.push(0xBF),
    }
}

fn unsigned(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn signed_32(out: &mut Vec<u8>, value: i32) {
    signed_64(out, value as i64);
}

/// Signed LEB128, minimal length. The loop stops when the remaining bits are
/// all copies of the sign bit *and* the byte just written carries that sign in
/// bit 6; dropping either half is the classic way to encode -64 as `0x40`.
fn signed_64(out: &mut Vec<u8>, value: i64) {
    let mut remaining = value;
    loop {
        let byte = (remaining & 0x7F) as u8;
        remaining >>= 7;
        let sign_bit_set = byte & 0x40 != 0;
        if (remaining == 0 && !sign_bit_set) || (remaining == -1 && sign_bit_set) {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}
