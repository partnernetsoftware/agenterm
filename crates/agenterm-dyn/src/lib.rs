pub mod abi;
mod exact_native;
#[cfg(unix)]
mod exec;
#[cfg(unix)]
mod exec_error;
mod fixed_native;
mod fixed_pointer;
mod unix_ioctl;

pub use abi::{AbiError, AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi, validate_abi};
pub use exact_native::{
    ExactNativeCall, ExactNativeError, ExactNativeType, ExactNativeValue, MAX_EXACT_NATIVE_ARITY,
    exact_native_stub_cardinality, invoke_exact, validate_exact_native_signature,
};
#[cfg(unix)]
pub use exec::{
    BufferState, CodeBuffer, NameEntry, NameTable, aarch64_mov_x0_ret, x86_64_call_thunk,
    x86_64_mov_rax_ret,
};
#[cfg(unix)]
pub use exec_error::ExecError;
pub use fixed_native::{
    FixedNativeCall, FixedNativeError, FixedNativePrototype, FixedNativeType, FixedNativeValue,
    invoke_fixed, validate_fixed_native_signature,
};
pub use fixed_pointer::{
    FixedPointerCall, FixedPointerError, FixedPointerPrototype, FixedPointerType,
    FixedPointerValue, invoke_fixed_pointer, validate_fixed_pointer_signature,
};
pub use unix_ioctl::{UnixIoctlError, UnixIoctlRequest, invoke_unix_ioctl};
