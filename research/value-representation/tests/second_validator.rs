//! Every product this experiment measures is validated a second time, by a
//! validator we did not write. Criterion 1 says "my encoder thinks it is
//! correct" is not evidence; neither is "my VM accepted it".

use valrepr::{Point, Variant, compile};

const CORPUS: &[(&str, &str, bool)] = &[
    ("arith", include_str!("../corpus/arith.qjs"), false),
    ("compare", include_str!("../corpus/compare.qjs"), false),
    ("loop", include_str!("../corpus/loop.qjs"), false),
    ("call", include_str!("../corpus/call.qjs"), false),
    ("mixed", include_str!("../corpus/mixed.qjs"), false),
    ("str_len", include_str!("../corpus/str_len.qjs"), true),
    ("str_concat", include_str!("../corpus/str_concat.qjs"), true),
    ("str_eq", include_str!("../corpus/str_eq.qjs"), true),
    ("str_poly", include_str!("../corpus/str_poly.qjs"), true),
    (
        "probe_neg2",
        include_str!("../corpus/probe_neg2.qjs"),
        false,
    ),
    (
        "probe_selfeq",
        include_str!("../corpus/probe_selfeq.qjs"),
        false,
    ),
];

#[test]
fn every_product_validates_under_an_independent_validator() {
    let mut checked = 0;
    for variant in Variant::ALL {
        for point in [Point::P1, Point::P2] {
            for (name, source, needs_strings) in CORPUS {
                if *needs_strings && point == Point::P1 {
                    continue;
                }
                let product = compile(source, variant, point).unwrap_or_else(|e| {
                    panic!("{} {} {name}: {e}", variant.label(), point.label())
                });
                // MVP plus multi-value: exactly what the two-word ABI needs and
                // what tinyvm accepts. No other proposal is enabled, so a
                // product that validates here needs nothing post-MVP beyond
                // multi-value.
                let mut features = wasmparser::WasmFeatures::empty();
                features.insert(wasmparser::WasmFeatures::MULTI_VALUE);
                features.insert(wasmparser::WasmFeatures::FLOATS);
                wasmparser::Validator::new_with_features(features)
                    .validate_all(&product.wasm)
                    .unwrap_or_else(|e| {
                        panic!(
                            "{} {} {name}: wasmparser rejected the product: {e}",
                            variant.label(),
                            point.label()
                        )
                    });
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 36, "four builds over the corpus and both probes");
}

/// Leak-list item L1.1, measured rather than asserted: the two-word ABI returns
/// two wasm values, so it needs the multi-value proposal. tinyvm has it, and so
/// does every current engine -- but a target that lacked it would force V1 to
/// return through memory, which is an escape hatch outside the representation.
/// The NaN-boxed ABI is one value and needs nothing beyond the MVP.
#[test]
fn multi_value_is_load_bearing_for_the_two_word_abi_only() {
    let mut mvp = wasmparser::WasmFeatures::empty();
    mvp.insert(wasmparser::WasmFeatures::FLOATS);
    let source = include_str!("../corpus/arith.qjs");

    let pair = compile(source, Variant::Pair, Point::P1).expect("compiles");
    assert!(
        wasmparser::Validator::new_with_features(mvp)
            .validate_all(&pair.wasm)
            .is_err(),
        "V1 should be rejected without multi-value"
    );

    let nanbox = compile(source, Variant::Nanbox, Point::P1).expect("compiles");
    wasmparser::Validator::new_with_features(mvp)
        .validate_all(&nanbox.wasm)
        .expect("V2 needs nothing beyond the MVP");
}
