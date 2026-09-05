//! Exclusive serial byte I/O bound to an inventory pseudonym.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub use crate::contract::device_io::*;

pub struct ResolvedDevice {
    public_id: String,
    private_state_dir: PathBuf,
    native: crate::selected::device_io::NativeResolvedDevice,
}

impl ResolvedDevice {
    #[must_use]
    pub fn public_id(&self) -> &str {
        &self.public_id
    }
}

pub struct OpenedDevice {
    native: Option<crate::selected::device_io::NativeOpenedDevice>,
    serial: SerialConfiguration,
}

/// Invocation-owned PTY device used only by the public black-box court.
///
/// The returned token selects a private, owner-bound registry record; no raw
/// device locator crosses this facade or appears in the fixture's stdout.
#[cfg(unix)]
#[doc(hidden)]
pub struct DeviceIoTestFixture {
    native: crate::selected::device_io::NativeDeviceIoTestFixture,
    token: String,
}

#[cfg(unix)]
impl DeviceIoTestFixture {
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn run(self) -> Result<(), DeviceIoError> {
        crate::selected::device_io::run_test_fixture(self.native)
    }
}

/// Create one private echoing serial fixture for an invocation-owned court.
///
/// This is deliberately absent on Windows, where qualification requires a
/// native COM or virtual-COM court rather than pretending a Unix PTY is one.
#[cfg(unix)]
#[doc(hidden)]
pub fn create_test_fixture(
    registry_root: &Path,
    lifetime: Duration,
) -> Result<DeviceIoTestFixture, DeviceIoError> {
    let (native, token) = crate::selected::device_io::create_test_fixture(registry_root, lifetime)?;
    Ok(DeviceIoTestFixture { native, token })
}

impl OpenedDevice {
    #[must_use]
    pub const fn exclusive_mode(&self) -> DeviceExclusiveMode {
        DeviceExclusiveMode::Kernel
    }

    /// The configuration accepted by exact native readback after opening.
    #[must_use]
    pub const fn serial_configuration(&self) -> SerialConfiguration {
        self.serial
    }

    pub fn read_once(&mut self, max_bytes: usize) -> Result<DeviceReadOutcome, DeviceIoError> {
        if !(1..=DEVICE_IO_MAX_BYTES).contains(&max_bytes) {
            return Err(error(
                DeviceIoErrorKind::InvalidArgument,
                "device-read-limit",
                format!("max_bytes must be between 1 and {DEVICE_IO_MAX_BYTES}"),
            ));
        }
        crate::selected::device_io::read_once(self.native_mut()?, max_bytes)
    }

    pub fn write_once(
        &mut self,
        bytes: &[u8],
        timeout: Duration,
    ) -> Result<DeviceWriteOutcome, DeviceIoError> {
        if bytes.is_empty() || bytes.len() > DEVICE_IO_MAX_BYTES {
            return Err(error(
                DeviceIoErrorKind::InvalidArgument,
                "device-write-limit",
                format!("write must contain between 1 and {DEVICE_IO_MAX_BYTES} bytes"),
            ));
        }
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        if !(1..=DEVICE_IO_TIMEOUT_MS_MAX).contains(&timeout_ms) {
            return Err(error(
                DeviceIoErrorKind::InvalidArgument,
                "device-io-timeout-invalid",
                "device write timeout must be within 1..=300000 milliseconds",
            ));
        }
        crate::selected::device_io::write_once(self.native_mut()?, bytes, timeout_ms as u32)
    }

    pub fn close_restore(mut self) -> Result<(), DeviceIoError> {
        let native = self
            .native
            .take()
            .expect("opened device owns native handle");
        crate::selected::device_io::close_restore(native)
    }

    fn native_mut(
        &mut self,
    ) -> Result<&mut crate::selected::device_io::NativeOpenedDevice, DeviceIoError> {
        self.native.as_mut().ok_or_else(|| {
            error(
                DeviceIoErrorKind::Disconnected,
                "device-disconnected",
                "device handle is closed",
            )
        })
    }
}

pub fn resolve(private_state_dir: &Path, public_id: &str) -> Result<ResolvedDevice, DeviceIoError> {
    let record = crate::device_inventory::resolve_native(private_state_dir, public_id)
        .map_err(map_inventory_error)?;
    let native = crate::selected::device_io::resolve_record(&record)?;
    Ok(ResolvedDevice {
        public_id: public_id.to_owned(),
        private_state_dir: private_state_dir.to_path_buf(),
        native,
    })
}

pub fn open_exclusive(
    resolved: &ResolvedDevice,
    serial: SerialConfiguration,
) -> Result<OpenedDevice, DeviceIoError> {
    let native = crate::selected::device_io::open_exclusive(&resolved.native, serial)?;
    let fresh =
        crate::device_inventory::resolve_native(&resolved.private_state_dir, &resolved.public_id)
            .map_err(|_| {
            error(
                DeviceIoErrorKind::IdentityChanged,
                "device-identity-changed",
                "device identity changed while it was being opened",
            )
        })?;
    if !crate::selected::device_io::matches_record(&resolved.native, &fresh) {
        let _ = crate::selected::device_io::close_restore(native);
        return Err(error(
            DeviceIoErrorKind::IdentityChanged,
            "device-identity-changed",
            "opened device no longer matches the resolved inventory identity",
        ));
    }
    Ok(OpenedDevice {
        native: Some(native),
        serial,
    })
}

fn map_inventory_error(failure: crate::device_inventory::DeviceInventoryError) -> DeviceIoError {
    let kind = match failure.code() {
        "device-id-invalid" => DeviceIoErrorKind::InvalidArgument,
        "device-not-found" => DeviceIoErrorKind::NotFound,
        "device-ambiguous" => DeviceIoErrorKind::Ambiguous,
        _ => DeviceIoErrorKind::OpenFailed,
    };
    error(kind, failure.code().to_owned(), failure.detail().to_owned())
}

pub(crate) fn error(
    kind: DeviceIoErrorKind,
    code: impl Into<std::borrow::Cow<'static, str>>,
    detail: impl Into<String>,
) -> DeviceIoError {
    DeviceIoError::new(kind, code, detail)
}

pub(crate) fn write_error(
    code: impl Into<std::borrow::Cow<'static, str>>,
    detail: impl Into<String>,
    known_written_lower_bound: usize,
    delivery_uncertain: bool,
    retry_safe: bool,
) -> DeviceIoError {
    DeviceIoError::new(DeviceIoErrorKind::WriteFailed, code, detail).with_write_failure(
        known_written_lower_bound,
        delivery_uncertain,
        retry_safe,
    )
}
