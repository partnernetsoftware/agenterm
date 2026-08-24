//! Decisive experiment: which universal value representation should the
//! `.qjs -> .wasm` compiler use?
//!
//! * **V1** a two-word `(tag: i32, payload: i64)` pair -- [`repr_pair`]
//! * **V2** single-`f64` NaN-boxing -- [`repr_nanbox`]
//!
//! Specification: `plan/design-value-representation-experiment.md`. Results and
//! rerun commands: `RESULTS.md` next to this file.
//!
//! ```text
//! source --lex--> tokens --parse--> AST --emit--> wasm IR --encode--> bytes
//!                                          |
//!                                          +-- Repr (the only forked layer)
//! ```
//!
//! Everything except `Repr` is written once. Two independent prototypes would
//! have made "which one got more effort" a confounder, which is why the
//! specification forbids them.

pub mod ast;
pub mod emit;
pub mod encode;
pub mod harness;
pub mod ir;
pub mod lex;
pub mod parse;
pub mod repr;
pub mod repr_nanbox;
pub mod repr_pair;
pub mod runtime;

pub use emit::Point;
pub use encode::SizeReport;
pub use repr::{Expect, HostVal, Repr};

/// The two variants under test.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Variant {
    Pair,
    Nanbox,
}

impl Variant {
    pub const ALL: [Variant; 2] = [Variant::Pair, Variant::Nanbox];

    pub fn label(self) -> &'static str {
        match self {
            Variant::Pair => "V1",
            Variant::Nanbox => "V2",
        }
    }
    pub fn full_name(self) -> &'static str {
        match self {
            Variant::Pair => "V1 two-word (tag:i32, payload:i64)",
            Variant::Nanbox => "V2 NaN-boxed f64",
        }
    }
    pub fn repr(self) -> &'static dyn Repr {
        match self {
            Variant::Pair => &repr_pair::Pair,
            Variant::Nanbox => &repr_nanbox::Nanbox,
        }
    }
}

pub struct Product {
    pub wasm: Vec<u8>,
    pub size: SizeReport,
}

/// Compile one corpus program for one variant at one point.
pub fn compile(source: &str, variant: Variant, point: Point) -> Result<Product, String> {
    compile_with(source, variant, point, false)
}

/// [`compile`] with the `__add` dispatch-order sensitivity switch. Applied to
/// both variants at once or to neither -- it is a property of the lowering, not
/// of a representation.
pub fn compile_with(
    source: &str,
    variant: Variant,
    point: Point,
    add_number_first: bool,
) -> Result<Product, String> {
    let tokens = lex::tokenize(source)?;
    let program = parse::parse(&tokens)?;
    let module = emit::lower_with(&program, variant.repr(), point, add_number_first)?;
    let encoded = encode::encode(&module);
    Ok(Product {
        wasm: encoded.bytes,
        size: encoded.size,
    })
}
