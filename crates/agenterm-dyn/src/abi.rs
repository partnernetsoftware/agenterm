//! A policy-free entry over the three native invocation families.
//!
//! # This is a migration bridge, not the finished layering
//!
//! `agenterm-dyn` owns the *mechanism*: which ABI shapes it can actually execute,
//! how a call is laid out, and the error it reports when a shape is beyond that
//! mechanism. It owns **no product policy**: it does not decide which symbols or
//! prototypes a product exposes, and it does not maintain an allow-list.
//!
//! The long-term direction is that the three families (`invoke_exact`,
//! `invoke_fixed`, `invoke_fixed_pointer`) become thin wrappers over one
//! mechanism entry. All three compatibility families now follow that direction:
//! each public legacy entry delegates to [`invoke_abi`], which selects a
//! crate-private family mechanism without re-entering the wrapper.
//!
//! # Nullability and pointee contracts live in the upper layer
//!
//! At the C ABI level a pointer is a pointer: `void *` and "a `void *` that may
//! be null" occupy the same register or stack slot and have the same machine
//! representation. There is therefore **one** [`AbiType::Pointer`] here, and its
//! [`AbiValue`] may hold a null address. Whether a given position may be null,
//! what the pointee's width is, how it must be aligned, and whether a string has
//! to be NUL-terminated are **schema and contract decisions of the caller and the
//! upper layer** — qjswasm spells them as `ptr` versus `ptr?` — and this module
//! neither reads nor enforces them. It does not claim any of them.
//!
//! Raw callers are not only supervised guests: any Rust caller that can uphold
//! [`invoke_abi`]'s contract may use this entry.
//!
//! # What is not listed
//!
//! No type is listed that the mechanism cannot execute today. `Void` is a result
//! position only; the current concrete shape is `void(ptr)`. There is no `F32`
//! (no trampoline takes or returns one). A shape outside the matrix is refused with
//! [`AbiError::SignatureUnsupported`] rather than approximated.

use std::ffi::c_void;
use std::fmt;

use crate::exact_native::{
    ExactNativeCall, ExactNativeError, ExactNativeType, ExactNativeValue, MAX_EXACT_NATIVE_ARITY,
    invoke_exact_mechanism, open_library,
};
use crate::fixed_native::{
    FixedNativeCall, FixedNativeError, FixedNativePrototype, FixedNativeType, FixedNativeValue,
    invoke_fixed_mechanism,
};
use crate::fixed_pointer::{
    FixedPointerCall, FixedPointerError, FixedPointerPrototype, FixedPointerType,
    FixedPointerValue, invoke_fixed_pointer_mechanism,
};

/// One ABI position the mechanism can express.
///
/// The set is exactly the union of the positions the three families' trampolines
/// mention; it is a *mechanism support matrix*, not a product allow-list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiType {
    /// No return register. This is valid only as a result position.
    Void,
    I32,
    U32,
    I64,
    U64,
    Isize,
    Usize,
    F64,
    /// An address argument. It may hold a null address: nullness is not an ABI
    /// distinction, and the pointee contract belongs to the caller.
    Pointer,
}

/// One argument value, tagged with its ABI position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbiValue {
    /// A native function returned through no register.
    Void,
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    Isize(isize),
    Usize(usize),
    F64(f64),
    /// An address argument, possibly null.
    Pointer(*mut c_void),
}

impl AbiValue {
    /// The ABI position this value occupies.
    pub const fn ty(self) -> AbiType {
        match self {
            Self::Void => AbiType::Void,
            Self::I32(_) => AbiType::I32,
            Self::U32(_) => AbiType::U32,
            Self::I64(_) => AbiType::I64,
            Self::U64(_) => AbiType::U64,
            Self::Isize(_) => AbiType::Isize,
            Self::Usize(_) => AbiType::Usize,
            Self::F64(_) => AbiType::F64,
            Self::Pointer(_) => AbiType::Pointer,
        }
    }
}

/// The shape of one native call, decided by the caller.
///
/// The parameter list is borrowed for any lifetime: a caller may build a
/// `Vec<AbiType>` at run time and pass a slice of it, or name a `const` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbiSignature<'a> {
    /// The result position.
    pub result: AbiType,
    /// The argument positions, in order.
    pub params: &'a [AbiType],
}

/// A caller-declared native call. The caller owns the ABI assertion.
#[derive(Debug, Clone, Copy)]
pub struct NativeCall<'a> {
    pub library: &'a str,
    pub symbol: &'a str,
    pub signature: AbiSignature<'a>,
    /// Must have exactly `signature.params.len()` entries.
    pub arguments: &'a [AbiValue],
}

/// A mechanism refusal. Every variant is about *what the mechanism can do*,
/// never about what a product chooses to expose.
#[derive(Debug, Clone, PartialEq)]
pub enum AbiError {
    /// No family trampoline takes this result/parameter shape.
    SignatureUnsupported {
        result: AbiType,
        params: Vec<AbiType>,
    },
    /// Loading the shared library failed.
    LibraryLoad { library: String, message: String },
    /// The symbol is absent, or its resolved type does not match.
    SymbolLookup {
        library: String,
        symbol: String,
        message: String,
    },
    /// The argument list length does not match the declared parameter list.
    ArgumentCount { expected: usize, actual: usize },
    /// An argument's value does not occupy the declared position.
    ArgumentShape {
        index: usize,
        expected: AbiType,
        actual: AbiType,
    },
}

impl fmt::Display for AbiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignatureUnsupported { result, params } => write!(
                formatter,
                "the mechanism has no trampoline for {result:?}({params:?})"
            ),
            Self::LibraryLoad { library, message } => {
                write!(
                    formatter,
                    "could not load native library {library:?}: {message}"
                )
            }
            Self::SymbolLookup {
                library,
                symbol,
                message,
            } => write!(
                formatter,
                "could not resolve {symbol:?} in {library:?}: {message}"
            ),
            Self::ArgumentCount { expected, actual } => write!(
                formatter,
                "the signature declares {expected} parameters but {actual} arguments were supplied"
            ),
            Self::ArgumentShape {
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "argument {index} occupies {actual:?} where the signature declares {expected:?}"
            ),
        }
    }
}

impl std::error::Error for AbiError {}

/// The family that can execute a shape, with the classification the family needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Homogeneous exact: the result type and every parameter share one type.
    Exact {
        ty: ExactNativeType,
        arity: usize,
    },
    Fixed(FixedNativePrototype),
    FixedPointer(FixedPointerPrototype),
    PointerResult(PointerResultPrototype),
    DirectScalar(DirectScalarPrototype),
}

/// Scalar shapes implemented directly by the unified mechanism rather than by
/// a legacy compatibility family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectScalarPrototype {
    VoidPointer,
    I64Pointer,
    IsizeU32,
    I32I32I32Pointer,
    I32I32I32U64PointerI32,
    I32PointerU32PointerPointerPointerUsize,
    UsizeI32PointerUsize,
    I32U32U32,
    I32I32U32,
}

/// Pointer-returning monomorphic shapes implemented directly by the unified
/// mechanism. Pointer ownership and pointee meaning remain the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerResultPrototype {
    NoArguments,
    U32,
    U64,
    PointerUsize,
}

/// Maps an exact-family position onto its family type. `None` means the exact
/// family has no such position.
const fn exact_type(ty: AbiType) -> Option<ExactNativeType> {
    match ty {
        AbiType::Void => None,
        AbiType::I32 => Some(ExactNativeType::I32),
        AbiType::U32 => Some(ExactNativeType::U32),
        AbiType::I64 => Some(ExactNativeType::I64),
        AbiType::U64 => Some(ExactNativeType::U64),
        AbiType::Isize => Some(ExactNativeType::Isize),
        AbiType::Usize => Some(ExactNativeType::Usize),
        AbiType::F64 => Some(ExactNativeType::F64),
        AbiType::Pointer => None,
    }
}

/// Every fixed-family prototype, used to match a declared shape.
const FIXED_PROTOTYPES: [FixedNativePrototype; 4] = [
    FixedNativePrototype::U64I32,
    FixedNativePrototype::IsizeI32,
    FixedNativePrototype::I32U64U64,
    FixedNativePrototype::I64I32I64I32,
];

/// The pointer-family prototypes this bridge maps onto.
///
/// Only the prototypes whose positions are all *required* pointers are listed.
/// The two nullable-tagged prototypes (`I32PointerNullablePointer`,
/// `I32NullablePointerPointer`) are deliberately **not** used: their tags exist to
/// carry an upper-layer schema distinction, and the family validator only
/// compares those tags, so routing a raw call through them would smuggle a policy
/// decision into a policy-free entry. Every address position is `Pointer` here,
/// and the family validator accepts a null `Pointer` like any other.
const POINTER_PROTOTYPES: [FixedPointerPrototype; 8] = [
    FixedPointerPrototype::I32Pointer,
    FixedPointerPrototype::I32PointerI32,
    FixedPointerPrototype::I32PointerU64,
    FixedPointerPrototype::I32I32Pointer,
    FixedPointerPrototype::I32I32PointerU32,
    FixedPointerPrototype::I32U64PointerU64,
    FixedPointerPrototype::I32PointerPointer,
    FixedPointerPrototype::I32PointerPointerPointer,
];

const fn abi_type(ty: FixedNativeType) -> AbiType {
    match ty {
        FixedNativeType::I32 => AbiType::I32,
        FixedNativeType::I64 => AbiType::I64,
        FixedNativeType::U64 => AbiType::U64,
        FixedNativeType::Isize => AbiType::Isize,
    }
}

const fn abi_pointer_type(ty: FixedPointerType) -> AbiType {
    match ty {
        FixedPointerType::I32 => AbiType::I32,
        FixedPointerType::U32 => AbiType::U32,
        FixedPointerType::U64 => AbiType::U64,
        // Both pointer positions are one ABI position. `NullablePointer` is only
        // reachable from an upper-layer schema tag, never from here.
        FixedPointerType::Pointer | FixedPointerType::NullablePointer => AbiType::Pointer,
    }
}

fn fixed_matches(prototype: FixedNativePrototype, signature: AbiSignature<'_>) -> bool {
    if abi_type(prototype.result()) != signature.result {
        return false;
    }
    let parameters = prototype.parameters();
    parameters.len() == signature.params.len()
        && parameters
            .iter()
            .zip(signature.params)
            .all(|(expected, declared)| abi_type(*expected) == *declared)
}

fn pointer_matches(prototype: FixedPointerPrototype, signature: AbiSignature<'_>) -> bool {
    if signature.result != AbiType::I32 {
        return false;
    }
    let parameters = prototype.parameters();
    parameters.len() == signature.params.len()
        && parameters
            .iter()
            .zip(signature.params)
            .all(|(expected, declared)| abi_pointer_type(*expected) == *declared)
}

/// Chooses the family that can execute `signature` with `arguments`.
///
/// This is a total function over the mechanism support matrix: a shape outside it
/// is refused, never approximated.
fn classify(signature: AbiSignature<'_>, arguments: &[AbiValue]) -> Result<Family, AbiError> {
    if arguments.len() != signature.params.len() {
        return Err(AbiError::ArgumentCount {
            expected: signature.params.len(),
            actual: arguments.len(),
        });
    }
    for (index, (argument, declared)) in arguments.iter().zip(signature.params).enumerate() {
        let actual = argument.ty();
        if actual != *declared {
            return Err(AbiError::ArgumentShape {
                index,
                expected: *declared,
                actual,
            });
        }
    }
    // Exact first: homogeneous, arity-bounded, and the most common shape.
    if let Some(ty) = exact_type(signature.result) {
        let arity = signature.params.len();
        let homogeneous = signature
            .params
            .iter()
            .all(|declared| *declared == signature.result);
        if homogeneous && arity <= MAX_EXACT_NATIVE_ARITY {
            return Ok(Family::Exact { ty, arity });
        }
    }
    if let Some(prototype) = FIXED_PROTOTYPES
        .into_iter()
        .find(|prototype| fixed_matches(*prototype, signature))
    {
        return Ok(Family::Fixed(prototype));
    }
    if let Some(prototype) = POINTER_PROTOTYPES
        .into_iter()
        .find(|prototype| pointer_matches(*prototype, signature))
    {
        return Ok(Family::FixedPointer(prototype));
    }
    if signature.result == AbiType::Pointer {
        match signature.params {
            [] => return Ok(Family::PointerResult(PointerResultPrototype::NoArguments)),
            [AbiType::U32] => return Ok(Family::PointerResult(PointerResultPrototype::U32)),
            [AbiType::U64] => return Ok(Family::PointerResult(PointerResultPrototype::U64)),
            [AbiType::Pointer, AbiType::Usize] => {
                return Ok(Family::PointerResult(PointerResultPrototype::PointerUsize));
            }
            _ => {}
        }
    }
    if signature.result == AbiType::Void && signature.params == [AbiType::Pointer] {
        return Ok(Family::DirectScalar(DirectScalarPrototype::VoidPointer));
    }
    if signature.result == AbiType::I64 && signature.params == [AbiType::Pointer] {
        return Ok(Family::DirectScalar(DirectScalarPrototype::I64Pointer));
    }
    if signature.result == AbiType::Isize && signature.params == [AbiType::U32] {
        return Ok(Family::DirectScalar(DirectScalarPrototype::IsizeU32));
    }
    if signature.result == AbiType::I32
        && signature.params == [AbiType::I32, AbiType::I32, AbiType::Pointer]
    {
        return Ok(Family::DirectScalar(
            DirectScalarPrototype::I32I32I32Pointer,
        ));
    }
    if signature.result == AbiType::I32
        && signature.params
            == [
                AbiType::I32,
                AbiType::I32,
                AbiType::U64,
                AbiType::Pointer,
                AbiType::I32,
            ]
    {
        return Ok(Family::DirectScalar(
            DirectScalarPrototype::I32I32I32U64PointerI32,
        ));
    }
    if signature.result == AbiType::I32
        && signature.params
            == [
                AbiType::Pointer,
                AbiType::U32,
                AbiType::Pointer,
                AbiType::Pointer,
                AbiType::Pointer,
                AbiType::Usize,
            ]
    {
        return Ok(Family::DirectScalar(
            DirectScalarPrototype::I32PointerU32PointerPointerPointerUsize,
        ));
    }
    if signature.result == AbiType::Usize
        && signature.params == [AbiType::I32, AbiType::Pointer, AbiType::Usize]
    {
        return Ok(Family::DirectScalar(
            DirectScalarPrototype::UsizeI32PointerUsize,
        ));
    }
    if signature.result == AbiType::I32 && signature.params == [AbiType::U32, AbiType::U32] {
        return Ok(Family::DirectScalar(DirectScalarPrototype::I32U32U32));
    }
    if signature.result == AbiType::I32 && signature.params == [AbiType::I32, AbiType::U32] {
        return Ok(Family::DirectScalar(DirectScalarPrototype::I32I32U32));
    }
    Err(AbiError::SignatureUnsupported {
        result: signature.result,
        params: signature.params.to_vec(),
    })
}

/// Validates a caller-declared call against the mechanism support matrix without
/// executing it.
pub fn validate_abi(call: &NativeCall<'_>) -> Result<(), AbiError> {
    classify(call.signature, call.arguments).map(|_| ())
}

fn exact_argument(value: AbiValue) -> Option<ExactNativeValue> {
    Some(match value {
        AbiValue::Void => return None,
        AbiValue::I32(bits) => ExactNativeValue::I32(bits),
        AbiValue::U32(bits) => ExactNativeValue::U32(bits),
        AbiValue::I64(bits) => ExactNativeValue::I64(bits),
        AbiValue::U64(bits) => ExactNativeValue::U64(bits),
        AbiValue::Isize(bits) => ExactNativeValue::Isize(bits),
        AbiValue::Usize(bits) => ExactNativeValue::Usize(bits),
        AbiValue::F64(bits) => ExactNativeValue::F64(bits),
        AbiValue::Pointer(_) => return None,
    })
}

fn fixed_argument(value: AbiValue) -> Option<FixedNativeValue> {
    Some(match value {
        AbiValue::Void => return None,
        AbiValue::I32(bits) => FixedNativeValue::I32(bits),
        AbiValue::I64(bits) => FixedNativeValue::I64(bits),
        AbiValue::U64(bits) => FixedNativeValue::U64(bits),
        AbiValue::Isize(bits) => FixedNativeValue::Isize(bits),
        _ => return None,
    })
}

fn pointer_argument(value: AbiValue) -> Option<FixedPointerValue> {
    Some(match value {
        AbiValue::Void => return None,
        AbiValue::I32(bits) => FixedPointerValue::I32(bits),
        AbiValue::U32(bits) => FixedPointerValue::U32(bits),
        AbiValue::U64(bits) => FixedPointerValue::U64(bits),
        // A null address is still an address: it stays the required-pointer tag,
        // because nullness is not an ABI position.
        AbiValue::Pointer(address) => FixedPointerValue::Pointer(address),
        _ => return None,
    })
}

const fn exact_abi_value(value: ExactNativeValue) -> AbiValue {
    match value {
        ExactNativeValue::I32(bits) => AbiValue::I32(bits),
        ExactNativeValue::U32(bits) => AbiValue::U32(bits),
        ExactNativeValue::I64(bits) => AbiValue::I64(bits),
        ExactNativeValue::U64(bits) => AbiValue::U64(bits),
        ExactNativeValue::Isize(bits) => AbiValue::Isize(bits),
        ExactNativeValue::Usize(bits) => AbiValue::Usize(bits),
        ExactNativeValue::F64(bits) => AbiValue::F64(bits),
    }
}

const fn fixed_abi_value(value: FixedNativeValue) -> AbiValue {
    match value {
        FixedNativeValue::I32(bits) => AbiValue::I32(bits),
        FixedNativeValue::I64(bits) => AbiValue::I64(bits),
        FixedNativeValue::U64(bits) => AbiValue::U64(bits),
        FixedNativeValue::Isize(bits) => AbiValue::Isize(bits),
    }
}

fn exact_error(
    error: ExactNativeError,
    signature: AbiSignature<'_>,
    call: &NativeCall<'_>,
) -> AbiError {
    match error {
        ExactNativeError::LibraryLoad { library, message } => {
            AbiError::LibraryLoad { library, message }
        }
        ExactNativeError::SymbolLoad { symbol, message } => AbiError::SymbolLookup {
            library: call.library.to_owned(),
            symbol,
            message,
        },
        ExactNativeError::SignatureUnsupported { .. } => AbiError::SignatureUnsupported {
            result: signature.result,
            params: signature.params.to_vec(),
        },
    }
}

fn fixed_error(
    error: FixedNativeError,
    signature: AbiSignature<'_>,
    call: &NativeCall<'_>,
) -> AbiError {
    match error {
        FixedNativeError::LibraryLoad { library, message } => {
            AbiError::LibraryLoad { library, message }
        }
        FixedNativeError::SymbolLoad { symbol, message } => AbiError::SymbolLookup {
            library: call.library.to_owned(),
            symbol,
            message,
        },
        FixedNativeError::SignatureUnsupported { .. } => AbiError::SignatureUnsupported {
            result: signature.result,
            params: signature.params.to_vec(),
        },
    }
}

fn pointer_error(
    error: FixedPointerError,
    signature: AbiSignature<'_>,
    call: &NativeCall<'_>,
) -> AbiError {
    match error {
        FixedPointerError::LibraryLoad { library, message } => {
            AbiError::LibraryLoad { library, message }
        }
        FixedPointerError::SymbolLoad { symbol, message } => AbiError::SymbolLookup {
            library: call.library.to_owned(),
            symbol,
            message,
        },
        FixedPointerError::SignatureUnsupported { .. } => AbiError::SignatureUnsupported {
            result: signature.result,
            params: signature.params.to_vec(),
        },
    }
}

/// Executes a caller-declared call through the family that owns its shape.
///
/// # Safety
///
/// The caller asserts the whole ABI contract of the resolved symbol:
///
/// * `signature` must be the symbol's real signature. This module checks that the
///   shape is one the mechanism can execute, and that each argument occupies the
///   position the signature declares — it cannot check the symbol's own
///   declaration, because it never sees one.
/// * Every [`AbiType::Pointer`] argument must be an address that stays valid,
///   aligned and aliasing-legal for the whole call, as the callee requires. A
///   null address is permitted by the mechanism but is only correct where the
///   callee accepts it: that nullability decision is the **caller's contract**,
///   not this module's.
/// * The library and thread requirements of the symbol are the caller's, and so
///   are its resource cleanup and process side effects.
pub unsafe fn invoke_abi(call: &NativeCall<'_>) -> Result<AbiValue, AbiError> {
    let signature = call.signature;
    match classify(signature, call.arguments)? {
        Family::Exact { ty, arity } => {
            let arguments = call
                .arguments
                .iter()
                .map(|value| exact_argument(*value))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| AbiError::SignatureUnsupported {
                    result: signature.result,
                    params: signature.params.to_vec(),
                })?;
            debug_assert_eq!(arguments.len(), arity);
            let exact = ExactNativeCall {
                library: call.library,
                symbol: call.symbol,
                result: ty,
                arguments: &arguments,
            };
            // SAFETY: the caller upholds `invoke_abi`'s contract; the shape was
            // admitted by the family's own validator.
            match unsafe { invoke_exact_mechanism(&exact) } {
                Ok(value) => Ok(exact_abi_value(value)),
                Err(error) => Err(exact_error(error, signature, call)),
            }
        }
        Family::Fixed(prototype) => {
            let arguments = call
                .arguments
                .iter()
                .map(|value| fixed_argument(*value))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| AbiError::SignatureUnsupported {
                    result: signature.result,
                    params: signature.params.to_vec(),
                })?;
            let fixed = FixedNativeCall {
                library: call.library,
                symbol: call.symbol,
                prototype,
                arguments: &arguments,
            };
            // SAFETY: as above.
            match unsafe { invoke_fixed_mechanism(&fixed) } {
                Ok(value) => Ok(fixed_abi_value(value)),
                Err(error) => Err(fixed_error(error, signature, call)),
            }
        }
        Family::FixedPointer(prototype) => {
            let arguments = call
                .arguments
                .iter()
                .map(|value| pointer_argument(*value))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| AbiError::SignatureUnsupported {
                    result: signature.result,
                    params: signature.params.to_vec(),
                })?;
            let pointer = FixedPointerCall {
                library: call.library,
                symbol: call.symbol,
                prototype,
                arguments: &arguments,
            };
            // SAFETY: as above.
            match unsafe { invoke_fixed_pointer_mechanism(&pointer) } {
                Ok(status) => Ok(AbiValue::I32(status)),
                Err(error) => Err(pointer_error(error, signature, call)),
            }
        }
        Family::PointerResult(prototype) => {
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: if call.library.is_empty() {
                    "<current-process>".to_owned()
                } else {
                    call.library.to_owned()
                },
                message: error.to_string(),
            })?;
            let address = match (prototype, call.arguments) {
                (PointerResultPrototype::NoArguments, []) => {
                    // SAFETY: classification admitted this exact shape; the caller
                    // asserts that the symbol really has the declared C ABI.
                    let function = unsafe {
                        library.get::<unsafe extern "C" fn() -> *mut c_void>(call.symbol.as_bytes())
                    }
                    .map_err(|error| pointer_result_symbol_error(call, error))?;
                    // SAFETY: the caller owns the symbol contract and library lifetime.
                    unsafe { function() }
                }
                (PointerResultPrototype::U32, [AbiValue::U32(argument)]) => {
                    // SAFETY: as above, for the admitted `ptr(u32)` shape.
                    let function = unsafe {
                        library
                            .get::<unsafe extern "C" fn(u32) -> *mut c_void>(call.symbol.as_bytes())
                    }
                    .map_err(|error| pointer_result_symbol_error(call, error))?;
                    // SAFETY: the caller owns the symbol contract and library lifetime.
                    unsafe { function(*argument) }
                }
                (PointerResultPrototype::U64, [AbiValue::U64(argument)]) => {
                    // SAFETY: as above, for the admitted `ptr(u64)` shape.
                    let function = unsafe {
                        library
                            .get::<unsafe extern "C" fn(u64) -> *mut c_void>(call.symbol.as_bytes())
                    }
                    .map_err(|error| pointer_result_symbol_error(call, error))?;
                    // SAFETY: the caller owns the symbol contract and library lifetime.
                    unsafe { function(*argument) }
                }
                (
                    PointerResultPrototype::PointerUsize,
                    [AbiValue::Pointer(buffer), AbiValue::Usize(length)],
                ) => {
                    // SAFETY: as above, for the admitted `ptr(ptr,usize)` shape.
                    let function = unsafe {
                        library.get::<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void>(
                            call.symbol.as_bytes(),
                        )
                    }
                    .map_err(|error| pointer_result_symbol_error(call, error))?;
                    // SAFETY: the caller owns the complete buffer contract.
                    unsafe { function(*buffer, *length) }
                }
                _ => unreachable!("classification checked pointer-result arguments"),
            };
            Ok(AbiValue::Pointer(address))
        }
        Family::DirectScalar(DirectScalarPrototype::IsizeU32) => {
            let [AbiValue::U32(argument)] = call.arguments else {
                unreachable!("classification checked isize(u32) arguments")
            };
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: display_library(call.library),
                message: error.to_string(),
            })?;
            // SAFETY: classification admitted `isize(u32)`; the caller asserts
            // that the resolved symbol really has this C ABI.
            let function = unsafe {
                library.get::<unsafe extern "C" fn(u32) -> isize>(call.symbol.as_bytes())
            }
            .map_err(|error| pointer_result_symbol_error(call, error))?;
            // SAFETY: the caller owns the symbol contract and library lifetime.
            Ok(AbiValue::Isize(unsafe { function(*argument) }))
        }
        Family::DirectScalar(DirectScalarPrototype::VoidPointer) => {
            let [AbiValue::Pointer(argument)] = call.arguments else {
                unreachable!("classification checked void(ptr) arguments")
            };
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: display_library(call.library),
                message: error.to_string(),
            })?;
            // SAFETY: classification admitted `void(ptr)`; the caller asserts
            // that the resolved symbol really has this C ABI.
            let function =
                unsafe { library.get::<unsafe extern "C" fn(*mut c_void)>(call.symbol.as_bytes()) }
                    .map_err(|error| pointer_result_symbol_error(call, error))?;
            // SAFETY: the caller owns the symbol contract, pointer validity and
            // library lifetime.
            unsafe { function(*argument) };
            Ok(AbiValue::Void)
        }
        Family::DirectScalar(DirectScalarPrototype::I64Pointer) => {
            let [AbiValue::Pointer(argument)] = call.arguments else {
                unreachable!("classification checked i64(ptr) arguments")
            };
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: display_library(call.library),
                message: error.to_string(),
            })?;
            // SAFETY: classification admitted `i64(ptr)`; the caller asserts
            // that the resolved symbol really has this C ABI.
            let function = unsafe {
                library.get::<unsafe extern "C" fn(*mut c_void) -> i64>(call.symbol.as_bytes())
            }
            .map_err(|error| pointer_result_symbol_error(call, error))?;
            // SAFETY: the caller owns the symbol contract, pointer validity and
            // library lifetime.
            Ok(AbiValue::I64(unsafe { function(*argument) }))
        }
        Family::DirectScalar(DirectScalarPrototype::I32I32I32Pointer) => {
            let [
                AbiValue::I32(first),
                AbiValue::I32(second),
                AbiValue::Pointer(output),
            ] = call.arguments
            else {
                unreachable!("classification checked i32(i32,i32,ptr) arguments")
            };
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: display_library(call.library),
                message: error.to_string(),
            })?;
            // SAFETY: classification admitted `i32(i32,i32,ptr)`; the caller
            // asserts that the resolved symbol really has this C ABI.
            let function = unsafe {
                library.get::<unsafe extern "C" fn(i32, i32, *mut c_void) -> i32>(
                    call.symbol.as_bytes(),
                )
            }
            .map_err(|error| pointer_result_symbol_error(call, error))?;
            // SAFETY: the caller owns the symbol contract, output storage and
            // library lifetime.
            Ok(AbiValue::I32(unsafe { function(*first, *second, *output) }))
        }
        Family::DirectScalar(DirectScalarPrototype::I32I32I32U64PointerI32) => {
            let [
                AbiValue::I32(pid),
                AbiValue::I32(flavor),
                AbiValue::U64(argument),
                AbiValue::Pointer(output),
                AbiValue::I32(size),
            ] = call.arguments
            else {
                unreachable!("classification checked i32(i32,i32,u64,ptr,i32) arguments")
            };
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: display_library(call.library),
                message: error.to_string(),
            })?;
            // SAFETY: classification admitted `i32(i32,i32,u64,ptr,i32)`; the
            // caller asserts that the resolved symbol really has this C ABI.
            let function = unsafe {
                library.get::<unsafe extern "C" fn(i32, i32, u64, *mut c_void, i32) -> i32>(
                    call.symbol.as_bytes(),
                )
            }
            .map_err(|error| pointer_result_symbol_error(call, error))?;
            // SAFETY: the caller owns the symbol contract, output storage and
            // library lifetime.
            Ok(AbiValue::I32(unsafe {
                function(*pid, *flavor, *argument, *output, *size)
            }))
        }
        Family::DirectScalar(DirectScalarPrototype::I32PointerU32PointerPointerPointerUsize) => {
            let [
                AbiValue::Pointer(name),
                AbiValue::U32(name_length),
                AbiValue::Pointer(old_value),
                AbiValue::Pointer(old_length),
                AbiValue::Pointer(new_value),
                AbiValue::Usize(new_length),
            ] = call.arguments
            else {
                unreachable!("classification checked i32(ptr,u32,ptr,ptr,ptr,usize) arguments")
            };
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: display_library(call.library),
                message: error.to_string(),
            })?;
            // SAFETY: classification admitted `i32(ptr,u32,ptr,ptr,ptr,usize)`;
            // the caller asserts that the resolved symbol really has this C ABI.
            let function = unsafe {
                library.get::<unsafe extern "C" fn(
                    *mut c_void,
                    u32,
                    *mut c_void,
                    *mut c_void,
                    *mut c_void,
                    usize,
                ) -> i32>(call.symbol.as_bytes())
            }
            .map_err(|error| pointer_result_symbol_error(call, error))?;
            // SAFETY: the caller owns the symbol contract, all pointer
            // lifetimes and the library lifetime.
            Ok(AbiValue::I32(unsafe {
                function(
                    *name,
                    *name_length,
                    *old_value,
                    *old_length,
                    *new_value,
                    *new_length,
                )
            }))
        }
        Family::DirectScalar(DirectScalarPrototype::UsizeI32PointerUsize) => {
            let [
                AbiValue::I32(name),
                AbiValue::Pointer(buffer),
                AbiValue::Usize(length),
            ] = call.arguments
            else {
                unreachable!("classification checked usize(i32,ptr,usize) arguments")
            };
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: display_library(call.library),
                message: error.to_string(),
            })?;
            // SAFETY: classification admitted `usize(i32,ptr,usize)`; the caller
            // asserts that the resolved symbol really has this C ABI.
            let function = unsafe {
                library.get::<unsafe extern "C" fn(i32, *mut c_void, usize) -> usize>(
                    call.symbol.as_bytes(),
                )
            }
            .map_err(|error| pointer_result_symbol_error(call, error))?;
            // SAFETY: the caller owns the symbol contract, pointer validity and
            // library lifetime.
            Ok(AbiValue::Usize(unsafe {
                function(*name, *buffer, *length)
            }))
        }
        Family::DirectScalar(DirectScalarPrototype::I32U32U32) => {
            let [AbiValue::U32(which), AbiValue::U32(who)] = call.arguments else {
                unreachable!("classification checked i32(u32,u32) arguments")
            };
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: display_library(call.library),
                message: error.to_string(),
            })?;
            // SAFETY: classification admitted `i32(u32,u32)`; the caller
            // asserts that the resolved symbol really has this C ABI.
            let function = unsafe {
                library.get::<unsafe extern "C" fn(u32, u32) -> i32>(call.symbol.as_bytes())
            }
            .map_err(|error| pointer_result_symbol_error(call, error))?;
            // SAFETY: the caller owns the symbol contract and library lifetime.
            Ok(AbiValue::I32(unsafe { function(*which, *who) }))
        }
        Family::DirectScalar(DirectScalarPrototype::I32I32U32) => {
            let [AbiValue::I32(which), AbiValue::U32(who)] = call.arguments else {
                unreachable!("classification checked i32(i32,u32) arguments")
            };
            let library = open_library(call.library).map_err(|error| AbiError::LibraryLoad {
                library: display_library(call.library),
                message: error.to_string(),
            })?;
            // SAFETY: classification admitted `i32(i32,u32)`; the caller
            // asserts that the resolved symbol really has this C ABI.
            let function = unsafe {
                library.get::<unsafe extern "C" fn(i32, u32) -> i32>(call.symbol.as_bytes())
            }
            .map_err(|error| pointer_result_symbol_error(call, error))?;
            // SAFETY: the caller owns the symbol contract and library lifetime.
            Ok(AbiValue::I32(unsafe { function(*which, *who) }))
        }
    }
}

fn display_library(library: &str) -> String {
    if library.is_empty() {
        "<current-process>".to_owned()
    } else {
        library.to_owned()
    }
}

fn pointer_result_symbol_error(call: &NativeCall<'_>, error: libloading::Error) -> AbiError {
    AbiError::SymbolLookup {
        library: display_library(call.library),
        symbol: call.symbol.to_owned(),
        message: error.to_string(),
    }
}
