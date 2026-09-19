//! Linux futex wait/wake on a shared u32 (cross-process: no FUTEX_PRIVATE).

use std::os::raw::{c_int, c_long, c_void};

pub const ETIMEDOUT: i32 = 110;

pub const FUTEX_WAIT: c_int = 0;
pub const FUTEX_WAKE: c_int = 1;

#[repr(C)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[cfg(target_arch = "aarch64")]
pub const SYS_FUTEX: c_long = 98;
#[cfg(target_arch = "x86_64")]
pub const SYS_FUTEX: c_long = 202;

#[link(name = "c")]
unsafe extern "C" {
    pub fn syscall(num: c_long, ...) -> c_long;
}

#[inline]
pub unsafe fn futex_wait(addr: *mut u32, expected: u32, timeout: *const Timespec) -> c_long {
    // FUTEX_WAIT: sleep while *addr == expected (shared/process-shared).
    unsafe {
        syscall(
            SYS_FUTEX,
            addr as *mut c_void,
            FUTEX_WAIT,
            expected,
            timeout as *const c_void,
            std::ptr::null_mut::<c_void>(),
            0,
        )
    }
}

#[inline]
pub unsafe fn futex_wake(addr: *mut u32, n: u32) -> c_long {
    unsafe {
        syscall(
            SYS_FUTEX,
            addr as *mut c_void,
            FUTEX_WAKE,
            n,
            std::ptr::null_mut::<c_void>(),
            std::ptr::null_mut::<c_void>(),
            0,
        )
    }
}
