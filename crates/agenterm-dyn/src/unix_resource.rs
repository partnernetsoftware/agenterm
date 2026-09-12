//! Typed ownership for the linked list returned by Unix `getifaddrs`.

use std::fmt;

/// Failure to acquire or safely snapshot the local interface list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceAddressesError {
    /// This resource is available only on Unix hosts.
    Unsupported,
    /// `getifaddrs` failed with the captured OS error code.
    Os(i32),
    /// The native list exceeded the public traversal bound.
    TooManyEntries { limit: usize },
}

impl fmt::Display for InterfaceAddressesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("getifaddrs is unsupported on this host"),
            Self::Os(code) => write!(formatter, "getifaddrs failed with OS error {code}"),
            Self::TooManyEntries { limit } => {
                write!(formatter, "getifaddrs returned more than {limit} entries")
            }
        }
    }
}

impl std::error::Error for InterfaceAddressesError {}

/// One pointer-free interface fact copied from the native list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceAddress {
    /// Interface name as native bytes, excluding its trailing NUL.
    pub name: Vec<u8>,
    /// Native interface flags.
    pub flags: u32,
    /// Address family, or `None` when this entry has no address.
    pub address_family: Option<u16>,
}

#[cfg(any(unix, test))]
trait FreeList<T> {
    fn free(&self, head: *mut T);
}

#[cfg(any(unix, test))]
struct OwnedList<T, F: FreeList<T>> {
    head: *mut T,
    freer: F,
}

#[cfg(any(unix, test))]
impl<T, F: FreeList<T>> Drop for OwnedList<T, F> {
    fn drop(&mut self) {
        self.freer.free(self.head);
    }
}

#[cfg(unix)]
struct SystemFreer;

#[cfg(unix)]
impl FreeList<libc::ifaddrs> for SystemFreer {
    fn free(&self, head: *mut libc::ifaddrs) {
        // SAFETY: `head` came from one successful `getifaddrs` call and this
        // private owner invokes its matching release exactly once.
        unsafe { raw::freeifaddrs(head) };
    }
}

/// One owned Unix interface-address list.
///
/// Its native pointers remain private. [`Self::snapshot`] copies bounded,
/// pointer-free facts, and dropping the owner calls `freeifaddrs` exactly once.
#[cfg(unix)]
pub struct InterfaceAddresses {
    owned: OwnedList<libc::ifaddrs, SystemFreer>,
}

#[cfg(not(unix))]
pub struct InterfaceAddresses;

impl InterfaceAddresses {
    /// Acquire the current Unix interface-address list.
    #[cfg(unix)]
    pub fn acquire() -> Result<Self, InterfaceAddressesError> {
        let mut head = std::ptr::null_mut();
        // SAFETY: `head` is writable storage for the one native output pointer.
        let status = unsafe { raw::getifaddrs(&mut head) };
        if status != 0 {
            return Err(InterfaceAddressesError::Os(
                std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(status),
            ));
        }
        Ok(Self {
            owned: OwnedList {
                head,
                freer: SystemFreer,
            },
        })
    }

    /// Copy interface names, flags, and address families out of the native list.
    #[cfg(unix)]
    pub fn snapshot(&self) -> Result<Vec<InterfaceAddress>, InterfaceAddressesError> {
        const MAX_ENTRIES: usize = 4_096;
        let mut facts = Vec::new();
        let mut cursor = self.owned.head;
        while !cursor.is_null() {
            if facts.len() == MAX_ENTRIES {
                return Err(InterfaceAddressesError::TooManyEntries { limit: MAX_ENTRIES });
            }
            // SAFETY: the owner keeps the native list alive; getifaddrs promises
            // valid nodes and NUL-terminated names until freeifaddrs is called.
            let entry = unsafe { &*cursor };
            let name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }
                .to_bytes()
                .to_vec();
            let address_family = if entry.ifa_addr.is_null() {
                None
            } else {
                // SAFETY: a non-null ifa_addr points to a sockaddr for this node.
                Some(unsafe { (*entry.ifa_addr).sa_family as u16 })
            };
            facts.push(InterfaceAddress {
                name,
                flags: entry.ifa_flags,
                address_family,
            });
            cursor = entry.ifa_next;
        }
        Ok(facts)
    }

    /// Return an honest typed failure on non-Unix hosts.
    #[cfg(not(unix))]
    pub fn acquire() -> Result<Self, InterfaceAddressesError> {
        Err(InterfaceAddressesError::Unsupported)
    }
}

#[cfg(unix)]
mod raw {
    unsafe extern "C" {
        pub(super) fn getifaddrs(list: *mut *mut libc::ifaddrs) -> libc::c_int;
        pub(super) fn freeifaddrs(list: *mut libc::ifaddrs);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::{FreeList, OwnedList};

    struct CountingFreer(Rc<Cell<u32>>);

    impl FreeList<u8> for CountingFreer {
        fn free(&self, _head: *mut u8) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn owned_list_drop_calls_free_exactly_once() {
        let calls = Rc::new(Cell::new(0));
        let owner = OwnedList {
            head: std::ptr::dangling_mut::<u8>(),
            freer: CountingFreer(Rc::clone(&calls)),
        };
        assert_eq!(calls.get(), 0);
        drop(owner);
        assert_eq!(calls.get(), 1);
    }
}
