//! Unix mmap / shm_open (Linux + Darwin). Open flags are OS-specific.

use std::os::raw::{c_char, c_int};

pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const MAP_SHARED: c_int = 1;

pub const O_RDWR: c_int = 0x0002;

#[cfg(target_os = "macos")]
pub const O_CREAT: c_int = 0x0200;
#[cfg(target_os = "macos")]
pub const O_EXCL: c_int = 0x0800;
#[cfg(target_os = "macos")]
pub const O_TRUNC: c_int = 0x0400;

#[cfg(target_os = "linux")]
pub const O_CREAT: c_int = 0x40;
#[cfg(target_os = "linux")]
pub const O_EXCL: c_int = 0x80;
#[cfg(target_os = "linux")]
pub const O_TRUNC: c_int = 0x200;

pub const EINTR: i32 = 4;
pub const EFAULT: i32 = 14;
pub const ENOMEM: i32 = 12;
pub const ENOENT: i32 = 2;

#[link(name = "c")]
unsafe extern "C" {
    pub fn mmap(
        addr: *mut u8,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: isize,
    ) -> *mut u8;
    pub fn munmap(addr: *mut u8, len: usize) -> c_int;
    pub fn shm_open(name: *const c_char, oflag: c_int, mode: u16) -> c_int;
    pub fn shm_unlink(name: *const c_char) -> c_int;
    pub fn ftruncate(fd: c_int, length: i64) -> c_int;
    pub fn close(fd: c_int) -> c_int;
    pub fn kill(pid: i32, sig: i32) -> c_int;
}

#[cfg(target_os = "macos")]
#[link(name = "c")]
unsafe extern "C" {
    pub fn dlsym(handle: *mut core::ffi::c_void, symbol: *const c_char) -> *mut core::ffi::c_void;
}

#[cfg(target_os = "macos")]
pub const PROC_PIDTBSDINFO: c_int = 3;

// `proc_pidinfo` lives in libproc (part of libSystem).
#[cfg(target_os = "macos")]
#[link(name = "proc")]
unsafe extern "C" {
    pub fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut u8,
        buffersize: c_int,
    ) -> c_int;
}
