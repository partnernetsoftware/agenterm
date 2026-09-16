//! Target-specific extensions for capabilities that inherently exchange native values.

/// Platform-independent core of the pre-ConPTY console agent's row re-encoding.
/// Compiled on every target so its logic can be unit-tested off Windows; the
/// Windows adapter (`windows::console_agent`) consumes it.
pub(crate) mod console_row_emit;

#[cfg(windows)]
pub mod windows;
