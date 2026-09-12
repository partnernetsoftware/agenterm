//! Fixed caller-buffer prototypes shared by native-door consumers.

use std::ffi::c_void;
use std::fmt;

use libloading::Library;

use crate::exact_native::open_library;

/// One argument type admitted by the fixed pointer core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedPointerType {
    I32,
    U32,
    Pointer,
    NullablePointer,
}

/// One canonical argument for a fixed pointer prototype.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedPointerValue {
    I32(i32),
    U32(u32),
    Pointer(*mut c_void),
    NullablePointer(*mut c_void),
}

impl FixedPointerValue {
    pub const fn ty(self) -> FixedPointerType {
        match self {
            Self::I32(_) => FixedPointerType::I32,
            Self::U32(_) => FixedPointerType::U32,
            Self::Pointer(_) => FixedPointerType::Pointer,
            Self::NullablePointer(_) => FixedPointerType::NullablePointer,
        }
    }
}

/// A concrete non-variadic C ABI prototype with a synchronous pointer argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedPointerPrototype {
    /// C `int function(void *)`, used by `uname`.
    I32Pointer,
    /// C `int function(int, void *)`, used by `clock_gettime` and `getrlimit`.
    I32I32Pointer,
    /// C `int function(void *, int)`, used by `access`.
    I32PointerI32,
    /// C `int function(void *, void *)`, with only the second pointer nullable.
    I32PointerNullablePointer,
    /// C `int function(void *, void *)`, with only the first pointer nullable.
    I32NullablePointerPointer,
    /// C `int function(int, void *, unsigned int)`, used by `proc_pidpath`.
    I32I32PointerU32,
}

impl FixedPointerPrototype {
    pub const fn parameters(self) -> &'static [FixedPointerType] {
        match self {
            Self::I32Pointer => &[FixedPointerType::Pointer],
            Self::I32I32Pointer => &[FixedPointerType::I32, FixedPointerType::Pointer],
            Self::I32PointerI32 => &[FixedPointerType::Pointer, FixedPointerType::I32],
            Self::I32PointerNullablePointer => {
                &[FixedPointerType::Pointer, FixedPointerType::NullablePointer]
            }
            Self::I32NullablePointerPointer => {
                &[FixedPointerType::NullablePointer, FixedPointerType::Pointer]
            }
            Self::I32I32PointerU32 => &[
                FixedPointerType::I32,
                FixedPointerType::Pointer,
                FixedPointerType::U32,
            ],
        }
    }
}

/// A caller-asserted fixed pointer call and its canonical arguments.
pub struct FixedPointerCall<'a> {
    pub library: &'a str,
    pub symbol: &'a str,
    pub prototype: FixedPointerPrototype,
    pub arguments: &'a [FixedPointerValue],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FixedPointerError {
    SignatureUnsupported {
        prototype: FixedPointerPrototype,
        parameters: Vec<FixedPointerType>,
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

impl fmt::Display for FixedPointerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignatureUnsupported {
                prototype,
                parameters,
            } => write!(
                f,
                "arguments do not match fixed pointer prototype {prototype:?}: {parameters:?}"
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

impl std::error::Error for FixedPointerError {}

/// Validate a fixed pointer prototype before loading a library or resolving a symbol.
pub fn validate_fixed_pointer_signature(
    prototype: FixedPointerPrototype,
    arguments: &[FixedPointerValue],
) -> Result<(), FixedPointerError> {
    let parameters = arguments
        .iter()
        .map(|argument| argument.ty())
        .collect::<Vec<_>>();
    if parameters == prototype.parameters() {
        Ok(())
    } else {
        Err(FixedPointerError::SignatureUnsupported {
            prototype,
            parameters,
        })
    }
}

/// Resolve and invoke one enumerated synchronous caller-buffer prototype.
///
/// # Safety
/// The caller asserts that `symbol` really has `prototype`'s fixed,
/// non-variadic C ABI. Every pointer must be valid for the callee's complete
/// synchronous access, correctly aligned, and obey its initialization,
/// mutability, aliasing, and pointee-size requirements. The callee must not
/// retain a pointer after returning. Native initializers, finalizers, and the
/// function itself may have arbitrary process effects.
pub unsafe fn invoke_fixed_pointer(call: &FixedPointerCall<'_>) -> Result<i32, FixedPointerError> {
    validate_fixed_pointer_signature(call.prototype, call.arguments)?;
    let library = open_library(call.library).map_err(|error| FixedPointerError::LibraryLoad {
        library: if call.library.is_empty() {
            "<current-process>".to_owned()
        } else {
            call.library.to_owned()
        },
        message: error.to_string(),
    })?;
    match (call.prototype, call.arguments) {
        (FixedPointerPrototype::I32Pointer, [FixedPointerValue::Pointer(a)]) => {
            invoke_i32_pointer(&library, call.symbol, *a)
        }
        (
            FixedPointerPrototype::I32I32Pointer,
            [FixedPointerValue::I32(a), FixedPointerValue::Pointer(b)],
        ) => invoke_i32_i32_pointer(&library, call.symbol, *a, *b),
        (
            FixedPointerPrototype::I32PointerI32,
            [FixedPointerValue::Pointer(a), FixedPointerValue::I32(b)],
        ) => invoke_i32_pointer_i32(&library, call.symbol, *a, *b),
        (
            FixedPointerPrototype::I32PointerNullablePointer,
            [
                FixedPointerValue::Pointer(a),
                FixedPointerValue::NullablePointer(b),
            ],
        ) => invoke_i32_pointer_pointer(&library, call.symbol, *a, *b),
        (
            FixedPointerPrototype::I32NullablePointerPointer,
            [
                FixedPointerValue::NullablePointer(a),
                FixedPointerValue::Pointer(b),
            ],
        ) => invoke_i32_pointer_pointer(&library, call.symbol, *a, *b),
        (
            FixedPointerPrototype::I32I32PointerU32,
            [
                FixedPointerValue::I32(a),
                FixedPointerValue::Pointer(b),
                FixedPointerValue::U32(c),
            ],
        ) => invoke_i32_i32_pointer_u32(&library, call.symbol, *a, *b, *c),
        _ => unreachable!("fixed pointer signature validation admitted the prototype"),
    }
}

fn invoke_i32_pointer_pointer(
    library: &Library,
    symbol: &str,
    a: *mut c_void,
    b: *mut c_void,
) -> Result<i32, FixedPointerError> {
    // SAFETY: see invoke_i32_pointer.
    let function = unsafe {
        library.get::<unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32>(symbol.as_bytes())
    }
    .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns both pointer contracts and the library stays live.
    Ok(unsafe { function(a, b) })
}

fn symbol_error(symbol: &str, error: libloading::Error) -> FixedPointerError {
    FixedPointerError::SymbolLoad {
        symbol: symbol.to_owned(),
        message: error.to_string(),
    }
}

fn invoke_i32_pointer(
    library: &Library,
    symbol: &str,
    a: *mut c_void,
) -> Result<i32, FixedPointerError> {
    // SAFETY: invoke_fixed_pointer admitted this exact prototype; the remaining
    // symbol and pointee assertions belong to its unsafe caller.
    let function =
        unsafe { library.get::<unsafe extern "C" fn(*mut c_void) -> i32>(symbol.as_bytes()) }
            .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns the pointer contract and the library stays live.
    Ok(unsafe { function(a) })
}

fn invoke_i32_i32_pointer(
    library: &Library,
    symbol: &str,
    a: i32,
    b: *mut c_void,
) -> Result<i32, FixedPointerError> {
    // SAFETY: see invoke_i32_pointer.
    let function =
        unsafe { library.get::<unsafe extern "C" fn(i32, *mut c_void) -> i32>(symbol.as_bytes()) }
            .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns the pointer contract and the library stays live.
    Ok(unsafe { function(a, b) })
}

fn invoke_i32_pointer_i32(
    library: &Library,
    symbol: &str,
    a: *mut c_void,
    b: i32,
) -> Result<i32, FixedPointerError> {
    // SAFETY: see invoke_i32_pointer.
    let function =
        unsafe { library.get::<unsafe extern "C" fn(*mut c_void, i32) -> i32>(symbol.as_bytes()) }
            .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns the pointer contract and the library stays live.
    Ok(unsafe { function(a, b) })
}

fn invoke_i32_i32_pointer_u32(
    library: &Library,
    symbol: &str,
    a: i32,
    b: *mut c_void,
    c: u32,
) -> Result<i32, FixedPointerError> {
    // SAFETY: see invoke_i32_pointer.
    let function = unsafe {
        library.get::<unsafe extern "C" fn(i32, *mut c_void, u32) -> i32>(symbol.as_bytes())
    }
    .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns the pointer contract and the library stays live.
    Ok(unsafe { function(a, b, c) })
}
