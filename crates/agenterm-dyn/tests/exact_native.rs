use agenterm_dyn::{
    ExactNativeCall, ExactNativeError, ExactNativeType, ExactNativeValue, MAX_EXACT_NATIVE_ARITY,
    exact_native_stub_cardinality, invoke_exact,
};

#[test]
fn exact_core_owns_all_seven_families_at_all_seven_arities() {
    let families = [
        ExactNativeType::I32,
        ExactNativeType::U32,
        ExactNativeType::I64,
        ExactNativeType::U64,
        ExactNativeType::Isize,
        ExactNativeType::Usize,
        ExactNativeType::F64,
    ];
    let mut combinations = 0;
    for family in families {
        for arity in 0..=MAX_EXACT_NATIVE_ARITY {
            combinations += 1;
            let arguments = values(family, arity);
            let call = ExactNativeCall {
                library: "agenterm-native-library-that-does-not-exist",
                symbol: "unused",
                result: family,
                arguments: &arguments,
            };
            // A library error proves the signature passed the core's sole admission gate.
            // SAFETY: the deliberately absent library prevents native execution.
            assert!(matches!(
                unsafe { invoke_exact(&call) },
                Err(ExactNativeError::LibraryLoad { .. })
            ));
        }
    }
    assert_eq!(combinations, 49);
    assert_eq!(exact_native_stub_cardinality(), combinations);
}

#[test]
fn exact_core_keeps_signature_library_and_symbol_failures_distinct() {
    let mixed = [ExactNativeValue::I32(1), ExactNativeValue::U32(2)];
    let call = ExactNativeCall {
        library: "agenterm-native-library-that-does-not-exist",
        symbol: "unused",
        result: ExactNativeType::I32,
        arguments: &mixed,
    };
    // SAFETY: signature rejection occurs before loading or calling.
    assert!(matches!(
        unsafe { invoke_exact(&call) },
        Err(ExactNativeError::SignatureUnsupported { .. })
    ));

    let arguments = [];
    let call = ExactNativeCall {
        library: "agenterm-native-library-that-does-not-exist",
        symbol: "unused",
        result: ExactNativeType::I32,
        arguments: &arguments,
    };
    // SAFETY: the deliberately absent library prevents native execution.
    assert!(matches!(
        unsafe { invoke_exact(&call) },
        Err(ExactNativeError::LibraryLoad { .. })
    ));

    let call = ExactNativeCall {
        library: "",
        symbol: "agenterm_native_symbol_that_does_not_exist",
        result: ExactNativeType::I32,
        arguments: &arguments,
    };
    // SAFETY: the deliberately absent symbol prevents native execution.
    assert!(matches!(
        unsafe { invoke_exact(&call) },
        Err(ExactNativeError::SymbolLoad { .. })
    ));
}

fn values(family: ExactNativeType, arity: usize) -> Vec<ExactNativeValue> {
    let value = match family {
        ExactNativeType::I32 => ExactNativeValue::I32(0),
        ExactNativeType::U32 => ExactNativeValue::U32(0),
        ExactNativeType::I64 => ExactNativeValue::I64(0),
        ExactNativeType::U64 => ExactNativeValue::U64(0),
        ExactNativeType::Isize => ExactNativeValue::Isize(0),
        ExactNativeType::Usize => ExactNativeValue::Usize(0),
        ExactNativeType::F64 => ExactNativeValue::F64(0.0),
    };
    vec![value; arity]
}
