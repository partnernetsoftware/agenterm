//! OS-neutral process facade service.
//!
//! This module owns the stable product-facing verbs while `selected` resolves
//! the one native adapter for the compilation target.

use std::{
    path::PathBuf,
    process::{ChildStderr, ChildStdout, Command},
};

use crate::{contract::process::ProcessInfo, selected::process as adapter};

pub use crate::contract::process::{
    PROCESS_ENVIRONMENT_MAX_BYTES, ProcessEnvironmentEntry, ProcessEnvironmentSnapshot,
    ProcessError, ProcessErrorKind, ProcessFileDescriptor, ProcessInspection, ProcessMemoryRegion,
    ProcessSocketInfo, ProcessThreadInfo,
};
pub use crate::contract::process::{PipeProbeError, PipeProbeToken};
pub use crate::process_observation::{ProcessObservation, observe, start_identity};
pub use crate::process_spawn::{
    DetachedSpawnMode, ProcessExit, classify_exit_status, configure_breakaway_visible_command,
    configure_detached_command, configure_owned_headless_command, is_breakaway_denied,
    spawn_breakaway_visible_child, spawn_breakaway_visible_command, spawn_detached_child,
    spawn_detached_command,
};
pub use adapter::ProcessTreeGuard;

pub fn list() -> Result<Vec<ProcessInfo>, ProcessError> {
    adapter::list()
}

/// Read one process's bounded command line for mechanism discovery. Callers
/// must not publish it wholesale: arguments routinely contain credentials.
pub fn command_line(pid: u32) -> Result<String, ProcessError> {
    adapter::command_line(pid)
}

/// Read one process's bounded argument vector without losing argument
/// boundaries. Callers must keep values opt-in because arguments routinely
/// contain credentials.
pub fn arguments(pid: u32) -> Result<Vec<String>, ProcessError> {
    adapter::arguments(pid)
}

/// Read the bounded initial environment block installed when a process was
/// executed. Later `setenv`/`putenv` mutations are not part of this contract.
/// Raw entry bytes remain undecoded so product callers can expose them without
/// collapsing non-UTF-8 native values.
pub fn environment_snapshot(pid: u32) -> Result<ProcessEnvironmentSnapshot, ProcessError> {
    if pid == 0 {
        return Err(ProcessError::new(
            ProcessErrorKind::IdOutOfRange,
            "process id must be greater than zero",
        ));
    }
    adapter::environment_snapshot(pid)
}

/// Read one live process's current working directory.
///
/// This is a point-in-time mechanism read. Callers that publish the result for
/// an existing process must bracket it with a stable process-start identity so
/// PID reuse cannot substitute a different target.
pub fn current_directory(pid: u32) -> Result<PathBuf, ProcessError> {
    if pid == 0 {
        return Err(ProcessError::new(
            ProcessErrorKind::IdOutOfRange,
            "process id must be greater than zero",
        ));
    }
    adapter::current_directory(pid)
}

fn validate_inspection(pid: u32, max_visited: usize) -> Result<(), ProcessError> {
    if pid == 0 {
        return Err(ProcessError::new(
            ProcessErrorKind::IdOutOfRange,
            "process id must be greater than zero",
        ));
    }
    if !(1..=10_000).contains(&max_visited) {
        return Err(ProcessError::new(
            ProcessErrorKind::InventoryTooLarge,
            "max_visited must be in 1..=10000",
        ));
    }
    Ok(())
}

/// Read one bounded process-local file-descriptor inventory.
pub fn file_descriptors(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessFileDescriptor>, ProcessError> {
    validate_inspection(pid, max_visited)?;
    adapter::file_descriptors(pid, max_visited)
}

/// Read one bounded virtual-memory map inventory.
pub fn memory_regions(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessMemoryRegion>, ProcessError> {
    validate_inspection(pid, max_visited)?;
    adapter::memory_regions(pid, max_visited)
}

/// Read one bounded native thread inventory.
pub fn threads(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessThreadInfo>, ProcessError> {
    validate_inspection(pid, max_visited)?;
    adapter::threads(pid, max_visited)
}

/// Read one bounded process-local socket inventory. Native source traversal is
/// bounded independently from product-side filtering and pagination.
pub fn sockets(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessSocketInfo>, ProcessError> {
    validate_inspection(pid, max_visited)?;
    adapter::sockets(pid, max_visited)
}

pub fn kill(pid: u32) -> Result<(), ProcessError> {
    crate::process_control::terminate(pid, crate::process_control::TerminationMode::Forceful)
        .map_err(|error| {
            use crate::process_control::ProcessControlErrorKind as ControlKind;
            let kind = match error.kind() {
                ControlKind::InvalidId => ProcessErrorKind::IdOutOfRange,
                ControlKind::IdOutOfRange => ProcessErrorKind::IdOutOfRange,
                ControlKind::Open => ProcessErrorKind::KillOpen,
                ControlKind::Terminate | ControlKind::Suspend | ControlKind::Resume => {
                    ProcessErrorKind::Kill
                }
                ControlKind::Unsupported => ProcessErrorKind::Unsupported,
            };
            ProcessError::new(kind, error.detail())
        })
}

pub fn configure_owned_command(command: &mut Command) -> Result<(), String> {
    adapter::configure_owned_command(command)
}

/// Backwards-compatible product-neutral verb used by Script Runtime.
pub fn configure_command(command: &mut Command) -> Result<(), String> {
    configure_owned_command(command)
}

pub fn stdout_probe_token(reader: &ChildStdout) -> Option<PipeProbeToken> {
    adapter::stdout_probe_token(reader)
}

pub fn stderr_probe_token(reader: &ChildStderr) -> Option<PipeProbeToken> {
    adapter::stderr_probe_token(reader)
}

pub fn pipe_available(token: PipeProbeToken) -> Result<usize, PipeProbeError> {
    adapter::pipe_available(token)
}

/// Write a launcher diagnostic to stderr or an already-existing parent
/// console. This never allocates a new console and reports best-effort success.
pub fn write_parent_console_stderr(message: &str) -> bool {
    crate::parent_console::write_stderr(message)
}

/// Write a CLI line (e.g. `--version`) to stdout or an attached parent console.
/// GUI-subsystem binaries on Windows attach to the parent terminal when present.
pub fn write_parent_console_stdout(message: &str) -> bool {
    crate::parent_console::write_stdout(message)
}

// Console attachment + std-handle duplication. The implementations (and
// their target `cfg`s) live in `selected::console_surface` per boundary
// policy; this contract file only re-exports the stable names.
pub use crate::selected::console_surface::{ScopedConsole, StdHandle, duplicated_std_handles};

#[cfg(test)]
mod tests {
    #[test]
    fn current_process_command_line_is_bounded_and_nonempty() {
        let line = super::command_line(std::process::id()).expect("current command line");
        assert!(!line.trim().is_empty());
        assert!(line.len() <= 1024 * 1024);
    }

    #[test]
    fn current_process_arguments_preserve_at_least_argv_zero() {
        let arguments = super::arguments(std::process::id()).expect("current arguments");
        assert_eq!(arguments, std::env::args().collect::<Vec<_>>());
        assert!(arguments.iter().map(String::len).sum::<usize>() <= 1024 * 1024);
    }

    #[test]
    fn zero_is_not_a_process_current_directory() {
        assert_eq!(
            super::current_directory(0).unwrap_err().kind(),
            super::ProcessErrorKind::IdOutOfRange
        );
    }

    #[test]
    fn zero_is_not_a_process_environment_target() {
        assert_eq!(
            super::environment_snapshot(0).unwrap_err().kind(),
            super::ProcessErrorKind::IdOutOfRange
        );
    }

    #[test]
    fn process_inspection_rejects_zero_and_unbounded_scans() {
        assert_eq!(
            super::file_descriptors(0, 1).unwrap_err().kind(),
            super::ProcessErrorKind::IdOutOfRange
        );
        assert_eq!(
            super::threads(std::process::id(), 10_001)
                .unwrap_err()
                .kind(),
            super::ProcessErrorKind::InventoryTooLarge
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn current_process_inspection_is_bounded() {
        let fds = super::file_descriptors(std::process::id(), 256).expect("current fds");
        assert!(fds.visited_count <= 256);
        let maps = super::memory_regions(std::process::id(), 10_000).expect("current maps");
        assert!(!maps.items.is_empty());
        let threads = super::threads(std::process::id(), 256).expect("current threads");
        assert!(!threads.items.is_empty());
        let sockets = super::sockets(std::process::id(), 256).expect("current sockets");
        assert!(sockets.visited_count <= 256);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn current_process_initial_environment_is_bounded_and_raw() {
        let snapshot = super::environment_snapshot(std::process::id())
            .expect("current process initial environment");
        assert!(snapshot.source_bytes <= super::PROCESS_ENVIRONMENT_MAX_BYTES);
        assert!(snapshot.entries.iter().any(|entry| {
            entry
                .bytes
                .iter()
                .position(|byte| *byte == b'=')
                .is_some_and(|equals| equals > 0)
        }));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn current_process_directory_matches_the_standard_library() {
        assert_eq!(
            super::current_directory(std::process::id()).expect("current process directory"),
            std::env::current_dir().expect("standard current directory")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_refuses_undocumented_remote_process_parameter_layouts() {
        assert_eq!(
            super::current_directory(std::process::id())
                .expect_err("Windows arbitrary-process cwd is unsupported")
                .kind(),
            super::ProcessErrorKind::Unsupported
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_refuses_undocumented_remote_environment_layouts() {
        assert_eq!(
            super::environment_snapshot(std::process::id())
                .expect_err("Windows arbitrary-process environment is unsupported")
                .kind(),
            super::ProcessErrorKind::Unsupported
        );
    }
}
