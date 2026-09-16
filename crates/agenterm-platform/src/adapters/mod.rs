//! Target-specific extensions for capabilities that inherently exchange native values.

/// Platform-independent core of the pre-ConPTY console agent's row re-encoding.
/// Compiled for the Windows adapter and for unit tests on every host, so its
/// logic remains testable off Windows without adding dead production code to
/// the other platform builds.
#[cfg(any(windows, test))]
pub(crate) mod console_row_emit;

#[cfg(windows)]
pub mod windows;
