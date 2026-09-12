//! Typed ownership for paths allocated by Unix `realpath(path, NULL)`.

use std::fmt;
use std::path::{Path, PathBuf};

/// Maximum native-byte length copied from one resolved path.
pub const MAX_REALPATH_BYTES: usize = 1_048_576;

/// Failure to resolve and safely copy a native path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealPathError {
    /// `realpath(path, NULL)` is available only on Unix hosts.
    Unsupported,
    /// The input path contains an interior NUL and cannot be passed to C.
    InputContainsNul,
    /// `realpath` failed with the captured OS error code.
    Os(i32),
    /// The resolved native path exceeded the public copy bound.
    TooLong { limit: usize },
}

impl fmt::Display for RealPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("realpath is unsupported on this host"),
            Self::InputContainsNul => formatter.write_str("realpath input contains a NUL byte"),
            Self::Os(code) => write!(formatter, "realpath failed with OS error {code}"),
            Self::TooLong { limit } => {
                write!(
                    formatter,
                    "realpath returned more than {limit} native bytes"
                )
            }
        }
    }
}

impl std::error::Error for RealPathError {}

/// A pointer-free copy of one path returned by Unix `realpath`.
///
/// Unix paths are native byte strings, not necessarily UTF-8. Acquisition
/// therefore copies the bytes directly into an `OsString`/`PathBuf` without
/// calling `to_str` or performing lossy conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    path: PathBuf,
}

impl ResolvedPath {
    /// Borrow the resolved native path.
    pub fn as_path(&self) -> &Path {
        &self.path
    }

    /// Consume the snapshot and return its owned path.
    pub fn into_path_buf(self) -> PathBuf {
        self.path
    }
}

#[cfg(unix)]
trait FreePath {
    fn free(&self, path: *mut std::ffi::c_char);
}

#[cfg(unix)]
struct OwnedPath<F: FreePath> {
    path: *mut std::ffi::c_char,
    freer: F,
}

#[cfg(unix)]
impl<F: FreePath> Drop for OwnedPath<F> {
    fn drop(&mut self) {
        self.freer.free(self.path);
    }
}

#[cfg(unix)]
trait ResolvePath {
    fn resolve(&self, input: &std::ffi::CStr) -> Result<*mut std::ffi::c_char, i32>;
}

#[cfg(unix)]
struct SystemResolver;

#[cfg(unix)]
impl ResolvePath for SystemResolver {
    fn resolve(&self, input: &std::ffi::CStr) -> Result<*mut libc::c_char, i32> {
        // SAFETY: `input` is NUL-terminated and `NULL` requests a malloc-owned
        // result. A non-null result is transferred to the private owner below.
        let resolved = unsafe { libc::realpath(input.as_ptr(), std::ptr::null_mut()) };
        if resolved.is_null() {
            Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1))
        } else {
            Ok(resolved)
        }
    }
}

#[cfg(unix)]
struct SystemFreer;

#[cfg(unix)]
impl FreePath for SystemFreer {
    fn free(&self, path: *mut std::ffi::c_char) {
        // SAFETY: `path` is the result of one successful
        // `realpath(input, NULL)` call and this private owner releases it once.
        unsafe { libc::free(path.cast()) };
    }
}

#[cfg(unix)]
fn resolve_with<R: ResolvePath, F: FreePath>(
    input: &std::ffi::CStr,
    resolver: &R,
    freer: F,
) -> Result<ResolvedPath, RealPathError> {
    let path = resolver.resolve(input).map_err(RealPathError::Os)?;
    let owned = OwnedPath { path, freer };
    // SAFETY: a successful resolver returns the NUL-terminated allocation
    // promised by `realpath`; the owner keeps it live while the bytes copy.
    let bytes = unsafe { std::ffi::CStr::from_ptr(owned.path) }.to_bytes();
    if bytes.len() > MAX_REALPATH_BYTES {
        return Err(RealPathError::TooLong {
            limit: MAX_REALPATH_BYTES,
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(ResolvedPath {
            path: std::ffi::OsString::from_vec(bytes.to_vec()).into(),
        })
    }
}

impl ResolvedPath {
    /// Resolve `input`, copy its native bytes, and release the C allocation.
    #[cfg(unix)]
    pub fn acquire(input: &Path) -> Result<Self, RealPathError> {
        use std::os::unix::ffi::OsStrExt;

        let input = std::ffi::CString::new(input.as_os_str().as_bytes())
            .map_err(|_| RealPathError::InputContainsNul)?;
        resolve_with(&input, &SystemResolver, SystemFreer)
    }

    /// Return an honest typed failure on non-Unix hosts.
    #[cfg(not(unix))]
    pub fn acquire(_input: &Path) -> Result<Self, RealPathError> {
        Err(RealPathError::Unsupported)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::cell::Cell;
    use std::ffi::{CStr, CString};
    use std::rc::Rc;

    use super::{FreePath, MAX_REALPATH_BYTES, RealPathError, ResolvePath, resolve_with};

    struct AllocatingResolver {
        bytes: Vec<u8>,
    }

    impl ResolvePath for AllocatingResolver {
        fn resolve(&self, _input: &CStr) -> Result<*mut std::ffi::c_char, i32> {
            Ok(CString::new(self.bytes.clone())
                .expect("fixture contains no NUL")
                .into_raw())
        }
    }

    struct FailingResolver(i32);

    impl ResolvePath for FailingResolver {
        fn resolve(&self, _input: &CStr) -> Result<*mut std::ffi::c_char, i32> {
            Err(self.0)
        }
    }

    struct CountingFreer(Rc<Cell<u32>>);

    impl FreePath for CountingFreer {
        fn free(&self, path: *mut std::ffi::c_char) {
            self.0.set(self.0.get() + 1);
            // SAFETY: the allocating fixture transferred this CString once.
            drop(unsafe { CString::from_raw(path) });
        }
    }

    #[test]
    fn successful_snapshot_releases_once_and_preserves_native_bytes() {
        use std::os::unix::ffi::OsStrExt;

        let calls = Rc::new(Cell::new(0));
        let input = c"ignored";
        let result = resolve_with(
            input,
            &AllocatingResolver {
                bytes: b"/tmp/native-\xFF-path".to_vec(),
            },
            CountingFreer(Rc::clone(&calls)),
        )
        .expect("fixture resolves");
        assert_eq!(
            result.as_path().as_os_str().as_bytes(),
            b"/tmp/native-\xFF-path"
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn oversized_snapshot_releases_once_before_returning_typed_failure() {
        let calls = Rc::new(Cell::new(0));
        let error = resolve_with(
            c"ignored",
            &AllocatingResolver {
                bytes: vec![b'x'; MAX_REALPATH_BYTES + 1],
            },
            CountingFreer(Rc::clone(&calls)),
        )
        .expect_err("oversized path is rejected");
        assert_eq!(
            error,
            RealPathError::TooLong {
                limit: MAX_REALPATH_BYTES
            }
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn resolver_failure_has_no_allocation_to_release() {
        let calls = Rc::new(Cell::new(0));
        let error = resolve_with(
            c"missing",
            &FailingResolver(2),
            CountingFreer(Rc::clone(&calls)),
        )
        .expect_err("fixture fails");
        assert_eq!(error, RealPathError::Os(2));
        assert_eq!(calls.get(), 0);
    }
}
