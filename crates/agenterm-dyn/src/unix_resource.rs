//! Typed, pointer-free snapshots of Unix native resources.

use std::fmt;
use std::path::Path;

/// One clock whose native identifier is selected inside dyn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockId {
    /// Wall-clock time since the Unix epoch.
    Realtime,
    /// Monotonic elapsed time, with an unspecified origin.
    Monotonic,
}

/// Failure to acquire a pointer-free clock snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSnapshotError {
    /// Native clocks are available only on Unix hosts.
    Unsupported,
    /// `clock_gettime` failed with the captured OS error code.
    Os(i32),
    /// The native result violated the required `[0, 1_000_000_000)` range.
    InvalidNanoseconds(i64),
}

impl fmt::Display for ClockSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("clock_gettime is unsupported on this host"),
            Self::Os(code) => write!(formatter, "clock_gettime failed with OS error {code}"),
            Self::InvalidNanoseconds(value) => {
                write!(
                    formatter,
                    "clock_gettime returned invalid nanoseconds {value}"
                )
            }
        }
    }
}

impl std::error::Error for ClockSnapshotError {}

/// An owned, pointer-free native clock reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClockSnapshot {
    pub seconds: i64,
    pub nanoseconds: u32,
}

impl ClockSnapshot {
    /// Read one controlled native clock and immediately copy its output.
    #[cfg(unix)]
    pub fn acquire(clock: ClockId) -> Result<Self, ClockSnapshotError> {
        let native_id = match clock {
            ClockId::Realtime => libc::CLOCK_REALTIME,
            ClockId::Monotonic => libc::CLOCK_MONOTONIC,
        };
        let mut native = std::mem::MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: `native` is writable storage for one timespec. The controlled
        // identifier comes from libc, and the value is read only after success.
        let status = unsafe { libc::clock_gettime(native_id, native.as_mut_ptr()) };
        if status != 0 {
            return Err(ClockSnapshotError::Os(
                std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(status),
            ));
        }
        // SAFETY: a zero status means clock_gettime initialized the complete value.
        snapshot_timespec(unsafe { native.assume_init() })
    }

    /// Return an honest typed failure on non-Unix hosts.
    #[cfg(not(unix))]
    pub fn acquire(_clock: ClockId) -> Result<Self, ClockSnapshotError> {
        Err(ClockSnapshotError::Unsupported)
    }
}

#[cfg(unix)]
fn snapshot_timespec(native: libc::timespec) -> Result<ClockSnapshot, ClockSnapshotError> {
    let nanoseconds = u32::try_from(native.tv_nsec)
        .ok()
        .filter(|value| *value < 1_000_000_000)
        .ok_or(ClockSnapshotError::InvalidNanoseconds(native.tv_nsec))?;
    Ok(ClockSnapshot {
        seconds: native.tv_sec,
        nanoseconds,
    })
}

/// Failure to acquire a Unix filesystem-statistics snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatVfsError {
    /// `statvfs` is available only on Unix hosts.
    Unsupported,
    /// The input path contains an interior NUL and cannot be passed to C.
    InputContainsNul,
    /// `statvfs` failed with the captured OS error code.
    Os(i32),
}

impl fmt::Display for StatVfsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("statvfs is unsupported on this host"),
            Self::InputContainsNul => formatter.write_str("statvfs input contains a NUL byte"),
            Self::Os(code) => write!(formatter, "statvfs failed with OS error {code}"),
        }
    }
}

impl std::error::Error for StatVfsError {}

/// One owned, pointer-free snapshot returned by Unix `statvfs`.
///
/// Values retain the native units: block counts use [`Self::fragment_size`],
/// while file counts and the maximum filename length are scalar facts. The
/// private, fixed-size native output structure exists only for the duration of
/// acquisition, so this API performs no size-query allocation or traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatVfsSnapshot {
    pub block_size: u64,
    pub fragment_size: u64,
    pub blocks: u64,
    pub blocks_free: u64,
    pub blocks_available: u64,
    pub files: u64,
    pub files_free: u64,
    pub files_available: u64,
    pub filesystem_id: u64,
    pub mount_flags: u64,
    pub maximum_name_bytes: u64,
}

impl StatVfsSnapshot {
    /// Snapshot filesystem statistics for one native Unix path.
    #[cfg(unix)]
    pub fn acquire(path: &Path) -> Result<Self, StatVfsError> {
        use std::os::unix::ffi::OsStrExt;

        let path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| StatVfsError::InputContainsNul)?;
        let mut native = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `path` is NUL-terminated and `native` points to writable
        // storage for one statvfs value. The value is read only after success.
        let status = unsafe { libc::statvfs(path.as_ptr(), native.as_mut_ptr()) };
        if status != 0 {
            return Err(StatVfsError::Os(
                std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(status),
            ));
        }
        // SAFETY: a zero status means statvfs initialized the complete value.
        Ok(snapshot_statvfs(unsafe { native.assume_init() }))
    }

    /// Return an honest typed failure on non-Unix hosts.
    #[cfg(not(unix))]
    pub fn acquire(_path: &Path) -> Result<Self, StatVfsError> {
        Err(StatVfsError::Unsupported)
    }
}

#[cfg(unix)]
fn snapshot_statvfs(native: libc::statvfs) -> StatVfsSnapshot {
    StatVfsSnapshot {
        block_size: widen_native_unsigned(native.f_bsize),
        fragment_size: widen_native_unsigned(native.f_frsize),
        blocks: widen_native_unsigned(native.f_blocks),
        blocks_free: widen_native_unsigned(native.f_bfree),
        blocks_available: widen_native_unsigned(native.f_bavail),
        files: widen_native_unsigned(native.f_files),
        files_free: widen_native_unsigned(native.f_ffree),
        files_available: widen_native_unsigned(native.f_favail),
        filesystem_id: widen_native_unsigned(native.f_fsid),
        mount_flags: widen_native_unsigned(native.f_flag),
        maximum_name_bytes: widen_native_unsigned(native.f_namemax),
    }
}

#[cfg(unix)]
fn widen_native_unsigned<T: Into<u64>>(value: T) -> u64 {
    value.into()
}

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

    #[cfg(unix)]
    use super::{ClockSnapshot, ClockSnapshotError};
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

    #[cfg(unix)]
    #[test]
    fn timespec_fields_are_copied_by_role_and_nanoseconds_are_validated() {
        assert_eq!(
            super::snapshot_timespec(libc::timespec {
                tv_sec: 123,
                tv_nsec: 456,
            }),
            Ok(ClockSnapshot {
                seconds: 123,
                nanoseconds: 456,
            })
        );
        assert_eq!(
            super::snapshot_timespec(libc::timespec {
                tv_sec: 123,
                tv_nsec: -1,
            }),
            Err(ClockSnapshotError::InvalidNanoseconds(-1))
        );
        assert_eq!(
            super::snapshot_timespec(libc::timespec {
                tv_sec: 123,
                tv_nsec: 1_000_000_000,
            }),
            Err(ClockSnapshotError::InvalidNanoseconds(1_000_000_000))
        );
    }
}
