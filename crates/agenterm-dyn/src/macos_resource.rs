//! Typed ownership for Mach resources that cannot safely cross the generic dlcall door.

use std::fmt;

/// Failure to acquire or inspect the process's owned host send-right reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachHostPortError {
    /// This resource exists only on Darwin hosts.
    Unsupported,
    /// `mach_host_self` unexpectedly returned `MACH_PORT_NULL`.
    NullPort,
    /// A Mach inspection operation returned this kernel status.
    Kernel(i32),
}

impl fmt::Display for MachHostPortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("mach_host_self is unsupported on this host"),
            Self::NullPort => formatter.write_str("mach_host_self returned MACH_PORT_NULL"),
            Self::Kernel(status) => write!(formatter, "Mach operation failed with status {status}"),
        }
    }
}

impl std::error::Error for MachHostPortError {}

#[cfg(any(target_os = "macos", test))]
trait ReleasePort {
    fn release(&self, port: u32);
}

#[cfg(any(target_os = "macos", test))]
struct OwnedPort<R: ReleasePort> {
    port: u32,
    releaser: R,
}

#[cfg(any(target_os = "macos", test))]
impl<R: ReleasePort> Drop for OwnedPort<R> {
    fn drop(&mut self) {
        self.releaser.release(self.port);
    }
}

#[cfg(target_os = "macos")]
struct SystemReleaser;

#[cfg(target_os = "macos")]
impl ReleasePort for SystemReleaser {
    fn release(&self, port: u32) {
        // SAFETY: this owner was created by one successful `mach_host_self`
        // acquisition, and Drop consumes exactly that one send-right user ref.
        unsafe {
            let _ = mach_port_deallocate(mach_task_self_, port);
        }
    }
}

/// One owned send-right reference returned by Darwin's `mach_host_self`.
///
/// Dropping this value releases exactly this reference. It does not destroy or
/// close the host represented by the process-wide Mach port name. The raw port
/// name is intentionally not exposed.
#[cfg(target_os = "macos")]
pub struct MachHostPort {
    owned: OwnedPort<SystemReleaser>,
}

#[cfg(not(target_os = "macos"))]
pub struct MachHostPort;

impl MachHostPort {
    /// Acquire one owned host-port send-right reference.
    #[cfg(target_os = "macos")]
    pub fn acquire() -> Result<Self, MachHostPortError> {
        // SAFETY: `mach_host_self` takes no arguments and returns one owned send ref.
        let port = unsafe { mach_host_self() };
        if port == 0 {
            return Err(MachHostPortError::NullPort);
        }
        Ok(Self {
            owned: OwnedPort {
                port,
                releaser: SystemReleaser,
            },
        })
    }

    /// Report the current user-reference count for this send right.
    #[cfg(target_os = "macos")]
    pub fn send_right_refs(&self) -> Result<u32, MachHostPortError> {
        let mut refs = 0;
        // SAFETY: the owner keeps `port` live and `refs` is valid writable storage.
        let status = unsafe {
            mach_port_get_refs(
                mach_task_self_,
                self.owned.port,
                MACH_PORT_RIGHT_SEND,
                &mut refs,
            )
        };
        if status == KERN_SUCCESS {
            Ok(refs)
        } else {
            Err(MachHostPortError::Kernel(status))
        }
    }

    /// Return an honest typed failure on non-Darwin hosts.
    #[cfg(not(target_os = "macos"))]
    pub fn acquire() -> Result<Self, MachHostPortError> {
        Err(MachHostPortError::Unsupported)
    }
}

#[cfg(target_os = "macos")]
const KERN_SUCCESS: i32 = 0;
#[cfg(target_os = "macos")]
const MACH_PORT_RIGHT_SEND: u32 = 0;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    static mach_task_self_: u32;
    fn mach_host_self() -> u32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
    fn mach_port_get_refs(task: u32, name: u32, right: u32, refs: *mut u32) -> i32;
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::{OwnedPort, ReleasePort};

    struct CountingReleaser(Rc<Cell<u32>>);

    impl ReleasePort for CountingReleaser {
        fn release(&self, _port: u32) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn owned_port_drop_calls_release_exactly_once() {
        let calls = Rc::new(Cell::new(0));
        let owner = OwnedPort {
            port: 74,
            releaser: CountingReleaser(Rc::clone(&calls)),
        };
        assert_eq!(calls.get(), 0);
        drop(owner);
        assert_eq!(calls.get(), 1);
    }
}
