use std::time::Duration;

use super::{
    SimulatorAppLifecycleReceipt, SimulatorAppList, SimulatorBootReceipt, SimulatorDeviceList,
    SimulatorError,
};

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

#[cfg(all(target_os = "macos", feature = "simulator"))]
pub(super) fn list_apps(udid: &str, max: usize) -> Result<SimulatorAppList, SimulatorError> {
    platform::list_apps(udid, max)
}

#[cfg(all(target_os = "macos", feature = "simulator"))]
pub(super) fn launch_exact(
    udid: &str,
    bundle_id: &str,
    timeout: Duration,
) -> Result<SimulatorAppLifecycleReceipt, SimulatorError> {
    platform::launch_exact(udid, bundle_id, timeout)
}

#[cfg(all(target_os = "macos", feature = "simulator"))]
pub(super) fn terminate_exact(
    udid: &str,
    bundle_id: &str,
    timeout: Duration,
) -> Result<SimulatorAppLifecycleReceipt, SimulatorError> {
    platform::terminate_exact(udid, bundle_id, timeout)
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

#[cfg(not(all(target_os = "macos", feature = "simulator")))]
pub(super) fn list_apps(_udid: &str, _max: usize) -> Result<SimulatorAppList, SimulatorError> {
    let _portable_parser = super::parse_app_list;
    Err(unsupported())
}

#[cfg(not(all(target_os = "macos", feature = "simulator")))]
pub(super) fn launch_exact(
    _udid: &str,
    _bundle_id: &str,
    _timeout: Duration,
) -> Result<SimulatorAppLifecycleReceipt, SimulatorError> {
    Err(unsupported())
}

#[cfg(not(all(target_os = "macos", feature = "simulator")))]
pub(super) fn terminate_exact(
    _udid: &str,
    _bundle_id: &str,
    _timeout: Duration,
) -> Result<SimulatorAppLifecycleReceipt, SimulatorError> {
    Err(unsupported())
}

#[cfg(not(all(target_os = "macos", feature = "simulator")))]
fn unsupported() -> SimulatorError {
    SimulatorError::new(
        super::SimulatorErrorKind::Unsupported,
        "CoreSimulator is unavailable on this build",
    )
}
