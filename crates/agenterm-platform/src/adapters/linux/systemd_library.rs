//! Process-local ownership of one dynamically loaded `libsystemd` image.
//!
//! Product adapters retain exact symbol names, prototypes and error meaning.
//! This module owns only the repeated Linux loader, lookup and close mechanism.

use std::ffi::{CStr, c_char, c_int, c_void};

#[cfg(test)]
std::thread_local! {
    static CLOSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

const LIBSYSTEMD_SONAME: &CStr = c"libsystemd.so.0";
const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 0;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

pub(crate) struct SystemdLibrary {
    handle: *mut c_void,
}

impl SystemdLibrary {
    pub(crate) fn open() -> Option<Self> {
        // SAFETY: the fixed SONAME is NUL-terminated and the returned handle is
        // retained until this owner is dropped.
        let handle = unsafe { dlopen(LIBSYSTEMD_SONAME.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if handle.is_null() {
            None
        } else {
            Some(Self { handle })
        }
    }

    /// Resolves one adapter-owned symbol and copies its typed function pointer.
    ///
    /// # Safety
    ///
    /// `T` must be the exact C function-pointer type of `symbol`. The returned
    /// pointer must not escape the object that also retains this library owner.
    pub(crate) unsafe fn symbol<T: Copy>(&self, symbol: &CStr) -> Option<T> {
        // SAFETY: self owns a live handle and symbol is NUL-terminated.
        let pointer = unsafe { dlsym(self.handle, symbol.as_ptr()) };
        if pointer.is_null() {
            return None;
        }
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
        // SAFETY: the caller supplies the exact function-pointer type and keeps
        // the returned pointer inside an object that retains this owner.
        Some(unsafe { std::mem::transmute_copy(&pointer) })
    }

    #[cfg(test)]
    pub(crate) fn close_count() -> usize {
        CLOSES.get()
    }
}

impl Drop for SystemdLibrary {
    fn drop(&mut self) {
        // SAFETY: this is the unique owner of the successful dlopen handle.
        unsafe { dlclose(self.handle) };
        #[cfg(test)]
        CLOSES.set(CLOSES.get() + 1);
    }
}
