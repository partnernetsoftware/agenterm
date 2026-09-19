//! Darwin address-wait (os_sync_* SHARED).

use std::os::raw::{c_int, c_void};

pub const OS_SYNC_WAIT_ON_ADDRESS_SHARED: u32 = 0x0000_0001;
pub const OS_SYNC_WAKE_BY_ADDRESS_SHARED: u32 = 0x0000_0001;
pub const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;
pub const ETIMEDOUT: i32 = 60;

/// RTLD_DEFAULT on Darwin.
pub const RTLD_DEFAULT: *mut c_void = (-2isize) as *mut c_void;

#[link(name = "c")]
unsafe extern "C" {
    pub fn os_sync_wait_on_address_with_timeout(
        addr: *mut c_void,
        value: u64,
        size: usize,
        flags: u32,
        clockid: u32,
        timeout_ns: u64,
    ) -> c_int;
    pub fn os_sync_wake_by_address_any(addr: *mut c_void, size: usize, flags: u32) -> c_int;
}
