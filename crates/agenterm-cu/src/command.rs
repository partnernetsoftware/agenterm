//! Abstract, target-agnostic command set (PRD_02_29).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{
    browser_bridge::ConnectionId,
    service_control::{ServiceOperation, ServiceScope},
    target::TargetRef,
};

fn is_false(value: &bool) -> bool {
    !*value
}

const fn service_scope_operation(scope: ServiceScope) -> &'static str {
    match scope {
        ServiceScope::User => "user",
        ServiceScope::System => "system",
    }
}

const fn service_operation(operation: ServiceOperation) -> &'static str {
    match operation {
        ServiceOperation::Start => "start",
        ServiceOperation::Stop => "stop",
        ServiceOperation::Restart => "restart",
        ServiceOperation::Bootstrap => "bootstrap",
        ServiceOperation::Bootout => "bootout",
    }
}

const JOB_COMMAND_PARTS_MAX: usize = 256;
const JOB_COMMAND_BYTES_MAX: usize = 32 * 1024;
const JOB_ENVIRONMENT_ENTRIES_MAX: usize = 128;
const JOB_ENVIRONMENT_BYTES_MAX: usize = 24 * 1024;
const JOB_CWD_BYTES_MAX: usize = 8 * 1024;
const JOB_TTL_SECONDS_MAX: u64 = 86_400;
const JOB_LIST_MAX: usize = 1_024;
const JOB_EVENTS_TIMEOUT_MS_MAX: u64 = 300_000;
const JOB_RESOURCES_WATCH_MS_MAX: u64 = 300_000;
const JOB_EVENTS_BYTES_MAX: usize = 1024 * 1024;
const JOB_WRITE_DECODED_BYTES_MAX: usize = 64 * 1024;
const JOB_WAIT_TIMEOUT_MS_MAX: u64 = 86_400_000;
const JOB_STOP_GRACE_MS_MAX: u64 = 60_000;
const JOB_PRUNE_MAX_AGE_SECONDS_MAX: u64 = 10 * 365 * 24 * 60 * 60;
pub const SIMULATOR_RESULTS_MAX: usize = 200;
pub const SIMULATOR_TIMEOUT_MS_MAX: u64 = 600_000;
pub const STORAGE_DEVICES_MAX: usize = 5_000;
pub const DEVICE_INVENTORY_MAX: usize = 5_000;
pub const DEVICE_WATCH_DURATION_MS_MAX: u64 = 3_600_000;
pub const DEVICE_WATCH_INTERVAL_MS_MIN: u64 = 250;
pub const DEVICE_WATCH_INTERVAL_MS_MAX: u64 = 60_000;
pub const DEVICE_WATCH_EVENTS_MAX: usize = 5_000;
pub const DEVICE_LEASE_LIST_MAX: usize = 1_024;
pub const DEVICE_IO_BYTES_MAX: usize = 64 * 1024;
pub const DEVICE_IO_TIMEOUT_MS_MAX: u64 = 300_000;
pub const DEVICE_LEASE_TTL_SECONDS_MAX: u64 = 86_400;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceDataEncoding {
    Base64,
    Hex,
}

impl DeviceDataEncoding {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Base64 => "base64",
            Self::Hex => "hex",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceSerialParity {
    None,
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceSerialFlow {
    None,
    Software,
    Hardware,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSerialConfiguration {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: DeviceSerialParity,
    pub stop_bits: u8,
    pub flow: DeviceSerialFlow,
}

pub fn validate_simulator_udid(udid: &str) -> Result<(), &'static str> {
    let bytes = udid.as_bytes();
    if bytes.len() != 36
        || !bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        return Err("simulator UDID must be a 36-byte hyphenated hexadecimal identifier");
    }
    Ok(())
}

pub fn validate_simulator_bundle_id(bundle_id: &str) -> Result<(), &'static str> {
    let valid = !bundle_id.is_empty()
        && bundle_id.len() <= 255
        && bundle_id.split('.').count() >= 2
        && bundle_id.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && segment
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && segment
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        });
    if !valid {
        return Err("simulator bundle id must be a bounded dotted ASCII identifier");
    }
    Ok(())
}

fn deserialize_simulator_udid<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    validate_simulator_udid(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn deserialize_simulator_bundle_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    validate_simulator_bundle_id(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn deserialize_simulator_max<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = usize::deserialize(deserializer)?;
    if !(1..=SIMULATOR_RESULTS_MAX).contains(&value) {
        return Err(serde::de::Error::custom("simulator max must be in 1..=200"));
    }
    Ok(value)
}

fn deserialize_storage_devices_max<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = usize::deserialize(deserializer)?;
    if !(1..=STORAGE_DEVICES_MAX).contains(&value) {
        return Err(serde::de::Error::custom(
            "storage devices max must be in 1..=5000",
        ));
    }
    Ok(value)
}

fn deserialize_device_inventory_max<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = usize::deserialize(deserializer)?;
    if !(1..=DEVICE_INVENTORY_MAX).contains(&value) {
        return Err(serde::de::Error::custom(
            "device inventory max must be in 1..=5000",
        ));
    }
    Ok(value)
}

fn deserialize_device_watch_duration_ms<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if !(1_000..=DEVICE_WATCH_DURATION_MS_MAX).contains(&value) {
        return Err(serde::de::Error::custom(
            "device watch duration_ms must be in 1000..=3600000",
        ));
    }
    Ok(value)
}

fn deserialize_device_watch_interval_ms<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if !(DEVICE_WATCH_INTERVAL_MS_MIN..=DEVICE_WATCH_INTERVAL_MS_MAX).contains(&value) {
        return Err(serde::de::Error::custom(
            "device watch interval_ms must be in 250..=60000",
        ));
    }
    Ok(value)
}

fn deserialize_device_watch_event_max<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = usize::deserialize(deserializer)?;
    if !(1..=DEVICE_WATCH_EVENTS_MAX).contains(&value) {
        return Err(serde::de::Error::custom(
            "device watch event_max must be in 1..=5000",
        ));
    }
    Ok(value)
}

fn deserialize_simulator_timeout_ms<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if !(1..=SIMULATOR_TIMEOUT_MS_MAX).contains(&value) {
        return Err(serde::de::Error::custom(
            "simulator timeout_ms must be in 1..=600000",
        ));
    }
    Ok(value)
}

fn deserialize_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = bool::deserialize(deserializer)?;
    if !value {
        return Err(serde::de::Error::custom(
            "simulator expectation acknowledgement must be true",
        ));
    }
    Ok(true)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobEnvironment {
    pub name: String,
    /// `None` removes an inherited value; `Some` sets it.
    pub value: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobProcessLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_files: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processes: Option<u32>,
}

impl JobProcessLimits {
    fn validate(self) -> Result<(), &'static str> {
        if self.cpu_seconds.is_none()
            && self.memory_bytes.is_none()
            && self.file_size_bytes.is_none()
            && self.open_files.is_none()
            && self.processes.is_none()
        {
            return Err("managed-job limits must contain at least one limit");
        }
        if self
            .cpu_seconds
            .is_some_and(|value| !(1..=86_400).contains(&value))
            || self
                .memory_bytes
                .is_some_and(|value| !(1024 * 1024..=1024_u64.pow(4)).contains(&value))
            || self
                .file_size_bytes
                .is_some_and(|value| !(1..=1024_u64.pow(4)).contains(&value))
            || self
                .open_files
                .is_some_and(|value| !(16..=1_048_576).contains(&value))
            || self
                .processes
                .is_some_and(|value| !(1..=1_048_576).contains(&value))
        {
            return Err("managed-job limits are outside their closed bounds");
        }
        #[cfg(windows)]
        if self.file_size_bytes.is_some() || self.open_files.is_some() {
            return Err("Windows managed jobs do not support file-size or open-file limits");
        }
        #[cfg(target_os = "macos")]
        if self.memory_bytes.is_some() {
            return Err("macOS managed jobs do not support a useful address-space memory limit");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobStateFilter {
    StartIntent,
    Starting,
    Running,
    StartFailed,
    Exited,
    Signaled,
    Detached,
    OrphanedUncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobOutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileTransactionAction {
    Status,
    Rollback,
    Recover,
    Finalize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceInventorySelector {
    Usb,
    Bluetooth,
    Audio,
    Camera,
    Gpu,
    All,
}

impl DeviceInventorySelector {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "usb" => Ok(Self::Usb),
            "bluetooth" => Ok(Self::Bluetooth),
            "audio" => Ok(Self::Audio),
            "camera" => Ok(Self::Camera),
            "gpu" => Ok(Self::Gpu),
            "all" => Ok(Self::All),
            _ => Err("device inventory type must be usb|bluetooth|audio|camera|gpu|all"),
        }
    }
}

/// An absolute byte position in one managed job output stream. Stdout and
/// stderr always carry separate cursors. Decimal text keeps the wire lossless
/// for clients whose number type cannot represent every `u64`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct JobOutputCursor(String);

impl JobOutputCursor {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        validate_job_cursor(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn value(&self) -> u64 {
        self.0
            .parse()
            .expect("JobOutputCursor construction validates u64 decimal text")
    }
}

impl<'de> Deserialize<'de> for JobOutputCursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

fn validate_job_cursor(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || value.parse::<u64>().is_err()
    {
        return Err("managed-job output cursor must be canonical decimal u64 text");
    }
    Ok(())
}

fn deserialize_job_command<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let command = Vec::<String>::deserialize(deserializer)?;
    validate_job_command(&command).map_err(serde::de::Error::custom)?;
    Ok(command)
}

fn validate_job_command(command: &[String]) -> Result<(), &'static str> {
    if command.is_empty()
        || command.len() > JOB_COMMAND_PARTS_MAX
        || command[0].is_empty()
        || command.iter().any(|part| part.as_bytes().contains(&0))
    {
        return Err("managed-job command must contain 1..=256 parts and a nonempty program");
    }
    let bytes = command
        .iter()
        .try_fold(0usize, |total, part| total.checked_add(part.len()))
        .ok_or("managed-job command byte length overflow")?;
    if bytes > JOB_COMMAND_BYTES_MAX {
        return Err("managed-job command content exceeds 32 KiB");
    }
    Ok(())
}

fn deserialize_job_environment<'de, D>(deserializer: D) -> Result<Vec<JobEnvironment>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let environment = Vec::<JobEnvironment>::deserialize(deserializer)?;
    validate_job_environment(&environment).map_err(serde::de::Error::custom)?;
    Ok(environment)
}

fn validate_job_environment(environment: &[JobEnvironment]) -> Result<(), &'static str> {
    let mut names = HashSet::with_capacity(environment.len());
    if environment.len() > JOB_ENVIRONMENT_ENTRIES_MAX
        || environment.iter().any(|entry| {
            entry.name.is_empty()
                || entry.name.as_bytes().contains(&0)
                || entry.name.as_bytes().contains(&b'=')
                || entry
                    .value
                    .as_ref()
                    .is_some_and(|value| value.as_bytes().contains(&0))
                || !names.insert(entry.name.to_lowercase())
        })
    {
        return Err("managed-job environment must contain at most 128 entries with nonempty names");
    }
    let bytes = environment.iter().try_fold(0usize, |total, entry| {
        total
            .checked_add(entry.name.len())
            .and_then(|total| total.checked_add(entry.value.as_ref().map_or(0, String::len)))
    });
    if bytes.is_none_or(|bytes| bytes > JOB_ENVIRONMENT_BYTES_MAX) {
        return Err("managed-job environment content exceeds 24 KiB");
    }
    Ok(())
}

fn deserialize_job_cwd<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let cwd = Option::<String>::deserialize(deserializer)?;
    if cwd
        .as_ref()
        .is_some_and(|path| path.len() > JOB_CWD_BYTES_MAX || path.as_bytes().contains(&0))
    {
        return Err(serde::de::Error::custom("managed-job cwd exceeds 8 KiB"));
    }
    Ok(cwd)
}

fn deserialize_job_process_limits<'de, D>(
    deserializer: D,
) -> Result<Option<JobProcessLimits>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let limits = Option::<JobProcessLimits>::deserialize(deserializer)?;
    if let Some(limits) = limits {
        limits.validate().map_err(serde::de::Error::custom)?;
    }
    Ok(limits)
}

fn deserialize_job_ttl<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if !(1..=JOB_TTL_SECONDS_MAX).contains(&value) {
        return Err(serde::de::Error::custom(
            "managed-job ttl_seconds must be in 1..=86400",
        ));
    }
    Ok(value)
}

fn deserialize_job_generation<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value == 0 {
        return Err(serde::de::Error::custom(
            "managed-job generation must be nonzero",
        ));
    }
    Ok(value)
}

fn deserialize_job_list_max<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<usize>::deserialize(deserializer)?;
    if value.is_some_and(|value| !(1..=JOB_LIST_MAX).contains(&value)) {
        return Err(serde::de::Error::custom(
            "managed-job list max must be in 1..=1024",
        ));
    }
    Ok(value)
}

fn deserialize_job_events_timeout<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value > JOB_EVENTS_TIMEOUT_MS_MAX {
        return Err(serde::de::Error::custom(
            "managed-job events timeout_ms must be at most 300000",
        ));
    }
    Ok(value)
}

fn deserialize_job_events_max<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = usize::deserialize(deserializer)?;
    if !(2..=JOB_EVENTS_BYTES_MAX).contains(&value) {
        return Err(serde::de::Error::custom(
            "managed-job events max_bytes must be in 2..=1048576",
        ));
    }
    Ok(value)
}

fn deserialize_job_output_max<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = usize::deserialize(deserializer)?;
    if !(1..=JOB_EVENTS_BYTES_MAX).contains(&value) {
        return Err(serde::de::Error::custom(
            "managed-job output max_bytes must be in 1..=1048576",
        ));
    }
    Ok(value)
}

fn deserialize_job_write<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    validate_job_write_base64(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn validate_job_write_base64(value: &str) -> Result<(), &'static str> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err("managed-job data_base64 must be padded standard base64");
    }
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2
        || bytes[..bytes.len().saturating_sub(padding)]
            .iter()
            .any(|byte| !byte.is_ascii_alphanumeric() && *byte != b'+' && *byte != b'/')
        || bytes[..bytes.len().saturating_sub(padding)].contains(&b'=')
    {
        return Err("managed-job data_base64 must be padded standard base64");
    }
    let decoded = bytes
        .len()
        .checked_div(4)
        .and_then(|groups| groups.checked_mul(3))
        .and_then(|bytes| bytes.checked_sub(padding))
        .ok_or("managed-job data_base64 length overflow")?;
    if decoded > JOB_WRITE_DECODED_BYTES_MAX {
        return Err("managed-job stdin write exceeds 64 KiB decoded");
    }
    Ok(())
}

fn deserialize_job_wait_timeout<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value > JOB_WAIT_TIMEOUT_MS_MAX {
        return Err(serde::de::Error::custom(
            "managed-job wait timeout_ms must be at most 86400000",
        ));
    }
    Ok(value)
}

fn deserialize_job_control_timeout<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if !(1..=60_000).contains(&value) {
        return Err(serde::de::Error::custom(
            "managed-job control timeout_ms must be in 1..=60000",
        ));
    }
    Ok(value)
}

fn deserialize_job_stop_grace<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value > JOB_STOP_GRACE_MS_MAX {
        return Err(serde::de::Error::custom(
            "managed-job stop grace_ms must be at most 60000",
        ));
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PointerButton {
    #[default]
    Left,
    Right,
    Middle,
}

/// The `invoke` action vocabulary (absorbed from `moltbaby/skills/mcu`,
/// 2026-08-30): one spelling on every platform; a platform without a
/// mapping answers typed `unsupported`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvokeAction {
    Press,
    SetValue,
    SelectOption,
    SetChecked,
    SetExpanded,
    Increment,
    Decrement,
    SetSelected,
    SetSelection,
    ScrollTo,
    Cancel,
    ShowDefaultUi,
}

/// What `app` does to the application owning the named window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppAction {
    Hide,
    Show,
    Quit,
    Launch,
}

/// Single-process termination mode. The native mechanism must target the
/// exact process object, not reopen a reusable PID after identity validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessKillMode {
    Graceful,
    Forceful,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessRunState {
    Running,
    Stopped,
}

/// Exact Darwin process-background policy operation.
///
/// `Status` is observation-only. Mutating actions are represented so callers
/// receive one stable typed contract even when the host cannot acquire the
/// exact native object authority required to perform them safely.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessPolicyAction {
    Status,
    Background,
    Normal,
}

impl ProcessPolicyAction {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "status" => Some(Self::Status),
            "background" => Some(Self::Background),
            "normal" => Some(Self::Normal),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Background => "background",
            Self::Normal => "normal",
        }
    }

    pub const fn requested_background(self) -> Option<bool> {
        match self {
            Self::Status => None,
            Self::Background => Some(true),
            Self::Normal => Some(false),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProcessSignalKind {
    #[serde(rename = "SIGHUP")]
    Hangup,
    #[serde(rename = "SIGINT")]
    Interrupt,
    #[serde(rename = "SIGTERM")]
    Terminate,
    #[serde(rename = "SIGKILL")]
    Kill,
    #[serde(rename = "SIGSTOP")]
    Stop,
    #[serde(rename = "SIGCONT")]
    Continue,
    #[serde(rename = "SIGUSR1")]
    User1,
    #[serde(rename = "SIGUSR2")]
    User2,
}

const fn default_process_tree_max_descendants() -> usize {
    500
}

impl ProcessSignalKind {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_uppercase().as_str() {
            "HUP" | "SIGHUP" => Self::Hangup,
            "INT" | "SIGINT" => Self::Interrupt,
            "TERM" | "SIGTERM" => Self::Terminate,
            "KILL" | "SIGKILL" => Self::Kill,
            "STOP" | "SIGSTOP" => Self::Stop,
            "CONT" | "SIGCONT" => Self::Continue,
            "USR1" | "SIGUSR1" => Self::User1,
            "USR2" | "SIGUSR2" => Self::User2,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hangup => "SIGHUP",
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
            Self::Kill => "SIGKILL",
            Self::Stop => "SIGSTOP",
            Self::Continue => "SIGCONT",
            Self::User1 => "SIGUSR1",
            Self::User2 => "SIGUSR2",
        }
    }
}

/// Signal semantics owned by the foreground process group of one retained PTY.
/// This is deliberately narrower than arbitrary process signaling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PtySignalKind {
    Interrupt,
    Terminate,
    Stop,
    Continue,
}

impl PtySignalKind {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_lowercase().as_str() {
            "int" | "sigint" | "interrupt" => Self::Interrupt,
            "term" | "sigterm" | "terminate" => Self::Terminate,
            "stop" | "sigstop" => Self::Stop,
            "cont" | "sigcont" | "continue" => Self::Continue,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Terminate => "terminate",
            Self::Stop => "stop",
            Self::Continue => "continue",
        }
    }

    pub const fn expected_postcondition(self) -> &'static str {
        match self {
            Self::Interrupt => "delivered",
            Self::Terminate => "exited",
            Self::Stop => "stopped",
            Self::Continue => "running",
        }
    }
}

impl ProcessRunState {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "running" => Some(Self::Running),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
        }
    }

    pub const fn is_stopped(self) -> bool {
        matches!(self, Self::Stopped)
    }
}

/// One bounded wait over an AgenTerm-owned tab. `Contains` observes the
/// current terminal screen; `Exited` observes process exit; `Finalized` also
/// waits for the reader/parser tail to drain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "value")]
pub enum TerminalWaitCondition {
    Contains(String),
    Exited,
    Finalized,
}

/// One explicit local scrollback-viewport mutation. This never sends cursor
/// keys or mouse input to the terminal application.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalScrollAction {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

impl TerminalScrollAction {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "up" => Self::Up,
            "down" => Self::Down,
            "page-up" => Self::PageUp,
            "page-down" => Self::PageDown,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::PageUp => "page-up",
            Self::PageDown => "page-down",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

impl ProcessKillMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "graceful" | "term" | "SIGTERM" => Some(Self::Graceful),
            "forceful" | "kill" | "SIGKILL" => Some(Self::Forceful),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Graceful => "graceful",
            Self::Forceful => "forceful",
        }
    }
}

impl AppAction {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim() {
            "hide" => Self::Hide,
            "show" => Self::Show,
            "quit" => Self::Quit,
            "launch" => Self::Launch,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hide => "hide",
            Self::Show => "show",
            Self::Quit => "quit",
            Self::Launch => "launch",
        }
    }

    /// `quit` ends an application; the gate applies to it and to nothing
    /// else here.
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Quit)
    }
}

/// MCU `orderwin TARGET above|below RELATIVE`: above raises target, below
/// raises relative.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrderRelation {
    Above,
    Below,
}

impl OrderRelation {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "above" => Some(Self::Above),
            "below" => Some(Self::Below),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Above => "above",
            Self::Below => "below",
        }
    }
}

/// What an `invoke` action's `VALUE` positional must be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvokeValueKind {
    /// No value (`press`, `increment`, `decrement`).
    None,
    /// Free text (`set-value`, `select-option`).
    Text,
    /// `true` / `false` (`set-checked`, `set-expanded`).
    Flag,
}

impl InvokeAction {
    pub const ALL: [InvokeAction; 12] = [
        Self::Press,
        Self::SetValue,
        Self::SelectOption,
        Self::SetChecked,
        Self::SetExpanded,
        Self::Increment,
        Self::Decrement,
        Self::SetSelected,
        Self::SetSelection,
        Self::ScrollTo,
        Self::Cancel,
        Self::ShowDefaultUi,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|action| action.as_str() == raw.trim())
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::SetValue => "set-value",
            Self::SelectOption => "select-option",
            Self::SetChecked => "set-checked",
            Self::SetExpanded => "set-expanded",
            Self::Increment => "increment",
            Self::Decrement => "decrement",
            Self::SetSelected => "set-selected",
            Self::SetSelection => "set-selection",
            Self::ScrollTo => "scroll-to",
            Self::Cancel => "cancel",
            Self::ShowDefaultUi => "show-default-ui",
        }
    }

    pub fn value_kind(self) -> InvokeValueKind {
        match self {
            Self::Press
            | Self::Increment
            | Self::Decrement
            | Self::ScrollTo
            | Self::Cancel
            | Self::ShowDefaultUi => InvokeValueKind::None,
            Self::SetValue | Self::SelectOption | Self::SetSelection => InvokeValueKind::Text,
            Self::SetChecked | Self::SetExpanded | Self::SetSelected => InvokeValueKind::Flag,
        }
    }
}

/// One `verify --expect` / `wait --expect` item: a target (at least one of
/// `node`, `index`, `name`, `identifier`, `role`; `role` narrows `name` /
/// `identifier` or stands alone) plus the states to compare. The shape is
/// closed: an unknown key fails at parse time, so a misspelled state can
/// never pass by being ignored.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    /// Case-insensitive substring of the accessible name (showing nodes).
    /// MCU `titleIncludes` is the same field (AX title ≈ name).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "titleIncludes"
    )]
    pub name: Option<String>,
    /// Exact toolkit identifier (showing nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    /// Role in either spelling (`AXCheckBox` / `check-box`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Exact node `text` (the value `query` reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused: Option<bool>,
}

impl Expectation {
    pub fn has_target(&self) -> bool {
        self.node.is_some()
            || self.index.is_some()
            || self.name.is_some()
            || self.identifier.is_some()
            || self.role.is_some()
    }

    pub fn has_state(&self) -> bool {
        self.value.is_some()
            || self.checked.is_some()
            || self.expanded.is_some()
            || self.focused.is_some()
    }

    /// Page identity: a title substring is enough for wait/verify (MCU
    /// WebArea title / Heading alias). State fields remain optional.
    pub fn has_page_identity(&self) -> bool {
        self.name.is_some()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum Command {
    Capabilities {
        target: TargetRef,
    },
    /// Inspect or atomically publish the stable current-user launcher for the
    /// exact packaged `agenterm-cu` executable. Apply also refreshes future
    /// activation without disturbing resident resource owners; permission
    /// repair remains separate.
    Setup {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "SetupAction::is_apply")]
        action: SetupAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bin_dir: Option<String>,
    },
    /// Read-only projection of the permission declaration embedded in
    /// `capabilities`. This is a first-class wire command so current, SSH and
    /// VNC workers all receive the same stable shape.
    Permissions {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "PermissionAction::is_status")]
        action: PermissionAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission: Option<PermissionKind>,
    },
    /// Bounded read-only health report composed from the canonical
    /// capability/permission declarations plus live inventory probes.
    Doctor {
        target: TargetRef,
    },
    /// Read-only truth about ACU's on-demand coordinator and per-resource
    /// resident owners. This deliberately does not invent a global daemon.
    RuntimeStatus {
        target: TargetRef,
    },
    /// Read the exact current default-output device, volume and mute state.
    AudioStatus {
        target: TargetRef,
    },
    /// Prepare a short-lived default-output volume plan without mutation.
    AudioPlanVolume {
        target: TargetRef,
        volume: u8,
        ttl_seconds: u64,
    },
    /// Prepare a short-lived default-output mute plan without mutation.
    AudioPlanMuted {
        target: TargetRef,
        muted: bool,
        ttl_seconds: u64,
    },
    /// Apply one encoded exact-device audio plan with durable replay closure.
    AudioApply {
        target: TargetRef,
        request: String,
        approval: String,
    },
    /// List a bounded, optionally filtered native service inventory.
    ServiceList {
        target: TargetRef,
        scope: ServiceScope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        match_text: Option<String>,
        max: usize,
    },
    /// Resolve and inspect one service in the current native authority domain.
    ServiceStatus {
        target: TargetRef,
        scope: ServiceScope,
        name: String,
    },
    /// Prepare a short-lived exact service lifecycle plan without mutation.
    ServicePlan {
        target: TargetRef,
        scope: ServiceScope,
        name: String,
        operation: ServiceOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        definition: Option<String>,
        ttl_seconds: u64,
    },
    /// Apply one encoded service plan with durable replay closure.
    ServiceApply {
        target: TargetRef,
        request: String,
        approval: String,
    },
    /// Execute the legacy one-call lifecycle shape under caller-supplied
    /// request/session identity. The executor still uses the same typed plan,
    /// approval and durable service transaction internally.
    ServiceTransact {
        target: TargetRef,
        scope: ServiceScope,
        operation: ServiceOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        definition: Option<String>,
        ttl_seconds: u64,
    },
    /// Read the bounded current console-session inventory and lock state.
    LoginSessionStatus {
        target: TargetRef,
    },
    /// Prepare a short-lived, exact-session lock plan without performing it.
    LoginSessionPlanLock {
        target: TargetRef,
        ttl_seconds: u64,
    },
    /// Apply one encoded exact-session lock plan with durable replay closure.
    LoginSessionApplyLock {
        target: TargetRef,
        request: String,
        approval: String,
    },
    /// Ask the host's registered application dispatcher to open one path or
    /// URL. Native acceptance is not proof that a handler consumed it.
    HostOpen {
        target: TargetRef,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        application: Option<String>,
        #[serde(default)]
        background: bool,
    },
    HostNotify {
        target: TargetRef,
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        #[serde(default)]
        sound: bool,
    },
    /// Newest-first bounded read of the append-only control audit. Byte,
    /// record-scan and returned-result budgets are independent.
    AuditQuery {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verb_filter: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since_ms: Option<u128>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scan_max: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        byte_max: Option<usize>,
    },
    /// Plan or atomically apply bounded retention to the local control audit.
    AuditCompact {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_age_days: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_events: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<usize>,
        #[serde(default)]
        apply: bool,
    },
    SessionStart {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        ttl_seconds: u64,
    },
    SessionList {
        target: TargetRef,
    },
    SessionStatus {
        target: TargetRef,
        session_id: String,
    },
    SessionRenew {
        target: TargetRef,
        session_id: String,
        lease: String,
        ttl_seconds: u64,
    },
    SessionEnd {
        target: TargetRef,
        session_id: String,
        lease: String,
        confirm: bool,
    },
    LockAcquire {
        target: TargetRef,
        session_id: String,
        lease: String,
        lock_target: String,
        ttl_seconds: u64,
    },
    LockList {
        target: TargetRef,
    },
    LockRelease {
        target: TargetRef,
        lock_id: String,
        lease: String,
    },
    JobSpawn {
        target: TargetRef,
        #[serde(deserialize_with = "deserialize_job_command")]
        command: Vec<String>,
        #[serde(
            default,
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "deserialize_job_environment"
        )]
        environment: Vec<JobEnvironment>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_job_cwd"
        )]
        cwd: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_job_process_limits"
        )]
        limits: Option<JobProcessLimits>,
        #[serde(deserialize_with = "deserialize_job_ttl")]
        ttl_seconds: u64,
    },
    JobAdopt {
        target: TargetRef,
        pid: u32,
        start_identity: String,
        #[serde(deserialize_with = "deserialize_job_ttl")]
        ttl_seconds: u64,
        #[serde(default)]
        stop_on_expiry: bool,
        #[serde(default)]
        force: bool,
    },
    JobList {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<JobStateFilter>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_job_list_max"
        )]
        max: Option<usize>,
    },
    JobStatus {
        target: TargetRef,
        job_id: String,
    },
    JobPrune {
        target: TargetRef,
        max_age_seconds: u64,
        keep_newest: usize,
        #[serde(default, skip_serializing_if = "is_false")]
        apply: bool,
    },
    JobResources {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        watch_ms: Option<u64>,
    },
    JobPriority {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        nice: i32,
    },
    JobEvents {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        stdout_cursor: JobOutputCursor,
        stderr_cursor: JobOutputCursor,
        #[serde(deserialize_with = "deserialize_job_events_timeout")]
        timeout_ms: u64,
        #[serde(deserialize_with = "deserialize_job_events_max")]
        max_bytes: usize,
    },
    JobOutput {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        stream: JobOutputStream,
        cursor: JobOutputCursor,
        #[serde(deserialize_with = "deserialize_job_output_max")]
        max_bytes: usize,
    },
    JobWrite {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        #[serde(deserialize_with = "deserialize_job_write")]
        data_base64: String,
        #[serde(default, skip_serializing_if = "is_false")]
        close_stdin: bool,
    },
    JobWait {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        #[serde(deserialize_with = "deserialize_job_wait_timeout")]
        timeout_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_exit: Option<i32>,
    },
    JobSetState {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        state: ProcessRunState,
        #[serde(deserialize_with = "deserialize_job_control_timeout")]
        timeout_ms: u64,
    },
    JobSignal {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        signal: ProcessSignalKind,
        #[serde(deserialize_with = "deserialize_job_control_timeout")]
        timeout_ms: u64,
        #[serde(default, skip_serializing_if = "is_false")]
        force: bool,
    },
    JobStop {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        #[serde(deserialize_with = "deserialize_job_stop_grace")]
        grace_ms: u64,
        expect_stopped: bool,
    },
    JobRenew {
        target: TargetRef,
        job_id: String,
        #[serde(deserialize_with = "deserialize_job_generation")]
        generation: u64,
        #[serde(deserialize_with = "deserialize_job_ttl")]
        ttl_seconds: u64,
    },
    /// Top-level window inventory. Without any filter or page field the
    /// reply `data` is the plain window array (unchanged shape); with one,
    /// `data` is the inventory object `{windows, visited, matched, returned,
    /// offset, truncated}` so a filtered read carries its counts.
    Windows {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        /// Case-insensitive substring of `app_name`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        /// Case-insensitive substring of `title`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focused: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minimized: Option<bool>,
        /// Case-insensitive substring of the row's `browser_profile` (the
        /// Chromium profile name a browser window's identity carries);
        /// windows without one never match.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        browser_profile: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// Poll-diff over the `windows` inventory (`appeared` / `disappeared` /
    /// `changed`). Not AXObserver. `duration_ms == 0` takes one extra sample.
    WindowsWatch {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "is_zero_u64")]
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_events: Option<usize>,
    },
    /// Running apps derived from top-level windows. Installed-but-not-running
    /// is not mapped (`running_only` in the reply).
    Apps {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        running: bool,
        /// Also list applications that are installed but not running --
        /// the ones no window can reveal.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all: bool,
    },
    /// Bounded cross-platform process inventory. Rich filters are evaluated
    /// only over the explicitly bounded native inventory; CPU percentage uses
    /// a stated sampling interval instead of relabelling cumulative CPU time.
    Ps {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cpu_above_percent: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memory_above_mb: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sample_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_visited: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<usize>,
        #[serde(default, skip_serializing_if = "is_false")]
        files: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        ports: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        meta: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// Observe one exact process instance without changing it. The optional
    /// start identity is the portable evidence later process mutations must
    /// bind to so a recycled pid cannot become a different target.
    ProcessState {
        target: TargetRef,
        pid: u32,
    },
    /// Read one exact process's bounded argument vector. Values are opt-in;
    /// the default reply contains only index, UTF-8 byte length and digest.
    ProcessArgv {
        target: TargetRef,
        pid: u32,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        values: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Read the live working directory for one exact process instance. The
    /// path is disclosed by this explicit command, but persistent evidence
    /// stores only its byte length and digest.
    ProcessCwd {
        target: TargetRef,
        pid: u32,
    },
    /// Read the process's bounded exec-time environment snapshot. Values are
    /// opt-in because environment entries routinely contain credentials.
    ProcessEnvironment {
        target: TargetRef,
        pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        values: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Inspect one identity-bracketed descriptor snapshot. Result pagination
    /// and native scan completeness are reported independently.
    ProcessFds {
        target: TargetRef,
        pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_filter: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_visited: Option<usize>,
    },
    /// Inspect one identity-bracketed virtual-memory region snapshot.
    ProcessMaps {
        target: TargetRef,
        pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permissions: Option<String>,
        #[serde(default)]
        executable_only: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_visited: Option<usize>,
    },
    /// Inspect one identity-bracketed native-thread snapshot.
    ProcessThreads {
        target: TargetRef,
        pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_visited: Option<usize>,
    },
    /// Inspect one identity-bracketed native socket snapshot. Endpoint bytes
    /// remain lossless and scan completeness is independent from pagination.
    ProcessSockets {
        target: TargetRef,
        pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        family: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_visited: Option<usize>,
    },
    /// Inspect one Linux cgroup v2 membership through an identity-bound
    /// process object. Other hosts return a typed not-applicable result rather
    /// than projecting process groups or Job Objects onto Linux semantics.
    ProcessCgroup {
        target: TargetRef,
        pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_identity: Option<String>,
    },
    /// One cumulative resource sample for an exact, identity-bound process.
    ProcessUsage {
        target: TargetRef,
        pid: u32,
        /// When present, collect a bounded series instead of one sample.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        watch_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_samples: Option<usize>,
    },
    /// Wait for one previously observed process instance to exit. Requiring
    /// the start identity prevents a recycled pid from becoming a new target.
    ProcessWait {
        target: TargetRef,
        pid: u32,
        start_identity: String,
        timeout_ms: u64,
    },
    /// Terminate one previously observed process instance and verify that the
    /// same retained native object exited. `expect_exited` is deliberately
    /// explicit because this is a destructive operation.
    ProcessKill {
        target: TargetRef,
        pid: u32,
        start_identity: String,
        mode: ProcessKillMode,
        timeout_ms: u64,
        expect_exited: bool,
    },
    /// Suspend or resume one exact process object and verify its scheduler
    /// state without reopening a mutable PID as effect authority.
    ProcessSetState {
        target: TargetRef,
        pid: u32,
        start_identity: String,
        state: ProcessRunState,
        timeout_ms: u64,
    },
    /// Observe the exact Darwin background flags or request an identity-bound
    /// mutation. Mutation remains fail-closed unless the executor owns an
    /// exact native task object; it never falls back to a reusable PID.
    ProcessPolicy {
        target: TargetRef,
        pid: u32,
        action: ProcessPolicyAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_identity: Option<String>,
    },
    /// Deliver one closed signal through a retained native process object.
    /// An optional prior start identity tightens caller intent; the executor
    /// always binds and returns the live identity before reserving the effect.
    ProcessSignal {
        target: TargetRef,
        pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_identity: Option<String>,
        signal: ProcessSignalKind,
        timeout_ms: u64,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        tree: bool,
        #[serde(default = "default_process_tree_max_descendants")]
        max_descendants: usize,
    },
    /// Prepare a canonical, expiring, identity-bound process-priority plan.
    /// This command is observation-only: applying it belongs to a later
    /// consented privilege-provider contract.
    PrivilegePlanProcessPriority {
        target: TargetRef,
        pid: u32,
        nice: i32,
        ttl_seconds: u64,
    },
    /// Observe a bounded process-set lifecycle. Every row is keyed by pid and
    /// start identity so pid reuse becomes one exit plus one start instead of
    /// silently changing the watched object.
    ProcessWatch {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all: bool,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_events: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_processes: Option<usize>,
    },
    /// Run one bounded host-shell command in a process tree contained before
    /// its first instruction. This is intentionally distinct from the
    /// transport worker entry mode named `exec`.
    ShellExec {
        target: TargetRef,
        command: String,
        timeout_ms: u64,
        max_output_bytes: usize,
    },
    /// Enumerate a bounded, stable-sorted snapshot of native network
    /// interface addresses. Native ids identify this snapshot only; callers
    /// must not treat display names as durable lease identities.
    NetworkInterfaces {
        target: TargetRef,
        max: usize,
    },
    /// Enumerate a bounded, stable-sorted snapshot of the native route table.
    /// Missing cross-platform fields remain explicit rather than inferred.
    NetworkRoutes {
        target: TargetRef,
        max: usize,
    },
    /// Enumerate the effective native DNS resolvers and search domains. The
    /// reply names incomplete stub/file coverage rather than presenting it as
    /// the complete system resolver state.
    NetworkDns {
        target: TargetRef,
        max: usize,
    },
    /// Resolve once through the host resolver, freeze the deduplicated address
    /// set, and perform an exact number of bounded TCP reachability attempts.
    NetworkProbe {
        target: TargetRef,
        host: String,
        port: u16,
        attempts: u8,
        timeout_ms: u64,
    },
    /// Inspect one final filesystem entry without following a link-like final
    /// component. Wide counters are serialized losslessly by the executor.
    FileInspect {
        target: TargetRef,
        path: String,
    },
    /// Plan or apply one recoverable regular-file copy. Planning is
    /// observation-only; `apply` persists the recovery receipt before the
    /// first filesystem mutation.
    FileCopy {
        target: TargetRef,
        source: String,
        destination: String,
        #[serde(default, skip_serializing_if = "is_false")]
        replace: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        apply: bool,
    },
    /// Plan or apply one recoverable regular-file move: the hardened copy
    /// transaction followed by retiring the source into a same-directory
    /// backup that is kept until finalize. Planning is observation-only.
    FileMove {
        target: TargetRef,
        source: String,
        destination: String,
        #[serde(default, skip_serializing_if = "is_false")]
        replace: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        apply: bool,
    },
    /// Inspect or advance one previously reserved file-copy or file-move
    /// transaction; the durable receipt's operation selects the owner.
    FileTransaction {
        target: TargetRef,
        action: FileTransactionAction,
        transaction_id: String,
    },
    /// Start one durable, headless AgenTerm-owned PTY job. The human name
    /// deterministically selects an isolated logical server instance.
    PtyStart {
        target: TargetRef,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        command: Vec<String>,
    },
    /// List durable headless PTY job names and reconcile their state
    /// directories with the live authorities.
    PtyList {
        target: TargetRef,
    },
    /// Reclaim one exact named state directory only after independently
    /// proving that its deterministic authority is stale.
    PtyPrune {
        target: TargetRef,
        name: String,
        expect_stale: bool,
    },
    /// Observe one named headless PTY job and its exact epoch-scoped tab.
    PtyStatus {
        target: TargetRef,
        name: String,
    },
    /// Read one loss-aware raw-output page from a named headless PTY job.
    PtyRead {
        target: TargetRef,
        name: String,
        cursor: String,
        max_bytes: usize,
    },
    /// Observe one bounded structured screen and event cursor for the sole tab
    /// owned by an exact durable PTY job.
    PtySnapshot {
        target: TargetRef,
        name: String,
    },
    /// Compare the current structured screen with one persisted, exact-job
    /// baseline and optionally publish the current screen as the next base.
    PtyDiff {
        target: TargetRef,
        name: String,
        base: String,
        advance: bool,
        max: Option<usize>,
    },
    /// Continue the loss-aware event journal for the sole tab owned by an
    /// exact durable PTY job.
    PtyEvents {
        target: TargetRef,
        name: String,
        epoch: String,
        after: u64,
        limit: usize,
    },
    /// Resize the sole terminal owned by an exact durable PTY job and verify
    /// the resulting grid through the same authority.
    PtyResize {
        target: TargetRef,
        name: String,
        rows: u16,
        columns: u16,
    },
    /// Send exact UTF-8 bytes to one named headless PTY job.
    PtySend {
        target: TargetRef,
        name: String,
        text: String,
    },
    /// Wait for exact UTF-8 bytes in one job's loss-aware retained output.
    PtyWait {
        target: TargetRef,
        name: String,
        contains: String,
        cursor: String,
        timeout_ms: u64,
    },
    /// Wait for one named job's terminal reader to finalize and optionally
    /// require an exact process exit status.
    PtyWaitExit {
        target: TargetRef,
        name: String,
        timeout_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_status: Option<i32>,
    },
    /// Signal the native foreground process group selected by the sole
    /// retained PTY master, with an explicit signal-specific postcondition.
    PtySignal {
        target: TargetRef,
        name: String,
        signal: PtySignalKind,
        expect: String,
    },
    /// Close the exact job tab and shut down its otherwise-empty authority.
    PtyStop {
        target: TargetRef,
        name: String,
        expect_stopped: bool,
    },
    /// Inventory AgenTerm-owned tabs using stable epoch-scoped `@N` ids.
    TerminalList {
        target: TargetRef,
    },
    /// Create one AgenTerm-owned tab and return its stable epoch-scoped id.
    TerminalNew {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        detached: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        command: Vec<String>,
    },
    /// Close one exact AgenTerm-owned tab after explicit close intent.
    TerminalClose {
        target: TargetRef,
        tab: String,
        expect_closed: bool,
    },
    /// Read one bounded current-screen snapshot. This is deliberately not an
    /// incremental byte-stream cursor.
    TerminalRead {
        target: TargetRef,
        tab: String,
        max_bytes: usize,
    },
    /// Read one bounded structured screen snapshot together with the exact
    /// server epoch/event cursor from which delta observation can continue.
    TerminalSnapshot {
        target: TargetRef,
        tab: String,
    },
    /// Mutate only the local scrollback viewport of one exact AgenTerm tab.
    /// This is not terminal input and is refused on the alternate screen.
    TerminalScroll {
        target: TargetRef,
        tab: String,
        action: TerminalScrollAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rows: Option<usize>,
    },
    /// Capture the already-active exact AgenTerm tab as a rendered PNG.
    /// The product publishes with no-clobber semantics and returns frame
    /// identity; this observe command never selects or activates a tab.
    TerminalScreenshot {
        target: TargetRef,
        tab: String,
        out: String,
    },
    /// Read one bounded, loss-aware event page after an explicit server
    /// epoch/sequence cursor. Events for other tabs are scanned so the cursor
    /// can advance, but are not exposed as terminal events for this tab.
    TerminalEvents {
        target: TargetRef,
        tab: String,
        epoch: String,
        after: u64,
        limit: usize,
    },
    /// Read retained raw terminal bytes from an explicit absolute stream
    /// cursor, or bootstrap at the earliest/current retained position.
    TerminalOutput {
        target: TargetRef,
        tab: String,
        cursor: String,
        max_bytes: usize,
    },
    /// Send exact UTF-8 bytes to one AgenTerm-owned tab.
    TerminalSend {
        target: TargetRef,
        tab: String,
        text: String,
    },
    /// Wait without a fixed sleep for one screen or lifecycle condition.
    TerminalWait {
        target: TargetRef,
        tab: String,
        condition: TerminalWaitCondition,
        timeout_ms: u64,
    },
    /// Read one exact external terminal window through its accessibility
    /// text buffer. A desktop window identity is not an AgenTerm `@tab`.
    TermRead {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tail: Option<usize>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        raw: bool,
        #[serde(default = "default_term_max_bytes")]
        max_bytes: usize,
    },
    /// Send to one exact external terminal. Background delivery must be
    /// independently observed; `foreground` explicitly permits a bounded
    /// activate, inject and previous-focus restore transaction.
    TermSend {
        target: TargetRef,
        window: isize,
        text: String,
        /// Independent postcondition. When absent, the literal text itself
        /// must newly appear in the selected terminal buffer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default = "default_term_enter")]
        enter: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        foreground: bool,
        #[serde(default = "default_term_send_timeout_ms")]
        verify_timeout_ms: u64,
    },
    /// Wait for a regular-expression match in one exact external terminal.
    /// Timeout diagnostics carry hashes and lengths, never terminal content.
    TermWait {
        target: TargetRef,
        window: isize,
        pattern: String,
        #[serde(default = "default_term_wait_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "default_term_wait_interval_ms")]
        interval_ms: u64,
        #[serde(default = "default_term_max_bytes")]
        max_bytes: usize,
    },
    /// Bounded control-tree observation. `depth` (root = 0) and `max_nodes`
    /// apply while the platform adapter walks the backend; the reply reports
    /// `truncated` / `visited` / `returned`. `flat` lists the same nodes in
    /// the same order with a `depth` and a flatten `index` per node — the
    /// numbering a later `invoke --index` addresses.
    Tree {
        target: TargetRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flat: bool,
    },
    /// One bounded desktop observation composed from a window inventory, one
    /// window-scoped accessibility tree, and the pointer position.  The
    /// selected window is re-enumerated after the tree read; a changed target
    /// fails closed instead of returning a mixed-instant snapshot.
    DesktopState {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
    },
    /// Bounded, filtered flat node list over the same walk `tree` makes
    /// (same node ids and flatten indices). Filters: `role` (comma list;
    /// `AXTextArea` and `text-area` both match), `text` (case-insensitive
    /// substring of name or text) or `text_exact`, `identifier` (exact),
    /// `actionable` (at least one action), `within` (bounds intersect
    /// `[x, y, w, h]`). `offset` / `max` page the matches. The reply reports
    /// `visited / matched / returned / truncated`.
    Query {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        role: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_exact: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identifier: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        actionable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        within: Option<[i32; 4]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
        /// MCU path: `Role[idx] / Role@title / *@title / #description`.
        /// Scopes the query to that node and its descendants.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
    },
    /// One semantic action on one node of `window` through the platform
    /// a11y backend, never activating or raising the window. Exactly one of
    /// `node` (path id), `index` (flatten index) or `name` [+ `role`] /
    /// `identifier` addresses the target; two or more showing matches are
    /// `ambiguous`, none is `a11y_node_not_found`, an action the node does
    /// not offer is `unsupported`. The reply carries `verified` (the
    /// postcondition was read back) and a receipt (target, node, action,
    /// before / after state).
    Invoke {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identifier: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        action: InvokeAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        /// Address the application's own focused control (the node
        /// `focused` reports) instead of naming one; `role` may narrow it.
        /// PID, window and focused identity are bound in one observation
        /// before the action.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        focused: bool,
        /// MCU `Role[idx] / Role@title` walk; exclusive of --node/--index/--name/--identifier/--focused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
    },
    /// Background menu-bar inventory of the application owning `window`
    /// (macOS `AXMenuBar`, read without opening a menu or activating the
    /// app). `depth` counts menu levels (0 = bar items only, 1 = their
    /// items, default 1, at most 8); `max_nodes` bounds the walk
    /// (1..=5000). `title` is a case-insensitive substring unless `exact`;
    /// `enabled` filters on the item state. `offset` / `max` page the
    /// items; the reply reports `visited / matched / returned / truncated`.
    MenuInspect {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        exact: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// Press the menu item at `path` (menu title, then item titles, exact)
    /// in the background: every segment must resolve to exactly one
    /// enabled item before anything is pressed, the last must be a leaf,
    /// and the reply carries `verified` (tree diff / mark read-back).
    MenuInvoke {
        target: TargetRef,
        window: isize,
        path: Vec<String>,
    },
    /// The application's own focused control inside `window` (identity,
    /// role, value preview), read without requiring the foreground. `role`
    /// binds the expected role (mismatch is typed `unverified`);
    /// `max_value_bytes` bounds the value preview (default 4096).
    Focused {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_value_bytes: Option<usize>,
    },
    /// A bounded, filtered event stream over the same bounded tree for
    /// `duration_ms`: the tree is polled every `interval_ms` and diffed
    /// (ValueChanged / TitleChanged / StateChanged / FocusChanged /
    /// Created / Destroyed), events carry a monotonic `seq` and `t_ms`,
    /// and the stream stops at `max_events` with `truncated: true`.
    Observe {
        target: TargetRef,
        window: isize,
        duration_ms: u64,
        /// Optional caller-owned readiness marker. For `poll-diff`, it is
        /// atomically published only after the complete baseline walk, so a
        /// concurrent actuator can change the UI without racing that walk.
        /// The caller owns removal. Native-notification mode rejects it until
        /// its subscription layer can expose the same ordering guarantee.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ready_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_events: Option<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        notifications: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval_ms: Option<u64>,
        /// `poll-diff` (default) or `notifications`. The two see different
        /// things and neither subsumes the other: polling compares two
        /// tree walks, so every event carries `before` and `after` but a
        /// change that reverts between walks is invisible; the backend's
        /// own notifications carry the order and arrival time of every
        /// change but not what it changed from. The caller picks; the
        /// reply always says which one ran.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
    },
    /// Read one tree and check every expectation against it. All met is
    /// `ok` with per-item results; a known mismatch is typed `unverified`;
    /// a state the node does not expose is typed `unsupported` (fail
    /// closed, never "probably fine").
    Verify {
        target: TargetRef,
        window: isize,
        expect: Vec<Expectation>,
    },
    Screenshot {
        target: TargetRef,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
    },
    /// Capture one frame from a physically connected device source published
    /// by the host capture stack. This is independent of desktop/window
    /// screenshots and carries the host Camera authorization in inventory
    /// replies, including when no source is published.
    DeviceScreenshot {
        target: TargetRef,
        /// Absent only for inventory mode.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Exact published source name or uid; optional for a sole source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        #[serde(default)]
        list: bool,
    },
    ResourceStatus {
        target: TargetRef,
    },
    PowerStatus {
        target: TargetRef,
    },
    StorageDevices {
        target: TargetRef,
        #[serde(deserialize_with = "deserialize_storage_devices_max")]
        max: usize,
    },
    DeviceList {
        target: TargetRef,
        selector: DeviceInventorySelector,
        #[serde(deserialize_with = "deserialize_device_inventory_max")]
        max: usize,
    },
    DeviceWatch {
        target: TargetRef,
        selector: DeviceInventorySelector,
        #[serde(deserialize_with = "deserialize_device_inventory_max")]
        max: usize,
        #[serde(deserialize_with = "deserialize_device_watch_duration_ms")]
        duration_ms: u64,
        #[serde(deserialize_with = "deserialize_device_watch_interval_ms")]
        interval_ms: u64,
        #[serde(deserialize_with = "deserialize_device_watch_event_max")]
        event_max: usize,
    },
    DeviceClaims {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    DeviceClaim {
        target: TargetRef,
        device_id: String,
        ttl_seconds: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        serial: Option<DeviceSerialConfiguration>,
    },
    DeviceStatus {
        target: TargetRef,
        lease_id: String,
        generation: u64,
    },
    DeviceRead {
        target: TargetRef,
        lease_id: String,
        generation: u64,
        lease: String,
        max_bytes: usize,
        timeout_ms: u64,
        encoding: DeviceDataEncoding,
    },
    DeviceWrite {
        target: TargetRef,
        lease_id: String,
        generation: u64,
        lease: String,
        data: String,
        encoding: DeviceDataEncoding,
        timeout_ms: u64,
    },
    DeviceRenew {
        target: TargetRef,
        lease_id: String,
        generation: u64,
        lease: String,
        ttl_seconds: u64,
    },
    DeviceRelease {
        target: TargetRef,
        lease_id: String,
        generation: u64,
        lease: String,
    },
    SimulatorDevices {
        target: TargetRef,
        #[serde(deserialize_with = "deserialize_simulator_max")]
        max: usize,
    },
    SimulatorBoot {
        target: TargetRef,
        #[serde(deserialize_with = "deserialize_simulator_udid")]
        udid: String,
        #[serde(deserialize_with = "deserialize_simulator_timeout_ms")]
        timeout_ms: u64,
        #[serde(deserialize_with = "deserialize_true")]
        expect_booted: bool,
    },
    SimulatorApps {
        target: TargetRef,
        #[serde(deserialize_with = "deserialize_simulator_udid")]
        udid: String,
        #[serde(deserialize_with = "deserialize_simulator_max")]
        max: usize,
    },
    SimulatorLaunch {
        target: TargetRef,
        #[serde(deserialize_with = "deserialize_simulator_udid")]
        udid: String,
        #[serde(deserialize_with = "deserialize_simulator_bundle_id")]
        bundle_id: String,
        #[serde(deserialize_with = "deserialize_simulator_timeout_ms")]
        timeout_ms: u64,
        #[serde(deserialize_with = "deserialize_true")]
        expect_accepted: bool,
    },
    SimulatorTerminate {
        target: TargetRef,
        #[serde(deserialize_with = "deserialize_simulator_udid")]
        udid: String,
        #[serde(deserialize_with = "deserialize_simulator_bundle_id")]
        bundle_id: String,
        #[serde(deserialize_with = "deserialize_simulator_timeout_ms")]
        timeout_ms: u64,
        #[serde(deserialize_with = "deserialize_true")]
        expect_accepted: bool,
    },
    /// Move the pointer to absolute target-session screen coordinates without
    /// pressing, releasing, clicking, dragging, or scrolling any button.
    PointerMove {
        target: TargetRef,
        x: i32,
        y: i32,
    },
    /// Observe the pointer's current absolute target-session screen
    /// coordinates without injecting input.
    PointerPosition {
        target: TargetRef,
    },
    Click {
        target: TargetRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        /// Accessible-name substring; resolved with the same showing/visible
        /// matcher as `WaitCondition::NodeNameContains` (exactly one match),
        /// then acted via `--node`. Two or more showing hits are
        /// `a11y_node_ambiguous`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        coords: Option<[i32; 2]>,
        #[serde(default)]
        degraded: bool,
        #[serde(default = "default_clicks")]
        clicks: u32,
        #[serde(default)]
        button: PointerButton,
    },
    Focus {
        target: TargetRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    SendText {
        target: TargetRef,
        text: String,
        /// Optional window scope. With `--name`, name-address the unique
        /// showing node. Without `--name`, write the showing focused node
        /// (same innermost Text candidate as `GetText` without `--name`).
        /// Neither flag keeps the plain focused inject.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        /// `--allow-browser-chrome`: with `window` and no `name`, write the
        /// focused control even when it is browser chrome (omnibox, toolbar,
        /// tab strip) instead of refusing `focused_node_is_browser_chrome`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_browser_chrome: bool,
    },
    /// Read the target session's clipboard. Without `type_name` this is
    /// Unicode text plus the host type list. With `type_name` it is one
    /// native type as bounded bytes (MCU `clipboard read`).
    ClipboardRead {
        target: TargetRef,
        /// Inspect native clipboard formats without reading their payloads.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        metadata_only: bool,
        #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
        type_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        out: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        replace: bool,
    },
    /// MCU `clipboard write <type> <file>`: publish one native type from a
    /// regular file (≤16 MiB) and read it back.
    ClipboardWrite {
        target: TargetRef,
        #[serde(rename = "type")]
        type_name: String,
        path: String,
    },
    /// MCU `clipboard write-file <path>`: put a file reference on the
    /// clipboard, not the file's bytes.
    ClipboardWriteFile {
        target: TargetRef,
        path: String,
    },
    /// MCU `clipboard clear`: empties the clipboard. Without `apply` this
    /// is a planned no-op.
    ClipboardClear {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        apply: bool,
    },
    /// Copy AT-SPI `Text.GetText` onto the native clipboard
    /// (`agt_clipboard_set_text`). With `--name`, the unique showing named
    /// node. With `--window` and no `--name`, the showing focused node
    /// (same innermost Text candidate as `GetText` without `--name`).
    /// Never XTest / `--coords` / screenshot when `--window` is set. A
    /// node with no Text interface typed-fails (`a11y_text_unavailable`).
    Copy {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Write clipboard text via native AT-SPI `EditableText` / `Text`.
    /// With `--name`, the unique showing named field. With `--window` and
    /// no `--name`, the showing focused node (same innermost Text
    /// candidate as `GetText` without `--name`). `--text` only seeds the
    /// clipboard; the field write always reads the clipboard. Never XTest
    /// / `--coords` when `--window` is set.
    Paste {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        /// `--allow-browser-chrome`: with `window` and no `name`, write the
        /// focused control even when it is browser chrome (omnibox, toolbar,
        /// tab strip) instead of refusing `focused_node_is_browser_chrome`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_browser_chrome: bool,
    },
    SendKeys {
        target: TargetRef,
        keys: String,
        /// Optional window scope. With `--name`, name-address the unique
        /// showing node and deliver Device/key events. Without `--name`,
        /// target the showing focused node (same innermost Text candidate
        /// as `GetText` without `--name`). Neither flag keeps the plain
        /// focused inject.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        /// `--allow-browser-chrome`: with `window` and no `name`, write the
        /// focused control even when it is browser chrome (omnibox, toolbar,
        /// tab strip) instead of refusing `focused_node_is_browser_chrome`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_browser_chrome: bool,
    },
    /// One-shot AT-SPI `Component.ScrollTo(TopEdge)` on the unique showing
    /// named node. Success is `via=scroll-to`. Missing / false /
    /// `UnknownMethod` typed-fails (`a11y_scroll_unavailable`). Never
    /// Action `scroll*`, XTest wheel, `--coords`, or screenshot.
    Scroll {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Independent AT-SPI `Component.GetExtents(Screen)` for the unique
    /// showing named node. Snapshot `node.bounds` do not count. Empty
    /// extents typed-fail (`a11y_extents_unavailable`).
    GetExtents {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// One-shot AT-SPI `Text.SetSelection(0, start, end)` on the unique
    /// showing named node. Success is `via=set-selection`. Missing Text /
    /// `UnknownMethod` typed-fails (`a11y_selection_unavailable`).
    /// SetSelection false typed-fails (`a11y_selection_no_effect`). Never
    /// XTest, mouse-drag, `--coords`, or screenshot. The reply is not
    /// proof; callers observe via `get-selection`.
    Select {
        target: TargetRef,
        start: i32,
        end: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Independent AT-SPI `Text.GetNSelections` + `GetSelection(0)` for
    /// the unique showing named node. Not the `select` reply payload.
    /// Missing Text typed-fails (`a11y_selection_unavailable`). `n == 0`
    /// is empty success.
    GetSelection {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// One-shot AT-SPI `Text.SetCaretOffset` on the unique showing named
    /// node. Success is `via=set-caret-offset`. Missing Text /
    /// `UnknownMethod` typed-fails (`a11y_caret_unavailable`).
    /// SetCaretOffset false typed-fails (`a11y_caret_no_effect`). Never
    /// XTest, `--coords`, or screenshot. The reply is not proof; callers
    /// observe via `get-caret`.
    SetCaret {
        target: TargetRef,
        offset: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Independent AT-SPI `Text.CaretOffset` / `GetCaretOffset` for the
    /// unique showing named node. Not the `set-caret` reply payload.
    /// Missing Text typed-fails (`a11y_caret_unavailable`).
    GetCaret {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// One-shot independent AT-SPI `Text.GetText` for the unique showing
    /// named node, or — with no `name` — for the node carrying the AT-SPI
    /// `focused` state. Not a `wait --text-equals` poll and not `send-text` /
    /// `paste` / `copy` `matched.text`, `last_text_write_via`, the WebKit
    /// eval helper's queued-job `OK`, or a tree snapshot `text`. Missing
    /// Text typed-fails (`a11y_text_unavailable`). Never XTest / `--coords`.
    GetText {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    Wait {
        target: TargetRef,
        timeout_ms: u64,
        #[serde(flatten)]
        condition: WaitCondition,
    },
    /// `frame` (PRD_02_32, absorbed from `moltbaby/skills/mcu`, slice 4)
    /// is one more closed action id: `--action frame --x X --y Y --width W
    /// --height H` replaces the catalog geometry step with the requested
    /// rect and rides the same preflight / apply / read-back / history
    /// transaction. `frame` is required for that action and refused for
    /// every other.
    WindowPlace {
        target: TargetRef,
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<[i32; 4]>,
    },
    /// MCU `orderwin`: relative z-order. `above` raises `window`, `below`
    /// raises `relative`, through native show / macOS AXRaise. Linux is
    /// typed unsupported (window-op not wired).
    #[serde(rename = "orderwin")]
    OrderWin {
        target: TargetRef,
        window: isize,
        relation: OrderRelation,
        relative: isize,
    },
    /// Application-level lifecycle for the app owning `window`.
    ///
    /// `hide` / `show` step the whole application aside and back, which is
    /// neither minimizing a window nor closing one. `quit` is destructive
    /// and carries the same three-part gate as `close`: it presses the
    /// application's own Quit menu item and reads the process back.
    App {
        target: TargetRef,
        window: isize,
        action: AppAction,
        /// `quit` only: the prior bounded snapshot the gate requires.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        snapshot: bool,
        /// `quit` only: the checkable postcondition (`gone`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        /// `quit` only: the pid the window must belong to, bound in the
        /// same inventory read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        /// `launch` only: the application to start, as `apps --all` lists
        /// it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// macOS managed Space inventory (SkyLight read SPI). Linux/Windows typed.
    Spaces {
        target: TargetRef,
    },
    /// MCU `displays`: native screen inventory (`agt_screen_list`).
    Displays {
        target: TargetRef,
    },
    /// The destructive verb (PRD_02_31): close one top-level window in the
    /// background through the platform's own close control (macOS
    /// `AXCloseButton` + `AXPress`, Windows `WM_CLOSE`). The three-part gate
    /// is checked before anything is touched: an exact target (`window`,
    /// optionally bound to `pid` / exact `title` in the same inventory
    /// read), a prior `snapshot` (the bounded tree of the window, written
    /// to the reserved receipt) and a checkable postcondition (`expect:
    /// "gone"`, read back from the window inventory). Missing any of the
    /// three is typed `refused` (`detail.reason = destructive_gate`) with
    /// nothing performed.
    Close {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        snapshot: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
    },
    /// Read back the crash-persistent receipt file of this target (the
    /// `reserved` / `completed` / `failed` lines every actuation appends
    /// before it returns): newest last, filtered by `window`, at most `max`
    /// lines (default 50, ceiling 1000).
    Receipts {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// Page JavaScript second knife: CDP `Runtime.evaluate` on
    /// `--remote-debugging-port` (default 9222). MAIN-world Function
    /// constructor is never used. At most one of `target_id` (exact CDP
    /// id), `target_url` / `target_title` (case-insensitive substring)
    /// picks the page target; none keeps the first page. Evaluation
    /// reaches background tabs without selecting or raising anything. A
    /// returned Promise is awaited under the bounded CDP call deadline;
    /// rejection and timeout are typed failures.
    PageJs {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expression: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
    },
    /// The CDP target inventory (`/json`) on `port`: id, url, title, type,
    /// attached, and whether a websocket is offered. No listener is typed
    /// `unsupported`, the same as `page-js`.
    PageTargets {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        /// When set, only the targets whose title equals (exactly) a tab
        /// title of a window whose `browser_profile` contains this
        /// substring are returned, each marked `profile_match: "title"`.
        /// A heuristic: CDP targets carry no profile field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        browser_profile: Option<String>,
    },
    /// The visible text of `window` in reading order, shaped from the
    /// accessibility tree: compact rows of `id`, `role`, `text`, `bounds`
    /// so the next step is `invoke --node` / `click --node`, never a
    /// screenshot. Bounded by `max_bytes` (default 16 KiB), optionally by
    /// a screen rectangle (`within`), and by the walk budget (`depth` /
    /// `max_nodes`, defaults 64 / 6000: deeper and wider than the
    /// platform's own, because a breadth-first walk spends 1000 nodes on
    /// browser chrome before it reaches web content).
    ///
    /// Without `window`, the same rows come from CDP instead: `port` /
    /// `target_id` / `target_url` / `target_title` pick one page target
    /// (a background tab in a background window included) and the rows
    /// are shaped from `Accessibility.getFullAXTree` (fallback: a DOM
    /// `innerText` walk), `backend: "cdp"`, `focus_changed: false`. The
    /// row `id` is then the backend DOM node id `page click --node` /
    /// `page fill --node` take. One backend per call: `window` with a
    /// CDP selector is `invalid_input`.
    PageText {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        within: Option<[i32; 4]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
    },
    /// `page find` over CDP: the nodes of one page target (background
    /// tabs included) that a CSS `selector`, a `text` substring, or a
    /// `role` (+ `name` substring) names -- exactly one of the three --
    /// each with its backend node id, a selector-ish path, role, name,
    /// text, value and layout box. Zero matches is `cdp_node_not_found`.
    /// Nothing is activated.
    PageFind {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// `page click` over CDP: either resolve exactly one node (`selector` /
    /// `text` / backend `node` id; more than one is `cdp_node_ambiguous`
    /// with candidates) or freeze one explicit viewport point, then dispatch
    /// mouse pressed + released through
    /// `Input.dispatchMouseEvent` on that target -- the tab and window
    /// stay where they are. Verified by reading the document and the
    /// node back; a receipt is written.
    PageClick {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<f64>,
        /// `left` (default) | `right` | `middle`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<String>,
        /// 1 (default) ..= 3.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        clicks: Option<u32>,
    },
    /// Start one browser download from a background page node, wait for
    /// Chromium's browser-level completion event, and prove the GUID-named
    /// regular file exists without reading its contents.
    PageDownload {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<u64>,
        download_dir: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u64>,
    },
    /// `page hover` over CDP: move the page pointer to viewport CSS
    /// coordinates without selecting the tab. The postcondition checks a
    /// trusted `mousemove` event's target and coordinates; CSS `:hover` is
    /// auxiliary because a headless/background target may not maintain it.
    PageHover {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        x: f64,
        y: f64,
    },
    /// `page scroll` over CDP: wheel at viewport CSS coordinates and read
    /// the chosen scroll container's offsets back. At a scroll boundary the
    /// event is performed but unverified.
    PageScroll {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        x: f64,
        y: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dx: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dy: Option<f64>,
    },
    /// `page drag` over CDP: hold the left button from one viewport CSS
    /// point to another and verify the trusted down/held-move/up sequence.
    PageDrag {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    /// `page dialog` waits for one real JavaScript dialog on an exact CDP
    /// target, accepts/dismisses it, and verifies the close event. Message and
    /// prompt contents are redacted from receipts.
    PageDialog {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        #[serde(default)]
        dismiss: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u64>,
    },
    /// `page files` over CDP: bind one or more browser-host local regular
    /// files to an exact input[type=file] and verify the resulting FileList.
    /// Receipts retain basename/size only, never local paths.
    PageFiles {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<u64>,
        files: Vec<String>,
    },
    /// `page fill` over CDP: focus one editable node (`selector` / backend
    /// `node` id) with `DOM.focus`, optionally select-all (`clear`),
    /// `Input.insertText` the text, read `.value` back; `submit` then
    /// dispatches Enter key events. Verified when the read-back equals
    /// the text (`clear`) or grew by exactly the text. Focus emulation
    /// makes the unfocused page accept the write; the tab is never
    /// brought to the front. A receipt is written.
    PageFill {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<u64>,
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clear: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        submit: bool,
    },
    /// Insert text at the page target's already-focused editable element.
    /// The element identity and old value are frozen before the receipt;
    /// plaintext values and inserted text are never persisted.
    PageType {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        text: String,
    },
    /// `page nav` over CDP: `Page.navigate` on that target (a background
    /// tab stays background), then wait up to `wait_ms` (default 10 s)
    /// for `Page.loadEventFired`; verified with the final url / title.
    /// A receipt is written.
    PageNav {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait_ms: Option<u64>,
    },
    /// `page screenshot` over CDP: `Page.captureScreenshot` of that
    /// target written as PNG to `out` (`replace` to overwrite). Chromium
    /// may refuse a background / occluded tab, which is typed
    /// `cdp_screenshot_unavailable`; only `activate` (an actuation) runs
    /// `Page.bringToFront` first and replies `focus_changed: true`.
    PageScreenshot {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_match: Option<String>,
        out: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        replace: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        activate: bool,
    },
    /// The browser tab strip of `window` read through the accessibility
    /// tree: each tab's index, title and selected state. macOS Chromium
    /// lists background tabs only here (as `radio-button` rows of the
    /// `tab-group`); their content is not in the tree.
    TabList {
        target: TargetRef,
        window: isize,
    },
    /// Make one tab of `window` the active one by pressing its tab-strip
    /// row in the background (never raises or activates the window).
    /// Exactly one of `title` (case-insensitive substring) / `index`
    /// (0-based strip order, as `tab list` numbers it). No such tab is
    /// `a11y_tab_not_found`, two title hits `a11y_tab_ambiguous`; verified
    /// by reading the `selected` state back.
    TabSelect {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// Close one tab of `window` through the tab-strip row's own close
    /// button (the child `button` of the Chromium tab `radio-button`).
    /// Destructive, so gated like `close`: an exact tab identity (`title`
    /// with `exact`), the strip snapshot the receipt carries, and the
    /// postcondition `expect == "gone"` (the title is read back as absent
    /// from the strip). `index` is the exact alternative to `title` for
    /// same-title duplicates (0-based, `tab list` order). A background
    /// tab whose row offers no close button is selected first, closed,
    /// and the previous selection is restored (`selection_restored`);
    /// with `port` the tab is closed by `Target.closeTarget` instead when
    /// its title names exactly one page target of the instance. A
    /// keyboard shortcut is never substituted.
    TabClose {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        exact: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
    },
    /// The profiles of one Chromium-family application's user data
    /// directory (`Local State` -> `profile.info_cache`), each joined to
    /// the windows of the inventory whose `browser_profile` is that name.
    /// `app` is a catalog substring (Brave Origin / Brave Browser / Google
    /// Chrome); absent, the one running catalog application.
    BrowserProfiles {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
    },
    /// Open a window (or, with `url`, a tab) of the profile named
    /// `profile` in the running instance: `open -na <app> --args
    /// --profile-directory=<dir> [url]`, then poll the window inventory
    /// (bounded by `timeout_ms`, default 8000) until a window of that
    /// profile appears that was not there before, or -- when the profile
    /// already had a window and a URL was given -- until that window's
    /// title changes. The browser is never quit or restarted.
    BrowserOpen {
        target: TargetRef,
        profile: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    /// Start one named, bounded browser automation session using the exact
    /// browser executable path supplied by the caller.
    BrowserSessionStart {
        target: TargetRef,
        name: String,
        browser: String,
        /// Materialize and load the fixed ACU MV3 bridge in this isolated
        /// profile. The live connection remains the proof of activation.
        #[serde(default, skip_serializing_if = "is_false")]
        bridge: bool,
        ready_timeout_ms: u64,
        ttl_ms: u64,
    },
    /// List the browser sessions owned by this computer-use runtime.
    BrowserSessionList {
        target: TargetRef,
    },
    /// Observe one named browser session.
    BrowserSessionStatus {
        target: TargetRef,
        name: String,
    },
    /// Stop one named browser session and verify the stopped postcondition.
    BrowserSessionStop {
        target: TargetRef,
        name: String,
        expect_stopped: bool,
        timeout_ms: u64,
    },
    /// Remove one named browser session whose exact terminal state the caller
    /// acknowledges. Failed starts are removable only after both recorded
    /// processes are independently observed absent.
    BrowserSessionRemove {
        target: TargetRef,
        name: String,
        expect_stopped: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        expect_failed: bool,
    },
    /// Materialize the fixed ACU MV3 bundle and register this exact executable
    /// as its current-user Native Messaging host. Chromium still requires the
    /// user to load the unpacked extension; setup never claims that activation.
    BrowserBridgeSetup {
        target: TargetRef,
    },
    /// List bounded, current-user and exact-process-validated bridge hosts.
    BrowserBridgeConnections {
        target: TargetRef,
    },
    BrowserBridgeStatus {
        target: TargetRef,
        connection_id: ConnectionId,
    },
    BrowserBridgeTabs {
        target: TargetRef,
        connection_id: ConnectionId,
    },
    /// Return bounded Chromium window state for one exact live bridge
    /// connection without changing browser focus or activation.
    BrowserBridgeWindows {
        target: TargetRef,
        connection_id: ConnectionId,
    },
    /// Create one exact ordinary Chromium window through its exact extension
    /// connection and verify the requested focus effect.
    BrowserBridgeWindowOpen {
        target: TargetRef,
        connection_id: ConnectionId,
        url: String,
        #[serde(default, skip_serializing_if = "is_false")]
        focused: bool,
    },
    /// Change one exact Chromium window state through its exact extension
    /// connection. The bridge verifies state and browser focus postconditions.
    BrowserBridgeWindowState {
        target: TargetRef,
        connection_id: ConnectionId,
        window_id: u32,
        state: crate::browser_bridge::BrowserWindowState,
    },
    BrowserBridgeDebugRead {
        target: TargetRef,
        connection_id: ConnectionId,
        tab_id: u32,
        max_frames: u16,
        max_depth: u8,
        max_scan: u32,
        max_results: u16,
    },
    /// Re-read the window tree and report `ax` / `next_actions`.
    /// AXManualAccessibility poke is not mapped; empty-chrome is not an empty page.
    Unlock {
        target: TargetRef,
        window: isize,
    },
    /// `activate`: make one exact top-level window the desktop foreground
    /// owner, then independently read the inventory's focused mark back.
    ///
    /// This is MCU's whole-window `focus <handle>` meaning. It is distinct
    /// from [`Command::Focus`] (one accessibility node) and [`Command::Raise`]
    /// (application-local stacking without foreground activation).
    Activate {
        target: TargetRef,
        window: isize,
    },
    /// `raise`: lift one window inside its **own application's** z-order
    /// (macOS `AXRaise` on the window element) without activating the
    /// application and without changing the system frontmost application.
    ///
    /// Distinct from `Focus`, which gives one accessibility *node* inside a
    /// window the keyboard focus and never touches stacking. `raise` moves
    /// a whole window in front of its siblings and never moves the
    /// accessibility focus. The reply carries the frontmost application pid
    /// read before and after, so "the foreground did not move" is measured
    /// rather than assumed.
    Raise {
        target: TargetRef,
        window: isize,
    },
    /// `minimize`: send one window to the dock through the window's own
    /// minimize affordance (macOS: the window attribute `AXMinimized` set
    /// to true), never a keyboard shortcut and never by activating the
    /// application.
    ///
    /// Gated like `Close`, minus the snapshot: an exact target (`window`)
    /// and a checkable postcondition (`expect: "minimized"`). A window that
    /// is already minimized is a verified no-op (`performed: false,
    /// verified: true`), the same contract `invoke set-checked` has for a
    /// desired state that already holds.
    Minimize {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
    },
    /// `restore`: bring one minimized window back (`AXMinimized` set to
    /// false) without activating its application. Gate and no-op contract
    /// are `Minimize`'s, with `expect: "restored"`.
    Restore {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
    },
    /// `drag`: one press, a bounded series of moves and one release,
    /// delivered as one gesture.
    ///
    /// macOS has no window-local pointer injection at all (measured: mouse
    /// events posted to a pid arrive with no window and AppKit routes them
    /// nowhere), so the only path there is the global one that moves the
    /// real cursor -- exactly the situation `click --coords` already names
    /// `degraded`. So `degraded` is a required opt-in wherever the host can
    /// only drag by moving the user's own pointer, and the reply always
    /// says which path ran plus the pointer position before and after.
    Drag {
        target: TargetRef,
        window: isize,
        from: [i32; 2],
        to: [i32; 2],
        #[serde(default)]
        button: PointerButton,
        /// Intermediate moves between press and release (1..=64).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steps: Option<u32>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        degraded: bool,
    },
    /// `hit`: screen coordinates -> the accessibility node under them, in
    /// the node shape `query` returns, so the `id` is directly usable with
    /// `invoke --node` / `click --node`.
    Hit {
        target: TargetRef,
        window: isize,
        x: i32,
        y: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
    },
    /// `zoom`: crop one region out of a window capture so a caller can
    /// inspect a detail without a full-screen image. A region that does not
    /// intersect the window is typed `region_outside_window` and writes no
    /// file.
    Zoom {
        target: TargetRef,
        window: isize,
        /// `x, y, width, height` in screen coordinates (the space
        /// `query --within` and node `bounds` use).
        region: [i32; 4],
        out: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        replace: bool,
        /// Pixels of context kept around the region (default 8).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pad: Option<u32>,
    },
    /// `snapshot`: capture the bounded tree as a named baseline and persist
    /// it beside the receipts (`<audit dir>/cu-snapshots`), so `diff` can
    /// answer "what changed since" without the caller holding the tree.
    Snapshot {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        /// Also write the baseline to this path, for a caller that wants
        /// the tree itself rather than only the id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        out: Option<String>,
    },
    /// `diff`: compare the window's current bounded tree against a stored
    /// baseline. Without `base` the most recent snapshot of that window is
    /// used and the reply says which. `advance` writes the tree it just
    /// read as the next baseline in the same call, so an agent can poll a
    /// window incrementally.
    Diff {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        advance: bool,
        /// Changes returned per bucket (default 200, at most 2000).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// MCU group this binary answers typed (no silent unknown command).
    Align {
        target: TargetRef,
        group: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "wait", rename_all = "kebab-case")]
pub enum WaitCondition {
    WindowCountGte {
        count: usize,
    },
    /// Polls the tree until every `verify` expectation is met. A missing
    /// target keeps polling; an ambiguous target or an unobservable state
    /// fails closed at once; the deadline is typed `timeout` carrying the
    /// last observation.
    Expect {
        window: isize,
        expect: Vec<Expectation>,
    },
    WindowTitleContains {
        pattern: String,
    },
    FocusedHandle {
        handle: isize,
    },
    /// Polls the accessibility tree until exactly one showing node matches.
    /// Two or more showing hits fail typed (`a11y_node_ambiguous`) instead of
    /// picking the first. Never falls back to pixels: addressing stays
    /// `accessibility-tree`.
    NodeNameContains {
        pattern: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
    },
    /// Polls AT-SPI `Text.GetText` on the unique showing node addressed by
    /// `--name` until that independent text equals `expected`. Snapshot
    /// `node.text`, `send-text` / `paste` / `copy` `matched.text`,
    /// `last_text_write_via`, and the WebKit eval helper's queued-job `OK`
    /// are not this condition.
    /// Timeout is typed. Never screenshot / XTest / `--coords`.
    NodeTextEquals {
        expected: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
    },
    /// Same independent `Text.GetText` poll as `NodeTextEquals`, but the
    /// hit is `gettext.contains(substring)`. Snapshot `node.text`,
    /// `send-text` / `paste` / `copy` `matched.text`, `last_text_write_via`,
    /// and the WebKit eval helper's queued-job `OK` are not this condition.
    /// Timeout is typed. Never screenshot / XTest / `--coords`.
    NodeTextContains {
        substring: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
    },
}

fn default_clicks() -> u32 {
    1
}

fn default_term_max_bytes() -> usize {
    1_048_576
}

fn default_term_enter() -> bool {
    true
}

fn default_term_send_timeout_ms() -> u64 {
    2_000
}

fn default_term_wait_timeout_ms() -> u64 {
    30_000
}

fn default_term_wait_interval_ms() -> u64 {
    100
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionAction {
    #[default]
    Status,
    Open,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SetupAction {
    Check,
    #[default]
    Apply,
}

impl SetupAction {
    fn is_apply(&self) -> bool {
        *self == Self::Apply
    }
}

impl PermissionAction {
    fn is_status(&self) -> bool {
        *self == Self::Status
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionKind {
    Accessibility,
    ScreenCapture,
}

impl PermissionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accessibility => "accessibility",
            Self::ScreenCapture => "screen-capture",
        }
    }
}

impl Command {
    pub fn verb(&self) -> String {
        match self {
            Self::Capabilities { .. } => "capabilities".into(),
            Self::Setup { .. } => "setup".into(),
            Self::Permissions { .. } => "permissions".into(),
            Self::Doctor { .. } => "doctor".into(),
            Self::RuntimeStatus { .. } => "runtime-status".into(),
            Self::AudioStatus { .. }
            | Self::AudioPlanVolume { .. }
            | Self::AudioPlanMuted { .. }
            | Self::AudioApply { .. } => "audio".into(),
            Self::ServiceList { .. }
            | Self::ServiceStatus { .. }
            | Self::ServicePlan { .. }
            | Self::ServiceApply { .. }
            | Self::ServiceTransact { .. } => "service".into(),
            Self::LoginSessionStatus { .. }
            | Self::LoginSessionPlanLock { .. }
            | Self::LoginSessionApplyLock { .. } => "login-session".into(),
            Self::HostOpen { .. } => "host-open".into(),
            Self::HostNotify { .. } => "host-notify".into(),
            Self::AuditQuery { .. } => "audit-query".into(),
            Self::AuditCompact { .. } => "audit-compact".into(),
            Self::SessionStart { .. } => "session-start".into(),
            Self::SessionList { .. } => "session-list".into(),
            Self::SessionStatus { .. } => "session-status".into(),
            Self::SessionRenew { .. } => "session-renew".into(),
            Self::SessionEnd { .. } => "session-end".into(),
            Self::LockAcquire { .. } => "lock-acquire".into(),
            Self::LockList { .. } => "lock-list".into(),
            Self::LockRelease { .. } => "lock-release".into(),
            Self::JobSpawn { .. } => "job-spawn".into(),
            Self::JobAdopt { .. } => "job-adopt".into(),
            Self::JobList { .. } => "job-list".into(),
            Self::JobStatus { .. } => "job-status".into(),
            Self::JobPrune { .. } => "job-prune".into(),
            Self::JobResources { .. } => "job-resources".into(),
            Self::JobPriority { .. } => "job-priority".into(),
            Self::JobEvents { .. } => "job-events".into(),
            Self::JobOutput { .. } => "job-output".into(),
            Self::JobWrite { .. } => "job-write".into(),
            Self::JobWait { .. } => "job-wait".into(),
            Self::JobSetState { .. } => "job-set-state".into(),
            Self::JobSignal { .. } => "job-signal".into(),
            Self::JobStop { .. } => "job-stop".into(),
            Self::JobRenew { .. } => "job-renew".into(),
            Self::Windows { .. } => "windows".into(),
            Self::WindowsWatch { .. } => "windows-watch".into(),
            Self::Apps { .. } => "apps".into(),
            Self::Ps { .. } => "ps".into(),
            Self::ProcessState { .. } => "process-state".into(),
            Self::ProcessArgv { .. } => "process-argv".into(),
            Self::ProcessCwd { .. } => "process-cwd".into(),
            Self::ProcessEnvironment { .. } => "process-environment".into(),
            Self::ProcessFds { .. } => "process-fds".into(),
            Self::ProcessMaps { .. } => "process-maps".into(),
            Self::ProcessThreads { .. } => "process-threads".into(),
            Self::ProcessSockets { .. } => "process-sockets".into(),
            Self::ProcessCgroup { .. } => "process-cgroup".into(),
            Self::ProcessUsage { .. } => "process-usage".into(),
            Self::ProcessWait { .. } => "process-wait".into(),
            Self::ProcessKill { .. } => "process-kill".into(),
            Self::ProcessSetState { .. } => "process-set-state".into(),
            Self::ProcessPolicy { .. } => "process-policy".into(),
            Self::ProcessSignal { .. } => "process-signal".into(),
            Self::PrivilegePlanProcessPriority { .. } => "privilege-plan".into(),
            Self::ProcessWatch { .. } => "process-watch".into(),
            Self::ShellExec { .. } => "shell-exec".into(),
            Self::NetworkInterfaces { .. } => "network-interfaces".into(),
            Self::NetworkRoutes { .. } => "network-routes".into(),
            Self::NetworkDns { .. } => "network-dns".into(),
            Self::NetworkProbe { .. } => "network-probe".into(),
            Self::FileInspect { .. } => "file-inspect".into(),
            Self::FileCopy { .. } => "file-copy".into(),
            Self::FileMove { .. } => "file-move".into(),
            Self::FileTransaction { .. } => "file-transaction".into(),
            Self::PtyStart { .. } => "pty-start".into(),
            Self::PtyList { .. } => "pty-list".into(),
            Self::PtyPrune { .. } => "pty-prune".into(),
            Self::PtyStatus { .. } => "pty-status".into(),
            Self::PtyRead { .. } => "pty-read".into(),
            Self::PtySnapshot { .. } => "pty-snapshot".into(),
            Self::PtyDiff { .. } => "pty-diff".into(),
            Self::PtyEvents { .. } => "pty-events".into(),
            Self::PtyResize { .. } => "pty-resize".into(),
            Self::PtySend { .. } => "pty-send".into(),
            Self::PtyWait { .. } => "pty-wait".into(),
            Self::PtyWaitExit { .. } => "pty-wait-exit".into(),
            Self::PtySignal { .. } => "pty-signal".into(),
            Self::PtyStop { .. } => "pty-stop".into(),
            Self::TerminalList { .. } => "terminal-list".into(),
            Self::TerminalNew { .. } => "terminal-new".into(),
            Self::TerminalClose { .. } => "terminal-close".into(),
            Self::TerminalRead { .. } => "terminal-read".into(),
            Self::TerminalSnapshot { .. } => "terminal-snapshot".into(),
            Self::TerminalScroll { .. } => "terminal-scroll".into(),
            Self::TerminalScreenshot { .. } => "terminal-screenshot".into(),
            Self::TerminalEvents { .. } => "terminal-events".into(),
            Self::TerminalOutput { .. } => "terminal-output".into(),
            Self::TerminalSend { .. } => "terminal-send".into(),
            Self::TerminalWait { .. } => "terminal-wait".into(),
            Self::TermRead { .. } => "term-read".into(),
            Self::TermSend { .. } => "term-send".into(),
            Self::TermWait { .. } => "term-wait".into(),
            Self::Tree { .. } => "tree".into(),
            Self::DesktopState { .. } => "desktop-state".into(),
            Self::Query { .. } => "query".into(),
            Self::Invoke { .. } => "invoke".into(),
            Self::MenuInspect { .. } => "menu-inspect".into(),
            Self::MenuInvoke { .. } => "menu-invoke".into(),
            Self::Focused { .. } => "focused".into(),
            Self::Observe { .. } => "observe".into(),
            Self::Verify { .. } => "verify".into(),
            Self::Screenshot { .. } => "screenshot".into(),
            Self::DeviceScreenshot { .. } => "device-screenshot".into(),
            Self::ResourceStatus { .. } => "resource-status".into(),
            Self::PowerStatus { .. } => "power-status".into(),
            Self::StorageDevices { .. } => "storage-devices".into(),
            Self::DeviceList { .. } => "device-list".into(),
            Self::DeviceWatch { .. } => "device-watch".into(),
            Self::DeviceClaims { .. } => "device-claims".into(),
            Self::DeviceClaim { .. } => "device-claim".into(),
            Self::DeviceStatus { .. } => "device-status".into(),
            Self::DeviceRead { .. } => "device-read".into(),
            Self::DeviceWrite { .. } => "device-write".into(),
            Self::DeviceRenew { .. } => "device-renew".into(),
            Self::DeviceRelease { .. } => "device-release".into(),
            Self::SimulatorDevices { .. } => "simulator-devices".into(),
            Self::SimulatorBoot { .. } => "simulator-boot".into(),
            Self::SimulatorApps { .. } => "simulator-apps".into(),
            Self::SimulatorLaunch { .. } => "simulator-launch".into(),
            Self::SimulatorTerminate { .. } => "simulator-terminate".into(),
            Self::PointerMove { .. } => "pointer-move".into(),
            Self::PointerPosition { .. } => "pointer-position".into(),
            Self::Click { .. } => "click".into(),
            Self::Focus { .. } => "focus".into(),
            Self::SendText { .. } => "send-text".into(),
            Self::ClipboardRead { .. } => "clipboard-read".into(),
            Self::ClipboardWrite { .. } => "clipboard-write".into(),
            Self::ClipboardWriteFile { .. } => "clipboard-write-file".into(),
            Self::ClipboardClear { .. } => "clipboard-clear".into(),
            Self::Copy { .. } => "copy".into(),
            Self::Paste { .. } => "paste".into(),
            Self::SendKeys { .. } => "send-keys".into(),
            Self::Scroll { .. } => "scroll".into(),
            Self::GetExtents { .. } => "get-extents".into(),
            Self::Select { .. } => "select".into(),
            Self::GetSelection { .. } => "get-selection".into(),
            Self::SetCaret { .. } => "set-caret".into(),
            Self::GetCaret { .. } => "get-caret".into(),
            Self::GetText { .. } => "get-text".into(),
            Self::Wait { .. } => "wait".into(),
            Self::WindowPlace { .. } => "window-place".into(),
            Self::OrderWin { .. } => "orderwin".into(),
            Self::App { .. } => "app".into(),
            Self::Spaces { .. } => "spaces".into(),
            Self::Displays { .. } => "displays".into(),
            Self::Close { .. } => "close".into(),
            Self::Receipts { .. } => "receipts".into(),
            Self::PageJs { .. } => "page-js".into(),
            Self::PageTargets { .. } => "page-targets".into(),
            Self::PageText { .. } => "page-text".into(),
            Self::PageFind { .. } => "page-find".into(),
            Self::PageClick { .. } => "page-click".into(),
            Self::PageDownload { .. } => "page-download".into(),
            Self::PageHover { .. } => "page-hover".into(),
            Self::PageScroll { .. } => "page-scroll".into(),
            Self::PageDrag { .. } => "page-drag".into(),
            Self::PageDialog { .. } => "page-dialog".into(),
            Self::PageFiles { .. } => "page-files".into(),
            Self::PageFill { .. } => "page-fill".into(),
            Self::PageType { .. } => "page-type".into(),
            Self::PageNav { .. } => "page-nav".into(),
            Self::PageScreenshot { .. } => "page-screenshot".into(),
            Self::TabList { .. } => "tab-list".into(),
            Self::TabSelect { .. } => "tab-select".into(),
            Self::TabClose { .. } => "tab-close".into(),
            Self::BrowserProfiles { .. } => "browser-profiles".into(),
            Self::BrowserOpen { .. } => "browser-open".into(),
            Self::BrowserSessionStart { .. } => "browser-session-start".into(),
            Self::BrowserSessionList { .. } => "browser-session-list".into(),
            Self::BrowserSessionStatus { .. } => "browser-session-status".into(),
            Self::BrowserSessionStop { .. } => "browser-session-stop".into(),
            Self::BrowserSessionRemove { .. } => "browser-session-remove".into(),
            Self::BrowserBridgeSetup { .. } => "browser-bridge-setup".into(),
            Self::BrowserBridgeConnections { .. } => "browser-bridge-connections".into(),
            Self::BrowserBridgeStatus { .. } => "browser-bridge-status".into(),
            Self::BrowserBridgeTabs { .. } => "browser-bridge-tabs".into(),
            Self::BrowserBridgeWindows { .. } => "browser-bridge-windows".into(),
            Self::BrowserBridgeWindowOpen { .. } => "browser-bridge-window-open".into(),
            Self::BrowserBridgeWindowState { .. } => "browser-bridge-window-state".into(),
            Self::BrowserBridgeDebugRead { .. } => "browser-bridge-debug-read".into(),
            Self::Unlock { .. } => "unlock".into(),
            Self::Activate { .. } => "activate".into(),
            Self::Raise { .. } => "raise".into(),
            Self::Minimize { .. } => "minimize".into(),
            Self::Restore { .. } => "restore".into(),
            Self::Drag { .. } => "drag".into(),
            Self::Hit { .. } => "hit".into(),
            Self::Zoom { .. } => "zoom".into(),
            Self::Snapshot { .. } => "snapshot".into(),
            Self::Diff { .. } => "diff".into(),
            Self::Align { group, .. } => group.clone(),
        }
    }

    /// Stable persisted-authority identity for this exact command shape.
    ///
    /// This is deliberately narrower than [`Self::verb`]: verbs with
    /// materially different effects receive different operation ids. Dynamic
    /// MCU alignment placeholders never become persisted authority.
    pub fn authorization_operation(&self) -> Option<String> {
        let operation = match self {
            Self::Setup { action, .. } => match action {
                SetupAction::Check => "setup.check".to_owned(),
                SetupAction::Apply => "setup.apply".to_owned(),
            },
            Self::Permissions { action, .. } => match action {
                PermissionAction::Status => "permissions.status".to_owned(),
                PermissionAction::Open => "permissions.open".to_owned(),
            },
            Self::AudioStatus { .. } => "audio.status".to_owned(),
            Self::AudioPlanVolume { .. } => "audio.plan-volume".to_owned(),
            Self::AudioPlanMuted { .. } => "audio.plan-muted".to_owned(),
            Self::AudioApply { .. } => "audio.apply".to_owned(),
            Self::ServiceList { scope, .. } => {
                format!("service.list.{}", service_scope_operation(*scope))
            }
            Self::ServiceStatus { scope, .. } => {
                format!("service.status.{}", service_scope_operation(*scope))
            }
            Self::ServicePlan {
                scope, operation, ..
            } => format!(
                "service.plan.{}.{}",
                service_scope_operation(*scope),
                service_operation(*operation)
            ),
            Self::ServiceApply { .. } => "service.apply".to_owned(),
            Self::ServiceTransact {
                scope, operation, ..
            } => format!(
                "service.transact.{}.{}",
                service_scope_operation(*scope),
                service_operation(*operation)
            ),
            Self::LoginSessionStatus { .. } => "login-session.status".to_owned(),
            Self::LoginSessionPlanLock { .. } => "login-session.plan-lock".to_owned(),
            Self::LoginSessionApplyLock { .. } => "login-session.apply-lock".to_owned(),
            Self::AuditCompact { apply, .. } => {
                format!("audit-compact.{}", if *apply { "apply" } else { "plan" })
            }
            Self::JobPrune { apply, .. } => {
                format!("job-prune.{}", if *apply { "apply" } else { "plan" })
            }
            Self::FileCopy { apply, .. } => {
                format!("file-copy.{}", if *apply { "apply" } else { "plan" })
            }
            Self::FileMove { apply, .. } => {
                format!("file-move.{}", if *apply { "apply" } else { "plan" })
            }
            Self::FileTransaction { action, .. } => format!(
                "file-transaction.{}",
                match action {
                    FileTransactionAction::Status => "status",
                    FileTransactionAction::Rollback => "rollback",
                    FileTransactionAction::Recover => "recover",
                    FileTransactionAction::Finalize => "finalize",
                }
            ),
            Self::ProcessKill { mode, .. } => format!(
                "process-kill.{}",
                match mode {
                    ProcessKillMode::Graceful => "graceful",
                    ProcessKillMode::Forceful => "forceful",
                }
            ),
            Self::ProcessSetState { state, .. } => format!(
                "process-set-state.{}",
                match state {
                    ProcessRunState::Running => "running",
                    ProcessRunState::Stopped => "stopped",
                }
            ),
            Self::ProcessPolicy { action, .. } => match action {
                ProcessPolicyAction::Status => return None,
                ProcessPolicyAction::Background => "process-policy.background".to_owned(),
                ProcessPolicyAction::Normal => "process-policy.normal".to_owned(),
            },
            Self::ProcessSignal {
                signal,
                force,
                tree,
                ..
            } => format!(
                "process-signal.{}.{}.{}",
                if *tree { "tree" } else { "single" },
                if *force { "force" } else { "normal" },
                signal.as_str().to_ascii_lowercase()
            ),
            Self::PtyDiff { advance, .. } => {
                format!("pty-diff.{}", if *advance { "advance" } else { "read" })
            }
            Self::PtySignal { signal, .. } => {
                format!("pty-signal.{}", signal.as_str())
            }
            Self::ClipboardClear { apply, .. } => {
                format!("clipboard-clear.{}", if *apply { "apply" } else { "plan" })
            }
            Self::PageScreenshot { activate, .. } => format!(
                "page-screenshot.{}",
                if *activate {
                    "capture-and-activate"
                } else {
                    "capture"
                }
            ),
            Self::Invoke { action, .. } => format!(
                "invoke.{}",
                match action {
                    InvokeAction::Press => "press",
                    InvokeAction::SetValue => "set-value",
                    InvokeAction::SelectOption => "select-option",
                    InvokeAction::SetChecked => "set-checked",
                    InvokeAction::SetExpanded => "set-expanded",
                    InvokeAction::Increment => "increment",
                    InvokeAction::Decrement => "decrement",
                    InvokeAction::SetSelected => "set-selected",
                    InvokeAction::SetSelection => "set-selection",
                    InvokeAction::ScrollTo => "scroll-to",
                    InvokeAction::Cancel => "cancel",
                    InvokeAction::ShowDefaultUi => "show-default-ui",
                }
            ),
            Self::App { action, .. } => format!(
                "app.{}",
                match action {
                    AppAction::Hide => "hide",
                    AppAction::Show => "show",
                    AppAction::Quit => "quit",
                    AppAction::Launch => "launch",
                }
            ),
            Self::WindowPlace { action, frame, .. } => {
                let canonical = match action.as_str() {
                    "frame" | "move" | "resize" if frame.is_some() => action.as_str(),
                    _ if frame.is_none() => crate::place::PlaceAction::parse(action)?.kebab(),
                    _ => return None,
                };
                format!("window-place.{canonical}")
            }
            Self::DeviceScreenshot { list, path, .. } => {
                if *list && path.is_none() {
                    "device-screenshot.list".to_owned()
                } else if !*list && path.is_some() {
                    "device-screenshot.capture".to_owned()
                } else {
                    return None;
                }
            }
            Self::PageDialog { dismiss, .. } => format!(
                "page-dialog.{}",
                if *dismiss { "dismiss" } else { "accept" }
            ),
            Self::Diff { advance, .. } => {
                format!("diff.{}", if *advance { "advance" } else { "read" })
            }
            Self::Align { .. } => return None,
            _ => self.verb(),
        };
        Some(operation)
    }

    pub fn target(&self) -> TargetRef {
        match self {
            Self::Capabilities { target, .. }
            | Self::Setup { target, .. }
            | Self::Permissions { target, .. }
            | Self::Doctor { target, .. }
            | Self::RuntimeStatus { target, .. }
            | Self::AudioStatus { target }
            | Self::AudioPlanVolume { target, .. }
            | Self::AudioPlanMuted { target, .. }
            | Self::AudioApply { target, .. }
            | Self::ServiceList { target, .. }
            | Self::ServiceStatus { target, .. }
            | Self::ServicePlan { target, .. }
            | Self::ServiceApply { target, .. }
            | Self::ServiceTransact { target, .. }
            | Self::LoginSessionStatus { target }
            | Self::LoginSessionPlanLock { target, .. }
            | Self::LoginSessionApplyLock { target, .. }
            | Self::HostOpen { target, .. }
            | Self::HostNotify { target, .. }
            | Self::AuditQuery { target, .. }
            | Self::AuditCompact { target, .. }
            | Self::SessionStart { target, .. }
            | Self::SessionList { target, .. }
            | Self::SessionStatus { target, .. }
            | Self::SessionRenew { target, .. }
            | Self::SessionEnd { target, .. }
            | Self::LockAcquire { target, .. }
            | Self::LockList { target, .. }
            | Self::LockRelease { target, .. }
            | Self::JobSpawn { target, .. }
            | Self::JobAdopt { target, .. }
            | Self::JobList { target, .. }
            | Self::JobStatus { target, .. }
            | Self::JobPrune { target, .. }
            | Self::JobResources { target, .. }
            | Self::JobPriority { target, .. }
            | Self::JobEvents { target, .. }
            | Self::JobOutput { target, .. }
            | Self::JobWrite { target, .. }
            | Self::JobWait { target, .. }
            | Self::JobSetState { target, .. }
            | Self::JobSignal { target, .. }
            | Self::JobStop { target, .. }
            | Self::JobRenew { target, .. }
            | Self::Windows { target, .. }
            | Self::WindowsWatch { target, .. }
            | Self::Apps { target, .. }
            | Self::Ps { target, .. }
            | Self::ProcessState { target, .. }
            | Self::ProcessArgv { target, .. }
            | Self::ProcessCwd { target, .. }
            | Self::ProcessEnvironment { target, .. }
            | Self::ProcessFds { target, .. }
            | Self::ProcessMaps { target, .. }
            | Self::ProcessThreads { target, .. }
            | Self::ProcessSockets { target, .. }
            | Self::ProcessCgroup { target, .. }
            | Self::ProcessUsage { target, .. }
            | Self::ProcessWait { target, .. }
            | Self::ProcessKill { target, .. }
            | Self::ProcessSetState { target, .. }
            | Self::ProcessPolicy { target, .. }
            | Self::ProcessSignal { target, .. }
            | Self::PrivilegePlanProcessPriority { target, .. }
            | Self::ProcessWatch { target, .. }
            | Self::ShellExec { target, .. }
            | Self::NetworkInterfaces { target, .. }
            | Self::NetworkRoutes { target, .. }
            | Self::NetworkDns { target, .. }
            | Self::NetworkProbe { target, .. }
            | Self::FileInspect { target, .. }
            | Self::FileCopy { target, .. }
            | Self::FileMove { target, .. }
            | Self::FileTransaction { target, .. }
            | Self::PtyStart { target, .. }
            | Self::PtyList { target, .. }
            | Self::PtyPrune { target, .. }
            | Self::PtyStatus { target, .. }
            | Self::PtyRead { target, .. }
            | Self::PtySnapshot { target, .. }
            | Self::PtyDiff { target, .. }
            | Self::PtyEvents { target, .. }
            | Self::PtyResize { target, .. }
            | Self::PtySend { target, .. }
            | Self::PtyWait { target, .. }
            | Self::PtyWaitExit { target, .. }
            | Self::PtySignal { target, .. }
            | Self::PtyStop { target, .. }
            | Self::TerminalList { target, .. }
            | Self::TerminalNew { target, .. }
            | Self::TerminalClose { target, .. }
            | Self::TerminalRead { target, .. }
            | Self::TerminalSnapshot { target, .. }
            | Self::TerminalScroll { target, .. }
            | Self::TerminalScreenshot { target, .. }
            | Self::TerminalEvents { target, .. }
            | Self::TerminalOutput { target, .. }
            | Self::TerminalSend { target, .. }
            | Self::TerminalWait { target, .. }
            | Self::TermRead { target, .. }
            | Self::TermSend { target, .. }
            | Self::TermWait { target, .. }
            | Self::Tree { target, .. }
            | Self::DesktopState { target, .. }
            | Self::Query { target, .. }
            | Self::Invoke { target, .. }
            | Self::MenuInspect { target, .. }
            | Self::MenuInvoke { target, .. }
            | Self::Focused { target, .. }
            | Self::Observe { target, .. }
            | Self::Verify { target, .. }
            | Self::Screenshot { target, .. }
            | Self::DeviceScreenshot { target, .. }
            | Self::ResourceStatus { target }
            | Self::PowerStatus { target }
            | Self::StorageDevices { target, .. }
            | Self::DeviceList { target, .. }
            | Self::DeviceWatch { target, .. }
            | Self::DeviceClaims { target, .. }
            | Self::DeviceClaim { target, .. }
            | Self::DeviceStatus { target, .. }
            | Self::DeviceRead { target, .. }
            | Self::DeviceWrite { target, .. }
            | Self::DeviceRenew { target, .. }
            | Self::DeviceRelease { target, .. }
            | Self::SimulatorDevices { target, .. }
            | Self::SimulatorBoot { target, .. }
            | Self::SimulatorApps { target, .. }
            | Self::SimulatorLaunch { target, .. }
            | Self::SimulatorTerminate { target, .. }
            | Self::PointerMove { target, .. }
            | Self::PointerPosition { target, .. }
            | Self::Click { target, .. }
            | Self::Focus { target, .. }
            | Self::SendText { target, .. }
            | Self::ClipboardRead { target, .. }
            | Self::ClipboardWrite { target, .. }
            | Self::ClipboardWriteFile { target, .. }
            | Self::ClipboardClear { target, .. }
            | Self::Copy { target, .. }
            | Self::Paste { target, .. }
            | Self::SendKeys { target, .. }
            | Self::Scroll { target, .. }
            | Self::GetExtents { target, .. }
            | Self::Select { target, .. }
            | Self::GetSelection { target, .. }
            | Self::SetCaret { target, .. }
            | Self::GetCaret { target, .. }
            | Self::GetText { target, .. }
            | Self::Wait { target, .. }
            | Self::WindowPlace { target, .. }
            | Self::OrderWin { target, .. }
            | Self::App { target, .. }
            | Self::Spaces { target, .. }
            | Self::Displays { target, .. }
            | Self::Close { target, .. }
            | Self::Receipts { target, .. }
            | Self::PageJs { target, .. }
            | Self::PageTargets { target, .. }
            | Self::PageText { target, .. }
            | Self::PageFind { target, .. }
            | Self::PageClick { target, .. }
            | Self::PageDownload { target, .. }
            | Self::PageHover { target, .. }
            | Self::PageScroll { target, .. }
            | Self::PageDrag { target, .. }
            | Self::PageDialog { target, .. }
            | Self::PageFiles { target, .. }
            | Self::PageFill { target, .. }
            | Self::PageType { target, .. }
            | Self::PageNav { target, .. }
            | Self::PageScreenshot { target, .. }
            | Self::TabList { target, .. }
            | Self::TabSelect { target, .. }
            | Self::TabClose { target, .. }
            | Self::BrowserProfiles { target, .. }
            | Self::BrowserOpen { target, .. }
            | Self::BrowserSessionStart { target, .. }
            | Self::BrowserSessionList { target, .. }
            | Self::BrowserSessionStatus { target, .. }
            | Self::BrowserSessionStop { target, .. }
            | Self::BrowserSessionRemove { target, .. }
            | Self::BrowserBridgeSetup { target, .. }
            | Self::BrowserBridgeConnections { target, .. }
            | Self::BrowserBridgeStatus { target, .. }
            | Self::BrowserBridgeTabs { target, .. }
            | Self::BrowserBridgeWindows { target, .. }
            | Self::BrowserBridgeWindowOpen { target, .. }
            | Self::BrowserBridgeWindowState { target, .. }
            | Self::BrowserBridgeDebugRead { target, .. }
            | Self::Unlock { target, .. }
            | Self::Activate { target, .. }
            | Self::Raise { target, .. }
            | Self::Minimize { target, .. }
            | Self::Restore { target, .. }
            | Self::Drag { target, .. }
            | Self::Hit { target, .. }
            | Self::Zoom { target, .. }
            | Self::Snapshot { target, .. }
            | Self::Diff { target, .. }
            | Self::Align { target, .. } => *target,
        }
    }

    pub fn required_grant(&self) -> crate::auth::Grant {
        if matches!(
            self,
            Self::ProcessPolicy {
                action: ProcessPolicyAction::Status,
                ..
            }
        ) {
            return crate::auth::Grant::Observe;
        }
        match self {
            Self::Setup {
                action: SetupAction::Apply,
                ..
            }
            | Self::Permissions {
                action: PermissionAction::Open,
                ..
            }
            | Self::HostOpen { .. }
            | Self::HostNotify { .. }
            | Self::AudioApply { .. }
            | Self::ServiceApply { .. }
            | Self::ServiceTransact { .. }
            | Self::LoginSessionApplyLock { .. }
            | Self::PointerMove { .. }
            | Self::AuditCompact { apply: true, .. }
            | Self::JobPrune { apply: true, .. }
            | Self::SessionStart { .. }
            | Self::SessionRenew { .. }
            | Self::SessionEnd { .. }
            | Self::LockAcquire { .. }
            | Self::LockRelease { .. }
            | Self::JobSpawn { .. }
            | Self::JobAdopt { .. }
            | Self::JobWrite { .. }
            | Self::JobPriority { .. }
            | Self::JobSetState { .. }
            | Self::JobSignal { .. }
            | Self::JobStop { .. }
            | Self::JobRenew { .. }
            | Self::DeviceClaim { .. }
            | Self::DeviceRead { .. }
            | Self::DeviceWrite { .. }
            | Self::DeviceRenew { .. }
            | Self::DeviceRelease { .. }
            | Self::ProcessKill { .. }
            | Self::ProcessSetState { .. }
            | Self::ProcessPolicy { .. }
            | Self::ProcessSignal { .. }
            | Self::ShellExec { .. }
            | Self::FileCopy { apply: true, .. }
            | Self::FileMove { apply: true, .. }
            | Self::FileTransaction {
                action:
                    FileTransactionAction::Rollback
                    | FileTransactionAction::Recover
                    | FileTransactionAction::Finalize,
                ..
            }
            | Self::PtyStart { .. }
            | Self::PtyPrune { .. }
            | Self::PtyResize { .. }
            | Self::PtySend { .. }
            | Self::PtySignal { .. }
            | Self::PtyStop { .. }
            | Self::TerminalSend { .. }
            | Self::TerminalScroll { .. }
            | Self::TerminalNew { .. }
            | Self::TerminalClose { .. }
            | Self::TermSend { .. }
            | Self::Invoke { .. }
            | Self::MenuInvoke { .. }
            | Self::Click { .. }
            | Self::Focus { .. }
            | Self::SendText { .. }
            | Self::Copy { .. }
            | Self::ClipboardWrite { .. }
            | Self::ClipboardWriteFile { .. }
            | Self::ClipboardClear { .. }
            | Self::Paste { .. }
            | Self::SendKeys { .. }
            | Self::Scroll { .. }
            | Self::Select { .. }
            | Self::SetCaret { .. }
            | Self::WindowPlace { .. }
            | Self::OrderWin { .. }
            | Self::Close { .. }
            | Self::TabSelect { .. }
            | Self::TabClose { .. }
            | Self::BrowserOpen { .. }
            | Self::BrowserSessionStart { .. }
            | Self::BrowserSessionStop { .. }
            | Self::BrowserSessionRemove { .. }
            | Self::BrowserBridgeSetup { .. }
            | Self::BrowserBridgeWindowOpen { .. }
            | Self::BrowserBridgeWindowState { .. }
            | Self::SimulatorBoot { .. }
            | Self::SimulatorLaunch { .. }
            | Self::SimulatorTerminate { .. }
            | Self::PageClick { .. }
            | Self::PageDownload { .. }
            | Self::PageHover { .. }
            | Self::PageScroll { .. }
            | Self::PageDrag { .. }
            | Self::PageDialog { .. }
            | Self::PageFiles { .. }
            | Self::PageFill { .. }
            | Self::PageType { .. }
            | Self::PageNav { .. }
            | Self::PageScreenshot { activate: true, .. }
            | Self::Activate { .. }
            | Self::Raise { .. }
            | Self::Minimize { .. }
            | Self::Restore { .. }
            | Self::Drag { .. }
            | Self::App { .. } => crate::auth::Grant::Actuate,
            _ => crate::auth::Grant::Observe,
        }
    }

    /// Validate a command assembled directly in Rust. Deserialization applies
    /// the same field bounds before constructing managed-job variants.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Setup {
                target, bin_dir, ..
            } => {
                if *target != TargetRef::Current {
                    return Err("setup supports only target=current");
                }
                if bin_dir.as_ref().is_some_and(|path| {
                    path.is_empty() || path.len() > 8192 || path.as_bytes().contains(&0)
                }) {
                    return Err("setup bin_dir must contain 1..=8192 non-NUL UTF-8 bytes");
                }
                Ok(())
            }
            Self::LoginSessionPlanLock { ttl_seconds, .. } => {
                if !(1..=600).contains(ttl_seconds) {
                    return Err("login-session plan lock ttl_seconds must be in 1..=600");
                }
                Ok(())
            }
            Self::AudioPlanVolume {
                volume,
                ttl_seconds,
                ..
            } => {
                if *volume > 100 {
                    return Err("audio plan volume must be in 0..=100");
                }
                if !(1..=600).contains(ttl_seconds) {
                    return Err("audio plan ttl_seconds must be in 1..=600");
                }
                Ok(())
            }
            Self::AudioPlanMuted { ttl_seconds, .. } => {
                if !(1..=600).contains(ttl_seconds) {
                    return Err("audio plan ttl_seconds must be in 1..=600");
                }
                Ok(())
            }
            Self::AudioApply {
                request, approval, ..
            } => {
                if request.is_empty() || request.len() > 32_768 {
                    return Err("audio apply request must be in 1..=32768 bytes");
                }
                if approval.len() != 64 || !approval.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err("audio apply approval must be a 64-hex digest");
                }
                Ok(())
            }
            Self::ServiceList {
                match_text, max, ..
            } => {
                if !(1..=5_000).contains(max) {
                    return Err("service list max must be in 1..=5000");
                }
                if match_text
                    .as_ref()
                    .is_some_and(|value| value.len() > 1_024 || value.chars().any(char::is_control))
                {
                    return Err("service list match_text must be <=1024 non-control bytes");
                }
                Ok(())
            }
            Self::ServiceStatus { name, .. } => validate_service_name(name),
            Self::ServicePlan {
                name,
                operation,
                definition,
                ttl_seconds,
                ..
            } => {
                validate_service_name(name)?;
                if !(1..=600).contains(ttl_seconds) {
                    return Err("service plan ttl_seconds must be in 1..=600");
                }
                if matches!(operation, ServiceOperation::Bootstrap) && definition.is_none() {
                    return Err("service bootstrap plan requires definition");
                }
                if !matches!(operation, ServiceOperation::Bootstrap) && definition.is_some() {
                    return Err("service definition is accepted only by bootstrap plans");
                }
                if definition.as_ref().is_some_and(|path| {
                    path.is_empty() || path.len() > 8_192 || path.as_bytes().contains(&0)
                }) {
                    return Err("service definition must be in 1..=8192 non-NUL bytes");
                }
                Ok(())
            }
            Self::ServiceApply {
                request, approval, ..
            } => {
                if request.is_empty() || request.len() > 65_536 {
                    return Err("service apply request must be in 1..=65536 bytes");
                }
                if approval.len() != 64 || !approval.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err("service apply approval must be a 64-hex digest");
                }
                Ok(())
            }
            Self::ServiceTransact {
                operation,
                name,
                definition,
                ttl_seconds,
                ..
            } => {
                if !(1..=600).contains(ttl_seconds) {
                    return Err("service transaction ttl_seconds must be in 1..=600");
                }
                if *operation == ServiceOperation::Bootstrap {
                    if name.is_some() || definition.is_none() {
                        return Err("service bootstrap transaction requires only a definition");
                    }
                } else if name.is_none() || definition.is_some() {
                    return Err("service lifecycle transaction requires only a service name");
                }
                if let Some(name) = name {
                    validate_service_name(name)?;
                }
                if definition.as_ref().is_some_and(|path| {
                    path.is_empty() || path.len() > 8_192 || path.as_bytes().contains(&0)
                }) {
                    return Err("service definition must be in 1..=8192 non-NUL bytes");
                }
                Ok(())
            }
            Self::LoginSessionApplyLock {
                request, approval, ..
            } => {
                if request.is_empty() || request.len() > 32_768 {
                    return Err("login-session apply request must be in 1..=32768 bytes");
                }
                if approval.len() != 64 || !approval.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err("login-session apply approval must be a 64-hex digest");
                }
                Ok(())
            }
            Self::JobSpawn {
                command,
                environment,
                cwd,
                limits,
                ttl_seconds,
                ..
            } => {
                validate_job_command(command)?;
                validate_job_environment(environment)?;
                if cwd
                    .as_ref()
                    .is_some_and(|path| path.len() > JOB_CWD_BYTES_MAX)
                {
                    return Err("managed-job cwd exceeds 8 KiB");
                }
                if !(1..=JOB_TTL_SECONDS_MAX).contains(ttl_seconds) {
                    return Err("managed-job ttl_seconds must be in 1..=86400");
                }
                if let Some(limits) = limits {
                    limits.validate()?;
                }
                Ok(())
            }
            Self::JobAdopt {
                pid,
                start_identity,
                ttl_seconds,
                stop_on_expiry,
                force,
                ..
            } => {
                if *pid <= 1 {
                    return Err("managed-job adopt pid must be in 2..=4294967295");
                }
                if start_identity.is_empty()
                    || start_identity.len() > 512
                    || start_identity.bytes().any(|byte| byte == 0)
                {
                    return Err(
                        "managed-job adopt start_identity must be in 1..=512 non-NUL bytes",
                    );
                }
                if !(1..=JOB_TTL_SECONDS_MAX).contains(ttl_seconds) {
                    return Err("managed-job ttl_seconds must be in 1..=86400");
                }
                if *stop_on_expiry != *force {
                    return Err(
                        "managed-job adopt expiry=stop requires --force, and --force is otherwise invalid",
                    );
                }
                Ok(())
            }
            Self::JobList { max, .. } => {
                if max.is_some_and(|value| !(1..=JOB_LIST_MAX).contains(&value)) {
                    return Err("managed-job list max must be in 1..=1024");
                }
                Ok(())
            }
            Self::JobStatus { job_id, .. } => validate_job_id(job_id),
            Self::JobPrune {
                max_age_seconds,
                keep_newest,
                ..
            } => {
                if *max_age_seconds > JOB_PRUNE_MAX_AGE_SECONDS_MAX {
                    return Err("managed-job prune max_age_seconds must be at most 315360000");
                }
                if *keep_newest > JOB_LIST_MAX {
                    return Err("managed-job prune keep_newest must be at most 1024");
                }
                Ok(())
            }
            Self::JobResources {
                job_id,
                generation,
                watch_ms,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                if watch_ms.is_some_and(|value| !(1..=JOB_RESOURCES_WATCH_MS_MAX).contains(&value))
                {
                    return Err("managed-job resources watch_ms must be in 1..=300000");
                }
                Ok(())
            }
            Self::JobPriority {
                job_id,
                generation,
                nice,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                if !(-20..=19).contains(nice) {
                    return Err("managed-job priority nice must be in -20..=19");
                }
                Ok(())
            }
            Self::JobEvents {
                job_id,
                generation,
                stdout_cursor,
                stderr_cursor,
                timeout_ms,
                max_bytes,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                validate_job_cursor(stdout_cursor.as_str())?;
                validate_job_cursor(stderr_cursor.as_str())?;
                if *timeout_ms > JOB_EVENTS_TIMEOUT_MS_MAX {
                    return Err("managed-job events timeout_ms must be at most 300000");
                }
                if !(2..=JOB_EVENTS_BYTES_MAX).contains(max_bytes) {
                    return Err("managed-job events max_bytes must be in 2..=1048576");
                }
                Ok(())
            }
            Self::JobOutput {
                job_id,
                generation,
                cursor,
                max_bytes,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                validate_job_cursor(cursor.as_str())?;
                if !(1..=JOB_EVENTS_BYTES_MAX).contains(max_bytes) {
                    return Err("managed-job output max_bytes must be in 1..=1048576");
                }
                Ok(())
            }
            Self::JobWrite {
                job_id,
                generation,
                data_base64,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                validate_job_write_base64(data_base64)
            }
            Self::JobWait {
                job_id,
                generation,
                timeout_ms,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                if *timeout_ms > JOB_WAIT_TIMEOUT_MS_MAX {
                    return Err("managed-job wait timeout_ms must be at most 86400000");
                }
                Ok(())
            }
            Self::JobSetState {
                job_id,
                generation,
                timeout_ms,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                if !(1..=60_000).contains(timeout_ms) {
                    return Err("managed-job state timeout_ms must be in 1..=60000");
                }
                Ok(())
            }
            Self::JobSignal {
                job_id,
                generation,
                signal,
                timeout_ms,
                force,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                if !(1..=60_000).contains(timeout_ms) {
                    return Err("managed-job signal timeout_ms must be in 1..=60000");
                }
                if !matches!(
                    signal,
                    ProcessSignalKind::Stop | ProcessSignalKind::Continue
                ) || *force
                {
                    return Err(
                        "managed-job signal currently accepts only retry-safe SIGSTOP or SIGCONT without --force",
                    );
                }
                Ok(())
            }
            Self::JobStop {
                job_id,
                generation,
                grace_ms,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                if *grace_ms > JOB_STOP_GRACE_MS_MAX {
                    return Err("managed-job stop grace_ms must be at most 60000");
                }
                Ok(())
            }
            Self::JobRenew {
                job_id,
                generation,
                ttl_seconds,
                ..
            } => {
                validate_job_id(job_id)?;
                validate_job_generation(*generation)?;
                if !(1..=JOB_TTL_SECONDS_MAX).contains(ttl_seconds) {
                    return Err("managed-job ttl_seconds must be in 1..=86400");
                }
                Ok(())
            }
            Self::SimulatorDevices { max, .. } => validate_simulator_max(*max),
            Self::StorageDevices { max, .. } => {
                if !(1..=STORAGE_DEVICES_MAX).contains(max) {
                    return Err("storage devices max must be in 1..=5000");
                }
                Ok(())
            }
            Self::DeviceList { max, .. } => {
                if !(1..=DEVICE_INVENTORY_MAX).contains(max) {
                    return Err("device inventory max must be in 1..=5000");
                }
                Ok(())
            }
            Self::DeviceWatch {
                max,
                duration_ms,
                interval_ms,
                event_max,
                ..
            } => {
                if !(1..=DEVICE_INVENTORY_MAX).contains(max)
                    || !(1_000..=DEVICE_WATCH_DURATION_MS_MAX).contains(duration_ms)
                    || !(DEVICE_WATCH_INTERVAL_MS_MIN..=DEVICE_WATCH_INTERVAL_MS_MAX)
                        .contains(interval_ms)
                    || !(1..=DEVICE_WATCH_EVENTS_MAX).contains(event_max)
                {
                    return Err(
                        "device watch requires max in 1..=5000, duration_ms in 1000..=3600000, interval_ms in 250..=60000 and event_max in 1..=5000",
                    );
                }
                Ok(())
            }
            Self::DeviceClaims { offset, max, .. } => {
                if offset.is_some_and(|value| value > DEVICE_LEASE_LIST_MAX)
                    || max.is_some_and(|value| !(1..=DEVICE_LEASE_LIST_MAX).contains(&value))
                {
                    return Err("device claims requires offset in 0..=1024 and max in 1..=1024");
                }
                Ok(())
            }
            Self::DeviceClaim {
                device_id,
                ttl_seconds,
                serial,
                ..
            } => {
                validate_device_public_id(device_id)?;
                validate_device_ttl(*ttl_seconds)?;
                if let Some(serial) = serial {
                    validate_device_serial(serial)?;
                }
                Ok(())
            }
            Self::DeviceStatus {
                lease_id,
                generation,
                ..
            } => {
                validate_device_lease_identity(lease_id, *generation)?;
                Ok(())
            }
            Self::DeviceRead {
                lease_id,
                generation,
                lease,
                max_bytes,
                timeout_ms,
                ..
            } => {
                validate_device_lease_identity(lease_id, *generation)?;
                validate_device_lease_secret(lease)?;
                if !(1..=DEVICE_IO_BYTES_MAX).contains(max_bytes) {
                    return Err("device read max_bytes must be in 1..=65536");
                }
                validate_device_io_timeout(*timeout_ms)
            }
            Self::DeviceWrite {
                lease_id,
                generation,
                lease,
                data,
                encoding,
                timeout_ms,
                ..
            } => {
                validate_device_lease_identity(lease_id, *generation)?;
                validate_device_lease_secret(lease)?;
                validate_device_encoded_data(data, *encoding)?;
                validate_device_io_timeout(*timeout_ms)
            }
            Self::DeviceRenew {
                lease_id,
                generation,
                lease,
                ttl_seconds,
                ..
            } => {
                validate_device_lease_identity(lease_id, *generation)?;
                validate_device_lease_secret(lease)?;
                validate_device_ttl(*ttl_seconds)
            }
            Self::DeviceRelease {
                lease_id,
                generation,
                lease,
                ..
            } => {
                validate_device_lease_identity(lease_id, *generation)?;
                validate_device_lease_secret(lease)
            }
            Self::SimulatorBoot {
                udid,
                timeout_ms,
                expect_booted,
                ..
            } => {
                validate_simulator_udid(udid)?;
                validate_simulator_timeout_ms(*timeout_ms)?;
                if !expect_booted {
                    return Err("simulator boot requires expect_booted=true");
                }
                Ok(())
            }
            Self::SimulatorApps { udid, max, .. } => {
                validate_simulator_udid(udid)?;
                validate_simulator_max(*max)
            }
            Self::SimulatorLaunch {
                udid,
                bundle_id,
                timeout_ms,
                expect_accepted,
                ..
            }
            | Self::SimulatorTerminate {
                udid,
                bundle_id,
                timeout_ms,
                expect_accepted,
                ..
            } => {
                validate_simulator_udid(udid)?;
                validate_simulator_bundle_id(bundle_id)?;
                validate_simulator_timeout_ms(*timeout_ms)?;
                if !expect_accepted {
                    return Err("simulator app lifecycle requires expect_accepted=true");
                }
                Ok(())
            }
            Self::TermRead {
                window,
                tail,
                max_bytes,
                ..
            } => {
                if *window == 0
                    || tail.is_some_and(|value| !(1..=100_000).contains(&value))
                    || !(1..=1_048_576).contains(max_bytes)
                {
                    return Err("term-read fields are outside their bounded contract");
                }
                Ok(())
            }
            Self::TermSend {
                window,
                text,
                expect,
                enter,
                verify_timeout_ms,
                ..
            } => {
                if *window == 0
                    || (text.is_empty() && !enter)
                    || (text.is_empty() && expect.is_none())
                    || text.len() > 65_536
                    || expect
                        .as_ref()
                        .is_some_and(|pattern| pattern.is_empty() || pattern.len() > 4_096)
                    || !(1..=30_000).contains(verify_timeout_ms)
                {
                    return Err("term-send fields are outside their bounded contract");
                }
                Ok(())
            }
            Self::TermWait {
                window,
                pattern,
                timeout_ms,
                interval_ms,
                max_bytes,
                ..
            } => {
                if *window == 0
                    || pattern.is_empty()
                    || pattern.len() > 4_096
                    || !(1..=86_400_000).contains(timeout_ms)
                    || !(10..=10_000).contains(interval_ms)
                    || !(1..=1_048_576).contains(max_bytes)
                {
                    return Err("term-wait fields are outside their bounded contract");
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn validate_simulator_max(max: usize) -> Result<(), &'static str> {
    if !(1..=SIMULATOR_RESULTS_MAX).contains(&max) {
        return Err("simulator max must be in 1..=200");
    }
    Ok(())
}

fn validate_simulator_timeout_ms(timeout_ms: u64) -> Result<(), &'static str> {
    if !(1..=SIMULATOR_TIMEOUT_MS_MAX).contains(&timeout_ms) {
        return Err("simulator timeout_ms must be in 1..=600000");
    }
    Ok(())
}

fn validate_job_id(job_id: &str) -> Result<(), &'static str> {
    let bytes = job_id.as_bytes();
    let valid = bytes.len() == 36
        && bytes.get(8) == Some(&b'-')
        && bytes.get(13) == Some(&b'-')
        && bytes.get(18) == Some(&b'-')
        && bytes.get(23) == Some(&b'-')
        && bytes.get(14) == Some(&b'4')
        && matches!(bytes.get(19), Some(b'8' | b'9' | b'a' | b'b'))
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23)
                || byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
        });
    if !valid {
        return Err("managed-job id must be a lowercase UUID v4");
    }
    Ok(())
}

fn validate_job_generation(generation: u64) -> Result<(), &'static str> {
    if generation == 0 {
        return Err("managed-job generation must be nonzero");
    }
    Ok(())
}

fn validate_device_public_id(device_id: &str) -> Result<(), &'static str> {
    if device_id.len() != 78
        || !device_id.starts_with("agt-device-v1-")
        || !is_lower_hex(&device_id[14..])
    {
        return Err("device id must be one installation-scoped agt-device-v1 identifier");
    }
    Ok(())
}

fn validate_device_lease_identity(lease_id: &str, generation: u64) -> Result<(), &'static str> {
    validate_job_id(lease_id).map_err(|_| "device lease id must be a lowercase UUID v4")?;
    if generation == 0 {
        return Err("device lease generation must be nonzero");
    }
    Ok(())
}

fn validate_device_lease_secret(lease: &str) -> Result<(), &'static str> {
    if lease.len() != 64 || !is_lower_hex(lease) {
        return Err("device lease secret must be 64 lowercase hexadecimal bytes");
    }
    Ok(())
}

fn validate_device_ttl(ttl_seconds: u64) -> Result<(), &'static str> {
    if !(1..=DEVICE_LEASE_TTL_SECONDS_MAX).contains(&ttl_seconds) {
        return Err("device lease ttl_seconds must be in 1..=86400");
    }
    Ok(())
}

fn validate_device_io_timeout(timeout_ms: u64) -> Result<(), &'static str> {
    if !(1..=DEVICE_IO_TIMEOUT_MS_MAX).contains(&timeout_ms) {
        return Err("device I/O timeout_ms must be in 1..=300000");
    }
    Ok(())
}

fn validate_device_serial(serial: &DeviceSerialConfiguration) -> Result<(), &'static str> {
    const BAUDS: [u32; 19] = [
        50, 75, 110, 134, 150, 200, 300, 600, 1_200, 1_800, 2_400, 4_800, 9_600, 19_200, 38_400,
        57_600, 115_200, 230_400, 460_800,
    ];
    if !BAUDS.contains(&serial.baud)
        || !(5..=8).contains(&serial.data_bits)
        || !matches!(serial.stop_bits, 1 | 2)
    {
        return Err("device serial configuration is outside the closed serial contract");
    }
    Ok(())
}

fn validate_device_encoded_data(
    data: &str,
    encoding: DeviceDataEncoding,
) -> Result<(), &'static str> {
    match encoding {
        DeviceDataEncoding::Hex => {
            if data.is_empty()
                || !data.len().is_multiple_of(2)
                || data.len() / 2 > DEVICE_IO_BYTES_MAX
                || !data.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err("device write hex must encode 1..=65536 bytes");
            }
        }
        DeviceDataEncoding::Base64 => {
            if data.is_empty() || data.len() > 4 * DEVICE_IO_BYTES_MAX.div_ceil(3) {
                return Err("device write base64 must encode 1..=65536 bytes");
            }
            let padding = data.bytes().rev().take_while(|byte| *byte == b'=').count();
            let alphabet = data.len().saturating_sub(padding);
            if !data.len().is_multiple_of(4)
                || padding > 2
                || !data[..alphabet]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
                || !data[alphabet..].bytes().all(|byte| byte == b'=')
            {
                return Err("device write base64 is not canonical padded base64");
            }
            let decoded = data.len() / 4 * 3 - padding;
            if decoded == 0 || decoded > DEVICE_IO_BYTES_MAX {
                return Err("device write base64 must encode 1..=65536 bytes");
            }
        }
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_service_name(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 1_024
        || value.chars().any(char::is_control)
        || value.contains('/')
    {
        Err("service name must be in 1..=1024 non-control bytes without '/'")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Grant;

    const TEST_JOB_ID: &str = "00000000-0000-4000-8000-000000000001";

    #[test]
    fn managed_job_cohort_round_trips_closed_wire_shapes_and_grants() {
        let spawn = serde_json::json!({
            "verb": "job-spawn",
            "target": "ssh",
            "command": ["tool", "--flag"],
            "environment": [{"name": "MODE", "value": "test"}],
            "cwd": "work",
            "ttl_seconds": 3_600
        });
        let spawn_command: Command = serde_json::from_value(spawn.clone()).expect("spawn");
        assert_eq!(spawn_command.verb(), "job-spawn");
        assert_eq!(spawn_command.target(), TargetRef::Ssh);
        assert_eq!(spawn_command.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&spawn_command).expect("serialize spawn"),
            spawn
        );
        spawn_command.validate().expect("valid direct command");

        let adopt = serde_json::json!({
            "verb": "job-adopt",
            "target": "ssh",
            "pid": 42,
            "start_identity": "opaque-start",
            "ttl_seconds": 3_600,
            "stop_on_expiry": false,
            "force": false
        });
        let adopt_command: Command = serde_json::from_value(adopt.clone()).expect("adopt");
        assert_eq!(adopt_command.verb(), "job-adopt");
        assert_eq!(adopt_command.target(), TargetRef::Ssh);
        assert_eq!(adopt_command.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&adopt_command).expect("serialize adopt"),
            adopt
        );
        adopt_command.validate().expect("valid adopt command");

        let stop_adopt: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-adopt",
            "target": "current",
            "pid": 42,
            "start_identity": "opaque-start",
            "ttl_seconds": 3_600,
            "stop_on_expiry": true,
            "force": true
        }))
        .expect("stop-on-expiry adopt");
        stop_adopt.validate().expect("confirmed stop-on-expiry");

        let list: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-list",
            "target": "current",
            "state": "running",
            "offset": 2,
            "max": 10
        }))
        .expect("list");
        assert_eq!(list.verb(), "job-list");
        assert_eq!(list.required_grant(), Grant::Observe);

        let prune_plan: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-prune", "target": "current", "max_age_seconds": 86400,
            "keep_newest": 128
        }))
        .expect("prune plan");
        assert_eq!(
            prune_plan.authorization_operation().as_deref(),
            Some("job-prune.plan")
        );
        assert_eq!(prune_plan.required_grant(), Grant::Observe);
        let prune_apply: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-prune", "target": "current", "max_age_seconds": 0,
            "keep_newest": 0, "apply": true
        }))
        .expect("prune apply");
        assert_eq!(
            prune_apply.authorization_operation().as_deref(),
            Some("job-prune.apply")
        );
        assert_eq!(prune_apply.required_grant(), Grant::Actuate);

        let events = serde_json::json!({
            "verb": "job-events",
            "target": "vnc",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "stdout_cursor": "12",
            "stderr_cursor": "34",
            "timeout_ms": 300_000,
            "max_bytes": 1_048_576
        });
        let events_command: Command = serde_json::from_value(events.clone()).expect("events");
        assert_eq!(events_command.verb(), "job-events");
        assert_eq!(events_command.target(), TargetRef::Vnc);
        assert_eq!(events_command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(events_command).expect("serialize events"),
            events
        );

        let output = serde_json::json!({
            "verb": "job-output",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "stream": "stderr",
            "cursor": "12",
            "max_bytes": 1_048_576
        });
        let output_command: Command = serde_json::from_value(output.clone()).expect("output");
        assert_eq!(output_command.verb(), "job-output");
        assert_eq!(output_command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(output_command).expect("serialize output"),
            output
        );

        let resources = serde_json::json!({
            "verb": "job-resources",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "watch_ms": 300_000
        });
        let resources_command: Command =
            serde_json::from_value(resources.clone()).expect("resources");
        assert_eq!(resources_command.verb(), "job-resources");
        assert_eq!(resources_command.required_grant(), Grant::Observe);
        resources_command.validate().expect("valid resources");
        assert_eq!(
            serde_json::to_value(resources_command).expect("serialize resources"),
            resources
        );
        let invalid_resources: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-resources",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "watch_ms": 300_001
        }))
        .expect("structurally valid resources");
        assert!(invalid_resources.validate().is_err());

        let priority = serde_json::json!({
            "verb": "job-priority",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "nice": 7
        });
        let priority_command: Command = serde_json::from_value(priority.clone()).expect("priority");
        assert_eq!(priority_command.verb(), "job-priority");
        assert_eq!(priority_command.required_grant(), Grant::Actuate);
        priority_command.validate().expect("valid priority");
        assert_eq!(
            serde_json::to_value(priority_command).expect("serialize priority"),
            priority
        );
        let invalid_priority: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-priority",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "nice": 20
        }))
        .expect("structurally valid priority");
        assert!(invalid_priority.validate().is_err());

        let write: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-write",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "data_base64": "AAEC",
            "close_stdin": true
        }))
        .expect("write");
        assert_eq!(write.verb(), "job-write");
        assert_eq!(write.required_grant(), Grant::Actuate);

        let wait: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-wait",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "timeout_ms": 86_400_000,
            "expect_exit": 0
        }))
        .expect("wait");
        assert_eq!(wait.verb(), "job-wait");
        assert_eq!(wait.required_grant(), Grant::Observe);

        let state = serde_json::json!({
            "verb": "job-set-state",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "state": "stopped",
            "timeout_ms": 5_000
        });
        let state_command: Command = serde_json::from_value(state.clone()).expect("set state");
        assert_eq!(state_command.verb(), "job-set-state");
        assert_eq!(state_command.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(state_command).expect("serialize set state"),
            state
        );

        let signal = serde_json::json!({
            "verb": "job-signal",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "signal": "SIGCONT",
            "timeout_ms": 5_000
        });
        let signal_command: Command = serde_json::from_value(signal.clone()).expect("signal");
        assert_eq!(signal_command.verb(), "job-signal");
        assert_eq!(signal_command.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(signal_command).expect("serialize signal"),
            signal
        );

        let stop: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-stop",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "grace_ms": 60_000,
            "expect_stopped": true
        }))
        .expect("stop");
        assert_eq!(stop.verb(), "job-stop");
        assert_eq!(stop.required_grant(), Grant::Actuate);

        let renew: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-renew",
            "target": "current",
            "job_id": TEST_JOB_ID,
            "generation": 7,
            "ttl_seconds": 86_400
        }))
        .expect("renew");
        assert_eq!(renew.verb(), "job-renew");
        assert_eq!(renew.required_grant(), Grant::Actuate);

        let status: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-status",
            "target": "current",
            "job_id": TEST_JOB_ID
        }))
        .expect("status does not require generation");
        assert_eq!(status.verb(), "job-status");
        assert_eq!(status.required_grant(), Grant::Observe);
    }

    #[test]
    fn managed_job_serde_rejects_unbounded_and_stale_inputs() {
        let reject =
            |value| serde_json::from_value::<Command>(value).expect_err("command must be rejected");

        reject(serde_json::json!({
            "verb": "job-spawn", "target": "current", "command": [], "ttl_seconds": 1
        }));
        reject(serde_json::json!({
            "verb": "job-spawn", "target": "current", "command": ["x"], "ttl_seconds": 0
        }));
        reject(serde_json::json!({
            "verb": "job-spawn", "target": "current", "command": ["x"], "ttl_seconds": 1,
            "limits": {}
        }));
        reject(serde_json::json!({
            "verb": "job-spawn", "target": "current", "command": ["x"], "ttl_seconds": 1,
            "limits": {"memory_bytes": 1048575}
        }));
        let limited: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-spawn", "target": "current", "command": ["x"], "ttl_seconds": 1,
            "limits": {
                "cpu_seconds": 60,
                "processes": 32
            }
        }))
        .expect("bounded launch limits");
        assert!(matches!(
            limited,
            Command::JobSpawn {
                limits: Some(_),
                ..
            }
        ));
        reject(serde_json::json!({
            "verb": "job-list", "target": "current", "max": 1025
        }));
        reject(serde_json::json!({
            "verb": "job-events", "target": "current", "job_id": TEST_JOB_ID, "generation": 0,
            "stdout_cursor": "0", "stderr_cursor": "0", "timeout_ms": 0, "max_bytes": 1
        }));
        reject(serde_json::json!({
            "verb": "job-events", "target": "current", "job_id": TEST_JOB_ID, "generation": 1,
            "stdout_cursor": "00", "stderr_cursor": "0", "timeout_ms": 0, "max_bytes": 1
        }));
        reject(serde_json::json!({
            "verb": "job-events", "target": "current", "job_id": TEST_JOB_ID, "generation": 1,
            "stdout_cursor": 0, "stderr_cursor": "0", "timeout_ms": 300001, "max_bytes": 1048577
        }));
        reject(serde_json::json!({
            "verb": "job-write", "target": "current", "job_id": TEST_JOB_ID, "generation": 1,
            "data_base64": "not base64"
        }));
        reject(serde_json::json!({
            "verb": "job-wait", "target": "current", "job_id": TEST_JOB_ID, "generation": 1,
            "timeout_ms": 86400001
        }));
        reject(serde_json::json!({
            "verb": "job-set-state", "target": "current", "job_id": TEST_JOB_ID,
            "generation": 1, "state": "stopped", "timeout_ms": 0
        }));
        let unsupported_signal: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-signal", "target": "current", "job_id": TEST_JOB_ID,
            "generation": 1, "signal": "SIGKILL", "timeout_ms": 5000
        }))
        .expect("closed signal enum");
        assert!(unsupported_signal.validate().is_err());
        let non_idempotent_signal: Command = serde_json::from_value(serde_json::json!({
            "verb": "job-signal", "target": "current", "job_id": TEST_JOB_ID,
            "generation": 1, "signal": "SIGUSR1", "timeout_ms": 5000
        }))
        .expect("closed signal enum");
        assert!(non_idempotent_signal.validate().is_err());
        reject(serde_json::json!({
            "verb": "job-stop", "target": "current", "job_id": TEST_JOB_ID, "generation": 1,
            "grace_ms": 60001, "expect_stopped": true
        }));
        reject(serde_json::json!({
            "verb": "job-renew", "target": "current", "job_id": TEST_JOB_ID, "generation": 1,
            "ttl_seconds": 86401
        }));
    }

    #[test]
    fn managed_job_large_collection_and_write_bounds_fail_closed() {
        let too_many_parts = vec!["x"; JOB_COMMAND_PARTS_MAX + 1];
        assert!(serde_json::from_value::<Command>(serde_json::json!({
            "verb": "job-spawn", "target": "current", "command": too_many_parts, "ttl_seconds": 1
        }))
        .is_err());

        let too_many_environment = (0..=JOB_ENVIRONMENT_ENTRIES_MAX)
            .map(|index| serde_json::json!({"name": format!("K{index}"), "value": "x"}))
            .collect::<Vec<_>>();
        assert!(
            serde_json::from_value::<Command>(serde_json::json!({
                "verb": "job-spawn", "target": "current", "command": ["x"],
                "environment": too_many_environment, "ttl_seconds": 1
            }))
            .is_err()
        );

        let oversized_base64 = "A".repeat((JOB_WRITE_DECODED_BYTES_MAX / 3 + 1) * 4);
        assert!(
            serde_json::from_value::<Command>(serde_json::json!({
                "verb": "job-write", "target": "current", "job_id": TEST_JOB_ID, "generation": 1,
                "data_base64": oversized_base64
            }))
            .is_err()
        );
    }

    #[test]
    fn device_screenshot_inventory_preserves_host_diagnostic_wire_shape() {
        let command = Command::DeviceScreenshot {
            target: TargetRef::Ssh,
            path: None,
            device: None,
            timeout_ms: None,
            list: true,
        };
        assert_eq!(command.verb(), "device-screenshot");
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(command.target(), TargetRef::Ssh);
        let encoded = serde_json::to_value(&command).expect("wire encode");
        assert_eq!(encoded["verb"], "device-screenshot");
        assert_eq!(encoded["list"], true);
        assert!(encoded.get("path").is_none());
        let decoded: Command = serde_json::from_value(encoded).expect("wire decode");
        assert!(matches!(
            decoded,
            Command::DeviceScreenshot {
                target: TargetRef::Ssh,
                list: true,
                ..
            }
        ));
    }

    #[test]
    fn shell_exec_is_distinct_from_transport_exec_and_keeps_closed_budgets() {
        let command = Command::ShellExec {
            target: TargetRef::Ssh,
            command: "printf marker".into(),
            timeout_ms: 10_000,
            max_output_bytes: 1_048_576,
        };
        assert_eq!(command.verb(), "shell-exec");
        assert_eq!(command.required_grant(), Grant::Actuate);
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(
            serde_json::to_value(command).expect("serialize"),
            serde_json::json!({
                "verb": "shell-exec",
                "target": "ssh",
                "command": "printf marker",
                "timeout_ms": 10_000,
                "max_output_bytes": 1_048_576,
            })
        );
    }

    #[test]
    fn setup_has_distinct_read_only_check_and_current_only_apply_shapes() {
        let check = Command::Setup {
            target: TargetRef::Current,
            action: SetupAction::Check,
            bin_dir: Some("fixture-bin".into()),
        };
        assert_eq!(check.verb(), "setup");
        assert_eq!(check.required_grant(), Grant::Observe);
        assert_eq!(check.validate(), Ok(()));
        let value = serde_json::to_value(&check).expect("serialize");
        assert_eq!(value["action"], "check");
        assert_eq!(value["bin_dir"], "fixture-bin");
        let back: Command = serde_json::from_value(value).expect("deserialize");
        assert!(matches!(
            back,
            Command::Setup {
                target: TargetRef::Current,
                action: SetupAction::Check,
                ..
            }
        ));

        let apply = Command::Setup {
            target: TargetRef::Current,
            action: SetupAction::Apply,
            bin_dir: None,
        };
        assert_eq!(apply.required_grant(), Grant::Actuate);
        let value = serde_json::to_value(&apply).expect("serialize default action");
        assert!(value.get("action").is_none());

        let remote = Command::Setup {
            target: TargetRef::Ssh,
            action: SetupAction::Check,
            bin_dir: None,
        };
        assert_eq!(remote.validate(), Err("setup supports only target=current"));
    }

    #[test]
    fn permissions_is_a_first_class_observe_wire_command() {
        let command = Command::Permissions {
            target: TargetRef::Ssh,
            action: PermissionAction::Status,
            permission: None,
        };
        assert_eq!(command.verb(), "permissions");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Observe);
        let value = serde_json::to_value(&command).expect("serialize");
        assert_eq!(value["verb"], "permissions");
        let back: Command = serde_json::from_value(value).expect("deserialize");
        assert!(matches!(
            back,
            Command::Permissions {
                target: TargetRef::Ssh,
                action: PermissionAction::Status,
                permission: None,
            }
        ));

        let open = Command::Permissions {
            target: TargetRef::Current,
            action: PermissionAction::Open,
            permission: Some(PermissionKind::Accessibility),
        };
        assert_eq!(open.required_grant(), Grant::Actuate);
    }

    #[test]
    fn doctor_is_a_first_class_observe_wire_command() {
        let command = Command::Doctor {
            target: TargetRef::Vnc,
        };
        assert_eq!(command.verb(), "doctor");
        assert_eq!(command.target(), TargetRef::Vnc);
        assert_eq!(command.required_grant(), Grant::Observe);
        let value = serde_json::to_value(&command).expect("serialize");
        assert_eq!(value["verb"], "doctor");
        let back: Command = serde_json::from_value(value).expect("deserialize");
        assert!(matches!(
            back,
            Command::Doctor {
                target: TargetRef::Vnc
            }
        ));
    }

    #[test]
    fn runtime_status_is_a_first_class_observe_wire_command() {
        let command = Command::RuntimeStatus {
            target: TargetRef::Ssh,
        };
        assert_eq!(command.verb(), "runtime-status");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Observe);
        let value = serde_json::to_value(&command).expect("serialize");
        assert_eq!(value["verb"], "runtime-status");
        let back: Command = serde_json::from_value(value).expect("deserialize");
        assert!(matches!(
            back,
            Command::RuntimeStatus {
                target: TargetRef::Ssh
            }
        ));
    }

    #[test]
    fn audio_shapes_keep_exact_targets_and_mixed_grants_on_the_wire() {
        let status = Command::AudioStatus {
            target: TargetRef::Ssh,
        };
        let plan_volume = Command::AudioPlanVolume {
            target: TargetRef::Vnc,
            volume: 37,
            ttl_seconds: 60,
        };
        let plan_muted = Command::AudioPlanMuted {
            target: TargetRef::Current,
            muted: true,
            ttl_seconds: 30,
        };
        let apply = Command::AudioApply {
            target: TargetRef::Current,
            request: "REQUEST".into(),
            approval: "a".repeat(64),
        };
        assert_eq!(status.required_grant(), Grant::Observe);
        assert_eq!(plan_volume.required_grant(), Grant::Observe);
        assert_eq!(plan_muted.required_grant(), Grant::Observe);
        assert_eq!(apply.required_grant(), Grant::Actuate);
        assert_eq!(status.target(), TargetRef::Ssh);
        assert_eq!(plan_volume.target(), TargetRef::Vnc);
        for (command, expected_wire_verb) in [
            (status, "audio-status"),
            (plan_volume, "audio-plan-volume"),
            (plan_muted, "audio-plan-muted"),
            (apply, "audio-apply"),
        ] {
            assert_eq!(command.verb(), "audio");
            let value = serde_json::to_value(&command).expect("serialize");
            assert_eq!(value["verb"], expected_wire_verb);
            let back: Command = serde_json::from_value(value).expect("deserialize");
            assert_eq!(back.verb(), "audio");
        }
    }

    #[test]
    fn service_shapes_keep_exact_targets_and_mixed_grants_on_the_wire() {
        let list = Command::ServiceList {
            target: TargetRef::Ssh,
            scope: ServiceScope::System,
            match_text: Some("agent".into()),
            max: 500,
        };
        let status = Command::ServiceStatus {
            target: TargetRef::Current,
            scope: ServiceScope::User,
            name: "example.service".into(),
        };
        let plan = Command::ServicePlan {
            target: TargetRef::Vnc,
            scope: ServiceScope::User,
            name: "example.service".into(),
            operation: ServiceOperation::Restart,
            definition: None,
            ttl_seconds: 60,
        };
        let apply = Command::ServiceApply {
            target: TargetRef::Current,
            request: "REQUEST".into(),
            approval: "a".repeat(64),
        };
        let transact = Command::ServiceTransact {
            target: TargetRef::Current,
            scope: ServiceScope::User,
            operation: ServiceOperation::Restart,
            name: Some("example.service".into()),
            definition: None,
            ttl_seconds: 60,
        };
        assert_eq!(list.required_grant(), Grant::Observe);
        assert_eq!(status.required_grant(), Grant::Observe);
        assert_eq!(plan.required_grant(), Grant::Observe);
        assert_eq!(apply.required_grant(), Grant::Actuate);
        assert_eq!(transact.required_grant(), Grant::Actuate);
        for (command, expected_wire_verb) in [
            (list, "service-list"),
            (status, "service-status"),
            (plan, "service-plan"),
            (apply, "service-apply"),
            (transact, "service-transact"),
        ] {
            assert_eq!(command.verb(), "service");
            assert_eq!(command.validate(), Ok(()));
            let value = serde_json::to_value(&command).expect("serialize");
            assert_eq!(value["verb"], expected_wire_verb);
            let back: Command = serde_json::from_value(value).expect("deserialize");
            assert_eq!(back.verb(), "service");
        }
    }

    #[test]
    fn login_session_shapes_keep_exact_targets_and_mixed_grants_on_the_wire() {
        let status = Command::LoginSessionStatus {
            target: TargetRef::Ssh,
        };
        let plan = Command::LoginSessionPlanLock {
            target: TargetRef::Vnc,
            ttl_seconds: 60,
        };
        let apply = Command::LoginSessionApplyLock {
            target: TargetRef::Current,
            request: "REQUEST".into(),
            approval: "a".repeat(64),
        };
        assert_eq!(status.required_grant(), Grant::Observe);
        assert_eq!(plan.required_grant(), Grant::Observe);
        assert_eq!(apply.required_grant(), Grant::Actuate);
        assert_eq!(status.target(), TargetRef::Ssh);
        assert_eq!(plan.target(), TargetRef::Vnc);
        assert_eq!(apply.target(), TargetRef::Current);
        for (command, expected_wire_verb) in [
            (status, "login-session-status"),
            (plan, "login-session-plan-lock"),
            (apply, "login-session-apply-lock"),
        ] {
            assert_eq!(command.verb(), "login-session");
            let value = serde_json::to_value(&command).expect("serialize");
            assert_eq!(value["verb"], expected_wire_verb);
            let back: Command = serde_json::from_value(value).expect("deserialize");
            assert_eq!(back.verb(), "login-session");
        }
    }

    #[test]
    fn host_open_is_a_first_class_actuate_wire_command() {
        let command = Command::HostOpen {
            target: TargetRef::Ssh,
            value: "https://example.invalid".into(),
            application: Some("Browser".into()),
            background: true,
        };
        assert_eq!(command.verb(), "host-open");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Actuate);
        let value = serde_json::to_value(&command).expect("serialize");
        assert_eq!(value["verb"], "host-open");
        let back: Command = serde_json::from_value(value).expect("deserialize");
        assert!(matches!(
            back,
            Command::HostOpen {
                target: TargetRef::Ssh,
                background: true,
                ..
            }
        ));
    }

    #[test]
    fn ps_is_an_observe_command_with_a_closed_remote_wire_shape() {
        let command = Command::Ps {
            target: TargetRef::Ssh,
            pid: Some(42),
            parent: Some(7),
            name: Some("worker".into()),
            app: None,
            command: None,
            cpu_above_percent: None,
            memory_above_mb: None,
            sort: None,
            sample_ms: None,
            max_visited: None,
            depth: None,
            files: false,
            ports: false,
            meta: false,
            offset: Some(3),
            max: Some(9),
        };
        assert_eq!(command.verb(), "ps");
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "ps",
                "target": "ssh",
                "pid": 42,
                "parent": 7,
                "name": "worker",
                "offset": 3,
                "max": 9,
            })
        );
    }

    #[test]
    fn process_state_is_observe_only_and_has_a_closed_remote_wire_shape() {
        let command = Command::ProcessState {
            target: TargetRef::Vnc,
            pid: 42,
        };
        assert_eq!(command.verb(), "process-state");
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-state",
                "target": "vnc",
                "pid": 42,
            })
        );
    }

    #[test]
    fn process_argv_is_observe_only_and_keeps_disclosure_and_page_on_the_wire() {
        let command = Command::ProcessArgv {
            target: TargetRef::Ssh,
            pid: 42,
            values: true,
            offset: Some(3),
            limit: Some(9),
        };
        assert_eq!(command.verb(), "process-argv");
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-argv",
                "target": "ssh",
                "pid": 42,
                "values": true,
                "offset": 3,
                "limit": 9,
            })
        );
    }

    #[test]
    fn process_cwd_is_observe_only_and_has_a_closed_remote_wire_shape() {
        let command = Command::ProcessCwd {
            target: TargetRef::Ssh,
            pid: 42,
        };
        assert_eq!(command.verb(), "process-cwd");
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-cwd",
                "target": "ssh",
                "pid": 42,
            })
        );
    }

    #[test]
    fn process_environment_keeps_disclosure_filter_and_page_on_the_wire() {
        let command = Command::ProcessEnvironment {
            target: TargetRef::Ssh,
            pid: 42,
            prefix: Some("APP_".into()),
            values: true,
            offset: Some(3),
            limit: Some(9),
        };
        assert_eq!(command.verb(), "process-environment");
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-environment",
                "target": "ssh",
                "pid": 42,
                "prefix": "APP_",
                "values": true,
                "offset": 3,
                "limit": 9,
            })
        );
    }

    #[test]
    fn process_usage_is_observe_only_and_has_a_closed_remote_wire_shape() {
        let command = Command::ProcessUsage {
            target: TargetRef::Ssh,
            pid: 42,
            watch_ms: None,
            interval_ms: None,
            max_samples: None,
        };
        assert_eq!(command.verb(), "process-usage");
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-usage",
                "target": "ssh",
                "pid": 42,
            })
        );
    }

    #[test]
    fn process_usage_watch_has_bounded_remote_wire_fields() {
        let command = Command::ProcessUsage {
            target: TargetRef::Vnc,
            pid: 42,
            watch_ms: Some(1_000),
            interval_ms: Some(100),
            max_samples: Some(4),
        };
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-usage",
                "target": "vnc",
                "pid": 42,
                "watch_ms": 1_000,
                "interval_ms": 100,
                "max_samples": 4,
            })
        );
    }

    #[test]
    fn process_wait_is_observe_only_and_binds_the_remote_wire_to_an_identity() {
        let command = Command::ProcessWait {
            target: TargetRef::Vnc,
            pid: 42,
            start_identity: "boot:123".into(),
            timeout_ms: 250,
        };
        assert_eq!(command.verb(), "process-wait");
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-wait",
                "target": "vnc",
                "pid": 42,
                "start_identity": "boot:123",
                "timeout_ms": 250,
            })
        );
    }

    #[test]
    fn process_kill_is_actuate_and_keeps_the_destructive_contract_on_the_wire() {
        let command = Command::ProcessKill {
            target: TargetRef::Ssh,
            pid: 42,
            start_identity: "boot:123".into(),
            mode: ProcessKillMode::Forceful,
            timeout_ms: 250,
            expect_exited: true,
        };
        assert_eq!(command.verb(), "process-kill");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-kill",
                "target": "ssh",
                "pid": 42,
                "start_identity": "boot:123",
                "mode": "forceful",
                "timeout_ms": 250,
                "expect_exited": true,
            })
        );
    }

    #[test]
    fn process_set_state_is_identity_bound_actuation_on_the_wire() {
        let command = Command::ProcessSetState {
            target: TargetRef::Vnc,
            pid: 42,
            start_identity: "boot:123".into(),
            state: ProcessRunState::Stopped,
            timeout_ms: 250,
        };
        assert_eq!(command.verb(), "process-set-state");
        assert_eq!(command.target(), TargetRef::Vnc);
        assert_eq!(command.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-set-state",
                "target": "vnc",
                "pid": 42,
                "start_identity": "boot:123",
                "state": "stopped",
                "timeout_ms": 250,
            })
        );
    }

    #[test]
    fn process_policy_separates_observation_from_identity_bound_intent() {
        let status = Command::ProcessPolicy {
            target: TargetRef::Current,
            pid: 42,
            action: ProcessPolicyAction::Status,
            start_identity: None,
        };
        assert_eq!(status.verb(), "process-policy");
        assert_eq!(status.required_grant(), Grant::Observe);
        assert_eq!(status.authorization_operation(), None);

        let background = Command::ProcessPolicy {
            target: TargetRef::Ssh,
            pid: 42,
            action: ProcessPolicyAction::Background,
            start_identity: Some("boot:123".into()),
        };
        assert_eq!(background.required_grant(), Grant::Actuate);
        assert_eq!(
            background.authorization_operation().as_deref(),
            Some("process-policy.background")
        );
        assert_eq!(
            serde_json::to_value(&background).expect("serialize"),
            serde_json::json!({
                "verb": "process-policy",
                "target": "ssh",
                "pid": 42,
                "action": "background",
                "start_identity": "boot:123",
            })
        );
    }

    #[test]
    fn process_signal_keeps_closed_exact_object_intent_on_the_wire() {
        let command = Command::ProcessSignal {
            target: TargetRef::Ssh,
            pid: 42,
            start_identity: Some("boot:123".into()),
            signal: ProcessSignalKind::User1,
            timeout_ms: 250,
            force: false,
            tree: true,
            max_descendants: 64,
        };
        assert_eq!(command.verb(), "process-signal");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-signal",
                "target": "ssh",
                "pid": 42,
                "start_identity": "boot:123",
                "signal": "SIGUSR1",
                "timeout_ms": 250,
                "force": false,
                "tree": true,
                "max_descendants": 64,
            })
        );
    }

    #[test]
    fn privilege_plan_is_observe_only_and_keeps_the_expiring_contract_on_the_wire() {
        let command = Command::PrivilegePlanProcessPriority {
            target: TargetRef::Ssh,
            pid: 42,
            nice: 10,
            ttl_seconds: 120,
        };
        assert_eq!(command.verb(), "privilege-plan");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "privilege-plan-process-priority",
                "target": "ssh",
                "pid": 42,
                "nice": 10,
                "ttl_seconds": 120,
            })
        );
    }

    #[test]
    fn process_watch_is_observe_only_and_has_bounded_remote_fields() {
        let command = Command::ProcessWatch {
            target: TargetRef::Ssh,
            pid: None,
            parent: None,
            name: Some("worker".into()),
            all: false,
            duration_ms: 1_000,
            interval_ms: Some(100),
            max_events: Some(8),
            max_processes: Some(20),
        };
        assert_eq!(command.verb(), "process-watch");
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "process-watch",
                "target": "ssh",
                "name": "worker",
                "duration_ms": 1_000,
                "interval_ms": 100,
                "max_events": 8,
                "max_processes": 20,
            })
        );
    }

    #[test]
    fn cdp_page_verbs_carry_their_grant_and_wire_shape() {
        let find = Command::PageFind {
            target: TargetRef::Current,
            port: None,
            pid: None,
            target_id: Some("B2".into()),
            target_url: None,
            target_title: None,
            target_match: None,
            selector: None,
            text: Some("Go".into()),
            role: None,
            name: None,
        };
        assert_eq!(find.verb(), "page-find");
        assert_eq!(find.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&find).expect("serialize"),
            serde_json::json!({ "verb": "page-find", "target": "current", "target_id": "B2", "text": "Go" })
        );
        let click: Command = serde_json::from_value(serde_json::json!({
            "verb": "page-click", "target": "current", "target_title": "B", "node": 17, "button": "right", "clicks": 2
        }))
        .expect("deserialize");
        assert_eq!(click.verb(), "page-click");
        assert_eq!(click.required_grant(), Grant::Actuate);
        assert!(matches!(
            click,
            Command::PageClick {
                node: Some(17),
                clicks: Some(2),
                ..
            }
        ));
        let download: Command = serde_json::from_value(serde_json::json!({
            "verb": "page-download",
            "target": "ssh",
            "port": 9222,
            "target_id": "B2",
            "selector": "#download",
            "download_dir": "~/downloads",
            "wait_ms": 30000
        }))
        .expect("deserialize download");
        assert_eq!(download.verb(), "page-download");
        assert_eq!(download.required_grant(), Grant::Actuate);
        assert!(matches!(
            download,
            Command::PageDownload {
                target: TargetRef::Ssh,
                wait_ms: Some(30000),
                ..
            }
        ));
        let hover: Command = serde_json::from_value(serde_json::json!({
            "verb": "page-hover", "target": "current", "target_id": "B2", "x": 12.5, "y": 40.0
        }))
        .expect("hover wire");
        assert_eq!(hover.verb(), "page-hover");
        assert_eq!(hover.required_grant(), Grant::Actuate);
        let scroll = Command::PageScroll {
            target: TargetRef::Current,
            port: Some(9222),
            pid: None,
            target_id: None,
            target_url: Some("docs".into()),
            target_title: None,
            target_match: None,
            x: 10.0,
            y: 20.0,
            dx: None,
            dy: Some(-120.0),
        };
        assert_eq!(scroll.verb(), "page-scroll");
        assert_eq!(scroll.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&scroll).expect("scroll wire")["dy"],
            -120.0
        );
        let fill = Command::PageFill {
            target: TargetRef::Current,
            port: Some(9222),
            pid: None,
            target_id: None,
            target_url: None,
            target_title: Some("B".into()),
            target_match: None,
            selector: Some("#q".into()),
            node: None,
            text: "hello".into(),
            clear: true,
            submit: false,
        };
        assert_eq!(fill.verb(), "page-fill");
        assert_eq!(fill.required_grant(), Grant::Actuate);
        let json = serde_json::to_value(&fill).expect("serialize");
        assert_eq!(json["clear"], true);
        assert!(
            json.get("submit").is_none(),
            "false switches are not echoed"
        );
        let nav = Command::PageNav {
            target: TargetRef::Current,
            port: None,
            pid: None,
            target_id: None,
            target_url: None,
            target_title: Some("B".into()),
            target_match: None,
            url: "https://docs.example/".into(),
            wait_ms: Some(500),
        };
        assert_eq!(nav.verb(), "page-nav");
        assert_eq!(nav.required_grant(), Grant::Actuate);
        let shot = Command::PageScreenshot {
            target: TargetRef::Current,
            port: None,
            pid: None,
            target_id: None,
            target_url: None,
            target_title: Some("B".into()),
            target_match: None,
            out: "shot.png".into(),
            replace: false,
            activate: false,
        };
        assert_eq!(shot.verb(), "page-screenshot");
        assert_eq!(shot.required_grant(), Grant::Observe);
        let raised = match shot.clone() {
            Command::PageScreenshot {
                target,
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                out,
                replace,
                ..
            } => Command::PageScreenshot {
                target,
                port,
                pid,
                target_id,
                target_url,
                target_title,
                target_match,
                out,
                replace,
                activate: true,
            },
            other => other,
        };
        assert_eq!(
            raised.required_grant(),
            Grant::Actuate,
            "--activate changes the front tab, so it is actuation"
        );
        let back: Command = serde_json::from_value(serde_json::to_value(&raised).unwrap()).unwrap();
        assert!(matches!(
            back,
            Command::PageScreenshot { activate: true, .. }
        ));
    }

    #[test]
    fn clipboard_read_is_target_neutral_observation() {
        let command = Command::ClipboardRead {
            target: TargetRef::Vnc,
            metadata_only: false,
            type_name: None,
            max_bytes: None,
            out: None,
            replace: false,
        };
        assert_eq!(command.verb(), "clipboard-read");
        assert_eq!(command.target(), TargetRef::Vnc);
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({ "verb": "clipboard-read", "target": "vnc" })
        );
    }

    #[test]
    fn clipboard_metadata_is_an_observation_and_serializes_without_payload_fields() {
        let command = Command::ClipboardRead {
            target: TargetRef::Current,
            metadata_only: true,
            type_name: None,
            max_bytes: None,
            out: None,
            replace: false,
        };
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "clipboard-read",
                "target": "current",
                "metadata_only": true
            })
        );
    }

    #[test]
    fn pointer_move_is_target_neutral_actuation_with_explicit_coordinates() {
        let command = Command::PointerMove {
            target: TargetRef::Ssh,
            x: -320,
            y: 1440,
        };
        assert_eq!(command.verb(), "pointer-move");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Actuate);
        let json = serde_json::to_value(&command).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "verb": "pointer-move",
                "target": "ssh",
                "x": -320,
                "y": 1440
            })
        );
        let decoded: Command = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(
            decoded,
            Command::PointerMove {
                target: TargetRef::Ssh,
                x: -320,
                y: 1440
            }
        ));
    }

    #[test]
    fn tree_defaults_keep_the_pre_budget_wire_shape() {
        // A pre-1.12 caller's `{"verb":"tree","target":"current","window":7}`
        // still decodes, and a default tree still encodes to exactly that.
        let decoded: Command = serde_json::from_value(
            serde_json::json!({ "verb": "tree", "target": "current", "window": 7 }),
        )
        .expect("deserialize");
        assert!(matches!(
            decoded,
            Command::Tree {
                target: TargetRef::Current,
                window: Some(7),
                depth: None,
                max_nodes: None,
                flat: false,
            }
        ));
        assert_eq!(
            serde_json::to_value(&decoded).expect("serialize"),
            serde_json::json!({ "verb": "tree", "target": "current", "window": 7 })
        );
        let bounded = Command::Tree {
            target: TargetRef::Ssh,
            window: Some(7),
            depth: Some(3),
            max_nodes: Some(5),
            flat: true,
        };
        assert_eq!(bounded.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&bounded).expect("serialize"),
            serde_json::json!({
                "verb": "tree", "target": "ssh", "window": 7,
                "depth": 3, "max_nodes": 5, "flat": true
            })
        );
    }

    #[test]
    fn desktop_state_is_an_observe_command_with_closed_wire_shape() {
        let command = Command::DesktopState {
            target: TargetRef::Ssh,
            window: Some(7),
            depth: Some(2),
            max_nodes: Some(10),
        };
        assert_eq!(command.verb(), "desktop-state");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({
                "verb": "desktop-state", "target": "ssh", "window": 7,
                "depth": 2, "max_nodes": 10
            })
        );
    }

    #[test]
    fn query_is_observation_and_round_trips_its_filters() {
        let command = Command::Query {
            target: TargetRef::Vnc,
            window: 14278,
            depth: Some(12),
            max_nodes: Some(500),
            role: vec!["AXTextArea".into(), "button".into()],
            text: Some("Fixture".into()),
            text_exact: None,
            identifier: None,
            actionable: true,
            within: Some([0, 0, 900, 700]),
            offset: Some(2),
            max: Some(10),
            selector: None,
        };
        assert_eq!(command.verb(), "query");
        assert_eq!(command.target(), TargetRef::Vnc);
        assert_eq!(command.required_grant(), Grant::Observe);
        let json = serde_json::to_value(&command).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "verb": "query", "target": "vnc", "window": 14278,
                "depth": 12, "max_nodes": 500,
                "role": ["AXTextArea", "button"], "text": "Fixture",
                "actionable": true, "within": [0, 0, 900, 700],
                "offset": 2, "max": 10
            })
        );
        let decoded: Command = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.verb(), "query");
        // The minimal wire form decodes with every filter at its default.
        let minimal: Command = serde_json::from_value(
            serde_json::json!({ "verb": "query", "target": "current", "window": 1 }),
        )
        .expect("deserialize minimal");
        assert!(matches!(
            minimal,
            Command::Query { window: 1, actionable: false, ref role, .. } if role.is_empty()
        ));
    }

    #[test]
    fn windows_inventory_filters_default_to_the_bare_verb() {
        let bare: Command =
            serde_json::from_value(serde_json::json!({ "verb": "windows", "target": "current" }))
                .expect("deserialize");
        assert!(matches!(
            bare,
            Command::Windows {
                pid: None,
                app: None,
                title: None,
                focused: None,
                minimized: None,
                offset: None,
                max: None,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(&bare).expect("serialize"),
            serde_json::json!({ "verb": "windows", "target": "current" })
        );
        let watch: Command = serde_json::from_value(serde_json::json!({
            "verb": "windows-watch",
            "target": "current"
        }))
        .expect("deserialize");
        assert_eq!(watch.verb(), "windows-watch");
        assert_eq!(watch.required_grant(), Grant::Observe);
        let apps = Command::Apps {
            target: TargetRef::Current,
            running: true,
            all: false,
        };
        assert_eq!(apps.verb(), "apps");
        assert_eq!(apps.required_grant(), Grant::Observe);
        let filtered = Command::Windows {
            target: TargetRef::Current,
            pid: Some(4242),
            app: Some("TextEdit".into()),
            title: None,
            focused: Some(true),
            minimized: Some(false),
            browser_profile: None,
            offset: None,
            max: Some(1),
        };
        assert_eq!(filtered.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&filtered).expect("serialize"),
            serde_json::json!({
                "verb": "windows", "target": "current", "pid": 4242,
                "app": "TextEdit", "focused": true, "minimized": false, "max": 1
            })
        );
    }

    #[test]
    fn invoke_is_actuation_and_verify_is_observation() {
        let invoke = Command::Invoke {
            target: TargetRef::Current,
            window: 7,
            node: None,
            index: None,
            name: Some("Fixture Check".into()),
            identifier: None,
            role: Some("AXCheckBox".into()),
            action: InvokeAction::SetChecked,
            value: Some("true".into()),
            focused: false,
            selector: None,
        };
        assert_eq!(invoke.verb(), "invoke");
        assert_eq!(invoke.required_grant(), Grant::Actuate);
        let json = serde_json::to_value(&invoke).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "verb": "invoke", "target": "current", "window": 7,
                "name": "Fixture Check", "role": "AXCheckBox",
                "action": "set-checked", "value": "true"
            })
        );
        let decoded: Command = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(
            decoded,
            Command::Invoke {
                action: InvokeAction::SetChecked,
                window: 7,
                ..
            }
        ));
        assert_eq!(
            InvokeAction::parse("select-option"),
            Some(InvokeAction::SelectOption)
        );
        assert_eq!(InvokeAction::parse("raise"), None);
        assert_eq!(
            InvokeAction::parse("scroll-to"),
            Some(InvokeAction::ScrollTo)
        );
        assert_eq!(
            InvokeAction::parse("set-selected"),
            Some(InvokeAction::SetSelected)
        );
        assert_eq!(InvokeAction::Press.value_kind(), InvokeValueKind::None);
        assert_eq!(InvokeAction::SetValue.value_kind(), InvokeValueKind::Text);
        assert_eq!(
            InvokeAction::SetExpanded.value_kind(),
            InvokeValueKind::Flag
        );

        let verify = Command::Verify {
            target: TargetRef::Ssh,
            window: 7,
            expect: vec![Expectation {
                identifier: Some("fixture-check".into()),
                checked: Some(true),
                ..Expectation::default()
            }],
        };
        assert_eq!(verify.verb(), "verify");
        assert_eq!(verify.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&verify).expect("serialize"),
            serde_json::json!({
                "verb": "verify", "target": "ssh", "window": 7,
                "expect": [{ "identifier": "fixture-check", "checked": true }]
            })
        );
    }

    #[test]
    fn background_verbs_have_grants_and_closed_wire_shapes() {
        let inspect = Command::MenuInspect {
            target: TargetRef::Current,
            window: 7,
            depth: Some(2),
            max_nodes: None,
            title: Some("Do".into()),
            exact: false,
            enabled: Some(true),
            offset: None,
            max: Some(20),
        };
        assert_eq!(inspect.verb(), "menu-inspect");
        assert_eq!(inspect.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&inspect).expect("serialize"),
            serde_json::json!({
                "verb": "menu-inspect", "target": "current", "window": 7,
                "depth": 2, "title": "Do", "enabled": true, "max": 20
            })
        );
        let invoke = Command::MenuInvoke {
            target: TargetRef::Ssh,
            window: 7,
            path: vec!["File".into(), "Do Thing".into()],
        };
        assert_eq!(invoke.verb(), "menu-invoke");
        assert_eq!(invoke.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&invoke).expect("serialize"),
            serde_json::json!({
                "verb": "menu-invoke", "target": "ssh", "window": 7,
                "path": ["File", "Do Thing"]
            })
        );
        let focused = Command::Focused {
            target: TargetRef::Current,
            window: 7,
            role: Some("AXTextField".into()),
            max_value_bytes: Some(0),
        };
        assert_eq!(focused.verb(), "focused");
        assert_eq!(focused.required_grant(), Grant::Observe);
        let observe: Command = serde_json::from_value(serde_json::json!({
            "verb": "observe", "target": "current", "window": 7, "duration_ms": 1500,
            "notifications": ["ValueChanged"], "max_events": 50,
            "ready_path": "observe-ready.json"
        }))
        .expect("deserialize");
        assert!(matches!(
            observe,
            Command::Observe { window: 7, duration_ms: 1500, max_events: Some(50), ref notifications, ref ready_path, .. }
                if notifications == &["ValueChanged".to_owned()]
                    && ready_path.as_deref() == Some("observe-ready.json")
        ));
        assert_eq!(observe.required_grant(), Grant::Observe);
        // A pre-1.14 invoke wire form still decodes with `focused` false.
        let older: Command = serde_json::from_value(serde_json::json!({
            "verb": "invoke", "target": "current", "window": 7,
            "identifier": "fixture-press", "action": "press"
        }))
        .expect("deserialize");
        assert!(matches!(older, Command::Invoke { focused: false, .. }));
    }

    #[test]
    fn expectation_shape_is_closed() {
        let parsed: Expectation =
            serde_json::from_str(r#"{"name":"Fixture","role":"AXButton","value":"x"}"#)
                .expect("known keys parse");
        assert!(parsed.has_target() && parsed.has_state());
        let unknown = serde_json::from_str::<Expectation>(r#"{"name":"a","cheked":true}"#);
        assert!(unknown.is_err(), "a misspelled state must not parse");
        let title: Expectation =
            serde_json::from_str(r#"{"role":"AXHeading","titleIncludes":"Nepal"}"#)
                .expect("MCU titleIncludes aliases name");
        assert_eq!(title.name.as_deref(), Some("Nepal"));
        assert!(title.has_page_identity());
        assert!(!title.has_state());
        let page_js = Command::PageJs {
            target: TargetRef::Current,
            window: Some(14278),
            expression: Some("document.title".into()),
            port: None,
            pid: None,
            target_id: None,
            target_url: None,
            target_title: Some("Nepal".into()),
            target_match: None,
        };
        assert_eq!(page_js.verb(), "page-js");
        assert_eq!(page_js.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&page_js).expect("serialize"),
            serde_json::json!({
                "verb": "page-js", "target": "current", "window": 14278,
                "expression": "document.title", "target_title": "Nepal"
            })
        );
        // A pre-selector wire form still decodes with no selector.
        let older: Command = serde_json::from_value(serde_json::json!({
            "verb": "page-js", "target": "current", "expression": "1+1"
        }))
        .expect("deserialize");
        assert!(matches!(
            older,
            Command::PageJs {
                target_id: None,
                target_url: None,
                target_title: None,
                ..
            }
        ));
        let targets = Command::PageTargets {
            target: TargetRef::Current,
            port: Some(9223),
            pid: None,
            browser_profile: None,
        };
        assert_eq!(targets.verb(), "page-targets");
        assert_eq!(targets.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&targets).expect("serialize"),
            serde_json::json!({ "verb": "page-targets", "target": "current", "port": 9223 })
        );
        let joined: Command = serde_json::from_value(serde_json::json!({
            "verb": "page-targets", "target": "current", "browser_profile": "work"
        }))
        .expect("deserialize");
        assert!(matches!(
            joined,
            Command::PageTargets { browser_profile: Some(ref profile), port: None, .. } if profile == "work"
        ));
        let text = Command::PageText {
            target: TargetRef::Current,
            window: Some(7),
            max_bytes: Some(4096),
            within: Some([0, 60, 800, 500]),
            depth: None,
            max_nodes: None,
            port: None,
            pid: None,
            target_id: None,
            target_url: None,
            target_title: None,
            target_match: None,
        };
        assert_eq!(text.verb(), "page-text");
        assert_eq!(text.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&text).expect("serialize"),
            serde_json::json!({
                "verb": "page-text", "target": "current", "window": 7,
                "max_bytes": 4096, "within": [0, 60, 800, 500]
            })
        );
        // The CDP spelling of the same verb: no window, a target selector.
        let cdp_text: Command = serde_json::from_value(serde_json::json!({
            "verb": "page-text", "target": "current", "target_title": "Inbox", "port": 9223
        }))
        .expect("deserialize");
        assert!(matches!(
            cdp_text,
            Command::PageText { window: None, port: Some(9223), target_title: Some(ref title), .. } if title == "Inbox"
        ));
        assert_eq!(cdp_text.required_grant(), Grant::Observe);
        let list = Command::TabList {
            target: TargetRef::Current,
            window: 7,
        };
        assert_eq!(list.verb(), "tab-list");
        assert_eq!(list.required_grant(), Grant::Observe);
        let select = Command::TabSelect {
            target: TargetRef::Current,
            window: 7,
            title: Some("Codex".into()),
            index: None,
        };
        assert_eq!(select.verb(), "tab-select");
        assert_eq!(select.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&select).expect("serialize"),
            serde_json::json!({
                "verb": "tab-select", "target": "current", "window": 7, "title": "Codex"
            })
        );
        let wait = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 500,
            condition: WaitCondition::Expect {
                window: 3,
                expect: vec![Expectation {
                    node: Some("/0/1".into()),
                    value: Some("pressed 1".into()),
                    ..Expectation::default()
                }],
            },
        };
        assert_eq!(wait.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&wait).expect("serialize"),
            serde_json::json!({
                "verb": "wait", "target": "current", "timeout_ms": 500,
                "wait": "expect", "window": 3,
                "expect": [{ "node": "/0/1", "value": "pressed 1" }]
            })
        );
    }

    #[test]
    fn browser_profile_verbs_and_tab_close_carry_their_grants_and_shapes() {
        let profiles = Command::BrowserProfiles {
            target: TargetRef::Current,
            app: None,
        };
        assert_eq!(profiles.verb(), "browser-profiles");
        assert_eq!(profiles.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&profiles).expect("serialize"),
            serde_json::json!({ "verb": "browser-profiles", "target": "current" })
        );
        let open = Command::BrowserOpen {
            target: TargetRef::Current,
            profile: "work".into(),
            url: Some("https://example.com/".into()),
            app: Some("Brave Origin".into()),
            timeout_ms: None,
        };
        assert_eq!(open.verb(), "browser-open");
        assert_eq!(open.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&open).expect("serialize"),
            serde_json::json!({
                "verb": "browser-open", "target": "current", "profile": "work",
                "url": "https://example.com/", "app": "Brave Origin"
            })
        );
        let close = Command::TabClose {
            target: TargetRef::Current,
            window: 7,
            title: Some("cu-live".into()),
            index: None,
            exact: true,
            expect: Some("gone".into()),
            port: None,
        };
        assert_eq!(close.verb(), "tab-close");
        assert_eq!(close.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&close).expect("serialize"),
            serde_json::json!({
                "verb": "tab-close", "target": "current", "window": 7,
                "title": "cu-live", "exact": true, "expect": "gone"
            })
        );
        let by_index = Command::TabClose {
            target: TargetRef::Current,
            window: 7,
            title: None,
            index: Some(3),
            exact: false,
            expect: Some("gone".into()),
            port: Some(9222),
        };
        assert_eq!(
            serde_json::to_value(&by_index).expect("serialize"),
            serde_json::json!({
                "verb": "tab-close", "target": "current", "window": 7,
                "index": 3, "expect": "gone", "port": 9222
            })
        );
        // The bare wire form decodes; the executor's gate refuses it.
        let bare: Command = serde_json::from_value(
            serde_json::json!({ "verb": "tab-close", "target": "current", "window": 7 }),
        )
        .expect("deserialize");
        assert!(matches!(
            bare,
            Command::TabClose {
                window: 7,
                title: None,
                index: None,
                exact: false,
                expect: None,
                port: None,
                ..
            }
        ));
        let filtered = Command::Windows {
            target: TargetRef::Current,
            pid: None,
            app: Some("Brave".into()),
            title: None,
            focused: None,
            minimized: None,
            browser_profile: Some("work".into()),
            offset: None,
            max: None,
        };
        assert_eq!(
            serde_json::to_value(&filtered).expect("serialize"),
            serde_json::json!({
                "verb": "windows", "target": "current", "app": "Brave", "browser_profile": "work"
            })
        );
    }

    #[test]
    fn browser_session_commands_have_closed_wire_shapes_and_grants() {
        let start = Command::BrowserSessionStart {
            target: TargetRef::Current,
            name: "research".into(),
            browser: "/opt/browser".into(),
            bridge: true,
            ready_timeout_ms: 15_000,
            ttl_ms: 3_600_000,
        };
        assert_eq!(start.verb(), "browser-session-start");
        assert_eq!(start.target(), TargetRef::Current);
        assert_eq!(start.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&start).expect("serialize"),
            serde_json::json!({
                "verb": "browser-session-start", "target": "current",
                "name": "research", "browser": "/opt/browser",
                "bridge": true,
                "ready_timeout_ms": 15000, "ttl_ms": 3600000
            })
        );

        let list = Command::BrowserSessionList {
            target: TargetRef::Ssh,
        };
        assert_eq!(list.verb(), "browser-session-list");
        assert_eq!(list.required_grant(), Grant::Observe);

        let status = Command::BrowserSessionStatus {
            target: TargetRef::Vnc,
            name: "research".into(),
        };
        assert_eq!(status.verb(), "browser-session-status");
        assert_eq!(status.required_grant(), Grant::Observe);

        let stop = Command::BrowserSessionStop {
            target: TargetRef::Current,
            name: "research".into(),
            expect_stopped: true,
            timeout_ms: 15_000,
        };
        assert_eq!(stop.verb(), "browser-session-stop");
        assert_eq!(stop.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&stop).expect("serialize"),
            serde_json::json!({
                "verb": "browser-session-stop", "target": "current",
                "name": "research", "expect_stopped": true, "timeout_ms": 15000
            })
        );

        let remove = Command::BrowserSessionRemove {
            target: TargetRef::Current,
            name: "research".into(),
            expect_stopped: true,
            expect_failed: false,
        };
        assert_eq!(remove.verb(), "browser-session-remove");
        assert_eq!(remove.required_grant(), Grant::Actuate);
        let back: Command =
            serde_json::from_value(serde_json::to_value(&remove).expect("serialize"))
                .expect("deserialize");
        assert!(matches!(
            back,
            Command::BrowserSessionRemove {
                target: TargetRef::Current,
                ref name,
                expect_stopped: true,
                expect_failed: false,
            } if name == "research"
        ));
    }

    #[test]
    fn close_is_actuation_with_a_closed_gate_shape_and_receipts_are_observation() {
        let close = Command::Close {
            target: TargetRef::Current,
            window: 7,
            pid: Some(4242),
            title: None,
            snapshot: true,
            expect: Some("gone".into()),
        };
        assert_eq!(close.verb(), "close");
        assert_eq!(close.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&close).expect("serialize"),
            serde_json::json!({
                "verb": "close", "target": "current", "window": 7,
                "pid": 4242, "snapshot": true, "expect": "gone"
            })
        );
        // The bare wire form (no snapshot, no postcondition) still decodes;
        // the executor refuses it, the shape does not hide it.
        let bare: Command = serde_json::from_value(
            serde_json::json!({ "verb": "close", "target": "current", "window": 7 }),
        )
        .expect("deserialize");
        assert!(matches!(
            bare,
            Command::Close {
                window: 7,
                snapshot: false,
                expect: None,
                pid: None,
                title: None,
                ..
            }
        ));
        let receipts = Command::Receipts {
            target: TargetRef::Ssh,
            window: Some(7),
            max: Some(5),
        };
        assert_eq!(receipts.verb(), "receipts");
        assert_eq!(receipts.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&receipts).expect("serialize"),
            serde_json::json!({ "verb": "receipts", "target": "ssh", "window": 7, "max": 5 })
        );
        // A pre-slice-4 window-place wire form decodes with no frame.
        let place: Command = serde_json::from_value(serde_json::json!({
            "verb": "window-place", "target": "current", "action": "left-half", "window": 7
        }))
        .expect("deserialize");
        assert!(matches!(place, Command::WindowPlace { frame: None, .. }));
        let framed = Command::WindowPlace {
            target: TargetRef::Current,
            action: "frame".into(),
            window: Some(7),
            frame: Some([10, 20, 300, 200]),
        };
        assert_eq!(
            serde_json::to_value(&framed).expect("serialize"),
            serde_json::json!({
                "verb": "window-place", "target": "current", "action": "frame",
                "window": 7, "frame": [10, 20, 300, 200]
            })
        );
        let order: Command = serde_json::from_value(serde_json::json!({
            "verb": "orderwin",
            "target": "current",
            "window": 1,
            "relation": "above",
            "relative": 2
        }))
        .expect("deserialize");
        assert_eq!(order.verb(), "orderwin");
        assert_eq!(order.required_grant(), Grant::Actuate);
        assert!(matches!(
            order,
            Command::OrderWin {
                window: 1,
                relation: OrderRelation::Above,
                relative: 2,
                ..
            }
        ));
    }

    #[test]
    fn pointer_position_is_target_neutral_observation() {
        let command = Command::PointerPosition {
            target: TargetRef::Vnc,
        };
        assert_eq!(command.verb(), "pointer-position");
        assert_eq!(command.target(), TargetRef::Vnc);
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({ "verb": "pointer-position", "target": "vnc" })
        );
    }

    #[test]
    fn terminal_facade_keeps_stable_ids_limits_and_grants_on_the_wire() {
        let jobs = Command::PtyList {
            target: TargetRef::Ssh,
        };
        assert_eq!(jobs.required_grant(), Grant::Observe);
        assert_eq!(jobs.verb(), "pty-list");
        assert_eq!(jobs.target(), TargetRef::Ssh);
        assert_eq!(
            serde_json::to_value(&jobs).unwrap(),
            serde_json::json!({ "verb": "pty-list", "target": "ssh" })
        );
        let prune = Command::PtyPrune {
            target: TargetRef::Current,
            name: "build".into(),
            expect_stale: true,
        };
        assert_eq!(prune.required_grant(), Grant::Actuate);
        assert_eq!(prune.verb(), "pty-prune");
        assert_eq!(
            serde_json::to_value(&prune).unwrap(),
            serde_json::json!({
                "verb": "pty-prune", "target": "current", "name": "build",
                "expect_stale": true
            })
        );
        let new = Command::TerminalNew {
            target: TargetRef::Current,
            title: Some("build".into()),
            parent: Some("@3".into()),
            detached: true,
            command: vec!["sh".into(), "-lc".into(), "printf ok".into()],
        };
        assert_eq!(new.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&new).unwrap(),
            serde_json::json!({
                "verb": "terminal-new", "target": "current", "title": "build",
                "parent": "@3", "detached": true,
                "command": ["sh", "-lc", "printf ok"]
            })
        );
        let close = Command::TerminalClose {
            target: TargetRef::Vnc,
            tab: "@9".into(),
            expect_closed: true,
        };
        assert_eq!(close.required_grant(), Grant::Actuate);
        assert_eq!(close.verb(), "terminal-close");
        let read = Command::TerminalRead {
            target: TargetRef::Ssh,
            tab: "@9".into(),
            max_bytes: 4096,
        };
        assert_eq!(read.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&read).unwrap(),
            serde_json::json!({
                "verb": "terminal-read", "target": "ssh", "tab": "@9", "max_bytes": 4096
            })
        );
        let job_snapshot = Command::PtySnapshot {
            target: TargetRef::Current,
            name: "build".into(),
        };
        assert_eq!(job_snapshot.required_grant(), Grant::Observe);
        assert_eq!(job_snapshot.verb(), "pty-snapshot");
        let signal = Command::PtySignal {
            target: TargetRef::Current,
            name: "build".into(),
            signal: PtySignalKind::Stop,
            expect: "stopped".into(),
        };
        assert_eq!(signal.required_grant(), Grant::Actuate);
        assert_eq!(signal.verb(), "pty-signal");
        assert_eq!(
            signal.authorization_operation().as_deref(),
            Some("pty-signal.stop")
        );
        assert_eq!(
            serde_json::to_value(&signal).unwrap(),
            serde_json::json!({
                "verb": "pty-signal", "target": "current", "name": "build",
                "signal": "stop", "expect": "stopped"
            })
        );
        assert_eq!(
            serde_json::to_value(&job_snapshot).unwrap(),
            serde_json::json!({
                "verb": "pty-snapshot", "target": "current", "name": "build"
            })
        );
        let job_events = Command::PtyEvents {
            target: TargetRef::Ssh,
            name: "build".into(),
            epoch: "epoch-a".into(),
            after: 12,
            limit: 32,
        };
        assert_eq!(job_events.required_grant(), Grant::Observe);
        assert_eq!(job_events.verb(), "pty-events");
        assert_eq!(job_events.target(), TargetRef::Ssh);
        let job_resize = Command::PtyResize {
            target: TargetRef::Current,
            name: "build".into(),
            rows: 40,
            columns: 120,
        };
        assert_eq!(job_resize.required_grant(), Grant::Actuate);
        assert_eq!(job_resize.verb(), "pty-resize");
        let job_diff = Command::PtyDiff {
            target: TargetRef::Current,
            name: "build".into(),
            base: "1-2-3".into(),
            advance: true,
            max: Some(8),
        };
        assert_eq!(job_diff.required_grant(), Grant::Observe);
        assert_eq!(job_diff.verb(), "pty-diff");
        let send = Command::TerminalSend {
            target: TargetRef::Current,
            tab: "@9".into(),
            text: "hello\r".into(),
        };
        assert_eq!(send.required_grant(), Grant::Actuate);
        let wait = Command::TerminalWait {
            target: TargetRef::Vnc,
            tab: "@9".into(),
            condition: TerminalWaitCondition::Finalized,
            timeout_ms: 5000,
        };
        assert_eq!(wait.required_grant(), Grant::Observe);
        assert_eq!(wait.verb(), "terminal-wait");
        let snapshot = Command::TerminalSnapshot {
            target: TargetRef::Current,
            tab: "@9".into(),
        };
        assert_eq!(snapshot.required_grant(), Grant::Observe);
        assert_eq!(snapshot.verb(), "terminal-snapshot");
        let scroll = Command::TerminalScroll {
            target: TargetRef::Current,
            tab: "@9".into(),
            action: TerminalScrollAction::Top,
            rows: None,
        };
        assert_eq!(scroll.verb(), "terminal-scroll");
        assert_eq!(
            scroll.authorization_operation().as_deref(),
            Some("terminal-scroll")
        );
        assert_eq!(scroll.required_grant(), Grant::Actuate);
        let screenshot = Command::TerminalScreenshot {
            target: TargetRef::Current,
            tab: "@9".into(),
            out: "/tmp/pane.png".into(),
        };
        assert_eq!(screenshot.verb(), "terminal-screenshot");
        assert_eq!(screenshot.required_grant(), Grant::Observe);
        let events = Command::TerminalEvents {
            target: TargetRef::Ssh,
            tab: "@9".into(),
            epoch: "epoch-a".into(),
            after: 12,
            limit: 32,
        };
        assert_eq!(events.required_grant(), Grant::Observe);
        assert_eq!(events.verb(), "terminal-events");
        assert_eq!(events.target(), TargetRef::Ssh);
        let output = Command::TerminalOutput {
            target: TargetRef::Current,
            tab: "@9".into(),
            cursor: "earliest".into(),
            max_bytes: 65_536,
        };
        assert_eq!(output.required_grant(), Grant::Observe);
        assert_eq!(output.verb(), "terminal-output");
    }

    #[test]
    fn network_probe_is_observe_only_and_keeps_its_closed_remote_shape() {
        let command = Command::NetworkProbe {
            target: TargetRef::Ssh,
            host: "fixture.invalid".into(),
            port: 8443,
            attempts: 4,
            timeout_ms: 750,
        };
        assert_eq!(command.verb(), "network-probe");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Observe);
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["host"], "fixture.invalid");
        assert_eq!(json["attempts"], 4);
        let round_trip: Command = serde_json::from_value(json).unwrap();
        assert!(matches!(
            round_trip,
            Command::NetworkProbe {
                target: TargetRef::Ssh,
                port: 8443,
                attempts: 4,
                timeout_ms: 750,
                ..
            }
        ));
    }

    #[test]
    fn network_interfaces_is_observe_only_and_keeps_its_closed_remote_shape() {
        let command = Command::NetworkInterfaces {
            target: TargetRef::Vnc,
            max: 5000,
        };
        assert_eq!(command.verb(), "network-interfaces");
        assert_eq!(command.target(), TargetRef::Vnc);
        assert_eq!(command.required_grant(), Grant::Observe);
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["max"], 5000);
        let round_trip: Command = serde_json::from_value(json).unwrap();
        assert!(matches!(
            round_trip,
            Command::NetworkInterfaces {
                target: TargetRef::Vnc,
                max: 5000,
            }
        ));
    }

    #[test]
    fn network_routes_is_observe_only_and_keeps_its_closed_remote_shape() {
        let command = Command::NetworkRoutes {
            target: TargetRef::Ssh,
            max: 5000,
        };
        assert_eq!(command.verb(), "network-routes");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Observe);
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["max"], 5000);
        let round_trip: Command = serde_json::from_value(json).unwrap();
        assert!(matches!(
            round_trip,
            Command::NetworkRoutes {
                target: TargetRef::Ssh,
                max: 5000
            }
        ));
    }

    #[test]
    fn network_dns_is_observe_only_and_keeps_its_closed_remote_shape() {
        let command = Command::NetworkDns {
            target: TargetRef::Ssh,
            max: 17,
        };
        assert_eq!(command.verb(), "network-dns");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Observe);
        let encoded = serde_json::to_string(&command).unwrap();
        let decoded: Command = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(
            decoded,
            Command::NetworkDns {
                target: TargetRef::Ssh,
                max: 17
            }
        ));
    }

    #[test]
    fn file_inspect_is_observe_only_and_keeps_its_remote_path_literal() {
        let command = Command::FileInspect {
            target: TargetRef::Ssh,
            path: "a path/with spaces".into(),
        };
        assert_eq!(command.verb(), "file-inspect");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Observe);
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["path"], "a path/with spaces");
        let round_trip: Command = serde_json::from_value(json).unwrap();
        assert!(matches!(
            round_trip,
            Command::FileInspect {
                target: TargetRef::Ssh,
                path,
            } if path == "a path/with spaces"
        ));
    }

    #[test]
    fn file_copy_plan_and_transaction_actions_have_exact_grants() {
        let plan = Command::FileCopy {
            target: TargetRef::Ssh,
            source: "source".into(),
            destination: "destination".into(),
            replace: true,
            apply: false,
        };
        assert_eq!(plan.required_grant(), Grant::Observe);
        let mut apply = plan.clone();
        if let Command::FileCopy { apply, .. } = &mut apply {
            *apply = true;
        }
        assert_eq!(apply.required_grant(), Grant::Actuate);
        let json = serde_json::to_value(&apply).unwrap();
        let round_trip: Command = serde_json::from_value(json).unwrap();
        assert!(matches!(
            round_trip,
            Command::FileCopy {
                target: TargetRef::Ssh,
                replace: true,
                apply: true,
                ..
            }
        ));
        let move_plan = Command::FileMove {
            target: TargetRef::Current,
            source: "source".into(),
            destination: "destination".into(),
            replace: false,
            apply: false,
        };
        assert_eq!(move_plan.required_grant(), Grant::Observe);
        assert_eq!(move_plan.verb(), "file-move");
        let mut move_apply = move_plan.clone();
        if let Command::FileMove { apply, .. } = &mut move_apply {
            *apply = true;
        }
        assert_eq!(move_apply.required_grant(), Grant::Actuate);
        let json = serde_json::to_value(&move_apply).unwrap();
        assert_eq!(
            json.get("replace"),
            None,
            "false switches stay off the wire"
        );
        let round_trip: Command = serde_json::from_value(json).unwrap();
        assert!(matches!(
            round_trip,
            Command::FileMove {
                replace: false,
                apply: true,
                ..
            }
        ));
        for (action, grant) in [
            (FileTransactionAction::Status, Grant::Observe),
            (FileTransactionAction::Rollback, Grant::Actuate),
            (FileTransactionAction::Recover, Grant::Actuate),
            (FileTransactionAction::Finalize, Grant::Actuate),
        ] {
            let command = Command::FileTransaction {
                target: TargetRef::Current,
                action,
                transaction_id: "0".repeat(32),
            };
            assert_eq!(command.required_grant(), grant);
        }
    }

    #[test]
    fn browser_bridge_commands_keep_exact_identity_and_grant_on_the_wire() {
        let connection_id = ConnectionId::parse(&"1".repeat(64)).unwrap();
        let setup = Command::BrowserBridgeSetup {
            target: TargetRef::Current,
        };
        assert_eq!(setup.required_grant(), Grant::Actuate);
        assert_eq!(setup.verb(), "browser-bridge-setup");

        for observe in [
            Command::BrowserBridgeConnections {
                target: TargetRef::Current,
            },
            Command::BrowserBridgeStatus {
                target: TargetRef::Current,
                connection_id: connection_id.clone(),
            },
            Command::BrowserBridgeTabs {
                target: TargetRef::Current,
                connection_id: connection_id.clone(),
            },
            Command::BrowserBridgeWindows {
                target: TargetRef::Current,
                connection_id: connection_id.clone(),
            },
        ] {
            assert_eq!(observe.required_grant(), Grant::Observe);
        }

        let state = Command::BrowserBridgeWindowState {
            target: TargetRef::Current,
            connection_id: connection_id.clone(),
            window_id: 9,
            state: crate::browser_bridge::BrowserWindowState::Minimized,
        };
        assert_eq!(state.required_grant(), Grant::Actuate);
        assert_eq!(state.verb(), "browser-bridge-window-state");

        let open = Command::BrowserBridgeWindowOpen {
            target: TargetRef::Current,
            connection_id: connection_id.clone(),
            url: "data:text/html,ACU".into(),
            focused: false,
        };
        assert_eq!(open.required_grant(), Grant::Actuate);
        assert_eq!(open.verb(), "browser-bridge-window-open");

        let debug = Command::BrowserBridgeDebugRead {
            target: TargetRef::Ssh,
            connection_id,
            tab_id: 7,
            max_frames: 4,
            max_depth: 9,
            max_scan: 300,
            max_results: 80,
        };
        assert_eq!(debug.required_grant(), Grant::Observe);
        assert_eq!(debug.target(), TargetRef::Ssh);
        assert_eq!(debug.verb(), "browser-bridge-debug-read");
        let encoded = serde_json::to_value(&debug).unwrap();
        assert_eq!(encoded["connection_id"], "1".repeat(64));
        let decoded: Command = serde_json::from_value(encoded).unwrap();
        assert!(matches!(
            decoded,
            Command::BrowserBridgeDebugRead {
                target: TargetRef::Ssh,
                tab_id: 7,
                max_frames: 4,
                max_depth: 9,
                max_scan: 300,
                max_results: 80,
                ..
            }
        ));

        let invalid = serde_json::json!({
            "verb": "browser-bridge-status",
            "target": "current",
            "connection_id": "ABC"
        });
        assert!(serde_json::from_value::<Command>(invalid).is_err());
    }

    #[test]
    fn device_watch_keeps_bounds_selector_and_observe_grant_on_the_wire() {
        let command = Command::DeviceWatch {
            target: TargetRef::Current,
            selector: DeviceInventorySelector::Camera,
            max: 50,
            duration_ms: 5_000,
            interval_ms: 500,
            event_max: 20,
        };
        assert_eq!(command.verb(), "device-watch");
        assert_eq!(command.required_grant(), Grant::Observe);
        let encoded = serde_json::to_value(&command).expect("serialize");
        assert_eq!(encoded["selector"], "camera");
        assert_eq!(encoded["duration_ms"], 5_000);
        assert_eq!(encoded["interval_ms"], 500);
        assert_eq!(encoded["event_max"], 20);
        let decoded: Command = serde_json::from_value(encoded).expect("deserialize");
        assert!(matches!(
            decoded,
            Command::DeviceWatch {
                target: TargetRef::Current,
                selector: DeviceInventorySelector::Camera,
                max: 50,
                duration_ms: 5_000,
                interval_ms: 500,
                event_max: 20,
            }
        ));
        for invalid in [
            serde_json::json!({
                "verb": "device-watch", "target": "current", "selector": "all",
                "max": 50, "duration_ms": 999, "interval_ms": 500, "event_max": 20
            }),
            serde_json::json!({
                "verb": "device-watch", "target": "current", "selector": "all",
                "max": 50, "duration_ms": 5000, "interval_ms": 249, "event_max": 20
            }),
            serde_json::json!({
                "verb": "device-watch", "target": "current", "selector": "all",
                "max": 50, "duration_ms": 5000, "interval_ms": 500, "event_max": 5001
            }),
        ] {
            assert!(serde_json::from_value::<Command>(invalid).is_err());
        }
    }

    #[test]
    fn device_lease_commands_keep_secret_io_and_grant_bounds_on_the_wire() {
        let device_id = format!("agt-device-v1-{}", "a".repeat(64));
        let lease_id = "00000000-0000-4000-8000-000000000001";
        let lease = "b".repeat(64);
        let claim = Command::DeviceClaim {
            target: TargetRef::Current,
            device_id: device_id.clone(),
            ttl_seconds: 60,
            serial: Some(DeviceSerialConfiguration {
                baud: 115_200,
                data_bits: 8,
                parity: DeviceSerialParity::None,
                stop_bits: 1,
                flow: DeviceSerialFlow::None,
            }),
        };
        assert_eq!(claim.required_grant(), Grant::Actuate);
        claim.validate().unwrap();
        let encoded = serde_json::to_value(&claim).unwrap();
        assert_eq!(encoded["device_id"], device_id);
        let _: Command = serde_json::from_value(encoded).unwrap();

        for command in [
            Command::DeviceRead {
                target: TargetRef::Current,
                lease_id: lease_id.into(),
                generation: 1,
                lease: lease.clone(),
                max_bytes: 64,
                timeout_ms: 1_000,
                encoding: DeviceDataEncoding::Hex,
            },
            Command::DeviceWrite {
                target: TargetRef::Current,
                lease_id: lease_id.into(),
                generation: 1,
                lease: lease.clone(),
                data: "AP8K".into(),
                encoding: DeviceDataEncoding::Base64,
                timeout_ms: 1_000,
            },
            Command::DeviceRenew {
                target: TargetRef::Current,
                lease_id: lease_id.into(),
                generation: 1,
                lease: lease.clone(),
                ttl_seconds: 60,
            },
            Command::DeviceRelease {
                target: TargetRef::Current,
                lease_id: lease_id.into(),
                generation: 1,
                lease: lease.clone(),
            },
        ] {
            command.validate().unwrap();
            assert_eq!(command.required_grant(), Grant::Actuate);
            let _: Command =
                serde_json::from_value(serde_json::to_value(command).unwrap()).unwrap();
        }

        let status = Command::DeviceStatus {
            target: TargetRef::Current,
            lease_id: lease_id.into(),
            generation: 1,
        };
        assert_eq!(status.required_grant(), Grant::Observe);
        status.validate().unwrap();
        let claims = Command::DeviceClaims {
            target: TargetRef::Current,
            offset: Some(0),
            max: Some(100),
        };
        assert_eq!(claims.required_grant(), Grant::Observe);
        claims.validate().unwrap();

        for invalid in [
            Command::DeviceClaim {
                target: TargetRef::Current,
                device_id: "path-is-not-authority".into(),
                ttl_seconds: 60,
                serial: None,
            },
            Command::DeviceRead {
                target: TargetRef::Current,
                lease_id: lease_id.into(),
                generation: 0,
                lease: lease.clone(),
                max_bytes: 1,
                timeout_ms: 1,
                encoding: DeviceDataEncoding::Hex,
            },
            Command::DeviceWrite {
                target: TargetRef::Current,
                lease_id: lease_id.into(),
                generation: 1,
                lease,
                data: "not hex".into(),
                encoding: DeviceDataEncoding::Hex,
                timeout_ms: 1,
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn simulator_commands_keep_exact_targets_bounds_and_expectations_on_the_wire() {
        let udid = "12345678-1234-1234-1234-123456789ABC";
        let devices = Command::SimulatorDevices {
            target: TargetRef::Current,
            max: 25,
        };
        assert_eq!(devices.required_grant(), Grant::Observe);
        assert_eq!(devices.verb(), "simulator-devices");

        let apps = Command::SimulatorApps {
            target: TargetRef::Ssh,
            udid: udid.into(),
            max: 40,
        };
        assert_eq!(apps.required_grant(), Grant::Observe);
        assert_eq!(apps.target(), TargetRef::Ssh);

        for command in [
            Command::SimulatorBoot {
                target: TargetRef::Current,
                udid: udid.into(),
                timeout_ms: 30_000,
                expect_booted: true,
            },
            Command::SimulatorLaunch {
                target: TargetRef::Current,
                udid: udid.into(),
                bundle_id: "com.example.app".into(),
                timeout_ms: 30_000,
                expect_accepted: true,
            },
            Command::SimulatorTerminate {
                target: TargetRef::Current,
                udid: udid.into(),
                bundle_id: "com.example.app".into(),
                timeout_ms: 30_000,
                expect_accepted: true,
            },
        ] {
            assert_eq!(command.required_grant(), Grant::Actuate);
            command.validate().unwrap();
            let encoded = serde_json::to_value(&command).unwrap();
            assert_eq!(encoded["udid"], udid);
            let _: Command = serde_json::from_value(encoded).unwrap();
        }

        for invalid in [
            serde_json::json!({"verb":"simulator-devices","target":"current","max":0}),
            serde_json::json!({"verb":"simulator-boot","target":"current","udid":"fuzzy","timeout_ms":10,"expect_booted":true}),
            serde_json::json!({"verb":"simulator-boot","target":"current","udid":udid,"timeout_ms":10,"expect_booted":false}),
            serde_json::json!({"verb":"simulator-launch","target":"current","udid":udid,"bundle_id":"not dotted","timeout_ms":10,"expect_accepted":true}),
            serde_json::json!({"verb":"simulator-terminate","target":"current","udid":udid,"bundle_id":"com.example.app","timeout_ms":600001,"expect_accepted":true}),
        ] {
            assert!(serde_json::from_value::<Command>(invalid).is_err());
        }
    }

    #[test]
    fn persisted_authority_uses_shape_specific_canonical_operations() {
        assert_eq!(
            Command::Capabilities {
                target: TargetRef::Current,
            }
            .authorization_operation()
            .as_deref(),
            Some("capabilities")
        );
        assert_eq!(
            Command::Setup {
                target: TargetRef::Current,
                action: SetupAction::Check,
                bin_dir: None,
            }
            .authorization_operation()
            .as_deref(),
            Some("setup.check")
        );
        assert_eq!(
            Command::Setup {
                target: TargetRef::Current,
                action: SetupAction::Apply,
                bin_dir: None,
            }
            .authorization_operation()
            .as_deref(),
            Some("setup.apply")
        );
        assert_eq!(
            Command::Diff {
                target: TargetRef::Current,
                window: 1,
                base: None,
                advance: true,
                max: None,
            }
            .authorization_operation()
            .as_deref(),
            Some("diff.advance")
        );
        assert_eq!(
            Command::DeviceScreenshot {
                target: TargetRef::Current,
                path: None,
                device: None,
                timeout_ms: None,
                list: false,
            }
            .authorization_operation(),
            None
        );
        assert_eq!(
            Command::Align {
                target: TargetRef::Current,
                group: "caller-controlled".to_owned(),
            }
            .authorization_operation(),
            None
        );
    }
}
