use std::{
    io::{self, Read, Write as _},
    process::{Child, ChildStderr, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    contained_process::{ContainedHeadlessCommand, ContainedOutput},
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
    command
        .args(&spec.args)
        .stdin(if spec.stdin_text.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
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
    let mut child = command.spawn()?;
    let tree = match ProcessTreeGuard::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(error));
        }
    };
    if let (Some(text), Some(mut stdin)) = (&spec.stdin_text, child.stdin.take()) {
        let text = text.clone();
        thread::spawn(move || {
            let _ = stdin.write_all(&text);
        });
    }
    Ok(ContainedChild { child, tree })
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
