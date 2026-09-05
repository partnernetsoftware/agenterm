//! Owned headless child processes contained before their first user instruction.

use std::{ffi::OsString, io, path::PathBuf, time::Duration};

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
}

impl ContainedHeadlessCommand {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
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

    pub fn spawn(&self) -> io::Result<ContainedChild> {
        validate(self)?;
        crate::selected::contained_process::spawn(self).map(ContainedChild)
    }
}

/// Exact root-process ownership plus its native descendant-containment owner.
pub struct ContainedChild(crate::selected::contained_process::ContainedChild);

impl ContainedChild {
    #[must_use]
    pub fn id(&self) -> u32 {
        self.0.id()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ProcessExit>> {
        self.0.try_wait()
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
    #[cfg(any(target_os = "linux", target_os = "macos"))]
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
}
