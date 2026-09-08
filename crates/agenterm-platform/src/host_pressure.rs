//! Native host-pressure observations without derived cross-platform levels.

#[path = "contract/host_pressure.rs"]
mod contract;

pub use contract::{
    HostPressureError, HostPressureErrorKind, HostPressureSignal, HostPressureSnapshot,
    HostPressureUnavailableReason, LinuxPsiWindow,
};

#[cfg(target_os = "linux")]
#[path = "adapters/linux/host_pressure.rs"]
mod adapter;
#[cfg(target_os = "macos")]
#[path = "adapters/macos/host_pressure.rs"]
mod adapter;
#[cfg(windows)]
#[path = "adapters/windows/host_pressure.rs"]
mod adapter;

pub fn snapshot() -> Result<HostPressureSnapshot, HostPressureError> {
    adapter::snapshot()
}
