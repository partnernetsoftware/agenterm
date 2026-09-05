use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::{
        ffi::OsStrExt as _,
        io::{AsHandle as _, AsRawHandle as _},
    },
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError},
    System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW,
        PROCESS_INFORMATION, ResumeThread, STARTUPINFOW,
    },
};

use crate::{
    contained_process::ContainedHeadlessCommand,
    contract::process_spawn::ProcessExit,
    process_containment::{ProcessContainment, ProcessContainmentOptions},
    process_reference::{ProcessReference, ProcessWait},
};

pub struct ContainedChild {
    process: ProcessReference,
    containment: ProcessContainment,
    exit: Option<ProcessExit>,
}

pub(crate) fn spawn(spec: &ContainedHeadlessCommand) -> io::Result<ContainedChild> {
    let containment = ProcessContainment::create(
        None,
        ProcessContainmentOptions {
            terminate_on_last_close: true,
            ..ProcessContainmentOptions::default()
        },
    )
    .map_err(io::Error::other)?;
    match spawn_suspended_into(spec, containment, 0) {
        Ok(child) => Ok(child),
        Err(AttemptError {
            assignment_denied: true,
            ..
        }) => {
            let containment = ProcessContainment::create(
                None,
                ProcessContainmentOptions {
                    terminate_on_last_close: true,
                    ..ProcessContainmentOptions::default()
                },
            )
            .map_err(io::Error::other)?;
            spawn_suspended_into(spec, containment, CREATE_BREAKAWAY_FROM_JOB)
                .map_err(|failure| failure.error)
        }
        Err(failure) => Err(failure.error),
    }
}

fn spawn_suspended_into(
    spec: &ContainedHeadlessCommand,
    containment: ProcessContainment,
    extra_flags: u32,
) -> Result<ContainedChild, AttemptError> {
    let application = nul_terminated(spec.program.as_os_str())?;
    let mut command_line = windows_command_line(&spec.program, &spec.args)?;
    let directory = spec
        .current_dir
        .as_deref()
        .map(|path| nul_terminated(path.as_os_str()))
        .transpose()?;
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW | CREATE_SUSPENDED | extra_flags,
            std::ptr::null(),
            directory
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            &startup,
            &raw mut information,
        )
    };
    if ok == 0 {
        return Err(AttemptError::io(io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32,
        )));
    }
    let handles = CreatedHandles(information);
    let process = match ProcessReference::duplicate_from(handles.process()) {
        Ok(process) => process,
        Err(error) => {
            abort_raw_suspended(&handles);
            return Err(AttemptError::io(error));
        }
    };
    if let Err(error) = containment.assign(&process) {
        let assignment_denied = error.native_code() == Some(5);
        abort_suspended(&process);
        return Err(AttemptError {
            error: io::Error::other(error),
            assignment_denied,
        });
    }
    if unsafe { ResumeThread(handles.thread().as_raw_handle()) } == u32::MAX {
        let error = io::Error::from_raw_os_error(unsafe { GetLastError() } as i32);
        let _ = containment.terminate(1);
        let _ = process.wait_for_exit(None);
        return Err(AttemptError::io(error));
    }
    Ok(ContainedChild {
        process,
        containment,
        exit: None,
    })
}

struct AttemptError {
    error: io::Error,
    assignment_denied: bool,
}

impl AttemptError {
    fn io(error: io::Error) -> Self {
        Self {
            error,
            assignment_denied: false,
        }
    }
}

impl From<io::Error> for AttemptError {
    fn from(error: io::Error) -> Self {
        Self::io(error)
    }
}

impl ContainedChild {
    pub(crate) fn id(&self) -> u32 {
        self.process.id()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ProcessExit>> {
        if let Some(exit) = self.exit {
            return Ok(Some(exit));
        }
        if self.process.wait_for_exit(Some(Duration::ZERO))? == ProcessWait::TimedOut {
            return Ok(None);
        }
        let raw = crate::process_reference::exit_code_handle(self.process.as_handle())?;
        let exit = ProcessExit::Code(raw as i32);
        self.exit = Some(exit);
        Ok(Some(exit))
    }

    pub(crate) fn terminate_and_wait(&mut self, timeout: Duration) -> io::Result<()> {
        let root_exited = self.try_wait()?.is_some();
        self.containment.terminate(1).map_err(io::Error::other)?;
        if !root_exited && self.process.wait_for_exit(Some(timeout))? == ProcessWait::TimedOut {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "contained process did not exit before deadline",
            ));
        }
        let _ = self.try_wait()?;
        Ok(())
    }
}

fn abort_suspended(process: &ProcessReference) {
    let _ = process.terminate(crate::process_control::TerminationMode::Forceful);
    let _ = process.wait_for_exit(None);
}

fn abort_raw_suspended(handles: &CreatedHandles) {
    use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};

    unsafe {
        TerminateProcess(handles.process().as_raw_handle(), 1);
        WaitForSingleObject(handles.process().as_raw_handle(), u32::MAX);
    }
}

fn nul_terminated(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut wide = value.encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "contained process parameter contains NUL",
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn windows_command_line(program: &std::path::Path, args: &[OsString]) -> io::Result<Vec<u16>> {
    let mut line = Vec::new();
    push_argument(&mut line, program.as_os_str(), true)?;
    for argument in args {
        line.push(b' ' as u16);
        push_argument(&mut line, argument, false)?;
    }
    line.push(0);
    Ok(line)
}

fn push_argument(output: &mut Vec<u16>, argument: &OsStr, force_quote: bool) -> io::Result<()> {
    let units = argument.encode_wide().collect::<Vec<_>>();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "contained process argument contains NUL",
        ));
    }
    let quote = force_quote
        || units.is_empty()
        || units.iter().any(|unit| {
            *unit == u16::from(b' ') || *unit == u16::from(b'\t') || *unit == u16::from(b'"')
        });
    if !quote {
        output.extend(units);
        return Ok(());
    }
    output.push(b'"' as u16);
    let mut backslashes = 0;
    for unit in units {
        if unit == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
        if unit == b'"' as u16 {
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes + 1));
        }
        output.push(unit);
        backslashes = 0;
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

struct CreatedHandles(PROCESS_INFORMATION);

impl CreatedHandles {
    fn process(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(self.0.hProcess) }
    }

    fn thread(&self) -> std::os::windows::io::BorrowedHandle<'_> {
        unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(self.0.hThread) }
    }
}

impl Drop for CreatedHandles {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0.hThread);
            CloseHandle(self.0.hProcess);
        }
    }
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        let _ = self.containment.terminate(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_containment::ProcessContainmentErrorKind;
    use std::{thread, time::Instant};

    const FIRST_INSTRUCTION_JOB: &str = r"Local\agenterm-contained-first-instruction-test";

    fn text(units: Vec<u16>) -> String {
        String::from_utf16(&units[..units.len() - 1]).expect("valid test UTF-16")
    }

    #[test]
    fn command_line_preserves_empty_quoted_and_wide_arguments() {
        let args = vec![
            OsString::from(""),
            OsString::from("two words"),
            OsString::from(r#"say "hello""#),
            OsString::from("中文"),
            OsString::from(r"C:\tail\"),
        ];
        assert_eq!(
            text(windows_command_line(std::path::Path::new("C:\\app.exe"), &args).unwrap()),
            r#""C:\app.exe" "" "two words" "say \"hello\"" 中文 C:\tail\"#
        );
    }

    #[test]
    fn first_child_instruction_observes_the_exact_containment_job() {
        match ProcessContainment::open(FIRST_INSTRUCTION_JOB) {
            Ok(containment) => {
                let process = ProcessReference::open(std::process::id())
                    .expect("retain contained child identity");
                assert!(
                    containment
                        .contains(&process)
                        .expect("query exact child membership"),
                    "the child ran before it belonged to the expected Job"
                );
                return;
            }
            Err(error) if error.kind() == ProcessContainmentErrorKind::NotFound => {}
            Err(error) => panic!("probe containment open failed: {error}"),
        }

        let containment = ProcessContainment::create(
            Some(FIRST_INSTRUCTION_JOB),
            ProcessContainmentOptions {
                terminate_on_last_close: true,
                ..ProcessContainmentOptions::default()
            },
        )
        .expect("create probe containment");
        let mut spec = ContainedHeadlessCommand::new(
            std::env::current_exe().expect("resolve test executable"),
        );
        spec.args([
            "--exact",
            "selected::contained_process::tests::first_child_instruction_observes_the_exact_containment_job",
            "--nocapture",
        ]);
        let mut child = spawn_suspended_into(&spec, containment, 0)
            .map_err(|failure| failure.error)
            .expect("spawn exact contained probe");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait().expect("wait contained probe") {
                Some(ProcessExit::Code(0)) => break,
                Some(exit) => panic!("contained probe failed: {exit:?}"),
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => panic!("contained probe timed out"),
            }
        }
    }
}
