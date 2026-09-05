use std::{
    io::{self, Read},
    process::{Child, ChildStderr, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    contained_process::ContainedHeadlessCommand,
    contract::process_spawn::ProcessExit,
    process::{ProcessTreeGuard, configure_owned_command},
};

pub struct ContainedChild {
    child: Child,
    tree: ProcessTreeGuard,
}

pub enum ContainedChildOutput {
    Stdout(ChildStdout),
    Stderr(ChildStderr),
}

impl Read for ContainedChildOutput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(stream) => stream.read(buffer),
            Self::Stderr(stream) => stream.read(buffer),
        }
    }
}

pub(crate) fn spawn(spec: &ContainedHeadlessCommand) -> io::Result<ContainedChild> {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args).stdin(Stdio::null());
    if spec.capture_output {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    if let Some(directory) = &spec.current_dir {
        command.current_dir(directory);
    }
    configure_owned_command(&mut command).map_err(io::Error::other)?;
    let mut child = command.spawn()?;
    let tree = match ProcessTreeGuard::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(error));
        }
    };
    Ok(ContainedChild { child, tree })
}

impl ContainedChild {
    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ProcessExit>> {
        self.child.try_wait().map(|status| {
            status
                .as_ref()
                .map(crate::process_spawn::classify_exit_status)
        })
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ContainedChildOutput> {
        self.child.stdout.take().map(ContainedChildOutput::Stdout)
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ContainedChildOutput> {
        self.child.stderr.take().map(ContainedChildOutput::Stderr)
    }

    pub(crate) fn terminate_and_wait(&mut self, timeout: Duration) -> io::Result<()> {
        let tree_result = self.tree.terminate().map_err(io::Error::other);
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait()? {
                Some(_) => return tree_result,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                None => {
                    self.child.kill()?;
                    self.child.wait()?;
                    return tree_result;
                }
            }
        }
    }
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        let _ = self.tree.terminate();
    }
}
