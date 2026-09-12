//! Bounded schema and exact invocation table for `agenterm.native_call`.
//!
//! It owns the guest-authored declaration and argument-block format, turns
//! hostile guest bytes into typed bounded data, and admits only exact scalar
//! function-pointer types at the invocation boundary. The host door has one
//! conversion from [`NativeDoorError`] to [`crate::QjswasmError::Door`].

use std::fmt;

use libloading::Library;

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

/// Strongly typed schema and invocation failures. The door maps this enum once.
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
    DoorArgumentNegative {
        index: usize,
        value: i32,
    },
    InvocationSignatureUnsupported {
        result: NativeType,
        parameters: Vec<NativeType>,
    },
    ScalarNotCanonical {
        index: usize,
        ty: NativeType,
        bits: u64,
    },
    LibraryLoad {
        library: String,
        message: String,
    },
    SymbolLoad {
        symbol: String,
        message: String,
    },
}

impl NativeDoorError {
    /// Stable machine-readable name used by the single host-door mapping.
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
            Self::DoorArgumentNegative { .. } => "native_door_argument_negative",
            Self::InvocationSignatureUnsupported { .. } => {
                "native_invocation_signature_unsupported"
            }
            Self::ScalarNotCanonical { .. } => "native_scalar_not_canonical",
            Self::LibraryLoad { .. } => "native_library_load_failed",
            Self::SymbolLoad { .. } => "native_symbol_load_failed",
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
            Self::DoorArgumentNegative { index, value } => {
                write!(f, "door argument {index} is negative: {value}")
            }
            Self::InvocationSignatureUnsupported { result, parameters } => {
                write!(
                    f,
                    "invocation does not have one exact homogeneous scalar type: {result:?}({parameters:?})"
                )
            }
            Self::ScalarNotCanonical { index, ty, bits } => {
                write!(
                    f,
                    "argument {index} is not a canonical {ty:?} value: 0x{bits:016x}"
                )
            }
            Self::LibraryLoad { library, message } => {
                write!(f, "could not load native library {library:?}: {message}")
            }
            Self::SymbolLoad { symbol, message } => {
                write!(f, "could not resolve native symbol {symbol:?}: {message}")
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

/// Number of exact Rust `extern C` stubs admitted by the executable slice.
///
/// This is intentionally smaller than [`native_register_pattern_cardinality`]:
/// seven exact scalar types, each at every arity from zero through six. The
/// result type and every parameter have to be that same exact type.
pub fn native_invocation_stub_cardinality() -> usize {
    let exact_scalar_types = [
        NativeType::I32,
        NativeType::U32,
        NativeType::I64,
        NativeType::U64,
        NativeType::Isize,
        NativeType::Usize,
        NativeType::F64,
    ];
    exact_scalar_types.len() * (0..=MAX_NATIVE_ARITY).count()
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

macro_rules! invoke_homogeneous {
    ($library:expr, $call:expr, $args:expr, $ty:ty) => {{
        match $args.as_slice() {
            [] => invoke_0::<$ty>($library, $call),
            [a] => invoke_1::<$ty>($library, $call, *a),
            [a, b] => invoke_2::<$ty>($library, $call, *a, *b),
            [a, b, c] => invoke_3::<$ty>($library, $call, *a, *b, *c),
            [a, b, c, d] => invoke_4::<$ty>($library, $call, *a, *b, *c, *d),
            [a, b, c, d, e] => invoke_5::<$ty>($library, $call, *a, *b, *c, *d, *e),
            [a, b, c, d, e, f] => invoke_6::<$ty>($library, $call, *a, *b, *c, *d, *e, *f),
            _ => unreachable!("the schema caps native arity at six"),
        }?
    }};
}

/// Invoke one decoded call and publish its result into the guest block.
///
/// The first executable slice deliberately supports only exact homogeneous
/// scalar signatures. A register class is not a Rust function-pointer type:
/// accepting every GP width or mixed GP/F64 pattern would turn a declaration
/// typo into undefined behaviour. Each admitted family below has seven fixed
/// stubs (arity zero through six) whose Rust type exactly matches the declared
/// native type. Pointers, narrow integers, mixed signatures, `void`, `f32`,
/// variadics and structure values remain typed refusals.
pub(crate) fn invoke_native_call(
    memory: &mut [u8],
    call: &DecodedNativeCall,
) -> Result<(), NativeDoorError> {
    let bits = match call.spec.result {
        NativeType::I32 if all_parameters(call, NativeType::I32) => {
            let args = exact_i32_arguments(call)?;
            let library = open_library(&call.spec.library)?;
            invoke_homogeneous!(&library, call, args, i32) as i64 as u64
        }
        NativeType::U32 if all_parameters(call, NativeType::U32) => {
            let args = exact_u32_arguments(call)?;
            let library = open_library(&call.spec.library)?;
            invoke_homogeneous!(&library, call, args, u32) as u64
        }
        NativeType::I64 if all_parameters(call, NativeType::I64) => {
            let args = exact_i64_arguments(call)?;
            let library = open_library(&call.spec.library)?;
            invoke_homogeneous!(&library, call, args, i64) as u64
        }
        NativeType::U64 if all_parameters(call, NativeType::U64) => {
            let args = exact_u64_arguments(call)?;
            let library = open_library(&call.spec.library)?;
            invoke_homogeneous!(&library, call, args, u64)
        }
        NativeType::Isize if all_parameters(call, NativeType::Isize) => {
            let args = exact_isize_arguments(call)?;
            let library = open_library(&call.spec.library)?;
            invoke_homogeneous!(&library, call, args, isize) as i64 as u64
        }
        NativeType::Usize if all_parameters(call, NativeType::Usize) => {
            let args = exact_usize_arguments(call)?;
            let library = open_library(&call.spec.library)?;
            invoke_homogeneous!(&library, call, args, usize) as u64
        }
        NativeType::F64 if all_parameters(call, NativeType::F64) => {
            let args = exact_f64_arguments(call)?;
            let library = open_library(&call.spec.library)?;
            invoke_homogeneous!(&library, call, args, f64).to_bits()
        }
        _ => {
            return Err(NativeDoorError::InvocationSignatureUnsupported {
                result: call.spec.result,
                parameters: call.spec.parameters.clone(),
            });
        }
    };
    let memory_len = memory.len();
    let slot = memory
        .get_mut(call.return_slot.offset..call.return_slot.offset + call.return_slot.len)
        .ok_or(NativeDoorError::SpanOutOfBounds {
            region: SpanRegion::Block,
            end: call.return_slot.offset + call.return_slot.len,
            memory_len,
        })?;
    slot.copy_from_slice(&bits.to_le_bytes());
    Ok(())
}

fn all_parameters(call: &DecodedNativeCall, ty: NativeType) -> bool {
    call.spec
        .parameters
        .iter()
        .all(|candidate| *candidate == ty)
}

macro_rules! exact_arguments {
    ($name:ident, $ty:ty, $native:expr, $convert:expr) => {
        fn $name(call: &DecodedNativeCall) -> Result<Vec<$ty>, NativeDoorError> {
            call.arguments
                .iter()
                .enumerate()
                .map(|(index, argument)| match argument {
                    NativeArgument::Scalar { ty, bits } if *ty == $native => ($convert)(*bits)
                        .ok_or(NativeDoorError::ScalarNotCanonical {
                            index,
                            ty: *ty,
                            bits: *bits,
                        }),
                    _ => Err(NativeDoorError::InvocationSignatureUnsupported {
                        result: call.spec.result,
                        parameters: call.spec.parameters.clone(),
                    }),
                })
                .collect()
        }
    };
}

exact_arguments!(exact_i32_arguments, i32, NativeType::I32, |bits: u64| {
    let value = bits as i32;
    (value as i64 as u64 == bits).then_some(value)
});
exact_arguments!(exact_u32_arguments, u32, NativeType::U32, |bits: u64| {
    u32::try_from(bits).ok()
});
exact_arguments!(exact_i64_arguments, i64, NativeType::I64, |bits: u64| {
    Some(bits as i64)
});
exact_arguments!(exact_u64_arguments, u64, NativeType::U64, |bits: u64| {
    Some(bits)
});
exact_arguments!(
    exact_isize_arguments,
    isize,
    NativeType::Isize,
    |bits: u64| {
        let value = bits as isize;
        (value as i64 as u64 == bits).then_some(value)
    }
);
exact_arguments!(
    exact_usize_arguments,
    usize,
    NativeType::Usize,
    |bits: u64| { usize::try_from(bits).ok() }
);
exact_arguments!(exact_f64_arguments, f64, NativeType::F64, |bits: u64| {
    Some(f64::from_bits(bits))
});

fn symbol_error(call: &DecodedNativeCall, error: libloading::Error) -> NativeDoorError {
    NativeDoorError::SymbolLoad {
        symbol: call.spec.symbol.clone(),
        message: error.to_string(),
    }
}

macro_rules! typed_invoker {
    ($name:ident, ($($arg:ident),*)) => {
        #[allow(clippy::too_many_arguments)]
        fn $name<T: Copy>(
            library: &Library,
            call: &DecodedNativeCall,
            $($arg: T),*
        ) -> Result<T, NativeDoorError> {
            // SAFETY: the caller selects this exact Rust `extern C` signature
            // only for the same exact declared scalar type and arity. As with
            // every dlsym-style API, the declaration is the caller's unsafe
            // assertion that the exported symbol has that signature.
            let function = unsafe {
                library.get::<unsafe extern "C" fn($($arg: T),*) -> T>(call.spec.symbol.as_bytes())
            }
            .map_err(|error| symbol_error(call, error))?;
            // SAFETY: the canonical arguments have the exact `T` selected
            // above, and the Library remains alive for the call.
            Ok(unsafe { function($($arg),*) })
        }
    };
}

typed_invoker!(invoke_0, ());
typed_invoker!(invoke_1, (a));
typed_invoker!(invoke_2, (a, b));
typed_invoker!(invoke_3, (a, b, c));
typed_invoker!(invoke_4, (a, b, c, d));
typed_invoker!(invoke_5, (a, b, c, d, e));
typed_invoker!(invoke_6, (a, b, c, d, e, f));

#[cfg(unix)]
fn current_process_library() -> Result<Library, libloading::Error> {
    Ok(libloading::os::unix::Library::this().into())
}

#[cfg(windows)]
fn current_process_library() -> Result<Library, libloading::Error> {
    libloading::os::windows::Library::this().map(Into::into)
}

fn open_library(name: &str) -> Result<Library, NativeDoorError> {
    if name.is_empty() {
        return current_process_library().map_err(|error| NativeDoorError::LibraryLoad {
            library: "<current-process>".to_owned(),
            message: error.to_string(),
        });
    }
    // SAFETY: loading a library deliberately executes that library's
    // initializer/finalizer routines. This unrestricted runtime operation is
    // synchronous, and the handle remains alive through symbol invocation.
    unsafe { Library::new(name) }.map_err(|error| NativeDoorError::LibraryLoad {
        library: name.to_owned(),
        message: error.to_string(),
    })
}
