//! Typed exact-homogeneous native invocation shared by native-door consumers.

use std::fmt;

use libloading::Library;

/// Maximum fixed arity supported by the exact native core.
pub const MAX_EXACT_NATIVE_ARITY: usize = 6;

const EXACT_NATIVE_FAMILIES: [ExactNativeType; 7] = [
    ExactNativeType::I32,
    ExactNativeType::U32,
    ExactNativeType::I64,
    ExactNativeType::U64,
    ExactNativeType::Isize,
    ExactNativeType::Usize,
    ExactNativeType::F64,
];

/// Number of exact family/arity stubs owned by this core.
pub const fn exact_native_stub_cardinality() -> usize {
    EXACT_NATIVE_FAMILIES.len() * (MAX_EXACT_NATIVE_ARITY + 1)
}

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
pub struct ExactNativeCall<'a> {
    pub library: &'a str,
    pub symbol: &'a str,
    pub result: ExactNativeType,
    pub arguments: &'a [ExactNativeValue],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExactNativeError {
    SignatureUnsupported {
        result: ExactNativeType,
        parameters: Vec<ExactNativeType>,
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

impl fmt::Display for ExactNativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignatureUnsupported { result, parameters } => write!(
                f,
                "invocation does not have one exact homogeneous scalar type: {result:?}({parameters:?})"
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

impl std::error::Error for ExactNativeError {}

/// Validate the exact homogeneous signature before argument conversion or loading.
pub fn validate_exact_native_signature(
    result: ExactNativeType,
    parameters: &[ExactNativeType],
) -> Result<(), ExactNativeError> {
    if parameters.len() <= MAX_EXACT_NATIVE_ARITY
        && parameters.iter().all(|parameter| *parameter == result)
    {
        Ok(())
    } else {
        Err(ExactNativeError::SignatureUnsupported {
            result,
            parameters: parameters.to_vec(),
        })
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

/// Resolve and invoke one exact homogeneous native call.
///
/// # Safety
/// The caller asserts that `symbol` really has the declared fixed, non-variadic
/// C ABI signature. Native initializers, finalizers, and the function itself may
/// have arbitrary process effects.
pub unsafe fn invoke_exact(
    call: &ExactNativeCall<'_>,
) -> Result<ExactNativeValue, ExactNativeError> {
    let parameters = call
        .arguments
        .iter()
        .map(|argument| argument.ty())
        .collect::<Vec<_>>();
    validate_exact_native_signature(call.result, &parameters)?;
    let family = call.result;
    let library = open_library(call.library)?;
    match family {
        ExactNativeType::I32 => {
            invoke_homogeneous!(&library, call.symbol, call.arguments, I32, i32)
        }
        ExactNativeType::U32 => {
            invoke_homogeneous!(&library, call.symbol, call.arguments, U32, u32)
        }
        ExactNativeType::I64 => {
            invoke_homogeneous!(&library, call.symbol, call.arguments, I64, i64)
        }
        ExactNativeType::U64 => {
            invoke_homogeneous!(&library, call.symbol, call.arguments, U64, u64)
        }
        ExactNativeType::Isize => {
            invoke_homogeneous!(&library, call.symbol, call.arguments, Isize, isize)
        }
        ExactNativeType::Usize => {
            invoke_homogeneous!(&library, call.symbol, call.arguments, Usize, usize)
        }
        ExactNativeType::F64 => {
            invoke_homogeneous!(&library, call.symbol, call.arguments, F64, f64)
        }
    }
}

fn symbol_error(symbol: &str, error: libloading::Error) -> ExactNativeError {
    ExactNativeError::SymbolLoad {
        symbol: symbol.to_owned(),
        message: error.to_string(),
    }
}

macro_rules! typed_invoker {
    ($name:ident, ($($arg:ident),*)) => {
        #[allow(clippy::too_many_arguments)]
        fn $name<T: Copy>(library: &Library, symbol: &str, $($arg: T),*) -> Result<T, ExactNativeError> {
            // SAFETY: invoke_exact admitted the exact homogeneous Rust type and arity;
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

fn open_library(name: &str) -> Result<Library, ExactNativeError> {
    if name.is_empty() {
        return current_process_library().map_err(|error| ExactNativeError::LibraryLoad {
            library: "<current-process>".to_owned(),
            message: error.to_string(),
        });
    }
    // SAFETY: this unrestricted native operation deliberately runs library init/fini code.
    unsafe { Library::new(name) }.map_err(|error| ExactNativeError::LibraryLoad {
        library: name.to_owned(),
        message: error.to_string(),
    })
}
