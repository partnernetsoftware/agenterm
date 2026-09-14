use std::collections::HashSet;

use agenterm_qjswasm::QjswasmError;
use agenterm_qjswasm::native::{
    DecodedNativeCall, GuestSpan, MAX_NATIVE_ARITY, MAX_NATIVE_LIBRARY_BYTES,
    MAX_NATIVE_SPEC_BYTES, MAX_NATIVE_SYMBOL_BYTES, NATIVE_ARGUMENT_RECORD_BYTES,
    NATIVE_BLOCK_HEADER_BYTES, NATIVE_BLOCK_VERSION, NativeArgument, NativeDoorError, NativeType,
    SpanRegion, decode_native_call, parse_native_spec,
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
fn the_supported_grammar_preserves_its_language_level_types() {
    let source = "libsample.so|sample_i64|u64(i8,u16,i32,u64,isize,ptr)";
    let spec = parse_native_spec(source.as_bytes()).expect("supported declaration");
    assert_eq!(spec.library, "libsample.so");
    assert_eq!(spec.symbol, "sample_i64");
    assert_eq!(spec.result, NativeType::U64);
    assert_eq!(spec.parameters.len(), MAX_NATIVE_ARITY);
    assert!(spec.parameters.iter().all(|ty| *ty != NativeType::F64));

    let floating = parse_native_spec(b"|mixed|f64(ptr?,f64)").expect("supported declaration");
    assert_eq!(floating.result, NativeType::F64);
    assert_eq!(
        floating.parameters,
        vec![NativeType::NullablePointer, NativeType::F64]
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

fn native_door_error_word(error: &NativeDoorError) -> &'static str {
    match error {
        NativeDoorError::SpecTooLong { .. } => "spec-too-long",
        NativeDoorError::SpanOverflow { .. } => "span-overflow",
        NativeDoorError::SpanOutOfBounds { .. } => "span-out-of-bounds",
        NativeDoorError::SpecNotUtf8 => "spec-not-utf8",
        NativeDoorError::MalformedSpec => "malformed-spec",
        NativeDoorError::LibraryTooLong { .. } => "library-too-long",
        NativeDoorError::SymbolTooLong { .. } => "symbol-too-long",
        NativeDoorError::InvalidLibrary => "invalid-library",
        NativeDoorError::InvalidSymbol => "invalid-symbol",
        NativeDoorError::UnknownType { .. } => "unknown-type",
        NativeDoorError::UnsupportedType { .. } => "unsupported-type",
        NativeDoorError::VoidParameter { .. } => "void-parameter",
        NativeDoorError::ArityTooLarge { .. } => "arity-too-large",
        NativeDoorError::HeaderTooShort { .. } => "header-too-short",
        NativeDoorError::UnsupportedVersion { .. } => "unsupported-version",
        NativeDoorError::ArityMismatch { .. } => "arity-mismatch",
        NativeDoorError::BlockTooShort { .. } => "block-too-short",
        NativeDoorError::BlockTooLong { .. } => "block-too-long",
        NativeDoorError::RecordReservedNonZero { .. } => "record-reserved-nonzero",
        NativeDoorError::UnknownArgumentKind { .. } => "unknown-argument-kind",
        NativeDoorError::HostAddressNotPermitted { .. } => "host-address-not-permitted",
        NativeDoorError::ResultPointerOutsideGuestSpans => "result-pointer-outside-guest-spans",
        NativeDoorError::ArgumentKindMismatch { .. } => "argument-kind-mismatch",
        NativeDoorError::NullForNonNullablePointer { .. } => "null-for-non-nullable-pointer",
        NativeDoorError::NullPayloadNonZero { .. } => "null-payload-nonzero",
        NativeDoorError::DoorArgumentNegative { .. } => "door-argument-negative",
        NativeDoorError::InvocationSignatureUnsupported { .. } => {
            "invocation-signature-unsupported"
        }
        NativeDoorError::InvocationTargetUnsupported { .. } => "invocation-target-unsupported",
        NativeDoorError::ScalarNotCanonical { .. } => "scalar-not-canonical",
        NativeDoorError::ArgumentsNotUtf8 => "arguments-not-utf8",
        NativeDoorError::ArgumentsMalformed => "arguments-malformed",
        NativeDoorError::ArgumentCountMismatch { .. } => "argument-count-mismatch",
        NativeDoorError::ArgumentValueInvalid { .. } => "argument-value-invalid",
        NativeDoorError::ResultNotFinite => "result-not-finite",
        NativeDoorError::NativeRegionRequired { .. } => "native-region-required",
        NativeDoorError::NativeRegionShapeInvalid { .. } => "native-region-shape-invalid",
        NativeDoorError::NativeRegionTooLarge { .. } => "native-region-too-large",
        NativeDoorError::NativeRegionUnterminated { .. } => "native-region-unterminated",
        NativeDoorError::NativeRegionNotUtf8 { .. } => "native-region-not-utf8",
        NativeDoorError::LibraryLoad { .. } => "library-load",
        NativeDoorError::SymbolLoad { .. } => "symbol-load",
    }
}

#[test]
fn every_native_door_error_has_one_stable_distinct_code() {
    let errors = vec![
        (
            NativeDoorError::SpecTooLong {
                actual: 2,
                maximum: 1,
            },
            "native_spec_too_long",
        ),
        (
            NativeDoorError::SpanOverflow {
                region: SpanRegion::Spec,
            },
            "native_span_overflow",
        ),
        (
            NativeDoorError::SpanOutOfBounds {
                region: SpanRegion::Block,
                end: 2,
                memory_len: 1,
            },
            "native_span_out_of_bounds",
        ),
        (NativeDoorError::SpecNotUtf8, "native_spec_not_utf8"),
        (NativeDoorError::MalformedSpec, "native_spec_malformed"),
        (
            NativeDoorError::LibraryTooLong {
                actual: 2,
                maximum: 1,
            },
            "native_library_too_long",
        ),
        (
            NativeDoorError::SymbolTooLong {
                actual: 2,
                maximum: 1,
            },
            "native_symbol_too_long",
        ),
        (NativeDoorError::InvalidLibrary, "native_library_invalid"),
        (NativeDoorError::InvalidSymbol, "native_symbol_invalid"),
        (
            NativeDoorError::UnknownType {
                name: "future".to_owned(),
            },
            "native_type_unknown",
        ),
        (
            NativeDoorError::UnsupportedType { name: "f32" },
            "native_type_unsupported",
        ),
        (
            NativeDoorError::VoidParameter { index: 0 },
            "native_void_parameter",
        ),
        (
            NativeDoorError::ArityTooLarge {
                actual: 7,
                maximum: 6,
            },
            "native_arity_too_large",
        ),
        (
            NativeDoorError::HeaderTooShort {
                actual: 0,
                minimum: 16,
            },
            "native_header_too_short",
        ),
        (
            NativeDoorError::UnsupportedVersion {
                actual: 2,
                expected: 1,
            },
            "native_block_version_unsupported",
        ),
        (
            NativeDoorError::ArityMismatch {
                declared: 1,
                encoded: 0,
            },
            "native_arity_mismatch",
        ),
        (
            NativeDoorError::BlockTooShort {
                actual: 16,
                expected: 32,
            },
            "native_block_too_short",
        ),
        (
            NativeDoorError::BlockTooLong {
                actual: 48,
                expected: 32,
            },
            "native_block_too_long",
        ),
        (
            NativeDoorError::RecordReservedNonZero { index: 0, value: 1 },
            "native_record_reserved_nonzero",
        ),
        (
            NativeDoorError::UnknownArgumentKind { index: 0, kind: 4 },
            "native_argument_kind_unknown",
        ),
        (
            NativeDoorError::HostAddressNotPermitted { index: 0 },
            "native_host_address_not_permitted",
        ),
        (
            NativeDoorError::ResultPointerOutsideGuestSpans,
            "native_result_pointer_outside_guest_spans",
        ),
        (
            NativeDoorError::ArgumentKindMismatch {
                index: 0,
                kind: 0,
                ty: NativeType::Pointer,
            },
            "native_argument_kind_mismatch",
        ),
        (
            NativeDoorError::NullForNonNullablePointer { index: 0 },
            "native_null_not_permitted",
        ),
        (
            NativeDoorError::NullPayloadNonZero { index: 0 },
            "native_null_payload_nonzero",
        ),
        (
            NativeDoorError::DoorArgumentNegative {
                index: 0,
                value: -1,
            },
            "native_door_argument_negative",
        ),
        (
            NativeDoorError::InvocationSignatureUnsupported {
                result: NativeType::I32,
                parameters: vec![NativeType::F64],
            },
            "native_invocation_signature_unsupported",
        ),
        (
            NativeDoorError::InvocationTargetUnsupported { operation: "ioctl" },
            "native_invocation_target_unsupported",
        ),
        (
            NativeDoorError::ScalarNotCanonical {
                index: 0,
                ty: NativeType::I32,
                bits: u64::MAX,
            },
            "native_scalar_not_canonical",
        ),
        (
            NativeDoorError::ArgumentsNotUtf8,
            "native_arguments_not_utf8",
        ),
        (
            NativeDoorError::ArgumentsMalformed,
            "native_arguments_malformed",
        ),
        (
            NativeDoorError::ArgumentCountMismatch {
                declared: 1,
                actual: 0,
            },
            "native_argument_count_mismatch",
        ),
        (
            NativeDoorError::ArgumentValueInvalid {
                index: 0,
                ty: NativeType::I32,
            },
            "native_argument_value_invalid",
        ),
        (NativeDoorError::ResultNotFinite, "native_result_not_finite"),
        (
            NativeDoorError::NativeRegionRequired { index: 0 },
            "native_region_required",
        ),
        (
            NativeDoorError::NativeRegionShapeInvalid {
                index: 0,
                reason: "fixture",
            },
            "native_region_shape_invalid",
        ),
        (
            NativeDoorError::NativeRegionTooLarge {
                requested: 2,
                maximum: 1,
            },
            "native_region_too_large",
        ),
        (
            NativeDoorError::NativeRegionUnterminated {
                index: 0,
                native_status: 0,
            },
            "native_region_unterminated",
        ),
        (
            NativeDoorError::NativeRegionNotUtf8 {
                index: 0,
                native_status: 0,
            },
            "native_region_not_utf8",
        ),
        (
            NativeDoorError::LibraryLoad {
                library: "fixture".to_owned(),
                message: "fixture".to_owned(),
            },
            "native_library_load_failed",
        ),
        (
            NativeDoorError::SymbolLoad {
                symbol: "fixture".to_owned(),
                message: "fixture".to_owned(),
            },
            "native_symbol_load_failed",
        ),
    ];
    let words: HashSet<_> = errors
        .iter()
        .map(|(error, _)| native_door_error_word(error))
        .collect();
    let codes: HashSet<_> = errors.iter().map(|(error, _)| error.code()).collect();
    for (error, expected_code) in &errors {
        assert_eq!(
            error.code(),
            *expected_code,
            "{} code drifted",
            native_door_error_word(error)
        );
    }
    assert_eq!(words.len(), errors.len());
    assert_eq!(codes.len(), errors.len());
}
