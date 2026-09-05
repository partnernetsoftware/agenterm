use std::{
    cmp::Ordering as CmpOrdering,
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{self, Read, Write},
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
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CREATE_SUSPENDED,
            CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
            EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
            LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
            ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
        },
    },
};

use crate::{
    contained_process::{ContainedHeadlessCommand, ContainedInput, ContainedOutput},
    contract::process_spawn::ProcessExit,
    process_containment::{ProcessContainment, ProcessContainmentOptions},
    process_reference::{ProcessReference, ProcessWait},
};

pub struct ContainedChild {
    process: ProcessReference,
    containment: ProcessContainment,
    stdin: Option<ContainedChildInput>,
    stdout: Option<ContainedChildOutput>,
    stderr: Option<ContainedChildOutput>,
    exit: Option<ProcessExit>,
}

pub struct ContainedChildOutput(File);

pub struct ContainedChildInput(File);

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
    let mut environment = environment_block(spec)?;
    let stdio = PreparedStdio::new(spec)?;
    let mut raw_inherited = stdio.raw_child_handles();
    let attributes = AttributeList::with_handle_list(&mut raw_inherited)?;
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdio.stdin_child.as_raw_handle();
    startup.StartupInfo.hStdOutput = stdio.stdout_child.as_raw_handle();
    startup.StartupInfo.hStdError = stdio.stderr_child.as_raw_handle();
    startup.lpAttributeList = attributes.raw;
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
            1,
            CREATE_NO_WINDOW
                | CREATE_SUSPENDED
                | extra_flags
                | EXTENDED_STARTUPINFO_PRESENT
                | if environment.is_some() {
                    CREATE_UNICODE_ENVIRONMENT
                } else {
                    0
                },
            environment
                .as_mut()
                .map_or(std::ptr::null(), |block| block.as_mut_ptr().cast()),
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
    drop(attributes);
    let (stdin, stdout, stderr) = stdio.into_parent_streams();
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
    let stdin = match &spec.stdin {
        ContainedInput::Text(text) => {
            if let Some(mut stdin) = stdin {
                let text = text.clone();
                std::thread::spawn(move || {
                    let _ = stdin.write_all(&text);
                });
            }
            None
        }
        ContainedInput::Pipe => stdin.map(ContainedChildInput),
        ContainedInput::Null => None,
    };
    Ok(ContainedChild {
        process,
        containment,
        stdin,
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

    pub(crate) fn containment_process_ids(&self, max_members: usize) -> io::Result<Vec<u32>> {
        let process_ids = self.containment.process_ids().map_err(io::Error::other)?;
        if process_ids.len() > max_members {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "contained Job Object exceeds the member bound",
            ));
        }
        Ok(process_ids)
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ContainedChildOutput> {
        self.stdout.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ContainedChildOutput> {
        self.stderr.take()
    }

    pub(crate) fn take_stdin(&mut self) -> Option<ContainedChildInput> {
        self.stdin.take()
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

struct PreparedStdio {
    stdin_child: OwnedHandle,
    stdin_parent: Option<OwnedHandle>,
    stdout_child: OwnedHandle,
    stdout_parent: Option<OwnedHandle>,
    stderr_child: OwnedHandle,
    stderr_parent: Option<OwnedHandle>,
}

impl PreparedStdio {
    fn new(spec: &ContainedHeadlessCommand) -> io::Result<Self> {
        let (stdin_child, stdin_parent) = match &spec.stdin {
            ContainedInput::Text(_) | ContainedInput::Pipe => {
                let (read, write) = pipe()?;
                (read, Some(write))
            }
            ContainedInput::Null => (File::open("NUL")?.into(), None),
        };
        let (stdout_child, stdout_parent) = output_handles(&spec.stdout)?;
        let (stderr_child, stderr_parent) = output_handles(&spec.stderr)?;
        Ok(Self {
            stdin_child,
            stdin_parent,
            stdout_child,
            stdout_parent,
            stderr_child,
            stderr_parent,
        })
    }

    fn raw_child_handles(&self) -> Vec<HANDLE> {
        vec![
            self.stdin_child.as_raw_handle(),
            self.stdout_child.as_raw_handle(),
            self.stderr_child.as_raw_handle(),
        ]
    }

    fn into_parent_streams(
        self,
    ) -> (
        Option<File>,
        Option<ContainedChildOutput>,
        Option<ContainedChildOutput>,
    ) {
        let Self {
            stdin_child,
            stdin_parent,
            stdout_child,
            stdout_parent,
            stderr_child,
            stderr_parent,
        } = self;
        drop(stdin_child);
        drop(stdout_child);
        drop(stderr_child);
        (
            stdin_parent.map(File::from),
            stdout_parent.map(|handle| ContainedChildOutput(File::from(handle))),
            stderr_parent.map(|handle| ContainedChildOutput(File::from(handle))),
        )
    }
}

fn output_handles(output: &ContainedOutput) -> io::Result<(OwnedHandle, Option<OwnedHandle>)> {
    match output {
        ContainedOutput::Null => Ok((OpenOptions::new().write(true).open("NUL")?.into(), None)),
        ContainedOutput::Capture => {
            let (read, write) = pipe()?;
            Ok((write, Some(read)))
        }
        ContainedOutput::File(file) => Ok((file.try_clone()?.into(), None)),
    }
}

fn environment_block(spec: &ContainedHeadlessCommand) -> io::Result<Option<Vec<u16>>> {
    if spec.env.is_empty() {
        return Ok(None);
    }
    let overrides = EnvironmentOverrides::from_spec(spec)?;
    let inherited = crate::selected::environment::InheritedEnvironment::capture()?;
    Ok(Some(merge_environment_block(
        inherited.units()?,
        &overrides,
    )))
}

struct EncodedEnvironmentEntry {
    key: Vec<u16>,
    value: Option<Vec<u16>>,
}

struct EnvironmentOverrides(Vec<EncodedEnvironmentEntry>);

impl EnvironmentOverrides {
    fn from_spec(spec: &ContainedHeadlessCommand) -> io::Result<Self> {
        let mut overrides = Self(Vec::new());
        for (key, value) in &spec.env {
            let key = key.encode_wide().collect::<Vec<_>>();
            let value = value
                .as_deref()
                .map(|value| value.encode_wide().collect::<Vec<_>>());
            if key.is_empty()
                || key.iter().any(|unit| *unit == 0 || *unit == b'=' as u16)
                || value.as_ref().is_some_and(|value| value.contains(&0))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "environment key is empty, contains '=' or NUL, or value contains NUL",
                ));
            }
            overrides.insert(EncodedEnvironmentEntry { key, value });
        }
        Ok(overrides)
    }

    fn insert(&mut self, entry: EncodedEnvironmentEntry) {
        let mut low = 0usize;
        let mut high = self.0.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if compare_environment_keys(&self.0[middle].key, &entry.key) == CmpOrdering::Less {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        if self
            .0
            .get(low)
            .is_some_and(|current| compare_environment_keys(&current.key, &entry.key).is_eq())
        {
            self.0[low] = entry;
        } else {
            self.0.insert(low, entry);
        }
    }
}

fn merge_environment_block(inherited: &[u16], overrides: &EnvironmentOverrides) -> Vec<u16> {
    let mut block = Vec::new();
    let mut inherited_at = 0usize;
    let mut override_at = 0usize;
    while inherited_at < inherited.len() && inherited[inherited_at] != 0 {
        let entry_end = inherited[inherited_at..]
            .iter()
            .position(|unit| *unit == 0)
            .map_or(inherited.len(), |offset| inherited_at + offset);
        let entry = &inherited[inherited_at..entry_end];
        let Some(key) = environment_entry_key(entry) else {
            inherited_at = entry_end.saturating_add(1);
            continue;
        };
        let mut keep_inherited = true;
        while let Some(override_entry) = overrides.0.get(override_at) {
            match compare_environment_keys(&override_entry.key, key) {
                CmpOrdering::Less => {
                    append_environment_entry(&mut block, override_entry);
                    override_at += 1;
                }
                CmpOrdering::Equal => {
                    append_environment_entry(&mut block, override_entry);
                    override_at += 1;
                    keep_inherited = false;
                    break;
                }
                CmpOrdering::Greater => break,
            }
        }
        if keep_inherited {
            block.extend_from_slice(entry);
            block.push(0);
        }
        inherited_at = entry_end.saturating_add(1);
    }
    for entry in &overrides.0[override_at..] {
        append_environment_entry(&mut block, entry);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

fn append_environment_entry(block: &mut Vec<u16>, entry: &EncodedEnvironmentEntry) {
    let Some(value) = &entry.value else {
        return;
    };
    block.extend_from_slice(&entry.key);
    block.push(b'=' as u16);
    block.extend_from_slice(value);
    block.push(0);
}

fn environment_entry_key(entry: &[u16]) -> Option<&[u16]> {
    let search_start = usize::from(entry.first() == Some(&(b'=' as u16)));
    let separator = entry
        .get(search_start..)?
        .iter()
        .position(|unit| *unit == b'=' as u16)?
        + search_start;
    (separator != 0).then_some(&entry[..separator])
}

fn compare_environment_keys(left: &[u16], right: &[u16]) -> CmpOrdering {
    for (left, right) in left.iter().copied().zip(right.iter().copied()) {
        match ascii_upper_unit(left).cmp(&ascii_upper_unit(right)) {
            CmpOrdering::Equal => {}
            ordering => return ordering,
        }
    }
    left.len().cmp(&right.len())
}

const fn ascii_upper_unit(unit: u16) -> u16 {
    if unit >= b'a' as u16 && unit <= b'z' as u16 {
        unit - (b'a' - b'A') as u16
    } else {
        unit
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
    fn environment_merge_preserves_drive_entries_and_applies_last_case_insensitive_mutation() {
        let mut spec = ContainedHeadlessCommand::new("cmd.exe");
        spec.env("Path", "new")
            .env("alpha", "first")
            .env("ALPHA", "two")
            .env_remove("remove_me");
        let overrides = EnvironmentOverrides::from_spec(&spec).expect("valid overrides");
        let inherited = "=C:=C:\\old\0alpha=old\0PATH=old\0REMOVE_ME=old\0ZED=last\0\0"
            .encode_utf16()
            .collect::<Vec<_>>();
        let actual = String::from_utf16(&merge_environment_block(&inherited, &overrides))
            .expect("valid UTF-16");
        assert_eq!(actual, "=C:=C:\\old\0ALPHA=two\0Path=new\0ZED=last\0\0");
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
