pub mod abi;
// Internal monomorphic trampoline families selected by `abi`.
mod exact_native;
#[cfg(unix)]
mod exec;
#[cfg(unix)]
mod exec_error;
mod fixed_native;
mod fixed_pointer;
mod unix_ioctl;

pub use abi::{
    AbiError, AbiSignature, AbiType, AbiValue, LibraryHandle, NativeCall, invoke_abi,
    invoke_abi_with_handle, validate_abi, validate_abi_signature,
};
#[cfg(unix)]
pub use exec::{
    BufferState, CodeBuffer, NameEntry, NameTable, aarch64_mov_x0_ret, x86_64_call_thunk,
    x86_64_mov_rax_ret,
};
#[cfg(unix)]
pub use exec_error::ExecError;
pub use unix_ioctl::{UnixIoctlError, UnixIoctlRequest, invoke_unix_ioctl};
