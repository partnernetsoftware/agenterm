//! V2: NaN-boxing. One JS value is one `f64`.
//!
//! A double is itself. Everything else hides in the negative quiet-NaN range,
//! which is `0xFFF8_0000_0000_0000 ..= 0xFFFF_FFFF_FFFF_FFFF` -- 2^51 bit
//! patterns that IEEE-754 all calls "NaN". Bits 48..50 are the tag, bits 0..47
//! the payload; a wasm32 pointer needs 32 of those 48.
//!
//! # Why every boxed double is canonicalised
//!
//! The range above is not free: it is where real NaNs live too. A double
//! arriving as `0xFFF8_0000_0000_0002` would otherwise read back as "string at
//! address 2" -- silent type confusion, the exact failure the load gate and
//! every trap in this stack exist to prevent. So [`Nanbox::box_number`] maps
//! every NaN onto one canonical positive quiet NaN before it can be stored.
//! This is what production NaN-boxing engines do, and it is the mechanism that
//! makes criterion 2 interesting: canonicalising is what *loses* a NaN payload.

use tinyvm::Val;

use crate::ir::{Body, Ins, ValType};
use crate::repr::{HostVal, Repr, trap_unless};

/// Lowest bit pattern that is a boxed non-number. Everything strictly below is
/// an ordinary double, `-Infinity` (`0xFFF0…`) and `-0` (`0x8000…`) included.
pub const BOX_BASE: u64 = 0xFFF8_0000_0000_0000;
/// Selects "is it boxed" plus the three tag bits in one comparison.
const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;
const TAG_UNDEFINED: u64 = BOX_BASE;
const TAG_BOOL: u64 = BOX_BASE | (1 << 48);
const TAG_STRING: u64 = BOX_BASE | (2 << 48);
/// The one NaN this representation can store: positive quiet, no payload.
pub const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;

const SLOTS: &[ValType] = &[ValType::F64];
/// One f64 scratch, needed by the canonicalisation select.
const SCRATCH: &[ValType] = &[ValType::F64];

pub struct Nanbox;

/// The double a host-supplied number becomes once this representation has
/// stored it. Exposed so the harness can state criterion 2 as a measurement
/// rather than as a claim.
pub fn canonicalise(value: f64) -> f64 {
    if value.is_nan() {
        f64::from_bits(CANONICAL_NAN)
    } else {
        value
    }
}

impl Nanbox {
    /// -> `i64` bits of the value at `base`.
    fn bits(&self, base: u32, out: &mut Body) {
        out.r(Ins::LocalGet(base));
        out.r(Ins::I64ReinterpretF64);
    }
    /// -> `i32` 1 when the value carries exactly this tag.
    fn tag_is(&self, base: u32, tag: u64, out: &mut Body) {
        self.bits(base, out);
        out.r(Ins::I64Const(TAG_MASK as i64));
        out.r(Ins::I64And);
        out.r(Ins::I64Const(tag as i64));
        out.r(Ins::I64Eq);
    }
    fn require_tag(&self, base: u32, tag: u64, out: &mut Body) {
        self.tag_is(base, tag, out);
        trap_unless(out);
    }
    /// A boxed payload is always in the low 32 bits on wasm32, so the pointer
    /// falls out of a wrap with no mask.
    fn payload_i32(&self, base: u32, out: &mut Body) {
        self.bits(base, out);
        out.r(Ins::I32WrapI64);
    }
    fn box_with_tag(&self, tag: u64, inner: Body, out: &mut Body) {
        out.r(Ins::I64Const(tag as i64));
        out.append(inner);
        out.r(Ins::I64ExtendI32U);
        out.r(Ins::I64Or);
        out.r(Ins::F64ReinterpretI64);
    }
}

impl Repr for Nanbox {
    fn name(&self) -> &'static str {
        "V2-nanbox"
    }
    fn slots(&self) -> &'static [ValType] {
        SLOTS
    }
    fn scratch(&self) -> &'static [ValType] {
        SCRATCH
    }

    fn box_number(&self, inner: Body, scratch_base: u32, out: &mut Body) {
        // `select` picks the first operand when the condition is non-zero, so
        // the canonical NaN is pushed before the payload -- the same bottom-up
        // shape the two-word side uses for its tag.
        out.r(Ins::F64Const(f64::from_bits(CANONICAL_NAN)));
        out.append(inner);
        out.r(Ins::LocalTee(scratch_base));
        out.r(Ins::LocalGet(scratch_base));
        out.r(Ins::LocalGet(scratch_base));
        out.r(Ins::F64Ne);
        out.r(Ins::Select);
    }
    fn box_bool(&self, inner: Body, out: &mut Body) {
        self.box_with_tag(TAG_BOOL, inner, out);
    }
    fn box_string(&self, inner: Body, out: &mut Body) {
        self.box_with_tag(TAG_STRING, inner, out);
    }
    fn const_number(&self, value: f64, out: &mut Body) {
        out.r(Ins::F64Const(canonicalise(value)));
    }
    fn const_bool(&self, value: bool, out: &mut Body) {
        out.r(Ins::F64Const(f64::from_bits(TAG_BOOL | u64::from(value))));
    }
    fn const_undefined(&self, out: &mut Body) {
        out.r(Ins::F64Const(f64::from_bits(TAG_UNDEFINED)));
    }

    fn unbox_number(&self, base: u32, out: &mut Body) {
        self.is_number(base, out);
        trap_unless(out);
        out.r(Ins::LocalGet(base));
    }
    fn unbox_bool(&self, base: u32, out: &mut Body) {
        self.require_tag(base, TAG_BOOL, out);
        self.payload_i32(base, out);
    }
    fn unbox_string(&self, base: u32, out: &mut Body) {
        self.require_tag(base, TAG_STRING, out);
        self.payload_i32(base, out);
    }
    fn is_number(&self, base: u32, out: &mut Body) {
        self.bits(base, out);
        out.r(Ins::I64Const(BOX_BASE as i64));
        out.r(Ins::I64LtU);
    }
    fn is_bool(&self, base: u32, out: &mut Body) {
        self.tag_is(base, TAG_BOOL, out);
    }
    fn is_string(&self, base: u32, out: &mut Body) {
        self.tag_is(base, TAG_STRING, out);
    }

    fn host_encode_number(&self, value: f64) -> Vec<Val> {
        // The door is the representation. A NaN payload is lost here, before
        // any guest instruction runs -- see criterion 2 in RESULTS.md.
        vec![Val::F64(canonicalise(value))]
    }

    fn host_decode(&self, vals: &[Val]) -> Result<HostVal, String> {
        match vals {
            [Val::F64(x)] => {
                let bits = x.to_bits();
                if bits < BOX_BASE {
                    return Ok(HostVal::Number(*x));
                }
                Ok(match bits & TAG_MASK {
                    TAG_UNDEFINED => HostVal::Undefined,
                    TAG_BOOL => HostVal::Bool(bits & 0xFFFF_FFFF != 0),
                    TAG_STRING => HostVal::StrPtr(bits as u32 as i32),
                    other => return Err(format!("V2: unknown tag {:#x}", other >> 48)),
                })
            }
            other => Err(format!("V2: expected one f64, got {} values", other.len())),
        }
    }
}
