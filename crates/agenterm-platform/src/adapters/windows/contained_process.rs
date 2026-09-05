use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read},
    os::windows::{
        ffi::OsStrExt as _,
        io::{AsHandle as _, AsRawHandle as _, BorrowedHandle, FromRawHandle as _, OwnedHandle},
    },
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE},
    System::{
        Pipes::CreatePipe,
        Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
            InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread,
            STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
        },
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
    stdout: Option<ContainedChildOutput>,
    stderr: Option<ContainedChildOutput>,
    exit: Option<ProcessExit>,
}

pub struct ContainedChildOutput(File);

impl Read for ContainedChildOutput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
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
    let mut capture = spec.capture_output.then(CaptureStdio::new).transpose()?;
    let mut raw_inherited = capture
        .as_ref()
        .map(CaptureStdio::raw_child_handles)
        .unwrap_or_default();
    let mut attributes = if raw_inherited.is_empty() {
        None
    } else {
        Some(AttributeList::with_handle_list(&mut raw_inherited)?)
    };
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    if let Some(capture) = &capture {
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = capture.stdin.as_raw_handle();
        startup.StartupInfo.hStdOutput = capture.stdout_write.as_raw_handle();
        startup.StartupInfo.hStdError = capture.stderr_write.as_raw_handle();
        startup.lpAttributeList = attributes
            .as_mut()
            .expect("captured stdio has an attribute list")
            .raw;
    }
    let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let borrowed = raw_inherited
        .iter()
        .map(|handle| unsafe { BorrowedHandle::borrow_raw(*handle) })
        .collect::<Vec<_>>();
    let created = crate::process_spawn::with_inheritable_handles(borrowed.as_slice(), || unsafe {
        let ok = CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            i32::from(!raw_inherited.is_empty()),
            CREATE_NO_WINDOW
                | CREATE_SUSPENDED
                | extra_flags
                | if raw_inherited.is_empty() {
                    0
                } else {
                    EXTENDED_STARTUPINFO_PRESENT
                },
            std::ptr::null(),
            directory
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            &startup.StartupInfo,
            &raw mut information,
        );
        let error = if ok == 0 { GetLastError() } else { 0 };
        (ok, error)
    });
    let (ok, native_error) = match created {
        Ok(result) => result,
        Err(error) => {
            cleanup_information(information);
            return Err(AttemptError::io(error));
        }
    };
    if ok == 0 {
        return Err(AttemptError::io(io::Error::from_raw_os_error(
            native_error as i32,
        )));
    }
    let handles = CreatedHandles(information);
    let (stdout, stderr) = capture
        .take()
        .map(CaptureStdio::into_parent_outputs)
        .unwrap_or((None, None));
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
        stdout,
        stderr,
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

    pub(crate) fn take_stdout(&mut self) -> Option<ContainedChildOutput> {
        self.stdout.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ContainedChildOutput> {
        self.stderr.take()
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

fn cleanup_information(information: PROCESS_INFORMATION) {
    if information.hProcess.is_null() {
        return;
    }
    let handles = CreatedHandles(information);
    abort_raw_suspended(&handles);
}

struct CaptureStdio {
    stdin: OwnedHandle,
    stdout_read: OwnedHandle,
    stdout_write: OwnedHandle,
    stderr_read: OwnedHandle,
    stderr_write: OwnedHandle,
}

impl CaptureStdio {
    fn new() -> io::Result<Self> {
        let stdin = File::open("NUL")?.into();
        let (stdout_read, stdout_write) = pipe()?;
        let (stderr_read, stderr_write) = pipe()?;
        Ok(Self {
            stdin,
            stdout_read,
            stdout_write,
            stderr_read,
            stderr_write,
        })
    }

    fn raw_child_handles(&self) -> Vec<HANDLE> {
        vec![
            self.stdin.as_raw_handle(),
            self.stdout_write.as_raw_handle(),
            self.stderr_write.as_raw_handle(),
        ]
    }

    fn into_parent_outputs(self) -> (Option<ContainedChildOutput>, Option<ContainedChildOutput>) {
        let Self {
            stdin,
            stdout_read,
            stdout_write,
            stderr_read,
            stderr_write,
        } = self;
        drop(stdin);
        drop(stdout_write);
        drop(stderr_write);
        (
            Some(ContainedChildOutput(File::from(stdout_read))),
            Some(ContainedChildOutput(File::from(stderr_read))),
        )
    }
}

fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    if unsafe { CreatePipe(&raw mut read, &raw mut write, std::ptr::null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    })
}

struct AttributeList {
    _storage: Vec<usize>,
    raw: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl AttributeList {
    fn with_handle_list(handles: &mut [HANDLE]) -> io::Result<Self> {
        let mut bytes = 0;
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &raw mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        let raw = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(raw, 1, 0, &raw mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let list = Self {
            _storage: storage,
            raw,
        };
        let handle_bytes = handles
            .len()
            .checked_mul(std::mem::size_of::<HANDLE>())
            .ok_or_else(|| io::Error::other("contained stdio handle list size overflow"))?;
        if unsafe {
            UpdateProcThreadAttribute(
                list.raw,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_mut_ptr().cast(),
                handle_bytes,
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(list)
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.raw) };
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
