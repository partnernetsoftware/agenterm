//! One resolver for the executable that hosts a resident owner process.
//!
//! The managed-job, browser-session and device-lease owners are **not** part of
//! the embedding process's own command surface: each is a separate long-lived
//! child that must run `agenterm-cu` with one internal argument. That binary is
//! a formally distributed sibling of the product executable — the release zip
//! ships `agenterm-cu` beside `agenterm`, `install.sh` requires both
//! executables colocated (`REQUIRED_EXECUTABLES=(agenterm agenterm-cu)`) and
//! verifies the `agenterm-cu`/`libagenterm` ABI pair, and the macOS bundle
//! validator requires `Contents/MacOS/agenterm-cu`.
//!
//! What this module owns is therefore only *resolution and honest file shape*:
//! the fixed sibling name next to the running executable, plus the cheapest
//! checks that keep a wrong object from being started. It is deliberately not
//! an atomic identity-bound exec: a path check followed by a spawn is still a
//! TOCTOU window, and claiming otherwise here would be a false guarantee. If a
//! future effect ever needs identity-bound execution, that is a platform
//! capability with its own evidence, not a promise smuggled into a helper.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The fixed sibling binary name every resident owner is started from.
pub(crate) const OWNER_EXECUTABLE_NAME: &str = "agenterm-cu";

/// Why the owner executable could not be resolved.
///
/// The variants exist so the three launch sites can report one structured
/// reason instead of three copies of a string, and so "missing" is
/// distinguishable from "present but the wrong shape".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OwnerExecutableError {
    /// `current_exe()` itself is unavailable.
    HostExecutableUnknown,
    /// The running executable has no parent directory to look beside.
    HostExecutableHasNoParent,
    /// That parent does not exist.
    HostParentMissing,
    /// That parent exists but is not a directory.
    HostParentNotDirectory,
    /// The parent could not be inspected for a reason other than absence.
    HostParentUnavailable(io::ErrorKind),
    /// No entry with the fixed sibling name exists beside the host.
    Missing,
    /// The entry exists but is a symlink, a directory, or another non-regular object.
    NotRegular,
    /// The entry is a regular file without an execute bit (Unix only).
    NotExecutable,
    /// The entry could not be inspected for a reason other than absence.
    Unavailable(io::ErrorKind),
}

impl OwnerExecutableError {
    /// Stable machine-readable reason for a structured detail field.
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::HostExecutableUnknown => "owner_host_executable_unknown",
            Self::HostExecutableHasNoParent => "owner_host_executable_has_no_parent",
            Self::HostParentMissing => "owner_host_parent_missing",
            Self::HostParentNotDirectory => "owner_host_parent_not_directory",
            Self::HostParentUnavailable(_) => "owner_host_parent_unavailable",
            Self::Missing => "owner_executable_missing",
            Self::NotRegular => "owner_executable_not_regular",
            Self::NotExecutable => "owner_executable_not_executable",
            Self::Unavailable(_) => "owner_executable_unavailable",
        }
    }

    /// The native error kind when the refusal was an inspection failure.
    ///
    /// Absence and an unreadable entry are different facts, so a caller can
    /// report the kind instead of collapsing permission or I/O failures into
    /// "missing".
    pub(crate) const fn kind(self) -> Option<io::ErrorKind> {
        match self {
            Self::HostParentUnavailable(kind) | Self::Unavailable(kind) => Some(kind),
            _ => None,
        }
    }
}

/// The fixed sibling name including the platform executable suffix.
fn owner_file_name() -> String {
    format!("{OWNER_EXECUTABLE_NAME}{}", std::env::consts::EXE_SUFFIX)
}

fn is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

/// Windows has no execute bit to check; the suffix plus a regular file is the
/// whole contract there, and an unexecutable object fails at spawn.
#[cfg(windows)]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

/// Resolves the fixed sibling owner executable for one host executable path.
///
/// Read-only: it creates, chmods, canonicalizes and follows nothing. Every
/// refusal happens before any owner process exists, so a caller can close its
/// durable record as a clean pre-effect failure.
pub(crate) fn sibling_of(host_executable: &Path) -> Result<PathBuf, OwnerExecutableError> {
    let parent = host_executable
        .parent()
        .ok_or(OwnerExecutableError::HostExecutableHasNoParent)?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(OwnerExecutableError::HostParentNotDirectory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(OwnerExecutableError::HostParentMissing);
        }
        Err(error) => {
            return Err(OwnerExecutableError::HostParentUnavailable(error.kind()));
        }
    }
    let candidate = parent.join(owner_file_name());
    let metadata = match fs::symlink_metadata(&candidate) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(OwnerExecutableError::Missing);
        }
        // Permission, I/O and every other inspection failure keep their own
        // identity: only a real absence may be reported as missing.
        Err(error) => return Err(OwnerExecutableError::Unavailable(error.kind())),
    };
    if is_link_like(&metadata) || !metadata.is_file() {
        return Err(OwnerExecutableError::NotRegular);
    }
    if !is_executable(&metadata) {
        return Err(OwnerExecutableError::NotExecutable);
    }
    Ok(candidate)
}

/// Resolves the owner executable beside the currently running executable.
pub(crate) fn resolve_current() -> Result<PathBuf, OwnerExecutableError> {
    let host = std::env::current_exe().map_err(|_| OwnerExecutableError::HostExecutableUnknown)?;
    sibling_of(&host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agenterm-owner-exec-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock")
                .as_nanos()
        ));
        fs::create_dir(&path).expect("sandbox directory");
        path
    }

    fn write_host(directory: &Path) -> PathBuf {
        let host = directory.join(format!("agenterm{}", std::env::consts::EXE_SUFFIX));
        fs::write(&host, b"host").expect("host file");
        host
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("execute bit");
    }

    #[test]
    fn resolves_the_fixed_sibling_beside_the_host() {
        let directory = sandbox("ok");
        let host = write_host(&directory);
        let owner = directory.join(owner_file_name());
        #[cfg(unix)]
        make_executable(&host);
        fs::write(&owner, b"owner").expect("owner file");
        #[cfg(unix)]
        make_executable(&owner);
        assert_eq!(sibling_of(&host).expect("resolved sibling"), owner);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_missing_sibling_is_typed_before_any_spawn() {
        let directory = sandbox("missing");
        let host = write_host(&directory);
        assert_eq!(
            sibling_of(&host).expect_err("missing sibling"),
            OwnerExecutableError::Missing
        );
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_directory_or_link_named_like_the_owner_is_not_regular() {
        let directory = sandbox("shape");
        let host = write_host(&directory);
        let owner = directory.join(owner_file_name());
        fs::create_dir(&owner).expect("directory in the owner's place");
        assert_eq!(
            sibling_of(&host).expect_err("directory is not an executable"),
            OwnerExecutableError::NotRegular
        );
        fs::remove_dir(&owner).expect("remove directory");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&host, &owner).expect("symlink");
            assert_eq!(
                sibling_of(&host).expect_err("a link is not a regular owner"),
                OwnerExecutableError::NotRegular
            );
            fs::remove_file(&owner).expect("remove symlink");
        }
        let _ = fs::remove_dir_all(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn a_regular_file_without_an_execute_bit_is_typed_on_unix() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = sandbox("noexec");
        let host = write_host(&directory);
        let owner = directory.join(owner_file_name());
        fs::write(&owner, b"owner").expect("owner file");
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o644)).expect("mode");
        assert_eq!(
            sibling_of(&host).expect_err("no execute bit"),
            OwnerExecutableError::NotExecutable
        );
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_host_without_a_parent_or_a_lost_parent_is_typed() {
        // A relative single-component path has an empty parent directory, not no
        // parent, so the no-parent case needs a real root.
        #[cfg(unix)]
        assert_eq!(
            sibling_of(Path::new("/")).expect_err("no parent"),
            OwnerExecutableError::HostExecutableHasNoParent
        );
        let directory = sandbox("noparent");
        let host = directory.join(format!("agenterm{}", std::env::consts::EXE_SUFFIX));
        fs::remove_dir(&directory).expect("remove sandbox");
        assert_eq!(
            sibling_of(&host).expect_err("parent is gone"),
            OwnerExecutableError::HostParentMissing
        );
    }

    #[test]
    fn a_parent_that_is_not_a_directory_is_typed_distinctly() {
        let directory = sandbox("parentfile");
        let file = directory.join("not-a-directory");
        fs::write(&file, b"file").expect("parent file");
        let host = file.join(format!("agenterm{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(
            sibling_of(&host).expect_err("parent is a file"),
            OwnerExecutableError::HostParentNotDirectory
        );
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn every_refusal_has_a_stable_distinct_reason_and_keeps_its_io_kind() {
        let refusals = [
            OwnerExecutableError::HostExecutableUnknown,
            OwnerExecutableError::HostExecutableHasNoParent,
            OwnerExecutableError::HostParentMissing,
            OwnerExecutableError::HostParentNotDirectory,
            OwnerExecutableError::HostParentUnavailable(io::ErrorKind::PermissionDenied),
            OwnerExecutableError::Missing,
            OwnerExecutableError::NotRegular,
            OwnerExecutableError::NotExecutable,
            OwnerExecutableError::Unavailable(io::ErrorKind::PermissionDenied),
        ];
        let reasons = refusals.map(OwnerExecutableError::reason);
        let mut unique = reasons.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), reasons.len(), "reasons must stay distinct");
        assert!(reasons.iter().all(|reason| reason.starts_with("owner_")));
        assert_eq!(
            OwnerExecutableError::Unavailable(io::ErrorKind::PermissionDenied).kind(),
            Some(io::ErrorKind::PermissionDenied),
            "an inspection failure must not be collapsed into absence"
        );
        assert_eq!(OwnerExecutableError::Missing.kind(), None);
    }
}
