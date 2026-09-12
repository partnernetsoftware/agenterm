//! Fixed heterogeneous scalar prototypes which cannot use the homogeneous core.

use std::fmt;

use libloading::Library;

use crate::exact_native::open_library;

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
}

impl FixedNativePrototype {
    pub const fn result(self) -> FixedNativeType {
        match self {
            Self::IsizeI32 => FixedNativeType::Isize,
            Self::I64I32I64I32 => FixedNativeType::I64,
            Self::U64I32 => FixedNativeType::U64,
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
        }
    }
}

/// A caller-asserted fixed native call and its canonical arguments.
pub struct FixedNativeCall<'a> {
    pub library: &'a str,
    pub symbol: &'a str,
    pub prototype: FixedNativePrototype,
    pub arguments: &'a [FixedNativeValue],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FixedNativeError {
    SignatureUnsupported {
        prototype: FixedNativePrototype,
        parameters: Vec<FixedNativeType>,
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

impl fmt::Display for FixedNativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignatureUnsupported {
                prototype,
                parameters,
            } => write!(
                f,
                "arguments do not match fixed prototype {prototype:?}: {parameters:?}"
            ),
            Self::LibraryLoad { library, message } => {
                write!(f, "could not load native library {library:?}: {message}")
            }
            Self::SymbolLoad { symbol, message } => {
                write!(f, "could not resolve native symbol {symbol:?}: {message}")
            }
        }
    }
}

impl std::error::Error for FixedNativeError {}

/// Validate a fixed prototype before loading a library or resolving a symbol.
pub fn validate_fixed_native_signature(
    prototype: FixedNativePrototype,
    arguments: &[FixedNativeValue],
) -> Result<(), FixedNativeError> {
    let parameters = arguments
        .iter()
        .map(|argument| argument.ty())
        .collect::<Vec<_>>();
    if parameters == prototype.parameters() {
        Ok(())
    } else {
        Err(FixedNativeError::SignatureUnsupported {
            prototype,
            parameters,
        })
    }
}

/// Resolve and invoke one enumerated heterogeneous scalar prototype.
///
/// # Safety
/// The caller asserts that `symbol` really has `prototype`'s fixed,
/// non-variadic C ABI. Native initializers, finalizers, and the function may
/// have arbitrary process effects.
pub unsafe fn invoke_fixed(
    call: &FixedNativeCall<'_>,
) -> Result<FixedNativeValue, FixedNativeError> {
    validate_fixed_native_signature(call.prototype, call.arguments)?;
    let library = open_library(call.library).map_err(|error| FixedNativeError::LibraryLoad {
        library: if call.library.is_empty() {
            "<current-process>".to_owned()
        } else {
            call.library.to_owned()
        },
        message: error.to_string(),
    })?;
    match (call.prototype, call.arguments) {
        (FixedNativePrototype::IsizeI32, [FixedNativeValue::I32(a)]) => {
            invoke_isize_i32(&library, call.symbol, *a).map(FixedNativeValue::Isize)
        }
        (
            FixedNativePrototype::I64I32I64I32,
            [
                FixedNativeValue::I32(a),
                FixedNativeValue::I64(b),
                FixedNativeValue::I32(c),
            ],
        ) => invoke_i64_i32_i64_i32(&library, call.symbol, *a, *b, *c).map(FixedNativeValue::I64),
        (FixedNativePrototype::U64I32, [FixedNativeValue::I32(a)]) => {
            invoke_u64_i32(&library, call.symbol, *a).map(FixedNativeValue::U64)
        }
        _ => unreachable!("fixed signature validation admitted the prototype"),
    }
}

fn invoke_u64_i32(library: &Library, symbol: &str, a: i32) -> Result<u64, FixedNativeError> {
    // SAFETY: invoke_fixed admitted this exact prototype; the remaining symbol
    // signature assertion belongs to its unsafe caller.
    let function = unsafe { library.get::<unsafe extern "C" fn(i32) -> u64>(symbol.as_bytes()) }
        .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the argument has the admitted type and the library stays live.
    Ok(unsafe { function(a) })
}

fn symbol_error(symbol: &str, error: libloading::Error) -> FixedNativeError {
    FixedNativeError::SymbolLoad {
        symbol: symbol.to_owned(),
        message: error.to_string(),
    }
}

fn invoke_isize_i32(library: &Library, symbol: &str, a: i32) -> Result<isize, FixedNativeError> {
    // SAFETY: invoke_fixed admitted this exact prototype; the remaining symbol
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
) -> Result<i64, FixedNativeError> {
    // SAFETY: invoke_fixed admitted this exact prototype; the remaining symbol
    // signature assertion belongs to its unsafe caller.
    let function =
        unsafe { library.get::<unsafe extern "C" fn(i32, i64, i32) -> i64>(symbol.as_bytes()) }
            .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the arguments have the admitted types and the library stays live.
    Ok(unsafe { function(a, b, c) })
}
