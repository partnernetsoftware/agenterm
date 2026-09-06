use std::{
    io::{self, Read, Write},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    contained_process::{
        ContainedHeadlessCommand, ContainedInput, ContainedOutput, ContainedProcessLimits,
    },
    contract::process_spawn::ProcessExit,
    process::{ProcessTreeGuard, configure_owned_command},
};

pub struct ContainedChild {
    child: Child,
    tree: ProcessTreeGuard,
    stdin: Option<ContainedChildInput>,
}

pub enum ContainedChildOutput {
    Stdout(ChildStdout),
    Stderr(ChildStderr),
}

pub struct ContainedChildInput(ChildStdin);

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
        match self {
            Self::Stdout(stream) => stream.read(buffer),
            Self::Stderr(stream) => stream.read(buffer),
        }
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
            let _ = child.kill();
            let _ = child.wait();
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
        ContainedInput::Pipe => child.stdin.take().map(ContainedChildInput),
        ContainedInput::Null => None,
    };
    Ok(ContainedChild { child, tree, stdin })
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
        self.child.id()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ProcessExit>> {
        self.child.try_wait().map(|status| {
            status
                .as_ref()
                .map(crate::process_spawn::classify_exit_status)
        })
    }

    pub(crate) fn containment_process_ids(&self, max_members: usize) -> io::Result<Vec<u32>> {
        self.tree.process_ids(max_members).map_err(io::Error::other)
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ContainedChildOutput> {
        self.child.stdout.take().map(ContainedChildOutput::Stdout)
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ContainedChildOutput> {
        self.child.stderr.take().map(ContainedChildOutput::Stderr)
    }

    pub(crate) fn take_stdin(&mut self) -> Option<ContainedChildInput> {
        self.stdin.take()
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
