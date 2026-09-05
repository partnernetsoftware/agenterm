use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::contained_process::{ContainedChild, ContainedHeadlessCommand};
use crate::process_spawn::ProcessExit;

use super::super::{
    SimulatorBootReceipt, SimulatorDevice, SimulatorDeviceList, SimulatorError, SimulatorErrorKind,
    parse_device_list,
};

const XCRUN: &str = "/usr/bin/xcrun";
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn list_devices(max: usize) -> Result<SimulatorDeviceList, SimulatorError> {
    let deadline = Instant::now()
        .checked_add(LIST_TIMEOUT)
        .ok_or_else(|| SimulatorError::new(SimulatorErrorKind::Timeout, "deadline overflow"))?;
    list_devices_until(max, deadline)
}

pub(crate) fn boot_exact(
    udid: &str,
    timeout: Duration,
) -> Result<SimulatorBootReceipt, SimulatorError> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        SimulatorError::new(SimulatorErrorKind::InvalidTimeout, "deadline overflow")
    })?;
    let initial = list_exact_until(udid, deadline)?;
    let before = exact_device(&initial, udid)?.ok_or_else(|| {
        SimulatorError::new(
            SimulatorErrorKind::NotFound,
            "the exact CoreSimulator UDID was not found",
        )
    })?;
    if before.is_booted() {
        return Ok(receipt(&before.udid, &before.state, &before.state, true));
    }

    let boot = run_xcrun(&["simctl", "boot", udid], deadline)?;
    let boot_accepted = matches!(boot.exit, ProcessExit::Code(0));

    loop {
        let observed = list_exact_until(udid, deadline)?;
        let current = exact_device(&observed, udid)?.ok_or_else(|| {
            SimulatorError::new(
                SimulatorErrorKind::Changed,
                "the exact CoreSimulator UDID disappeared during boot verification",
            )
        })?;
        if current.runtime != before.runtime || current.device_type != before.device_type {
            return Err(SimulatorError::new(
                SimulatorErrorKind::Changed,
                "the CoreSimulator identity metadata changed during boot verification",
            ));
        }
        if current.is_booted() {
            return Ok(receipt(&before.udid, &before.state, &current.state, false));
        }
        if !boot_accepted {
            if current.state != before.state {
                return Err(SimulatorError::new(
                    SimulatorErrorKind::Changed,
                    "the CoreSimulator state changed after the boot command was rejected",
                ));
            }
            return Err(SimulatorError::new(
                SimulatorErrorKind::Unavailable,
                "xcrun simctl boot did not accept the exact device",
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(SimulatorError::new(
                SimulatorErrorKind::Timeout,
                "CoreSimulator did not reach Booted before the deadline",
            ));
        }
        thread::sleep(POLL_INTERVAL.min(remaining));
    }
}

fn receipt(
    udid: &str,
    before_state: &str,
    after_state: &str,
    already_booted: bool,
) -> SimulatorBootReceipt {
    SimulatorBootReceipt {
        udid: udid.to_owned(),
        before_state: before_state.to_owned(),
        after_state: after_state.to_owned(),
        already_booted,
    }
}

fn exact_device<'a>(
    list: &'a SimulatorDeviceList,
    udid: &str,
) -> Result<Option<&'a SimulatorDevice>, SimulatorError> {
    let mut matches = list
        .devices
        .iter()
        .filter(|device| device.udid.eq_ignore_ascii_case(udid));
    let first = matches.next();
    if matches.next().is_some() {
        return Err(SimulatorError::new(
            SimulatorErrorKind::InvalidJson,
            "simctl returned the same UDID more than once",
        ));
    }
    Ok(first)
}

fn list_devices_until(
    max: usize,
    deadline: Instant,
) -> Result<SimulatorDeviceList, SimulatorError> {
    let output = run_xcrun(&["simctl", "list", "devices", "--json"], deadline)?;
    if !matches!(output.exit, ProcessExit::Code(0)) {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            "xcrun simctl list devices failed",
        ));
    }
    parse_device_list(&output.stdout, max)
}

fn list_exact_until(udid: &str, deadline: Instant) -> Result<SimulatorDeviceList, SimulatorError> {
    let output = run_xcrun(&["simctl", "list", "--json", "devices", udid], deadline)?;
    if !matches!(output.exit, ProcessExit::Code(0)) {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            "xcrun simctl could not query the exact device",
        ));
    }
    let list = parse_device_list(&output.stdout, super::super::MAX_SIMULATOR_DEVICES)?;
    if list.truncated {
        return Err(SimulatorError::new(
            SimulatorErrorKind::InvalidJson,
            "exact-UDID simctl query returned more than 200 devices",
        ));
    }
    Ok(list)
}

struct CommandOutput {
    exit: ProcessExit,
    stdout: Vec<u8>,
}

fn run_xcrun(args: &[&str], deadline: Instant) -> Result<CommandOutput, SimulatorError> {
    validate_xcrun()?;
    if Instant::now() >= deadline {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Timeout,
            "CoreSimulator command deadline expired before spawn",
        ));
    }
    let mut command = ContainedHeadlessCommand::new(XCRUN);
    command.args(args.iter().copied()).capture_output();
    let mut child = command.spawn().map_err(|_| {
        SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            "the fixed system xcrun could not be started",
        )
    })?;
    let stdout = child
        .take_stdout()
        .ok_or_else(|| cleanup_error(&mut child, "xcrun stdout capture was unavailable"))?;
    let stderr = match child.take_stderr() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            return Err(cleanup_error(
                &mut child,
                "xcrun stderr capture was unavailable",
            ));
        }
    };
    let capture = Arc::new(Mutex::new(Capture::new()));
    let stdout_thread = drain(stdout, Arc::clone(&capture), Stream::Stdout);
    let stderr_thread = drain(stderr, Arc::clone(&capture), Stream::Stderr);

    let exit = loop {
        if capture.lock().map_or(true, |capture| {
            capture.exceeded || capture.allocation_failed
        }) {
            terminate(&mut child)?;
            join(stdout_thread, stderr_thread)?;
            let capture = capture.lock().map_err(|_| {
                SimulatorError::new(SimulatorErrorKind::Io, "capture state was poisoned")
            })?;
            return Err(if capture.allocation_failed {
                SimulatorError::new(
                    SimulatorErrorKind::Io,
                    "xcrun output allocation was unavailable",
                )
            } else {
                SimulatorError::new(
                    SimulatorErrorKind::OutputLimit,
                    "xcrun output exceeded the aggregate 2 MiB limit",
                )
            });
        }
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                terminate(&mut child)?;
                join(stdout_thread, stderr_thread)?;
                return Err(SimulatorError::new(
                    SimulatorErrorKind::Timeout,
                    "CoreSimulator command exceeded its deadline",
                ));
            }
            Err(_) => {
                terminate(&mut child)?;
                join(stdout_thread, stderr_thread)?;
                return Err(SimulatorError::new(
                    SimulatorErrorKind::Io,
                    "CoreSimulator child status could not be observed",
                ));
            }
        }
    };
    terminate(&mut child)?;
    join(stdout_thread, stderr_thread)?;
    let capture = Arc::try_unwrap(capture)
        .map_err(|_| {
            SimulatorError::new(SimulatorErrorKind::Io, "capture ownership remained shared")
        })?
        .into_inner()
        .map_err(|_| SimulatorError::new(SimulatorErrorKind::Io, "capture state was poisoned"))?;
    if capture.exceeded {
        return Err(SimulatorError::new(
            SimulatorErrorKind::OutputLimit,
            "xcrun output exceeded the aggregate 2 MiB limit",
        ));
    }
    if capture.allocation_failed {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Io,
            "xcrun output allocation was unavailable",
        ));
    }
    Ok(CommandOutput {
        exit,
        stdout: capture.stdout,
    })
}

fn validate_xcrun() -> Result<(), SimulatorError> {
    let metadata = std::fs::symlink_metadata(Path::new(XCRUN)).map_err(|_| {
        SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            "the fixed system xcrun is unavailable",
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            "the fixed system xcrun is not a regular non-symlink file",
        ));
    }
    Ok(())
}

fn terminate(child: &mut ContainedChild) -> Result<(), SimulatorError> {
    child.terminate_and_wait(CLEANUP_TIMEOUT).map_err(|_| {
        SimulatorError::new(
            SimulatorErrorKind::Io,
            "CoreSimulator command cleanup could not be verified",
        )
    })
}

fn cleanup_error(child: &mut ContainedChild, message: &'static str) -> SimulatorError {
    if terminate(child).is_err() {
        return SimulatorError::new(
            SimulatorErrorKind::Io,
            "CoreSimulator capture failed and cleanup could not be verified",
        );
    }
    SimulatorError::new(SimulatorErrorKind::Io, message)
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
            remaining: MAX_OUTPUT_BYTES,
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
        let mut buffer = [0u8; 8_192];
        loop {
            let length = stream.read(&mut buffer)?;
            if length == 0 {
                return Ok(());
            }
            let mut capture = capture
                .lock()
                .map_err(|_| std::io::Error::other("capture state was poisoned"))?;
            capture.push(target, &buffer[..length]);
        }
    })
}

fn join(
    stdout: thread::JoinHandle<std::io::Result<()>>,
    stderr: thread::JoinHandle<std::io::Result<()>>,
) -> Result<(), SimulatorError> {
    for worker in [stdout, stderr] {
        worker
            .join()
            .map_err(|_| SimulatorError::new(SimulatorErrorKind::Io, "capture worker panicked"))?
            .map_err(|_| SimulatorError::new(SimulatorErrorKind::Io, "xcrun output read failed"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn already_booted_receipt_is_idempotent() {
        let udid = "12345678-1234-1234-1234-123456789ABC";
        let result = receipt(udid, "Booted", "Booted", true);
        assert_eq!(result.udid, udid);
        assert_eq!(result.before_state, "Booted");
        assert_eq!(result.after_state, "Booted");
        assert!(result.already_booted);
    }
}
