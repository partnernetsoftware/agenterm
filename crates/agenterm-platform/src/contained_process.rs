//! Owned headless child processes contained before their first user instruction.

use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    path::PathBuf,
    time::Duration,
};

pub use crate::contract::process_spawn::ProcessExit;

/// Native limits installed before the first user instruction of an owned
/// child. Every field is inherited by descendants inside the containment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContainedProcessLimits {
    pub cpu_seconds: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub file_size_bytes: Option<u64>,
    pub open_files: Option<u64>,
    pub active_processes: Option<u32>,
}

impl ContainedProcessLimits {
    /// Validates the closed bounds and current-platform support without
    /// creating a process or native containment object.
    pub fn validate(self) -> io::Result<()> {
        validate_limits(self)
    }
}

/// A command whose standard streams are discarded and whose complete process
/// tree remains owned by the returned child.
///
/// The environment and working directory are inherited unless `current_dir`
/// is set. Windows creates the root suspended, assigns it to a kill-on-close
/// Job, and only then resumes its primary thread. Unix establishes a fresh
/// process group in the pre-exec child.
pub struct ContainedHeadlessCommand {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
    pub(crate) current_dir: Option<PathBuf>,
    pub(crate) env: Vec<(OsString, Option<OsString>)>,
    pub(crate) stdin: ContainedInput,
    pub(crate) stdout: ContainedOutput,
    pub(crate) stderr: ContainedOutput,
    pub(crate) limits: ContainedProcessLimits,
}

pub(crate) enum ContainedOutput {
    Null,
    Capture,
    File(File),
}

pub(crate) enum ContainedInput {
    Null,
    Text(Vec<u8>),
    Pipe,
}

impl ContainedHeadlessCommand {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
            env: Vec::new(),
            stdin: ContainedInput::Null,
            stdout: ContainedOutput::Null,
            stderr: ContainedOutput::Null,
            limits: ContainedProcessLimits::default(),
        }
    }

    pub fn arg(&mut self, value: impl Into<OsString>) -> &mut Self {
        self.args.push(value.into());
        self
    }

    pub fn args<I, S>(&mut self, values: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(values.into_iter().map(Into::into));
        self
    }

    pub fn current_dir(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.current_dir = Some(path.into());
        self
    }

    /// Sets one variable in the child's inherited environment.
    pub fn env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> &mut Self {
        self.env.push((key.into(), Some(value.into())));
        self
    }

    /// Removes one variable from the child's inherited environment.
    pub fn env_remove(&mut self, key: impl Into<OsString>) -> &mut Self {
        self.env.push((key.into(), None));
        self
    }

    /// Feeds these UTF-8 bytes to the child's standard input and then closes it.
    pub fn stdin_text(&mut self, text: impl Into<String>) -> &mut Self {
        self.stdin = ContainedInput::Text(text.into().into_bytes());
        self
    }

    /// Keeps an owned writable pipe to the contained child's standard input.
    ///
    /// The caller must take the writer from the spawned child. Dropping that
    /// writer delivers EOF without changing process-tree ownership.
    pub fn pipe_stdin(&mut self) -> &mut Self {
        self.stdin = ContainedInput::Pipe;
        self
    }

    /// Captures stdout and stderr through independent owned streams.
    ///
    /// Callers should drain both streams concurrently before waiting for a
    /// verbose child, so neither bounded operating-system pipe can block the
    /// other stream or the child process.
    pub fn capture_output(&mut self) -> &mut Self {
        self.stdout = ContainedOutput::Capture;
        self.stderr = ContainedOutput::Capture;
        self
    }

    /// Redirects stdout to an already-open file instead of capturing it.
    pub fn stdout_file(&mut self, file: File) -> &mut Self {
        self.stdout = ContainedOutput::File(file);
        self
    }

    /// Redirects stderr to an already-open file instead of capturing it.
    pub fn stderr_file(&mut self, file: File) -> &mut Self {
        self.stderr = ContainedOutput::File(file);
        self
    }

    /// Installs hard native resource limits before the target program begins.
    pub fn limits(&mut self, limits: ContainedProcessLimits) -> &mut Self {
        self.limits = limits;
        self
    }

    pub fn spawn(&self) -> io::Result<ContainedChild> {
        validate(self)?;
        crate::selected::contained_process::spawn(self).map(ContainedChild)
    }

    /// Spawns this command SUSPENDED, freezes the root's exact native identity
    /// before its first user instruction, establishes containment, and only then
    /// resumes it.
    ///
    /// This is the race-free form of [`Self::spawn`] for a short-lived owned
    /// child whose identity must be captured before it can exit (a launcher that
    /// finishes in milliseconds leaves no readable start identity). It is the
    /// same containment contract: the root is still the requested program, never
    /// a wrapper. A platform that cannot freeze before the first instruction
    /// returns [`io::ErrorKind::Unsupported`] without creating a process.
    pub fn spawn_suspended_frozen(&self) -> io::Result<(ContainedChild, FrozenProcessIdentity)> {
        validate(self)?;
        crate::selected::contained_process::spawn_suspended_frozen(self)
            .map(|(child, identity)| (ContainedChild(child), identity))
    }
}

/// Exact root-process ownership plus its native descendant-containment owner.
pub struct ContainedChild(crate::selected::contained_process::ContainedChild);

/// The exact native identity of a contained root, frozen before its first user
/// instruction.
///
/// `pid` alone is not ownership: a reused pid names another process. The
/// platform's opaque `start_identity` token (a start timestamp in the host's own
/// unit) makes the pair survive pid reuse, so a later observation can prove the
/// same process object. Both fields are stable across the child's exit: the
/// values are read from the native process record, not from a live handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrozenProcessIdentity {
    pub pid: u32,
    pub start_identity: String,
}

/// One bounded native-containment membership inventory.
///
/// The inventory describes the named containment group, not every genealogical
/// descendant a member might have moved into another group.
pub struct ContainedMemberIds {
    pub provider: &'static str,
    pub breakaway_prevented: bool,
    pub process_ids: Vec<u32>,
}

/// One captured child output stream.
///
/// The stream is owned and `Send`, so stdout and stderr can be drained on two
/// independent worker threads.
pub struct ContainedChildOutput(crate::selected::contained_process::ContainedChildOutput);

/// One owned writer for a contained child's standard input.
///
/// The writer is `Send`, not shared. A resident owner should serialize writes
/// on one thread and drop it exactly once when stdin is closed.
pub struct ContainedChildInput(crate::selected::contained_process::ContainedChildInput);

impl Read for ContainedChildOutput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Write for ContainedChildInput {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl ContainedChild {
    #[must_use]
    pub fn id(&self) -> u32 {
        self.0.id()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ProcessExit>> {
        self.0.try_wait()
    }

    /// Enumerates every current member of this child's native containment
    /// group, or refuses when the caller's hard member bound is exceeded.
    pub fn containment_members(&self, max_members: usize) -> io::Result<ContainedMemberIds> {
        if max_members == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "contained member bound must be nonzero",
            ));
        }
        let mut process_ids = self.0.containment_process_ids(max_members)?;
        process_ids.sort_unstable();
        process_ids.dedup();
        if process_ids.len() > max_members {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "contained process group exceeds the member bound",
            ));
        }
        Ok(ContainedMemberIds {
            provider: if cfg!(windows) {
                "windows-job-object"
            } else {
                "posix-process-group"
            },
            breakaway_prevented: cfg!(windows),
            process_ids,
        })
    }

    /// Takes the captured stdout stream, when output capture was requested.
    pub fn take_stdout(&mut self) -> Option<ContainedChildOutput> {
        self.0.take_stdout().map(ContainedChildOutput)
    }

    /// Takes the captured stderr stream, when output capture was requested.
    pub fn take_stderr(&mut self) -> Option<ContainedChildOutput> {
        self.0.take_stderr().map(ContainedChildOutput)
    }

    /// Takes the configured stdin pipe exactly once.
    pub fn take_stdin(&mut self) -> Option<ContainedChildInput> {
        self.0.take_stdin().map(ContainedChildInput)
    }

    /// Terminates the complete owned tree and waits boundedly for the root.
    pub fn terminate_and_wait(&mut self, timeout: Duration) -> io::Result<()> {
        self.0.terminate_and_wait(timeout)
    }
}

fn validate(command: &ContainedHeadlessCommand) -> io::Result<()> {
    if command.program.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "contained process executable is empty",
        ));
    }
    if command.program.as_os_str().as_encoded_bytes().contains(&0)
        || command
            .args
            .iter()
            .any(|argument| argument.as_encoded_bytes().contains(&0))
        || command
            .current_dir
            .as_deref()
            .is_some_and(|path| path.as_os_str().as_encoded_bytes().contains(&0))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "contained process parameter contains NUL",
        ));
    }
    if command.env.iter().any(|(key, value)| {
        key.is_empty()
            || key.as_encoded_bytes().contains(&0)
            || key.as_encoded_bytes().contains(&b'=')
            || value
                .as_ref()
                .is_some_and(|value| value.as_encoded_bytes().contains(&0))
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "contained process environment key is empty, contains '=' or NUL, or value contains NUL",
        ));
    }
    validate_limits(command.limits)?;
    Ok(())
}

fn validate_limits(limits: ContainedProcessLimits) -> io::Result<()> {
    let invalid = limits
        .cpu_seconds
        .is_some_and(|value| !(1..=86_400).contains(&value))
        || limits
            .memory_bytes
            .is_some_and(|value| !(1024 * 1024..=1024_u64.pow(4)).contains(&value))
        || limits
            .file_size_bytes
            .is_some_and(|value| !(1..=1024_u64.pow(4)).contains(&value))
        || limits
            .open_files
            .is_some_and(|value| !(16..=1_048_576).contains(&value))
        || limits
            .active_processes
            .is_some_and(|value| !(1..=1_048_576).contains(&value));
    if invalid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "contained process limits are outside their closed bounds",
        ));
    }
    #[cfg(windows)]
    if limits.file_size_bytes.is_some() || limits.open_files.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows Job Objects do not provide file-size or open-file limits",
        ));
    }
    #[cfg(target_os = "macos")]
    if limits.memory_bytes.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "macOS cannot impose a useful RLIMIT_AS below the process-wide dyld mapping",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Instant};

    #[test]
    fn empty_program_and_nul_arguments_are_rejected_before_spawn() {
        assert_eq!(
            ContainedHeadlessCommand::new("")
                .spawn()
                .err()
                .expect("empty executable must fail")
                .kind(),
            io::ErrorKind::InvalidInput
        );
        let mut command = ContainedHeadlessCommand::new("unused");
        command.arg("bad\0arg");
        assert_eq!(
            command
                .spawn()
                .err()
                .expect("NUL argument must fail")
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unix_child_enters_its_own_process_group_before_exec() {
        let mut command = ContainedHeadlessCommand::new("/bin/sh");
        command.args(["-c", "test \"$$\" = \"$(ps -o pgid= -p $$ | tr -d ' ')\""]);
        let mut child = command.spawn().expect("spawn contained probe");
        assert!(child.take_stdout().is_none());
        assert!(child.take_stderr().is_none());
        let deadline = Instant::now() + Duration::from_secs(5);
        let exit = loop {
            if let Some(exit) = child.try_wait().expect("wait contained probe") {
                break exit;
            }
            assert!(Instant::now() < deadline, "contained probe timed out");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(exit, ProcessExit::Code(0));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn current_user_group_leader_can_be_adopted_by_exact_identity() {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        crate::process::configure_owned_command(&mut command).expect("configure process group");
        let mut child = command.spawn().expect("spawn adoptable process group");
        let identity = crate::process_observation::start_identity(child.id())
            .expect("adoptable root identity");
        assert!(
            crate::process::ProcessTreeGuard::adopt_group_leader(
                child.id(),
                "wrong-start-identity",
                8,
            )
            .is_err()
        );
        let mut group =
            crate::process::ProcessTreeGuard::adopt_group_leader(child.id(), &identity, 8)
                .expect("adopt exact current-user group");
        assert!(
            group
                .process_ids(8)
                .expect("group members")
                .contains(&child.id())
        );
        assert!(
            group.terminate().is_err(),
            "observation adoption cannot mutate"
        );
        drop(group);
        let mut group = crate::process::ProcessTreeGuard::adopt_group_leader_for_termination(
            child.id(),
            &identity,
            8,
        )
        .expect("retain exact termination authority");
        group.terminate().expect("terminate adopted test group");
        child.wait().expect("reap adopted test root");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unix_containment_inventory_is_group_complete_and_bounded() {
        let mut command = ContainedHeadlessCommand::new("/bin/sh");
        command.args(["-c", "sleep 30 & wait"]);
        let mut child = command.spawn().expect("spawn contained group");
        let deadline = Instant::now() + Duration::from_secs(5);
        let members = loop {
            let members = child
                .containment_members(8)
                .expect("inventory contained group");
            if members.process_ids.len() >= 2 {
                break members;
            }
            assert!(Instant::now() < deadline, "descendant did not join group");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(members.provider, "posix-process-group");
        assert!(!members.breakaway_prevented);
        assert!(members.process_ids.contains(&child.id()));
        assert!(child.containment_members(1).is_err());
        child
            .terminate_and_wait(Duration::from_secs(5))
            .expect("terminate contained group");
    }

    #[test]
    fn captured_output_probe() {
        println!("contained-stdout-probe");
        eprintln!("contained-stderr-probe");
    }

    #[test]
    fn captured_stdout_and_stderr_are_independent_send_readers() {
        fn assert_send<T: Send>() {}
        assert_send::<ContainedChildOutput>();

        let mut command = ContainedHeadlessCommand::new(
            std::env::current_exe().expect("resolve contained capture test executable"),
        );
        command.args([
            "--exact",
            "contained_process::tests::captured_output_probe",
            "--nocapture",
        ]);
        command.capture_output();
        let mut child = command.spawn().expect("spawn contained capture probe");
        let mut stdout = child.take_stdout().expect("captured stdout");
        let mut stderr = child.take_stderr().expect("captured stderr");
        assert!(child.take_stdout().is_none());
        assert!(child.take_stderr().is_none());

        let stdout_drain = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).expect("drain child stdout");
            bytes
        });
        let stderr_drain = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).expect("drain child stderr");
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let exit = loop {
            match child.try_wait().expect("wait capture probe") {
                Some(exit) => break exit,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => panic!("contained capture probe timed out"),
            }
        };
        assert_eq!(exit, ProcessExit::Code(0));
        let stdout = String::from_utf8(stdout_drain.join().expect("join stdout drain"))
            .expect("stdout is UTF-8");
        let stderr = String::from_utf8(stderr_drain.join().expect("join stderr drain"))
            .expect("stderr is UTF-8");
        assert!(stdout.contains("contained-stdout-probe"));
        assert!(stderr.contains("contained-stderr-probe"));
    }

    #[test]
    fn configured_stdio_probe() {
        if std::env::var_os("AGENTERM_CONTAINED_CONFIGURED_PROBE").is_some() {
            let mut stdin = String::new();
            std::io::stdin()
                .read_to_string(&mut stdin)
                .expect("read configured stdin");
            println!(
                "{stdin}|{}",
                std::env::var_os("AGENTERM_CONTAINED_REMOVED").is_none()
            );
            eprint!("configured-stderr");
            return;
        }

        let path = std::env::temp_dir().join(format!(
            "agenterm-contained-stderr-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        ));
        let mut command = ContainedHeadlessCommand::new(
            std::env::current_exe().expect("resolve contained configured test executable"),
        );
        command.args([
            "--exact",
            "contained_process::tests::configured_stdio_probe",
            "--nocapture",
        ]);
        command
            .env("AGENTERM_CONTAINED_CONFIGURED_PROBE", "1")
            .env("AGENTERM_CONTAINED_REMOVED", "present")
            .env_remove("AGENTERM_CONTAINED_REMOVED")
            .stdin_text("configured-stdin")
            .capture_output()
            .stderr_file(File::create(&path).expect("create redirected stderr"));
        let mut child = command.spawn().expect("spawn configured contained probe");
        let mut stdout = child.take_stdout().expect("captured stdout");
        assert!(child.take_stderr().is_none());
        let stdout_drain = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).expect("drain child stdout");
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let exit = loop {
            match child.try_wait().expect("wait configured probe") {
                Some(exit) => break exit,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => panic!("configured contained probe timed out"),
            }
        };
        assert_eq!(exit, ProcessExit::Code(0));
        let stdout = String::from_utf8(stdout_drain.join().expect("join stdout drain"))
            .expect("stdout is UTF-8");
        assert!(stdout.contains("configured-stdin|true"), "{stdout:?}");
        drop(command);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read redirected stderr"),
            "configured-stderr"
        );
        std::fs::remove_file(path).expect("remove redirected stderr");
    }

    #[test]
    fn piped_stdin_is_a_send_writer_and_drop_delivers_eof() {
        fn assert_send<T: Send>() {}
        assert_send::<ContainedChildInput>();

        if std::env::var_os("AGENTERM_CONTAINED_PIPE_PROBE").is_some() {
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .expect("read piped stdin through EOF");
            println!("pipe-bytes={}", bytes.len());
            eprintln!("pipe-stderr");
            return;
        }

        let mut command = ContainedHeadlessCommand::new(
            std::env::current_exe().expect("resolve contained pipe test executable"),
        );
        command.args([
            "--exact",
            "contained_process::tests::piped_stdin_is_a_send_writer_and_drop_delivers_eof",
            "--nocapture",
        ]);
        command
            .env("AGENTERM_CONTAINED_PIPE_PROBE", "1")
            .pipe_stdin()
            .capture_output();
        let mut child = command.spawn().expect("spawn contained pipe probe");
        let mut stdin = child.take_stdin().expect("piped stdin");
        assert!(child.take_stdin().is_none());
        stdin.write_all(b"first").expect("write first stdin chunk");
        stdin
            .write_all(b"second")
            .expect("write second stdin chunk");
        drop(stdin);

        let mut stdout = child.take_stdout().expect("captured stdout");
        let mut stderr = child.take_stderr().expect("captured stderr");
        let stdout_drain = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).expect("drain pipe stdout");
            bytes
        });
        let stderr_drain = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).expect("drain pipe stderr");
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let exit = loop {
            match child.try_wait().expect("wait pipe probe") {
                Some(exit) => break exit,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => panic!("contained pipe probe timed out"),
            }
        };
        assert_eq!(exit, ProcessExit::Code(0));
        let stdout = String::from_utf8(stdout_drain.join().expect("join pipe stdout"))
            .expect("pipe stdout is UTF-8");
        let stderr = String::from_utf8(stderr_drain.join().expect("join pipe stderr"))
            .expect("pipe stderr is UTF-8");
        assert!(stdout.contains("pipe-bytes=11"), "{stdout:?}");
        assert!(stderr.contains("pipe-stderr"), "{stderr:?}");
    }

    #[test]
    fn invalid_environment_is_rejected_before_spawn() {
        let mut command = ContainedHeadlessCommand::new("unused");
        command.env("BAD=KEY", "value");
        assert_eq!(
            command
                .spawn()
                .err()
                .expect("invalid environment must fail")
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn process_limit_bounds_fail_before_spawn() {
        for limits in [
            ContainedProcessLimits {
                cpu_seconds: Some(0),
                ..ContainedProcessLimits::default()
            },
            ContainedProcessLimits {
                memory_bytes: Some(1024),
                ..ContainedProcessLimits::default()
            },
            ContainedProcessLimits {
                open_files: Some(15),
                ..ContainedProcessLimits::default()
            },
        ] {
            let mut command = ContainedHeadlessCommand::new("unused");
            command.limits(limits);
            assert_eq!(
                command
                    .spawn()
                    .err()
                    .expect("invalid limits must fail")
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unix_limits_are_installed_before_the_first_user_instruction() {
        if std::env::var_os("AGENTERM_CONTAINED_LIMIT_PROBE").is_some() {
            println!(
                "{} {} {} {} {}",
                current_limit(libc::RLIMIT_CPU),
                current_limit(libc::RLIMIT_AS),
                current_limit(libc::RLIMIT_FSIZE),
                current_limit(libc::RLIMIT_NOFILE),
                current_limit(libc::RLIMIT_NPROC),
            );
            return;
        }

        let limits = ContainedProcessLimits {
            cpu_seconds: Some(120),
            #[cfg(target_os = "linux")]
            memory_bytes: Some(64 * 1024 * 1024 * 1024),
            #[cfg(target_os = "macos")]
            memory_bytes: None,
            file_size_bytes: Some(64 * 1024 * 1024),
            open_files: Some(4096),
            active_processes: Some(1024),
        };
        let mut command = ContainedHeadlessCommand::new(
            std::env::current_exe().expect("resolve contained limit test executable"),
        );
        command.args([
            "--exact",
            "contained_process::tests::unix_limits_are_installed_before_the_first_user_instruction",
            "--nocapture",
        ]);
        command
            .env("AGENTERM_CONTAINED_LIMIT_PROBE", "1")
            .limits(limits)
            .capture_output();
        let mut child = command.spawn().expect("spawn contained limit probe");
        let mut stdout = child.take_stdout().expect("capture limit stdout");
        let mut stderr = child.take_stderr().expect("capture limit stderr");
        let stdout_drain = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).expect("drain limit stdout");
            bytes
        });
        let stderr_drain = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).expect("drain limit stderr");
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let exit = loop {
            match child.try_wait().expect("wait limit probe") {
                Some(exit) => break exit,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => panic!("contained limit probe timed out"),
            }
        };
        let stderr = String::from_utf8(stderr_drain.join().expect("join limit stderr"))
            .expect("limit stderr is UTF-8");
        assert_eq!(exit, ProcessExit::Code(0), "{stderr}");
        let stdout = String::from_utf8(stdout_drain.join().expect("join limit stdout"))
            .expect("limit stdout is UTF-8");
        let values = stdout
            .lines()
            .find_map(|line| {
                let values = line
                    .split_ascii_whitespace()
                    .map(str::parse::<u64>)
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                (values.len() == 5).then_some(values)
            })
            .unwrap_or_else(|| panic!("missing limit probe line: {stdout:?}"));
        assert_eq!(values[0], limits.cpu_seconds.unwrap());
        if let Some(memory_bytes) = limits.memory_bytes {
            assert_eq!(values[1], memory_bytes);
        }
        assert_eq!(values[2], limits.file_size_bytes.unwrap());
        assert_eq!(values[3], limits.open_files.unwrap());
        assert_eq!(values[4], u64::from(limits.active_processes.unwrap()));
    }

    #[cfg(target_os = "linux")]
    type TestRlimitResource = libc::__rlimit_resource_t;
    #[cfg(target_os = "macos")]
    type TestRlimitResource = libc::c_int;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn current_limit(resource: TestRlimitResource) -> u64 {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(unsafe { libc::getrlimit(resource, &raw mut limit) }, 0);
        limit.rlim_cur
    }

    /// The frozen launch is a macOS primitive; other Unix targets must refuse
    /// before creating anything rather than fabricate an identity.
    #[cfg(target_os = "linux")]
    #[test]
    fn frozen_spawn_is_typed_unsupported_off_macos() {
        let command = ContainedHeadlessCommand::new("/bin/true");
        assert_eq!(
            command
                .spawn_suspended_frozen()
                .err()
                .expect("frozen spawn must be unsupported here")
                .kind(),
            io::ErrorKind::Unsupported
        );
    }

    /// `std::process::Command` swaps `environ` to the child's environment before
    /// `execvp`, so a slash-free program is resolved through the CHILD's `PATH`
    /// (with `_CS_PATH` when the child has none). The frozen launch must agree
    /// with `Command` on all three shapes instead of silently using the
    /// caller's `PATH`.
    #[cfg(target_os = "macos")]
    #[test]
    fn frozen_spawn_resolves_a_slash_free_program_like_command() {
        let directory = std::env::temp_dir().join(format!(
            "agenterm-frozen-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("create frozen PATH scratch");
        // A symlink, not a copy: a copied signed system binary loses its code
        // signature on Apple silicon and the kernel kills it with SIGKILL before
        // it can run. Executing through a symlink runs the signed target.
        let probe = directory.join("agenterm-frozen-probe");
        std::os::unix::fs::symlink("/bin/sh", &probe).expect("link PATH probe to the real shell");
        let probe_path = directory.to_str().expect("scratch path is UTF-8");

        // (a) reachable ONLY through the child's PATH.
        let mut command = ContainedHeadlessCommand::new("agenterm-frozen-probe");
        command.args(["-c", "exit 0"]).env("PATH", probe_path);
        let (mut child, identity) = command
            .spawn_suspended_frozen()
            .expect("frozen spawn resolves the child PATH");
        assert_eq!(identity.pid, child.id());
        wait_for_exit(&mut child);
        let mut control = std::process::Command::new("agenterm-frozen-probe");
        control
            .env("PATH", probe_path)
            .args(["-c", "exit 0"])
            .status()
            .expect("Command resolves the child PATH too");

        // (b) absent from the child's PATH -> the same NotFound as `Command`.
        let mut command = ContainedHeadlessCommand::new("agenterm-frozen-probe");
        command.env("PATH", "/nonexistent");
        let frozen_error = match command.spawn_suspended_frozen() {
            Ok(_) => panic!("an absent program must fail"),
            Err(error) => error,
        };
        assert_eq!(frozen_error.kind(), io::ErrorKind::NotFound);
        let mut control = std::process::Command::new("agenterm-frozen-probe");
        control.env("PATH", "/nonexistent");
        let control_error = match control.spawn() {
            Ok(_) => panic!("Command must agree the program is absent"),
            Err(error) => error,
        };
        assert_eq!(control_error.kind(), io::ErrorKind::NotFound);

        // (c) no child PATH -> both fall back to the platform default path.
        let mut command = ContainedHeadlessCommand::new("sh");
        command.env_remove("PATH").args(["-c", "exit 0"]);
        let (mut child, _identity) = command
            .spawn_suspended_frozen()
            .expect("frozen spawn uses the default path");
        wait_for_exit(&mut child);
        let mut control = std::process::Command::new("sh");
        control
            .env_remove("PATH")
            .args(["-c", "exit 0"])
            .status()
            .expect("Command uses the default path too");

        std::fs::remove_dir_all(&directory).expect("remove frozen PATH scratch");
    }

    #[cfg(target_os = "macos")]
    fn wait_for_exit(child: &mut ContainedChild) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait().expect("wait for frozen child") {
                Some(exit) => {
                    assert_eq!(exit, ProcessExit::Code(0));
                    return;
                }
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                None => panic!("frozen child timed out"),
            }
        }
    }

    /// A program that exits at once must still yield a frozen identity: the root
    /// is suspended before any user instruction, so the freeze cannot race the
    /// exit. A normal `spawn` plus a later read would lose this race.
    #[cfg(target_os = "macos")]
    #[test]
    fn frozen_spawn_freezes_identity_for_an_immediate_exit() {
        for round in 0..100 {
            let mut command = ContainedHeadlessCommand::new("/bin/sh");
            command.args(["-c", "exit 0"]);
            let (mut child, identity) = command
                .spawn_suspended_frozen()
                .expect("frozen spawn of an immediate exit");
            assert_eq!(identity.pid, child.id(), "round {round}");
            assert!(
                identity.start_identity.starts_with("macos-start-time:"),
                "round {round}: {}",
                identity.start_identity
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait().expect("wait frozen immediate exit") {
                    Some(_) => break,
                    None if Instant::now() < deadline => thread::sleep(Duration::from_millis(1)),
                    None => panic!("round {round}: frozen child did not exit"),
                }
            }
        }
    }

    /// While the child is still live the frozen identity must equal an
    /// independent observation of the same pid, byte for byte.
    #[cfg(target_os = "macos")]
    #[test]
    fn frozen_identity_matches_an_independent_observation() {
        let mut command = ContainedHeadlessCommand::new("/bin/sh");
        command.args(["-c", "sleep 0.3"]);
        let (mut child, identity) = command.spawn_suspended_frozen().expect("frozen spawn");
        assert_eq!(
            crate::process_observation::start_identity(identity.pid).expect("live frozen identity"),
            identity.start_identity
        );
        child
            .terminate_and_wait(Duration::from_secs(5))
            .expect("terminate frozen child");
    }

    /// Non-default limits cannot be installed by `posix_spawn`; the refusal must
    /// precede any side effect.
    #[cfg(target_os = "macos")]
    #[test]
    fn frozen_spawn_refuses_limits_before_creating_a_process() {
        let mut command = ContainedHeadlessCommand::new("/usr/bin/true");
        command.limits(ContainedProcessLimits {
            cpu_seconds: Some(1),
            ..ContainedProcessLimits::default()
        });
        assert_eq!(
            command
                .spawn_suspended_frozen()
                .err()
                .expect("limits must be refused")
                .kind(),
            io::ErrorKind::Unsupported
        );
    }

    /// The `posix_spawn` path must carry cwd, `env`, `env_remove`, argv and
    /// captured stdio, not silently drop them.
    #[cfg(target_os = "macos")]
    #[test]
    fn frozen_spawn_carries_cwd_environment_and_captured_output() {
        use std::io::Read as _;

        let directory = std::env::temp_dir().join(format!(
            "agenterm-frozen-cwd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("create frozen cwd scratch");
        let resolved = std::fs::canonicalize(&directory).expect("resolve frozen cwd scratch");

        let mut command = ContainedHeadlessCommand::new("/bin/sh");
        command
            .args([
                "-c",
                "pwd; printf '%s|%s' \"$AGENTERM_FROZEN_SET\" \"${AGENTERM_FROZEN_GONE-unset}\"",
            ])
            .current_dir(&resolved)
            .env("AGENTERM_FROZEN_SET", "present")
            .env("AGENTERM_FROZEN_GONE", "present")
            .env_remove("AGENTERM_FROZEN_GONE")
            .capture_output();
        let (mut child, identity) = command.spawn_suspended_frozen().expect("frozen spawn");
        assert_eq!(identity.pid, child.id());
        let mut stdout = child.take_stdout().expect("captured frozen stdout");
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).expect("drain frozen stdout");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait().expect("wait frozen probe") {
                Some(exit) => {
                    assert_eq!(exit, ProcessExit::Code(0));
                    break;
                }
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                None => panic!("frozen probe timed out"),
            }
        }
        let stdout = String::from_utf8(bytes).expect("frozen stdout is UTF-8");
        assert_eq!(stdout, format!("{}\npresent|unset", resolved.display()));
        std::fs::remove_dir_all(&directory).expect("remove frozen cwd scratch");
    }

    /// `stdin_text` must reach a frozen child exactly like a `Command` root.
    #[cfg(target_os = "macos")]
    #[test]
    fn frozen_spawn_feeds_stdin_text() {
        use std::io::Read as _;

        let mut command = ContainedHeadlessCommand::new("/bin/cat");
        command.stdin_text("frozen-stdin").capture_output();
        let (mut child, _identity) = command.spawn_suspended_frozen().expect("frozen spawn");
        let mut stdout = child.take_stdout().expect("captured frozen stdout");
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).expect("drain frozen stdout");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait().expect("wait frozen cat") {
                Some(_) => break,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                None => panic!("frozen cat timed out"),
            }
        }
        assert_eq!(String::from_utf8(bytes).expect("UTF-8"), "frozen-stdin");
    }
}

/// Differential evidence that the frozen launch resolves a program the way
/// `std::process::Command` (Apple libc `execvp`) does. Each shape is run through
/// both paths; the exit code, stdout, or the typed error kind must agree.
///
/// The fixtures are real: a "permission denied" candidate is a file or directory
/// ***named exactly like the program***, not a directory that merely lacks the
/// program. A first candidate that does not exist at all only ever exercises
/// `ENOENT`-continues, which is a different rule.
#[cfg(all(test, target_os = "macos"))]
mod frozen_program_resolution_tests {
    use super::*;
    use std::{
        path::{Path, PathBuf},
        thread,
        time::Instant,
    };

    /// A scratch tree that removes itself even when an assertion panics.
    struct Fixture {
        base: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "agenterm-frozen-resolve-{}-{}-{}",
                std::process::id(),
                tag,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system clock after epoch")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&base).expect("create fixture base");
            Self { base }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.base.join(relative)
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.path(relative);
            std::fs::create_dir_all(&path).expect("create fixture directory");
            path
        }

        fn executable(&self, relative: &str, contents: &str) -> PathBuf {
            self.write(relative, contents, 0o755)
        }

        fn not_executable(&self, relative: &str, contents: &str) -> PathBuf {
            self.write(relative, contents, 0o644)
        }

        fn write(&self, relative: &str, contents: &str, mode: u32) -> PathBuf {
            let path = self.path(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create fixture parent");
            }
            std::fs::write(&path, contents).expect("write fixture");
            self.chmod(&path, mode);
            path
        }

        fn chmod(&self, path: &Path, mode: u32) {
            let mut permissions = std::fs::metadata(path).expect("fixture meta").permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, mode);
            std::fs::set_permissions(path, permissions).expect("chmod fixture");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Restore traversal before removing, so a deliberately unsearchable
            // ancestor cannot make cleanup fail.
            self.chmod(&self.base.join("locked"), 0o755);
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn frozen(spec: &ContainedHeadlessCommand) -> io::Result<(Option<i32>, String)> {
        let (mut child, _identity) = spec.spawn_suspended_frozen()?;
        let mut stdout = child.take_stdout().expect("captured frozen stdout");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut stdout, &mut bytes).expect("drain frozen stdout");
        let deadline = Instant::now() + Duration::from_secs(10);
        let exit = loop {
            match child.try_wait().expect("wait frozen child") {
                Some(exit) => break exit,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                None => panic!("frozen child timed out"),
            }
        };
        let code = match exit {
            ProcessExit::Code(code) => Some(code),
            _ => None,
        };
        Ok((code, String::from_utf8_lossy(&bytes).into_owned()))
    }

    fn control(command: &mut std::process::Command) -> io::Result<(Option<i32>, String)> {
        let output = command.output()?;
        Ok((
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ))
    }

    fn assert_same(
        label: &str,
        frozen_outcome: io::Result<(Option<i32>, String)>,
        control_outcome: io::Result<(Option<i32>, String)>,
    ) {
        match (frozen_outcome, control_outcome) {
            (Ok(frozen_outcome), Ok(control_outcome)) => assert_eq!(
                frozen_outcome, control_outcome,
                "{label}: frozen and Command must agree"
            ),
            (Err(frozen_error), Err(control_error)) => assert_eq!(
                frozen_error.kind(),
                control_error.kind(),
                "{label}: frozen `{frozen_error}` vs Command `{control_error}`"
            ),
            (frozen_outcome, control_outcome) => {
                panic!("{label}: frozen={frozen_outcome:?} control={control_outcome:?}")
            }
        }
    }

    /// Compare one search over `PATH` between both launchers.
    fn assert_search(
        fixture: &Fixture,
        label: &str,
        program: &str,
        path: &str,
        current_dir: Option<&Path>,
    ) {
        let mut spec = ContainedHeadlessCommand::new(program);
        spec.env("PATH", path).capture_output();
        if let Some(directory) = current_dir {
            spec.current_dir(directory);
        }
        let mut reference = std::process::Command::new(program);
        reference.env("PATH", path);
        if let Some(directory) = current_dir {
            reference.current_dir(directory);
        }
        assert_same(
            &format!("{label} ({})", fixture.base.display()),
            frozen(&spec),
            control(&mut reference),
        );
    }

    fn running_as_root() -> bool {
        // SAFETY: `geteuid` takes no arguments and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn frozen_spawn_matches_command_program_resolution() {
        let fixture = Fixture::new("resolve");

        // (A) An explicit path to a file with no shebang: `execvp` retries it
        //     through /bin/sh and `$0` is the path as written.
        let plain = fixture.executable("plain", "echo \"plain:$0:$1\"\nexit 7\n");
        let mut spec = ContainedHeadlessCommand::new(&plain);
        spec.arg("A").capture_output();
        let mut reference = std::process::Command::new(&plain);
        reference.arg("A");
        assert_same("explicit-enoexec", frozen(&spec), control(&mut reference));

        // (B)/(C) A relative and an empty PATH element belong to the child's cwd.
        let mut spec = ContainedHeadlessCommand::new("plain");
        spec.arg("B")
            .current_dir(&fixture.base)
            .env("PATH", ".")
            .capture_output();
        let output = std::process::Command::new("plain")
            .arg("B")
            .current_dir(&fixture.base)
            .env("PATH", ".")
            .output()
            .expect("control relative PATH");
        assert_same(
            "relative-child-path",
            frozen(&spec),
            Ok((
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).into_owned(),
            )),
        );
        let mut spec = ContainedHeadlessCommand::new("plain");
        spec.arg("C")
            .current_dir(&fixture.base)
            .env("PATH", "")
            .capture_output();
        let output = std::process::Command::new("plain")
            .arg("C")
            .current_dir(&fixture.base)
            .env("PATH", "")
            .output()
            .expect("control empty PATH");
        assert_same(
            "empty-path-element",
            frozen(&spec),
            Ok((
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).into_owned(),
            )),
        );

        // (D) A candidate that IS a non-executable file named like the program,
        //     then a candidate that IS a directory named like the program, then a
        //     real one: both earlier candidates must be skipped for the right
        //     reason (EACCES) and the third used. `Command` is the oracle.
        fixture.not_executable("denied/entry", "x");
        fixture.dir("directory/entry");
        fixture.executable("valid/entry", "#!/bin/sh\necho entry:$1\nexit 3\n");
        let path = format!(
            "{}:{}:{}",
            fixture.path("denied").display(),
            fixture.path("directory").display(),
            fixture.path("valid").display()
        );
        assert_search(
            &fixture,
            "eacces-and-directory-then-valid",
            "entry",
            &path,
            None,
        );

        // (E) The same two bad candidates with no valid successor: `execvp`
        //     prefers the remembered EACCES over ENOENT.
        let path = format!(
            "{}:{}",
            fixture.path("denied").display(),
            fixture.path("directory").display()
        );
        assert_search(&fixture, "all-eacces", "entry", &path, None);

        // (F) A first candidate whose shebang interpreter does not exist reports
        //     ENOENT (not ENOEXEC, so no /bin/sh retry) and the search continues
        //     to the NEXT CANDIDATE OF THE SAME NAME, which must be real.
        fixture.executable(
            "missing-interp/broken",
            "#!/nonexistent/interpreter\nexit 9\n",
        );
        fixture.executable("good/broken", "#!/bin/sh\necho good:$1\nexit 4\n");
        let path = format!(
            "{}:{}",
            fixture.path("missing-interp").display(),
            fixture.path("good").display()
        );
        assert_search(
            &fixture,
            "missing-interpreter-then-valid",
            "broken",
            &path,
            None,
        );

        // (G) An ancestor without search permission is NOT a permission error for
        //     a PATH search: Apple's `execvp` normalises the kernel's EACCES on an
        //     unsearchable ancestor to ENOENT instead of remembering it, so the
        //     search continues and an all-unsearchable search reports NotFound
        //     (only a reachable-but-not-executable component is remembered as
        //     EACCES). An EXPLICIT path is not a search, so it keeps EACCES.
        //     A root user bypasses the check, so only assert as a normal user.
        if !running_as_root() {
            fixture.dir("locked/inner");
            fixture.executable("locked/inner/locked-prog", "#!/bin/sh\nexit 1\n");
            fixture.executable("ok/ok-prog", "#!/bin/sh\nexit 2\n");
            fixture.chmod(&fixture.path("locked"), 0o000);
            let path = format!(
                "{}:{}",
                fixture.path("locked/inner").display(),
                fixture.path("ok").display()
            );
            assert_search(
                &fixture,
                "locked-ancestor-then-valid",
                "locked-prog",
                &path,
                None,
            );
            let path = fixture.path("locked/inner").display().to_string();
            assert_search(&fixture, "locked-ancestor-only", "locked-prog", &path, None);
            // An EXPLICIT path under the unsearchable ancestor keeps the kernel's
            // own EACCES (it is not a search, so no ENOENT normalization).
            let explicit = fixture.path("locked/inner/locked-prog");
            let mut spec = ContainedHeadlessCommand::new(&explicit);
            spec.capture_output();
            let mut reference = std::process::Command::new(&explicit);
            assert_same(
                "explicit-locked-ancestor",
                frozen(&spec),
                control(&mut reference),
            );
            fixture.chmod(&fixture.path("locked"), 0o755);
        }

        // (H) Nothing exists in the effective PATH.
        assert_search(
            &fixture,
            "absent",
            "definitely-absent-prog",
            "/nonexistent",
            None,
        );

        // (I) Explicit non-executable path and explicit directory.
        for name in ["plain-nonexec", "plain-dir"] {
            let explicit = match name {
                "plain-nonexec" => fixture.not_executable("plain-nonexec", "x"),
                _ => fixture.dir("plain-dir"),
            };
            let mut spec = ContainedHeadlessCommand::new(&explicit);
            spec.capture_output();
            let mut reference = std::process::Command::new(&explicit);
            assert_same(name, frozen(&spec), control(&mut reference));
        }
    }
}

/// The public `terminate_and_wait` bound must be real: a root that does not
/// become reapable still returns inside the deadline. `SIGKILL` cannot be
/// blocked or ignored, so the shape a caller can actually construct is a
/// deadline that has already expired.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod terminate_bounds_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn terminate_and_wait_returns_inside_an_expired_deadline() {
        let mut command = ContainedHeadlessCommand::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let mut child = command.spawn().expect("spawn bounded-terminate probe");
        let started = Instant::now();
        let outcome = child.terminate_and_wait(Duration::from_millis(0));
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(3),
            "terminate_and_wait must be bounded, took {elapsed:?}"
        );
        if let Err(error) = outcome {
            assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        }
        // A second call with a real deadline must still reap it cleanly.
        child
            .terminate_and_wait(Duration::from_secs(5))
            .expect("a real deadline reaps the root");
    }
}
