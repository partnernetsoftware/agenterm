//! Fixed caller-buffer prototypes shared by native-door consumers.

use std::ffi::c_void;

use libloading::Library;

use crate::abi::{MechanismError, mechanism_symbol_error as symbol_error};

/// One argument type admitted by the fixed pointer core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedPointerType {
    I32,
    U32,
    U64,
    Pointer,
}

/// One canonical argument for a fixed pointer prototype.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedPointerValue {
    I32(i32),
    U32(u32),
    U64(u64),
    Pointer(*mut c_void),
}

impl FixedPointerValue {
    pub const fn ty(self) -> FixedPointerType {
        match self {
            Self::I32(_) => FixedPointerType::I32,
            Self::U32(_) => FixedPointerType::U32,
            Self::U64(_) => FixedPointerType::U64,
            Self::Pointer(_) => FixedPointerType::Pointer,
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
    /// C `int function(void *, uint64_t)`, used by `getentropy`.
    I32PointerU64,
    /// C `int function(void *, void *)`, with both pointers required.
    I32PointerPointer,
    /// C `int function(void *, void *, void *)`, with all pointers required.
    I32PointerPointerPointer,
    /// C `int function(int, void *, unsigned int)`, used by `proc_pidpath`.
    I32I32PointerU32,
    /// C `int function(uint64_t, void *, uint64_t)`, used by `pthread_getname_np`.
    I32U64PointerU64,
}

impl FixedPointerPrototype {
    pub const fn parameters(self) -> &'static [FixedPointerType] {
        match self {
            Self::I32Pointer => &[FixedPointerType::Pointer],
            Self::I32I32Pointer => &[FixedPointerType::I32, FixedPointerType::Pointer],
            Self::I32PointerI32 => &[FixedPointerType::Pointer, FixedPointerType::I32],
            Self::I32PointerU64 => &[FixedPointerType::Pointer, FixedPointerType::U64],
            Self::I32PointerPointer => &[FixedPointerType::Pointer, FixedPointerType::Pointer],
            Self::I32PointerPointerPointer => &[
                FixedPointerType::Pointer,
                FixedPointerType::Pointer,
                FixedPointerType::Pointer,
            ],
            Self::I32I32PointerU32 => &[
                FixedPointerType::I32,
                FixedPointerType::Pointer,
                FixedPointerType::U32,
            ],
            Self::I32U64PointerU64 => &[
                FixedPointerType::U64,
                FixedPointerType::Pointer,
                FixedPointerType::U64,
            ],
        }
    }
}

/// A caller-asserted fixed pointer call and its canonical arguments.
///
/// The library is not a field: the caller opens it once and passes the handle.
pub struct FixedPointerCall<'a> {
    pub symbol: &'a str,
    pub prototype: FixedPointerPrototype,
    pub arguments: &'a [FixedPointerValue],
}

/// Validate a fixed pointer prototype before loading a library or resolving a symbol.
pub fn validate_fixed_pointer_signature(
    prototype: FixedPointerPrototype,
    arguments: &[FixedPointerValue],
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

/// Executes an admitted pointer-family shape against an already open library.
///
/// # Safety
///
/// The caller must uphold [`crate::invoke_abi`]'s complete ABI contract, and
/// `library` must be the library the call names: this entry opens nothing.
pub(crate) unsafe fn invoke_fixed_pointer_mechanism_with_library(
    library: &Library,
    call: &FixedPointerCall<'_>,
) -> Result<i32, MechanismError> {
    validate_fixed_pointer_signature(call.prototype, call.arguments)?;
    match (call.prototype, call.arguments) {
        (FixedPointerPrototype::I32Pointer, [FixedPointerValue::Pointer(a)]) => {
            invoke_i32_pointer(library, call.symbol, *a)
        }
        (
            FixedPointerPrototype::I32I32Pointer,
            [FixedPointerValue::I32(a), FixedPointerValue::Pointer(b)],
        ) => invoke_i32_i32_pointer(library, call.symbol, *a, *b),
        (
            FixedPointerPrototype::I32PointerI32,
            [FixedPointerValue::Pointer(a), FixedPointerValue::I32(b)],
        ) => invoke_i32_pointer_i32(library, call.symbol, *a, *b),
        (
            FixedPointerPrototype::I32PointerU64,
            [FixedPointerValue::Pointer(a), FixedPointerValue::U64(b)],
        ) => invoke_i32_pointer_u64(library, call.symbol, *a, *b),
        (
            FixedPointerPrototype::I32PointerPointer,
            [FixedPointerValue::Pointer(a), FixedPointerValue::Pointer(b)],
        ) => invoke_i32_pointer_pointer(library, call.symbol, *a, *b),
        (
            FixedPointerPrototype::I32PointerPointerPointer,
            [
                FixedPointerValue::Pointer(a),
                FixedPointerValue::Pointer(b),
                FixedPointerValue::Pointer(c),
            ],
        ) => invoke_i32_pointer_pointer_pointer(library, call.symbol, *a, *b, *c),
        (
            FixedPointerPrototype::I32I32PointerU32,
            [
                FixedPointerValue::I32(a),
                FixedPointerValue::Pointer(b),
                FixedPointerValue::U32(c),
            ],
        ) => invoke_i32_i32_pointer_u32(library, call.symbol, *a, *b, *c),
        (
            FixedPointerPrototype::I32U64PointerU64,
            [
                FixedPointerValue::U64(a),
                FixedPointerValue::Pointer(b),
                FixedPointerValue::U64(c),
            ],
        ) => invoke_i32_u64_pointer_u64(library, call.symbol, *a, *b, *c),
        _ => unreachable!("fixed pointer signature validation admitted the prototype"),
    }
}

fn invoke_i32_pointer_pointer(
    library: &Library,
    symbol: &str,
    a: *mut c_void,
    b: *mut c_void,
) -> Result<i32, MechanismError> {
    // SAFETY: see invoke_i32_pointer.
    let function = unsafe {
        library.get::<unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32>(symbol.as_bytes())
    }
    .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns both pointer contracts and the library stays live.
    Ok(unsafe { function(a, b) })
}

fn invoke_i32_pointer_u64(
    library: &Library,
    symbol: &str,
    a: *mut c_void,
    b: u64,
) -> Result<i32, MechanismError> {
    // SAFETY: see invoke_i32_pointer.
    let function =
        unsafe { library.get::<unsafe extern "C" fn(*mut c_void, u64) -> i32>(symbol.as_bytes()) }
            .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns the pointer contract and the library stays live.
    Ok(unsafe { function(a, b) })
}

fn invoke_i32_pointer_pointer_pointer(
    library: &Library,
    symbol: &str,
    a: *mut c_void,
    b: *mut c_void,
    c: *mut c_void,
) -> Result<i32, MechanismError> {
    // SAFETY: see invoke_i32_pointer.
    let function = unsafe {
        library.get::<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32>(
            symbol.as_bytes(),
        )
    }
    .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns all pointer contracts and the library stays live.
    Ok(unsafe { function(a, b, c) })
}

fn invoke_i32_pointer(
    library: &Library,
    symbol: &str,
    a: *mut c_void,
) -> Result<i32, MechanismError> {
    // SAFETY: invoke_abi admitted this exact prototype; the remaining
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
) -> Result<i32, MechanismError> {
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
) -> Result<i32, MechanismError> {
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
) -> Result<i32, MechanismError> {
    // SAFETY: see invoke_i32_pointer.
    let function = unsafe {
        library.get::<unsafe extern "C" fn(i32, *mut c_void, u32) -> i32>(symbol.as_bytes())
    }
    .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns the pointer contract and the library stays live.
    Ok(unsafe { function(a, b, c) })
}

fn invoke_i32_u64_pointer_u64(
    library: &Library,
    symbol: &str,
    a: u64,
    b: *mut c_void,
    c: u64,
) -> Result<i32, MechanismError> {
    // SAFETY: see invoke_i32_pointer.
    let function = unsafe {
        library.get::<unsafe extern "C" fn(u64, *mut c_void, u64) -> i32>(symbol.as_bytes())
    }
    .map_err(|error| symbol_error(symbol, error))?;
    // SAFETY: the caller owns the pointer contract and the library stays live.
    Ok(unsafe { function(a, b, c) })
}
