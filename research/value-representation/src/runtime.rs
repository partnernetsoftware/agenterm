//! The guest-side runtime the compiler emits into every product.
//!
//! Shared by both variants: the arithmetic, the dispatch shape, the bump
//! allocator and the string helpers are written once and parameterised by
//! [`Repr`]. Only the boxing, unboxing and tag tests inside them differ, and
//! those are tagged [`Origin::Repr`] so criterion 6 can separate them.
//!
//! Every operator is a call. That is the straightforward lowering for a
//! compiler with no optimiser: `a + b` in a language where `+` dispatches on
//! type is a runtime call, exactly as it is in an unoptimised bytecode engine.
//! Inlining it would be an optimisation, and experiment constraint 4 forbids
//! optimising either side.

use crate::ir::{BlockType, Body, Func, FuncType, Ins, ValType};
use crate::repr::{Repr, load_local, store_local};

/// The emitted runtime functions, in index order. Position in this list is the
/// wasm function index, so the list is the call table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rt {
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
    Neg,
    Truthy,
    Len,
    Alloc,
    StrConcat,
    StrEq,
}

pub const P1_SET: &[Rt] = &[
    Rt::Add,
    Rt::Sub,
    Rt::Mul,
    Rt::Div,
    Rt::Lt,
    Rt::Le,
    Rt::Gt,
    Rt::Ge,
    Rt::Eq,
    Rt::Ne,
    Rt::Neg,
    Rt::Truthy,
];

pub const P2_SET: &[Rt] = &[
    Rt::Add,
    Rt::Sub,
    Rt::Mul,
    Rt::Div,
    Rt::Lt,
    Rt::Le,
    Rt::Gt,
    Rt::Ge,
    Rt::Eq,
    Rt::Ne,
    Rt::Neg,
    Rt::Truthy,
    Rt::Len,
    Rt::Alloc,
    Rt::StrConcat,
    Rt::StrEq,
];

impl Rt {
    pub fn symbol(self) -> &'static str {
        match self {
            Rt::Add => "__add",
            Rt::Sub => "__sub",
            Rt::Mul => "__mul",
            Rt::Div => "__div",
            Rt::Lt => "__lt",
            Rt::Le => "__le",
            Rt::Gt => "__gt",
            Rt::Ge => "__ge",
            Rt::Eq => "__eq",
            Rt::Ne => "__ne",
            Rt::Neg => "__neg",
            Rt::Truthy => "__truthy",
            Rt::Len => "__len",
            Rt::Alloc => "__alloc",
            Rt::StrConcat => "__str_concat",
            Rt::StrEq => "__str_eq",
        }
    }
}

/// Index of a runtime symbol in the emitted module.
pub fn index_of(set: &[Rt], want: Rt) -> u32 {
    set.iter()
        .position(|r| *r == want)
        .unwrap_or_else(|| panic!("{:?} is not in this point's runtime set", want)) as u32
}

/// A function under construction: parameters are fixed, locals grow.
pub struct FnBuild {
    pub param_words: u32,
    pub extra: Vec<ValType>,
    pub body: Body,
}

impl FnBuild {
    pub fn new(param_words: u32) -> Self {
        Self {
            param_words,
            extra: Vec::new(),
            body: Body::new(),
        }
    }
    pub fn local(&mut self, ty: ValType) -> u32 {
        let index = self.param_words + self.extra.len() as u32;
        self.extra.push(ty);
        index
    }
    /// Reserve one JS value's worth of locals and return its base index.
    pub fn value_local(&mut self, repr: &dyn Repr) -> u32 {
        let base = self.param_words + self.extra.len() as u32;
        for t in repr.slots() {
            self.extra.push(*t);
        }
        base
    }
    /// Run-length groups, as the code section wants them.
    pub fn local_groups(&self) -> Vec<(u32, ValType)> {
        let mut groups: Vec<(u32, ValType)> = Vec::new();
        for t in &self.extra {
            match groups.last_mut() {
                Some((n, prev)) if prev == t => *n += 1,
                _ => groups.push((1, *t)),
            }
        }
        groups
    }
}

pub struct Ctx<'a> {
    pub repr: &'a dyn Repr,
    pub set: &'a [Rt],
    /// Global index of the bump pointer, when this point has a heap.
    pub heap_global: u32,
    pub point_has_strings: bool,
    /// Sensitivity switch, not a variant: which type `__add` tests for first.
    /// The default (`false`) tests for strings first, so every numeric
    /// addition pays the new type's test and criterion 3's slope is non-zero.
    /// Setting it moves the number path in front, so the added type costs
    /// nothing on the shared corpus. Both variants always use the same
    /// setting; see `RESULTS.md`, sensitivity S-ADD.
    pub add_number_first: bool,
}

impl Ctx<'_> {
    pub fn call(&self, rt: Rt) -> Ins {
        Ins::Call(index_of(self.set, rt))
    }
    fn width(&self) -> u32 {
        self.repr.width()
    }
    fn value_type(&self, module_types: &mut Vec<FuncType>, params: usize) -> u32 {
        let ty = FuncType {
            params: (0..params)
                .flat_map(|_| self.repr.slots().to_vec())
                .collect(),
            results: self.repr.slots().to_vec(),
        };
        intern(module_types, ty)
    }
}

fn intern(types: &mut Vec<FuncType>, wanted: FuncType) -> u32 {
    if let Some(i) = types.iter().position(|t| *t == wanted) {
        return i as u32;
    }
    types.push(wanted);
    (types.len() - 1) as u32
}

/// Build every runtime function for this point, in index order.
pub fn build(ctx: &Ctx<'_>, types: &mut Vec<FuncType>) -> Vec<Func> {
    ctx.set.iter().map(|rt| one(ctx, types, *rt)).collect()
}

fn one(ctx: &Ctx<'_>, types: &mut Vec<FuncType>, rt: Rt) -> Func {
    let (type_index, build) = match rt {
        Rt::Add => (ctx.value_type(types, 2), add(ctx)),
        Rt::Sub => (ctx.value_type(types, 2), arith(ctx, Ins::F64Sub)),
        Rt::Mul => (ctx.value_type(types, 2), arith(ctx, Ins::F64Mul)),
        Rt::Div => (ctx.value_type(types, 2), arith(ctx, Ins::F64Div)),
        Rt::Lt => (ctx.value_type(types, 2), relational(ctx, Ins::F64Lt)),
        Rt::Le => (ctx.value_type(types, 2), relational(ctx, Ins::F64Le)),
        Rt::Gt => (ctx.value_type(types, 2), relational(ctx, Ins::F64Gt)),
        Rt::Ge => (ctx.value_type(types, 2), relational(ctx, Ins::F64Ge)),
        Rt::Eq => (ctx.value_type(types, 2), equality(ctx)),
        Rt::Ne => (ctx.value_type(types, 2), inequality(ctx)),
        Rt::Neg => (ctx.value_type(types, 1), negate(ctx)),
        Rt::Truthy => {
            let ty = FuncType {
                params: ctx.repr.slots().to_vec(),
                results: vec![ValType::I32],
            };
            (intern(types, ty), truthy(ctx))
        }
        Rt::Len => (ctx.value_type(types, 1), length(ctx)),
        Rt::Alloc => {
            let ty = FuncType {
                params: vec![ValType::I32],
                results: vec![ValType::I32],
            };
            (intern(types, ty), alloc(ctx))
        }
        Rt::StrConcat => {
            let ty = FuncType {
                params: vec![ValType::I32, ValType::I32],
                results: vec![ValType::I32],
            };
            (intern(types, ty), str_concat(ctx))
        }
        Rt::StrEq => {
            let ty = FuncType {
                params: vec![ValType::I32, ValType::I32],
                results: vec![ValType::I32],
            };
            (intern(types, ty), str_eq(ctx))
        }
    };
    Func {
        name: rt.symbol().to_string(),
        type_index,
        locals: build.local_groups(),
        body: build.body,
        is_runtime: true,
    }
}

/// `box_number(f64op(to_number(a), to_number(b)))`.
fn arith(ctx: &Ctx<'_>, op: Ins) -> FnBuild {
    let w = ctx.width();
    let mut f = FnBuild::new(2 * w);
    let scratch = reserve_scratch(ctx, &mut f);
    let mut inner = Body::new();
    ctx.repr.unbox_number(0, &mut inner);
    ctx.repr.unbox_number(w, &mut inner);
    inner.t(op);
    ctx.repr.box_number(inner, scratch, &mut f.body);
    f
}

fn relational(ctx: &Ctx<'_>, op: Ins) -> FnBuild {
    let w = ctx.width();
    let mut f = FnBuild::new(2 * w);
    let mut inner = Body::new();
    ctx.repr.unbox_number(0, &mut inner);
    ctx.repr.unbox_number(w, &mut inner);
    inner.t(op);
    ctx.repr.box_bool(inner, &mut f.body);
    f
}

fn negate(ctx: &Ctx<'_>) -> FnBuild {
    let mut f = FnBuild::new(ctx.width());
    let scratch = reserve_scratch(ctx, &mut f);
    let mut inner = Body::new();
    ctx.repr.unbox_number(0, &mut inner);
    inner.t(Ins::F64Neg);
    ctx.repr.box_number(inner, scratch, &mut f.body);
    f
}

/// `+`: number addition, and from P2 onward string concatenation as well.
/// The growth of this one function is the clearest single instance of "the
/// language gained a type".
fn add(ctx: &Ctx<'_>) -> FnBuild {
    if ctx.add_number_first {
        return add_number_first(ctx);
    }
    let w = ctx.width();
    let mut f = FnBuild::new(2 * w);
    let scratch = reserve_scratch(ctx, &mut f);
    if ctx.point_has_strings {
        ctx.repr.is_string(0, &mut f.body);
        ctx.repr.is_string(w, &mut f.body);
        f.body.t(Ins::I32Or);
        f.body.t(Ins::If(BlockType::Empty));
        let mut inner = Body::new();
        ctx.repr.unbox_string(0, &mut inner);
        ctx.repr.unbox_string(w, &mut inner);
        inner.t(ctx.call(Rt::StrConcat));
        ctx.repr.box_string(inner, &mut f.body);
        f.body.t(Ins::Return);
        f.body.t(Ins::End);
    }
    let mut inner = Body::new();
    ctx.repr.unbox_number(0, &mut inner);
    ctx.repr.unbox_number(w, &mut inner);
    inner.t(Ins::F64Add);
    ctx.repr.box_number(inner, scratch, &mut f.body);
    f
}

/// The same `+`, with the number test in front of the string test. Same shape
/// at both points so the P2 - P1 delta stays a like-for-like comparison.
fn add_number_first(ctx: &Ctx<'_>) -> FnBuild {
    let w = ctx.width();
    let mut f = FnBuild::new(2 * w);
    let scratch = reserve_scratch(ctx, &mut f);

    ctx.repr.is_number(0, &mut f.body);
    ctx.repr.is_number(w, &mut f.body);
    f.body.t(Ins::I32And);
    f.body.t(Ins::If(BlockType::Empty));
    let mut inner = Body::new();
    ctx.repr.unbox_number(0, &mut inner);
    ctx.repr.unbox_number(w, &mut inner);
    inner.t(Ins::F64Add);
    ctx.repr.box_number(inner, scratch, &mut f.body);
    f.body.t(Ins::Return);
    f.body.t(Ins::End);

    if ctx.point_has_strings {
        ctx.repr.is_string(0, &mut f.body);
        ctx.repr.is_string(w, &mut f.body);
        f.body.t(Ins::I32Or);
        f.body.t(Ins::If(BlockType::Empty));
        let mut inner = Body::new();
        ctx.repr.unbox_string(0, &mut inner);
        ctx.repr.unbox_string(w, &mut inner);
        inner.t(ctx.call(Rt::StrConcat));
        ctx.repr.box_string(inner, &mut f.body);
        f.body.t(Ins::Return);
        f.body.t(Ins::End);
    }
    f.body.t(Ins::Unreachable);
    f
}

/// `==`: same-type comparison, false across types. Research-grade minimum --
/// no coercion ladder. Identical on both sides, so it cannot tilt the slope.
fn equality(ctx: &Ctx<'_>) -> FnBuild {
    let w = ctx.width();
    let mut f = FnBuild::new(2 * w);

    ctx.repr.is_number(0, &mut f.body);
    ctx.repr.is_number(w, &mut f.body);
    f.body.t(Ins::I32And);
    f.body.t(Ins::If(BlockType::Empty));
    let mut inner = Body::new();
    ctx.repr.unbox_number(0, &mut inner);
    ctx.repr.unbox_number(w, &mut inner);
    inner.t(Ins::F64Eq);
    ctx.repr.box_bool(inner, &mut f.body);
    f.body.t(Ins::Return);
    f.body.t(Ins::End);

    ctx.repr.is_bool(0, &mut f.body);
    ctx.repr.is_bool(w, &mut f.body);
    f.body.t(Ins::I32And);
    f.body.t(Ins::If(BlockType::Empty));
    let mut inner = Body::new();
    ctx.repr.unbox_bool(0, &mut inner);
    ctx.repr.unbox_bool(w, &mut inner);
    inner.t(Ins::I32Eq);
    ctx.repr.box_bool(inner, &mut f.body);
    f.body.t(Ins::Return);
    f.body.t(Ins::End);

    if ctx.point_has_strings {
        ctx.repr.is_string(0, &mut f.body);
        ctx.repr.is_string(w, &mut f.body);
        f.body.t(Ins::I32And);
        f.body.t(Ins::If(BlockType::Empty));
        let mut inner = Body::new();
        ctx.repr.unbox_string(0, &mut inner);
        ctx.repr.unbox_string(w, &mut inner);
        inner.t(ctx.call(Rt::StrEq));
        ctx.repr.box_bool(inner, &mut f.body);
        f.body.t(Ins::Return);
        f.body.t(Ins::End);
    }

    ctx.repr.const_bool(false, &mut f.body);
    f
}

fn inequality(ctx: &Ctx<'_>) -> FnBuild {
    let w = ctx.width();
    let mut f = FnBuild::new(2 * w);
    let tmp = f.value_local(ctx.repr);
    load_local(w, 0, crate::ir::Origin::Runtime, &mut f.body);
    load_local(w, w, crate::ir::Origin::Runtime, &mut f.body);
    f.body.t(ctx.call(Rt::Eq));
    store_local(w, tmp, crate::ir::Origin::Runtime, &mut f.body);
    let mut inner = Body::new();
    ctx.repr.unbox_bool(tmp, &mut inner);
    inner.t(Ins::I32Eqz);
    ctx.repr.box_bool(inner, &mut f.body);
    f
}

/// ECMA-262 ToBoolean, over the types this point has. `+0`, `-0` and `NaN` are
/// the falsy numbers, which is why the number arm needs the value twice.
fn truthy(ctx: &Ctx<'_>) -> FnBuild {
    let w = ctx.width();
    let mut f = FnBuild::new(w);
    let sf = f.local(ValType::F64);

    ctx.repr.is_number(0, &mut f.body);
    f.body.t(Ins::If(BlockType::Empty));
    ctx.repr.unbox_number(0, &mut f.body);
    f.body.t(Ins::LocalTee(sf));
    f.body.t(Ins::F64Const(0.0));
    f.body.t(Ins::F64Ne);
    f.body.t(Ins::LocalGet(sf));
    f.body.t(Ins::LocalGet(sf));
    f.body.t(Ins::F64Eq);
    f.body.t(Ins::I32And);
    f.body.t(Ins::Return);
    f.body.t(Ins::End);

    ctx.repr.is_bool(0, &mut f.body);
    f.body.t(Ins::If(BlockType::Empty));
    ctx.repr.unbox_bool(0, &mut f.body);
    f.body.t(Ins::Return);
    f.body.t(Ins::End);

    if ctx.point_has_strings {
        ctx.repr.is_string(0, &mut f.body);
        f.body.t(Ins::If(BlockType::Empty));
        ctx.repr.unbox_string(0, &mut f.body);
        f.body.t(Ins::I32Load(2, 0));
        f.body.t(Ins::I32Const(0));
        f.body.t(Ins::I32Ne);
        f.body.t(Ins::Return);
        f.body.t(Ins::End);
    }

    f.body.t(Ins::I32Const(0));
    f
}

fn length(ctx: &Ctx<'_>) -> FnBuild {
    let mut f = FnBuild::new(ctx.width());
    let scratch = reserve_scratch(ctx, &mut f);
    let mut inner = Body::new();
    ctx.repr.unbox_string(0, &mut inner);
    inner.t(Ins::I32Load(2, 0));
    inner.t(Ins::F64ConvertI32S);
    ctx.repr.box_number(inner, scratch, &mut f.body);
    f
}

/// Bump allocation with no free and no collector: experiment scope, section 1.
/// Grows linear memory rather than trapping at the first page boundary.
fn alloc(ctx: &Ctx<'_>) -> FnBuild {
    let mut f = FnBuild::new(1);
    let p = f.local(ValType::I32);
    let g = ctx.heap_global;
    let b = &mut f.body;
    b.t(Ins::GlobalGet(g));
    b.t(Ins::LocalSet(p));
    b.t(Ins::GlobalGet(g));
    b.t(Ins::LocalGet(0));
    b.t(Ins::I32Const(3));
    b.t(Ins::I32Add);
    b.t(Ins::I32Const(-4));
    b.t(Ins::I32And);
    b.t(Ins::I32Add);
    b.t(Ins::GlobalSet(g));
    b.t(Ins::Block(BlockType::Empty));
    b.t(Ins::Loop(BlockType::Empty));
    b.t(Ins::MemorySize);
    b.t(Ins::I32Const(16));
    b.t(Ins::I32Shl);
    b.t(Ins::GlobalGet(g));
    b.t(Ins::I32GeU);
    b.t(Ins::BrIf(1));
    b.t(Ins::I32Const(1));
    b.t(Ins::MemoryGrow);
    b.t(Ins::I32Const(-1));
    b.t(Ins::I32Eq);
    b.t(Ins::If(BlockType::Empty));
    b.t(Ins::Unreachable);
    b.t(Ins::End);
    b.t(Ins::Br(0));
    b.t(Ins::End);
    b.t(Ins::End);
    b.t(Ins::LocalGet(p));
    f
}

/// `[len: i32][bytes]`, UTF-8, no interning, no 8/16-bit forms, no collector.
fn str_concat(ctx: &Ctx<'_>) -> FnBuild {
    let mut f = FnBuild::new(2);
    let la = f.local(ValType::I32);
    let lb = f.local(ValType::I32);
    let p = f.local(ValType::I32);
    let i = f.local(ValType::I32);
    let b = &mut f.body;
    b.t(Ins::LocalGet(0));
    b.t(Ins::I32Load(2, 0));
    b.t(Ins::LocalSet(la));
    b.t(Ins::LocalGet(1));
    b.t(Ins::I32Load(2, 0));
    b.t(Ins::LocalSet(lb));
    b.t(Ins::LocalGet(la));
    b.t(Ins::LocalGet(lb));
    b.t(Ins::I32Add);
    b.t(Ins::I32Const(4));
    b.t(Ins::I32Add);
    b.t(ctx.call(Rt::Alloc));
    b.t(Ins::LocalSet(p));
    b.t(Ins::LocalGet(p));
    b.t(Ins::LocalGet(la));
    b.t(Ins::LocalGet(lb));
    b.t(Ins::I32Add);
    b.t(Ins::I32Store(2, 0));
    copy_loop(b, 0, la, p, None, i);
    copy_loop(b, 1, lb, p, Some(la), i);
    b.t(Ins::LocalGet(p));
    f
}

/// `dst[base + k] = src[k]` for `k < len`, one byte at a time. A byte loop
/// rather than `memory.copy`: bulk memory is a post-MVP proposal, and the
/// experiment gains nothing from depending on it.
fn copy_loop(b: &mut Body, src: u32, len: u32, dst: u32, dst_shift: Option<u32>, i: u32) {
    b.t(Ins::I32Const(0));
    b.t(Ins::LocalSet(i));
    b.t(Ins::Block(BlockType::Empty));
    b.t(Ins::Loop(BlockType::Empty));
    b.t(Ins::LocalGet(i));
    b.t(Ins::LocalGet(len));
    b.t(Ins::I32GeU);
    b.t(Ins::BrIf(1));
    b.t(Ins::LocalGet(dst));
    if let Some(shift) = dst_shift {
        b.t(Ins::LocalGet(shift));
        b.t(Ins::I32Add);
    }
    b.t(Ins::LocalGet(i));
    b.t(Ins::I32Add);
    b.t(Ins::LocalGet(src));
    b.t(Ins::LocalGet(i));
    b.t(Ins::I32Add);
    b.t(Ins::I32Load8U(0, 4));
    b.t(Ins::I32Store8(0, 4));
    b.t(Ins::LocalGet(i));
    b.t(Ins::I32Const(1));
    b.t(Ins::I32Add);
    b.t(Ins::LocalSet(i));
    b.t(Ins::Br(0));
    b.t(Ins::End);
    b.t(Ins::End);
}

fn str_eq(_ctx: &Ctx<'_>) -> FnBuild {
    let mut f = FnBuild::new(2);
    let la = f.local(ValType::I32);
    let i = f.local(ValType::I32);
    let b = &mut f.body;
    b.t(Ins::LocalGet(0));
    b.t(Ins::I32Load(2, 0));
    b.t(Ins::LocalSet(la));
    b.t(Ins::LocalGet(la));
    b.t(Ins::LocalGet(1));
    b.t(Ins::I32Load(2, 0));
    b.t(Ins::I32Ne);
    b.t(Ins::If(BlockType::Empty));
    b.t(Ins::I32Const(0));
    b.t(Ins::Return);
    b.t(Ins::End);
    b.t(Ins::I32Const(0));
    b.t(Ins::LocalSet(i));
    b.t(Ins::Block(BlockType::Empty));
    b.t(Ins::Loop(BlockType::Empty));
    b.t(Ins::LocalGet(i));
    b.t(Ins::LocalGet(la));
    b.t(Ins::I32GeU);
    b.t(Ins::BrIf(1));
    b.t(Ins::LocalGet(0));
    b.t(Ins::LocalGet(i));
    b.t(Ins::I32Add);
    b.t(Ins::I32Load8U(0, 4));
    b.t(Ins::LocalGet(1));
    b.t(Ins::LocalGet(i));
    b.t(Ins::I32Add);
    b.t(Ins::I32Load8U(0, 4));
    b.t(Ins::I32Ne);
    b.t(Ins::If(BlockType::Empty));
    b.t(Ins::I32Const(0));
    b.t(Ins::Return);
    b.t(Ins::End);
    b.t(Ins::LocalGet(i));
    b.t(Ins::I32Const(1));
    b.t(Ins::I32Add);
    b.t(Ins::LocalSet(i));
    b.t(Ins::Br(0));
    b.t(Ins::End);
    b.t(Ins::End);
    b.t(Ins::I32Const(1));
    f
}

/// The scratch locals this representation needs, allocated once per function.
fn reserve_scratch(ctx: &Ctx<'_>, f: &mut FnBuild) -> u32 {
    let base = f.param_words + f.extra.len() as u32;
    for t in ctx.repr.scratch() {
        f.extra.push(*t);
    }
    base
}
