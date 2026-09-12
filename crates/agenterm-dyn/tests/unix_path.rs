//! Public contract tests for the owned Unix `realpath(path, NULL)` result.

use std::path::Path;

#[cfg(windows)]
use agenterm_dyn::RealPathError;
use agenterm_dyn::ResolvedPath;

#[cfg(unix)]
#[test]
fn live_existing_path_matches_the_standard_library_oracle() {
    let input = Path::new(".");
    let resolved = ResolvedPath::acquire(input).expect("realpath resolves the repository root");
    let oracle = std::fs::canonicalize(input).expect("standard library resolves the same path");
    assert_eq!(resolved.as_path(), oracle);
}

#[cfg(unix)]
#[test]
fn live_missing_path_returns_the_native_os_error() {
    let input = Path::new("agenterm-dyn-realpath-fixture-that-does-not-exist");
    let error = ResolvedPath::acquire(input).expect_err("missing path must fail");
    assert!(matches!(error, agenterm_dyn::RealPathError::Os(code) if code > 0));
}

#[cfg(unix)]
#[test]
fn input_nul_is_rejected_before_calling_realpath() {
    use std::os::unix::ffi::OsStrExt;

    let input = std::ffi::OsStr::from_bytes(b"part\0part");
    assert_eq!(
        ResolvedPath::acquire(Path::new(input)),
        Err(agenterm_dyn::RealPathError::InputContainsNul)
    );
}

#[cfg(windows)]
#[test]
fn acquisition_is_honestly_unsupported_on_windows() {
    assert!(matches!(
        ResolvedPath::acquire(Path::new(".")),
        Err(RealPathError::Unsupported)
    ));
}
