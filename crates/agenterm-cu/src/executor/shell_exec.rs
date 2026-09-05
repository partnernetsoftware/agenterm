//! Bounded one-shot host shell execution for the MCU compatibility frontier.

use std::{
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use agenterm_platform::{contained_process::ContainedHeadlessCommand, process_spawn::ProcessExit};
use serde_json::{Value, json};

use crate::CuError;

pub(super) fn shell_exec_payload(
    command_text: &str,
    timeout_ms: u64,
    max_output_bytes: usize,
) -> Result<Value, CuError> {
    let (program, arguments, shell) = shell_command(command_text)?;
    let mut command = ContainedHeadlessCommand::new(program);
    command.args(arguments).capture_output();
    let mut child = command.spawn().map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::NotFound {
            "shell_unavailable"
        } else {
            "shell_exec_spawn_failed"
        };
        CuError::new(code, format!("could not start the host shell: {error}"))
    })?;
    let pid = child.id();
    let stdout = match child.take_stdout() {
        Some(stream) => stream,
        None => {
            terminate(&mut child, "shell_exec_capture_failed")?;
            return Err(CuError::new(
                "shell_exec_capture_failed",
                "contained shell omitted its stdout capture stream",
            )
            .with_detail(json!({"cleanup": "verified"})));
        }
    };
    let stderr = match child.take_stderr() {
        Some(stream) => stream,
        None => {
            drop(stdout);
            terminate(&mut child, "shell_exec_capture_failed")?;
            return Err(CuError::new(
                "shell_exec_capture_failed",
                "contained shell omitted its stderr capture stream",
            )
            .with_detail(json!({"cleanup": "verified"})));
        }
    };
    let capture = Arc::new(Mutex::new(Capture::new(max_output_bytes)));
    let stdout_thread = drain(stdout, Arc::clone(&capture), Stream::Stdout);
    let stderr_thread = drain(stderr, Arc::clone(&capture), Stream::Stderr);
    let started = Instant::now();
    let deadline = started + Duration::from_millis(timeout_ms);
    let exit = loop {
        if capture.lock().map_or(true, |capture| capture.exceeded) {
            terminate(&mut child, "shell_exec_output_limit")?;
            join_drains(stdout_thread, stderr_thread)?;
            return Err(CuError::new(
                "shell_exec_output_limit",
                "shell stdout and stderr exceeded the caller's aggregate byte limit",
            )
            .with_detail(json!({
                "cleanup": "verified",
                "max_output_bytes": max_output_bytes,
            })));
        }
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                terminate(&mut child, "shell_exec_timeout")?;
                join_drains(stdout_thread, stderr_thread)?;
                return Err(CuError::new(
                    "shell_exec_timeout",
                    "shell command exceeded its deadline",
                )
                .with_detail(json!({
                    "cleanup": "verified",
                    "timeout_ms": timeout_ms,
                })));
            }
            Err(error) => {
                terminate(&mut child, "shell_exec_wait_failed")?;
                join_drains(stdout_thread, stderr_thread)?;
                return Err(CuError::new(
                    "shell_exec_wait_failed",
                    format!("could not observe the contained shell: {error}"),
                )
                .with_detail(json!({"cleanup": "verified"})));
            }
        }
    };
    // A shell may return while a deliberately backgrounded descendant still
    // owns one of its inherited output handles. Close the native containment
    // only after preserving the root exit fact, then drain to EOF. On Windows
    // this closes the Job; on Unix the EXIT trap below has already waited for
    // ordinary descendants while the process-group identity was still owned.
    terminate(&mut child, "shell_exec_root_exited")?;
    join_drains(stdout_thread, stderr_thread)?;
    let capture = Arc::try_unwrap(capture)
        .map_err(|_| {
            CuError::new(
                "shell_exec_capture_failed",
                "capture workers did not release",
            )
        })?
        .into_inner()
        .map_err(|_| CuError::new("shell_exec_capture_failed", "capture state was poisoned"))?;
    Ok(json!({
        "schema_version": 1,
        "shell": shell,
        "pid": pid,
        "elapsed_ms": started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        "exit": {
            "kind": exit.as_str(),
            "code": exit.conventional_code(),
        },
        "success": matches!(exit, ProcessExit::Code(0)),
        "stdout": String::from_utf8_lossy(&capture.stdout),
        "stderr": String::from_utf8_lossy(&capture.stderr),
        "stdout_bytes": capture.stdout.len(),
        "stderr_bytes": capture.stderr.len(),
        "output_complete": !capture.exceeded,
        "cleanup": "verified",
    }))
}

#[cfg(unix)]
fn shell_command(command: &str) -> Result<(PathBuf, Vec<String>, &'static str), CuError> {
    // `wait` keeps ordinary background jobs under the live root until the
    // caller's deadline can terminate the complete process group.
    Ok((
        PathBuf::from("/bin/sh"),
        vec![
            "-c".into(),
            "trap 'status=$?; trap - EXIT; wait; exit \"$status\"' EXIT; eval \"$1\"".into(),
            "agenterm-shell".into(),
            command.into(),
        ],
        "sh",
    ))
}

#[cfg(windows)]
fn shell_command(command: &str) -> Result<(PathBuf, Vec<String>, &'static str), CuError> {
    let system_root = std::env::var_os("SystemRoot").ok_or_else(|| {
        CuError::new(
            "shell_unavailable",
            "Windows SystemRoot is unavailable; PowerShell cannot be resolved",
        )
    })?;
    let powershell = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    Ok((
        powershell,
        vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            command.into(),
        ],
        "powershell",
    ))
}

fn terminate(
    child: &mut agenterm_platform::contained_process::ContainedChild,
    reason: &'static str,
) -> Result<(), CuError> {
    child
        .terminate_and_wait(Duration::from_secs(2))
        .map_err(|error| {
            CuError::new(
                "shell_exec_cleanup_uncertain",
                format!("{reason}; contained process cleanup could not be verified: {error}"),
            )
            .with_detail(json!({"cleanup": "uncertain", "reason": reason}))
        })
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
}

impl Capture {
    fn new(limit: usize) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            remaining: limit,
            exceeded: false,
        }
    }

    fn push(&mut self, stream: Stream, bytes: &[u8]) {
        let accepted = self.remaining.min(bytes.len());
        let target = match stream {
            Stream::Stdout => &mut self.stdout,
            Stream::Stderr => &mut self.stderr,
        };
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
                .map_err(|_| std::io::Error::other("capture state was poisoned"))?;
            capture.push(target, &buffer[..length]);
        }
    })
}

fn join_drains(
    stdout: thread::JoinHandle<std::io::Result<()>>,
    stderr: thread::JoinHandle<std::io::Result<()>>,
) -> Result<(), CuError> {
    for worker in [stdout, stderr] {
        worker
            .join()
            .map_err(|_| CuError::new("shell_exec_capture_failed", "capture worker panicked"))?
            .map_err(|error| {
                CuError::new(
                    "shell_exec_capture_failed",
                    format!("could not read shell output: {error}"),
                )
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_capture_never_exceeds_its_budget() {
        let mut capture = Capture::new(5);
        capture.push(Stream::Stdout, b"abc");
        capture.push(Stream::Stderr, b"defg");
        assert_eq!(capture.stdout, b"abc");
        assert_eq!(capture.stderr, b"de");
        assert!(capture.exceeded);
        assert_eq!(capture.remaining, 0);
    }
}
