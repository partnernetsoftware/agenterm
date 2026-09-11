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
    contained_process::{
        ContainedHeadlessCommand, ContainedInput, ContainedOutput, FrozenProcessIdentity,
    },
    contract::process_spawn::ProcessExit,
    process_containment::{
        ProcessContainment, ProcessContainmentLimits, ProcessContainmentOptions,
    },
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
    spawn_contained(spec, false).map(|(child, _identity)| child)
}

/// The Windows frozen launch reuses the existing suspended window: the process
/// is created with `CREATE_SUSPENDED`, assigned to its kill-on-close Job, and its
/// exact start identity is read with `GetProcessTimes` **before** `ResumeThread`.
/// The child therefore cannot run its first instruction before the identity is
/// fixed.
pub(crate) fn spawn_suspended_frozen(
    spec: &ContainedHeadlessCommand,
) -> io::Result<(ContainedChild, FrozenProcessIdentity)> {
    let (child, identity) = spawn_contained(spec, true)?;
    let identity = identity.ok_or_else(|| {
        io::Error::other("frozen spawn completed without a frozen process identity")
    })?;
    Ok((child, identity))
}

fn spawn_contained(
    spec: &ContainedHeadlessCommand,
    freeze: bool,
) -> io::Result<(ContainedChild, Option<FrozenProcessIdentity>)> {
    let limits = ProcessContainmentLimits {
        memory_bytes: spec.limits.memory_bytes,
        cpu_time_seconds: spec.limits.cpu_seconds,
        active_processes: spec.limits.active_processes,
        ..ProcessContainmentLimits::default()
    };
    let containment = ProcessContainment::create(
        None,
        ProcessContainmentOptions {
            terminate_on_last_close: true,
            limits,
            ..ProcessContainmentOptions::default()
        },
    )
    .map_err(io::Error::other)?;
    match spawn_suspended_into(spec, containment, 0, freeze) {
        Ok(child) => Ok(child),
        Err(AttemptError {
            assignment_denied: true,
            ..
        }) => {
            let containment = ProcessContainment::create(
                None,
                ProcessContainmentOptions {
                    terminate_on_last_close: true,
                    limits,
                    ..ProcessContainmentOptions::default()
                },
            )
            .map_err(io::Error::other)?;
            spawn_suspended_into(spec, containment, CREATE_BREAKAWAY_FROM_JOB, freeze)
                .map_err(|failure| failure.error)
        }
        Err(failure) => Err(failure.error),
    }
}

fn spawn_suspended_into(
    spec: &ContainedHeadlessCommand,
    containment: ProcessContainment,
    extra_flags: u32,
    freeze: bool,
) -> Result<(ContainedChild, Option<FrozenProcessIdentity>), AttemptError> {
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
            let cleanup = cleanup_information(information);
            return Err(AttemptError::io(combine_failure(error, cleanup)));
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
            let cleanup = abort_raw_suspended(&handles);
            return Err(AttemptError::io(combine_failure(error, cleanup)));
        }
    };
    if let Err(error) = containment.assign(&process) {
        let assignment_denied = error.native_code() == Some(5);
        let cleanup = abort_failed_launch(&process, &containment);
        return Err(AttemptError {
            error: combine_failure(io::Error::other(error), cleanup),
            assignment_denied,
        });
    }
    // Freeze the exact identity inside the existing suspended window, before the
    // primary thread can run. A failure here aborts the still-suspended root the
    // same way a failed Job assignment does.
    let identity = if freeze {
        let pid = process.id();
        match crate::process_observation::start_identity(pid) {
            Ok(start_identity) if !start_identity.is_empty() => Some(FrozenProcessIdentity {
                pid,
                start_identity,
            }),
            Ok(_) => {
                let cleanup = abort_failed_launch(&process, &containment);
                return Err(AttemptError::io(combine_failure(
                    io::Error::other("frozen child start identity is empty"),
                    cleanup,
                )));
            }
            Err(reason) => {
                let cleanup = abort_failed_launch(&process, &containment);
                return Err(AttemptError::io(combine_failure(
                    io::Error::other(format!(
                        "frozen child start identity is unavailable: {reason}"
                    )),
                    cleanup,
                )));
            }
        }
    } else {
        None
    };
    if unsafe { ResumeThread(handles.thread().as_raw_handle()) } == u32::MAX {
        let error = io::Error::from_raw_os_error(unsafe { GetLastError() } as i32);
        let cleanup = abort_failed_launch(&process, &containment);
        return Err(AttemptError::io(combine_failure(error, cleanup)));
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
    Ok((
        ContainedChild {
            process,
            containment,
            stdin,
            stdout,
            stderr,
            exit: None,
        },
        identity,
    ))
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

/// Deadline for waiting on a failed launch's termination. It is deliberately
/// bounded: an unbounded wait turns a cleanup bug into a hang.
const FAILED_LAUNCH_WAIT_MS: u32 = 5_000;
const FAILED_LAUNCH_WAIT: Duration = Duration::from_millis(FAILED_LAUNCH_WAIT_MS as u64);

/// Abort a launch that failed after its process object existed (Job assignment,
/// identity freeze, or `ResumeThread`).
///
/// The Job termination is the first effect; when it fails or cannot be proven,
/// the exact process object this launch created is force-terminated, so a failed
/// frozen launch can never leave a running process behind. Every failure is
/// returned as a detail for the caller to keep beside the original reason
/// instead of dropping either one.
fn abort_failed_launch(
    process: &ProcessReference,
    containment: &ProcessContainment,
) -> Option<String> {
    let mut failures = Vec::new();
    if let Err(error) = containment.terminate(1) {
        failures.push(format!("job_terminate:{error}"));
    }
    if let Err(error) = process.terminate(crate::process_control::TerminationMode::Forceful) {
        failures.push(format!("force_terminate:{error}"));
    }
    match process.wait_for_exit(Some(FAILED_LAUNCH_WAIT)) {
        Ok(ProcessWait::Exited) => {}
        Ok(_) => failures.push("wait_timed_out".to_owned()),
        Err(error) => failures.push(format!("wait:{error}")),
    }
    (!failures.is_empty()).then(|| failures.join(","))
}

/// Keep the original error's kind and text and append the cleanup detail, so a
/// cleanup failure is never the only thing a caller sees (or silently lost).
fn combine_failure(primary: io::Error, cleanup: Option<String>) -> io::Error {
    match cleanup {
        None => primary,
        Some(cleanup) => io::Error::new(
            primary.kind(),
            format!("{primary}; failed-launch cleanup did not complete: {cleanup}"),
        ),
    }
}

/// Abort the raw handles of a process that was created but never handed to a
/// `ProcessReference` (or whose handle duplication failed).
///
/// Returns a cleanup detail instead of swallowing it, and the wait is bounded --
/// `WaitForSingleObject` with an explicit 5 s budget, never `u32::MAX` -- so a
/// stuck handle cannot hang the caller. Every native failure is checked:
/// `TerminateProcess`, and `WAIT_OBJECT_0` / `WAIT_TIMEOUT` / `WAIT_FAILED`.
fn abort_raw_suspended(handles: &CreatedHandles) -> Option<String> {
    use windows_sys::Win32::{
        Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{TerminateProcess, WaitForSingleObject},
    };

    let mut failures = Vec::new();
    // SAFETY: `handles.process()` is a live borrowed process handle owned by
    // `handles`; the exit code is this module's own abort marker.
    if unsafe { TerminateProcess(handles.process().as_raw_handle(), 1) } == 0 {
        // SAFETY: read immediately after the failing call, before any other FFI.
        failures.push(format!("terminate_process:{}", unsafe { GetLastError() }));
    }
    // SAFETY: as above.
    match unsafe { WaitForSingleObject(handles.process().as_raw_handle(), FAILED_LAUNCH_WAIT_MS) } {
        WAIT_OBJECT_0 => {}
        WAIT_TIMEOUT => failures.push("wait_timeout".to_owned()),
        WAIT_FAILED => {
            // SAFETY: read immediately after the failing call, before any other FFI.
            failures.push(format!("wait_failed:{}", unsafe { GetLastError() }));
        }
        other => failures.push(format!("wait_unexpected:{other}")),
    }
    (!failures.is_empty()).then(|| failures.join(","))
}

/// Abort the raw handles of a `CreateProcessW` result that never became a
/// `ProcessReference`. Returns its cleanup detail so the caller can keep it
/// beside the original error instead of dropping either one.
fn cleanup_information(information: PROCESS_INFORMATION) -> Option<String> {
    if information.hProcess.is_null() {
        return None;
    }
    let handles = CreatedHandles(information);
    abort_raw_suspended(&handles)
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
        let mut child = spawn_suspended_into(&spec, containment, 0, false)
            .map(|(child, _identity)| child)
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

#[cfg(all(test, target_os = "windows"))]
mod frozen_windows_tests {
    use super::*;
    use crate::process_containment::ProcessContainmentErrorKind;
    use std::{thread, time::Instant};

    const FROZEN_FIRST_INSTRUCTION_JOB: &str = r"Local\agenterm-frozen-first-instruction-test";

    /// Wait for a frozen probe to exit, whichever `ContainedChild` type the
    /// caller owns (the facade wrapper or the selected adapter).
    fn wait_until_exit<T>(mut poll: impl FnMut() -> std::io::Result<Option<T>>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match poll().expect("wait for frozen probe") {
                Some(_) => return,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => panic!("frozen probe timed out"),
            }
        }
    }

    /// The frozen window must yield a readable identity even when the target
    /// exits at once: `GetProcessTimes` is read before `ResumeThread`.
    #[test]
    fn frozen_spawn_freezes_an_identity_for_an_immediate_exit() {
        for _ in 0..20 {
            let mut command = ContainedHeadlessCommand::new("cmd.exe");
            command.args(["/c", "exit", "0"]);
            let (mut child, identity) = command
                .spawn_suspended_frozen()
                .expect("frozen spawn of an immediate exit");
            assert_eq!(identity.pid, child.id());
            assert!(
                identity.start_identity.starts_with("windows-filetime:"),
                "unexpected identity: {}",
                identity.start_identity
            );
            wait_until_exit(|| child.try_wait());
        }
    }

    /// The frozen root must already belong to its Job at its first instruction,
    /// and the frozen identity must be returned for that same process.
    #[test]
    fn frozen_launch_freezes_identity_before_resume_inside_the_job() {
        match ProcessContainment::open(FROZEN_FIRST_INSTRUCTION_JOB) {
            Ok(containment) => {
                let process =
                    ProcessReference::open(std::process::id()).expect("retain child identity");
                assert!(
                    containment
                        .contains(&process)
                        .expect("query exact child membership"),
                    "the frozen child ran before it belonged to the expected Job"
                );
                return;
            }
            Err(error) if error.kind() == ProcessContainmentErrorKind::NotFound => {}
            Err(error) => panic!("probe containment open failed: {error}"),
        }

        let containment = ProcessContainment::create(
            Some(FROZEN_FIRST_INSTRUCTION_JOB),
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
            "selected::contained_process::frozen_windows_tests::frozen_launch_freezes_identity_before_resume_inside_the_job",
            "--nocapture",
        ]);
        let (mut child, identity) = spawn_suspended_into(&spec, containment, 0, true)
            .map_err(|failure| failure.error)
            .expect("frozen spawn exact contained probe");
        let identity = identity.expect("a frozen launch must return the frozen identity");
        assert_eq!(identity.pid, child.id());
        assert!(!identity.start_identity.is_empty());
        wait_until_exit(|| child.try_wait());
    }

    /// Create one real suspended process the way the pre-`ProcessReference` path
    /// does. Returns its `PROCESS_INFORMATION` and a duplicate capable of
    /// observing the exit after the raw handles are gone.
    fn raw_suspended_probe() -> (PROCESS_INFORMATION, ProcessReference) {
        use windows_sys::Win32::System::Threading::STARTUPINFOW;

        let mut command_line: Vec<u16> = "cmd.exe /c ping -n 30 127.0.0.1"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                command_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_NO_WINDOW | CREATE_SUSPENDED,
                std::ptr::null(),
                std::ptr::null(),
                &startup,
                &raw mut information,
            )
        };
        assert_ne!(ok, 0, "CreateProcessW failed: {}", unsafe {
            GetLastError()
        });
        let handles = CreatedHandles(information);
        let process = ProcessReference::duplicate_from(handles.process())
            .expect("retain an observing duplicate");
        drop(handles);
        (information, process)
    }

    /// The pre-`ProcessReference` abort must actually terminate the process and
    /// bound its wait, and it must report cleanup failures rather than swallowing
    /// them. This covers the raw path, not only the healthy
    /// `abort_failed_launch` branch.
    #[test]
    fn raw_abort_is_bounded_and_reports_cleanup() {
        let (information, process) = raw_suspended_probe();
        drop(process);
        // `cleanup_information` owns the raw handles of this fresh process.
        assert_eq!(
            cleanup_information(information),
            None,
            "a healthy raw abort reports no cleanup failure"
        );
        let pid = information.dwProcessId;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match ProcessReference::open(pid) {
                Ok(reference) => {
                    assert!(
                        Instant::now() < deadline,
                        "the raw abort did not terminate the process"
                    );
                    drop(reference);
                    thread::sleep(Duration::from_millis(10));
                }
                // The abort terminated it: the exact pid is no longer openable.
                Err(_) => break,
            }
        }
    }

    /// A failed frozen launch must terminate and reap the exact process object it
    /// created, on a bounded deadline, and report no cleanup failure when the
    /// abort is healthy.
    #[test]
    fn failed_launch_abort_terminates_and_reaps_the_exact_process() {
        let containment = ProcessContainment::create(
            None,
            ProcessContainmentOptions {
                terminate_on_last_close: true,
                ..ProcessContainmentOptions::default()
            },
        )
        .expect("create containment");
        let mut spec = ContainedHeadlessCommand::new("cmd.exe");
        spec.args(["/c", "ping", "-n", "30", "127.0.0.1"]);
        let (child, _identity) = spawn_suspended_into(&spec, containment, 0, false)
            .map_err(|failure| failure.error)
            .expect("spawn suspended probe");
        let cleanup = abort_failed_launch(&child.process, &child.containment);
        assert_eq!(cleanup, None, "a healthy abort reports no cleanup failure");
        assert_eq!(
            child
                .process
                .wait_for_exit(Some(Duration::from_secs(5)))
                .expect("wait for the aborted process"),
            ProcessWait::Exited
        );
    }
}
