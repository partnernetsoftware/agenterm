use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::contained_process::{ContainedChild, ContainedHeadlessCommand};
use crate::process_spawn::ProcessExit;

use super::super::{
    SimulatorAppAction, SimulatorAppLifecycleReceipt, SimulatorAppList, SimulatorBootReceipt,
    SimulatorDevice, SimulatorDeviceList, SimulatorError, SimulatorErrorKind, parse_app_list,
    parse_device_list,
};

const XCRUN: &str = "/usr/bin/xcrun";
const PLUTIL: &str = "/usr/bin/plutil";
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

pub(crate) fn list_apps(udid: &str, max: usize) -> Result<SimulatorAppList, SimulatorError> {
    let deadline = Instant::now()
        .checked_add(LIST_TIMEOUT)
        .ok_or_else(|| SimulatorError::new(SimulatorErrorKind::Timeout, "deadline overflow"))?;
    let before = require_booted_device(udid, deadline)?;
    let apps = list_apps_until(udid, max, deadline)?;
    verify_same_booted_device(&before, deadline)?;
    Ok(apps)
}

pub(crate) fn launch_exact(
    udid: &str,
    bundle_id: &str,
    timeout: Duration,
) -> Result<SimulatorAppLifecycleReceipt, SimulatorError> {
    app_lifecycle(udid, bundle_id, timeout, SimulatorAppAction::Launch)
}

pub(crate) fn terminate_exact(
    udid: &str,
    bundle_id: &str,
    timeout: Duration,
) -> Result<SimulatorAppLifecycleReceipt, SimulatorError> {
    app_lifecycle(udid, bundle_id, timeout, SimulatorAppAction::Terminate)
}

fn app_lifecycle(
    udid: &str,
    bundle_id: &str,
    timeout: Duration,
    action: SimulatorAppAction,
) -> Result<SimulatorAppLifecycleReceipt, SimulatorError> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        SimulatorError::new(SimulatorErrorKind::InvalidTimeout, "deadline overflow")
    })?;
    let before = require_booted_device(udid, deadline)?;
    let inventory = list_apps_until(udid, super::super::MAX_VISITED_APPS, deadline)?;
    if !inventory.apps.iter().any(|app| app.bundle_id == bundle_id) {
        return Err(SimulatorError::new(
            SimulatorErrorKind::NotFound,
            "the exact app bundle id is not installed on the simulator",
        ));
    }
    let verb = match action {
        SimulatorAppAction::Launch => "launch",
        SimulatorAppAction::Terminate => "terminate",
    };
    let output = run_xcrun(&["simctl", verb, udid, bundle_id], deadline)?;
    let after = verify_same_booted_device(&before, deadline)?;
    if !matches!(output.exit, ProcessExit::Code(0)) {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            "xcrun simctl did not accept the exact app lifecycle request",
        ));
    }
    let launch_pid = match action {
        SimulatorAppAction::Launch => Some(parse_launch_pid(&output.stdout, bundle_id)?),
        SimulatorAppAction::Terminate => {
            if output.stdout.iter().any(|byte| !byte.is_ascii_whitespace()) {
                return Err(SimulatorError::new(
                    SimulatorErrorKind::Unavailable,
                    "simctl terminate returned an unexpected acknowledgement",
                ));
            }
            None
        }
    };
    Ok(SimulatorAppLifecycleReceipt {
        device_udid: before.udid,
        bundle_id: bundle_id.to_owned(),
        action,
        device_state_before: before.state,
        device_state_after: after.state,
        accepted: true,
        verified: false,
        launch_pid,
    })
}

fn parse_launch_pid(stdout: &[u8], bundle_id: &str) -> Result<u32, SimulatorError> {
    let text = std::str::from_utf8(stdout).map_err(|_| {
        SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            "simctl launch acknowledgement is not UTF-8",
        )
    })?;
    let line = text.trim();
    let expected_prefix = format!("{bundle_id}: ");
    let pid = line
        .strip_prefix(&expected_prefix)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            SimulatorError::new(
                SimulatorErrorKind::Unavailable,
                "simctl launch acknowledgement has an unexpected shape",
            )
        })?;
    Ok(pid)
}

fn require_booted_device(udid: &str, deadline: Instant) -> Result<SimulatorDevice, SimulatorError> {
    let inventory = list_exact_until(udid, deadline)?;
    let device = exact_device(&inventory, udid)?.cloned().ok_or_else(|| {
        SimulatorError::new(SimulatorErrorKind::NotFound, "exact simulator not found")
    })?;
    if !device.is_booted() {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            "installed apps require an already booted simulator",
        ));
    }
    Ok(device)
}

fn verify_same_booted_device(
    before: &SimulatorDevice,
    deadline: Instant,
) -> Result<SimulatorDevice, SimulatorError> {
    let inventory = list_exact_until(&before.udid, deadline)?;
    let after = exact_device(&inventory, &before.udid)?
        .cloned()
        .ok_or_else(|| SimulatorError::new(SimulatorErrorKind::Changed, "simulator disappeared"))?;
    if after.runtime != before.runtime
        || after.device_type != before.device_type
        || !after.is_booted()
    {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Changed,
            "simulator identity or Booted state changed during the operation",
        ));
    }
    Ok(after)
}

fn list_apps_until(
    udid: &str,
    max: usize,
    deadline: Instant,
) -> Result<SimulatorAppList, SimulatorError> {
    let raw = run_xcrun(&["simctl", "listapps", udid], deadline)?;
    if !matches!(raw.exit, ProcessExit::Code(0)) {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            "xcrun simctl listapps failed",
        ));
    }
    let open_step = String::from_utf8(raw.stdout).map_err(|_| {
        SimulatorError::new(
            SimulatorErrorKind::InvalidJson,
            "listapps output is not UTF-8",
        )
    })?;
    let json = run_plutil(open_step, deadline)?;
    if !matches!(json.exit, ProcessExit::Code(0)) {
        return Err(SimulatorError::new(
            SimulatorErrorKind::InvalidJson,
            "system plutil rejected simctl listapps output",
        ));
    }
    parse_app_list(&json.stdout, udid, max)
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
    run_program(XCRUN, args, None, deadline)
}

fn run_plutil(input: String, deadline: Instant) -> Result<CommandOutput, SimulatorError> {
    validate_system_tool(PLUTIL, "plutil")?;
    run_program(
        PLUTIL,
        &["-convert", "json", "-o", "-", "--", "-"],
        Some(input),
        deadline,
    )
}

fn run_program(
    program: &str,
    args: &[&str],
    stdin: Option<String>,
    deadline: Instant,
) -> Result<CommandOutput, SimulatorError> {
    if Instant::now() >= deadline {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Timeout,
            "CoreSimulator command deadline expired before spawn",
        ));
    }
    let mut command = ContainedHeadlessCommand::new(program);
    command.args(args.iter().copied()).capture_output();
    if program == XCRUN {
        for (key, _) in std::env::vars_os() {
            if key
                .to_str()
                .is_some_and(|key| key.starts_with("SIMCTL_CHILD_"))
            {
                command.env_remove(key);
            }
        }
    }
    if let Some(stdin) = stdin {
        command.stdin_text(stdin);
    }
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
    validate_system_tool(XCRUN, "xcrun")
}

fn validate_system_tool(path: &str, name: &'static str) -> Result<(), SimulatorError> {
    let metadata = std::fs::symlink_metadata(Path::new(path)).map_err(|_| {
        SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            format!("the fixed system {name} is unavailable"),
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(SimulatorError::new(
            SimulatorErrorKind::Unavailable,
            format!("the fixed system {name} is not a regular non-symlink file"),
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

    #[test]
    fn launch_acknowledgement_is_exact_and_pid_is_not_verification() {
        assert_eq!(
            parse_launch_pid(b"com.example.app: 4242\n", "com.example.app").unwrap(),
            4242
        );
        for invalid in [
            b"com.example.other: 4242\n".as_slice(),
            b"com.example.app: 0\n",
            b"com.example.app: 42\nextra\n",
        ] {
            assert_eq!(
                parse_launch_pid(invalid, "com.example.app")
                    .unwrap_err()
                    .kind,
                SimulatorErrorKind::Unavailable
            );
        }
    }
}
