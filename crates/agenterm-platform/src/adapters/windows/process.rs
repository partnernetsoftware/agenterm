//! Windows implementation of the process facade contract.

use std::process::{Child, ChildStderr, ChildStdout, Command};

use crate::contract::process::{PipeProbeError, PipeProbeToken};
use crate::contract::process::{ProcessError, ProcessErrorKind, ProcessInfo};

pub(crate) fn stdout_probe_token(reader: &ChildStdout) -> Option<PipeProbeToken> {
    use std::os::windows::io::AsRawHandle as _;
    Some(PipeProbeToken(reader.as_raw_handle() as usize))
}

pub(crate) fn stderr_probe_token(reader: &ChildStderr) -> Option<PipeProbeToken> {
    use std::os::windows::io::AsRawHandle as _;
    Some(PipeProbeToken(reader.as_raw_handle() as usize))
}

pub(crate) fn pipe_available(token: PipeProbeToken) -> Result<usize, PipeProbeError> {
    use windows_sys::Win32::{
        Foundation::{ERROR_BROKEN_PIPE, ERROR_NO_DATA, GetLastError},
        System::Pipes::PeekNamedPipe,
    };
    let mut available = 0_u32;
    if unsafe {
        PeekNamedPipe(
            token.0 as windows_sys::Win32::Foundation::HANDLE,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Ok(available as usize);
    }
    let error = unsafe { GetLastError() };
    if error == ERROR_BROKEN_PIPE || error == ERROR_NO_DATA {
        Err(PipeProbeError::Closed)
    } else {
        Err(PipeProbeError::Failed)
    }
}

pub(crate) fn list() -> Result<Vec<ProcessInfo>, ProcessError> {
    use std::mem::size_of;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(ProcessError::new(
            ProcessErrorKind::Inventory,
            "snapshot failed",
        ));
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let mut processes = Vec::new();
    let mut present = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while present {
        let length = entry
            .szExeFile
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(entry.szExeFile.len());
        let executable_name = String::from_utf16_lossy(&entry.szExeFile[..length]);
        if !executable_name.is_empty() {
            processes.push(ProcessInfo {
                id: entry.th32ProcessID,
                parent_id: entry.th32ParentProcessID,
                executable_name,
            });
        }
        present = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    Ok(processes)
}

pub(crate) fn command_line(pid: u32) -> Result<String, ProcessError> {
    use std::ffi::c_void;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };

    const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
    const MAX_BYTES: u32 = 1024 * 1024;
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationProcess(
            process: HANDLE,
            information_class: u32,
            information: *mut c_void,
            information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(ProcessError::new(
            ProcessErrorKind::Inspect,
            std::io::Error::last_os_error().to_string(),
        ));
    }
    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    let handle = Owned(handle);
    let mut required = 0u32;
    let first = unsafe {
        NtQueryInformationProcess(
            handle.0,
            PROCESS_COMMAND_LINE_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut required,
        )
    };
    if first != STATUS_INFO_LENGTH_MISMATCH || required == 0 || required > MAX_BYTES {
        return Err(ProcessError::new(
            if required > MAX_BYTES {
                ProcessErrorKind::InventoryTooLarge
            } else {
                ProcessErrorKind::Inspect
            },
            "native process command-line query did not return a bounded size",
        ));
    }
    let mut bytes = vec![0u8; required as usize];
    let status = unsafe {
        NtQueryInformationProcess(
            handle.0,
            PROCESS_COMMAND_LINE_INFORMATION,
            bytes.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    };
    if status < 0 || bytes.len() < std::mem::size_of::<UnicodeString>() {
        return Err(ProcessError::new(
            ProcessErrorKind::Inspect,
            format!("native process command-line query failed with status 0x{status:08x}"),
        ));
    }
    let string = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<UnicodeString>()) };
    let base = bytes.as_ptr() as usize;
    let start = (string.buffer as usize).checked_sub(base).ok_or_else(|| {
        ProcessError::new(
            ProcessErrorKind::Inspect,
            "native command-line pointer is outside its response buffer",
        )
    })?;
    let length = string.length as usize;
    let end = start.checked_add(length).ok_or_else(|| {
        ProcessError::new(
            ProcessErrorKind::Inspect,
            "native command-line length overflow",
        )
    })?;
    if length % 2 != 0 || end > bytes.len() {
        return Err(ProcessError::new(
            ProcessErrorKind::Inspect,
            "native command-line string is outside its response buffer",
        ));
    }
    let units =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr().add(start).cast::<u16>(), length / 2) };
    Ok(String::from_utf16_lossy(units))
}

pub struct ProcessTreeGuard {
    containment: crate::process_containment::ProcessContainment,
    active: bool,
}

pub(crate) fn configure_owned_command(_command: &mut Command) -> Result<(), String> {
    Ok(())
}

impl ProcessTreeGuard {
    pub fn attach(child: &Child) -> Result<Self, String> {
        use crate::{
            process_containment::{ProcessContainment, ProcessContainmentOptions},
            process_reference::ProcessReference,
        };
        use std::os::windows::io::AsHandle as _;
        let containment = ProcessContainment::create(
            None,
            ProcessContainmentOptions {
                terminate_on_last_close: true,
                allow_breakaway: true,
                ..ProcessContainmentOptions::default()
            },
        )
        .map_err(|error| error.to_string())?;
        let process = ProcessReference::duplicate_from(child.as_handle())
            .map_err(|error| format!("retain child process failed: {error}"))?;
        containment
            .assign(&process)
            .map_err(|error| error.to_string())?;
        let guard = Self {
            containment,
            active: true,
        };
        Ok(guard)
    }

    pub fn terminate(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        self.containment
            .terminate(1)
            .map_err(|error| error.to_string())?;
        self.active = false;
        Ok(())
    }
}
