//! Bounded schema and typed invocation dispatch for `agenterm.native_call`.
//!
//! It owns the guest-authored declaration and argument-block format, turns
//! hostile guest bytes into typed bounded data, and admits only enumerated
//! scalar function-pointer types at the invocation boundary. The host door has
//! one conversion from [`NativeDoorError`] to [`crate::QjswasmError::Door`].

use std::fmt;

use agenterm_dyn::{AbiValue, UnixIoctlError, UnixIoctlRequest, invoke_unix_ioctl};

use crate::native_cache::{LibrarySource, NativeLibraryCache};

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

/// Scalar positions admitted by the qjswasm exact-family catalog.
///
/// This is product/schema data owned here. dyn independently decides whether
/// the caller-provided ABI signature has a concrete mechanism trampoline.
const EXACT_SCALAR_TYPES: [NativeType; 7] = [
    NativeType::I32,
    NativeType::U32,
    NativeType::I64,
    NativeType::U64,
    NativeType::Isize,
    NativeType::Usize,
    NativeType::F64,
];

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
    ResultPointerOutsideGuestSpans,
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
    InvocationTargetUnsupported {
        operation: &'static str,
    },
    ScalarNotCanonical {
        index: usize,
        ty: NativeType,
        bits: u64,
    },
    ArgumentsNotUtf8,
    ArgumentsMalformed,
    ArgumentCountMismatch {
        declared: usize,
        actual: usize,
    },
    ArgumentValueInvalid {
        index: usize,
        ty: NativeType,
    },
    /// A pointer parameter position arrived without a call-scoped region.
    NativeRegionRequired {
        index: usize,
    },
    /// The region record itself is malformed. `reason` is a bounded,
    /// stable sentence; the code is what a caller matches on.
    NativeRegionShapeInvalid {
        index: usize,
        reason: &'static str,
    },
    /// This call's regions together exceed the slot's region budget. Charged
    /// before any allocation, and a refusal rather than a smaller region.
    NativeRegionTooLarge {
        requested: usize,
        maximum: usize,
    },
    /// `termination: "nul"` and the call left no NUL in the region.
    NativeRegionUnterminated {
        index: usize,
        native_status: i32,
    },
    /// `output: "text"` and the bytes before the terminator are not UTF-8.
    NativeRegionNotUtf8 {
        index: usize,
        native_status: i32,
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
            Self::ResultPointerOutsideGuestSpans => "native_result_pointer_outside_guest_spans",
            Self::ArgumentKindMismatch { .. } => "native_argument_kind_mismatch",
            Self::NullForNonNullablePointer { .. } => "native_null_not_permitted",
            Self::NullPayloadNonZero { .. } => "native_null_payload_nonzero",
            Self::DoorArgumentNegative { .. } => "native_door_argument_negative",
            Self::InvocationSignatureUnsupported { .. } => {
                "native_invocation_signature_unsupported"
            }
            Self::InvocationTargetUnsupported { .. } => "native_invocation_target_unsupported",
            Self::ScalarNotCanonical { .. } => "native_scalar_not_canonical",
            Self::ArgumentsNotUtf8 => "native_arguments_not_utf8",
            Self::ArgumentsMalformed => "native_arguments_malformed",
            Self::ArgumentCountMismatch { .. } => "native_argument_count_mismatch",
            Self::ArgumentValueInvalid { .. } => "native_argument_value_invalid",
            Self::NativeRegionRequired { .. } => "native_region_required",
            Self::NativeRegionShapeInvalid { .. } => "native_region_shape_invalid",
            Self::NativeRegionTooLarge { .. } => "native_region_too_large",
            Self::NativeRegionUnterminated { .. } => "native_region_unterminated",
            Self::NativeRegionNotUtf8 { .. } => "native_region_not_utf8",
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
            Self::ResultPointerOutsideGuestSpans => {
                f.write_str("native result pointer is outside every declared guest span")
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
            Self::InvocationTargetUnsupported { operation } => {
                write!(f, "{operation} is unsupported on this target")
            }
            Self::ScalarNotCanonical { index, ty, bits } => {
                write!(
                    f,
                    "argument {index} is not a canonical {ty:?} value: 0x{bits:016x}"
                )
            }
            Self::ArgumentsNotUtf8 => f.write_str("argument document is not UTF-8"),
            Self::ArgumentsMalformed => f.write_str("argument document must be a JSON array"),
            Self::ArgumentCountMismatch { declared, actual } => {
                write!(
                    f,
                    "declaration has {declared} parameters, JSON has {actual}"
                )
            }
            Self::ArgumentValueInvalid { index, ty } => {
                write!(f, "argument {index} is not an exact JSON value for {ty:?}")
            }
            Self::NativeRegionRequired { index } => {
                write!(
                    f,
                    "argument {index} is a pointer position and needs one region record"
                )
            }
            Self::NativeRegionShapeInvalid { index, reason } => {
                write!(f, "argument {index} region record: {reason}")
            }
            Self::NativeRegionTooLarge { requested, maximum } => {
                write!(
                    f,
                    "arguments ask for {requested} region bytes, maximum {maximum}"
                )
            }
            Self::NativeRegionUnterminated {
                index,
                native_status,
            } => {
                write!(
                    f,
                    "argument {index} region has no NUL after the call though termination is \"nul\"; native status was {native_status}"
                )
            }
            Self::NativeRegionNotUtf8 {
                index,
                native_status,
            } => {
                write!(
                    f,
                    "argument {index} region output is not UTF-8 though output is \"text\"; native status was {native_status}"
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
    Ok(DecodedNativeCall {
        spec,
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

/// Invoke one decoded call and publish its result into the guest block.
///
/// The executable slice supports the exact homogeneous families plus a small
/// enumerated set of heterogeneous scalar and synchronous caller-buffer
/// prototypes. A register class is not
/// a Rust function-pointer type: accepting every GP width or mixed GP/F64
/// pattern would turn a declaration typo into undefined behaviour. Pointer
/// returns, retained pointers, narrow integers, unlisted mixed signatures,
/// `f32`, variadics and structure values remain typed refusals. The one
/// catalogued void shape is `void(ptr?)`, used for `free(NULL)`.
///
/// `libraries` is the engine's loaded-library table. It changes where a load
/// comes from, never whether a call is admitted: the catalog above still decides
/// that, a full table takes the one-shot entry rather than a refusal, and a load
/// that failed is reported once instead of being re-attempted.
pub(crate) fn invoke_native_call(
    memory: &mut [u8],
    call: &DecodedNativeCall,
    libraries: &NativeLibraryCache,
) -> Result<(), NativeDoorError> {
    // Take the guest memory base once. Pointer prototypes receive raw addresses
    // derived from this allocation, never overlapping `&mut` slices: guest
    // spans are allowed to alias intentionally.
    let memory_base = memory.as_mut_ptr();
    let memory_len = memory.len();
    let bits = match native_dispatch(&call.spec)? {
        dispatch @ (NativeDispatch::Scalar | NativeDispatch::Pointer) => {
            // For pointers, the guest remains the unsafe ABI caller: it must
            // declare spans large and aligned enough for the selected C symbol's
            // complete pointee contract. The generic door cannot infer that
            // contract from an opaque `ptr` prototype.
            let arguments = call
                .arguments
                .iter()
                .enumerate()
                .map(|(index, argument)| match dispatch {
                    NativeDispatch::Scalar => exact_argument(index, argument, &call.spec),
                    NativeDispatch::Pointer => {
                        fixed_pointer_argument(memory_base, index, argument, &call.spec)
                    }
                    NativeDispatch::UnixIoctl(_) => unreachable!(),
                })
                .collect::<Result<Vec<_>, _>>()?;
            // SAFETY: the guest declaration is the native-door caller's explicit
            // ABI assertion, and native_dispatch admitted the exact scalar family
            // or one enumerated fixed-pointer prototype. For pointers,
            // decode_native_call bounded every declared span within this one live
            // memory allocation; the foreign call is synchronous.
            unsafe { invoke_prepared(&call.spec, &arguments, libraries) }
                .and_then(|value| abi_result_bits(value, call, memory_base, memory_len))?
        }
        NativeDispatch::UnixIoctl(prototype) => {
            let fd = ioctl_i32_argument(0, &call.arguments[0], &call.spec)?;
            let request = match prototype {
                UnixIoctlPrototype::I32Request => UnixIoctlRequest::I32Bits(ioctl_i32_argument(
                    1,
                    &call.arguments[1],
                    &call.spec,
                )?),
                UnixIoctlPrototype::U64Request => {
                    UnixIoctlRequest::U64(ioctl_u64_argument(1, &call.arguments[1], &call.spec)?)
                }
            };
            let argument = ioctl_pointer_argument(memory_base, 2, &call.arguments[2], &call.spec)?;
            // SAFETY: this is the one enumerated Unix variadic prototype. The
            // decoder bounded the caller-owned guest span and the call is
            // synchronous; the request-specific pointee contract remains the
            // native-door caller's explicit ABI assertion.
            unsafe { invoke_unix_ioctl(fd, request, argument) }
                .map(|value| value as i64 as u64)
                .map_err(map_unix_ioctl_error)?
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

/// Invoke the scalar native slice from a language-level JSON argument array.
///
/// This is the `.qjs` adapter for the existing door, not a second native
/// implementation: parsing and canonical conversion remain here, while the
/// Parsing, the canonical conversion and the catalog remain here in qjswasm;
/// **execution delegates to `agenterm_dyn::invoke_abi`**, the policy-free ABI entry.
///
/// `libraries` is the engine's loaded-library table, exactly as in
/// [`invoke_native_call`].
///
/// # Call-scoped regions
///
/// A JSON caller has no guest linear memory to point into, so a pointer
/// parameter position takes one **region record** instead of a raw address:
///
/// ```text
/// {"region":{"capacity":128,"bytes":[47,116,109,112],"termination":"nul","output":"text"}}
/// ```
///
/// The host allocates that storage for exactly this call, zero-fills it,
/// copies `bytes` into the front, and passes its address to the foreign
/// function. `capacity` is required, positive, at least `bytes.len()`, and
/// billed against `max_region_bytes` for the whole call *before* anything is
/// allocated. `bytes` may be omitted (then empty). `termination` is `"nul"`
/// (the input must contain no NUL and must leave room for one; after the call
/// the region must contain a NUL) or `"raw"` (no terminator is added or
/// required). `output` is `"text"` (answer the bytes before the terminator,
/// which must be UTF-8; needs `termination: "nul"`) or `"bytes"` (answer them
/// as integers -- before the terminator for `"nul"`, the whole capacity for
/// `"raw"`).
///
/// Every pointer position needs its own region; a scalar or `null` there is a
/// typed refusal, not a null pointer. Nothing about a region survives the
/// call: the answer carries no address, no handle, no registry and no guest
/// offset, and there is no cross-call lifetime. The readback happens whether
/// the callee reported success or failure, so a region the callee left
/// unterminated or non-UTF-8 refuses the whole call rather than being hidden
/// behind a status.
///
/// Only the pointer signatures whose result is `i32` are served here; a pointer result has no guest span to
/// rebase onto and stays refused, as do the `i64`/`void` pointer shapes and the
/// Unix `ioctl` shapes. The guest remains the unsafe ABI caller: an opaque
/// `ptr` position does not tell the door how many bytes the selected C symbol
/// writes, so an under-sized region is exactly the `native_call` hazard.
pub(crate) fn invoke_native_json(
    spec: &[u8],
    arguments_json: &[u8],
    libraries: &NativeLibraryCache,
    max_region_bytes: usize,
) -> Result<String, NativeDoorError> {
    let spec = parse_native_spec(spec)?;
    let dispatch = native_dispatch(&spec)?;
    // The shapes this adapter still does not serve are refused before the JSON
    // is even parsed, so a declaration mistake keeps outranking an argument
    // mistake. Exhaustive on purpose: a new dispatch variant cannot slip
    // through unclassified.
    let refused = match dispatch {
        NativeDispatch::Scalar => false,
        NativeDispatch::Pointer => spec.result != NativeType::I32,
        NativeDispatch::UnixIoctl(_) => true,
    };
    if refused {
        return Err(NativeDoorError::InvocationSignatureUnsupported {
            result: spec.result,
            parameters: spec.parameters.clone(),
        });
    }
    let text =
        std::str::from_utf8(arguments_json).map_err(|_| NativeDoorError::ArgumentsNotUtf8)?;
    let values = serde_json::from_str::<serde_json::Value>(text)
        .map_err(|_| NativeDoorError::ArgumentsMalformed)?;
    let values = values
        .as_array()
        .ok_or(NativeDoorError::ArgumentsMalformed)?;
    if values.len() != spec.parameters.len() {
        return Err(NativeDoorError::ArgumentCountMismatch {
            declared: spec.parameters.len(),
            actual: values.len(),
        });
    }
    let value = match dispatch {
        NativeDispatch::Scalar => {
            let arguments = spec
                .parameters
                .iter()
                .copied()
                .zip(values)
                .enumerate()
                .map(|(index, (ty, value))| exact_json_argument(index, ty, value))
                .collect::<Result<Vec<_>, _>>()?;
            // SAFETY: native_dispatch admitted this exact or enumerated scalar
            // declaration for the JSON argument encoding above.
            unsafe { invoke_prepared(&spec, &arguments, libraries) }.and_then(|value| {
                abi_json_result(value, spec.result).ok_or_else(|| unsupported_json_spec(&spec))
            })?
        }
        NativeDispatch::Pointer => {
            debug_assert_eq!(
                spec.result,
                NativeType::I32,
                "the refusal above admits only i32-returning pointer signatures"
            );
            invoke_pointer_json(&spec, values, libraries, max_region_bytes)?
        }
        NativeDispatch::UnixIoctl(_) => unreachable!("ioctl JSON calls reject above"),
    };
    Ok(value.to_string())
}

/// One admitted pointer call, with every pointer position backed by a region.
///
/// The layout is built in three phases on purpose. The first decodes every
/// argument into a plan without allocating its pointee buffer. The second checks the whole
/// call's storage and conservative encoded-answer bounds. Only then does the
/// third allocate regions and turn them into addresses. No region is added
/// after an address is taken, so every pointer stays fixed for the synchronous
/// call, and a later oversized argument cannot leave earlier buffers allocated.
///
/// The answer is the scalar result plus one entry per pointer position:
/// `{"type":"i32","value":0,"regions":[...]}`, in the same `"type"`/`"value"`
/// vocabulary the scalar path already answers with. A region entry states the
/// declaration it was read under (`output`, `termination`) and the bytes it
/// holds; it never states a written length the door would have to guess.
fn invoke_pointer_json(
    spec: &NativeSpec,
    values: &[serde_json::Value],
    libraries: &NativeLibraryCache,
    max_region_bytes: usize,
) -> Result<serde_json::Value, NativeDoorError> {
    let mut plans: Vec<RegionPlan> = Vec::new();
    let mut layout: Vec<JsonPointerArgument> = Vec::with_capacity(values.len());
    for (index, (ty, value)) in spec.parameters.iter().copied().zip(values).enumerate() {
        if ty.is_pointer() {
            let plan = RegionPlan::from_json(index, value)?;
            let region_index = plans.len();
            plans.push(plan);
            layout.push(JsonPointerArgument::Region(region_index));
        } else {
            // A scalar position keeps the exact value grammar it has in the
            // fixed family; only the pointer positions changed meaning.
            layout.push(JsonPointerArgument::Scalar(exact_json_argument(
                index, ty, value,
            )?));
        }
    }

    preflight_region_plans(&plans, max_region_bytes)?;
    let mut regions: Vec<NativeRegion> = plans.into_iter().map(RegionPlan::materialize).collect();

    let mut arguments: Vec<agenterm_dyn::AbiValue> = Vec::with_capacity(layout.len());
    let mut positions: Vec<(usize, usize)> = Vec::new();
    for (index, argument) in layout.into_iter().enumerate() {
        match argument {
            JsonPointerArgument::Region(region) => {
                arguments.push(agenterm_dyn::AbiValue::Pointer(regions[region].pointer()));
                positions.push((index, region));
            }
            JsonPointerArgument::Scalar(value) => arguments.push(value),
        }
    }

    // SAFETY: native_dispatch admitted one enumerated pointer prototype and the
    // JSON adapter admitted it only because its result is `i32`; every pointer
    // argument is storage this call allocated and zero-filled, and the foreign
    // call is synchronous, so the addresses stay valid for its whole duration.
    // The guest still owns the pointee contract: an opaque `ptr` position does
    // not say how many bytes the selected C symbol writes.
    let value = unsafe { invoke_prepared(spec, &arguments, libraries) }?;
    let agenterm_dyn::AbiValue::I32(status) = value else {
        return Err(unsupported_json_spec(spec));
    };

    let mut answer = serde_json::Map::new();
    answer.insert("type".to_owned(), serde_json::Value::from("i32"));
    answer.insert("value".to_owned(), serde_json::Value::from(status));
    answer.insert(
        "regions".to_owned(),
        serde_json::Value::Array(
            positions
                .iter()
                .map(|(index, region)| regions[*region].readback(*index, status))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    Ok(serde_json::Value::Object(answer))
}

/// One already-validated argument of a pointer call.
enum JsonPointerArgument {
    /// A region this call allocated; the value is its index in that call's
    /// region list, never an address.
    Region(usize),
    Scalar(agenterm_dyn::AbiValue),
}

/// The refusal this crate states for a declaration it does not execute.
///
/// One construction serves every refusal of this shape: the JSON adapter's
/// pre-classification, the ABI position conversion, the raw family argument
/// converters and the post-execution result encoders all refuse by naming the
/// declared result and parameters. Those four paths used to build the same
/// variant three times over.
fn unsupported_json_spec(spec: &NativeSpec) -> NativeDoorError {
    NativeDoorError::InvocationSignatureUnsupported {
        result: spec.result,
        parameters: spec.parameters.clone(),
    }
}

/// Run one admitted call through this engine's loaded handle when it has one.
///
/// The cache decides where a load comes from and nothing else. A hit reuses a
/// load this engine has already paid for; a full table takes the one-shot entry,
/// which is exactly what this crate did before the cache existed; a load that
/// failed is **not** retried — the error the cache already holds goes to this
/// function's caller, whose existing mapping reports it, because a second
/// attempt would run the library's initialisers twice for one call.
///
/// # Safety
///
/// The caller asserts `agenterm_dyn::invoke_abi`'s complete ABI contract for
/// `call`. A cached handle was opened for exactly the string `call` names, which
/// is the one thing `invoke_abi_with_handle` re-checks.
///
/// [`invoke_prepared`] is its only caller.
unsafe fn invoke_with(
    libraries: &NativeLibraryCache,
    call: &agenterm_dyn::NativeCall<'_>,
) -> Result<AbiValue, agenterm_dyn::AbiError> {
    match libraries.resolve(call.library) {
        // SAFETY: forwarded from this function's caller.
        Ok(LibrarySource::Cached(handle)) => {
            // The door's own evidence that this arm is the one that ran: a door
            // that resolved a handle and then called the one-shot entry instead
            // would leave this count at zero and redden the owning test.
            #[cfg(all(test, unix))]
            libraries.note_cached_hit();
            unsafe { agenterm_dyn::invoke_abi_with_handle(&handle, call) }
        }
        // SAFETY: forwarded from this function's caller; the one-shot entry
        // loads and checks everything itself.
        Ok(LibrarySource::AtCapacity) => unsafe { agenterm_dyn::invoke_abi(call) },
        // The load already ran once, in the table. A second attempt would run
        // the library's initialisers twice for one call, so dyn's own error goes
        // to the caller's existing mapping untouched.
        Err(error) => Err(error),
    }
}

/// Run one already-prepared call through dyn's policy-free ABI entry.
///
/// This is the **one** execution phase of the six admitted production call
/// sites: the raw exact / fixed / fixed-pointer families of
/// [`invoke_native_call`] and the JSON exact / fixed / pointer-region paths of
/// [`invoke_native_json`]. It takes exactly what those sites disagreed about —
/// the declaration and the already-converted `AbiValue` positions — and owns
/// what they repeated: the parameter conversion, the call construction and the
/// loaded-handle reuse policy of [`invoke_with`]. Everything that stays
/// family-specific is post-processing at the call site and is deliberately
/// absent here: the raw path's result-position check and pointer rebase, the
/// JSON path's scalar encoding, and the region path's post-call readback.
///
/// Unix `ioctl` keeps its own entry (`invoke_unix_ioctl`) and does not call
/// this function: it has a request argument and a variadic pointee that the
/// described-argument entry cannot express.
///
/// # Safety
///
/// The caller asserts `agenterm_dyn::invoke_abi`'s complete ABI contract for the
/// symbol `spec` names, and that this declaration is one `native_dispatch`
/// admitted for the argument encoding it converted. Nothing here re-checks the
/// catalog: admission, nullability and guest-storage policy belong to the
/// declaration parser and the family argument converters above it.
unsafe fn invoke_prepared(
    spec: &NativeSpec,
    arguments: &[AbiValue],
    libraries: &NativeLibraryCache,
) -> Result<AbiValue, NativeDoorError> {
    let abi_params = abi_parameters_for_spec(spec)?;
    let abi_call = agenterm_dyn::NativeCall {
        library: &spec.library,
        symbol: &spec.symbol,
        signature: agenterm_dyn::AbiSignature {
            result: abi_type(spec.result).ok_or_else(|| unsupported_json_spec(spec))?,
            params: &abi_params,
        },
        arguments,
    };
    // SAFETY: forwarded from this function's caller.
    unsafe { invoke_with(libraries, &abi_call) }.map_err(|error| map_abi_error(spec, error))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeDispatch {
    Scalar,
    Pointer,
    UnixIoctl(UnixIoctlPrototype),
}

/// Heterogeneous scalar shapes exposed by the qjswasm native catalog.
const HETEROGENEOUS_SCALAR_SIGNATURES: &[(NativeType, &[NativeType])] = &[
    (NativeType::I32, &[NativeType::U64, NativeType::U64]),
    (NativeType::U64, &[NativeType::I32]),
    (NativeType::Isize, &[NativeType::I32]),
    (
        NativeType::I64,
        &[NativeType::I32, NativeType::I64, NativeType::I32],
    ),
    (NativeType::I32, &[NativeType::U32, NativeType::U32]),
    (NativeType::I32, &[NativeType::I32, NativeType::U32]),
];

/// Pointer-bearing shapes exposed by the qjswasm native catalog.
///
/// Nullable positions remain explicit declaration data: raw decoding uses them
/// to decide where `KIND_NULL` is valid, while dyn receives an ordinary pointer.
const POINTER_SIGNATURES: &[(NativeType, &[NativeType])] = &[
    (NativeType::Void, &[NativeType::NullablePointer]),
    (NativeType::I64, &[NativeType::NullablePointer]),
    (NativeType::I64, &[NativeType::Pointer]),
    (
        NativeType::Pointer,
        &[NativeType::Pointer, NativeType::Usize],
    ),
    (NativeType::I32, &[NativeType::Pointer]),
    (NativeType::I32, &[NativeType::I32, NativeType::Pointer]),
    (NativeType::I32, &[NativeType::Pointer, NativeType::I32]),
    (
        NativeType::I32,
        &[NativeType::Pointer, NativeType::NullablePointer],
    ),
    (
        NativeType::I32,
        &[NativeType::NullablePointer, NativeType::Pointer],
    ),
    (NativeType::I32, &[NativeType::Pointer, NativeType::Pointer]),
    (
        NativeType::I32,
        &[
            NativeType::Pointer,
            NativeType::Pointer,
            NativeType::Pointer,
        ],
    ),
    (NativeType::I32, &[NativeType::Pointer, NativeType::U64]),
    (
        NativeType::I32,
        &[NativeType::I32, NativeType::Pointer, NativeType::U32],
    ),
    (
        NativeType::I32,
        &[NativeType::U64, NativeType::Pointer, NativeType::U64],
    ),
];

/// The natural alignment every call-scoped region carries.
///
/// A region is host-allocated storage handed to an opaque `ptr` position, and
/// the door cannot know what the selected C symbol expects. 16 bytes is the
/// largest natural alignment an admitted prototype can require on a repository
/// target (`long double` / `max_align_t` on both SysV x86_64 and AArch64), so
/// the storage is over-allocated to a whole number of these units and every
/// region starts aligned for anything a C callee may assume.
const REGION_ALIGNMENT: usize = 16;

/// One alignment unit of region storage. The wrapper exists only so the
/// *allocation* — not just the struct — carries [`REGION_ALIGNMENT`]: a
/// `Vec<u8>` may be byte-aligned, a `Vec<RegionWord>` is not.
#[repr(align(16))]
#[derive(Clone, Copy)]
struct RegionWord([u8; REGION_ALIGNMENT]);

impl RegionWord {
    /// The zeroed unit every region starts as.
    const ZEROED: Self = Self([0; REGION_ALIGNMENT]);

    /// The unit's own bytes.
    ///
    /// Reading the array through the wrapper is what makes the region and the
    /// alignment unit *the same* storage rather than two allocations that
    /// happen to agree in size.
    fn bytes(&self) -> &[u8; REGION_ALIGNMENT] {
        &self.0
    }
}

/// The region contract as one choice rather than two independent fields.
///
/// `termination` and `output` are not orthogonal: `"raw"` has no end, so
/// `"raw" + "text"` has nothing to decode. Keeping the three admitted
/// combinations in an enum makes the refused fourth unrepresentable, so the
/// readback cannot be reached in a state the decoder never validated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegionContract {
    /// NUL-terminated; answer as text (the bytes before the terminator).
    NulText,
    /// NUL-terminated; answer as the bytes before the terminator.
    NulBytes,
    /// No terminator; answer as every byte of the capacity.
    RawBytes,
}

impl RegionContract {
    fn termination(self) -> &'static str {
        match self {
            Self::NulText | Self::NulBytes => "nul",
            Self::RawBytes => "raw",
        }
    }

    fn output(self) -> &'static str {
        match self {
            Self::NulText => "text",
            Self::NulBytes | Self::RawBytes => "bytes",
        }
    }

    fn requires_nul(self) -> bool {
        matches!(self, Self::NulText | Self::NulBytes)
    }
}

/// The JSON key a pointer argument's region record lives under.
const REGION_KEY: &str = "region";

/// Conservative JSON envelope bytes independent of region contents.
const REGION_ANSWER_BASE_BYTES: usize = 64;
/// Conservative JSON metadata bytes for one entry in `regions`.
const REGION_ANSWER_ENTRY_BYTES: usize = 96;

/// A validated region description before its pointee buffer is allocated.
struct RegionPlan {
    input: Vec<u8>,
    capacity: usize,
    contract: RegionContract,
}

impl RegionPlan {
    /// Decode one `{"region": {...}}` argument without allocating its pointee.
    fn from_json(index: usize, value: &serde_json::Value) -> Result<Self, NativeDoorError> {
        let shape =
            |reason: &'static str| NativeDoorError::NativeRegionShapeInvalid { index, reason };
        let Some(argument) = value.as_object() else {
            return Err(NativeDoorError::NativeRegionRequired { index });
        };
        if !argument.contains_key(REGION_KEY) {
            return Err(NativeDoorError::NativeRegionRequired { index });
        }
        if argument.len() != 1 {
            return Err(shape("the record carries a key beside \"region\""));
        }
        let Some(record) = argument[REGION_KEY].as_object() else {
            return Err(shape("region must be an object"));
        };
        for key in record.keys() {
            if !matches!(
                key.as_str(),
                "capacity" | "bytes" | "termination" | "output"
            ) {
                return Err(shape(
                    "region admits only capacity, bytes, termination and output",
                ));
            }
        }
        let capacity = record
            .get("capacity")
            .and_then(serde_json::Value::as_u64)
            .and_then(|capacity| usize::try_from(capacity).ok())
            .filter(|capacity| *capacity > 0)
            .ok_or_else(|| shape("capacity must be a positive integer"))?;
        let input = match record.get("bytes") {
            None => Vec::new(),
            Some(bytes) => bytes
                .as_array()
                .ok_or_else(|| shape("bytes must be an array of 0..=255"))?
                .iter()
                .map(|byte| {
                    byte.as_u64()
                        .filter(|byte| *byte <= u64::from(u8::MAX))
                        .map(|byte| byte as u8)
                        .ok_or_else(|| shape("bytes must be an array of 0..=255"))
                })
                .collect::<Result<Vec<u8>, NativeDoorError>>()?,
        };
        if input.len() > capacity {
            return Err(shape("capacity must not be smaller than bytes"));
        }
        let termination = record
            .get("termination")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| shape("termination must be \"nul\" or \"raw\""))?;
        let output = record
            .get("output")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| shape("output must be \"text\" or \"bytes\""))?;
        let contract = match (termination, output) {
            ("nul", "text") => RegionContract::NulText,
            ("nul", "bytes") => RegionContract::NulBytes,
            ("raw", "bytes") => RegionContract::RawBytes,
            ("raw", "text") => {
                return Err(shape("output \"text\" needs termination \"nul\""));
            }
            ("nul" | "raw", _) => return Err(shape("output must be \"text\" or \"bytes\"")),
            _ => return Err(shape("termination must be \"nul\" or \"raw\"")),
        };
        if contract.requires_nul() {
            if input.contains(&0) {
                return Err(shape("termination \"nul\" refuses input bytes of NUL"));
            }
            if input.len() >= capacity {
                return Err(shape(
                    "termination \"nul\" needs one byte of room for the terminator",
                ));
            }
        }
        Ok(Self {
            input,
            capacity,
            contract,
        })
    }

    fn materialize(self) -> NativeRegion {
        let mut region = NativeRegion {
            words: vec![RegionWord::ZEROED; self.capacity.div_ceil(REGION_ALIGNMENT)],
            capacity: self.capacity,
            contract: self.contract,
        };
        region.bytes_mut()[..self.input.len()].copy_from_slice(&self.input);
        region
    }

    fn encoded_content_bound(&self) -> Option<usize> {
        let bytes_per_input = match self.contract {
            RegionContract::NulText => 6,
            RegionContract::NulBytes | RegionContract::RawBytes => 4,
        };
        self.capacity
            .checked_mul(bytes_per_input)
            .and_then(|bytes| bytes.checked_add(REGION_ANSWER_ENTRY_BYTES))
    }
}

/// Refuse before any pointee allocation when either raw storage or the worst
/// case serialized answer could exceed the slot's byte ceiling. JSON text may
/// escape each byte as `\\u00XX` (six bytes); numeric byte arrays need at most
/// three digits plus a comma (four). The fixed allowances cover the stable
/// top-level and per-region field names. This is intentionally conservative;
/// the host still checks the actual serialized result as defense in depth.
fn preflight_region_plans(plans: &[RegionPlan], maximum: usize) -> Result<(), NativeDoorError> {
    let capacity = plans
        .iter()
        .try_fold(0_usize, |total, plan| total.checked_add(plan.capacity))
        .unwrap_or(usize::MAX);
    if capacity > maximum {
        return Err(NativeDoorError::NativeRegionTooLarge {
            requested: capacity,
            maximum,
        });
    }
    let encoded = plans
        .iter()
        .try_fold(REGION_ANSWER_BASE_BYTES, |total, plan| {
            total.checked_add(plan.encoded_content_bound()?)
        })
        .unwrap_or(usize::MAX);
    if encoded > maximum {
        return Err(NativeDoorError::NativeRegionTooLarge {
            requested: encoded,
            maximum,
        });
    }
    Ok(())
}

/// One call-scoped host region: storage the host owns for exactly one
/// synchronous call, plus the contract the guest stated for reading it back.
///
/// The region is not a guest span and not a host address the guest can name:
/// it is created by this call, passed as the call's pointer argument, and
/// dropped when the call returns. Nothing about it survives into a second
/// call, and no field of the answer exposes its address.
struct NativeRegion {
    words: Vec<RegionWord>,
    capacity: usize,
    contract: RegionContract,
}

impl NativeRegion {
    /// The region's first `capacity` bytes, zero-filled except for the input.
    fn bytes(&self) -> &[u8] {
        // SAFETY: `capacity.div_ceil(REGION_ALIGNMENT)` units were allocated, so
        // `words.len() * REGION_ALIGNMENT >= capacity`; every byte of every unit
        // was initialised by `RegionWord::ZEROED`. The cast reinterprets `[u8; N]`
        // storage as `u8` storage, which changes neither alignment nor validity,
        // and the slice starts at the wrapper's own first unit.
        unsafe { std::slice::from_raw_parts(self.words[0].bytes().as_ptr(), self.capacity) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: forwarded from `bytes`; this region is exclusively borrowed.
        unsafe {
            std::slice::from_raw_parts_mut(self.words.as_mut_ptr().cast::<u8>(), self.capacity)
        }
    }

    /// The raw address handed to the foreign call.
    ///
    /// This is the only place a region becomes a pointer, and the pointer is
    /// call-scoped by construction: it borrows storage this call allocated,
    /// and the storage outlives the synchronous `invoke_abi` that consumes it.
    fn pointer(&mut self) -> *mut std::ffi::c_void {
        self.words.as_mut_ptr().cast()
    }

    /// The region's declared output contract, as one `regions` entry.
    ///
    /// `output: "bytes"` answers a post-call snapshot of the selected bytes,
    /// never a guessed written length and never proof that the callee wrote
    /// them. Bytes left untouched by the callee remain the host's zero fill or
    /// the caller's input. The native status is retained on readback failures
    /// because rejecting an encoding contract must not erase the C result.
    fn readback(
        &self,
        index: usize,
        native_status: i32,
    ) -> Result<serde_json::Value, NativeDoorError> {
        let bytes = self.bytes();
        let content = match self.contract {
            RegionContract::NulText | RegionContract::NulBytes => {
                let end = bytes.iter().position(|byte| *byte == 0).ok_or(
                    NativeDoorError::NativeRegionUnterminated {
                        index,
                        native_status,
                    },
                )?;
                &bytes[..end]
            }
            RegionContract::RawBytes => bytes,
        };
        let value = match self.contract {
            RegionContract::NulText => serde_json::Value::String(
                std::str::from_utf8(content)
                    .map_err(|_| NativeDoorError::NativeRegionNotUtf8 {
                        index,
                        native_status,
                    })?
                    .to_owned(),
            ),
            RegionContract::NulBytes | RegionContract::RawBytes => content
                .iter()
                .map(|byte| serde_json::Value::from(*byte))
                .collect(),
        };
        Ok(serde_json::json!({
            "index": index,
            "output": self.contract.output(),
            "termination": self.contract.termination(),
            "value": value,
        }))
    }
}

/// Whether the JSON adapter serves this pointer prototype.
///
/// Every ABI shape this catalog exposes, as the caller's declaration.
///
/// Derived from the same tables dispatch uses, so it is not a second catalog: it
/// exists for the owning test that keeps `exposure ⊆ mechanism` true by asking
/// dyn rather than by trusting this list.
#[cfg(test)]
fn exposure_declarations() -> Vec<(NativeType, Vec<NativeType>)> {
    let mut exposures = Vec::new();
    for ty in EXACT_SCALAR_TYPES {
        for arity in 0..=MAX_NATIVE_ARITY {
            exposures.push((ty, vec![ty; arity]));
        }
    }
    for (result, parameters) in HETEROGENEOUS_SCALAR_SIGNATURES
        .iter()
        .chain(POINTER_SIGNATURES)
        .copied()
    {
        exposures.push((result, parameters.to_vec()));
    }
    exposures
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnixIoctlPrototype {
    I32Request,
    U64Request,
}

fn native_dispatch(spec: &NativeSpec) -> Result<NativeDispatch, NativeDoorError> {
    if spec.library.is_empty() && spec.symbol == "ioctl" {
        return match (spec.result, spec.parameters.as_slice()) {
            (NativeType::I32, [NativeType::I32, NativeType::I32, NativeType::Pointer]) => {
                Ok(NativeDispatch::UnixIoctl(UnixIoctlPrototype::I32Request))
            }
            (NativeType::I32, [NativeType::I32, NativeType::U64, NativeType::Pointer]) => {
                Ok(NativeDispatch::UnixIoctl(UnixIoctlPrototype::U64Request))
            }
            _ => Err(NativeDoorError::InvocationSignatureUnsupported {
                result: spec.result,
                parameters: spec.parameters.clone(),
            }),
        };
    }
    if let Some(result) = exact_scalar_type(spec.result)
        && spec.parameters.len() <= MAX_NATIVE_ARITY
        && spec
            .parameters
            .iter()
            .all(|parameter| exact_scalar_type(*parameter) == Some(result))
    {
        return Ok(NativeDispatch::Scalar);
    }
    let fixed =
        HETEROGENEOUS_SCALAR_SIGNATURES.contains(&(spec.result, spec.parameters.as_slice()));
    if fixed {
        return Ok(NativeDispatch::Scalar);
    }
    if POINTER_SIGNATURES.contains(&(spec.result, spec.parameters.as_slice())) {
        return Ok(NativeDispatch::Pointer);
    }
    Err(NativeDoorError::InvocationSignatureUnsupported {
        result: spec.result,
        parameters: spec.parameters.clone(),
    })
}

fn ioctl_i32_argument(
    index: usize,
    argument: &NativeArgument,
    spec: &NativeSpec,
) -> Result<i32, NativeDoorError> {
    let NativeArgument::Scalar {
        ty: NativeType::I32,
        bits,
    } = argument
    else {
        return Err(unsupported_json_spec(spec));
    };
    let value = *bits as i32;
    (value as i64 as u64 == *bits)
        .then_some(value)
        .ok_or(NativeDoorError::ScalarNotCanonical {
            index,
            ty: NativeType::I32,
            bits: *bits,
        })
}

fn ioctl_u64_argument(
    _index: usize,
    argument: &NativeArgument,
    spec: &NativeSpec,
) -> Result<u64, NativeDoorError> {
    match argument {
        NativeArgument::Scalar {
            ty: NativeType::U64,
            bits,
        } => Ok(*bits),
        _ => Err(unsupported_json_spec(spec)),
    }
}

fn ioctl_pointer_argument(
    memory_base: *mut u8,
    _index: usize,
    argument: &NativeArgument,
    spec: &NativeSpec,
) -> Result<*mut std::ffi::c_void, NativeDoorError> {
    match argument {
        NativeArgument::GuestSpan {
            ty: NativeType::Pointer,
            span,
        } => {
            // SAFETY: decode_native_call proved this offset is within the live
            // guest allocation; no aliased Rust reference is constructed.
            Ok(unsafe { memory_base.add(span.offset) }.cast())
        }
        _ => Err(unsupported_json_spec(spec)),
    }
}

fn map_unix_ioctl_error(error: UnixIoctlError) -> NativeDoorError {
    match error {
        UnixIoctlError::Unsupported => NativeDoorError::InvocationTargetUnsupported {
            operation: "Unix ioctl",
        },
    }
}

fn exact_json_argument(
    index: usize,
    ty: NativeType,
    value: &serde_json::Value,
) -> Result<AbiValue, NativeDoorError> {
    let invalid = || NativeDoorError::ArgumentValueInvalid { index, ty };
    match ty {
        NativeType::I32 => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(AbiValue::I32)
            .ok_or_else(invalid),
        NativeType::U32 => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(AbiValue::U32)
            .ok_or_else(invalid),
        NativeType::I64 => value
            .as_str()
            .and_then(|value| value.parse().ok())
            .map(AbiValue::I64)
            .ok_or_else(invalid),
        NativeType::U64 => value
            .as_str()
            .and_then(|value| value.parse().ok())
            .map(AbiValue::U64)
            .ok_or_else(invalid),
        NativeType::Isize => value
            .as_str()
            .and_then(|value| value.parse().ok())
            .map(AbiValue::Isize)
            .ok_or_else(invalid),
        NativeType::Usize => value
            .as_str()
            .and_then(|value| value.parse().ok())
            .map(AbiValue::Usize)
            .ok_or_else(invalid),
        NativeType::F64 => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(AbiValue::F64)
            .ok_or_else(invalid),
        _ => Err(NativeDoorError::InvocationSignatureUnsupported {
            result: ty,
            parameters: vec![ty],
        }),
    }
}

fn exact_scalar_type(ty: NativeType) -> Option<NativeType> {
    EXACT_SCALAR_TYPES.contains(&ty).then_some(ty)
}

fn exact_argument(
    index: usize,
    argument: &NativeArgument,
    spec: &NativeSpec,
) -> Result<AbiValue, NativeDoorError> {
    let NativeArgument::Scalar { ty, bits } = argument else {
        return Err(unsupported_json_spec(spec));
    };
    let invalid = || NativeDoorError::ScalarNotCanonical {
        index,
        ty: *ty,
        bits: *bits,
    };
    match ty {
        NativeType::I32 => {
            let value = *bits as i32;
            (value as i64 as u64 == *bits)
                .then_some(AbiValue::I32(value))
                .ok_or_else(invalid)
        }
        NativeType::U32 => u32::try_from(*bits)
            .map(AbiValue::U32)
            .map_err(|_| invalid()),
        NativeType::I64 => Ok(AbiValue::I64(*bits as i64)),
        NativeType::U64 => Ok(AbiValue::U64(*bits)),
        NativeType::Isize => {
            let value = *bits as isize;
            (value as i64 as u64 == *bits)
                .then_some(AbiValue::Isize(value))
                .ok_or_else(invalid)
        }
        NativeType::Usize => usize::try_from(*bits)
            .map(AbiValue::Usize)
            .map_err(|_| invalid()),
        NativeType::F64 => Ok(AbiValue::F64(f64::from_bits(*bits))),
        _ => Err(unsupported_json_spec(spec)),
    }
}

fn fixed_pointer_argument(
    memory_base: *mut u8,
    index: usize,
    argument: &NativeArgument,
    spec: &NativeSpec,
) -> Result<AbiValue, NativeDoorError> {
    match argument {
        NativeArgument::Scalar {
            ty: NativeType::I32 | NativeType::U32 | NativeType::U64 | NativeType::Usize,
            ..
        } => exact_argument(index, argument, spec),
        NativeArgument::GuestSpan { ty, span } if ty.is_pointer() => {
            // SAFETY: decode_native_call proved offset + len is within the one
            // guest allocation. `add` therefore yields an in-bounds or one-past
            // raw address without constructing an aliased Rust reference.
            let pointer = unsafe { memory_base.add(span.offset) }.cast();
            Ok(AbiValue::Pointer(pointer))
        }
        NativeArgument::Null {
            ty: NativeType::NullablePointer,
        } => Ok(AbiValue::Pointer(std::ptr::null_mut())),
        NativeArgument::Null { .. }
        | NativeArgument::GuestSpan { .. }
        | NativeArgument::Scalar { .. } => Err(unsupported_json_spec(spec)),
    }
}

/// The exposure catalog must stay inside dyn's mechanism matrix.
///
/// qjswasm owns the exposure allowlist; dyn owns the trampoline matrix. This gate
/// keeps the one-way inclusion true by *asking dyn*, so a shape the catalog
/// exposes but no trampoline can execute reddens here instead of reaching a guest
/// as a "supported" declaration. It never asks dyn for a symbol, a nullability
/// verdict or a guest span: those stay this crate's policy.
#[cfg(test)]
mod mechanism_compatibility {
    use super::*;

    fn abi_signature(
        result: NativeType,
        parameters: &[NativeType],
    ) -> (agenterm_dyn::AbiType, Vec<agenterm_dyn::AbiType>) {
        (
            abi_type(result).expect("an exposed result maps to an ABI position"),
            parameters
                .iter()
                .map(|ty| abi_type(*ty).expect("an exposed parameter maps to an ABI position"))
                .collect(),
        )
    }

    #[test]
    fn every_exposed_shape_has_a_dyn_mechanism_trampoline() {
        let exposures = exposure_declarations();
        assert_eq!(
            exposures.len(),
            69,
            "exposure catalog inventory: 49 exact + 6 fixed + 14 pointer"
        );
        for (result, parameters) in &exposures {
            let (abi_result, abi_parameters) = abi_signature(*result, parameters);
            let signature = agenterm_dyn::AbiSignature {
                result: abi_result,
                params: &abi_parameters,
            };
            assert!(
                agenterm_dyn::validate_abi_signature(signature).is_ok(),
                "qjswasm exposes {result:?}({parameters:?}) but dyn has no trampoline for it"
            );
        }
    }

    /// Every result position `agenterm_dyn::AbiType` can express.
    ///
    /// This is the mechanism's own public type list, not a mechanism table.
    /// `Void` is a result here and is excluded from the argument positions below
    /// because dyn's own type documentation states it is valid only as a result
    /// position; that exclusion is the mechanism's statement, not this test's.
    const ABI_RESULT_POSITIONS: [agenterm_dyn::AbiType; 9] = [
        agenterm_dyn::AbiType::Void,
        agenterm_dyn::AbiType::I32,
        agenterm_dyn::AbiType::U32,
        agenterm_dyn::AbiType::I64,
        agenterm_dyn::AbiType::U64,
        agenterm_dyn::AbiType::Isize,
        agenterm_dyn::AbiType::Usize,
        agenterm_dyn::AbiType::F64,
        agenterm_dyn::AbiType::Pointer,
    ];

    /// Every argument position `agenterm_dyn::AbiType` can express.
    const ABI_ARGUMENT_POSITIONS: [agenterm_dyn::AbiType; 8] = [
        agenterm_dyn::AbiType::I32,
        agenterm_dyn::AbiType::U32,
        agenterm_dyn::AbiType::I64,
        agenterm_dyn::AbiType::U64,
        agenterm_dyn::AbiType::Isize,
        agenterm_dyn::AbiType::Usize,
        agenterm_dyn::AbiType::F64,
        agenterm_dyn::AbiType::Pointer,
    ];

    /// The mechanism matrix as the mechanism itself answers it.
    ///
    /// A *query over a vocabulary*, never a second mechanism table: the
    /// positions come from `agenterm_dyn::AbiType`'s own public list, the arity
    /// bound is this door's parser bound (`MAX_NATIVE_ARITY`, the same 6 dyn's
    /// exact family uses), and the only authority on membership is
    /// `agenterm_dyn::validate_abi_signature`, which answers from the shape
    /// alone and needs no argument values. Every element of the returned vector
    /// was admitted by dyn's own classification during this call.
    ///
    /// Refusal *above* the matrix is owned elsewhere: `agenterm-dyn`'s
    /// `tests/abi.rs` asks the same query about the shapes outside it,
    /// including a homogeneous arity beyond the bound.
    fn mechanism_shapes_by_query() -> Vec<(agenterm_dyn::AbiType, Vec<agenterm_dyn::AbiType>)> {
        let mut shapes = Vec::new();
        let mut params: Vec<agenterm_dyn::AbiType> = Vec::new();
        for result in ABI_RESULT_POSITIONS {
            collect_mechanism_shapes(result, &mut params, &mut shapes);
        }
        shapes
    }

    /// One result position against every argument sequence up to the arity
    /// bound, keeping the shapes dyn answers with a real trampoline.
    fn collect_mechanism_shapes(
        result: agenterm_dyn::AbiType,
        params: &mut Vec<agenterm_dyn::AbiType>,
        shapes: &mut Vec<(agenterm_dyn::AbiType, Vec<agenterm_dyn::AbiType>)>,
    ) {
        let signature = agenterm_dyn::AbiSignature {
            result,
            params: params.as_slice(),
        };
        if agenterm_dyn::validate_abi_signature(signature).is_ok() {
            shapes.push((result, params.clone()));
        }
        if params.len() == MAX_NATIVE_ARITY {
            return;
        }
        for parameter in ABI_ARGUMENT_POSITIONS {
            params.push(parameter);
            collect_mechanism_shapes(result, params, shapes);
            params.pop();
        }
    }

    /// One ABI position under the name this door's declarations use.
    fn position_name(position: agenterm_dyn::AbiType) -> &'static str {
        match position {
            agenterm_dyn::AbiType::Void => "void",
            agenterm_dyn::AbiType::I32 => "i32",
            agenterm_dyn::AbiType::U32 => "u32",
            agenterm_dyn::AbiType::I64 => "i64",
            agenterm_dyn::AbiType::U64 => "u64",
            agenterm_dyn::AbiType::Isize => "isize",
            agenterm_dyn::AbiType::Usize => "usize",
            agenterm_dyn::AbiType::F64 => "f64",
            agenterm_dyn::AbiType::Pointer => "ptr",
        }
    }

    /// One shape as `result(parameter,parameter)`.
    fn shape_name(shape: &(agenterm_dyn::AbiType, Vec<agenterm_dyn::AbiType>)) -> String {
        let parameters = shape
            .1
            .iter()
            .map(|position| position_name(*position))
            .collect::<Vec<_>>()
            .join(",");
        format!("{}({parameters})", position_name(shape.0))
    }

    /// The catalog's declarations as the distinct ABI shapes they name.
    ///
    /// The declarations are not one-to-one with shapes: `ptr` and `ptr?` are one
    /// ABI position, so a nullable variant collapses onto the shape its
    /// non-nullable twin already names.
    fn exposure_abi_shapes() -> Vec<(agenterm_dyn::AbiType, Vec<agenterm_dyn::AbiType>)> {
        let mut shapes: Vec<_> = exposure_declarations()
            .iter()
            .map(|(result, parameters)| abi_signature(*result, parameters))
            .collect();
        shapes.sort_by_key(shape_name);
        shapes.dedup();
        shapes
    }

    /// The complete mechanism-only account: what dyn can execute and this
    /// catalog does not expose today.
    ///
    /// This is the *difference*, not the mechanism matrix: 4 pointer-result
    /// shapes (this door has no guest span or region to rebase a returned
    /// address onto) plus 5 direct-scalar shapes (taken over from the retired
    /// Darwin probes, never exposed here). Restate it when either side
    /// legitimately moves; the assertion below names every entry that is missing
    /// or unexpected instead of only counting, so a silent omission fails by
    /// name.
    const MECHANISM_ONLY: [(agenterm_dyn::AbiType, &[agenterm_dyn::AbiType]); 9] = [
        (agenterm_dyn::AbiType::Pointer, &[]),
        (
            agenterm_dyn::AbiType::Pointer,
            &[agenterm_dyn::AbiType::U32],
        ),
        (
            agenterm_dyn::AbiType::Pointer,
            &[agenterm_dyn::AbiType::U64],
        ),
        (
            agenterm_dyn::AbiType::Pointer,
            &[agenterm_dyn::AbiType::Pointer],
        ),
        (agenterm_dyn::AbiType::Isize, &[agenterm_dyn::AbiType::U32]),
        (
            agenterm_dyn::AbiType::I32,
            &[
                agenterm_dyn::AbiType::I32,
                agenterm_dyn::AbiType::I32,
                agenterm_dyn::AbiType::Pointer,
            ],
        ),
        (
            agenterm_dyn::AbiType::I32,
            &[
                agenterm_dyn::AbiType::I32,
                agenterm_dyn::AbiType::I32,
                agenterm_dyn::AbiType::U64,
                agenterm_dyn::AbiType::Pointer,
                agenterm_dyn::AbiType::I32,
            ],
        ),
        (
            agenterm_dyn::AbiType::I32,
            &[
                agenterm_dyn::AbiType::Pointer,
                agenterm_dyn::AbiType::U32,
                agenterm_dyn::AbiType::Pointer,
                agenterm_dyn::AbiType::Pointer,
                agenterm_dyn::AbiType::Pointer,
                agenterm_dyn::AbiType::Usize,
            ],
        ),
        (
            agenterm_dyn::AbiType::Usize,
            &[
                agenterm_dyn::AbiType::I32,
                agenterm_dyn::AbiType::Pointer,
                agenterm_dyn::AbiType::Usize,
            ],
        ),
    ];

    /// The mechanism-only account is derived, and every entry is named.
    ///
    /// The old form of this court hand-listed three pointer-result shapes and
    /// never asked the mechanism which shapes it actually has, so `ptr(ptr)`
    /// (the `getenv` trampoline dyn proves against its own oracle) was missing
    /// from the account while the assertion stayed green. This form asks dyn for
    /// the whole matrix, subtracts the catalog's own declarations, and compares
    /// the difference against `MECHANISM_ONLY` both ways: a shape dyn lost, a
    /// shape this catalog silently stopped exposing, and a shape either side
    /// gained all fail with their own names in the message. The inclusion itself
    /// still runs one way — a shape may never enter the catalog before the
    /// mechanism can execute it.
    #[test]
    fn the_mechanism_only_account_is_derived_and_names_every_entry() {
        let mechanism = mechanism_shapes_by_query();
        let exposures = exposure_abi_shapes();
        for exposure in &exposures {
            assert!(
                mechanism.contains(exposure),
                "qjswasm exposes {} but dyn has no trampoline for it",
                shape_name(exposure)
            );
        }

        let mechanism_only: Vec<_> = mechanism
            .iter()
            .filter(|shape| !exposures.contains(shape))
            .cloned()
            .collect();
        let expected: Vec<_> = MECHANISM_ONLY
            .iter()
            .map(|(result, parameters)| (*result, parameters.to_vec()))
            .collect();
        let missing: Vec<_> = expected
            .iter()
            .filter(|shape| !mechanism_only.contains(shape))
            .map(shape_name)
            .collect();
        let unexpected: Vec<_> = mechanism_only
            .iter()
            .filter(|shape| !expected.contains(shape))
            .map(shape_name)
            .collect();
        assert!(
            missing.is_empty() && unexpected.is_empty(),
            "the mechanism-only account ({} shapes today) drifted: missing [{}], unexpected [{}]",
            mechanism_only.len(),
            missing.join(", "),
            unexpected.join(", ")
        );

        // The account above is only meaningful while the query still covers the
        // whole mechanism, so the two inventories are asserted last.
        assert_eq!(
            exposures.len(),
            66,
            "the 69 declarations name these distinct ABI shapes: {}",
            exposures
                .iter()
                .map(shape_name)
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert_eq!(
            mechanism.len(),
            75,
            "the query over the mechanism's own vocabulary must answer dyn's real matrix \
             (49 exact + 4 fixed + 8 fixed-pointer + 5 pointer-result + 9 direct-scalar); \
             a different number means this universe stopped covering the matrix, or dyn's \
             matrix moved and both this account and PRD 02.36 need restating"
        );
    }

    #[test]
    fn the_ioctl_shapes_stay_out_of_the_abi_exposure_inventory() {
        let exposures = exposure_declarations();
        for declaration in [
            (
                NativeType::I32,
                vec![NativeType::I32, NativeType::I32, NativeType::Pointer],
            ),
            (
                NativeType::I32,
                vec![NativeType::I32, NativeType::U64, NativeType::Pointer],
            ),
        ] {
            assert!(
                !exposures.contains(&declaration),
                "{declaration:?} belongs to the ioctl mechanism, not to the ABI inventory"
            );
        }
        // The u64 request is why `invoke_unix_ioctl` exists as its own entry: the
        // ABI matrix has no trampoline for that variadic shape.
        let (abi_result, abi_parameters) = abi_signature(
            NativeType::I32,
            &[NativeType::I32, NativeType::U64, NativeType::Pointer],
        );
        let signature = agenterm_dyn::AbiSignature {
            result: abi_result,
            params: &abi_parameters,
        };
        assert!(
            agenterm_dyn::validate_abi_signature(signature).is_err(),
            "the u64 ioctl request must stay outside the ABI matrix"
        );
    }
}

/// Maps one declared schema position onto the policy-free ABI position.
///
/// `ptr` and `ptr?` are the same ABI position: nullability is an upper-layer
/// schema distinction, and qjswasm keeps it in its own null check.
fn abi_type(ty: NativeType) -> Option<agenterm_dyn::AbiType> {
    match ty {
        NativeType::Void => Some(agenterm_dyn::AbiType::Void),
        NativeType::I32 => Some(agenterm_dyn::AbiType::I32),
        NativeType::U32 => Some(agenterm_dyn::AbiType::U32),
        NativeType::I64 => Some(agenterm_dyn::AbiType::I64),
        NativeType::U64 => Some(agenterm_dyn::AbiType::U64),
        NativeType::Isize => Some(agenterm_dyn::AbiType::Isize),
        NativeType::Usize => Some(agenterm_dyn::AbiType::Usize),
        NativeType::F64 => Some(agenterm_dyn::AbiType::F64),
        NativeType::Pointer | NativeType::NullablePointer => Some(agenterm_dyn::AbiType::Pointer),
        // Narrow integers are refused by the upper catalog before an
        // execution arm runs; if one ever arrives, the mechanism has no position
        // for it and the call is refused rather than approximated.
        NativeType::I8 | NativeType::U8 | NativeType::I16 | NativeType::U16 => None,
    }
}

/// The result bit pattern, accepted **only** in the position the spec declared.
///
/// There is no fallback arm: a result that does not match the declared type is a
/// typed refusal, never a silent zero.
fn abi_result_bits(
    value: agenterm_dyn::AbiValue,
    call: &DecodedNativeCall,
    memory_base: *mut u8,
    memory_len: usize,
) -> Result<u64, NativeDoorError> {
    let spec = &call.spec;
    let scalar = match (spec.result, value) {
        (NativeType::Void, agenterm_dyn::AbiValue::Void) => Some(0),
        (NativeType::I32, agenterm_dyn::AbiValue::I32(bits)) => Some(bits as i64 as u64),
        (NativeType::U32, agenterm_dyn::AbiValue::U32(bits)) => Some(u64::from(bits)),
        (NativeType::I64, agenterm_dyn::AbiValue::I64(bits)) => Some(bits as u64),
        (NativeType::U64, agenterm_dyn::AbiValue::U64(bits)) => Some(bits),
        (NativeType::Isize, agenterm_dyn::AbiValue::Isize(bits)) => Some(bits as i64 as u64),
        (NativeType::Usize, agenterm_dyn::AbiValue::Usize(bits)) => Some(bits as u64),
        (NativeType::F64, agenterm_dyn::AbiValue::F64(bits)) => Some(bits.to_bits()),
        (NativeType::Pointer, agenterm_dyn::AbiValue::Pointer(pointer)) => {
            if pointer.is_null() {
                return Ok(0);
            }
            let offset = (pointer as usize)
                .checked_sub(memory_base as usize)
                .filter(|offset| *offset < memory_len)
                .ok_or(NativeDoorError::ResultPointerOutsideGuestSpans)?;
            let declared = call.arguments.iter().any(|argument| {
                matches!(argument, NativeArgument::GuestSpan { span, .. }
                    if offset >= span.offset && offset < span.offset + span.len)
            });
            if !declared {
                return Err(NativeDoorError::ResultPointerOutsideGuestSpans);
            }
            return Ok(offset as u64);
        }
        _ => None,
    };
    scalar.ok_or_else(|| unsupported_json_spec(spec))
}

/// The JSON rendering of a result, replicating the retired per-family helpers:
/// 32-bit values are numbers, wider integers are decimal strings.
fn abi_json_result(
    value: agenterm_dyn::AbiValue,
    expected: NativeType,
) -> Option<serde_json::Value> {
    match (expected, value) {
        (NativeType::Void, agenterm_dyn::AbiValue::Void) => {
            Some(serde_json::json!({"type":"void"}))
        }
        (NativeType::I32, agenterm_dyn::AbiValue::I32(bits)) => {
            Some(serde_json::json!({"type":"i32","value":bits}))
        }
        (NativeType::U32, agenterm_dyn::AbiValue::U32(bits)) => {
            Some(serde_json::json!({"type":"u32","value":bits}))
        }
        (NativeType::I64, agenterm_dyn::AbiValue::I64(bits)) => {
            Some(serde_json::json!({"type":"i64","value":bits.to_string()}))
        }
        (NativeType::U64, agenterm_dyn::AbiValue::U64(bits)) => {
            Some(serde_json::json!({"type":"u64","value":bits.to_string()}))
        }
        (NativeType::Isize, agenterm_dyn::AbiValue::Isize(bits)) => {
            Some(serde_json::json!({"type":"isize","value":bits.to_string()}))
        }
        (NativeType::Usize, agenterm_dyn::AbiValue::Usize(bits)) => {
            Some(serde_json::json!({"type":"usize","value":bits.to_string()}))
        }
        (NativeType::F64, agenterm_dyn::AbiValue::F64(bits)) => {
            Some(serde_json::json!({"type":"f64","value":bits}))
        }
        _ => None,
    }
}

/// One ABI failure mapped onto the door's typed vocabulary.
///
/// The two call sites that need this mapping disagree about nothing but where
/// they keep the declaration: the raw path holds a decoded call, the JSON paths
/// hold a spec. The mapping is therefore stated once over the spec — dyn keeps a
/// third account of the same failure (`SymbolLookup::library`), while the door's
/// `SymbolLoad` has no library field, so neither mapper reads it — and the
/// decoded-call caller passes `&call.spec`, exactly as `unsupported_signature`
/// already did. Byte for byte the answers are the ones the two former mappers
/// gave.
fn map_abi_error(spec: &NativeSpec, error: agenterm_dyn::AbiError) -> NativeDoorError {
    match error {
        agenterm_dyn::AbiError::SignatureUnsupported { .. }
        | agenterm_dyn::AbiError::ArgumentCount { .. }
        | agenterm_dyn::AbiError::ArgumentShape { .. } => unsupported_json_spec(spec),
        agenterm_dyn::AbiError::LibraryLoad { library, message } => {
            NativeDoorError::LibraryLoad { library, message }
        }
        agenterm_dyn::AbiError::SymbolLookup {
            symbol, message, ..
        } => NativeDoorError::SymbolLoad { symbol, message },
    }
}

fn abi_parameters_for_spec(
    spec: &NativeSpec,
) -> Result<Vec<agenterm_dyn::AbiType>, NativeDoorError> {
    spec.parameters
        .iter()
        .map(|ty| abi_type(*ty).ok_or_else(|| unsupported_json_spec(spec)))
        .collect()
}

#[cfg(test)]
mod json_adapter_tests {
    use super::*;

    /// One declared library is one adopted handle: the door's first call adopts it,
    /// the next calls run through the adopted handle, and a second engine's table
    /// knows neither.
    ///
    /// The hit count lives on the table, not in a process-global, so this test
    /// reads only its own engine and needs no serialisation. It is the evidence
    /// the library table exists for: a door that resolved a handle and then ran the
    /// one-shot entry anyway would leave this count at zero.
    #[cfg(unix)]
    #[test]
    fn a_repeated_declared_library_is_adopted_once_and_reused_by_the_later_calls() {
        let engine = NativeLibraryCache::new();
        assert_eq!(engine.len(), 0, "a fresh engine holds no library");
        assert_eq!(engine.cached_hits(), 0, "and has served no adopted handle");

        let mut previous = engine.cached_hits();
        for call in 0..3 {
            let answer = invoke_native_json(b"|getpid|i32()", b"[]", &engine, region_bound())
                .expect("getpid runs through the native door");
            let answer: serde_json::Value = serde_json::from_str(&answer).expect("result JSON");
            assert_eq!(
                answer["type"], "i32",
                "call {call} kept the result position"
            );
            assert_eq!(
                answer["value"].as_i64().map(|value| value as u64),
                Some(u64::from(std::process::id())),
                "call {call} answered this process"
            );
            assert_eq!(
                engine.cached_hits(),
                previous + 1,
                "call {call} must run through the adopted-handle branch"
            );
            previous = engine.cached_hits();
            assert_eq!(
                engine.len(),
                1,
                "call {call} names one declared string, which is one adopted library"
            );
        }
        assert_eq!(
            engine.cached_hits(),
            3,
            "three calls on one declared library take the adopted handle three times"
        );

        let other = NativeLibraryCache::new();
        assert_eq!(other.len(), 0, "a second engine adopts nothing of its own");
        assert_eq!(
            other.cached_hits(),
            0,
            "and the second engine's count is its own"
        );
    }

    #[test]
    fn wide_integer_arguments_use_exact_decimal_strings() {
        let maximum = serde_json::Value::String(u64::MAX.to_string());
        assert_eq!(
            exact_json_argument(0, NativeType::U64, &maximum),
            Ok(AbiValue::U64(u64::MAX))
        );
        assert_eq!(
            exact_json_argument(0, NativeType::U64, &serde_json::json!(42)),
            Err(NativeDoorError::ArgumentValueInvalid {
                index: 0,
                ty: NativeType::U64,
            })
        );
    }

    #[test]
    fn wide_integer_results_remain_exact_decimal_strings() {
        assert_eq!(
            abi_json_result(agenterm_dyn::AbiValue::I64(i64::MIN), NativeType::I64),
            Some(serde_json::json!({"type":"i64","value":i64::MIN.to_string()}))
        );
        // The fixed family's wide integer keeps the same exact-decimal rule.
        assert_eq!(
            abi_json_result(agenterm_dyn::AbiValue::Isize(isize::MIN), NativeType::Isize),
            Some(serde_json::json!({"type":"isize","value":isize::MIN.to_string()}))
        );
    }

    #[test]
    fn dispatch_unifies_admitted_scalars_and_distinguishes_other_families() {
        let parse = |text: &str| parse_native_spec(text.as_bytes()).expect("spec parses");
        assert_eq!(
            native_dispatch(&parse("|abs|i32(i32)")),
            Ok(NativeDispatch::Scalar)
        );
        assert_eq!(
            native_dispatch(&parse("|sysconf|isize(i32)")),
            Ok(NativeDispatch::Scalar)
        );
        assert_eq!(
            native_dispatch(&parse("|lseek|i64(i32,i64,i32)")),
            Ok(NativeDispatch::Scalar)
        );
        for spec in [
            "|uname|i32(ptr)",
            "|getrlimit|i32(i32,ptr)",
            "|access|i32(ptr,i32)",
            "|gethostuuid|i32(ptr,ptr)",
            "|getentropy|i32(ptr,u64)",
            "|sysctlnametomib|i32(ptr,ptr,ptr)",
            "|gettimeofday|i32(ptr,ptr?)",
            "|free|void(ptr?)",
            "|time|i64(ptr?)",
            "|times|i64(ptr)",
            "|getcwd|ptr(ptr,usize)",
            "|pthread_threadid_np|i32(ptr?,ptr)",
            "|proc_pidpath|i32(i32,ptr,u32)",
            "|pthread_getname_np|i32(u64,ptr,u64)",
        ] {
            assert_eq!(native_dispatch(&parse(spec)), Ok(NativeDispatch::Pointer));
        }
        assert_eq!(
            native_dispatch(&parse("|ioctl|i32(i32,i32,ptr)")),
            Ok(NativeDispatch::UnixIoctl(UnixIoctlPrototype::I32Request))
        );
        assert_eq!(
            native_dispatch(&parse("|ioctl|i32(i32,u64,ptr)")),
            Ok(NativeDispatch::UnixIoctl(UnixIoctlPrototype::U64Request))
        );
        assert!(matches!(
            native_dispatch(&parse("libSystem.B.dylib|ioctl|i32(i32,u64,ptr)")),
            Err(NativeDoorError::InvocationSignatureUnsupported { .. })
        ));
        assert!(matches!(
            native_dispatch(&parse("missing|unused|isize(i64)")),
            Err(NativeDoorError::InvocationSignatureUnsupported { .. })
        ));
    }

    #[test]
    fn pointer_results_must_land_in_a_declared_guest_span() {
        let spec = parse_native_spec(b"|getcwd|ptr(ptr,usize)").expect("spec parses");
        let call = DecodedNativeCall {
            spec,
            return_slot: GuestSpan { offset: 8, len: 8 },
            initial_return_bits: 0,
            arguments: vec![
                NativeArgument::GuestSpan {
                    ty: NativeType::Pointer,
                    span: GuestSpan {
                        offset: 16,
                        len: 16,
                    },
                },
                NativeArgument::Scalar {
                    ty: NativeType::Usize,
                    bits: 16,
                },
            ],
        };
        let mut memory = [0_u8; 64];
        let base = memory.as_mut_ptr();
        assert_eq!(
            abi_result_bits(
                agenterm_dyn::AbiValue::Pointer(unsafe { base.add(16) }.cast()),
                &call,
                base,
                memory.len(),
            ),
            Ok(16)
        );
        assert_eq!(
            abi_result_bits(
                agenterm_dyn::AbiValue::Pointer(unsafe { base.add(40) }.cast()),
                &call,
                base,
                memory.len(),
            ),
            Err(NativeDoorError::ResultPointerOutsideGuestSpans)
        );
    }

    #[test]
    fn json_adapter_refuses_pointer_prototypes_without_inventing_host_addresses() {
        // A pointer position with no region record is a refusal, not a null
        // pointer and not an invented address.
        for argument in [&b"[0]"[..], &b"[null]"[..]] {
            assert_eq!(
                invoke_native_json(
                    b"|uname|i32(ptr)",
                    argument,
                    &NativeLibraryCache::new(),
                    region_bound(),
                ),
                Err(NativeDoorError::NativeRegionRequired { index: 0 })
            );
        }
    }

    /// The bound the host passes in: the slot's byte budget, as `host.rs` does.
    fn region_bound() -> usize {
        crate::Budget::default().max_bridge_result_bytes
    }

    /// Both `uname(2)` and the JSON region model are exercised here against an
    /// oracle that shares neither: the POSIX `uname` program.
    #[cfg(unix)]
    #[test]
    fn a_json_region_carries_uname_through_the_door_and_matches_the_program() {
        let output = std::process::Command::new("uname")
            .arg("-s")
            .output()
            .expect("the POSIX uname oracle runs");
        assert!(output.status.success(), "uname -s must succeed");
        let expected =
            std::str::from_utf8(output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout))
                .expect("uname -s emits UTF-8 text")
                .to_owned();
        // `uname` takes no length argument, so the region must be at least
        // `sizeof(struct utsname)` -- 1280 bytes on Darwin, 390 on Linux. 4096
        // covers both repository Unix hosts; an under-sized region here would
        // be a real C overflow, which is the caller's contract, not the door's.
        let answer = invoke_native_json(
            b"|uname|i32(ptr)",
            br#"[{"region":{"capacity":4096,"termination":"nul","output":"text"}}]"#,
            &NativeLibraryCache::new(),
            region_bound(),
        )
        .expect("uname runs through one call-scoped region");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&answer).expect("result JSON"),
            serde_json::json!({
                "type": "i32",
                "value": 0,
                "regions": [{
                    "index": 0,
                    "output": "text",
                    "termination": "nul",
                    "value": expected,
                }],
            })
        );
    }

    /// Byte output is the exact bytes, in both termination modes.
    #[cfg(unix)]
    #[test]
    fn a_json_region_reads_back_exactly_what_the_callee_left() {
        let libraries = NativeLibraryCache::new();
        let name = {
            let output = std::process::Command::new("uname")
                .arg("-s")
                .output()
                .expect("the POSIX uname oracle runs");
            output
                .stdout
                .strip_suffix(b"\n")
                .unwrap_or(&output.stdout)
                .to_vec()
        };
        let read = |termination: &str, output: &str, capacity: usize| {
            let arguments = format!(
                r#"[{{"region":{{"capacity":{capacity},"termination":"{termination}","output":"{output}"}}}}]"#
            );
            let answer = invoke_native_json(
                b"|uname|i32(ptr)",
                arguments.as_bytes(),
                &libraries,
                region_bound(),
            )
            .expect("uname runs through one call-scoped region");
            let answer: serde_json::Value = serde_json::from_str(&answer).expect("result JSON");
            answer["regions"][0]["value"].clone()
        };

        // `nul` + `bytes`: the bytes before the terminator, so the array stops
        // exactly where the text form stops -- the capacity is not the answer.
        let bytes = read("nul", "bytes", 4096);
        assert_eq!(
            bytes,
            serde_json::json!(name),
            "the byte form must answer the same content the text form does"
        );
        // `raw` + `bytes`: the whole capacity, with no invented written length
        // and the host's zero fill still visible past what `uname` wrote.
        let raw = read("raw", "bytes", 4096);
        let raw = raw.as_array().expect("the raw form answers an array");
        assert_eq!(raw.len(), 4096, "raw answers the whole capacity");
        assert_eq!(
            raw[..name.len()]
                .iter()
                .map(|byte| byte.as_u64())
                .collect::<Vec<_>>(),
            name.iter()
                .map(|byte| Some(u64::from(*byte)))
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            raw[name.len()].as_u64(),
            Some(0),
            "the callee terminated it"
        );
        assert_eq!(
            raw[4095].as_u64(),
            Some(0),
            "past the C struct the region is still the host's zero fill"
        );
    }

    /// The region's input bytes are really the callee's pointee, and the
    /// readback happens even when the callee reports failure.
    #[cfg(unix)]
    #[test]
    fn a_json_region_input_reaches_the_callee_and_is_read_back_after_a_failure() {
        let answer = invoke_native_json(
            b"|access|i32(ptr,i32)",
            br#"[{"region":{"capacity":64,"bytes":[47,110,111,110,101,120,105,115,116,101,110,116],"termination":"nul","output":"bytes"}},0]"#,
            &NativeLibraryCache::new(),
            region_bound(),
        )
        .expect("access runs through one call-scoped region");
        let answer: serde_json::Value = serde_json::from_str(&answer).expect("result JSON");
        assert_eq!(
            answer["value"], -1,
            "a missing path is access's own failure"
        );
        assert_eq!(
            answer["regions"][0]["value"],
            serde_json::json!(b"/nonexistent".to_vec()),
            "readback is the post-call buffer snapshot: a non-writing callee leaves the caller's input unchanged"
        );
    }

    /// `output: "text"` is a promise about the region, and a callee that leaves
    /// non-UTF-8 behind refuses the call rather than lossily decoding it.
    #[cfg(unix)]
    #[test]
    fn a_json_text_region_that_is_not_utf8_is_refused_by_name() {
        // `access` writes nothing, so the region keeps the byte the guest put
        // there -- which is not text, and is exactly what the contract forbids.
        assert_eq!(
            invoke_native_json(
                b"|access|i32(ptr,i32)",
                br#"[{"region":{"capacity":64,"bytes":[255],"termination":"nul","output":"text"}},0]"#,
                &NativeLibraryCache::new(),
                region_bound(),
            ),
            Err(NativeDoorError::NativeRegionNotUtf8 {
                index: 0,
                native_status: -1,
            })
        );
    }

    /// A `nul` region with no terminator cannot be answered as anything.
    ///
    /// The live courts cannot own this one deterministically: the host zero
    /// fills every region and requires room for the terminator, so only a
    /// callee that writes the *whole* capacity can remove it -- and the one
    /// catalogued symbol that fills a bounded buffer (`getentropy`) writes
    /// random bytes, which happen to contain a zero about once in 256 bytes.
    /// The state is therefore built here exactly as such a callee would leave
    /// it, and the readback rule is what is under test.
    #[test]
    fn a_nul_region_without_a_terminator_is_refused_by_name() {
        let region = NativeRegion {
            words: vec![RegionWord([0x41; REGION_ALIGNMENT]); 4],
            capacity: 64,
            contract: RegionContract::NulBytes,
        };
        assert_eq!(
            region.readback(3, -17),
            Err(NativeDoorError::NativeRegionUnterminated {
                index: 3,
                native_status: -17,
            })
        );
    }

    /// The region bill is per call, charged before anything is allocated.
    #[cfg(unix)]
    #[test]
    fn a_region_bill_over_the_slot_bound_is_refused_before_allocation() {
        let libraries = NativeLibraryCache::new();
        assert_eq!(
            invoke_native_json(
                b"|uname|i32(ptr)",
                br#"[{"region":{"capacity":65,"termination":"nul","output":"text"}}]"#,
                &libraries,
                64,
            ),
            Err(NativeDoorError::NativeRegionTooLarge {
                requested: 65,
                maximum: 64,
            })
        );
        // Raw capacity alone fits, but the successful numeric JSON array can
        // require four encoded bytes per native byte. The adapter must reject
        // that upper bound before loading `access` or allocating its pointee.
        assert_eq!(
            invoke_native_json(
                b"|access|i32(ptr,i32)",
                br#"[{"region":{"capacity":64,"bytes":[47],"termination":"raw","output":"bytes"}},0]"#,
                &libraries,
                64,
            ),
            Err(NativeDoorError::NativeRegionTooLarge {
                requested: 416,
                maximum: 64,
            })
        );
        assert_eq!(
            libraries.len(),
            0,
            "encoded-answer refusal must precede both allocation and loading"
        );
        // Two regions that each fit are still refused together when their sum
        // crosses the bound: the bill is the call's, not the position's.
        assert_eq!(
            invoke_native_json(
                b"|gettimeofday|i32(ptr,ptr?)",
                br#"[{"region":{"capacity":40,"termination":"nul","output":"bytes"}},{"region":{"capacity":40,"termination":"nul","output":"bytes"}}]"#,
                &libraries,
                64,
            ),
            Err(NativeDoorError::NativeRegionTooLarge {
                requested: 80,
                maximum: 64,
            })
        );
    }

    /// Every malformed record has its own named refusal.
    ///
    /// `access` is the callee on purpose: it writes nothing, so a case that
    /// were accidentally well-formed still cannot overflow the region it was
    /// given -- a mistake in this table must redden the assertion, not run a
    /// C function with a wrong pointee.
    #[test]
    fn the_region_record_refuses_every_malformed_shape_by_name() {
        let libraries = NativeLibraryCache::new();
        let cases = [
            r#"{"capacity":0,"termination":"nul","output":"text"}"#,
            r#"{"capacity":1.5,"termination":"nul","output":"text"}"#,
            r#"{"capacity":4,"bytes":[1,2,3,4,5],"termination":"raw","output":"bytes"}"#,
            r#"{"capacity":8,"bytes":[256],"termination":"raw","output":"bytes"}"#,
            r#"{"capacity":8,"bytes":["A"],"termination":"raw","output":"bytes"}"#,
            r#"{"capacity":8,"bytes":-1,"termination":"raw","output":"bytes"}"#,
            r#"{"capacity":8,"termination":"raw"}"#,
            r#"{"capacity":8,"termination":"truncate","output":"bytes"}"#,
            r#"{"capacity":8,"termination":"nul","output":"chunk"}"#,
            r#"{"capacity":1,"bytes":[65],"termination":"nul","output":"bytes"}"#,
            r#"{"capacity":8,"bytes":[65,0],"termination":"nul","output":"bytes"}"#,
            r#"{"capacity":8,"termination":"raw","output":"text"}"#,
            r#"{"capacity":8,"termination":"nul","output":"text","extra":1}"#,
        ];
        for case in cases {
            let arguments = format!(r#"[{{"region":{case}}},0]"#);
            let error = invoke_native_json(
                b"|access|i32(ptr,i32)",
                arguments.as_bytes(),
                &libraries,
                region_bound(),
            )
            .expect_err("a malformed region record is refused");
            assert!(
                matches!(
                    error,
                    NativeDoorError::NativeRegionShapeInvalid { index: 0, .. }
                ),
                "{case} answered {error:?}"
            );
            assert!(
                error.code() == "native_region_shape_invalid",
                "{case} answered the code {}",
                error.code()
            );
        }
        // Everything that is not a record carrying exactly one `region` key is
        // the *required* refusal, not the shape one: a scalar, null, an array,
        // an object with no `region`, and an object with only other keys.
        for (arguments, expected) in [
            (r#"[8,0]"#, "native_region_required"),
            (r#"[null,0]"#, "native_region_required"),
            (r#"[[],0]"#, "native_region_required"),
            (r#"[{},0]"#, "native_region_required"),
            (r#"[{"scope":8},0]"#, "native_region_required"),
            (
                r#"[{"region":{"capacity":8,"termination":"nul","output":"text"},"extra":1},0]"#,
                "native_region_shape_invalid",
            ),
        ] {
            let error = invoke_native_json(
                b"|access|i32(ptr,i32)",
                arguments.as_bytes(),
                &libraries,
                region_bound(),
            )
            .expect_err("a position without one region record is refused");
            assert_eq!(error.code(), expected, "{arguments} answered {error:?}");
        }
    }

    /// The argument refusal precedes the loader: a guest with both mistakes
    /// hears the argument one, and no library is opened for a refused call.
    #[test]
    fn a_refused_region_outranks_a_library_that_could_not_load() {
        let libraries = NativeLibraryCache::new();
        assert_eq!(
            invoke_native_json(
                b"no_such_library_agenterm_h6a|uname|i32(ptr)",
                b"[0]",
                &libraries,
                region_bound(),
            ),
            Err(NativeDoorError::NativeRegionRequired { index: 0 })
        );
        assert_eq!(libraries.len(), 0, "a refused call loads nothing");
    }

    /// The JSON pointer adapter admits the `i32` results and nothing else.
    #[test]
    fn only_the_i32_pointer_prototypes_are_admitted_by_the_json_adapter() {
        assert_eq!(
            POINTER_SIGNATURES
                .iter()
                .filter(|(result, _)| *result == NativeType::I32)
                .count(),
            10,
            "exactly ten pointer declarations have the JSON adapter's i32 answer shape"
        );
        assert_eq!(POINTER_SIGNATURES.len(), 14);
        let libraries = NativeLibraryCache::new();
        for spec in [
            b"|getcwd|ptr(ptr,usize)".as_slice(),
            b"|free|void(ptr?)",
            b"|time|i64(ptr?)",
            b"|ioctl|i32(i32,i32,ptr)",
        ] {
            let error = invoke_native_json(spec, b"[]", &libraries, region_bound())
                .expect_err("a shape outside the JSON pointer family stays refused");
            assert!(
                matches!(
                    error,
                    NativeDoorError::InvocationSignatureUnsupported { .. }
                ),
                "{:?} answered {error:?}",
                String::from_utf8_lossy(spec)
            );
        }
    }

    /// A call with no pointer position answers the same bytes it always did.
    ///
    /// `unix` because the comparison needs one real symbol the host process
    /// image exports; the guarantee itself is structural -- the exact and fixed
    /// arms this test exercises are the ones the region work never touched.
    #[cfg(unix)]
    #[test]
    fn a_scalar_json_call_without_a_region_answers_unchanged_bytes() {
        let libraries = NativeLibraryCache::new();
        for arguments in [&b"[-7]"[..], &b"[7]"[..]] {
            assert_eq!(
                invoke_native_json(b"|abs|i32(i32)", arguments, &libraries, region_bound()),
                Ok(r#"{"type":"i32","value":7}"#.to_owned())
            );
        }
    }

    /// Region storage is aligned for anything a C callee may assume.
    #[test]
    fn region_storage_carries_the_maximum_natural_alignment() {
        let plan = RegionPlan::from_json(
            0,
            &serde_json::json!({"region":{"capacity":1,"termination":"raw","output":"bytes"}}),
        )
        .expect("a one-byte region decodes");
        assert_eq!(
            plan.capacity, 1,
            "the storage bill is the capacity, not the padded allocation"
        );
        let mut region = plan.materialize();
        assert_eq!(
            region.pointer() as usize % REGION_ALIGNMENT,
            0,
            "every region starts at the alignment a C pointee may require"
        );
    }

    #[cfg(unix)]
    #[test]
    fn json_adapter_invokes_sysconf_through_the_policy_free_abi_core() {
        #[cfg(target_os = "macos")]
        let pagesize_key = 29;
        #[cfg(target_os = "linux")]
        let pagesize_key = 30;
        let output = std::process::Command::new("getconf")
            .arg("PAGESIZE")
            .output()
            .expect("the POSIX getconf oracle runs");
        assert!(output.status.success(), "getconf PAGESIZE must succeed");
        let expected = String::from_utf8(output.stdout)
            .expect("getconf emits UTF-8 digits")
            .trim()
            .to_owned();
        let actual = invoke_native_json(
            b"|sysconf|isize(i32)",
            format!("[{pagesize_key}]").as_bytes(),
            &NativeLibraryCache::new(),
            region_bound(),
        )
        .expect("the JSON adapter reaches dyn's policy-free ABI core");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&actual).expect("result JSON"),
            serde_json::json!({"type":"isize","value":expected})
        );
    }

    #[cfg(unix)]
    #[test]
    fn json_and_raw_fixed_calls_share_the_u32_argument_grammar() {
        #[cfg(target_os = "macos")]
        let spec = b"|getpriority|i32(i32,u32)";
        #[cfg(target_os = "linux")]
        let spec = b"|getpriority|i32(u32,u32)";

        // SAFETY: this is the independent libc oracle for the same fixed call.
        let expected = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
        let actual = invoke_native_json(spec, b"[0,0]", &NativeLibraryCache::new(), region_bound())
            .expect("the JSON transport accepts the fixed family's declared u32 position");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&actual).expect("result JSON"),
            serde_json::json!({"type":"i32","value":expected})
        );
    }
}
