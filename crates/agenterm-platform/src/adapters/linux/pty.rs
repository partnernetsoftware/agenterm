use std::env;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::{Duration, Instant};

use libc::{self, c_int, pid_t};

use crate::contract::pty::{
    NativeInputOwnership, NativeTerminalKey, ProcessId, PtyCleanupReceipt, PtyError,
    PtyForegroundSignal, PtyForegroundSignalReceipt, PtyResult, TerminalSize,
};
use crate::process_control::TerminationMode;
use crate::process_reference::{ProcessReference, ProcessWait};

/// Return the native login-shell argument for a bare supported POSIX shell.
pub fn login_shell_argument(
    program: &std::path::Path,
    explicit_arguments: usize,
) -> Option<&'static str> {
    if explicit_arguments != 0 {
        return None;
    }
    program
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            matches!(
                *name,
                "bash" | "zsh" | "fish" | "sh" | "dash" | "ksh" | "tcsh" | "csh"
            )
        })
        .map(|_| "-l")
}

const PTY_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const PTY_EXEC_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const PTY_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const PTY_FOREGROUND_SIGNAL_TIMEOUT: Duration = Duration::from_secs(2);
const PTY_SESSION_MEMBER_LIMIT: usize = 4096;
const PTY_SESSION_FREEZE_ROUNDS: usize = 16;

/// A command configuration for spawning a process inside a newly allocated PTY.
#[derive(Clone, Debug)]
pub struct ChildCommand {
    program: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    current_dir: Option<PathBuf>,
    size: Option<TerminalSize>,
}

impl ChildCommand {
    #[must_use]
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            current_dir: None,
            size: None,
        }
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    #[must_use]
    pub fn current_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(path.into());
        self
    }

    #[must_use]
    pub fn size(mut self, size: TerminalSize) -> Self {
        self.size = Some(size);
        self
    }

    pub fn spawn(self) -> PtyResult<SpawnedPty> {
        spawn_child(self).map_err(|error| PtyError::failed("spawn", "pty_spawn_failed", error))
    }
}

/// A spawned process together with the PTY master used to communicate with it.
#[derive(Debug)]
pub struct SpawnedPty {
    master: PtyMaster,
    child: PtyChild,
}

impl SpawnedPty {
    #[must_use]
    pub fn into_parts(self) -> (PtyMaster, PtyChild) {
        (self.master, self.child)
    }
}

/// The I/O endpoint for a pseudoterminal master descriptor.
#[derive(Debug)]
pub struct PtyIo {
    fd: Arc<OwnedFd>,
}

impl PtyIo {
    pub fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        read_fd(self.fd.as_raw_fd(), buffer)
    }
}

/// The master handle of a pseudoterminal.
#[derive(Debug)]
pub struct PtyMaster {
    io: PtyIo,
}

impl PtyMaster {
    fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        set_nonblocking(fd.as_raw_fd())?;
        Ok(Self {
            io: PtyIo { fd: Arc::new(fd) },
        })
    }

    pub fn resize(&self, size: TerminalSize) -> PtyResult<()> {
        apply_winsize(self.io.fd.as_raw_fd(), size)
            .map_err(|error| PtyError::failed("resize", "pty_resize_failed", error))
    }

    pub fn try_clone(&self) -> PtyResult<Self> {
        let fd = dup_fd(self.io.fd.as_raw_fd())
            .map_err(|error| PtyError::failed("clone reader", "pty_reader_clone_failed", error))?;
        Self::from_fd(fd)
            .map_err(|error| PtyError::failed("clone reader", "pty_reader_clone_failed", error))
    }

    pub fn try_clone_for_startup_reader(&mut self) -> PtyResult<Self> {
        self.try_clone()
    }

    #[must_use]
    pub fn io(&self) -> &PtyIo {
        &self.io
    }

    pub fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        write_all_with_timeout(self.io.fd.as_raw_fd(), bytes, PTY_WRITE_TIMEOUT)
    }
}

/// A handle for signaling and reaping a PTY-backed child process.
pub struct PtyChild {
    pid: ProcessId,
    session_id: u32,
    root_reference: Option<ProcessReference>,
}

impl std::fmt::Debug for PtyChild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PtyChild")
            .field("pid", &self.pid)
            .field("session_id", &self.session_id)
            .field("owns_cleanup_authority", &self.root_reference.is_some())
            .finish()
    }
}

impl PtyChild {
    #[must_use]
    pub fn pid(&self) -> ProcessId {
        self.pid
    }

    pub fn wait(&mut self) -> PtyResult<ExitStatus> {
        let mut status: c_int = 0;
        loop {
            let result = unsafe { libc::waitpid(self.pid.as_u32() as pid_t, &mut status, 0) };
            if result == -1 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(PtyError::failed("wait", "pty_wait_failed", error));
            }
            if result == self.pid.as_u32() as pid_t {
                return Ok(ExitStatus::from_raw(status));
            }
        }
    }

    pub fn try_clone_for_wait(&self) -> PtyResult<Self> {
        Ok(Self {
            pid: self.pid,
            session_id: self.session_id,
            root_reference: None,
        })
    }

    pub fn close_pseudoconsole(&self) {}

    pub fn terminate_forcefully(&self) -> PtyResult<PtyCleanupReceipt> {
        let root = self.root_reference.as_ref().ok_or_else(|| {
            PtyError::failed(
                "terminate owned session",
                "pty_terminate_without_authority",
                "the wait-only PTY child clone has no cleanup authority",
            )
        })?;
        terminate_owned_session(root, self.session_id)
    }

    /// Delivers one signal to the native foreground process group selected by
    /// this retained PTY master. Process inventory is used only for bounded
    /// post-state evidence; `tcgetpgrp(master)` is the effect authority.
    pub fn signal_foreground(
        &self,
        master: &PtyMaster,
        signal: PtyForegroundSignal,
    ) -> PtyResult<PtyForegroundSignalReceipt> {
        let root = self.root_reference.as_ref().ok_or_else(|| {
            PtyError::failed(
                "signal foreground process group",
                "pty_signal_without_authority",
                "the wait-only PTY child clone has no foreground-signal authority",
            )
        })?;
        signal_foreground_process_group(root, self.session_id, master.io.fd.as_raw_fd(), signal)
    }

    pub fn send_native_key(&self, _key: NativeTerminalKey, _repeat_count: u16) -> PtyResult<()> {
        Err(PtyError::unsupported(
            "send native key",
            "the POSIX PTY adapter has no native console key-event transport",
        ))
    }

    pub fn native_input_ownership(&self) -> PtyResult<NativeInputOwnership> {
        Err(PtyError::unsupported(
            "inspect native input ownership",
            "the POSIX PTY adapter has no Win32 console input mode",
        ))
    }
}

fn signal_foreground_process_group(
    root: &ProcessReference,
    session_id: u32,
    master_fd: RawFd,
    signal: PtyForegroundSignal,
) -> PtyResult<PtyForegroundSignalReceipt> {
    if !root.is_alive().map_err(|error| {
        PtyError::failed("inspect PTY owner", "pty_signal_owner_query_failed", error)
    })? {
        return Err(PtyError::failed(
            "signal foreground process group",
            "pty_signal_owner_exited",
            "the retained PTY owner has already exited",
        ));
    }
    let foreground = unsafe { libc::tcgetpgrp(master_fd) };
    if foreground <= 0 {
        return Err(PtyError::failed(
            "resolve foreground process group",
            "pty_foreground_group_unavailable",
            io::Error::last_os_error(),
        ));
    }
    if native_session_id(foreground as u32).map_err(|error| {
        PtyError::failed(
            "validate foreground process group",
            "pty_foreground_group_query_failed",
            error,
        )
    })? != Some(session_id)
    {
        return Err(PtyError::failed(
            "validate foreground process group",
            "pty_foreground_group_changed",
            "the foreground process group does not belong to the retained PTY session",
        ));
    }

    let mut members = Vec::new();
    for pid in session_process_ids(session_id).map_err(|error| {
        PtyError::failed(
            "enumerate foreground process group",
            "pty_foreground_group_enumeration_failed",
            error,
        )
    })? {
        let group = unsafe { libc::getpgid(pid as pid_t) };
        if group == foreground {
            match ProcessReference::open(pid) {
                Ok(reference) => members.push(reference),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(PtyError::failed(
                        "retain foreground process member",
                        "pty_foreground_member_retain_failed",
                        error,
                    ));
                }
            }
        } else if group == -1 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::NotFound && error.raw_os_error() != Some(libc::ESRCH)
            {
                return Err(PtyError::failed(
                    "inspect foreground process member",
                    "pty_foreground_member_query_failed",
                    error,
                ));
            }
        }
    }
    if members.is_empty() {
        return Err(PtyError::failed(
            "signal foreground process group",
            "pty_foreground_group_empty",
            "the retained PTY foreground process group had no observable live members",
        ));
    }
    if unsafe { libc::tcgetpgrp(master_fd) } != foreground {
        return Err(PtyError::failed(
            "signal foreground process group",
            "pty_foreground_group_changed",
            "the PTY foreground process group changed before signal delivery",
        ));
    }

    let native_signal = match signal {
        PtyForegroundSignal::Interrupt => libc::SIGINT,
        PtyForegroundSignal::Terminate => libc::SIGTERM,
        PtyForegroundSignal::Stop => libc::SIGSTOP,
        PtyForegroundSignal::Continue => libc::SIGCONT,
    };
    if unsafe { libc::killpg(foreground, native_signal) } == -1 {
        return Err(PtyError::failed(
            "signal foreground process group",
            "pty_foreground_signal_failed",
            io::Error::last_os_error(),
        ));
    }

    let observed = members.len().try_into().unwrap_or(u32::MAX);
    if signal == PtyForegroundSignal::Interrupt {
        return Ok(PtyForegroundSignalReceipt {
            containment: "posix-foreground-process-group",
            signal: signal.as_str(),
            members_observed: observed,
            members_retained_for_verification: observed,
            delivered: true,
            verified: false,
            postcondition: "application-acknowledgement-required",
        });
    }

    let deadline = Instant::now() + PTY_FOREGROUND_SIGNAL_TIMEOUT;
    loop {
        let mut all_verified = true;
        for member in &members {
            let alive = member.is_alive().map_err(|error| {
                PtyError::failed(
                    "verify foreground process member",
                    "pty_foreground_signal_verify_failed",
                    error,
                )
            })?;
            let matches = match signal {
                PtyForegroundSignal::Terminate => !alive,
                PtyForegroundSignal::Stop if alive => {
                    let stopped =
                        crate::process_metrics::is_stopped(member.id()).map_err(|error| {
                            PtyError::failed(
                                "verify foreground process member",
                                "pty_foreground_signal_verify_failed",
                                error,
                            )
                        })?;
                    stopped
                        && member.is_alive().map_err(|error| {
                            PtyError::failed(
                                "revalidate foreground process member",
                                "pty_foreground_signal_verify_failed",
                                error,
                            )
                        })?
                }
                PtyForegroundSignal::Continue if alive => {
                    let stopped =
                        crate::process_metrics::is_stopped(member.id()).map_err(|error| {
                            PtyError::failed(
                                "verify foreground process member",
                                "pty_foreground_signal_verify_failed",
                                error,
                            )
                        })?;
                    !stopped
                        && member.is_alive().map_err(|error| {
                            PtyError::failed(
                                "revalidate foreground process member",
                                "pty_foreground_signal_verify_failed",
                                error,
                            )
                        })?
                }
                PtyForegroundSignal::Stop | PtyForegroundSignal::Continue => false,
                PtyForegroundSignal::Interrupt => unreachable!(),
            };
            all_verified &= matches;
        }
        if all_verified {
            let postcondition = match signal {
                PtyForegroundSignal::Terminate => "exited",
                PtyForegroundSignal::Stop => "stopped",
                PtyForegroundSignal::Continue => "running",
                PtyForegroundSignal::Interrupt => unreachable!(),
            };
            return Ok(PtyForegroundSignalReceipt {
                containment: "posix-foreground-process-group",
                signal: signal.as_str(),
                members_observed: observed,
                members_retained_for_verification: observed,
                delivered: true,
                verified: true,
                postcondition,
            });
        }
        if Instant::now() >= deadline {
            return Ok(PtyForegroundSignalReceipt {
                containment: "posix-foreground-process-group",
                signal: signal.as_str(),
                members_observed: observed,
                members_retained_for_verification: observed,
                delivered: true,
                verified: false,
                postcondition: "deadline-before-observable-postcondition",
            });
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Resolve `command.program` the way a POSIX shell would, failing with
/// `NotFound` when no executable exists. Absolute paths and paths with a
/// directory component are not looked up on `PATH`.
fn resolve_posix_executable(command: &ChildCommand) -> io::Result<PathBuf> {
    let program = &command.program;
    if program.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "PTY executable path is empty",
        ));
    }
    if program.is_absolute() || program.components().count() > 1 {
        let candidate = if program.is_absolute() {
            program.clone()
        } else {
            let base = command
                .current_dir
                .clone()
                .or_else(|| env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            base.join(program)
        };
        return if is_executable_file(&candidate) {
            Ok(candidate)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("PTY executable not found: {}", candidate.to_string_lossy()),
            ))
        };
    }

    let path_value = command
        .env
        .iter()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.clone())
        .or_else(|| env::var_os("PATH"));
    let Some(path_value) = path_value else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "PTY executable not found on PATH: {}",
                program.to_string_lossy()
            ),
        ));
    };
    for directory in env::split_paths(&path_value) {
        let candidate = directory.join(program);
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "PTY executable not found on PATH: {}",
            program.to_string_lossy()
        ),
    ))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

fn spawn_child(command: ChildCommand) -> io::Result<SpawnedPty> {
    // Fail before fork when the program is missing — matches Windows ConPTY
    // "executable not found" and keeps agenterm-con `-e bad` exit non-zero on
    // macOS/Linux instead of opening a host that only discovers exit 127 later.
    let resolved_program = resolve_posix_executable(&command)?;
    let (master_fd, slave_fd) = open_pty_pair(command.size)?;
    let master = PtyMaster::from_fd(master_fd)?;

    let program = CString::new(resolved_program.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "program path contains NUL"))?;
    let args = build_argv(&program, &command.args)?;
    let env_pairs = build_env_pairs(&command.env)?;
    let current_dir = command
        .current_dir
        .as_ref()
        .map(|directory| CString::new(directory.as_os_str().as_bytes()))
        .transpose()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "working directory contains NUL",
            )
        })?;
    // Built pre-fork: a multithreaded process only clones the calling thread on
    // fork(), so the child may only call async-signal-safe functions until it
    // execve()s. The Rust allocator is not on that list, so every pointer the
    // child needs (argv, envp, cwd) must already exist before fork() runs.
    let argv_ptrs = build_ptr_array(&args);
    let envp_ptrs = build_ptr_array(&env_pairs);
    let (exec_status_read, exec_status_write) = cloexec_pipe()?;

    let child_pid = unsafe { libc::fork() };
    if child_pid == -1 {
        return Err(io::Error::last_os_error());
    }

    if child_pid == 0 {
        drop(exec_status_read);
        let master_raw = master.io.fd.as_raw_fd();
        if let Err(error) = child_setup(
            master_raw,
            slave_fd.as_raw_fd(),
            current_dir.as_deref(),
            &program,
            &argv_ptrs,
            &envp_ptrs,
        ) {
            let native_error = error.raw_os_error().unwrap_or(libc::EIO);
            unsafe {
                libc::write(
                    exec_status_write.as_raw_fd(),
                    std::ptr::from_ref(&native_error).cast(),
                    std::mem::size_of::<c_int>(),
                );
            }
            let message = b"pty child setup failed\n";
            unsafe {
                libc::write(
                    libc::STDERR_FILENO,
                    message.as_ptr() as *const libc::c_void,
                    message.len(),
                );
            }
            unsafe {
                libc::_exit(127);
            }
        }
        unreachable!("exec replaces the child process image");
    }

    drop(exec_status_write);
    drop(slave_fd);

    let pid = ProcessId::new(child_pid as u32).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("child process returned an invalid pid: {error}"),
        )
    })?;

    if let Err(error) = await_exec_handshake(&exec_status_read) {
        unsafe {
            libc::kill(child_pid, libc::SIGKILL);
            libc::waitpid(child_pid, std::ptr::null_mut(), 0);
        }
        return Err(error);
    }

    let root_reference = match ProcessReference::open(pid.as_u32()) {
        Ok(reference) => reference,
        Err(error) => {
            unsafe {
                libc::kill(-(pid.as_u32() as pid_t), libc::SIGKILL);
                libc::kill(pid.as_u32() as pid_t, libc::SIGKILL);
            }
            loop {
                let waited =
                    unsafe { libc::waitpid(pid.as_u32() as pid_t, std::ptr::null_mut(), 0) };
                if waited >= 0 {
                    break;
                }
                let wait_error = io::Error::last_os_error();
                if wait_error.kind() != io::ErrorKind::Interrupted {
                    break;
                }
            }
            return Err(io::Error::new(
                error.kind(),
                format!("retain PTY cleanup authority: {error}"),
            ));
        }
    };

    Ok(SpawnedPty {
        master,
        child: PtyChild {
            pid,
            session_id: pid.as_u32(),
            root_reference: Some(root_reference),
        },
    })
}

fn cloexec_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut raw = [-1; 2];
    if unsafe { libc::pipe(raw.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let read = unsafe { OwnedFd::from_raw_fd(raw[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(raw[1]) };
    for descriptor in [&read, &write] {
        let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) };
        if flags == -1
            || unsafe {
                libc::fcntl(
                    descriptor.as_raw_fd(),
                    libc::F_SETFD,
                    flags | libc::FD_CLOEXEC,
                )
            } == -1
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read, write))
}

fn await_exec_handshake(status: &OwnedFd) -> io::Result<()> {
    let mut poll = libc::pollfd {
        fd: status.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    };
    let deadline = Instant::now() + PTY_EXEC_HANDSHAKE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "PTY child did not complete exec handshake",
            ));
        }
        let timeout = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
        let ready = unsafe { libc::poll(&raw mut poll, 1, timeout) };
        if ready == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "PTY child did not complete exec handshake",
            ));
        }
        let mut native_error = 0_i32;
        let count = unsafe {
            libc::read(
                status.as_raw_fd(),
                std::ptr::from_mut(&mut native_error).cast(),
                std::mem::size_of::<c_int>(),
            )
        };
        return match count {
            0 => Ok(()),
            value if value == std::mem::size_of::<c_int>() as isize => {
                Err(io::Error::from_raw_os_error(native_error))
            }
            -1 => Err(io::Error::last_os_error()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PTY child returned a truncated exec failure",
            )),
        };
    }
}

/// Runs between `fork()` and `execve()`. Every argument is pre-built by the
/// parent so this function performs no heap allocation: POSIX only guarantees
/// async-signal-safe functions are safe to call here in a process that had
/// more than one thread at fork time (see fork(2)), and the Rust allocator
/// gives no such guarantee.
fn child_setup(
    master_fd: RawFd,
    slave_fd: RawFd,
    current_dir: Option<&CStr>,
    program: &CStr,
    argv: &[*const libc::c_char],
    envp: &[*const libc::c_char],
) -> io::Result<()> {
    unsafe {
        if libc::close(master_fd) == -1 {
            return Err(io::Error::last_os_error());
        }

        if libc::dup2(slave_fd, libc::STDIN_FILENO) == -1
            || libc::dup2(slave_fd, libc::STDOUT_FILENO) == -1
            || libc::dup2(slave_fd, libc::STDERR_FILENO) == -1
        {
            return Err(io::Error::last_os_error());
        }

        if slave_fd > libc::STDERR_FILENO {
            libc::close(slave_fd);
        }

        if libc::setsid() == -1 {
            return Err(io::Error::last_os_error());
        }

        // Let the selected libc declaration infer the request type: Linux GNU
        // uses c_ulong, Linux musl uses c_int, and the BSD/macOS declaration
        // uses c_ulong without exporting Linux's `Ioctl` alias. This source is
        // shared by both Unix adapters, so naming either platform typedef here
        // makes another supported target fail at compile time.
        if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) == -1 {
            return Err(io::Error::last_os_error());
        }

        let child_pid = libc::getpid();
        if libc::tcsetpgrp(libc::STDIN_FILENO, child_pid) == -1 {
            return Err(io::Error::last_os_error());
        }

        if let Some(directory) = current_dir
            && libc::chdir(directory.as_ptr()) == -1
        {
            return Err(io::Error::last_os_error());
        }

        let result = libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

struct FrozenMember {
    reference: ProcessReference,
}

fn terminate_owned_session(
    root: &ProcessReference,
    session_id: u32,
) -> PtyResult<PtyCleanupReceipt> {
    let deadline = Instant::now() + PTY_CLEANUP_TIMEOUT;
    let mut members = Vec::<FrozenMember>::new();
    let mut known = std::collections::BTreeSet::<u32>::new();
    let mut observed = 0_u32;
    let mut stabilized = false;

    for _ in 0..PTY_SESSION_FREEZE_ROUNDS {
        let ids = session_process_ids(session_id).map_err(|error| {
            resume_frozen(&members);
            PtyError::failed(
                "enumerate owned session",
                "pty_session_enumeration_failed",
                error,
            )
        })?;
        observed = observed.max(ids.len().try_into().unwrap_or(u32::MAX));
        let mut added = false;
        for pid in ids {
            if !known.insert(pid) {
                continue;
            }
            if pid == root.id() && !root.is_alive().unwrap_or(false) {
                continue;
            }
            let Some(reference) =
                retain_and_freeze_member(pid, session_id, deadline).map_err(|error| {
                    resume_frozen(&members);
                    PtyError::failed(
                        "freeze owned session member",
                        "pty_session_freeze_failed",
                        error,
                    )
                })?
            else {
                continue;
            };
            if pid == root.id() && !root.is_alive().unwrap_or(false) {
                let _ = reference.set_suspended(false);
                continue;
            }
            members.push(FrozenMember { reference });
            added = true;
        }
        if !added {
            stabilized = true;
            break;
        }
        if Instant::now() >= deadline {
            resume_frozen(&members);
            return Err(PtyError::failed(
                "freeze owned session",
                "pty_session_freeze_timeout",
                "session membership did not stabilize before the cleanup deadline",
            ));
        }
    }
    if !stabilized {
        resume_frozen(&members);
        return Err(PtyError::failed(
            "freeze owned session",
            "pty_session_membership_unstable",
            "session membership kept changing through the bounded freeze rounds",
        ));
    }

    // A POSIX session leader exiting can orphan stopped process groups, which
    // makes the kernel deliver SIGHUP/SIGCONT. Keep it frozen and kill it last
    // so descendants cannot resume and exec between identity validation and
    // their terminal signal.
    members.sort_by_key(|member| member.reference.id() == root.id());
    let mut terminated = 0_u32;
    for member in &mut members {
        match terminate_frozen_member(member, session_id, deadline) {
            Ok(true) => terminated = terminated.saturating_add(1),
            Ok(false) => {}
            Err(error) => {
                resume_frozen(&members);
                return Err(PtyError::failed(
                    "terminate owned session member",
                    "pty_session_terminate_failed",
                    error,
                ));
            }
        }
    }
    loop {
        let mut live = false;
        for member in &members {
            let member_live = member
                .reference
                .wait_for_exit(Some(Duration::ZERO))
                .map_err(|error| {
                    resume_frozen(&members);
                    PtyError::failed(
                        "verify owned session cleanup",
                        "pty_session_verify_failed",
                        error,
                    )
                })?
                == ProcessWait::TimedOut;
            live |= member_live;
        }
        if !live {
            return Ok(PtyCleanupReceipt {
                containment: "posix-session",
                members_observed: observed,
                members_terminated: terminated,
                verified_empty: true,
            });
        }
        if Instant::now() >= deadline {
            resume_frozen(&members);
            return Err(PtyError::failed(
                "verify owned session cleanup",
                "pty_session_cleanup_incomplete",
                "one or more exact session members remained live at the cleanup deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn terminate_frozen_member(
    member: &mut FrozenMember,
    session_id: u32,
    deadline: Instant,
) -> io::Result<bool> {
    loop {
        if !member.reference.is_alive()? {
            return Ok(false);
        }
        match member.reference.terminate(TerminationMode::Forceful) {
            Ok(()) => return Ok(true),
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                if !member.reference.is_alive()? {
                    return Ok(false);
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "exact process identity kept changing before termination",
                    ));
                }
                let Some(replacement) =
                    retain_and_freeze_member(member.reference.id(), session_id, deadline)?
                else {
                    return Ok(false);
                };
                member.reference = replacement;
            }
            Err(error) => return Err(error),
        }
    }
}

fn retain_and_freeze_member(
    pid: u32,
    session_id: u32,
    deadline: Instant,
) -> io::Result<Option<ProcessReference>> {
    let mut reference = match ProcessReference::open_for_termination(pid) {
        Ok(reference) => reference,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    loop {
        match native_session_id(pid)? {
            Some(actual) if actual == session_id => {}
            None | Some(_) => return Ok(None),
        }
        match reference.set_suspended(true) {
            Ok(()) => return Ok(Some(reference)),
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                if !reference.is_alive()? {
                    return Ok(None);
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "exact process identity kept changing before session freeze",
                    ));
                }
                let replacement = ProcessReference::open_for_termination(pid)?;
                if !reference.is_alive()? {
                    return Ok(None);
                }
                reference = replacement;
                std::thread::yield_now();
            }
            Err(error) => return Err(error),
        }
    }
}

fn resume_frozen(members: &[FrozenMember]) {
    for member in members {
        let _ = member.reference.set_suspended(false);
    }
}

fn native_session_id(pid: u32) -> io::Result<Option<u32>> {
    let result = unsafe { libc::getsid(pid as pid_t) };
    if result >= 0 {
        return Ok(Some(result as u32));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(error)
    }
}

#[cfg(target_os = "linux")]
fn session_process_ids(session_id: u32) -> io::Result<Vec<u32>> {
    let mut ids = Vec::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        if native_session_id(pid)? == Some(session_id) {
            ids.push(pid);
            if ids.len() > PTY_SESSION_MEMBER_LIMIT {
                return Err(io::Error::other("PTY session member limit exceeded"));
            }
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

#[cfg(target_os = "macos")]
fn session_process_ids(session_id: u32) -> io::Result<Vec<u32>> {
    const PROC_ALL_PIDS: u32 = 1;
    unsafe extern "C" {
        fn proc_listpids(
            kind: u32,
            typeinfo: u32,
            buffer: *mut libc::c_void,
            buffersize: i32,
        ) -> i32;
    }
    let required = unsafe { proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
    if required < 0 {
        return Err(io::Error::last_os_error());
    }
    const PROCESS_INVENTORY_LIMIT: usize = 131_072;
    let mut capacity = (required as usize / std::mem::size_of::<i32>())
        .saturating_add(64)
        .min(PROCESS_INVENTORY_LIMIT);
    let (raw, written) = loop {
        let mut raw = vec![0_i32; capacity];
        let bytes = i32::try_from(raw.len().saturating_mul(std::mem::size_of::<i32>()))
            .map_err(|_| io::Error::other("process list buffer exceeds i32"))?;
        let written = unsafe { proc_listpids(PROC_ALL_PIDS, 0, raw.as_mut_ptr().cast(), bytes) };
        if written < 0 {
            return Err(io::Error::last_os_error());
        }
        if written < bytes {
            break (raw, written);
        }
        if capacity >= PROCESS_INVENTORY_LIMIT {
            return Err(io::Error::other(
                "system process inventory exceeded its bound",
            ));
        }
        capacity = capacity.saturating_mul(2).min(PROCESS_INVENTORY_LIMIT);
    };
    let mut ids = Vec::new();
    for pid in raw
        .into_iter()
        .take(written as usize / std::mem::size_of::<i32>())
    {
        let Ok(pid) = u32::try_from(pid) else {
            continue;
        };
        if native_session_id(pid)? == Some(session_id) {
            ids.push(pid);
            if ids.len() > PTY_SESSION_MEMBER_LIMIT {
                return Err(io::Error::other("PTY session member limit exceeded"));
            }
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

/// Builds a null-terminated argv/envp-style pointer array. Must be called
/// before `fork()`; see [`child_setup`].
fn build_ptr_array(entries: &[CString]) -> Vec<*const libc::c_char> {
    entries
        .iter()
        .map(|entry| entry.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect()
}

fn build_argv(program: &CStr, args: &[OsString]) -> io::Result<Vec<CString>> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(
        CString::new(program.to_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "argv0 contains NUL"))?,
    );
    for arg in args {
        argv.push(
            CString::new(arg.as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "argument contains NUL")
            })?,
        );
    }
    Ok(argv)
}

fn build_env_pairs(overrides: &[(OsString, OsString)]) -> io::Result<Vec<CString>> {
    let mut entries: Vec<CString> = std::env::vars_os()
        .map(|(key, value)| env_entry_to_cstring(&key, &value))
        .collect::<io::Result<_>>()?;

    for (key, value) in overrides {
        let encoded = env_entry_to_cstring(key, value)?;
        if let Some(existing) = entries.iter_mut().find(|entry| {
            entry
                .to_bytes()
                .split(|byte| *byte == b'=')
                .next()
                .is_some_and(|name| name == key.as_bytes())
        }) {
            *existing = encoded;
        } else {
            entries.push(encoded);
        }
    }

    Ok(entries)
}

fn env_entry_to_cstring(key: &OsStr, value: &OsStr) -> io::Result<CString> {
    let mut bytes = Vec::with_capacity(key.len() + value.len() + 1);
    bytes.extend_from_slice(key.as_bytes());
    bytes.push(b'=');
    bytes.extend_from_slice(value.as_bytes());
    CString::new(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "environment value contains NUL",
        )
    })
}

fn open_pty_pair(size: Option<TerminalSize>) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master: c_int = 0;
    let mut slave: c_int = 0;
    let mut winsize = size.map(into_winsize);
    let winsize_ptr = winsize
        .as_mut()
        .map_or(std::ptr::null_mut(), |value| value as *mut libc::winsize);

    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            winsize_ptr,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }

    let master_fd = unsafe { OwnedFd::from_raw_fd(master) };
    let slave_fd = unsafe { OwnedFd::from_raw_fd(slave) };
    set_cloexec(master_fd.as_raw_fd())?;
    set_cloexec(slave_fd.as_raw_fd())?;
    Ok((master_fd, slave_fd))
}

fn into_winsize(size: TerminalSize) -> libc::winsize {
    libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn apply_winsize(fd: RawFd, size: TerminalSize) -> io::Result<()> {
    let winsize = into_winsize(size);
    let result = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &winsize) };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn dup_fd(fd: RawFd) -> io::Result<OwnedFd> {
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated == -1 {
        return Err(io::Error::last_os_error());
    }
    let owned = unsafe { OwnedFd::from_raw_fd(duplicated) };
    set_cloexec(owned.as_raw_fd())?;
    Ok(owned)
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read_fd(fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        let result =
            unsafe { libc::read(fd, buffer.as_mut_ptr().cast::<libc::c_void>(), buffer.len()) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                wait_until_readable(fd)?;
                continue;
            }
            return Err(error);
        }
        return Ok(result as usize);
    }
}

fn write_all_with_timeout(fd: RawFd, mut buffer: &[u8], timeout: Duration) -> io::Result<()> {
    let started = std::time::Instant::now();
    while !buffer.is_empty() {
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("pty write made no progress for {} ms", timeout.as_millis()),
            ));
        }
        let result =
            unsafe { libc::write(fd, buffer.as_ptr().cast::<libc::c_void>(), buffer.len()) };
        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                wait_until_writable(fd, started, timeout)?;
                continue;
            }
            return Err(error);
        }
        if result == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0"));
        }
        buffer = &buffer[result as usize..];
    }
    Ok(())
}

fn wait_until_readable(fd: RawFd) -> io::Result<()> {
    loop {
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll_fd, 1, -1) };
        if ready > 0 {
            return Ok(());
        }
        if ready == 0 {
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error);
    }
}

fn wait_until_writable(
    fd: RawFd,
    started: std::time::Instant,
    timeout: Duration,
) -> io::Result<()> {
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("pty write made no progress for {} ms", timeout.as_millis()),
            ));
        }
        let millis = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll_fd, 1, millis) };
        if ready > 0 {
            if poll_fd.revents & libc::POLLOUT != 0 {
                return Ok(());
            }
            if poll_fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "pty is no longer writable",
                ));
            }
            continue;
        }
        if ready == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("pty write made no progress for {} ms", timeout.as_millis()),
            ));
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct FixtureCleanup(u32);

    impl Drop for FixtureCleanup {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-(self.0 as pid_t), libc::SIGKILL);
                libc::kill(self.0 as pid_t, libc::SIGKILL);
            }
        }
    }

    fn read_until(reader: &PtyMaster, needle: &[u8]) -> Vec<u8> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        let mut buffer = [0_u8; 512];
        while !output.windows(needle.len()).any(|window| window == needle) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out reading PTY output: {:?}",
                String::from_utf8_lossy(&output)
            );
            match reader.io().read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => output.extend_from_slice(&buffer[..size]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("PTY read failed: {error}"),
            }
        }
        output
    }

    #[test]
    fn openpty_spawn_write_read_round_trip() {
        let spawned = ChildCommand::new("/bin/sh")
            .arg("-c")
            .arg("echo marker")
            .size(TerminalSize { rows: 24, cols: 80 })
            .spawn()
            .expect("spawn shell in pty");

        let (mut master, mut child) = spawned.into_parts();
        let reader = master
            .try_clone_for_startup_reader()
            .expect("clone pty reader");

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut output = Vec::new();
        let mut buffer = [0_u8; 256];
        while output.len() < b"marker".len() && std::time::Instant::now() < deadline {
            match reader.io().read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => output.extend_from_slice(&buffer[..size]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("pty read failed: {error}"),
            }
        }

        child.terminate_forcefully().ok();
        let _ = child.wait();

        assert!(
            output
                .windows(b"marker".len())
                .any(|window| window == b"marker"),
            "expected marker in pty output, got {:?}",
            String::from_utf8_lossy(&output)
        );
    }

    #[test]
    fn forced_cleanup_empties_the_owned_session_with_resistant_children() {
        let spawned = ChildCommand::new("/bin/sh")
            .arg("-c")
            .arg(
                r#"trap '' HUP TERM INT
/bin/sh -c 'trap "" HUP TERM INT; while :; do sleep 60; done' & a=$!
/bin/sh -c 'trap "" HUP TERM INT; while :; do sleep 60; done' & b=$!
printf 'READY %s %s %s\n' "$$" "$a" "$b"
wait "$b""#,
            )
            .spawn()
            .expect("spawn resistant PTY fixture");
        let (master, mut child) = spawned.into_parts();
        let cleanup = FixtureCleanup(child.pid().as_u32());
        let output = read_until(&master, b"READY ");
        let line = String::from_utf8_lossy(&output)
            .lines()
            .find(|line| line.contains("READY "))
            .expect("fixture identity line")
            .trim_end_matches('\r')
            .to_owned();
        let ids = line
            .split_whitespace()
            .skip_while(|part| *part != "READY")
            .skip(1)
            .take(3)
            .map(|part| part.parse::<u32>().expect("numeric fixture PID"))
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 3, "fixture emitted root and two child PIDs");
        let children = ids[1..]
            .iter()
            .map(|pid| ProcessReference::open(*pid).expect("retain child observation"))
            .collect::<Vec<_>>();

        let receipt = child
            .terminate_forcefully()
            .expect("empty exact owned PTY session");

        assert_eq!(receipt.containment, "posix-session");
        assert!(receipt.verified_empty);
        assert!(receipt.members_observed >= 3);
        for child in children {
            assert_eq!(
                child
                    .wait_for_exit(Some(Duration::from_secs(2)))
                    .expect("observe child exit"),
                ProcessWait::Exited
            );
        }
        child.wait().expect("reap terminated PTY root");
        drop(cleanup);
    }

    #[test]
    fn retained_master_signals_and_verifies_its_foreground_group() {
        let spawned = ChildCommand::new("/bin/sh")
            .arg("-c")
            .arg("printf 'FOREGROUND_READY\\n'; while :; do sleep 60; done")
            .spawn()
            .expect("spawn foreground PTY fixture");
        let (master, mut child) = spawned.into_parts();
        let cleanup = FixtureCleanup(child.pid().as_u32());
        read_until(&master, b"FOREGROUND_READY");

        let stopped = child
            .signal_foreground(&master, PtyForegroundSignal::Stop)
            .expect("stop exact foreground group");
        assert_eq!(stopped.signal, "stop");
        assert!(stopped.delivered && stopped.verified);
        assert_eq!(stopped.postcondition, "stopped");

        let continued = child
            .signal_foreground(&master, PtyForegroundSignal::Continue)
            .expect("continue exact foreground group");
        assert_eq!(continued.signal, "continue");
        assert!(continued.delivered && continued.verified);
        assert_eq!(continued.postcondition, "running");

        let terminated = child
            .signal_foreground(&master, PtyForegroundSignal::Terminate)
            .expect("terminate exact foreground group");
        assert_eq!(terminated.signal, "terminate");
        assert!(terminated.delivered && terminated.verified);
        assert_eq!(terminated.postcondition, "exited");
        child.wait().expect("reap terminated foreground root");
        drop(cleanup);
    }

    #[test]
    fn native_console_key_injection_is_explicitly_unsupported() {
        let child = PtyChild {
            pid: ProcessId::new(1).expect("valid fixture pid"),
            session_id: 1,
            root_reference: None,
        };

        let error = child
            .send_native_key(NativeTerminalKey::Up, 3)
            .expect_err("POSIX PTYs do not expose Win32 console key events");

        assert!(matches!(error, PtyError::Unsupported { .. }));
        assert!(error.to_string().contains("send native key unsupported"));
    }

    #[test]
    fn native_input_ownership_is_explicitly_unsupported() {
        let child = PtyChild {
            pid: ProcessId::new(1).expect("valid fixture pid"),
            session_id: 1,
            root_reference: None,
        };

        let error = child
            .native_input_ownership()
            .expect_err("POSIX PTYs do not expose Win32 console input modes");

        assert!(matches!(error, PtyError::Unsupported { .. }));
        assert!(
            error
                .to_string()
                .contains("inspect native input ownership unsupported")
        );
    }
}
