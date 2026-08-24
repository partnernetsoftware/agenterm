//! The one layer that has two implementations.
//!
//! Everything above this file -- lexer, parser, AST, lowering, emitted runtime,
//! encoder, corpus, expected-value table -- is shared verbatim between the two
//! variants. That is experiment constraint 1: two independent prototypes would
//! introduce "which one got more effort" as a confounder and duplicate 90% of
//! the work.
//!
//! # What a representation must provide
//!
//! Constructors take the payload as an already-built [`Body`] rather than
//! expecting it on the stack, because a two-word value is naturally built
//! bottom-up (tag first, then payload) and a stack-based constructor would
//! force a scratch local on the two-word side for no reason other than the API
//! shape. The NaN-boxing side gets the same courtesy: its canonical-NaN
//! constant is pushed before the payload too.
//!
//! Accessors read from *locals*, not from the stack. Type dispatch needs to
//! look at a value more than once (`is_string(a) && is_string(b)`, then
//! `to_string(a)`), and a stack-only accessor would force a spill on both
//! sides. Every JS value that gets inspected is already a parameter or a local.

use tinyvm::Val;

use crate::ir::{Body, Ins, Origin, ValType};

/// A JavaScript value as the host sees it. `StrPtr` is a guest pointer the
/// harness resolves against the instance's memory; the guest heap layout is
/// representation-independent, so both variants hand back the same pointer.
#[derive(Clone, Debug, PartialEq)]
pub enum HostVal {
    Undefined,
    Number(f64),
    Bool(bool),
    StrPtr(i32),
}

/// A JavaScript value in the expected-value table. One table, both variants.
#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    Undefined,
    Number(f64),
    /// Bit-exact number comparison: `-0` must not match `+0`, and a NaN
    /// pattern must match exactly. Criterion 2 lives here.
    NumberBits(u64),
    Bool(bool),
    Str(&'static str),
}

pub trait Repr {
    fn name(&self) -> &'static str;

    /// The wasm value types one JS value occupies, in stack order.
    fn slots(&self) -> &'static [ValType];

    /// Scratch locals any function that boxes a number must declare.
    fn scratch(&self) -> &'static [ValType];

    fn width(&self) -> u32 {
        self.slots().len() as u32
    }

    // ---- constructors -------------------------------------------------

    /// `inner` leaves exactly one `f64` on the stack. Result: one JS Number.
    fn box_number(&self, inner: Body, scratch_base: u32, out: &mut Body);
    /// `inner` leaves exactly one `i32` (0 or 1). Result: one JS Boolean.
    fn box_bool(&self, inner: Body, out: &mut Body);
    /// `inner` leaves exactly one `i32` guest pointer. Result: one JS String.
    fn box_string(&self, inner: Body, out: &mut Body);
    fn const_number(&self, value: f64, out: &mut Body);
    fn const_bool(&self, value: bool, out: &mut Body);
    fn const_undefined(&self, out: &mut Body);

    // ---- accessors, reading the value stored at local `base` -----------

    /// -> `f64`. Traps when the value is not a Number.
    fn unbox_number(&self, base: u32, out: &mut Body);
    /// -> `i32` 0/1. Traps when the value is not a Boolean.
    fn unbox_bool(&self, base: u32, out: &mut Body);
    /// -> `i32` guest pointer. Traps when the value is not a String.
    fn unbox_string(&self, base: u32, out: &mut Body);
    /// -> `i32` 0/1.
    fn is_number(&self, base: u32, out: &mut Body);
    fn is_bool(&self, base: u32, out: &mut Body);
    fn is_string(&self, base: u32, out: &mut Body);

    // ---- the host door -------------------------------------------------

    /// Encode a host-supplied JS value into this representation's call ABI.
    /// The NaN-boxing side canonicalises here, which is exactly where its
    /// criterion-2 behaviour becomes observable.
    fn host_encode_number(&self, value: f64) -> Vec<Val>;
    fn host_decode(&self, vals: &[Val]) -> Result<HostVal, String>;
}

/// Push every word of the JS value held at local `base`.
///
/// Tagged with the *caller's* origin, not `Repr`: reading a variable is program
/// code whose width happens to be representation-determined. The width effect
/// is reported by criteria 4 and 5, which is where it belongs; folding it into
/// L1 would make L1 mean "representation plus every variable read".
pub fn load_local(width: u32, base: u32, origin: Origin, out: &mut Body) {
    for k in 0..width {
        push(out, origin, Ins::LocalGet(base + k));
    }
}

/// Pop one JS value off the stack into the locals starting at `base`.
pub fn store_local(width: u32, base: u32, origin: Origin, out: &mut Body) {
    for k in (0..width).rev() {
        push(out, origin, Ins::LocalSet(base + k));
    }
}

/// Drop one JS value from the stack.
pub fn drop_value(width: u32, origin: Origin, out: &mut Body) {
    for _ in 0..width {
        push(out, origin, Ins::Drop);
    }
}

pub fn push(out: &mut Body, origin: Origin, ins: Ins) {
    match origin {
        Origin::Repr => out.r(ins),
        Origin::Runtime => out.t(ins),
        Origin::Corpus => out.c(ins),
    }
}

/// `if (!cond) unreachable` -- the trap every accessor raises on a type it was
/// not handed. Emitted by the representation layer, so it counts as L1.
pub fn trap_unless(out: &mut Body) {
    out.r(Ins::I32Eqz);
    out.r(Ins::If(crate::ir::BlockType::Empty));
    out.r(Ins::Unreachable);
    out.r(Ins::End);
}
