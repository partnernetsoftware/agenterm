//! Fixed heterogeneous scalar prototypes which cannot use the homogeneous core.

use libloading::Library;

use crate::abi::{MechanismError, mechanism_symbol_error as symbol_error};

/// One scalar type admitted by the fixed-prototype core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedNativeType {
    I32,
    I64,
    U64,
    Isize,
}

/// One canonical scalar argument or result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedNativeValue {
    I32(i32),
    I64(i64),
    U64(u64),
    Isize(isize),
}

impl FixedNativeValue {
    pub const fn ty(self) -> FixedNativeType {
        match self {
            Self::I32(_) => FixedNativeType::I32,
            Self::I64(_) => FixedNativeType::I64,
            Self::U64(_) => FixedNativeType::U64,
            Self::Isize(_) => FixedNativeType::Isize,
        }
    }
}

/// A concrete non-variadic C ABI prototype supported by this core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedNativePrototype {
    /// C `long function(int)`, used by `sysconf` on Unix targets.
    IsizeI32,
    /// C `int64_t function(int, int64_t, int)`, used by `lseek` on supported Unix targets.
    I64I32I64I32,
    /// C `uint64_t function(int)`, used by `clock_gettime_nsec_np` on Darwin.
    U64I32,
    /// C `int function(uint64_t, uint64_t)`, used by `pthread_equal` on Darwin.
    I32U64U64,
}

impl FixedNativePrototype {
    pub const fn result(self) -> FixedNativeType {
        match self {
            Self::IsizeI32 => FixedNativeType::Isize,
            Self::I64I32I64I32 => FixedNativeType::I64,
            Self::U64I32 => FixedNativeType::U64,
            Self::I32U64U64 => FixedNativeType::I32,
        }
    }

    pub const fn parameters(self) -> &'static [FixedNativeType] {
        match self {
            Self::IsizeI32 => &[FixedNativeType::I32],
            Self::I64I32I64I32 => &[
                FixedNativeType::I32,
                FixedNativeType::I64,
                FixedNativeType::I32,
            ],
            Self::U64I32 => &[FixedNativeType::I32],
            Self::I32U64U64 => &[FixedNativeType::U64, FixedNativeType::U64],
        }
    }
}

/// A caller-asserted fixed native call and its canonical arguments.
///
/// The library is not a field: the caller opens it once and passes the handle.
pub struct FixedNativeCall<'a> {
    pub symbol: &'a str,
    pub prototype: FixedNativePrototype,
    pub arguments: &'a [FixedNativeValue],
}

/// Validate a fixed prototype before loading a library or resolving a symbol.
pub fn validate_fixed_native_signature(
    prototype: FixedNativePrototype,
    arguments: &[FixedNativeValue],
) -> Result<(), MechanismError> {
    let parameters = arguments
        .iter()
        .map(|argument| argument.ty())
        .collect::<Vec<_>>();
    if parameters == prototype.parameters() {
        Ok(())
    } else {
        Err(MechanismError::SignatureUnsupported)
    }
}

/// Executes an admitted fixed-family shape against an already open library.
///
/// # Safety
///
/// The caller must uphold [`crate::invoke_abi`]'s complete ABI contract, and
/// `library` must be the library the call names: this entry opens nothing.
pub(crate) unsafe fn invoke_fixed_mechanism_with_library(
    library: &Library,
    call: &FixedNativeCall<'_>,
) -> Result<FixedNativeValue, MechanismError> {
    validate_fixed_native_signature(call.prototype, call.arguments)?;
    match (call.prototype, call.arguments) {
        (FixedNativePrototype::IsizeI32, [FixedNativeValue::I32(a)]) => {
            invoke_isize_i32(library, call.symbol, *a).map(FixedNativeValue::Isize)
        }
        (
            FixedNativePrototype::I64I32I64I32,
            [
                FixedNativeValue::I32(a),
                FixedNativeValue::I64(b),
                FixedNativeValue::I32(c),
            ],
        ) => invoke_i64_i32_i64_i32(library, call.symbol, *a, *b, *c).map(FixedNativeValue::I64),
        (FixedNativePrototype::U64I32, [FixedNativeValue::I32(a)]) => {
            invoke_u64_i32(library, call.symbol, *a).map(FixedNativeValue::U64)
        }
        (FixedNativePrototype::I32U64U64, [FixedNativeValue::U64(a), FixedNativeValue::U64(b)]) => {
            invoke_i32_u64_u64(library, call.symbol, *a, *b).map(FixedNativeValue::I32)
        }
        _ => unreachable!("fixed signature validation admitted the prototype"),
    }
}

fn invoke_i32_u64_u64(
    library: &Library,
    symbol: &str,
    a: u64,
    b: u64,
) -> Result<i32, MechanismError> {
    // SAFETY: invoke_abi admitted this exact prototype; the remaining symbol
    // signature assertion belongs to its unsafe caller.
    let function =
        unsafe { library.get::<unsafe extern "C" fn(u64, u64) -> i32>(symbol.as_bytes()) }
            .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the arguments have the admitted types and the library stays live.
    Ok(unsafe { function(a, b) })
}

fn invoke_u64_i32(library: &Library, symbol: &str, a: i32) -> Result<u64, MechanismError> {
    // SAFETY: invoke_abi admitted this exact prototype; the remaining symbol
    // signature assertion belongs to its unsafe caller.
    let function = unsafe { library.get::<unsafe extern "C" fn(i32) -> u64>(symbol.as_bytes()) }
        .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the argument has the admitted type and the library stays live.
    Ok(unsafe { function(a) })
}

fn invoke_isize_i32(library: &Library, symbol: &str, a: i32) -> Result<isize, MechanismError> {
    // SAFETY: invoke_abi admitted this exact prototype; the remaining symbol
    // signature assertion belongs to its unsafe caller.
    let function = unsafe { library.get::<unsafe extern "C" fn(i32) -> isize>(symbol.as_bytes()) }
        .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the argument has the admitted type and the library stays live.
    Ok(unsafe { function(a) })
}

fn invoke_i64_i32_i64_i32(
    library: &Library,
    symbol: &str,
    a: i32,
    b: i64,
    c: i32,
) -> Result<i64, MechanismError> {
    // SAFETY: invoke_abi admitted this exact prototype; the remaining symbol
    // signature assertion belongs to its unsafe caller.
    let function =
        unsafe { library.get::<unsafe extern "C" fn(i32, i64, i32) -> i64>(symbol.as_bytes()) }
            .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the arguments have the admitted types and the library stays live.
    Ok(unsafe { function(a, b, c) })
}
