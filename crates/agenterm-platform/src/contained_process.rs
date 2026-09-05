//! Owned headless child processes contained before their first user instruction.

use std::{
    ffi::OsString,
    io::{self, Read},
    path::PathBuf,
    time::Duration,
};

pub use crate::contract::process_spawn::ProcessExit;

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
    pub(crate) capture_output: bool,
}

impl ContainedHeadlessCommand {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
            capture_output: false,
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

    /// Captures stdout and stderr through independent owned streams.
    ///
    /// Callers should drain both streams concurrently before waiting for a
    /// verbose child, so neither bounded operating-system pipe can block the
    /// other stream or the child process.
    pub fn capture_output(&mut self) -> &mut Self {
        self.capture_output = true;
        self
    }

    pub fn spawn(&self) -> io::Result<ContainedChild> {
        validate(self)?;
        crate::selected::contained_process::spawn(self).map(ContainedChild)
    }
}

/// Exact root-process ownership plus its native descendant-containment owner.
pub struct ContainedChild(crate::selected::contained_process::ContainedChild);

/// One captured child output stream.
///
/// The stream is owned and `Send`, so stdout and stderr can be drained on two
/// independent worker threads.
pub struct ContainedChildOutput(crate::selected::contained_process::ContainedChildOutput);

impl Read for ContainedChildOutput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
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

    /// Takes the captured stdout stream, when output capture was requested.
    pub fn take_stdout(&mut self) -> Option<ContainedChildOutput> {
        self.0.take_stdout().map(ContainedChildOutput)
    }

    /// Takes the captured stderr stream, when output capture was requested.
    pub fn take_stderr(&mut self) -> Option<ContainedChildOutput> {
        self.0.take_stderr().map(ContainedChildOutput)
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
}
