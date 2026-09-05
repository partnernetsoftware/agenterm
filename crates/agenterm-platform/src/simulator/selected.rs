use std::time::Duration;

use super::{SimulatorBootReceipt, SimulatorDeviceList, SimulatorError};

#[cfg(all(target_os = "macos", feature = "simulator"))]
#[path = "../adapters/macos/simulator.rs"]
mod platform;

#[cfg(all(target_os = "macos", feature = "simulator"))]
pub(super) fn list_devices(max: usize) -> Result<SimulatorDeviceList, SimulatorError> {
    platform::list_devices(max)
}

#[cfg(all(target_os = "macos", feature = "simulator"))]
pub(super) fn boot_exact(
    udid: &str,
    timeout: Duration,
) -> Result<SimulatorBootReceipt, SimulatorError> {
    platform::boot_exact(udid, timeout)
}

#[cfg(not(all(target_os = "macos", feature = "simulator")))]
pub(super) fn list_devices(_max: usize) -> Result<SimulatorDeviceList, SimulatorError> {
    // Keep the portable parser type-checked on unsupported target cells; live
    // dispatch remains unsupported and never feeds it synthetic native data.
    let _portable_parser = super::parse_device_list;
    Err(SimulatorError::new(
        super::SimulatorErrorKind::Unsupported,
        "CoreSimulator is unavailable on this build",
    ))
}

#[cfg(not(all(target_os = "macos", feature = "simulator")))]
pub(super) fn boot_exact(
    _udid: &str,
    _timeout: Duration,
) -> Result<SimulatorBootReceipt, SimulatorError> {
    Err(SimulatorError::new(
        super::SimulatorErrorKind::Unsupported,
        "CoreSimulator is unavailable on this build",
    ))
}
