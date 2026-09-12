pub mod abi;
// Internal monomorphic trampoline families. Their legacy compatibility types
// remain implementation details until the tables are folded into `abi`.
#[allow(dead_code)]
mod exact_native;
#[cfg(unix)]
mod exec;
#[cfg(unix)]
mod exec_error;
#[allow(dead_code)]
mod fixed_native;
#[allow(dead_code)]
mod fixed_pointer;
mod unix_ioctl;

pub use abi::{AbiError, AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi, validate_abi};
pub use exact_native::exact_native_stub_cardinality;
#[cfg(unix)]
pub use exec::{
    BufferState, CodeBuffer, NameEntry, NameTable, aarch64_mov_x0_ret, x86_64_call_thunk,
    x86_64_mov_rax_ret,
};
#[cfg(unix)]
pub use exec_error::ExecError;
pub use unix_ioctl::{UnixIoctlError, UnixIoctlRequest, invoke_unix_ioctl};
