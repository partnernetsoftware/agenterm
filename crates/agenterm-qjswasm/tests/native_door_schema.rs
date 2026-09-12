use std::collections::HashSet;

use agenterm_qjswasm::QjswasmError;
use agenterm_qjswasm::native::{
    DecodedNativeCall, GuestSpan, MAX_NATIVE_ARITY, MAX_NATIVE_LIBRARY_BYTES,
    MAX_NATIVE_SPEC_BYTES, MAX_NATIVE_SYMBOL_BYTES, NATIVE_ARGUMENT_RECORD_BYTES,
    NATIVE_BLOCK_HEADER_BYTES, NATIVE_BLOCK_VERSION, NativeArgument, NativeDoorError, NativeType,
    ParameterRegisterClass, ReturnRegisterClass, SpanRegion, decode_native_call,
    native_register_pattern_cardinality, parse_native_spec,
};

const SPEC_OFFSET: usize = 0;
const BLOCK_OFFSET: usize = 128;

fn record(kind: u32, reserved: u32, payload: u64) -> [u8; NATIVE_ARGUMENT_RECORD_BYTES] {
    let mut bytes = [0; NATIVE_ARGUMENT_RECORD_BYTES];
    bytes[0..4].copy_from_slice(&kind.to_le_bytes());
    bytes[4..8].copy_from_slice(&reserved.to_le_bytes());
    bytes[8..16].copy_from_slice(&payload.to_le_bytes());
    bytes
}

fn memory_with(spec: &str, return_bits: u64, records: &[[u8; 16]]) -> Vec<u8> {
    let mut memory = vec![0; 512];
    memory[SPEC_OFFSET..SPEC_OFFSET + spec.len()].copy_from_slice(spec.as_bytes());
    let block = &mut memory[BLOCK_OFFSET..];
    block[0..4].copy_from_slice(&NATIVE_BLOCK_VERSION.to_le_bytes());
    block[4..8].copy_from_slice(&(records.len() as u32).to_le_bytes());
    block[8..16].copy_from_slice(&return_bits.to_le_bytes());
    for (index, record) in records.iter().enumerate() {
        let start = NATIVE_BLOCK_HEADER_BYTES + index * NATIVE_ARGUMENT_RECORD_BYTES;
        block[start..start + NATIVE_ARGUMENT_RECORD_BYTES].copy_from_slice(record);
    }
    memory
}

fn decode(spec: &str, memory: &[u8], records: usize) -> Result<DecodedNativeCall, NativeDoorError> {
    decode_native_call(
        memory,
        SPEC_OFFSET,
        spec.len(),
        BLOCK_OFFSET,
        NATIVE_BLOCK_HEADER_BYTES + records * NATIVE_ARGUMENT_RECORD_BYTES,
    )
}

#[test]
fn the_supported_grammar_is_distinct_from_its_register_classes() {
    let source = "libsample.so|sample_i64|u64(i8,u16,i32,u64,isize,ptr)";
    let spec = parse_native_spec(source.as_bytes()).expect("supported declaration");
    assert_eq!(spec.library, "libsample.so");
    assert_eq!(spec.symbol, "sample_i64");
    assert_eq!(spec.result, NativeType::U64);
    assert_eq!(spec.parameters.len(), MAX_NATIVE_ARITY);
    assert!(
        spec.parameters.iter().all(|ty| *ty != NativeType::F64),
        "this declaration uses many language types but one GP ABI class"
    );

    let floating = parse_native_spec(b"|mixed|f64(ptr?,f64)").expect("supported declaration");
    let classes = agenterm_qjswasm::native::classify_signature(&floating);
    assert_eq!(classes.result, ReturnRegisterClass::F64);
    assert_eq!(
        classes.parameters,
        vec![ParameterRegisterClass::Gp, ParameterRegisterClass::F64]
    );
    assert_eq!(
        parse_native_spec(b"|future|i32(f32)"),
        Err(NativeDoorError::UnsupportedType { name: "f32" })
    );
    assert!(matches!(
        parse_native_spec(b"|bad|i32(vector128)"),
        Err(NativeDoorError::UnknownType { .. })
    ));
}

#[test]
fn source_library_symbol_and_arity_limits_are_independent() {
    let too_long_source = vec![b'x'; MAX_NATIVE_SPEC_BYTES + 1];
    assert!(matches!(
        parse_native_spec(&too_long_source),
        Err(NativeDoorError::SpecTooLong { .. })
    ));

    let library = "x".repeat(MAX_NATIVE_LIBRARY_BYTES + 1);
    assert!(matches!(
        parse_native_spec(format!("{library}|f|i32()").as_bytes()),
        Err(NativeDoorError::LibraryTooLong { .. })
    ));
    let symbol = "x".repeat(MAX_NATIVE_SYMBOL_BYTES + 1);
    assert!(matches!(
        parse_native_spec(format!("|{symbol}|i32()").as_bytes()),
        Err(NativeDoorError::SymbolTooLong { .. })
    ));
    assert!(matches!(
        parse_native_spec(b"|f|i32(i32,i32,i32,i32,i32,i32,i32)"),
        Err(NativeDoorError::ArityTooLarge {
            actual: 7,
            maximum: MAX_NATIVE_ARITY
        })
    ));
    for decorated in ["?Func@@YAHH@Z", "foo@GLIBC_2.2.5", "operator new"] {
        let parsed = parse_native_spec(format!("|{decorated}|i32()").as_bytes())
            .expect("decorated native symbol bytes are accepted");
        assert_eq!(parsed.symbol, decorated);
    }
    assert_eq!(
        parse_native_spec(b"||i32()"),
        Err(NativeDoorError::InvalidSymbol)
    );
    assert_eq!(
        parse_native_spec(b"|bad\0symbol|i32()"),
        Err(NativeDoorError::InvalidSymbol)
    );
    assert_eq!(
        parse_native_spec(b"lib\0name|f|i32()"),
        Err(NativeDoorError::InvalidLibrary)
    );
    assert_eq!(
        parse_native_spec(b"|f|i32(void)"),
        Err(NativeDoorError::VoidParameter { index: 0 })
    );
    assert_eq!(
        parse_native_spec(&[0xff, b'|', b'f', b'|', b'i', b'3', b'2', b'(', b')']),
        Err(NativeDoorError::SpecNotUtf8)
    );
    assert_eq!(
        parse_native_spec(b"missing-separators"),
        Err(NativeDoorError::MalformedSpec)
    );
}

#[test]
fn header_and_fixed_record_table_have_exact_checked_bounds() {
    let spec = "|getpid|i32()";
    let memory = memory_with(spec, 0, &[]);
    assert!(matches!(
        decode_native_call(&memory, 0, spec.len(), BLOCK_OFFSET, 15),
        Err(NativeDoorError::HeaderTooShort { .. })
    ));
    assert!(matches!(
        decode_native_call(&memory, usize::MAX, 2, BLOCK_OFFSET, 16),
        Err(NativeDoorError::SpanOverflow {
            region: SpanRegion::Spec
        })
    ));
    let mut unaligned = memory_with(spec, 17, &[]);
    unaligned.copy_within(
        BLOCK_OFFSET..BLOCK_OFFSET + NATIVE_BLOCK_HEADER_BYTES,
        BLOCK_OFFSET + 1,
    );
    let call = decode_native_call(
        &unaligned,
        0,
        spec.len(),
        BLOCK_OFFSET + 1,
        NATIVE_BLOCK_HEADER_BYTES,
    )
    .expect("the wire block is byte-decoded and need not be host aligned");
    assert_eq!(
        call.return_slot,
        GuestSpan {
            offset: BLOCK_OFFSET + 9,
            len: 8,
        }
    );
    assert_eq!(call.initial_return_bits, 17);

    let one_spec = "|f|i32(i32)";
    let one = memory_with(one_spec, 0, &[record(0, 0, 7)]);
    assert!(matches!(
        decode_native_call(&one, 0, one_spec.len(), BLOCK_OFFSET, 16),
        Err(NativeDoorError::BlockTooShort { expected: 32, .. })
    ));
    assert!(matches!(
        decode_native_call(&one, 0, one_spec.len(), BLOCK_OFFSET, 33),
        Err(NativeDoorError::BlockTooLong { expected: 32, .. })
    ));

    let mut wrong_version = memory_with("|getpid|i32()", 0, &[]);
    wrong_version[BLOCK_OFFSET..BLOCK_OFFSET + 4].copy_from_slice(&2_u32.to_le_bytes());
    assert_eq!(
        decode("|getpid|i32()", &wrong_version, 0),
        Err(NativeDoorError::UnsupportedVersion {
            actual: 2,
            expected: NATIVE_BLOCK_VERSION,
        })
    );
}

#[test]
fn declaration_and_encoded_arity_must_agree() {
    let spec = "|f|i32(i32)";
    let memory = memory_with(spec, 0, &[]);
    assert_eq!(
        decode_native_call(&memory, 0, spec.len(), BLOCK_OFFSET, 16),
        Err(NativeDoorError::ArityMismatch {
            declared: 1,
            encoded: 0,
        })
    );
}

#[test]
fn guest_spans_are_checked_but_may_alias_everything() {
    let spec = "|copy|i32(ptr,ptr?,ptr)";
    let aliased = ((spec.len() as u64) << 32) | SPEC_OFFSET as u64;
    let overlaps_block = ((NATIVE_BLOCK_HEADER_BYTES as u64) << 32) | BLOCK_OFFSET as u64;
    let memory = memory_with(
        spec,
        9,
        &[
            record(1, 0, aliased),
            record(1, 0, aliased),
            record(1, 0, overlaps_block),
        ],
    );
    let call = decode(spec, &memory, 3).expect("aliasing is valid C ABI input");
    assert_eq!(call.initial_return_bits, 9);
    assert_eq!(
        call.return_slot,
        GuestSpan {
            offset: BLOCK_OFFSET + 8,
            len: 8,
        }
    );
    assert_eq!(
        call.arguments,
        vec![
            NativeArgument::GuestSpan {
                ty: NativeType::Pointer,
                span: GuestSpan {
                    offset: 0,
                    len: spec.len(),
                },
            },
            NativeArgument::GuestSpan {
                ty: NativeType::NullablePointer,
                span: GuestSpan {
                    offset: 0,
                    len: spec.len(),
                },
            },
            NativeArgument::GuestSpan {
                ty: NativeType::Pointer,
                span: GuestSpan {
                    offset: BLOCK_OFFSET,
                    len: NATIVE_BLOCK_HEADER_BYTES,
                },
            },
        ]
    );

    let out_of_bounds = ((40_u64) << 32) | 500;
    let memory = memory_with("|f|i32(ptr)", 0, &[record(1, 0, out_of_bounds)]);
    assert!(matches!(
        decode("|f|i32(ptr)", &memory, 1),
        Err(NativeDoorError::SpanOutOfBounds {
            region: SpanRegion::Argument(0),
            ..
        })
    ));

    let overflow = ((2_u64) << 32) | u32::MAX as u64;
    let memory = memory_with("|f|i32(ptr)", 0, &[record(1, 0, overflow)]);
    assert_eq!(
        decode("|f|i32(ptr)", &memory, 1),
        Err(NativeDoorError::SpanOverflow {
            region: SpanRegion::Argument(0),
        })
    );
}

#[test]
fn record_kinds_are_compatible_with_declared_types() {
    let scalar = memory_with("|f|i32(i32)", 0, &[record(0, 0, 42)]);
    assert!(matches!(
        decode("|f|i32(i32)", &scalar, 1)
            .unwrap()
            .arguments
            .as_slice(),
        [NativeArgument::Scalar {
            ty: NativeType::I32,
            bits: 42
        }]
    ));

    let null = memory_with("|f|i32(ptr?)", 0, &[record(2, 0, 0)]);
    assert!(matches!(
        decode("|f|i32(ptr?)", &null, 1)
            .unwrap()
            .arguments
            .as_slice(),
        [NativeArgument::Null {
            ty: NativeType::NullablePointer
        }]
    ));
    let required = memory_with("|f|i32(ptr)", 0, &[record(2, 0, 0)]);
    assert!(matches!(
        decode("|f|i32(ptr)", &required, 1),
        Err(NativeDoorError::NullForNonNullablePointer { index: 0 })
    ));
    let non_pointer = memory_with("|f|i32(i64)", 0, &[record(1, 0, 0)]);
    assert!(matches!(
        decode("|f|i32(i64)", &non_pointer, 1),
        Err(NativeDoorError::ArgumentKindMismatch { index: 0, .. })
    ));

    let dirty_reserved = memory_with("|f|i32(i64)", 0, &[record(0, 1, 0)]);
    assert_eq!(
        decode("|f|i32(i64)", &dirty_reserved, 1),
        Err(NativeDoorError::RecordReservedNonZero { index: 0, value: 1 })
    );
    let dirty_null = memory_with("|f|i32(ptr?)", 0, &[record(2, 0, 1)]);
    assert_eq!(
        decode("|f|i32(ptr?)", &dirty_null, 1),
        Err(NativeDoorError::NullPayloadNonZero { index: 0 })
    );
}

#[test]
fn raw_host_addresses_and_unknown_kinds_are_typed_refusals() {
    let raw = memory_with("|f|i32(ptr)", 0, &[record(3, 0, 0xfeed)]);
    let error = decode("|f|i32(ptr)", &raw, 1).expect_err("raw host address is not admitted");
    assert_eq!(error.code(), "native_host_address_not_permitted");
    assert!(matches!(
        QjswasmError::from(error),
        QjswasmError::Door(message) if message.starts_with("native_host_address_not_permitted:")
    ));

    let unknown = memory_with("|f|i32(i32)", 0, &[record(99, 0, 0)]);
    assert!(matches!(
        decode("|f|i32(i32)", &unknown, 1),
        Err(NativeDoorError::UnknownArgumentKind { index: 0, kind: 99 })
    ));
}

#[test]
fn every_schema_error_has_a_stable_distinct_code() {
    let errors = vec![
        NativeDoorError::SpecTooLong {
            actual: 2,
            maximum: 1,
        },
        NativeDoorError::SpanOverflow {
            region: SpanRegion::Spec,
        },
        NativeDoorError::SpanOutOfBounds {
            region: SpanRegion::Block,
            end: 2,
            memory_len: 1,
        },
        NativeDoorError::SpecNotUtf8,
        NativeDoorError::MalformedSpec,
        NativeDoorError::LibraryTooLong {
            actual: 2,
            maximum: 1,
        },
        NativeDoorError::SymbolTooLong {
            actual: 2,
            maximum: 1,
        },
        NativeDoorError::InvalidLibrary,
        NativeDoorError::InvalidSymbol,
        NativeDoorError::UnknownType {
            name: "future".to_owned(),
        },
        NativeDoorError::UnsupportedType { name: "f32" },
        NativeDoorError::VoidParameter { index: 0 },
        NativeDoorError::ArityTooLarge {
            actual: 7,
            maximum: 6,
        },
        NativeDoorError::HeaderTooShort {
            actual: 0,
            minimum: 16,
        },
        NativeDoorError::UnsupportedVersion {
            actual: 2,
            expected: 1,
        },
        NativeDoorError::ArityMismatch {
            declared: 1,
            encoded: 0,
        },
        NativeDoorError::BlockTooShort {
            actual: 16,
            expected: 32,
        },
        NativeDoorError::BlockTooLong {
            actual: 48,
            expected: 32,
        },
        NativeDoorError::RecordReservedNonZero { index: 0, value: 1 },
        NativeDoorError::UnknownArgumentKind { index: 0, kind: 4 },
        NativeDoorError::HostAddressNotPermitted { index: 0 },
        NativeDoorError::ArgumentKindMismatch {
            index: 0,
            kind: 0,
            ty: NativeType::Pointer,
        },
        NativeDoorError::NullForNonNullablePointer { index: 0 },
        NativeDoorError::NullPayloadNonZero { index: 0 },
    ];
    let codes: HashSet<_> = errors.iter().map(NativeDoorError::code).collect();
    assert_eq!(codes.len(), errors.len());
}

#[test]
fn register_pattern_cardinality_is_derived_from_classes_and_arity() {
    fn enumerate_parameter_patterns(
        remaining: usize,
        prefix: &mut Vec<ParameterRegisterClass>,
        seen: &mut HashSet<Vec<ParameterRegisterClass>>,
    ) {
        seen.insert(prefix.clone());
        if remaining == 0 {
            return;
        }
        for class in [ParameterRegisterClass::Gp, ParameterRegisterClass::F64] {
            prefix.push(class);
            enumerate_parameter_patterns(remaining - 1, prefix, seen);
            prefix.pop();
        }
    }

    let mut patterns = HashSet::new();
    enumerate_parameter_patterns(MAX_NATIVE_ARITY, &mut Vec::new(), &mut patterns);
    let returns = [
        ReturnRegisterClass::Void,
        ReturnRegisterClass::Gp,
        ReturnRegisterClass::F64,
    ];
    let independently_enumerated = patterns.len() * returns.len();
    assert_eq!(patterns.len(), 127);
    assert_eq!(independently_enumerated, 381);
    assert_eq!(
        native_register_pattern_cardinality(),
        independently_enumerated
    );
}
