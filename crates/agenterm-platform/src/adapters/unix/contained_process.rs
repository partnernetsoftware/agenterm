use std::{
    fs::File,
    io::{self, Read, Write},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    contained_process::{
        ContainedHeadlessCommand, ContainedInput, ContainedOutput, ContainedProcessLimits,
        FrozenProcessIdentity,
    },
    contract::process_spawn::ProcessExit,
    process::{ProcessTreeGuard, configure_owned_command},
};

pub struct ContainedChild {
    root: Root,
    tree: ProcessTreeGuard,
    stdin: Option<ContainedChildInput>,
    /// Parent-side capture streams for a `posix_spawn`ed root. A
    /// `std::process::Child` keeps its own, so these stay `None` there.
    stdout: Option<ContainedChildOutput>,
    stderr: Option<ContainedChildOutput>,
}

/// The native owner of the contained root.
///
/// A `Command`-spawned root is owned by `std::process::Child`; a
/// `posix_spawn`ed frozen root exists only as a pid we must `waitpid` ourselves.
/// Both carry the same containment guard and the same public contract.
enum Root {
    Command(Child),
    /// Only the macOS frozen launch constructs this variant (it is the only
    /// platform with a suspended `posix_spawn`). It stays here so a
    /// `Command`-spawned root and a frozen root share one containment contract
    /// instead of forking the type; on Linux it is intentionally unused.
    ///
    /// `exit` caches the reaped status exactly like `std::process::Child` does,
    /// so a second `try_wait` after a kill/wait is a replay rather than an
    /// `ECHILD` error. Without it `process.kill` followed by `process.wait`
    /// would break only on the frozen root, a silent behavioural divergence.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Raw {
        pid: u32,
        exit: Option<ProcessExit>,
    },
}

impl Root {
    fn id(&self) -> u32 {
        match self {
            Self::Command(child) => child.id(),
            Self::Raw { pid, .. } => *pid,
        }
    }

    fn try_wait(&mut self) -> io::Result<Option<ProcessExit>> {
        match self {
            Self::Command(child) => child.try_wait().map(|status| {
                status
                    .as_ref()
                    .map(crate::process_spawn::classify_exit_status)
            }),
            Self::Raw { pid, exit } => {
                if let Some(reaped) = exit {
                    return Ok(Some(*reaped));
                }
                let status = raw_try_wait(*pid)?;
                if let Some(reaped) = status {
                    *exit = Some(reaped);
                }
                Ok(status)
            }
        }
    }

    /// Signal the root to terminate.
    ///
    /// A root that already exited is not an error: `Command` reports a status
    /// that was already observed as `InvalidInput`, and a raw root reports
    /// `ESRCH` when it exited between the last observation and this signal. Both
    /// are "nothing left to signal", so the caller's cleanup continues.
    fn kill(&mut self) -> io::Result<()> {
        match self {
            Self::Command(child) => match child.kill() {
                Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(()),
                other => other,
            },
            Self::Raw { pid, exit } => {
                if exit.is_some() {
                    return Ok(());
                }
                let native = libc::pid_t::try_from(*pid)
                    .map_err(|_| io::Error::other("child pid exceeds pid_t"))?;
                if unsafe { libc::kill(native, libc::SIGKILL) } == 0 {
                    Ok(())
                } else {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(libc::ESRCH) {
                        Ok(())
                    } else {
                        Err(error)
                    }
                }
            }
        }
    }

    /// The single bounded lifecycle method for both root kinds.
    ///
    /// Signal the root, then poll for its exit with the SAME deadline and the
    /// real `try_wait` (so the reaped status is cached and a later call replays
    /// it instead of failing with `ECHILD`). Nothing returns early: every failure
    /// is appended to `failures`, so a later failing step can never hide an
    /// earlier cleanup failure. Returns `true` when the reap deadline expired.
    fn terminate_bounded(&mut self, deadline: Instant, failures: &mut Vec<String>) -> bool {
        if let Err(error) = self.kill() {
            failures.push(format!("root_signal_failed:{error}"));
        }
        loop {
            match self.try_wait() {
                // The status is now cached; a later call replays it.
                Ok(Some(_)) => return false,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        failures.push(ROOT_REAP_TIMEOUT.to_owned());
                        return true;
                    }
                    thread::sleep(ROOT_REAP_POLL);
                }
                Err(error) => {
                    // `ECHILD` means it was reaped elsewhere: nothing left to wait
                    // for, and not a cleanup failure.
                    if error.raw_os_error() == Some(libc::ECHILD) {
                        return false;
                    }
                    failures.push(format!("root_reap_failed:{error}"));
                    return false;
                }
            }
        }
    }
}

/// `waitpid(WNOHANG)` for a `posix_spawn`ed root. `0` means still running.
fn raw_try_wait(pid: u32) -> io::Result<Option<ProcessExit>> {
    use std::os::unix::process::ExitStatusExt as _;
    let native =
        libc::pid_t::try_from(pid).map_err(|_| io::Error::other("child pid exceeds pid_t"))?;
    let mut status = 0;
    loop {
        let reaped = unsafe { libc::waitpid(native, &mut status, libc::WNOHANG) };
        if reaped == 0 {
            return Ok(None);
        }
        if reaped == native {
            return Ok(Some(crate::process_spawn::classify_exit_status(
                &std::process::ExitStatus::from_raw(status),
            )));
        }
        if reaped < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Err(io::Error::other("waitpid returned an unexpected pid"));
    }
}

/// The one bound on killing plus reaping a root. `Drop` uses it, and it is the
/// only place a raw root's cleanup deadline comes from, so the call can never
/// exceed it by more than a single poll.
const ROOT_TERMINATE_TIMEOUT: Duration = Duration::from_secs(5);

/// The poll interval inside [`Root::terminate_bounded`].
const ROOT_REAP_POLL: Duration = Duration::from_millis(5);

/// The reason recorded when a root did not become reapable in time.
const ROOT_REAP_TIMEOUT: &str = "root_reap_timeout";

/// One `waitpid(WNOHANG)` observation.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChildWait {
    /// This call reaped the child.
    Reaped,
    /// The child is still running.
    Running,
    /// `ECHILD`: there is nothing left to reap.
    AlreadyReaped,
    /// `EINTR`: retry, but the deadline still applies.
    Interrupted,
    /// Any other native error.
    Failed(i32),
}

/// Pure, injectable bounded reap loop.
///
/// `wait` is one `WNOHANG` observation; `now` and `pause` are injected so the
/// deadline behaviour is testable without a real child. Every retry reason --
/// a still-running child and an `EINTR` alike -- consumes the SAME deadline, and
/// there is no blocking wait anywhere, so a child that never becomes reapable
/// returns `TimedOut` instead of hanging the caller.
#[cfg(target_os = "macos")]
fn reap_with_deadline(
    deadline: Instant,
    mut wait: impl FnMut() -> ChildWait,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(),
) -> io::Result<()> {
    loop {
        match wait() {
            ChildWait::Reaped | ChildWait::AlreadyReaped => return Ok(()),
            ChildWait::Failed(errno) => {
                let error = io::Error::from_raw_os_error(errno);
                return Err(io::Error::new(
                    error.kind(),
                    format!("raw child reap failed: {error}"),
                ));
            }
            ChildWait::Running | ChildWait::Interrupted => {
                if now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "raw child did not become reapable before the deadline",
                    ));
                }
                pause();
            }
        }
    }
}

/// Bounded reap of a raw child this module created.
///
/// Accepts exactly two outcomes: this pid was reaped by this call, or `ECHILD`
/// proves there is nothing left to reap. Reaching the deadline is a typed
/// `TimedOut` -- the loop never falls back to a blocking `waitpid`, so a child
/// that cannot be reaped cannot hang the caller. The caller must already have
/// signalled the child; a `TimedOut` result means the child may still exist.
#[cfg(target_os = "macos")]
fn reap_raw_child(native: libc::pid_t, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut status = 0;
    reap_with_deadline(
        deadline,
        || {
            let reaped = unsafe { libc::waitpid(native, &mut status, libc::WNOHANG) };
            if reaped == native {
                ChildWait::Reaped
            } else if reaped == 0 {
                ChildWait::Running
            } else if reaped > 0 {
                // Only this pid is a legal success; any other positive result is
                // an unexpected waitpid outcome rather than "still running".
                ChildWait::Failed(libc::EIO)
            } else {
                match io::Error::last_os_error().raw_os_error() {
                    Some(libc::ECHILD) => ChildWait::AlreadyReaped,
                    Some(libc::EINTR) => ChildWait::Interrupted,
                    Some(errno) => ChildWait::Failed(errno),
                    None => ChildWait::Failed(libc::EIO),
                }
            }
        },
        Instant::now,
        || thread::sleep(Duration::from_millis(1)),
    )
}

/// The single raw-child cleanup helper. `kill` is not allowed to fail silently:
/// a non-`ESRCH` kill error is reported even when the reap later succeeds, and
/// the reap itself is bounded and strict about its accepted outcomes.
#[cfg(target_os = "macos")]
pub(super) fn kill_and_reap_child(native: libc::pid_t, timeout: Duration) -> io::Result<()> {
    if native <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "raw child pid must be greater than 1",
        ));
    }
    // `ESRCH` only means the child already exited; it is not a cleanup failure.
    let kill_error = if unsafe { libc::kill(native, libc::SIGKILL) } == 0 {
        None
    } else {
        let error = io::Error::last_os_error();
        (error.raw_os_error() != Some(libc::ESRCH)).then_some(error)
    };
    let reaped = reap_raw_child(native, timeout);
    match (kill_error, reaped) {
        (None, Ok(())) => Ok(()),
        (Some(error), Ok(())) => Err(io::Error::new(
            error.kind(),
            format!("raw child kill failed (reaped anyway): {error}"),
        )),
        (None, Err(error)) => Err(error),
        // Both effects failed: keep BOTH reasons, do not let the kill failure
        // disappear behind the reap failure (or the reverse).
        (Some(kill), Err(reap)) => Err(io::Error::new(
            reap.kind(),
            format!("raw child cleanup failed: reap: {reap}; kill: {kill}"),
        )),
    }
}

pub struct ContainedChildOutput(File);

pub struct ContainedChildInput(File);

/// A `std::process::Child` stdio handle owns a descriptor; wrap it as `File` so a
/// `Command`-spawned root and a `posix_spawn`ed frozen root expose one stream
/// type. The conversion takes ownership of the handle exactly once.
fn file_from_child_handle(handle: impl Into<std::os::fd::OwnedFd>) -> File {
    File::from(handle.into())
}

impl Write for ContainedChildInput {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Read for ContainedChildOutput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

pub(crate) fn spawn(spec: &ContainedHeadlessCommand) -> io::Result<ContainedChild> {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args).stdin(match &spec.stdin {
        ContainedInput::Null => Stdio::null(),
        ContainedInput::Text(_) | ContainedInput::Pipe => Stdio::piped(),
    });
    command.stdout(output_stdio(&spec.stdout)?);
    command.stderr(output_stdio(&spec.stderr)?);
    if let Some(directory) = &spec.current_dir {
        command.current_dir(directory);
    }
    for (key, value) in &spec.env {
        match value {
            Some(value) => {
                command.env(key, value);
            }
            None => {
                command.env_remove(key);
            }
        }
    }
    configure_owned_command(&mut command).map_err(io::Error::other)?;
    install_limits(&mut command, spec.limits);
    let mut child = command.spawn()?;
    let tree = match ProcessTreeGuard::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            // Bounded like every other cleanup path: the root is killed and then
            // polled for its exit under one deadline instead of an unbounded
            // `Child::wait`.
            let mut root = Root::Command(child);
            let mut failures = Vec::new();
            root.terminate_bounded(Instant::now() + ROOT_TERMINATE_TIMEOUT, &mut failures);
            return Err(io::Error::other(error));
        }
    };
    let stdin = match &spec.stdin {
        ContainedInput::Text(text) => {
            if let Some(mut stdin) = child.stdin.take() {
                let text = text.clone();
                thread::spawn(move || {
                    let _ = stdin.write_all(&text);
                });
            }
            None
        }
        ContainedInput::Pipe => child
            .stdin
            .take()
            .map(|stdin| ContainedChildInput(file_from_child_handle(stdin))),
        ContainedInput::Null => None,
    };
    Ok(ContainedChild {
        root: Root::Command(child),
        tree,
        stdin,
        stdout: None,
        stderr: None,
    })
}

/// The platform's race-free frozen launch. macOS implements it with
/// `posix_spawn` + `POSIX_SPAWN_START_SUSPENDED`; every other Unix target
/// refuses before creating a process rather than pretending to freeze.
#[cfg(target_os = "linux")]
pub(crate) fn spawn_suspended_frozen(
    _spec: &ContainedHeadlessCommand,
) -> io::Result<(ContainedChild, FrozenProcessIdentity)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this platform has no suspended pre-resume launch; use spawn() instead",
    ))
}

#[cfg(target_os = "macos")]
pub(crate) fn spawn_suspended_frozen(
    spec: &ContainedHeadlessCommand,
) -> io::Result<(ContainedChild, FrozenProcessIdentity)> {
    macos_frozen::spawn(spec)
}

fn install_limits(command: &mut Command, limits: ContainedProcessLimits) {
    use std::os::unix::process::CommandExt as _;
    unsafe {
        command.pre_exec(move || apply_limits(limits));
    }
}

fn apply_limits(limits: ContainedProcessLimits) -> io::Result<()> {
    if let Some(value) = limits.cpu_seconds {
        set_limit("cpu_seconds", libc::RLIMIT_CPU, value)?;
    }
    if let Some(value) = limits.memory_bytes {
        set_limit("memory_bytes", libc::RLIMIT_AS, value)?;
    }
    if let Some(value) = limits.file_size_bytes {
        set_limit("file_size_bytes", libc::RLIMIT_FSIZE, value)?;
    }
    if let Some(value) = limits.open_files {
        set_limit("open_files", libc::RLIMIT_NOFILE, value)?;
    }
    if let Some(value) = limits.active_processes {
        set_limit("active_processes", libc::RLIMIT_NPROC, u64::from(value))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(target_os = "macos")]
type RlimitResource = libc::c_int;

fn set_limit(name: &'static str, resource: RlimitResource, value: u64) -> io::Result<()> {
    let value = libc::rlim_t::try_from(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "contained resource limit exceeds the native rlim_t width",
        )
    })?;
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    if unsafe { libc::setrlimit(resource, &raw const limit) } == 0 {
        Ok(())
    } else {
        let source = io::Error::last_os_error();
        Err(io::Error::new(
            source.kind(),
            format!("could not install contained {name} limit: {source}"),
        ))
    }
}

fn output_stdio(output: &ContainedOutput) -> io::Result<Stdio> {
    match output {
        ContainedOutput::Null => Ok(Stdio::null()),
        ContainedOutput::Capture => Ok(Stdio::piped()),
        ContainedOutput::File(file) => file.try_clone().map(Stdio::from),
    }
}

impl ContainedChild {
    pub(crate) fn id(&self) -> u32 {
        self.root.id()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ProcessExit>> {
        self.root.try_wait()
    }

    pub(crate) fn containment_process_ids(&self, max_members: usize) -> io::Result<Vec<u32>> {
        self.tree.process_ids(max_members).map_err(io::Error::other)
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ContainedChildOutput> {
        if let Some(captured) = self.stdout.take() {
            return Some(captured);
        }
        match &mut self.root {
            Root::Command(child) => child
                .stdout
                .take()
                .map(|stdout| ContainedChildOutput(file_from_child_handle(stdout))),
            Root::Raw { .. } => None,
        }
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ContainedChildOutput> {
        if let Some(captured) = self.stderr.take() {
            return Some(captured);
        }
        match &mut self.root {
            Root::Command(child) => child
                .stderr
                .take()
                .map(|stderr| ContainedChildOutput(file_from_child_handle(stderr))),
            Root::Raw { .. } => None,
        }
    }

    pub(crate) fn take_stdin(&mut self) -> Option<ContainedChildInput> {
        self.stdin.take()
    }

    /// Terminate the contained tree, then the root, and reap the root within
    /// `timeout`.
    ///
    /// The bound is real: the tree termination is a bounded signal sweep and the
    /// root is killed and then polled for its exit under ONE deadline, so the
    /// call cannot exceed `timeout` by more than a single poll. Every step's
    /// failure is reported -- a failing reap never hides a failing tree
    /// termination, and the root's own signal failure is kept beside both.
    pub(crate) fn terminate_and_wait(&mut self, timeout: Duration) -> io::Result<()> {
        let mut failures = Vec::new();
        if let Err(error) = self.tree.terminate() {
            failures.push(format!("tree_terminate_failed:{error}"));
        }
        let deadline = Instant::now() + timeout;
        let timed_out = self.root.terminate_bounded(deadline, &mut failures);
        merge_cleanup_failures(timed_out, &failures)
    }
}

/// Merge every cleanup failure into one typed error.
///
/// Pure so the merge rule is testable without a real process: all reasons are
/// kept (none is dropped behind another), and a deadline expiry is reported as
/// `TimedOut` rather than a generic error.
fn merge_cleanup_failures(timed_out: bool, failures: &[String]) -> io::Result<()> {
    if failures.is_empty() {
        return Ok(());
    }
    let kind = if timed_out {
        io::ErrorKind::TimedOut
    } else {
        io::ErrorKind::Other
    };
    Err(io::Error::new(kind, failures.join("; ")))
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        // The same unified, bounded path the public `terminate_and_wait` uses.
        // `Drop` cannot report, so the failures are dropped, but it must still be
        // bounded and must never panic: there is no `unwrap`, no unbounded
        // `waitpid`/`Child::wait`, and the deadline is fixed.
        let _ = self.tree.terminate();
        let mut failures = Vec::new();
        self.root
            .terminate_bounded(Instant::now() + ROOT_TERMINATE_TIMEOUT, &mut failures);
        let _ = merge_cleanup_failures(false, &failures);
    }
}

/// macOS `posix_spawn` + `POSIX_SPAWN_START_SUSPENDED`.
///
/// `std::process::Command` cannot express a suspended launch: its `pre_exec`
/// handshake blocks the parent on the CLOEXEC exec-error pipe if the child stops
/// before the image is entered, and a `Command`-spawned `Child` has no public
/// constructor from a pid. `posix_spawn` returns with the image loaded and the
/// file actions/exec preparation already done, holding back the target
/// executable's first user instruction, so the exact start identity can be frozen
/// before any user instruction runs.
#[cfg(target_os = "macos")]
mod macos_frozen {
    use std::{
        ffi::{CString, OsStr, OsString},
        fs::File,
        io::{self, Write},
        os::{
            fd::{AsRawFd as _, FromRawFd as _, IntoRawFd as _, OwnedFd, RawFd},
            unix::ffi::OsStrExt as _,
        },
        ptr, thread,
        time::Duration,
    };

    use crate::{
        contained_process::{
            ContainedHeadlessCommand, ContainedInput, ContainedOutput, ContainedProcessLimits,
            FrozenProcessIdentity,
        },
        process::ProcessTreeGuard,
    };

    use super::{ContainedChild, ContainedChildInput, ContainedChildOutput, Root};

    /// Bounded deadline for reaping a failed frozen launch.
    const FAILED_LAUNCH_REAP_TIMEOUT: Duration = Duration::from_secs(5);

    /// The interpreter `execvp` falls back to for a file that is not a valid
    /// image format and carries no shebang.
    const SHELL_FALLBACK: &str = "/bin/sh";

    unsafe extern "C" {
        /// macOS 10.15+, declared here because the pinned `libc` does not export
        /// it. Sets the child's working directory through the spawn file-action
        /// list, so no process-global `chdir` is involved.
        fn posix_spawn_file_actions_addchdir_np(
            actions: *mut libc::posix_spawn_file_actions_t,
            path: *const libc::c_char,
        ) -> libc::c_int;
    }

    pub(super) fn spawn(
        spec: &ContainedHeadlessCommand,
    ) -> io::Result<(ContainedChild, FrozenProcessIdentity)> {
        // posix_spawn cannot install rlimits or a Job-like limit set. Refuse
        // BEFORE creating anything rather than silently dropping the caller's
        // limits.
        if spec.limits != ContainedProcessLimits::default() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "macOS suspended-frozen spawn cannot install process limits; use spawn() instead",
            ));
        }

        let environment = effective_environment(spec)?;
        let candidates = program_candidates(spec.program.as_os_str(), &environment);
        let searched = candidates.searched;
        // `argv[0]` stays the program exactly as the caller wrote it, like
        // `Command`; only the exec path is the candidate.
        let argv = CStringArray::argv(spec)?;
        let envp = CStringArray::envp(&environment)?;
        let mut actions = FileActions::new()?;
        let mut stdio = Stdio::prepare(spec, &mut actions)?;
        if let Some(directory) = &spec.current_dir {
            let path = cstring(directory.as_os_str())?;
            // SAFETY: `actions.raw` is an initialised file-action list owned by
            // `actions`, and `path` is a live NUL-terminated C string.
            let rc =
                unsafe { posix_spawn_file_actions_addchdir_np(&mut actions.raw, path.as_ptr()) };
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc));
            }
        }
        let attr = Attr::suspended_new_group()?;
        let shell = cstring(OsStr::new(SHELL_FALLBACK))?;

        let mut pid: libc::pid_t = 0;
        let mut seen_eacces = false;
        // `execvp`'s search: try each candidate with the real exec, and classify
        // the error the kernel reports. There is no pre-check, so a candidate
        // that changes between decision and execution cannot be mis-selected.
        let rc = 'search: {
            for candidate in &candidates.paths {
                let candidate_c = cstring(candidate.as_os_str())?;
                // SAFETY: every pointer is live for the duration of the call:
                // `candidate_c`, `argv` and `envp` own their bytes, and
                // `actions`/`attr` are initialised native objects. `pid` is a
                // valid out-parameter. argv and envp are NULL-terminated arrays
                // of NUL-terminated strings, and every fd named by a file action
                // is still open (the child ends are closed only after this
                // returns). POSIX_SPAWN_CLOEXEC_DEFAULT keeps any other
                // descriptor out of the child.
                //
                // `posix_spawn` (not `posix_spawnp`) is deliberate: the candidate
                // list is already the CHILD's `PATH` order, and `posix_spawnp`
                // would search the CALLER's `PATH` instead -- a silent difference
                // for any spec that sets or removes `PATH`.
                pid = 0;
                let mut rc = unsafe {
                    libc::posix_spawn(
                        &mut pid,
                        candidate_c.as_ptr(),
                        &actions.raw,
                        &attr.raw,
                        argv.ptrs.as_ptr(),
                        envp.ptrs.as_ptr(),
                    )
                };
                if rc == libc::ENOEXEC {
                    // A file that is not a valid image and carries no shebang is
                    // run through `/bin/sh` by `execvp`, with the candidate as the
                    // script operand so the child's `$0` matches `Command`. The
                    // failed attempt created no child (the error came back from
                    // the spawn's exec handshake), so this is a retry of the same
                    // candidate, not a second process.
                    let fallback = CStringArray::shell_fallback(candidate, &spec.args)?;
                    pid = 0;
                    // SAFETY: as above; `fallback` and `shell` own their bytes.
                    rc = unsafe {
                        libc::posix_spawn(
                            &mut pid,
                            shell.as_ptr(),
                            &actions.raw,
                            &attr.raw,
                            fallback.ptrs.as_ptr(),
                            envp.ptrs.as_ptr(),
                        )
                    };
                }
                if rc == 0 {
                    break 'search 0;
                }
                match rc {
                    // The candidate does not name a traversable file here.
                    libc::ENOENT | libc::ENOTDIR => continue,
                    // Permission denied. Apple's `execvp` distinguishes two
                    // causes, and the difference is observable:
                    //
                    // * the final component is reachable but not executable (a
                    //   non-executable file, or a directory) -> remember EACCES;
                    // * an ANCESTOR denies search -> the candidate contributes
                    //   ENOENT instead, so a single unreachable directory does
                    //   not turn the whole search into a permission error.
                    //
                    // `metadata` is used only to classify an error the exec
                    // attempt already produced -- never to pick the candidate --
                    // and an explicit path is not a search, so it keeps the
                    // kernel's own EACCES.
                    libc::EACCES => {
                        if !searched || candidate.metadata().is_ok() {
                            seen_eacces = true;
                        }
                        continue;
                    }
                    // Any other error is terminal.
                    other => break 'search other,
                }
            }
            // Nothing exec'd: `execvp` prefers the remembered EACCES.
            if seen_eacces {
                libc::EACCES
            } else {
                libc::ENOENT
            }
        };
        if rc != 0 {
            // No child exists, so there is nothing to reap. `stdio` and `actions`
            // drop here, closing every pipe end.
            return Err(io::Error::from_raw_os_error(rc));
        }

        // The root now exists and is suspended BEFORE its first user
        // instruction. From here every failure must kill and reap it.
        let child_pid = match u32::try_from(pid) {
            Ok(child_pid) => child_pid,
            Err(_) => {
                return Err(fail(pid_of(pid), "child pid is out of range"));
            }
        };
        stdio.close_child_ends();

        // 1. Freeze the exact start identity while still suspended.
        let start_identity = match crate::process_observation::start_identity(child_pid) {
            Ok(identity) if !identity.is_empty() => identity,
            Ok(_) => return Err(fail(child_pid, "child start identity is empty")),
            Err(reason) => {
                return Err(fail(
                    child_pid,
                    format!("child start identity is unavailable: {reason}"),
                ));
            }
        };

        // 2. Establish containment while still suspended.
        let tree = match ProcessTreeGuard::attach_pid(child_pid) {
            Ok(tree) => tree,
            Err(reason) => return Err(fail(child_pid, reason)),
        };

        // 2b. The guard read the identity again; the two independent observations
        //     must agree byte-for-byte, or the freeze is not trustworthy.
        if tree.root_start_identity() != Some(start_identity.as_str()) {
            drop(tree);
            return Err(fail(
                child_pid,
                "contained root start identity changed between freeze and attach",
            ));
        }

        // 3. Resume only after identity and containment are both established.
        // SAFETY: `pid` is the pid `posix_spawn` just returned and whose identity
        // was verified above, so this cannot signal an unrelated process.
        if unsafe { libc::kill(pid, libc::SIGCONT) } != 0 {
            let error = io::Error::last_os_error();
            drop(tree);
            return Err(fail(child_pid, error));
        }

        let stdin = match &spec.stdin {
            ContainedInput::Text(text) => {
                if let Some(mut writer) = stdio.stdin.take() {
                    let text = text.clone();
                    thread::spawn(move || {
                        let _ = writer.write_all(&text);
                    });
                }
                None
            }
            ContainedInput::Pipe => stdio.stdin.take(),
            ContainedInput::Null => None,
        };
        Ok((
            ContainedChild {
                root: Root::Raw {
                    pid: child_pid,
                    exit: None,
                },
                tree,
                stdin,
                stdout: stdio.stdout.take(),
                stderr: stdio.stderr.take(),
            },
            FrozenProcessIdentity {
                pid: child_pid,
                start_identity,
            },
        ))
    }

    /// Kill the still-suspended root and reap it, so a failed frozen launch
    /// leaves no process and no zombie behind. No descendants can exist because
    /// the root has not executed.
    ///
    /// The original reason and any cleanup failure are BOTH preserved: the
    /// returned error keeps the primary text and appends the cleanup detail
    /// rather than dropping either.
    fn fail(pid: u32, error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
        let primary = error.into();
        let text = primary.to_string();
        if pid <= 1 {
            return io::Error::other(primary);
        }
        let Ok(native) = libc::pid_t::try_from(pid) else {
            return io::Error::other(primary);
        };
        match super::kill_and_reap_child(native, FAILED_LAUNCH_REAP_TIMEOUT) {
            Ok(()) => io::Error::other(primary),
            Err(cleanup) => io::Error::other(format!(
                "{text}; failed-launch cleanup did not complete: {cleanup}"
            )),
        }
    }

    fn pid_of(pid: libc::pid_t) -> u32 {
        u32::try_from(pid).unwrap_or_default()
    }

    fn cstring(value: &OsStr) -> io::Result<CString> {
        CString::new(value.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "contained process parameter contains NUL",
            )
        })
    }

    /// Owns the backing `CString`s so the pointer array stays valid.
    struct CStringArray {
        _storage: Vec<CString>,
        ptrs: Vec<*mut libc::c_char>,
    }

    impl CStringArray {
        fn from_entries(entries: impl IntoIterator<Item = Vec<u8>>) -> io::Result<Self> {
            let mut storage = Vec::new();
            for entry in entries {
                storage.push(CString::new(entry).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "contained process parameter contains NUL",
                    )
                })?);
            }
            let mut ptrs = storage
                .iter()
                .map(|entry| entry.as_ptr().cast_mut())
                .collect::<Vec<_>>();
            ptrs.push(ptr::null_mut());
            Ok(Self {
                _storage: storage,
                ptrs,
            })
        }

        fn argv(spec: &ContainedHeadlessCommand) -> io::Result<Self> {
            let mut entries = vec![spec.program.as_os_str().as_bytes().to_vec()];
            for argument in &spec.args {
                entries.push(argument.as_bytes().to_vec());
            }
            Self::from_entries(entries)
        }

        fn envp(environment: &[(OsString, OsString)]) -> io::Result<Self> {
            Self::from_entries(environment.iter().map(environment_entry))
        }

        /// The `execvp` `ENOEXEC` retry: run the resolved program as a script
        /// through the fallback interpreter, preserving the caller's arguments.
        fn shell_fallback(program: &std::path::Path, args: &[OsString]) -> io::Result<Self> {
            let mut entries = vec![
                SHELL_FALLBACK.as_bytes().to_vec(),
                program.as_os_str().as_bytes().to_vec(),
            ];
            for argument in args {
                entries.push(argument.as_bytes().to_vec());
            }
            Self::from_entries(entries)
        }
    }

    fn environment_entry((key, value): &(OsString, OsString)) -> Vec<u8> {
        let mut entry = key.as_bytes().to_vec();
        entry.push(b'=');
        entry.extend_from_slice(value.as_bytes());
        entry
    }

    /// The child's effective environment: the inherited one with `env_remove`
    /// applied and `env` set, preserving raw bytes. Entries are de-duplicated so
    /// the child sees one value per key with the last mutation winning, exactly
    /// like `Command::env`/`env_remove`.
    fn effective_environment(
        spec: &ContainedHeadlessCommand,
    ) -> io::Result<Vec<(OsString, OsString)>> {
        let mut entries: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        for (key, value) in &spec.env {
            entries.retain(|(existing, _)| existing != key);
            if let Some(value) = value {
                entries.push((key.clone(), value.clone()));
            }
        }
        Ok(entries)
    }

    /// The candidate paths `execvp` would try, in order.
    ///
    /// `Command` swaps `environ` to the CHILD's environment before calling
    /// `execvp`, so a slash-free program is searched through the child's `PATH`
    /// (and, when the child has no `PATH`, through `confstr(_CS_PATH)`), not
    /// through the caller's `PATH`. `posix_spawnp` always searches the CALLER's
    /// `PATH`, so using it would silently change program resolution for any spec
    /// that sets or removes `PATH`; instead the candidates are spelled exactly as
    /// `execvp` would spell them and handed to `posix_spawn` one at a time.
    ///
    /// A relative or empty `PATH` element therefore stays relative: the CHILD
    /// resolves it after its `chdir` file action, so the decided and executed
    /// objects are the same one. No candidate is chosen ahead of the exec --
    /// `access`/`metadata` must never select the one candidate to try, because
    /// that both opens a TOCTOU hole and models `execvp` wrongly. The real error
    /// has to come from the exec attempt itself.
    struct ProgramCandidates {
        paths: Vec<std::path::PathBuf>,
        /// False for an explicit path (the program contained a separator).
        searched: bool,
    }

    fn program_candidates(
        program: &OsStr,
        environment: &[(OsString, OsString)],
    ) -> ProgramCandidates {
        if program.as_bytes().contains(&b'/') {
            // An explicit path is a single candidate, used verbatim.
            return ProgramCandidates {
                paths: vec![std::path::PathBuf::from(program)],
                searched: false,
            };
        }
        let path = environment
            .iter()
            .find(|(key, _)| key.as_bytes() == b"PATH")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(default_path);
        ProgramCandidates {
            paths: path
                .as_bytes()
                .split(|byte| *byte == b':')
                .map(|directory| {
                    let element = if directory.is_empty() {
                        std::path::PathBuf::from(".")
                    } else {
                        std::path::PathBuf::from(OsStr::from_bytes(directory))
                    };
                    element.join(program)
                })
                .collect(),
            searched: true,
        }
    }

    /// `execvp`'s fallback when the child environment has no `PATH`.
    fn default_path() -> OsString {
        // `confstr` reports the required length; a 0 return means "no value".
        let length = unsafe { libc::confstr(libc::_CS_PATH, ptr::null_mut(), 0) };
        if length > 1 {
            let mut buffer = vec![0_u8; length];
            let written =
                unsafe { libc::confstr(libc::_CS_PATH, buffer.as_mut_ptr().cast(), buffer.len()) };
            if written > 0 && written <= buffer.len() {
                buffer.truncate(written.saturating_sub(1));
                return OsStr::from_bytes(&buffer).to_owned();
            }
        }
        OsString::from("/usr/bin:/bin:/usr/sbin:/sbin")
    }

    struct Attr {
        raw: libc::posix_spawnattr_t,
    }

    impl Attr {
        fn suspended_new_group() -> io::Result<Self> {
            let mut raw: libc::posix_spawnattr_t = ptr::null_mut();
            // SAFETY: `raw` is a valid out-parameter for an attribute list.
            let rc = unsafe { libc::posix_spawnattr_init(&mut raw) };
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc));
            }
            let mut attr = Self { raw };
            let flags = (libc::POSIX_SPAWN_START_SUSPENDED
                | libc::POSIX_SPAWN_SETPGROUP
                | libc::POSIX_SPAWN_CLOEXEC_DEFAULT) as libc::c_short;
            // SAFETY: `attr.raw` is initialised and owned by `attr`.
            let rc = unsafe { libc::posix_spawnattr_setflags(&mut attr.raw, flags) };
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc));
            }
            // `pgroup = 0` makes the child the leader of a fresh process group
            // whose id equals its pid, matching `configure_owned_command`.
            // SAFETY: as above.
            let rc = unsafe { libc::posix_spawnattr_setpgroup(&mut attr.raw, 0) };
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc));
            }
            Ok(attr)
        }
    }

    impl Drop for Attr {
        fn drop(&mut self) {
            // SAFETY: `self.raw` was initialised and is destroyed exactly once.
            unsafe {
                libc::posix_spawnattr_destroy(&mut self.raw);
            }
        }
    }

    struct FileActions {
        raw: libc::posix_spawn_file_actions_t,
    }

    impl FileActions {
        fn new() -> io::Result<Self> {
            let mut raw: libc::posix_spawn_file_actions_t = ptr::null_mut();
            // SAFETY: `raw` is a valid out-parameter for a file-action list.
            let rc = unsafe { libc::posix_spawn_file_actions_init(&mut raw) };
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc));
            }
            Ok(Self { raw })
        }

        fn dup2(&mut self, fd: RawFd, target: RawFd) -> io::Result<()> {
            // SAFETY: `self.raw` is initialised; `fd` is open and `target` is 0/1/2.
            let rc = unsafe { libc::posix_spawn_file_actions_adddup2(&mut self.raw, fd, target) };
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc));
            }
            Ok(())
        }

        fn open_null(&mut self, target: RawFd, flags: libc::c_int) -> io::Result<()> {
            // SAFETY: `self.raw` is initialised and the path is a static C string.
            let rc = unsafe {
                libc::posix_spawn_file_actions_addopen(
                    &mut self.raw,
                    target,
                    c"/dev/null".as_ptr(),
                    flags,
                    0,
                )
            };
            if rc != 0 {
                return Err(io::Error::from_raw_os_error(rc));
            }
            Ok(())
        }
    }

    impl Drop for FileActions {
        fn drop(&mut self) {
            // SAFETY: `self.raw` was initialised and is destroyed exactly once.
            unsafe {
                libc::posix_spawn_file_actions_destroy(&mut self.raw);
            }
        }
    }

    /// Parent-side and child-side pipe/fd ownership for one spawn.
    struct Stdio {
        /// Child-side ends that must be closed AFTER `posix_spawn` returns, so
        /// the child inherits them through its file actions.
        child_ends: Vec<OwnedFd>,
        stdin: Option<ContainedChildInput>,
        stdout: Option<ContainedChildOutput>,
        stderr: Option<ContainedChildOutput>,
    }

    impl Stdio {
        fn prepare(spec: &ContainedHeadlessCommand, actions: &mut FileActions) -> io::Result<Self> {
            let mut child_ends = Vec::new();
            let (stdin, stdin_child) = match &spec.stdin {
                ContainedInput::Null => {
                    actions.open_null(0, libc::O_RDONLY)?;
                    (None, None)
                }
                ContainedInput::Text(_) | ContainedInput::Pipe => {
                    let (read, write) = pipe()?;
                    actions.dup2(read.as_raw_fd(), 0)?;
                    (Some(pipe_writer(write)), Some(read))
                }
            };
            if let Some(end) = stdin_child {
                child_ends.push(end);
            }
            let (stdout, stdout_child) = output(&spec.stdout, 1, actions)?;
            if let Some(end) = stdout_child {
                child_ends.push(end);
            }
            let (stderr, stderr_child) = output(&spec.stderr, 2, actions)?;
            if let Some(end) = stderr_child {
                child_ends.push(end);
            }
            Ok(Self {
                child_ends,
                stdin,
                stdout,
                stderr,
            })
        }

        fn close_child_ends(&mut self) {
            self.child_ends.clear();
        }
    }

    fn pipe_writer(write: OwnedFd) -> ContainedChildInput {
        ContainedChildInput(unsafe { File::from_raw_fd(write.into_raw_fd()) })
    }

    /// Wires one output stream and returns `(parent_end, child_end)`.
    fn output(
        output: &ContainedOutput,
        target: RawFd,
        actions: &mut FileActions,
    ) -> io::Result<(Option<ContainedChildOutput>, Option<OwnedFd>)> {
        match output {
            ContainedOutput::Null => {
                actions.open_null(target, libc::O_WRONLY)?;
                Ok((None, None))
            }
            ContainedOutput::Capture => {
                let (read, write) = pipe()?;
                actions.dup2(write.as_raw_fd(), target)?;
                let parent = ContainedChildOutput(unsafe { File::from_raw_fd(read.into_raw_fd()) });
                Ok((Some(parent), Some(write)))
            }
            ContainedOutput::File(file) => {
                actions.dup2(file.as_raw_fd(), target)?;
                Ok((None, None))
            }
        }
    }

    fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: `fds` is a two-element out-parameter for `pipe`.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // Marking both close-on-exec keeps a spawn's pipes out of an unrelated
        // later child; the child's own dup2'd 0/1/2 are unaffected (dup2 clears
        // it). The flag is checked, not assumed: if either call fails the
        // descriptors are closed here and the spawn is refused BEFORE it can
        // leak an inheritable descriptor.
        for descriptor in fds {
            // SAFETY: `descriptor` was just created and is owned by this frame.
            if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                let error = io::Error::last_os_error();
                // SAFETY: both descriptors are still owned by this frame.
                unsafe {
                    libc::close(fds[0]);
                    libc::close(fds[1]);
                }
                return Err(io::Error::new(
                    error.kind(),
                    format!("contained pipe could not be marked close-on-exec: {error}"),
                ));
            }
        }
        // SAFETY: each descriptor is owned exactly once and wrapped exactly once.
        Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod frozen_cleanup_tests {
    use super::*;

    /// Native proof that this process has nothing left to reap for `pid`.
    /// `WNOHANG` returning `-1/ECHILD` is the real "already reaped" evidence;
    /// a process observation that is merely "not live" cannot distinguish a
    /// reaped child from a zombie or a pid that has not been reaped yet.
    fn assert_reaped(pid: u32) {
        let native = libc::pid_t::try_from(pid).expect("pid fits pid_t");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let mut status = 0;
            let reaped = unsafe { libc::waitpid(native, &mut status, libc::WNOHANG) };
            if reaped == native {
                return;
            }
            if reaped < 0 {
                let error = std::io::Error::last_os_error();
                assert_eq!(
                    error.raw_os_error(),
                    Some(libc::ECHILD),
                    "expected ECHILD for reaped pid {pid}, got {error}"
                );
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pid {pid} is still a live unreaped child"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Every post-`posix_spawn` failure path funnels through one kill/reap
    /// helper. Drive it against a real suspended root (the same state a failed
    /// frozen launch is in) and prove with a native `waitpid` that the child is
    /// reaped, so the failure cannot leave an orphan or a zombie.
    #[test]
    fn failure_cleanup_kills_and_reaps_a_suspended_root() {
        let mut command = ContainedHeadlessCommand::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let (child, identity) = command
            .spawn_suspended_frozen()
            .expect("frozen spawn for failure cleanup");
        assert_eq!(identity.pid, child.id());
        kill_and_reap_child(identity.pid as libc::pid_t, Duration::from_secs(5))
            .expect("the shared cleanup helper reaps the suspended root");
        assert_reaped(identity.pid);
        // Dropping must be a no-op after the explicit reap.
        drop(child);
        assert_reaped(identity.pid);
    }

    /// The normal terminate path must leave a genuinely reaped child behind
    /// across many iterations.
    #[test]
    fn repeated_frozen_terminate_reaps_every_root() {
        for _ in 0..20 {
            let mut command = ContainedHeadlessCommand::new("/bin/sh");
            command.args(["-c", "sleep 30"]);
            let (mut child, identity) = command.spawn_suspended_frozen().expect("frozen spawn");
            child
                .terminate_and_wait(Duration::from_secs(5))
                .expect("terminate frozen child");
            assert_reaped(identity.pid);
        }
    }

    /// A dropped frozen root is killed and reliably reaped by `Drop`; one
    /// `WNOHANG` sample would not be a reap.
    #[test]
    fn dropped_frozen_root_is_reaped_by_drop() {
        for _ in 0..20 {
            let mut command = ContainedHeadlessCommand::new("/bin/sh");
            command.args(["-c", "sleep 30"]);
            let (child, identity) = command.spawn_suspended_frozen().expect("frozen spawn");
            drop(child);
            assert_reaped(identity.pid);
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod reap_deadline_tests {
    use super::{ChildWait, reap_with_deadline};
    use std::{
        cell::Cell,
        io,
        time::{Duration, Instant},
    };

    /// A deterministic clock that advances by `step` on every read, so the
    /// deadline behaviour is testable without a real child or a real sleep.
    struct Ticks {
        base: Instant,
        ticks: Cell<u64>,
        step: Duration,
    }

    impl Ticks {
        fn new(step: Duration) -> Self {
            Self {
                base: Instant::now(),
                ticks: Cell::new(0),
                step,
            }
        }

        fn deadline_after(&self, after: Duration) -> Instant {
            self.base + after
        }

        fn now(&self) -> Instant {
            let tick = self.ticks.get();
            self.ticks.set(tick + 1);
            self.base + self.step * u32::try_from(tick).unwrap_or(u32::MAX)
        }
    }

    fn run(
        step: Duration,
        budget: Duration,
        mut wait: impl FnMut() -> ChildWait,
    ) -> (io::Result<()>, u32) {
        let clock = Ticks::new(step);
        let probes = Cell::new(0u32);
        let result = reap_with_deadline(
            clock.deadline_after(budget),
            || {
                probes.set(probes.get() + 1);
                wait()
            },
            || clock.now(),
            || {},
        );
        (result, probes.get())
    }

    /// A child that never becomes reapable must time out; there is no blocking
    /// fallback and the probe count stays inside the budget.
    #[test]
    fn a_still_running_child_times_out_within_the_budget() {
        let (result, probes) = run(Duration::from_millis(1), Duration::from_millis(10), || {
            ChildWait::Running
        });
        assert_eq!(
            result.expect_err("must time out").kind(),
            io::ErrorKind::TimedOut
        );
        assert!(probes <= 13, "probes={probes} must stay inside the budget");
    }

    /// `EINTR` consumes the SAME deadline: it can never spin forever.
    #[test]
    fn repeated_eintr_still_times_out() {
        let (result, probes) = run(Duration::from_millis(1), Duration::from_millis(10), || {
            ChildWait::Interrupted
        });
        assert_eq!(
            result.expect_err("must time out").kind(),
            io::ErrorKind::TimedOut
        );
        assert!(probes <= 13, "probes={probes} must stay inside the budget");
    }

    /// An already-expired deadline stops after one probe.
    #[test]
    fn an_expired_deadline_probes_once() {
        let clock = Ticks::new(Duration::from_millis(1));
        let probes = Cell::new(0u32);
        let result = reap_with_deadline(
            clock.deadline_after(Duration::from_millis(0)),
            || {
                probes.set(probes.get() + 1);
                ChildWait::Running
            },
            || clock.now(),
            || {},
        );
        assert_eq!(
            result.expect_err("must time out").kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(probes.get(), 1);
    }

    #[test]
    fn reaping_and_already_reaped_succeed_immediately() {
        for outcome in [ChildWait::Reaped, ChildWait::AlreadyReaped] {
            let (result, probes) =
                run(Duration::from_millis(1), Duration::from_secs(1), || outcome);
            assert!(result.is_ok(), "{outcome:?} must succeed");
            assert_eq!(probes, 1, "{outcome:?} must not retry");
        }
    }

    #[test]
    fn a_native_failure_is_reported_with_its_kind() {
        let (result, _) = run(Duration::from_millis(1), Duration::from_secs(1), || {
            ChildWait::Failed(libc::EPERM)
        });
        assert_eq!(
            result.expect_err("must fail").kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod lifecycle_merge_tests {
    use super::*;

    /// The merge rule is pure, so every combination is provable without a real
    /// process: no reason is dropped behind another, and a deadline expiry keeps
    /// its typed kind while still carrying all the other reasons.
    #[test]
    fn merging_cleanup_failures_keeps_every_reason() {
        assert!(merge_cleanup_failures(false, &[]).is_ok());

        let single = merge_cleanup_failures(false, &["tree_terminate_failed:t".to_owned()])
            .expect_err("one reason");
        assert_eq!(single.kind(), io::ErrorKind::Other);
        assert_eq!(single.to_string(), "tree_terminate_failed:t");

        let merged = merge_cleanup_failures(
            false,
            &[
                "tree_terminate_failed:t".to_owned(),
                "root_signal_failed:k".to_owned(),
                "root_reap_failed:r".to_owned(),
            ],
        )
        .expect_err("three reasons");
        assert_eq!(merged.kind(), io::ErrorKind::Other);
        for needle in [
            "tree_terminate_failed:t",
            "root_signal_failed:k",
            "root_reap_failed:r",
        ] {
            assert!(merged.to_string().contains(needle), "{merged}");
        }

        // A timeout does not erase the other failures -- and it is typed.
        let timed_out = merge_cleanup_failures(
            true,
            &[
                "tree_terminate_failed:t".to_owned(),
                "root_reap_timeout".to_owned(),
            ],
        )
        .expect_err("timeout");
        assert_eq!(timed_out.kind(), io::ErrorKind::TimedOut);
        assert!(timed_out.to_string().contains("tree_terminate_failed:t"));
        assert!(timed_out.to_string().contains("root_reap_timeout"));
    }

    /// A raw root's exit status must be cached exactly like
    /// `std::process::Child`: after a bounded terminate, a second `try_wait`
    /// replays the same exit instead of failing with `ECHILD`.
    #[test]
    fn raw_root_replays_its_exit_after_a_bounded_terminate() {
        let mut command = ContainedHeadlessCommand::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let (mut child, identity) = command.spawn_suspended_frozen().expect("frozen spawn");
        child
            .terminate_and_wait(Duration::from_secs(5))
            .expect("bounded terminate of a raw root");
        let first = child
            .try_wait()
            .expect("first try_wait")
            .expect("an exit was observed");
        let second = child
            .try_wait()
            .expect("a second try_wait must replay, not fail with ECHILD")
            .expect("the cached exit is replayed");
        assert_eq!(first, second, "the exit status must be replayed verbatim");
        assert_eq!(child.id(), identity.pid);
        // And the public status accessor replays it too.
        assert_eq!(
            child.try_wait().expect("third try_wait"),
            Some(first),
            "every later observation replays the one exit"
        );
    }
}
