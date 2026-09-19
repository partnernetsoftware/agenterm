//! Windows file mapping + WaitOnAddress / WakeByAddressSingle.

use std::os::raw::{c_int, c_void};

pub type BOOL = c_int;
pub type DWORD = u32;
pub type HANDLE = *mut c_void;

pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
pub const PAGE_READWRITE: DWORD = 0x04;
pub const FILE_MAP_ALL_ACCESS: DWORD = 0x000F_001F;
pub const GENERIC_READ: DWORD = 0x8000_0000;
pub const GENERIC_WRITE: DWORD = 0x4000_0000;
pub const CREATE_ALWAYS: DWORD = 2;
pub const OPEN_ALWAYS: DWORD = 4;
pub const OPEN_EXISTING: DWORD = 3;
pub const FILE_ATTRIBUTE_NORMAL: DWORD = 0x80;
pub const INFINITE: DWORD = 0xFFFF_FFFF;
pub const PROCESS_QUERY_LIMITED_INFORMATION: DWORD = 0x1000;

#[link(name = "kernel32")]
#[link(name = "synchronization")]
unsafe extern "system" {
    pub fn CreateFileW(
        name: *const u16,
        access: DWORD,
        share: DWORD,
        sa: *mut c_void,
        disposition: DWORD,
        flags: DWORD,
        template: HANDLE,
    ) -> HANDLE;
    pub fn CreateFileMappingW(
        file: HANDLE,
        sa: *mut c_void,
        protect: DWORD,
        max_high: DWORD,
        max_low: DWORD,
        name: *const u16,
    ) -> HANDLE;
    pub fn MapViewOfFile(
        mapping: HANDLE,
        access: DWORD,
        off_high: DWORD,
        off_low: DWORD,
        bytes: usize,
    ) -> *mut c_void;
    pub fn UnmapViewOfFile(base: *const c_void) -> BOOL;
    pub fn CloseHandle(h: HANDLE) -> BOOL;
    pub fn GetLastError() -> DWORD;
    pub fn OpenProcess(access: DWORD, inherit: BOOL, pid: DWORD) -> HANDLE;
    pub fn WaitOnAddress(
        addr: *const c_void,
        compare: *const c_void,
        size: usize,
        timeout_ms: DWORD,
    ) -> BOOL;
    pub fn WakeByAddressSingle(addr: *mut c_void);
    pub fn WakeByAddressAll(addr: *mut c_void);
}
