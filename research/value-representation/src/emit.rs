//! [`Program`] -> [`Module`]. Shared by both variants.
//!
//! Nothing in the lowering knows what a value looks like. It asks [`Repr`] for
//! constants and calls the emitted runtime for every operator, so the whole
//! difference between the two products is inside the two files that implement
//! that one trait.

use std::collections::BTreeMap;

use crate::ast::{BinOp, Expr, FuncDecl, Program, Stmt, UnOp};
use crate::ir::{BlockType, Export, Func, FuncType, Global, Ins, Module, Origin, ValType};
use crate::repr::{Repr, load_local, store_local};
use crate::runtime::{Ctx, FnBuild, P1_SET, P2_SET, Rt, index_of};

/// Which point on the growth axis a product is built at.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Point {
    /// Numbers only. The M2 "integer world": locals, assignment, `if`/`while`,
    /// function declaration, call and return -- and no heap.
    P1,
    /// Numbers and strings: the first heap type.
    P2,
}

impl Point {
    pub fn label(self) -> &'static str {
        match self {
            Point::P1 => "P1",
            Point::P2 => "P2",
        }
    }
    fn set(self) -> &'static [Rt] {
        match self {
            Point::P1 => P1_SET,
            Point::P2 => P2_SET,
        }
    }
    fn has_strings(self) -> bool {
        self == Point::P2
    }
}

/// Byte 0..7 is left unmapped so a null pointer is never a valid string.
const DATA_ORIGIN: u32 = 8;
const MEMORY_MIN_PAGES: u32 = 1;
const MEMORY_MAX_PAGES: u32 = 16;

pub fn lower(program: &Program, repr: &dyn Repr, point: Point) -> Result<Module, String> {
    lower_with(program, repr, point, false)
}

/// [`lower`] with the `__add` dispatch-order sensitivity switch.
pub fn lower_with(
    program: &Program,
    repr: &dyn Repr,
    point: Point,
    add_number_first: bool,
) -> Result<Module, String> {
    let mut literals = Literals::default();
    for f in &program.funcs {
        for s in &f.body {
            collect_stmt(s, &mut literals)?;
        }
    }
    if !literals.order.is_empty() && !point.has_strings() {
        return Err("this program needs strings, and P1 has none".to_string());
    }

    let mut module = Module::default();
    let heap_global = 0;
    let ctx = Ctx {
        repr,
        set: point.set(),
        heap_global,
        point_has_strings: point.has_strings(),
        add_number_first,
    };

    let runtime = crate::runtime::build(&ctx, &mut module.types);
    let runtime_count = runtime.len() as u32;
    module.funcs = runtime;

    let names: BTreeMap<&str, u32> = program
        .funcs
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name.as_str(), runtime_count + i as u32))
        .collect();

    for decl in &program.funcs {
        let func = lower_func(decl, &ctx, &names, &literals, &mut module.types)?;
        module.funcs.push(func);
    }

    let main = *names
        .get("main")
        .ok_or_else(|| "a corpus program must declare `main`".to_string())?;
    module.exports.push(Export {
        name: "main".to_string(),
        func_index: main,
    });

    if point.has_strings() {
        let (bytes, heap_start) = literals.layout();
        module.memory = Some((MEMORY_MIN_PAGES, MEMORY_MAX_PAGES));
        module.globals.push(Global {
            ty: ValType::I32,
            mutable: true,
            init_i32: heap_start as i32,
        });
        if !bytes.is_empty() {
            module.data = Some((DATA_ORIGIN, bytes));
        }
    }
    Ok(module)
}

/// String literals, in first-appearance order, deduplicated by content.
#[derive(Default)]
struct Literals {
    order: Vec<String>,
    offsets: BTreeMap<String, u32>,
}

impl Literals {
    fn add(&mut self, s: &str) {
        if !self.offsets.contains_key(s) {
            let offset = self.next_offset();
            self.offsets.insert(s.to_string(), offset);
            self.order.push(s.to_string());
        }
    }
    fn next_offset(&self) -> u32 {
        let mut at = DATA_ORIGIN;
        for s in &self.order {
            at += record_size(s);
        }
        at
    }
    fn get(&self, s: &str) -> u32 {
        self.offsets[s]
    }
    /// `[len: i32][bytes]`, each record 4-byte aligned. Returns the segment and
    /// the first free address after it, which is where the bump heap starts.
    fn layout(&self) -> (Vec<u8>, u32) {
        let mut bytes = Vec::new();
        for s in &self.order {
            bytes.extend_from_slice(&(s.len() as u32).to_le_bytes());
            bytes.extend_from_slice(s.as_bytes());
            while bytes.len() % 4 != 0 {
                bytes.push(0);
            }
        }
        (bytes.clone(), DATA_ORIGIN + bytes.len() as u32)
    }
}

fn record_size(s: &str) -> u32 {
    let raw = 4 + s.len() as u32;
    raw.div_ceil(4) * 4
}

fn collect_stmt(s: &Stmt, out: &mut Literals) -> Result<(), String> {
    match s {
        Stmt::Let(_, e) | Stmt::Assign(_, e) | Stmt::Return(e) => collect_expr(e, out),
        Stmt::If(c, a, b) => {
            collect_expr(c, out)?;
            for s in a.iter().chain(b) {
                collect_stmt(s, out)?;
            }
            Ok(())
        }
        Stmt::While(c, body) => {
            collect_expr(c, out)?;
            for s in body {
                collect_stmt(s, out)?;
            }
            Ok(())
        }
    }
}

fn collect_expr(e: &Expr, out: &mut Literals) -> Result<(), String> {
    match e {
        Expr::Num(_) | Expr::Var(_) => Ok(()),
        Expr::Str(s) => {
            out.add(s);
            Ok(())
        }
        Expr::Call(_, args) => {
            for a in args {
                collect_expr(a, out)?;
            }
            Ok(())
        }
        Expr::Unary(_, inner) => collect_expr(inner, out),
        Expr::Binary(_, l, r) => {
            collect_expr(l, out)?;
            collect_expr(r, out)
        }
    }
}

struct Scope {
    /// JS name -> base local index.
    slots: BTreeMap<String, u32>,
}

fn lower_func(
    decl: &FuncDecl,
    ctx: &Ctx<'_>,
    names: &BTreeMap<&str, u32>,
    literals: &Literals,
    types: &mut Vec<FuncType>,
) -> Result<Func, String> {
    let w = ctx.repr.width();
    let mut build = FnBuild::new(w * decl.params.len() as u32);
    let mut scope = Scope {
        slots: BTreeMap::new(),
    };
    for (i, p) in decl.params.iter().enumerate() {
        scope.slots.insert(p.clone(), w * i as u32);
    }
    // One flat function scope: no block scoping, no shadowing. A research-grade
    // simplification, applied identically to both variants.
    let mut declared = Vec::new();
    for s in &decl.body {
        collect_lets(s, &mut declared);
    }
    for name in declared {
        if let std::collections::btree_map::Entry::Vacant(slot) = scope.slots.entry(name) {
            let base = build.value_local(ctx.repr);
            slot.insert(base);
        }
    }
    // No representation scratch here. A corpus function never calls
    // `box_number` -- every boxing of a computed number happens inside a
    // runtime helper, and literals are boxed at compile time. Reserving one
    // anyway would charge the NaN-boxing side an unused local in every
    // function, which would inflate exactly the criterion-4 number this
    // experiment turns on.
    debug_assert!(
        ctx.repr.scratch().len() <= 1,
        "a wider scratch set would need a per-function decision here"
    );

    let mut em = Emitter {
        ctx,
        names,
        literals,
        scope: &scope,
    };
    let mut body = std::mem::take(&mut build.body);
    for s in &decl.body {
        em.stmt(s, &mut body)?;
    }
    // A function that falls off its end yields `undefined`. When the body ended
    // in `return`, this is dead code the validator accepts polymorphically.
    ctx.repr.const_undefined(&mut body);
    build.body = body;

    let ty = FuncType {
        params: (0..decl.params.len())
            .flat_map(|_| ctx.repr.slots().to_vec())
            .collect(),
        results: ctx.repr.slots().to_vec(),
    };
    let type_index = if let Some(i) = types.iter().position(|t| *t == ty) {
        i as u32
    } else {
        types.push(ty);
        (types.len() - 1) as u32
    };

    Ok(Func {
        name: decl.name.clone(),
        type_index,
        locals: build.local_groups(),
        body: build.body,
        is_runtime: false,
    })
}

fn collect_lets(s: &Stmt, out: &mut Vec<String>) {
    match s {
        Stmt::Let(n, _) => out.push(n.clone()),
        Stmt::If(_, a, b) => {
            for s in a.iter().chain(b) {
                collect_lets(s, out);
            }
        }
        Stmt::While(_, body) => {
            for s in body {
                collect_lets(s, out);
            }
        }
        Stmt::Assign(_, _) | Stmt::Return(_) => {}
    }
}

struct Emitter<'a> {
    ctx: &'a Ctx<'a>,
    names: &'a BTreeMap<&'a str, u32>,
    literals: &'a Literals,
    scope: &'a Scope,
}

impl Emitter<'_> {
    fn stmt(&mut self, s: &Stmt, out: &mut crate::ir::Body) -> Result<(), String> {
        let w = self.ctx.repr.width();
        match s {
            Stmt::Let(name, e) | Stmt::Assign(name, e) => {
                self.expr(e, out)?;
                let base = *self
                    .scope
                    .slots
                    .get(name)
                    .ok_or_else(|| format!("assignment to undeclared `{name}`"))?;
                store_local(w, base, Origin::Corpus, out);
            }
            Stmt::Return(e) => {
                self.expr(e, out)?;
                out.c(Ins::Return);
            }
            Stmt::If(cond, then, otherwise) => {
                self.expr(cond, out)?;
                out.c(self.ctx.call(Rt::Truthy));
                out.c(Ins::If(BlockType::Empty));
                for s in then {
                    self.stmt(s, out)?;
                }
                if !otherwise.is_empty() {
                    out.c(Ins::Else);
                    for s in otherwise {
                        self.stmt(s, out)?;
                    }
                }
                out.c(Ins::End);
            }
            Stmt::While(cond, body) => {
                out.c(Ins::Block(BlockType::Empty));
                out.c(Ins::Loop(BlockType::Empty));
                self.expr(cond, out)?;
                out.c(self.ctx.call(Rt::Truthy));
                out.c(Ins::I32Eqz);
                out.c(Ins::BrIf(1));
                for s in body {
                    self.stmt(s, out)?;
                }
                out.c(Ins::Br(0));
                out.c(Ins::End);
                out.c(Ins::End);
            }
        }
        Ok(())
    }

    fn expr(&mut self, e: &Expr, out: &mut crate::ir::Body) -> Result<(), String> {
        let w = self.ctx.repr.width();
        match e {
            Expr::Num(v) => self.ctx.repr.const_number(*v, out),
            Expr::Str(s) => {
                let mut inner = crate::ir::Body::new();
                inner.c(Ins::I32Const(self.literals.get(s) as i32));
                self.ctx.repr.box_string(inner, out);
            }
            Expr::Var(name) => {
                let base = *self
                    .scope
                    .slots
                    .get(name)
                    .ok_or_else(|| format!("unresolved name `{name}`"))?;
                load_local(w, base, Origin::Corpus, out);
            }
            Expr::Call(name, args) => {
                for a in args {
                    self.expr(a, out)?;
                }
                if name == "len" {
                    if args.len() != 1 {
                        return Err("len() takes one argument".to_string());
                    }
                    if !self.ctx.point_has_strings {
                        return Err("len() needs strings, and P1 has none".to_string());
                    }
                    out.c(Ins::Call(index_of(self.ctx.set, Rt::Len)));
                } else {
                    let index = *self
                        .names
                        .get(name.as_str())
                        .ok_or_else(|| format!("call to unknown function `{name}`"))?;
                    out.c(Ins::Call(index));
                }
            }
            Expr::Unary(UnOp::Neg, inner) => {
                self.expr(inner, out)?;
                out.c(self.ctx.call(Rt::Neg));
            }
            Expr::Binary(op, l, r) => {
                self.expr(l, out)?;
                self.expr(r, out)?;
                out.c(self.ctx.call(match op {
                    BinOp::Add => Rt::Add,
                    BinOp::Sub => Rt::Sub,
                    BinOp::Mul => Rt::Mul,
                    BinOp::Div => Rt::Div,
                    BinOp::Lt => Rt::Lt,
                    BinOp::Le => Rt::Le,
                    BinOp::Gt => Rt::Gt,
                    BinOp::Ge => Rt::Ge,
                    BinOp::Eq => Rt::Eq,
                    BinOp::Ne => Rt::Ne,
                }));
            }
        }
        Ok(())
    }
}
