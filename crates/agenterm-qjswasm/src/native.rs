//! Bounded schema for the planned `agenterm.native_call` door.
//!
//! This module deliberately stops before symbol lookup or invocation. It owns
//! the guest-authored declaration and argument-block format, and turns hostile
//! guest bytes into typed, bounded host data. The future host-door wiring has
//! one conversion from [`NativeDoorError`] to [`crate::QjswasmError::Door`].

use std::fmt;

/// Schema version stored in every argument-block header.
pub const NATIVE_BLOCK_VERSION: u32 = 1;
/// Bytes in `[version:u32, argc:u32, initial_return_bits:u64]`.
pub const NATIVE_BLOCK_HEADER_BYTES: usize = 16;
/// Bytes in one `[kind:u32, reserved:u32, payload:u64]` argument record.
pub const NATIVE_ARGUMENT_RECORD_BYTES: usize = 16;
/// Largest complete `library|symbol|ret(args...)` declaration.
pub const MAX_NATIVE_SPEC_BYTES: usize = 1024;
/// Largest library component. Empty means the current process image.
pub const MAX_NATIVE_LIBRARY_BYTES: usize = 512;
/// Largest native symbol component.
pub const MAX_NATIVE_SYMBOL_BYTES: usize = 255;
/// Largest fixed native-call arity admitted by the schema.
pub const MAX_NATIVE_ARITY: usize = 6;

const KIND_SCALAR: u32 = 0;
const KIND_GUEST_SPAN: u32 = 1;
const KIND_NULL: u32 = 2;
const KIND_HOST_ADDRESS: u32 = 3;

/// A type in the bounded native declaration grammar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeType {
    Void,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    Isize,
    Usize,
    Pointer,
    NullablePointer,
    F64,
}

impl NativeType {
    fn is_pointer(self) -> bool {
        matches!(self, Self::Pointer | Self::NullablePointer)
    }

    fn is_nullable_pointer(self) -> bool {
        matches!(self, Self::NullablePointer)
    }
}

/// Parsed declaration, independent of the ABI register classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSpec {
    pub library: String,
    pub symbol: String,
    pub result: NativeType,
    pub parameters: Vec<NativeType>,
}

/// The two parameter register classes admitted by this first fixed ABI set.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ParameterRegisterClass {
    Gp,
    F64,
}

/// Return register classes admitted by this first fixed ABI set.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReturnRegisterClass {
    Void,
    Gp,
    F64,
}

/// ABI classes derived from a parsed declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSignatureClasses {
    pub result: ReturnRegisterClass,
    pub parameters: Vec<ParameterRegisterClass>,
}

/// A checked guest-memory span. Aliasing is intentionally permitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestSpan {
    pub offset: usize,
    pub len: usize,
}

/// One decoded argument record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeArgument {
    Scalar { ty: NativeType, bits: u64 },
    GuestSpan { ty: NativeType, span: GuestSpan },
    Null { ty: NativeType },
}

/// A declaration and argument block that passed every schema boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedNativeCall {
    pub spec: NativeSpec,
    pub signature: NativeSignatureClasses,
    /// The result field's exact location in guest memory.
    pub return_slot: GuestSpan,
    /// Bits found in that slot before any native call occurs.
    pub initial_return_bits: u64,
    pub arguments: Vec<NativeArgument>,
}

/// Which guest span failed validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpanRegion {
    Spec,
    Block,
    Argument(usize),
}

/// Strongly typed schema failures. The future door maps this enum once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeDoorError {
    SpecTooLong {
        actual: usize,
        maximum: usize,
    },
    SpanOverflow {
        region: SpanRegion,
    },
    SpanOutOfBounds {
        region: SpanRegion,
        end: usize,
        memory_len: usize,
    },
    SpecNotUtf8,
    MalformedSpec,
    LibraryTooLong {
        actual: usize,
        maximum: usize,
    },
    SymbolTooLong {
        actual: usize,
        maximum: usize,
    },
    InvalidLibrary,
    InvalidSymbol,
    UnknownType {
        name: String,
    },
    UnsupportedType {
        name: &'static str,
    },
    VoidParameter {
        index: usize,
    },
    ArityTooLarge {
        actual: usize,
        maximum: usize,
    },
    HeaderTooShort {
        actual: usize,
        minimum: usize,
    },
    UnsupportedVersion {
        actual: u32,
        expected: u32,
    },
    ArityMismatch {
        declared: usize,
        encoded: usize,
    },
    BlockTooShort {
        actual: usize,
        expected: usize,
    },
    BlockTooLong {
        actual: usize,
        expected: usize,
    },
    RecordReservedNonZero {
        index: usize,
        value: u32,
    },
    UnknownArgumentKind {
        index: usize,
        kind: u32,
    },
    HostAddressNotPermitted {
        index: usize,
    },
    ArgumentKindMismatch {
        index: usize,
        kind: u32,
        ty: NativeType,
    },
    NullForNonNullablePointer {
        index: usize,
    },
    NullPayloadNonZero {
        index: usize,
    },
}

impl NativeDoorError {
    /// Stable machine-readable name used by the single future door mapping.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SpecTooLong { .. } => "native_spec_too_long",
            Self::SpanOverflow { .. } => "native_span_overflow",
            Self::SpanOutOfBounds { .. } => "native_span_out_of_bounds",
            Self::SpecNotUtf8 => "native_spec_not_utf8",
            Self::MalformedSpec => "native_spec_malformed",
            Self::LibraryTooLong { .. } => "native_library_too_long",
            Self::SymbolTooLong { .. } => "native_symbol_too_long",
            Self::InvalidLibrary => "native_library_invalid",
            Self::InvalidSymbol => "native_symbol_invalid",
            Self::UnknownType { .. } => "native_type_unknown",
            Self::UnsupportedType { .. } => "native_type_unsupported",
            Self::VoidParameter { .. } => "native_void_parameter",
            Self::ArityTooLarge { .. } => "native_arity_too_large",
            Self::HeaderTooShort { .. } => "native_header_too_short",
            Self::UnsupportedVersion { .. } => "native_block_version_unsupported",
            Self::ArityMismatch { .. } => "native_arity_mismatch",
            Self::BlockTooShort { .. } => "native_block_too_short",
            Self::BlockTooLong { .. } => "native_block_too_long",
            Self::RecordReservedNonZero { .. } => "native_record_reserved_nonzero",
            Self::UnknownArgumentKind { .. } => "native_argument_kind_unknown",
            Self::HostAddressNotPermitted { .. } => "native_host_address_not_permitted",
            Self::ArgumentKindMismatch { .. } => "native_argument_kind_mismatch",
            Self::NullForNonNullablePointer { .. } => "native_null_not_permitted",
            Self::NullPayloadNonZero { .. } => "native_null_payload_nonzero",
        }
    }
}

impl fmt::Display for NativeDoorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::SpecTooLong { actual, maximum }
            | Self::LibraryTooLong { actual, maximum }
            | Self::SymbolTooLong { actual, maximum }
            | Self::ArityTooLarge { actual, maximum } => {
                write!(f, "{actual} exceeds maximum {maximum}")
            }
            Self::SpanOverflow { region } => write!(f, "{region:?} offset plus length overflowed"),
            Self::SpanOutOfBounds {
                region,
                end,
                memory_len,
            } => write!(f, "{region:?} ends at {end}, memory length is {memory_len}"),
            Self::SpecNotUtf8 => f.write_str("declaration is not UTF-8"),
            Self::MalformedSpec => f.write_str("expected library|symbol|result(parameters)"),
            Self::InvalidLibrary => f.write_str("library contains a NUL byte"),
            Self::InvalidSymbol => f.write_str("symbol is empty or contains a NUL byte"),
            Self::UnknownType { name } => write!(f, "unknown type {name:?}"),
            Self::UnsupportedType { name } => {
                write!(f, "type {name:?} is recognized but unsupported")
            }
            Self::VoidParameter { index } => write!(f, "parameter {index} is void"),
            Self::HeaderTooShort { actual, minimum } => {
                write!(f, "block has {actual} bytes, header requires {minimum}")
            }
            Self::UnsupportedVersion { actual, expected } => {
                write!(f, "block version {actual}, expected {expected}")
            }
            Self::ArityMismatch { declared, encoded } => {
                write!(
                    f,
                    "declaration has {declared} parameters, block encodes {encoded}"
                )
            }
            Self::BlockTooShort { actual, expected } => {
                write!(f, "block has {actual} bytes, expected exactly {expected}")
            }
            Self::BlockTooLong { actual, expected } => {
                write!(f, "block has {actual} bytes, expected exactly {expected}")
            }
            Self::RecordReservedNonZero { index, value } => {
                write!(f, "argument {index} reserved word is {value}")
            }
            Self::UnknownArgumentKind { index, kind } => {
                write!(f, "argument {index} has unknown kind {kind}")
            }
            Self::HostAddressNotPermitted { index } => {
                write!(f, "argument {index} attempts to supply a raw host address")
            }
            Self::ArgumentKindMismatch { index, kind, ty } => {
                write!(
                    f,
                    "argument {index} kind {kind} is incompatible with {ty:?}"
                )
            }
            Self::NullForNonNullablePointer { index } => {
                write!(
                    f,
                    "argument {index} is null but its pointer is not nullable"
                )
            }
            Self::NullPayloadNonZero { index } => {
                write!(f, "argument {index} null record has a nonzero payload")
            }
        }
    }
}

impl std::error::Error for NativeDoorError {}

impl From<NativeDoorError> for crate::QjswasmError {
    fn from(error: NativeDoorError) -> Self {
        Self::Door(error.to_string())
    }
}

/// Parse one bounded native declaration.
pub fn parse_native_spec(bytes: &[u8]) -> Result<NativeSpec, NativeDoorError> {
    if bytes.len() > MAX_NATIVE_SPEC_BYTES {
        return Err(NativeDoorError::SpecTooLong {
            actual: bytes.len(),
            maximum: MAX_NATIVE_SPEC_BYTES,
        });
    }
    let source = std::str::from_utf8(bytes).map_err(|_| NativeDoorError::SpecNotUtf8)?;
    let mut components = source.split('|');
    let library = components.next().ok_or(NativeDoorError::MalformedSpec)?;
    let symbol = components.next().ok_or(NativeDoorError::MalformedSpec)?;
    let signature = components.next().ok_or(NativeDoorError::MalformedSpec)?;
    if components.next().is_some() {
        return Err(NativeDoorError::MalformedSpec);
    }
    if library.len() > MAX_NATIVE_LIBRARY_BYTES {
        return Err(NativeDoorError::LibraryTooLong {
            actual: library.len(),
            maximum: MAX_NATIVE_LIBRARY_BYTES,
        });
    }
    if symbol.len() > MAX_NATIVE_SYMBOL_BYTES {
        return Err(NativeDoorError::SymbolTooLong {
            actual: symbol.len(),
            maximum: MAX_NATIVE_SYMBOL_BYTES,
        });
    }
    if library.as_bytes().contains(&0) {
        return Err(NativeDoorError::InvalidLibrary);
    }
    if symbol.is_empty() || symbol.as_bytes().contains(&0) {
        return Err(NativeDoorError::InvalidSymbol);
    }

    let open = signature.find('(').ok_or(NativeDoorError::MalformedSpec)?;
    if !signature.ends_with(')') || signature[..open].is_empty() {
        return Err(NativeDoorError::MalformedSpec);
    }
    let result = parse_type(&signature[..open])?;
    let parameter_source = &signature[open + 1..signature.len() - 1];
    let mut parameters = Vec::new();
    if !parameter_source.is_empty() {
        for (index, name) in parameter_source.split(',').enumerate() {
            if index >= MAX_NATIVE_ARITY {
                return Err(NativeDoorError::ArityTooLarge {
                    actual: index + 1,
                    maximum: MAX_NATIVE_ARITY,
                });
            }
            let ty = parse_type(name)?;
            if ty == NativeType::Void {
                return Err(NativeDoorError::VoidParameter { index });
            }
            parameters.push(ty);
        }
    }
    Ok(NativeSpec {
        library: library.to_owned(),
        symbol: symbol.to_owned(),
        result,
        parameters,
    })
}

fn parse_type(name: &str) -> Result<NativeType, NativeDoorError> {
    Ok(match name {
        "void" => NativeType::Void,
        "i8" => NativeType::I8,
        "u8" => NativeType::U8,
        "i16" => NativeType::I16,
        "u16" => NativeType::U16,
        "i32" => NativeType::I32,
        "u32" => NativeType::U32,
        "i64" => NativeType::I64,
        "u64" => NativeType::U64,
        "isize" => NativeType::Isize,
        "usize" => NativeType::Usize,
        "ptr" => NativeType::Pointer,
        "ptr?" => NativeType::NullablePointer,
        "f64" => NativeType::F64,
        "f32" => return Err(NativeDoorError::UnsupportedType { name: "f32" }),
        _ => {
            return Err(NativeDoorError::UnknownType {
                name: name.to_owned(),
            });
        }
    })
}

/// Derive ABI classes from language-level types.
pub fn classify_signature(spec: &NativeSpec) -> NativeSignatureClasses {
    let result = match spec.result {
        NativeType::Void => ReturnRegisterClass::Void,
        NativeType::F64 => ReturnRegisterClass::F64,
        _ => ReturnRegisterClass::Gp,
    };
    let parameters = spec
        .parameters
        .iter()
        .map(|ty| match ty {
            NativeType::F64 => ParameterRegisterClass::F64,
            _ => ParameterRegisterClass::Gp,
        })
        .collect();
    NativeSignatureClasses { result, parameters }
}

/// Number of register-position patterns implied by the supported classes.
///
/// This is derived rather than written as a remembered constant: for each
/// arity `0..=MAX_NATIVE_ARITY`, every parameter has one of two classes, and
/// each parameter pattern combines with each of the three return classes.
/// It is not a count of safe Rust `extern fn` stubs. Integer-width/sign
/// compatibility and the function-pointer types used for invocation belong to
/// the next ABI experiment.
pub fn native_register_pattern_cardinality() -> usize {
    let parameter_class_count = [ParameterRegisterClass::Gp, ParameterRegisterClass::F64].len();
    let return_class_count = [
        ReturnRegisterClass::Void,
        ReturnRegisterClass::Gp,
        ReturnRegisterClass::F64,
    ]
    .len();
    let parameter_patterns = (0..=MAX_NATIVE_ARITY)
        .map(|arity| parameter_class_count.pow(arity as u32))
        .sum::<usize>();
    parameter_patterns * return_class_count
}

/// Validate and decode one declaration plus fixed-layout argument block.
///
/// `spec_offset`, `block_offset`, and every kind-1 pointee are offsets in the
/// same `memory`. Their ranges may alias. The fixed record table is decoded as
/// bytes and requires exact length, not host alignment.
pub fn decode_native_call(
    memory: &[u8],
    spec_offset: usize,
    spec_len: usize,
    block_offset: usize,
    block_len: usize,
) -> Result<DecodedNativeCall, NativeDoorError> {
    if spec_len > MAX_NATIVE_SPEC_BYTES {
        return Err(NativeDoorError::SpecTooLong {
            actual: spec_len,
            maximum: MAX_NATIVE_SPEC_BYTES,
        });
    }
    let spec_bytes = checked_span(memory, spec_offset, spec_len, SpanRegion::Spec)?;
    let spec = parse_native_spec(spec_bytes)?;
    let block = checked_span(memory, block_offset, block_len, SpanRegion::Block)?;
    if block.len() < NATIVE_BLOCK_HEADER_BYTES {
        return Err(NativeDoorError::HeaderTooShort {
            actual: block.len(),
            minimum: NATIVE_BLOCK_HEADER_BYTES,
        });
    }
    let version = le_u32(&block[0..4]);
    if version != NATIVE_BLOCK_VERSION {
        return Err(NativeDoorError::UnsupportedVersion {
            actual: version,
            expected: NATIVE_BLOCK_VERSION,
        });
    }
    let encoded_arity = le_u32(&block[4..8]) as usize;
    if encoded_arity != spec.parameters.len() {
        return Err(NativeDoorError::ArityMismatch {
            declared: spec.parameters.len(),
            encoded: encoded_arity,
        });
    }
    let records_len = encoded_arity
        .checked_mul(NATIVE_ARGUMENT_RECORD_BYTES)
        .ok_or(NativeDoorError::SpanOverflow {
            region: SpanRegion::Block,
        })?;
    let expected = NATIVE_BLOCK_HEADER_BYTES.checked_add(records_len).ok_or(
        NativeDoorError::SpanOverflow {
            region: SpanRegion::Block,
        },
    )?;
    if block.len() < expected {
        return Err(NativeDoorError::BlockTooShort {
            actual: block.len(),
            expected,
        });
    }
    if block.len() > expected {
        return Err(NativeDoorError::BlockTooLong {
            actual: block.len(),
            expected,
        });
    }

    let initial_return_bits = le_u64(&block[8..16]);
    let return_slot = GuestSpan {
        offset: block_offset
            .checked_add(8)
            .ok_or(NativeDoorError::SpanOverflow {
                region: SpanRegion::Block,
            })?,
        len: 8,
    };
    let mut arguments = Vec::with_capacity(encoded_arity);
    for (index, ty) in spec.parameters.iter().copied().enumerate() {
        let start = NATIVE_BLOCK_HEADER_BYTES + index * NATIVE_ARGUMENT_RECORD_BYTES;
        let record = &block[start..start + NATIVE_ARGUMENT_RECORD_BYTES];
        let kind = le_u32(&record[0..4]);
        let reserved = le_u32(&record[4..8]);
        let payload = le_u64(&record[8..16]);
        if reserved != 0 {
            return Err(NativeDoorError::RecordReservedNonZero {
                index,
                value: reserved,
            });
        }
        let argument = match kind {
            KIND_SCALAR if !ty.is_pointer() && ty != NativeType::Void => {
                NativeArgument::Scalar { ty, bits: payload }
            }
            KIND_SCALAR => {
                return Err(NativeDoorError::ArgumentKindMismatch { index, kind, ty });
            }
            KIND_GUEST_SPAN if ty.is_pointer() => {
                let offset = payload as u32;
                let len = (payload >> 32) as u32;
                offset
                    .checked_add(len)
                    .ok_or(NativeDoorError::SpanOverflow {
                        region: SpanRegion::Argument(index),
                    })?;
                checked_span(
                    memory,
                    offset as usize,
                    len as usize,
                    SpanRegion::Argument(index),
                )?;
                NativeArgument::GuestSpan {
                    ty,
                    span: GuestSpan {
                        offset: offset as usize,
                        len: len as usize,
                    },
                }
            }
            KIND_GUEST_SPAN => {
                return Err(NativeDoorError::ArgumentKindMismatch { index, kind, ty });
            }
            KIND_NULL if !ty.is_pointer() => {
                return Err(NativeDoorError::ArgumentKindMismatch { index, kind, ty });
            }
            KIND_NULL if !ty.is_nullable_pointer() => {
                return Err(NativeDoorError::NullForNonNullablePointer { index });
            }
            KIND_NULL if payload != 0 => {
                return Err(NativeDoorError::NullPayloadNonZero { index });
            }
            KIND_NULL => NativeArgument::Null { ty },
            KIND_HOST_ADDRESS => {
                return Err(NativeDoorError::HostAddressNotPermitted { index });
            }
            _ if kind > KIND_HOST_ADDRESS => {
                return Err(NativeDoorError::UnknownArgumentKind { index, kind });
            }
            _ => {
                return Err(NativeDoorError::ArgumentKindMismatch { index, kind, ty });
            }
        };
        arguments.push(argument);
    }
    let signature = classify_signature(&spec);
    Ok(DecodedNativeCall {
        spec,
        signature,
        return_slot,
        initial_return_bits,
        arguments,
    })
}

fn checked_span(
    memory: &[u8],
    offset: usize,
    len: usize,
    region: SpanRegion,
) -> Result<&[u8], NativeDoorError> {
    let end = offset
        .checked_add(len)
        .ok_or(NativeDoorError::SpanOverflow { region })?;
    if end > memory.len() {
        return Err(NativeDoorError::SpanOutOfBounds {
            region,
            end,
            memory_len: memory.len(),
        });
    }
    Ok(&memory[offset..end])
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("schema slices four bytes"))
}

fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("schema slices eight bytes"))
}
