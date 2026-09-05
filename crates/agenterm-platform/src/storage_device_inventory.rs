//! Bounded current-host physical and block storage inventory.
//!
//! This facade intentionally stays separate from [`crate::storage`], which
//! reports capacity for one mounted path. Device identities here are
//! provider-local and never include serial numbers, WWNs, or Windows UniqueId.

use std::{
    io::Read,
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::{
    contained_process::{ContainedChild, ContainedHeadlessCommand},
    process_spawn::ProcessExit,
};

#[path = "contract/storage_device_inventory.rs"]
mod contract;

pub use contract::{
    STORAGE_DEVICE_FIELD_CEILING, STORAGE_DEVICE_MAX_ROWS, STORAGE_DEVICE_PROVIDER_OUTPUT_CEILING,
    STORAGE_DEVICE_SCAN_CEILING, StorageDevice, StorageDeviceError, StorageDeviceErrorKind,
    StorageDeviceInventory,
};

#[cfg(target_os = "linux")]
#[path = "adapters/linux/storage_device_inventory.rs"]
mod adapter;
#[cfg(target_os = "macos")]
#[path = "adapters/macos/storage_device_inventory.rs"]
mod adapter;
#[cfg(windows)]
#[path = "adapters/windows/storage_device_inventory.rs"]
mod adapter;

const INVENTORY_TIMEOUT: Duration = Duration::from_secs(15);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

/// Enumerate at most `max_rows` rows from one bounded native snapshot.
pub fn enumerate(max_rows: usize) -> Result<StorageDeviceInventory, StorageDeviceError> {
    if !(1..=STORAGE_DEVICE_MAX_ROWS).contains(&max_rows) {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::InvalidLimit,
            format!("max_rows must be between 1 and {STORAGE_DEVICE_MAX_ROWS}, got {max_rows}"),
        ));
    }
    let deadline = Instant::now()
        .checked_add(INVENTORY_TIMEOUT)
        .ok_or_else(|| {
            StorageDeviceError::new(
                StorageDeviceErrorKind::Timeout,
                "inventory deadline overflow",
            )
        })?;
    finish_inventory(adapter::enumerate_native(deadline)?, max_rows)
}

fn finish_inventory(
    mut inventory: StorageDeviceInventory,
    max_rows: usize,
) -> Result<StorageDeviceInventory, StorageDeviceError> {
    if inventory.visited > STORAGE_DEVICE_SCAN_CEILING
        || inventory.devices.len() > inventory.visited
        || inventory.read_errors > inventory.visited
    {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::MalformedSnapshot,
            "provider returned incoherent inventory counts",
        ));
    }
    inventory.devices.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.node.cmp(&right.node))
            .then_with(|| left.name.cmp(&right.name))
    });
    let projection_truncated = inventory.devices.len() > max_rows;
    inventory.devices.truncate(max_rows);
    inventory.truncated |= inventory.truncated_scan || projection_truncated;
    inventory.complete = !inventory.truncated_scan && inventory.read_errors == 0;
    Ok(inventory)
}

pub(crate) struct ProviderOutput {
    pub(crate) stdout: Vec<u8>,
}

pub(crate) fn run_fixed_provider(
    program: &Path,
    args: &[&str],
    stdin: Option<String>,
    deadline: Instant,
    provider_name: &'static str,
) -> Result<ProviderOutput, StorageDeviceError> {
    validate_fixed_tool(program, provider_name)?;
    if Instant::now() >= deadline {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::Timeout,
            format!("{provider_name} deadline expired before spawn"),
        ));
    }
    let mut command = ContainedHeadlessCommand::new(program);
    command.args(args.iter().copied()).capture_output();
    if let Some(stdin) = stdin {
        command.stdin_text(stdin);
    }
    let mut child = command.spawn().map_err(|error| {
        let kind = match error.kind() {
            std::io::ErrorKind::NotFound => StorageDeviceErrorKind::ProviderUnavailable,
            std::io::ErrorKind::PermissionDenied => StorageDeviceErrorKind::PermissionDenied,
            _ => StorageDeviceErrorKind::ProviderFailed,
        };
        StorageDeviceError::new(kind, format!("{provider_name} could not be started"))
    })?;
    let stdout = child.take_stdout().ok_or_else(|| {
        cleanup_error(
            &mut child,
            format!("{provider_name} stdout capture was unavailable"),
        )
    })?;
    let stderr = match child.take_stderr() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            return Err(cleanup_error(
                &mut child,
                format!("{provider_name} stderr capture was unavailable"),
            ));
        }
    };
    let capture = Arc::new(Mutex::new(Capture::new()));
    let stdout_thread = drain(stdout, Arc::clone(&capture), Stream::Stdout);
    let stderr_thread = drain(stderr, Arc::clone(&capture), Stream::Stderr);

    let exit = loop {
        let capture_failed = capture
            .lock()
            .map_or(true, |value| value.exceeded || value.allocation_failed);
        if capture_failed {
            terminate(&mut child)?;
            join(stdout_thread, stderr_thread, provider_name)?;
            let capture = capture.lock().map_err(|_| {
                StorageDeviceError::new(
                    StorageDeviceErrorKind::ProviderFailed,
                    "provider capture state was poisoned",
                )
            })?;
            return Err(if capture.allocation_failed {
                StorageDeviceError::new(
                    StorageDeviceErrorKind::ResourceLimit,
                    format!("{provider_name} output allocation failed"),
                )
            } else {
                StorageDeviceError::new(
                    StorageDeviceErrorKind::OutputLimit,
                    format!("{provider_name} output exceeded the aggregate 2 MiB ceiling"),
                )
            });
        }
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                terminate(&mut child)?;
                join(stdout_thread, stderr_thread, provider_name)?;
                return Err(StorageDeviceError::new(
                    StorageDeviceErrorKind::Timeout,
                    format!("{provider_name} exceeded the shared inventory deadline"),
                ));
            }
            Err(_) => {
                terminate(&mut child)?;
                join(stdout_thread, stderr_thread, provider_name)?;
                return Err(StorageDeviceError::new(
                    StorageDeviceErrorKind::ProviderFailed,
                    format!("{provider_name} child status could not be observed"),
                ));
            }
        }
    };
    terminate(&mut child)?;
    join(stdout_thread, stderr_thread, provider_name)?;
    let capture = Arc::try_unwrap(capture)
        .map_err(|_| {
            StorageDeviceError::new(
                StorageDeviceErrorKind::ProviderFailed,
                "provider capture ownership remained shared",
            )
        })?
        .into_inner()
        .map_err(|_| {
            StorageDeviceError::new(
                StorageDeviceErrorKind::ProviderFailed,
                "provider capture state was poisoned",
            )
        })?;
    if capture.exceeded {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::OutputLimit,
            format!("{provider_name} output exceeded the aggregate 2 MiB ceiling"),
        ));
    }
    if capture.allocation_failed {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::ResourceLimit,
            format!("{provider_name} output allocation failed"),
        ));
    }
    if !matches!(exit, ProcessExit::Code(0)) {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::ProviderFailed,
            format!(
                "{provider_name} exited unsuccessfully ({})",
                exit.conventional_code()
                    .map_or_else(|| "unknown".to_owned(), |value| value.to_string())
            ),
        ));
    }
    Ok(ProviderOutput {
        stdout: capture.stdout,
    })
}

fn validate_fixed_tool(path: &Path, name: &'static str) -> Result<(), StorageDeviceError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| {
        StorageDeviceError::new(
            StorageDeviceErrorKind::ProviderUnavailable,
            format!("the fixed system {name} is unavailable"),
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::ProviderUnavailable,
            format!("the fixed system {name} is not a regular non-symlink file"),
        ));
    }
    Ok(())
}

fn terminate(child: &mut ContainedChild) -> Result<(), StorageDeviceError> {
    child.terminate_and_wait(CLEANUP_TIMEOUT).map_err(|_| {
        StorageDeviceError::new(
            StorageDeviceErrorKind::CleanupFailed,
            "provider process-tree cleanup could not be verified",
        )
    })
}

fn cleanup_error(child: &mut ContainedChild, message: String) -> StorageDeviceError {
    if terminate(child).is_err() {
        return StorageDeviceError::new(
            StorageDeviceErrorKind::CleanupFailed,
            "provider capture failed and process-tree cleanup could not be verified",
        );
    }
    StorageDeviceError::new(StorageDeviceErrorKind::ProviderFailed, message)
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

struct Capture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    remaining: usize,
    exceeded: bool,
    allocation_failed: bool,
}

impl Capture {
    fn new() -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            remaining: STORAGE_DEVICE_PROVIDER_OUTPUT_CEILING,
            exceeded: false,
            allocation_failed: false,
        }
    }

    fn push(&mut self, stream: Stream, bytes: &[u8]) {
        let accepted = self.remaining.min(bytes.len());
        let target = match stream {
            Stream::Stdout => &mut self.stdout,
            Stream::Stderr => &mut self.stderr,
        };
        if target.try_reserve(accepted).is_err() {
            self.allocation_failed = true;
            return;
        }
        target.extend_from_slice(&bytes[..accepted]);
        self.remaining -= accepted;
        self.exceeded |= accepted != bytes.len();
    }
}

fn drain(
    mut stream: impl Read + Send + 'static,
    capture: Arc<Mutex<Capture>>,
    target: Stream,
) -> thread::JoinHandle<std::io::Result<()>> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8_192];
        loop {
            let length = stream.read(&mut buffer)?;
            if length == 0 {
                return Ok(());
            }
            let mut capture = capture
                .lock()
                .map_err(|_| std::io::Error::other("provider capture state was poisoned"))?;
            capture.push(target, &buffer[..length]);
        }
    })
}

fn join(
    stdout: thread::JoinHandle<std::io::Result<()>>,
    stderr: thread::JoinHandle<std::io::Result<()>>,
    provider_name: &'static str,
) -> Result<(), StorageDeviceError> {
    for worker in [stdout, stderr] {
        worker
            .join()
            .map_err(|_| {
                StorageDeviceError::new(
                    StorageDeviceErrorKind::ProviderFailed,
                    format!("{provider_name} capture worker panicked"),
                )
            })?
            .map_err(|_| {
                StorageDeviceError::new(
                    StorageDeviceErrorKind::ProviderFailed,
                    format!("{provider_name} output could not be read"),
                )
            })?;
    }
    Ok(())
}

pub(crate) fn parse_json(bytes: &[u8]) -> Result<serde_json::Value, StorageDeviceError> {
    serde_json::from_slice(bytes).map_err(|_| {
        StorageDeviceError::new(
            StorageDeviceErrorKind::MalformedSnapshot,
            "storage provider emitted malformed or excessively deep JSON",
        )
    })
}

pub(crate) fn bounded_text(
    value: Option<&serde_json::Value>,
    field: &'static str,
) -> Result<Option<String>, StorageDeviceError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let text = value.as_str().ok_or_else(|| malformed(field))?;
    if text.is_empty() {
        return Ok(None);
    }
    if text.len() > STORAGE_DEVICE_FIELD_CEILING || text.chars().any(char::is_control) {
        return Err(malformed(field));
    }
    Ok(Some(text.to_owned()))
}

#[cfg(windows)]
pub(crate) fn bounded_string_list(
    value: Option<&serde_json::Value>,
    field: &'static str,
) -> Result<Vec<String>, StorageDeviceError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    if value.is_string() {
        return bounded_text(Some(value), field).map(|value| value.into_iter().collect());
    }
    let rows = value.as_array().ok_or_else(|| malformed(field))?;
    if rows.len() > 64 {
        return Err(StorageDeviceError::new(
            StorageDeviceErrorKind::ResourceLimit,
            format!("{field} exceeds 64 values"),
        ));
    }
    rows.iter()
        .map(|value| bounded_text(Some(value), field)?.ok_or_else(|| malformed(field)))
        .collect()
}

pub(crate) fn optional_u64(
    value: Option<&serde_json::Value>,
    field: &'static str,
) -> Result<Option<u64>, StorageDeviceError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(value) = value.as_u64() {
        return Ok(Some(value));
    }
    let text = value.as_str().ok_or_else(|| malformed(field))?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(malformed(field));
    }
    text.parse::<u64>().map(Some).map_err(|_| malformed(field))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn optional_bool(
    value: Option<&serde_json::Value>,
    field: &'static str,
) -> Result<Option<bool>, StorageDeviceError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(value) = value.as_bool() {
        return Ok(Some(value));
    }
    match value.as_u64() {
        Some(0) => Ok(Some(false)),
        Some(1) => Ok(Some(true)),
        _ => Err(malformed(field)),
    }
}

pub(crate) fn malformed(field: &'static str) -> StorageDeviceError {
    StorageDeviceError::new(
        StorageDeviceErrorKind::MalformedSnapshot,
        format!("storage provider emitted an invalid {field}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str) -> StorageDevice {
        StorageDevice {
            id: id.into(),
            node: None,
            name: id.into(),
            kind: Some("disk".into()),
            size_bytes: Some(9_007_199_254_740_993),
            media_type: None,
            bus: None,
            health: None,
            health_semantics: None,
            operational: Vec::new(),
            internal: None,
            removable: None,
            ejectable: None,
            solid_state: None,
            read_only: None,
            virtual_device: None,
        }
    }

    #[test]
    fn rejects_limits_before_native_enumeration() {
        assert_eq!(
            enumerate(0).unwrap_err().kind(),
            StorageDeviceErrorKind::InvalidLimit
        );
        assert_eq!(
            enumerate(STORAGE_DEVICE_MAX_ROWS + 1).unwrap_err().kind(),
            StorageDeviceErrorKind::InvalidLimit
        );
    }

    #[test]
    fn projection_is_stable_bounded_and_preserves_u64_capacity() {
        let inventory = StorageDeviceInventory {
            devices: vec![device("z"), device("a")],
            visited: 2,
            read_errors: 0,
            truncated_scan: false,
            truncated: false,
            complete: false,
            provider: "fixture",
        };
        let inventory = finish_inventory(inventory, 1).unwrap();
        assert_eq!(inventory.devices[0].id, "a");
        assert_eq!(inventory.devices[0].size_bytes, Some(9_007_199_254_740_993));
        assert!(inventory.truncated);
        assert!(inventory.complete);
    }

    #[test]
    fn malformed_numeric_values_do_not_become_zero() {
        for value in [
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("12x"),
        ] {
            assert_eq!(
                optional_u64(Some(&value), "size").unwrap_err().kind(),
                StorageDeviceErrorKind::MalformedSnapshot
            );
        }
    }
}
