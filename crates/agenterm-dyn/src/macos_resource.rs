//! Typed ownership for Mach resources that cannot safely cross the generic dlcall door.

use std::fmt;

/// Maximum bytes copied from either native string returned by `dladdr`.
#[cfg(any(target_os = "macos", test))]
pub const MAX_DLADDR_STRING_BYTES: usize = 64 * 1024;

/// Failure to snapshot the image that contains agenterm-dyn's own code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DlAddressError {
    /// `dladdr` exists only on Darwin hosts in this contract.
    Unsupported,
    /// Darwin could not resolve the address to an image.
    NotFound,
    /// A native string exceeded the bounded snapshot contract.
    StringTooLong { limit: usize },
}

impl fmt::Display for DlAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("dladdr is unsupported on this host"),
            Self::NotFound => formatter.write_str("dladdr did not resolve the current image"),
            Self::StringTooLong { limit } => {
                write!(formatter, "dladdr string exceeds the {limit}-byte limit")
            }
        }
    }
}

impl std::error::Error for DlAddressError {}

/// Pointer-free facts copied from Darwin's `Dl_info` for this crate's image.
///
/// Native path and symbol bytes are retained without requiring UTF-8. Raw
/// addresses and borrowed C strings never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlAddressSnapshot {
    image_path: Vec<u8>,
    symbol_name: Option<Vec<u8>>,
}

impl DlAddressSnapshot {
    /// Snapshot the image containing a private function in this crate.
    ///
    /// The queried address is selected internally; this API does not expose a
    /// general raw-address lookup facility.
    #[cfg(target_os = "macos")]
    pub fn current_image() -> Result<Self, DlAddressError> {
        let mut info = DlInfo::empty();
        // SAFETY: `dladdr_anchor` is a live function in this image and `info`
        // is valid writable storage for the duration of the call.
        let found = unsafe { dladdr(dladdr_anchor as *const () as *const _, &mut info) };
        if found == 0 {
            return Err(DlAddressError::NotFound);
        }
        // SAFETY: successful `dladdr` initializes `info`; Darwin owns the
        // pointed-to NUL-terminated strings and keeps them live while loaded.
        unsafe { snapshot_dl_info(&info) }
    }

    /// Return an honest typed failure on non-Darwin hosts.
    #[cfg(not(target_os = "macos"))]
    pub fn current_image() -> Result<Self, DlAddressError> {
        Err(DlAddressError::Unsupported)
    }

    /// Native bytes of the containing image path, excluding its trailing NUL.
    pub fn image_path(&self) -> &[u8] {
        &self.image_path
    }

    /// Native bytes of the nearest symbol name, when Darwin reports one.
    pub fn symbol_name(&self) -> Option<&[u8]> {
        self.symbol_name.as_deref()
    }
}

#[cfg(any(target_os = "macos", test))]
#[repr(C)]
struct DlInfo {
    image_path: *const std::ffi::c_char,
    image_base: *mut std::ffi::c_void,
    symbol_name: *const std::ffi::c_char,
    symbol_address: *mut std::ffi::c_void,
}

#[cfg(any(target_os = "macos", test))]
impl DlInfo {
    fn empty() -> Self {
        Self {
            image_path: std::ptr::null(),
            image_base: std::ptr::null_mut(),
            symbol_name: std::ptr::null(),
            symbol_address: std::ptr::null_mut(),
        }
    }
}

#[cfg(any(target_os = "macos", test))]
unsafe fn copy_native_string(pointer: *const std::ffi::c_char) -> Result<Vec<u8>, DlAddressError> {
    // SAFETY: the caller establishes that a non-null pointer names a live
    // NUL-terminated native string for the duration of this copy.
    let bytes = unsafe { std::ffi::CStr::from_ptr(pointer) }.to_bytes();
    if bytes.len() > MAX_DLADDR_STRING_BYTES {
        return Err(DlAddressError::StringTooLong {
            limit: MAX_DLADDR_STRING_BYTES,
        });
    }
    Ok(bytes.to_vec())
}

#[cfg(any(target_os = "macos", test))]
unsafe fn snapshot_dl_info(info: &DlInfo) -> Result<DlAddressSnapshot, DlAddressError> {
    if info.image_path.is_null() {
        return Err(DlAddressError::NotFound);
    }
    // SAFETY: the caller establishes that `info` came from successful dladdr.
    let image_path = unsafe { copy_native_string(info.image_path) }?;
    let symbol_name = if info.symbol_name.is_null() {
        None
    } else {
        // SAFETY: same successful-dladdr guarantee as `image_path`.
        Some(unsafe { copy_native_string(info.symbol_name) }?)
    };
    Ok(DlAddressSnapshot {
        image_path,
        symbol_name,
    })
}

#[cfg(target_os = "macos")]
extern "C" fn dladdr_anchor() {}

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
    fn dladdr(address: *const std::ffi::c_void, info: *mut DlInfo) -> std::ffi::c_int;
    fn mach_host_self() -> u32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
    fn mach_port_get_refs(task: u32, name: u32, right: u32, refs: *mut u32) -> i32;
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::ffi::c_char;
    use std::rc::Rc;

    use super::{DlAddressError, DlInfo, OwnedPort, ReleasePort, snapshot_dl_info};

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

    #[test]
    fn dladdr_snapshot_copies_native_bytes_without_exposing_pointers() {
        static IMAGE: &[u8] = b"/synthetic/image\0";
        static SYMBOL: &[u8] = b"symbol_\xff\0";
        let info = DlInfo {
            image_path: IMAGE.as_ptr().cast::<c_char>(),
            image_base: std::ptr::dangling_mut(),
            symbol_name: SYMBOL.as_ptr().cast::<c_char>(),
            symbol_address: std::ptr::dangling_mut(),
        };

        // SAFETY: both pointers above reference static NUL-terminated byte strings.
        let snapshot = unsafe { snapshot_dl_info(&info) }.expect("synthetic dladdr snapshot");
        assert_eq!(snapshot.image_path(), b"/synthetic/image");
        assert_eq!(snapshot.symbol_name(), Some(&b"symbol_\xff"[..]));
    }

    #[test]
    fn dladdr_snapshot_requires_an_image_name_but_allows_no_symbol() {
        static IMAGE: &[u8] = b"image\0";
        let info = DlInfo {
            image_path: IMAGE.as_ptr().cast::<c_char>(),
            image_base: std::ptr::null_mut(),
            symbol_name: std::ptr::null(),
            symbol_address: std::ptr::null_mut(),
        };
        // SAFETY: the image pointer references a static NUL-terminated byte string.
        let snapshot = unsafe { snapshot_dl_info(&info) }.expect("image-only snapshot");
        assert_eq!(snapshot.image_path(), b"image");
        assert_eq!(snapshot.symbol_name(), None);

        let missing = DlInfo::empty();
        // SAFETY: null is handled before any string dereference.
        assert_eq!(
            unsafe { snapshot_dl_info(&missing) },
            Err(DlAddressError::NotFound)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn current_image_resolves_a_real_pointer_free_snapshot() {
        let snapshot = super::DlAddressSnapshot::current_image().expect("current image");
        assert!(!snapshot.image_path().is_empty());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn current_image_is_honestly_unsupported_off_darwin() {
        assert_eq!(
            super::DlAddressSnapshot::current_image(),
            Err(DlAddressError::Unsupported)
        );
    }
}
