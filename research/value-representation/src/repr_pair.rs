//! V1: the two-word value, `(tag: i32, payload: i64)`.
//!
//! One JS value is two wasm values everywhere: two parameters per argument, two
//! results (wasm multi-value, which tinyvm supports, so nothing goes through
//! memory), two locals per variable, two operand slots on the stack.
//!
//! The payload is `i64` rather than `i32` because a JS Number is an IEEE-754
//! double and has to fit without a second indirection. A wasm32 pointer is an
//! `i32` zero-extended into the same field.

use tinyvm::Val;

use crate::ir::{Body, Ins, ValType};
use crate::repr::{HostVal, Repr, trap_unless};

pub const TAG_UNDEFINED: i32 = 0;
pub const TAG_NUMBER: i32 = 1;
pub const TAG_BOOL: i32 = 2;
pub const TAG_STRING: i32 = 3;

const SLOTS: &[ValType] = &[ValType::I32, ValType::I64];
const SCRATCH: &[ValType] = &[];

pub struct Pair;

impl Pair {
    /// `if (tag != want) unreachable`.
    fn require_tag(&self, base: u32, tag: i32, out: &mut Body) {
        out.r(Ins::LocalGet(base));
        out.r(Ins::I32Const(tag));
        out.r(Ins::I32Eq);
        trap_unless(out);
    }
    fn tag_is(&self, base: u32, tag: i32, out: &mut Body) {
        out.r(Ins::LocalGet(base));
        out.r(Ins::I32Const(tag));
        out.r(Ins::I32Eq);
    }
}

impl Repr for Pair {
    fn name(&self) -> &'static str {
        "V1-pair"
    }
    fn slots(&self) -> &'static [ValType] {
        SLOTS
    }
    fn scratch(&self) -> &'static [ValType] {
        SCRATCH
    }

    fn box_number(&self, inner: Body, _scratch_base: u32, out: &mut Body) {
        out.r(Ins::I32Const(TAG_NUMBER));
        out.append(inner);
        out.r(Ins::I64ReinterpretF64);
    }
    fn box_bool(&self, inner: Body, out: &mut Body) {
        out.r(Ins::I32Const(TAG_BOOL));
        out.append(inner);
        out.r(Ins::I64ExtendI32U);
    }
    fn box_string(&self, inner: Body, out: &mut Body) {
        out.r(Ins::I32Const(TAG_STRING));
        out.append(inner);
        out.r(Ins::I64ExtendI32U);
    }
    fn const_number(&self, value: f64, out: &mut Body) {
        out.r(Ins::I32Const(TAG_NUMBER));
        out.r(Ins::I64Const(value.to_bits() as i64));
    }
    fn const_bool(&self, value: bool, out: &mut Body) {
        out.r(Ins::I32Const(TAG_BOOL));
        out.r(Ins::I64Const(i64::from(value)));
    }
    fn const_undefined(&self, out: &mut Body) {
        out.r(Ins::I32Const(TAG_UNDEFINED));
        out.r(Ins::I64Const(0));
    }

    fn unbox_number(&self, base: u32, out: &mut Body) {
        self.require_tag(base, TAG_NUMBER, out);
        out.r(Ins::LocalGet(base + 1));
        out.r(Ins::F64ReinterpretI64);
    }
    fn unbox_bool(&self, base: u32, out: &mut Body) {
        self.require_tag(base, TAG_BOOL, out);
        out.r(Ins::LocalGet(base + 1));
        out.r(Ins::I32WrapI64);
    }
    fn unbox_string(&self, base: u32, out: &mut Body) {
        self.require_tag(base, TAG_STRING, out);
        out.r(Ins::LocalGet(base + 1));
        out.r(Ins::I32WrapI64);
    }
    fn is_number(&self, base: u32, out: &mut Body) {
        self.tag_is(base, TAG_NUMBER, out);
    }
    fn is_bool(&self, base: u32, out: &mut Body) {
        self.tag_is(base, TAG_BOOL, out);
    }
    fn is_string(&self, base: u32, out: &mut Body) {
        self.tag_is(base, TAG_STRING, out);
    }

    fn host_encode_number(&self, value: f64) -> Vec<Val> {
        // Bit-exact: the payload is the double's bits, so nothing is lost --
        // not the sign of a zero, not a NaN's payload.
        vec![Val::I32(TAG_NUMBER), Val::I64(value.to_bits() as i64)]
    }

    fn host_decode(&self, vals: &[Val]) -> Result<HostVal, String> {
        match vals {
            [Val::I32(tag), Val::I64(payload)] => {
                let p = *payload as u64;
                Ok(match *tag {
                    TAG_UNDEFINED => HostVal::Undefined,
                    TAG_NUMBER => HostVal::Number(f64::from_bits(p)),
                    TAG_BOOL => HostVal::Bool(p != 0),
                    TAG_STRING => HostVal::StrPtr(p as u32 as i32),
                    other => return Err(format!("V1: unknown tag {other}")),
                })
            }
            other => Err(format!(
                "V1: expected (i32, i64), got {} values",
                other.len()
            )),
        }
    }
}
