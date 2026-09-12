//! Narrow typed ownership of Unix's variadic `ioctl` ABI.

use std::ffi::c_void;
use std::fmt;

/// The two request representations admitted by the former `dlcall` special case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnixIoctlRequest {
    /// Preserve the low 32 bits of a signed script value and zero-extend them.
    I32Bits(i32),
    /// Pass an already canonical unsigned 64-bit request.
    U64(u64),
}

impl UnixIoctlRequest {
    /// Return the request bits in Unix's `unsigned long` value domain.
    pub const fn as_u64(self) -> u64 {
        match self {
            Self::I32Bits(value) => value as u32 as u64,
            Self::U64(value) => value,
        }
    }
}

/// A target-level failure before the native call can begin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnixIoctlError {
    /// This target does not expose the Unix variadic ABI.
    Unsupported,
}

impl fmt::Display for UnixIoctlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("Unix ioctl is unsupported on this target"),
        }
    }
}

impl std::error::Error for UnixIoctlError {}

/// Invoke Unix `ioctl(int, unsigned long, ...)` with one pointer argument.
///
/// The returned `i32` is the native result, including `-1`; this boundary does
/// not consume or reinterpret thread-local `errno`.
///
/// # Safety
/// The caller must ensure that `argument` is valid for the request's native
/// pointee type, size, alignment, mutability, aliasing, and call duration. The
/// file descriptor and request must describe that same operation. This call
/// borrows the pointer only for the duration of `ioctl` and acquires no resource
/// requiring cleanup.
#[cfg(unix)]
pub unsafe fn invoke_unix_ioctl(
    fd: i32,
    request: UnixIoctlRequest,
    argument: *mut c_void,
) -> Result<i32, UnixIoctlError> {
    type IoctlFn = unsafe extern "C" fn(i32, libc::c_ulong, ...) -> i32;
    let ioctl: IoctlFn = libc::ioctl;
    let request = request.as_u64() as libc::c_ulong;
    // SAFETY: the public contract assigns the pointee and descriptor
    // obligations to the caller; this declaration preserves the unnamed third
    // argument required by Unix, including Apple arm64's variadic convention.
    Ok(unsafe { ioctl(fd, request, argument) })
}

/// Return the target's typed unsupported result without inspecting the pointer.
///
/// # Safety
/// This target performs no native call and never dereferences `argument`.
#[cfg(not(unix))]
pub unsafe fn invoke_unix_ioctl(
    _fd: i32,
    _request: UnixIoctlRequest,
    _argument: *mut c_void,
) -> Result<i32, UnixIoctlError> {
    Err(UnixIoctlError::Unsupported)
}
