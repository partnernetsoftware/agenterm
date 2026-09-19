//! Platform FFI. Zero crates.io deps.

#![allow(dead_code)]

#[cfg(target_os = "macos")]
mod darwin;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::*;

#[cfg(target_os = "macos")]
pub use darwin::*;

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(windows)]
pub use windows::*;
