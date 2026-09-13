//! Typed exact-homogeneous native invocation shared by native-door consumers.

use libloading::Library;

use crate::abi::{MechanismError, mechanism_symbol_error as symbol_error};

/// Maximum fixed arity supported by the exact native core.
pub const MAX_EXACT_NATIVE_ARITY: usize = 6;

/// One exact Rust scalar family with a corresponding `extern "C"` ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExactNativeType {
    I32,
    U32,
    I64,
    U64,
    Isize,
    Usize,
    F64,
}

/// One canonical argument or result in an exact scalar family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExactNativeValue {
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    Isize(isize),
    Usize(usize),
    F64(f64),
}

impl ExactNativeValue {
    pub const fn ty(self) -> ExactNativeType {
        match self {
            Self::I32(_) => ExactNativeType::I32,
            Self::U32(_) => ExactNativeType::U32,
            Self::I64(_) => ExactNativeType::I64,
            Self::U64(_) => ExactNativeType::U64,
            Self::Isize(_) => ExactNativeType::Isize,
            Self::Usize(_) => ExactNativeType::Usize,
            Self::F64(_) => ExactNativeType::F64,
        }
    }
}

/// A caller-asserted fixed native signature and its canonical arguments.
///
/// The library is not a field: the caller opens it once and passes the handle, so
/// a shape cannot silently name one library while executing in another.
pub struct ExactNativeCall<'a> {
    pub symbol: &'a str,
    pub result: ExactNativeType,
    pub arguments: &'a [ExactNativeValue],
}

/// Validate the exact homogeneous signature before argument conversion or loading.
pub fn validate_exact_native_signature(
    result: ExactNativeType,
    parameters: &[ExactNativeType],
) -> Result<(), MechanismError> {
    if parameters.len() <= MAX_EXACT_NATIVE_ARITY
        && parameters.iter().all(|parameter| *parameter == result)
    {
        Ok(())
    } else {
        Err(MechanismError::SignatureUnsupported)
    }
}

macro_rules! invoke_homogeneous {
    ($library:expr, $symbol:expr, $args:expr, $variant:ident, $ty:ty) => {{
        let values = $args
            .iter()
            .map(|value| match value {
                ExactNativeValue::$variant(value) => Some(*value),
                _ => None,
            })
            .collect::<Option<Vec<$ty>>>()
            .expect("signature validation checked every argument");
        match values.as_slice() {
            [] => invoke_0::<$ty>($library, $symbol),
            [a] => invoke_1::<$ty>($library, $symbol, *a),
            [a, b] => invoke_2::<$ty>($library, $symbol, *a, *b),
            [a, b, c] => invoke_3::<$ty>($library, $symbol, *a, *b, *c),
            [a, b, c, d] => invoke_4::<$ty>($library, $symbol, *a, *b, *c, *d),
            [a, b, c, d, e] => invoke_5::<$ty>($library, $symbol, *a, *b, *c, *d, *e),
            [a, b, c, d, e, f] => invoke_6::<$ty>($library, $symbol, *a, *b, *c, *d, *e, *f),
            _ => unreachable!("signature validation caps arity"),
        }
        .map(ExactNativeValue::$variant)
    }};
}

/// Executes an admitted exact-family shape against an already open library.
///
/// # Safety
///
/// The caller must uphold [`crate::invoke_abi`]'s complete ABI contract, and
/// `library` must be the library the call names: this entry does not open
/// anything, so a mismatched handle would resolve the symbol in the wrong image.
pub(crate) unsafe fn invoke_exact_mechanism_with_library(
    library: &Library,
    call: &ExactNativeCall<'_>,
) -> Result<ExactNativeValue, MechanismError> {
    let parameters = call
        .arguments
        .iter()
        .map(|argument| argument.ty())
        .collect::<Vec<_>>();
    validate_exact_native_signature(call.result, &parameters)?;
    let family = call.result;
    match family {
        ExactNativeType::I32 => {
            invoke_homogeneous!(library, call.symbol, call.arguments, I32, i32)
        }
        ExactNativeType::U32 => {
            invoke_homogeneous!(library, call.symbol, call.arguments, U32, u32)
        }
        ExactNativeType::I64 => {
            invoke_homogeneous!(library, call.symbol, call.arguments, I64, i64)
        }
        ExactNativeType::U64 => {
            invoke_homogeneous!(library, call.symbol, call.arguments, U64, u64)
        }
        ExactNativeType::Isize => {
            invoke_homogeneous!(library, call.symbol, call.arguments, Isize, isize)
        }
        ExactNativeType::Usize => {
            invoke_homogeneous!(library, call.symbol, call.arguments, Usize, usize)
        }
        ExactNativeType::F64 => {
            invoke_homogeneous!(library, call.symbol, call.arguments, F64, f64)
        }
    }
}

macro_rules! typed_invoker {
    ($name:ident, ($($arg:ident),*)) => {
        #[allow(clippy::too_many_arguments)]
        fn $name<T: Copy>(library: &Library, symbol: &str, $($arg: T),*) -> Result<T, MechanismError> {
            // SAFETY: invoke_abi admitted the exact homogeneous Rust type and arity;
            // the remaining symbol-signature assertion belongs to its unsafe caller.
            let function = unsafe { library.get::<unsafe extern "C" fn($($arg: T),*) -> T>(symbol.as_bytes()) }
                .map_err(|error| symbol_error(symbol, error))?;
            // SAFETY: arguments have the exact admitted type and the library stays live.
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

/// Test-only: how many times this process entered the one loader entry.
///
/// A delta instrument, not a product fact. It exists so the owning court can
/// prove the reuse claim the handle entry was added for: the one-shot entry loads
/// once per call, and one adopted handle loads once for any number of calls.
/// Incremented under `cfg(test)` only, so a release build has no counter, no
/// accessor and no bytes.
#[cfg(test)]
static LOADER_ENTRIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Test-only: the loader entry count, for a delta taken around one call sequence.
///
/// This counter is **process-global**, so a test may only assert a delta. One test
/// function below takes every delta it needs, in order: `cargo test` runs the
/// functions of one test binary in parallel, and a second test loading a library
/// at the same time would make any delta unreadable. Serialising the binary, or
/// trusting a global number, is not a fix for that — the fix is one delta owner.
#[cfg(test)]
fn loader_entries() -> usize {
    LOADER_ENTRIES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Opens one library through the single loader.
///
/// The empty name means the current process, exactly as it does for
/// `NativeCall::library`; both branches are one loader entry.
pub(crate) fn open_library(name: &str) -> Result<Library, libloading::Error> {
    #[cfg(test)]
    LOADER_ENTRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if name.is_empty() {
        return current_process_library();
    }
    // SAFETY: this unrestricted native operation deliberately runs library init/fini code.
    unsafe { Library::new(name) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::{
        AbiError, AbiSignature, AbiType, AbiValue, LibraryHandle, NativeCall, invoke_abi,
        invoke_abi_with_handle,
    };

    /// The library is this process and the symbol is absent: the load still
    /// happens (that is what is counted) and the lookup fails after it, so the
    /// addresses are never called and the exact shape can take no arguments.
    const NO_PARAMS: &[AbiType] = &[];
    const NO_ARGUMENTS: &[AbiValue] = &[];
    const ABSENT_SYMBOL: &str = "agenterm_dyn_absent_symbol_for_the_loader_count";
    /// Calls in each half of the sequence. The two halves assert a *relation*, so
    /// any count above one proves the claim.
    const CALLS: usize = 3;

    fn absent_call() -> NativeCall<'static> {
        NativeCall {
            library: "",
            symbol: ABSENT_SYMBOL,
            signature: AbiSignature {
                result: AbiType::I32,
                params: NO_PARAMS,
            },
            arguments: NO_ARGUMENTS,
        }
    }

    /// One adopted handle enters the loader once; the one-shot entry enters it per
    /// call. Both deltas are taken inside this one function — see
    /// [`loader_entries`].
    #[test]
    fn one_adopted_handle_enters_the_loader_once_while_the_one_shot_entry_enters_it_per_call() {
        // Half one: the one-shot entry loads, then resolves the absent symbol, so
        // every call is one loader entry and one `SymbolLookup`.
        let before = loader_entries();
        for _ in 0..CALLS {
            let error = unsafe { invoke_abi(&absent_call()) }
                .expect_err("an absent symbol is refused after loading");
            assert!(
                matches!(error, AbiError::SymbolLookup { .. }),
                "expected SymbolLookup, got {error:?}"
            );
        }
        assert_eq!(
            loader_entries() - before,
            CALLS,
            "the one-shot entry must load once per call"
        );

        // Half two: one open, then the same load serves every call. A handle that
        // re-opened the library per call would grow the delta by CALLS here.
        let before = loader_entries();
        let handle = LibraryHandle::open("").expect("this process is loadable");
        assert_eq!(
            loader_entries() - before,
            1,
            "opening the handle is exactly one load"
        );
        let after_open = loader_entries();
        for _ in 0..CALLS {
            let error = unsafe { invoke_abi_with_handle(&handle, &absent_call()) }
                .expect_err("an absent symbol is refused through the handle");
            assert!(
                matches!(error, AbiError::SymbolLookup { .. }),
                "expected SymbolLookup, got {error:?}"
            );
        }
        assert_eq!(
            loader_entries() - after_open,
            0,
            "repeated calls through one handle must add no load"
        );
    }
}
