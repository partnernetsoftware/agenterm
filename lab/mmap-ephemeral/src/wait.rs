//! Wait / wake policies for the mailbox state word.
//!
//! | Kind | macOS | Linux | Windows |
//! |------|-------|-------|---------|
//! | `Yield` | spin+yield | spin+yield | spin+yield |
//! | `Native` | `os_sync_*_SHARED` | `futex` WAIT/WAKE | `WaitOnAddress` |

use crate::slot::Header;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitKind {
    /// Busy `thread::yield_now` until the state word matches.
    Yield,
    /// OS address-wait (cross-process shared mapping).
    ///
    /// - macOS: `os_sync_wait_on_address` / `os_sync_wake_by_address_*` (`*_SHARED`)
    /// - Linux: `futex(FUTEX_WAIT/WAKE)` (not PRIVATE)
    /// - Windows: `WaitOnAddress` / `WakeByAddressSingle`
    Native,
}

impl WaitKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "yield" => Some(Self::Yield),
            // aliases
            "native" | "os_sync" | "futex" | "wait_on_address" | "park" => Some(Self::Native),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yield => "yield",
            Self::Native => "native",
        }
    }
}

/// Probe the native wait backend on this OS.
pub fn probe_native() -> Result<&'static str, String> {
    #[cfg(target_os = "macos")]
    {
        probe_os_sync()?;
        Ok("os_sync_SHARED")
    }
    #[cfg(target_os = "linux")]
    {
        Ok("futex")
    }
    #[cfg(windows)]
    {
        Ok("WaitOnAddress")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Err("no native wait backend for this OS".into())
    }
}

/// Back-compat name used by the CLI / older docs.
pub fn probe_os_sync() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::ffi::{RTLD_DEFAULT, dlsym};
        let needed = [
            "os_sync_wait_on_address\0",
            "os_sync_wait_on_address_with_timeout\0",
            "os_sync_wake_by_address_any\0",
            "os_sync_wake_by_address_all\0",
        ];
        for sym in needed {
            // SAFETY: RTLD_DEFAULT + NUL-terminated literal.
            let p = unsafe { dlsym(RTLD_DEFAULT, sym.as_ptr() as *const std::os::raw::c_char) };
            if p.is_null() {
                return Err(format!(
                    "dlsym({}): NULL (API unavailable on this SDK/OS)",
                    &sym[..sym.len() - 1]
                ));
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        probe_native().map(|_| ())
    }
}

pub(crate) fn require_native(wait: WaitKind) -> io::Result<()> {
    if wait == WaitKind::Native {
        probe_native().map(|_| ()).map_err(io::Error::other)?;
    }
    Ok(())
}

fn state_addr(h: &Header) -> *mut AtomicU32 {
    (&h.state as *const AtomicU32).cast_mut()
}

pub(crate) fn wake_state(h: &Header, wait: WaitKind) {
    if wait != WaitKind::Native {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        use crate::ffi::{ENOENT, OS_SYNC_WAKE_BY_ADDRESS_SHARED, os_sync_wake_by_address_any};
        // SAFETY: state is 4-byte aligned; SHARED matches MAP_SHARED.
        let rc = unsafe {
            os_sync_wake_by_address_any(state_addr(h).cast(), 4, OS_SYNC_WAKE_BY_ADDRESS_SHARED)
        };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(ENOENT) {
                eprintln!("shmbox: wake warning: {err}");
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        use crate::ffi::futex_wake;
        // SAFETY: shared mapping u32; wake one waiter.
        let _ = unsafe { futex_wake(state_addr(h).cast(), 1) };
    }
    #[cfg(windows)]
    {
        use crate::ffi::WakeByAddressSingle;
        // SAFETY: shared mapping; address is the state word.
        unsafe { WakeByAddressSingle(state_addr(h).cast()) };
    }
}

pub(crate) fn wait_state(
    h: &Header,
    want: u32,
    timeout: Duration,
    wait: WaitKind,
) -> io::Result<()> {
    match wait {
        WaitKind::Yield => wait_state_yield(h, want, timeout),
        WaitKind::Native => wait_state_native(h, want, timeout),
    }
}

fn wait_state_yield(h: &Header, want: u32, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    loop {
        if h.state.load(Ordering::Acquire) == want {
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timeout waiting for state {want}"),
            ));
        }
        thread::yield_now();
    }
}

fn wait_state_native(h: &Header, want: u32, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    loop {
        let cur = h.state.load(Ordering::Acquire);
        if cur == want {
            return Ok(());
        }
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timeout waiting for state {want}"),
            ));
        }
        let remaining = timeout - elapsed;
        wait_once_native(h, cur, remaining)?;
    }
}

fn wait_once_native(h: &Header, cur: u32, remaining: Duration) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use crate::ffi::{
            EFAULT, EINTR, ENOMEM, ETIMEDOUT, OS_CLOCK_MACH_ABSOLUTE_TIME,
            OS_SYNC_WAIT_ON_ADDRESS_SHARED, os_sync_wait_on_address_with_timeout,
        };
        let timeout_ns = remaining.as_nanos().min(u64::MAX as u128) as u64;
        if timeout_ns == 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
        }
        // SAFETY: aligned AtomicU32 in shared mapping.
        let rc = unsafe {
            os_sync_wait_on_address_with_timeout(
                state_addr(h).cast(),
                u64::from(cur),
                4,
                OS_SYNC_WAIT_ON_ADDRESS_SHARED,
                OS_CLOCK_MACH_ABSOLUTE_TIME,
                timeout_ns,
            )
        };
        if rc < 0 {
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(EINTR) | Some(EFAULT) | Some(ENOMEM) => Ok(()),
                Some(ETIMEDOUT) => Err(io::Error::new(io::ErrorKind::TimedOut, "timeout")),
                _ => Err(err),
            }
        } else {
            Ok(())
        }
    }
    #[cfg(target_os = "linux")]
    {
        use crate::ffi::{EINTR, ETIMEDOUT, Timespec, futex_wait};
        let ts = Timespec {
            tv_sec: remaining.as_secs() as i64,
            tv_nsec: remaining.subsec_nanos() as i64,
        };
        // SAFETY: shared u32; FUTEX_WAIT (not PRIVATE).
        let rc = unsafe { futex_wait(state_addr(h).cast(), cur, &ts) };
        if rc == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(EINTR) | Some(crate::ffi::EFAULT) => Ok(()),
            // EAGAIN: value already changed
            Some(11) => Ok(()),
            Some(ETIMEDOUT) => Err(io::Error::new(io::ErrorKind::TimedOut, "timeout")),
            _ => Err(err),
        }
    }
    #[cfg(windows)]
    {
        use crate::ffi::WaitOnAddress;
        let ms = remaining.as_millis().min(u32::MAX as u128) as u32;
        if ms == 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
        }
        let compare = cur;
        // SAFETY: 4-byte wait on shared state; compare is stack local copy.
        let ok =
            unsafe { WaitOnAddress(state_addr(h).cast(), (&compare as *const u32).cast(), 4, ms) };
        if ok != 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        // ERROR_TIMEOUT = 1460
        if err.raw_os_error() == Some(1460) {
            Err(io::Error::new(io::ErrorKind::TimedOut, "timeout"))
        } else {
            // Spurious / value changed → retry outer loop.
            Ok(())
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = (h, cur, remaining);
        Err(io::Error::other("native wait unsupported on this OS"))
    }
}

/// Idle-loop wait used by resident servers (short slices so shutdown is polled).
pub(crate) fn wait_idle_slice(h: &Header, wait: WaitKind) -> io::Result<()> {
    match wait {
        WaitKind::Yield => {
            thread::yield_now();
            Ok(())
        }
        WaitKind::Native => {
            let cur = h.state.load(Ordering::Acquire);
            wait_once_native(h, cur, Duration::from_millis(50)).or_else(|e| {
                if e.kind() == io::ErrorKind::TimedOut {
                    Ok(())
                } else {
                    Err(e)
                }
            })
        }
    }
}
