//! Resident owner for one exclusive native serial device handle.

use std::{
    io::Read,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use agenterm_platform::{
    device_io::{self, DeviceReadOutcome, DeviceWriteOutcome, OpenedDevice, SerialConfiguration},
    process::start_identity,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::device_lease_store::{
    DeviceLeaseHandle, DeviceLeaseRecord, DeviceLeaseState, DeviceLeaseStore, DeviceOwnerIdentity,
    DeviceSerialRecord,
};

pub(crate) const LAUNCH_SCHEMA_VERSION: u32 = 1;
const LAUNCH_MAX_BYTES: usize = 32 * 1024;
pub(crate) const TTL_MIN_MS: u64 = 1;
pub(crate) const TTL_MAX_MS: u64 = 86_400_000;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceLeaseLaunch {
    pub schema_version: u32,
    pub state_path: PathBuf,
    pub identity_state_dir: PathBuf,
    pub handle: DeviceLeaseHandle,
    pub device_id: String,
    /// Plaintext authority crosses only the inherited stdin launch pipe and is
    /// retained in this owner process. It is never argv/env/durable metadata.
    pub lease_secret: String,
    /// Session authority is likewise launch-pipe-only. It lets session-end
    /// ask the resident owner to restore/close without persisting the device
    /// lease secret anywhere.
    pub session_id: String,
    pub session_lease: String,
    pub ttl_ms: u64,
    pub serial: SerialConfigurationWire,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SerialConfigurationWire {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: SerialParityWire,
    pub stop_bits: u8,
    pub flow: SerialFlowWire,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SerialParityWire {
    None,
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SerialFlowWire {
    None,
    Software,
    Hardware,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeviceOwnerError {
    pub code: String,
    pub known_written_lower_bound: Option<usize>,
    pub delivery_uncertain: Option<bool>,
    pub retry_safe: Option<bool>,
}

impl DeviceOwnerError {
    pub(crate) fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            known_written_lower_bound: None,
            delivery_uncertain: None,
            retry_safe: None,
        }
    }

    fn after_write(
        code: impl Into<String>,
        known_written_lower_bound: usize,
        delivery_uncertain: bool,
        retry_safe: bool,
    ) -> Self {
        Self {
            code: code.into(),
            known_written_lower_bound: Some(known_written_lower_bound),
            delivery_uncertain: Some(delivery_uncertain),
            retry_safe: Some(retry_safe),
        }
    }
}

pub(crate) struct ResidentDeviceOwner {
    store: DeviceLeaseStore,
    handle: DeviceLeaseHandle,
    owner: DeviceOwnerIdentity,
    opened: Option<OpenedDevice>,
    lease_digest: [u8; 32],
    session_id: String,
    session_lease_digest: [u8; 32],
    lease_deadline: Instant,
    expires_at_utc_ms: i64,
    bytes_read: u64,
    bytes_written: u64,
}

impl ResidentDeviceOwner {
    pub(crate) fn record(&self) -> Result<DeviceLeaseRecord, DeviceOwnerError> {
        self.store
            .get(&self.handle.lease_id)
            .map_err(|_| DeviceOwnerError::new("device_lease_store_unavailable"))?
            .ok_or_else(|| DeviceOwnerError::new("device_lease_not_found"))
    }

    pub(crate) fn authenticate(&self, secret: &str) -> Result<(), DeviceOwnerError> {
        let actual = Sha256::digest(secret.as_bytes());
        if constant_time_equal(&self.lease_digest, actual.as_slice()) {
            Ok(())
        } else {
            Err(DeviceOwnerError::new("device_lease_invalid"))
        }
    }

    pub(crate) fn authenticate_session(
        &self,
        session_id: &str,
        session_lease: &str,
    ) -> Result<(), DeviceOwnerError> {
        let actual = Sha256::digest(session_lease.as_bytes());
        if self.session_id == session_id
            && constant_time_equal(&self.session_lease_digest, actual.as_slice())
        {
            Ok(())
        } else {
            Err(DeviceOwnerError::new("device_session_authority_invalid"))
        }
    }

    pub(crate) fn read_once(
        &mut self,
        max_bytes: usize,
        timeout: Duration,
    ) -> Result<DeviceReadOutcome, DeviceOwnerError> {
        self.require_active()?;
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| DeviceOwnerError::new("device_read_timeout"))?;
        loop {
            let result = self
                .opened
                .as_mut()
                .ok_or_else(|| DeviceOwnerError::new("device_owner_unavailable"))?
                .read_once(max_bytes)
                .map_err(map_device_error)?;
            if result.state != device_io::DeviceReadState::WouldBlock {
                self.bytes_read = self
                    .bytes_read
                    .checked_add(result.bytes.len() as u64)
                    .ok_or_else(|| DeviceOwnerError::new("device_counter_overflow"))?;
                self.publish()?;
                return Ok(result);
            }
            if Instant::now() >= deadline {
                return Err(DeviceOwnerError::new("device_read_timeout"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(crate) fn write_once(
        &mut self,
        bytes: &[u8],
        timeout: Duration,
    ) -> Result<DeviceWriteOutcome, DeviceOwnerError> {
        self.require_active()?;
        let outcome = self
            .opened
            .as_mut()
            .ok_or_else(|| DeviceOwnerError::new("device_owner_unavailable"))?
            .write_once(bytes, timeout)
            .map_err(map_device_error)?;
        self.bytes_written = self
            .bytes_written
            .checked_add(outcome.written_bytes as u64)
            .ok_or_else(|| {
                DeviceOwnerError::after_write(
                    "device_counter_overflow",
                    outcome.written_bytes,
                    false,
                    outcome.written_bytes == 0,
                )
            })?;
        self.publish().map_err(|error| {
            DeviceOwnerError::after_write(
                error.code,
                outcome.written_bytes,
                false,
                outcome.written_bytes == 0,
            )
        })?;
        Ok(outcome)
    }

    pub(crate) fn renew(&mut self, ttl_ms: u64) -> Result<DeviceLeaseRecord, DeviceOwnerError> {
        validate_ttl(ttl_ms)?;
        self.require_active()?;
        self.lease_deadline = Instant::now()
            .checked_add(Duration::from_millis(ttl_ms))
            .ok_or_else(|| DeviceOwnerError::new("device_ttl_invalid"))?;
        self.expires_at_utc_ms = now_utc_ms()?
            .checked_add(ttl_ms as i64)
            .ok_or_else(|| DeviceOwnerError::new("device_ttl_invalid"))?;
        self.store
            .publish_counters(
                &self.handle,
                &self.owner,
                self.bytes_read,
                self.bytes_written,
                self.expires_at_utc_ms,
                now_utc_ms()?,
            )
            .map_err(|_| DeviceOwnerError::new("device_lease_store_unavailable"))
    }

    pub(crate) fn release(&mut self) -> Result<DeviceLeaseRecord, DeviceOwnerError> {
        self.finish(DeviceLeaseState::Released)
    }

    pub(crate) fn expire_if_due(&mut self) -> Result<bool, DeviceOwnerError> {
        if Instant::now() < self.lease_deadline {
            return Ok(false);
        }
        self.finish(DeviceLeaseState::Expired)?;
        Ok(true)
    }

    fn require_active(&self) -> Result<(), DeviceOwnerError> {
        if self.opened.is_none() || Instant::now() >= self.lease_deadline {
            Err(DeviceOwnerError::new("device_lease_expired"))
        } else {
            Ok(())
        }
    }

    fn publish(&self) -> Result<(), DeviceOwnerError> {
        self.store
            .publish_counters(
                &self.handle,
                &self.owner,
                self.bytes_read,
                self.bytes_written,
                self.expires_at_utc_ms,
                now_utc_ms()?,
            )
            .map(|_| ())
            .map_err(|_| DeviceOwnerError::new("device_lease_store_unavailable"))
    }

    fn finish(
        &mut self,
        success_state: DeviceLeaseState,
    ) -> Result<DeviceLeaseRecord, DeviceOwnerError> {
        let Some(opened) = self.opened.take() else {
            return self.record();
        };
        let state = match opened.close_restore() {
            Ok(()) => success_state,
            Err(error) => DeviceLeaseState::CleanupUncertain {
                code: error.code().replace('-', "_"),
            },
        };
        self.store
            .mark_terminal(
                &self.handle,
                &self.owner,
                state,
                self.bytes_read,
                self.bytes_written,
                now_utc_ms()?,
            )
            .map_err(|_| DeviceOwnerError::new("device_release_cleanup_uncertain"))
    }
}

impl Drop for ResidentDeviceOwner {
    fn drop(&mut self) {
        if self.opened.is_some() {
            let _ = self.finish(DeviceLeaseState::Released);
        }
    }
}

pub(crate) fn start_owner_from_launch(
    launch: DeviceLeaseLaunch,
) -> Result<ResidentDeviceOwner, DeviceOwnerError> {
    validate_launch(&launch)?;
    let store = DeviceLeaseStore::open_at(&launch.state_path)
        .map_err(|_| DeviceOwnerError::new("device_lease_store_unavailable"))?;
    let owner = DeviceOwnerIdentity {
        pid: std::process::id(),
        start_identity: start_identity(std::process::id())
            .map_err(|_| DeviceOwnerError::new("device_owner_identity_unknown"))?,
    };
    store
        .claim_opening(&launch.handle, owner.clone(), now_utc_ms()?)
        .map_err(|_| DeviceOwnerError::new("device_claim_intent_failed"))?;
    let serial = launch.serial.to_platform()?;
    let resolved = match device_io::resolve(&launch.identity_state_dir, &launch.device_id) {
        Ok(resolved) => resolved,
        Err(error) => {
            let _ = store.mark_open_failed(
                &launch.handle,
                &owner,
                &error.code().replace('-', "_"),
                now_utc_ms()?,
            );
            return Err(map_device_error(error));
        }
    };
    let opened = match device_io::open_exclusive(&resolved, serial) {
        Ok(opened) => opened,
        Err(error) => {
            let _ = store.mark_open_failed(
                &launch.handle,
                &owner,
                &error.code().replace('-', "_"),
                now_utc_ms()?,
            );
            return Err(map_device_error(error));
        }
    };
    let actual = opened.serial_configuration();
    let serial_record = serial_record(actual);
    store
        .mark_active(
            &launch.handle,
            &owner,
            "kernel",
            Some(serial_record),
            now_utc_ms()?,
        )
        .map_err(|_| DeviceOwnerError::new("device_active_publish_failed"))?;
    let expires_at_utc_ms = now_utc_ms()?
        .checked_add(launch.ttl_ms as i64)
        .ok_or_else(|| DeviceOwnerError::new("device_ttl_invalid"))?;
    Ok(ResidentDeviceOwner {
        store,
        handle: launch.handle,
        owner,
        opened: Some(opened),
        lease_digest: Sha256::digest(launch.lease_secret.as_bytes()).into(),
        session_id: launch.session_id,
        session_lease_digest: Sha256::digest(launch.session_lease.as_bytes()).into(),
        lease_deadline: Instant::now()
            .checked_add(Duration::from_millis(launch.ttl_ms))
            .ok_or_else(|| DeviceOwnerError::new("device_ttl_invalid"))?,
        expires_at_utc_ms,
        bytes_read: 0,
        bytes_written: 0,
    })
}

pub(crate) fn read_launch(mut reader: impl Read) -> Result<DeviceLeaseLaunch, DeviceOwnerError> {
    let mut bytes = Vec::with_capacity(4096);
    reader
        .by_ref()
        .take((LAUNCH_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| DeviceOwnerError::new("device_owner_launch_read_failed"))?;
    if bytes.len() > LAUNCH_MAX_BYTES {
        return Err(DeviceOwnerError::new("device_owner_launch_too_large"));
    }
    let launch = serde_json::from_slice(&bytes)
        .map_err(|_| DeviceOwnerError::new("device_owner_launch_invalid"))?;
    validate_launch(&launch)?;
    Ok(launch)
}

fn validate_launch(launch: &DeviceLeaseLaunch) -> Result<(), DeviceOwnerError> {
    if launch.schema_version != LAUNCH_SCHEMA_VERSION
        || !launch.state_path.is_absolute()
        || !launch.identity_state_dir.is_absolute()
        || launch.device_id.len() != 78
        || !launch.device_id.starts_with("agt-device-v1-")
        || launch.lease_secret.len() != 64
        || !launch
            .lease_secret
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || launch.session_id.is_empty()
        || launch.session_id.len() > 128
        || launch.session_id.chars().any(char::is_control)
        || launch.session_lease.is_empty()
        || launch.session_lease.len() > 512
        || launch.session_lease.chars().any(char::is_control)
    {
        return Err(DeviceOwnerError::new("device_owner_launch_invalid"));
    }
    validate_ttl(launch.ttl_ms)?;
    let _ = launch.serial.to_platform()?;
    Ok(())
}

impl SerialConfigurationWire {
    pub(crate) fn to_platform(self) -> Result<SerialConfiguration, DeviceOwnerError> {
        use agenterm_platform::device_io::{
            SerialDataBits, SerialFlowControl, SerialParity, SerialStopBits,
        };
        let data_bits = match self.data_bits {
            5 => SerialDataBits::Five,
            6 => SerialDataBits::Six,
            7 => SerialDataBits::Seven,
            8 => SerialDataBits::Eight,
            _ => return Err(DeviceOwnerError::new("device_serial_invalid")),
        };
        let stop_bits = match self.stop_bits {
            1 => SerialStopBits::One,
            2 => SerialStopBits::Two,
            _ => return Err(DeviceOwnerError::new("device_serial_invalid")),
        };
        Ok(SerialConfiguration {
            baud_rate: self.baud,
            data_bits,
            parity: match self.parity {
                SerialParityWire::None => SerialParity::None,
                SerialParityWire::Even => SerialParity::Even,
                SerialParityWire::Odd => SerialParity::Odd,
            },
            stop_bits,
            flow_control: match self.flow {
                SerialFlowWire::None => SerialFlowControl::None,
                SerialFlowWire::Software => SerialFlowControl::Software,
                SerialFlowWire::Hardware => SerialFlowControl::Hardware,
            },
        })
    }
}

fn serial_record(serial: SerialConfiguration) -> DeviceSerialRecord {
    use agenterm_platform::device_io::{
        SerialDataBits, SerialFlowControl, SerialParity, SerialStopBits,
    };
    DeviceSerialRecord {
        baud: serial.baud_rate,
        data_bits: match serial.data_bits {
            SerialDataBits::Five => 5,
            SerialDataBits::Six => 6,
            SerialDataBits::Seven => 7,
            SerialDataBits::Eight => 8,
        },
        parity: match serial.parity {
            SerialParity::None => "none",
            SerialParity::Even => "even",
            SerialParity::Odd => "odd",
        }
        .to_owned(),
        stop_bits: match serial.stop_bits {
            SerialStopBits::One => 1,
            SerialStopBits::Two => 2,
        },
        flow: match serial.flow_control {
            SerialFlowControl::None => "none",
            SerialFlowControl::Software => "software",
            SerialFlowControl::Hardware => "hardware",
        }
        .to_owned(),
    }
}

fn validate_ttl(ttl_ms: u64) -> Result<(), DeviceOwnerError> {
    if !(TTL_MIN_MS..=TTL_MAX_MS).contains(&ttl_ms) {
        Err(DeviceOwnerError::new("device_ttl_invalid"))
    } else {
        Ok(())
    }
}

fn map_device_error(error: device_io::DeviceIoError) -> DeviceOwnerError {
    DeviceOwnerError {
        code: error.code().replace('-', "_"),
        known_written_lower_bound: error.known_written_lower_bound(),
        delivery_uncertain: error.delivery_uncertain(),
        retry_safe: error.retry_safe(),
    }
}

pub(crate) fn now_utc_ms() -> Result<i64, DeviceOwnerError> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DeviceOwnerError::new("device_lease_clock_invalid"))?
        .as_millis();
    i64::try_from(value).map_err(|_| DeviceOwnerError::new("device_lease_clock_invalid"))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}
