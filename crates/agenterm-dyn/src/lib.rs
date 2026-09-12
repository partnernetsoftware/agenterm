pub mod abi;
mod exact_native;
#[cfg(unix)]
mod exec;
#[cfg(unix)]
mod exec_error;
mod fixed_native;
mod fixed_pointer;
mod hosts;
mod macos_resource;
mod unix_groups;
mod unix_ioctl;
mod unix_path;
mod unix_resource;

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
pub use hosts::{
    ALL_CELLS, CU_ADJACENT_PROBE_CATALOG, CuAdjacentProbeCell, HostArch, HostCell, HostOs,
    LAYER3_CANDIDATES, LINUX_AARCH64, LINUX_ATSPI_EXISTENCE_LIBS, LINUX_X86_64, MACOS_AARCH64,
    MACOS_X86_64, PLATFORM_CANDIDATES, ProbeFact, SecondaryProbe, SizeProbe, SystemProbe,
    SystemProbeStatus, WINDOWS_AARCH64, WINDOWS_X86_64, cell, cu_adjacent_probe, live_cell,
};
pub use macos_resource::{
    CpuCountError, CpuCountSnapshot, DlAddressError, DlAddressSnapshot, DomainNameError,
    DomainNameSnapshot, LoginNameError, LoginNameSnapshot, MAX_DOMAIN_NAME_BYTES,
    MAX_LOGIN_NAME_BYTES, MachHostPort, MachHostPortError, MachTimebaseError, MachTimebaseSnapshot,
};
pub use unix_groups::{MAX_SUPPLEMENTARY_GROUPS, SupplementaryGroups, SupplementaryGroupsError};
pub use unix_ioctl::{UnixIoctlError, UnixIoctlRequest, invoke_unix_ioctl};
pub use unix_path::{MAX_REALPATH_BYTES, RealPathError, ResolvedPath};
pub use unix_resource::{
    ClockId, ClockSnapshot, ClockSnapshotError, HostnameError, HostnameSnapshot, InterfaceAddress,
    InterfaceAddresses, InterfaceAddressesError, MAX_HOSTNAME_BYTES, StatVfsError, StatVfsSnapshot,
};
