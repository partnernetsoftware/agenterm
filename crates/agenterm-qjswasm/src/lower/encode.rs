//! [`ir::Module`] -> standard `.wasm` bytes.
//!
//! Written by hand, and that is a requirement rather than an accident: the
//! output has to clear tinyvm's load gate, which is strict about the things a
//! general-purpose encoder crate hides -- canonical section order, minimal
//! LEB128, the exact `end` that terminates a function expression, memarg
//! alignment (when memory arrives), signed-LEB range (when `i64` arrives). We
//! own that correctness because the product depends on it; a dependency would
//! only let us assume it.
//!
//! The encoder is total. Anything it could reject, [`super::parse`] already
//! rejected with a diagnostic that names a capability boundary, so nothing here
//! returns a `Result` and no failure reaches the user as bytes.

use super::ir::{ExportKind, FuncType, Ins, Module, ValType};

/// `\0asm` plus version 1, little-endian. The eight bytes every module starts
/// with.
const HEADER: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

// Section ids, in the order the specification requires them to appear. Only
// the four M0 needs are named; the gaps are the sections it does not emit.
const SECTION_TYPE: u8 = 1;
const SECTION_FUNCTION: u8 = 3;
const SECTION_EXPORT: u8 = 7;
const SECTION_CODE: u8 = 10;

/// Marks the start of a function type in the type section.
const FUNC_TYPE_TAG: u8 = 0x60;
/// Terminates an expression -- the `end` opcode.
const END: u8 = 0x0b;

pub(crate) fn encode(module: &Module) -> Vec<u8> {
    let mut out = HEADER.to_vec();
    section(&mut out, SECTION_TYPE, |body| {
        vector(body, &module.types, func_type);
    });
    section(&mut out, SECTION_FUNCTION, |body| {
        vector(body, &module.funcs, |body, func| {
            unsigned(body, func.type_index);
        });
    });
    section(&mut out, SECTION_EXPORT, |body| {
        vector(body, &module.exports, |body, export| {
            name(body, &export.name);
            body.push(match export.kind {
                ExportKind::Func => 0x00,
            });
            unsigned(body, export.index);
        });
    });
    section(&mut out, SECTION_CODE, |body| {
        vector(body, &module.funcs, |body, func| {
            // Each entry is size-prefixed, and the size covers the locals
            // declaration plus the expression. Build it, then measure it.
            let mut code = Vec::new();
            vector(&mut code, &func.locals, |code, (count, ty)| {
                unsigned(code, *count);
                code.push(val_type(*ty));
            });
            for ins in &func.body {
                instruction(&mut code, ins);
            }
            code.push(END);
            unsigned(body, code.len() as u32);
            body.extend_from_slice(&code);
        });
    });
    out
}

/// A section: its id, its byte length, then its contents. The length is
/// measured rather than predicted, which is why the body is built first.
fn section(out: &mut Vec<u8>, id: u8, build: impl FnOnce(&mut Vec<u8>)) {
    let mut body = Vec::new();
    build(&mut body);
    out.push(id);
    unsigned(out, body.len() as u32);
    out.extend_from_slice(&body);
}

/// A wasm vector: an element count, then the elements.
fn vector<T>(out: &mut Vec<u8>, items: &[T], mut element: impl FnMut(&mut Vec<u8>, &T)) {
    unsigned(out, items.len() as u32);
    for item in items {
        element(out, item);
    }
}

fn func_type(out: &mut Vec<u8>, ty: &FuncType) {
    out.push(FUNC_TYPE_TAG);
    vector(out, &ty.params, |out, t| out.push(val_type(*t)));
    vector(out, &ty.results, |out, t| out.push(val_type(*t)));
}

fn val_type(ty: ValType) -> u8 {
    match ty {
        ValType::I32 => 0x7f,
    }
}

/// A name: its byte length, then its UTF-8 bytes. Rust `str` is already valid
/// UTF-8, which is exactly what the load gate checks for.
fn name(out: &mut Vec<u8>, text: &str) {
    unsigned(out, text.len() as u32);
    out.extend_from_slice(text.as_bytes());
}

fn instruction(out: &mut Vec<u8>, ins: &Ins) {
    match ins {
        Ins::I32Const(value) => {
            out.push(0x41);
            signed_32(out, *value);
        }
        Ins::LocalGet(index) => {
            out.push(0x20);
            unsigned(out, *index);
        }
        Ins::I32Add => out.push(0x6a),
        Ins::I32Sub => out.push(0x6b),
        Ins::I32Mul => out.push(0x6c),
        Ins::I32DivS => out.push(0x6d),
        Ins::I32RemS => out.push(0x6f),
    }
}

/// Unsigned LEB128, minimal length. "Minimal" matters: a validator may reject
/// a padded encoding, and a padded one is never what a canonical producer
/// emits.
fn unsigned(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Signed LEB128 for an `i32`, minimal length.
///
/// The loop stops when the remaining bits are all copies of the sign bit *and*
/// the byte just written carries that sign in bit 6. Dropping either half of
/// that condition is the classic way to encode `-64` as `0x40` (which reads
/// back as `64`) or to emit a redundant trailing byte.
fn signed_32(out: &mut Vec<u8>, value: i32) {
    let mut remaining = value;
    loop {
        let byte = (remaining & 0x7f) as u8;
        // Arithmetic shift: the sign extends, so a negative value converges on
        // -1 rather than on 0.
        remaining >>= 7;
        let sign_bit_set = byte & 0x40 != 0;
        if (remaining == 0 && !sign_bit_set) || (remaining == -1 && sign_bit_set) {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uleb(value: u32) -> Vec<u8> {
        let mut out = Vec::new();
        unsigned(&mut out, value);
        out
    }

    fn sleb(value: i32) -> Vec<u8> {
        let mut out = Vec::new();
        signed_32(&mut out, value);
        out
    }

    #[test]
    fn unsigned_leb128_is_minimal() {
        assert_eq!(uleb(0), [0x00]);
        assert_eq!(uleb(1), [0x01]);
        assert_eq!(uleb(63), [0x3f]);
        assert_eq!(uleb(64), [0x40]);
        assert_eq!(uleb(127), [0x7f]);
        assert_eq!(uleb(128), [0x80, 0x01]);
        assert_eq!(uleb(624_485), [0xe5, 0x8e, 0x26]);
        assert_eq!(uleb(u32::MAX), [0xff, 0xff, 0xff, 0xff, 0x0f]);
    }

    #[test]
    fn signed_leb128_is_minimal_and_round_trips() {
        assert_eq!(sleb(0), [0x00]);
        assert_eq!(sleb(1), [0x01]);
        assert_eq!(sleb(63), [0x3f]);
        // 64 needs a second byte: 0x40 alone has bit 6 set and would read back
        // as -64. This is the case a naive encoder gets wrong.
        assert_eq!(sleb(64), [0xc0, 0x00]);
        assert_eq!(sleb(-1), [0x7f]);
        assert_eq!(sleb(-64), [0x40]);
        assert_eq!(sleb(-65), [0xbf, 0x7f]);
        assert_eq!(sleb(-123_456), [0xc0, 0xbb, 0x78]);
        assert_eq!(sleb(i32::MAX), [0xff, 0xff, 0xff, 0xff, 0x07]);
        assert_eq!(sleb(i32::MIN), [0x80, 0x80, 0x80, 0x80, 0x78]);

        // Every encoding decodes back to what it came from, and none is
        // longer than it has to be.
        for value in [
            i32::MIN,
            i32::MIN + 1,
            -1_000_000,
            -128,
            -65,
            -64,
            -1,
            0,
            1,
            63,
            64,
            127,
            128,
            1_000_000,
            i32::MAX - 1,
            i32::MAX,
        ] {
            let bytes = sleb(value);
            assert_eq!(decode_signed(&bytes), value, "round trip of {value}");
            assert_minimal(value, &bytes);
        }
    }

    /// A signed LEB128 encoding is minimal exactly when its last byte is not a
    /// pure sign extension of the one before it: a trailing `0x00` after a byte
    /// with bit 6 clear, or a trailing `0x7f` after a byte with bit 6 set,
    /// could both be dropped without changing the value.
    fn assert_minimal(value: i32, bytes: &[u8]) {
        if bytes.len() < 2 {
            return;
        }
        let last = bytes[bytes.len() - 1];
        let previous_is_negative = bytes[bytes.len() - 2] & 0x40 != 0;
        assert!(
            !(last == 0x00 && !previous_is_negative) && !(last == 0x7f && previous_is_negative),
            "{value} encoded with a redundant trailing byte: {bytes:02x?}"
        );
    }

    fn decode_signed(bytes: &[u8]) -> i32 {
        let mut result: i64 = 0;
        let mut shift = 0;
        for (i, byte) in bytes.iter().enumerate() {
            result |= i64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                assert_eq!(i, bytes.len() - 1, "continuation byte past the end");
                if shift < 64 && byte & 0x40 != 0 {
                    result |= -1i64 << shift;
                }
                break;
            }
        }
        result as i32
    }
}
