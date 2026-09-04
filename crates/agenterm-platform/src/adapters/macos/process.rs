//! macOS implementation of the process facade contract.

use std::process::{Child, ChildStderr, ChildStdout, Command};

use crate::contract::process::{
    PROCESS_ENVIRONMENT_MAX_BYTES, ProcessEnvironmentSnapshot, ProcessError, ProcessErrorKind,
    ProcessInfo, ProcessObservation,
};
use crate::contract::process::{PipeProbeError, PipeProbeToken};
use crate::process_observation::observe;

pub(crate) fn stdout_probe_token(_reader: &ChildStdout) -> Option<PipeProbeToken> {
    None
}
pub(crate) fn stderr_probe_token(_reader: &ChildStderr) -> Option<PipeProbeToken> {
    None
}
pub(crate) fn pipe_available(_token: PipeProbeToken) -> Result<usize, PipeProbeError> {
    Err(PipeProbeError::Failed)
}

pub(crate) fn list() -> Result<Vec<ProcessInfo>, ProcessError> {
    use std::{
        ffi::{c_int, c_void},
        mem::size_of,
    };
    const PROC_ALL_PIDS: u32 = 1;
    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_listpids(
            process_type: u32,
            type_info: u32,
            buffer: *mut c_void,
            buffer_size: c_int,
        ) -> c_int;
    }
    let required = unsafe { proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
    if required <= 0 {
        return Err(ProcessError::new(
            ProcessErrorKind::Inventory,
            "size failed",
        ));
    }
    let capacity = usize::try_from(required).unwrap_or_default() / size_of::<c_int>() + 32;
    let mut ids: Vec<c_int> = vec![0; capacity];
    let buffer_size = c_int::try_from(ids.len() * size_of::<c_int>())
        .map_err(|_| ProcessError::new(ProcessErrorKind::InventoryTooLarge, "buffer overflow"))?;
    let bytes = unsafe { proc_listpids(PROC_ALL_PIDS, 0, ids.as_mut_ptr().cast(), buffer_size) };
    if bytes <= 0 {
        return Err(ProcessError::new(
            ProcessErrorKind::Inventory,
            "snapshot failed",
        ));
    }
    ids.truncate(usize::try_from(bytes).unwrap_or_default() / size_of::<c_int>());
    let mut processes = Vec::new();
    for id in ids.into_iter().filter(|id| *id > 0) {
        let Ok(id) = u32::try_from(id) else {
            continue;
        };
        let Ok(executable) = crate::selected::process_image::executable_path(id) else {
            continue;
        };
        let executable_name = executable
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_owned();
        if !executable_name.is_empty() {
            let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
            let info_size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let info_bytes = unsafe {
                libc::proc_pidinfo(
                    id as libc::pid_t,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    (&raw mut info).cast(),
                    info_size,
                )
            };
            let parent_id = if info_bytes == info_size {
                info.pbi_ppid
            } else {
                0
            };
            processes.push(ProcessInfo {
                id,
                parent_id,
                executable_name,
            });
        }
    }
    Ok(processes)
}

pub(crate) fn command_line(pid: u32) -> Result<String, ProcessError> {
    Ok(arguments(pid)?.join(" "))
}

pub(crate) fn arguments(pid: u32) -> Result<Vec<String>, ProcessError> {
    const MAX_BYTES: usize = 1024 * 1024;
    let bytes = read_procargs2(pid, MAX_BYTES, "arguments")?;
    let (arguments, _) = procargs2_sections(&bytes)?;
    if arguments.is_empty() {
        return Err(ProcessError::new(
            ProcessErrorKind::Inspect,
            "process arguments are unavailable",
        ));
    }
    Ok(arguments
        .into_iter()
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect())
}

pub(crate) fn environment_snapshot(pid: u32) -> Result<ProcessEnvironmentSnapshot, ProcessError> {
    let bytes = read_procargs2(pid, PROCESS_ENVIRONMENT_MAX_BYTES, "initial environment")?;
    let (_, environment) = procargs2_sections(&bytes)?;
    if environment.is_empty() {
        // XNU intentionally returns an argv-only KERN_PROCARGS2 buffer for a
        // cs-restricted target unless the caller has a private entitlement.
        // That shape is indistinguishable from a genuinely empty environment.
        return Err(ProcessError::new(
            ProcessErrorKind::Unavailable,
            "process initial environment is empty or omitted by macOS",
        ));
    }
    Ok(ProcessEnvironmentSnapshot::from_nul_delimited(environment))
}

fn read_procargs2(pid: u32, max_bytes: usize, subject: &str) -> Result<Vec<u8>, ProcessError> {
    use std::mem::size_of;

    let pid = libc::c_int::try_from(pid)
        .map_err(|_| ProcessError::new(ProcessErrorKind::IdOutOfRange, "pid exceeds c_int"))?;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size = 0usize;
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(procargs2_error(std::io::Error::last_os_error()));
    }
    if size < size_of::<libc::c_int>() {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            format!("process {subject} buffer is too short"),
        ));
    }
    if size > max_bytes {
        return Err(ProcessError::new(
            ProcessErrorKind::InventoryTooLarge,
            format!("process {subject} buffer exceeds {} bytes", max_bytes),
        ));
    }
    let mut bytes = vec![0u8; size];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(procargs2_error(std::io::Error::last_os_error()));
    }
    if size > bytes.len() {
        return Err(ProcessError::new(
            ProcessErrorKind::InventoryTooLarge,
            format!("process {subject} grew beyond the bounded buffer"),
        ));
    }
    bytes.truncate(size);
    Ok(bytes)
}

fn procargs2_error(error: std::io::Error) -> ProcessError {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => ProcessErrorKind::NotFound,
        std::io::ErrorKind::PermissionDenied => ProcessErrorKind::PermissionDenied,
        _ if error.raw_os_error() == Some(libc::ESRCH) => ProcessErrorKind::NotFound,
        _ if error.raw_os_error() == Some(libc::ENOMEM) => ProcessErrorKind::InventoryTooLarge,
        _ => ProcessErrorKind::Inspect,
    };
    ProcessError::new(kind, error.to_string())
}

fn procargs2_sections(bytes: &[u8]) -> Result<(Vec<&[u8]>, &[u8]), ProcessError> {
    use std::mem::size_of;

    if bytes.len() < size_of::<libc::c_int>() {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "process argument buffer is too short",
        ));
    }
    let argc = libc::c_int::from_ne_bytes(
        bytes[..size_of::<libc::c_int>()]
            .try_into()
            .expect("validated argc width"),
    );
    let argc = usize::try_from(argc).map_err(|_| {
        ProcessError::new(ProcessErrorKind::InvalidData, "process argc is negative")
    })?;
    if argc > bytes.len() {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "process argc exceeds its bounded native buffer",
        ));
    }
    let mut offset = size_of::<libc::c_int>();
    while offset < bytes.len() && bytes[offset] != 0 {
        offset += 1;
    }
    if offset == bytes.len() {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "process executable path is not NUL terminated",
        ));
    }
    while offset < bytes.len() && bytes[offset] == 0 {
        offset += 1;
    }
    let mut arguments = Vec::with_capacity(argc);
    for _ in 0..argc {
        if offset >= bytes.len() {
            return Err(ProcessError::new(
                ProcessErrorKind::InvalidData,
                "process argument buffer ends before argc",
            ));
        }
        let relative_end = bytes[offset..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| {
                ProcessError::new(
                    ProcessErrorKind::InvalidData,
                    "process argument buffer ends before argc",
                )
            })?;
        let end = offset + relative_end;
        arguments.push(&bytes[offset..end]);
        offset = end + 1;
    }
    while offset < bytes.len() && bytes[offset] == 0 {
        offset += 1;
    }
    let environment_start = offset;
    while offset < bytes.len() && bytes[offset] != 0 {
        let Some(relative_end) = bytes[offset..].iter().position(|byte| *byte == 0) else {
            return Err(ProcessError::new(
                ProcessErrorKind::InvalidData,
                "process environment entry is not NUL terminated",
            ));
        };
        offset += relative_end + 1;
    }
    Ok((arguments, &bytes[environment_start..offset]))
}

pub(crate) fn current_directory(pid: u32) -> Result<std::path::PathBuf, ProcessError> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let pid = libc::c_int::try_from(pid)
        .map_err(|_| ProcessError::new(ProcessErrorKind::IdOutOfRange, "pid exceeds c_int"))?;
    let mut info = unsafe { std::mem::zeroed::<libc::proc_vnodepathinfo>() };
    let size =
        libc::c_int::try_from(std::mem::size_of::<libc::proc_vnodepathinfo>()).map_err(|_| {
            ProcessError::new(ProcessErrorKind::InvalidData, "cwd buffer overflows c_int")
        })?;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if read <= 0 {
        let error = std::io::Error::last_os_error();
        let kind = match error.kind() {
            std::io::ErrorKind::NotFound => ProcessErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied => ProcessErrorKind::PermissionDenied,
            _ => ProcessErrorKind::Inspect,
        };
        return Err(ProcessError::new(kind, error.to_string()));
    }
    if read != size {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "PROC_PIDVNODEPATHINFO returned a partial record",
        ));
    }

    let path = &info.pvi_cdir.vip_path;
    let capacity = std::mem::size_of_val(path);
    // SAFETY: vip_path is an inline [[c_char; 32]; 32] byte array. Reading its
    // complete object representation as u8 is valid, and the NUL scan remains
    // bounded by that exact inline capacity.
    let bytes = unsafe { std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), capacity) };
    let length = bytes.iter().position(|byte| *byte == 0).ok_or_else(|| {
        ProcessError::new(
            ProcessErrorKind::InvalidData,
            "current-directory path is not NUL terminated",
        )
    })?;
    if length == 0 {
        return Err(ProcessError::new(
            ProcessErrorKind::Unavailable,
            "current-directory path is unavailable",
        ));
    }
    Ok(std::path::PathBuf::from(OsString::from_vec(
        bytes[..length].to_vec(),
    )))
}

pub struct ProcessTreeGuard {
    process_group: libc::pid_t,
    root_start_identity: Option<String>,
    active: bool,
}

pub(crate) fn configure_owned_command(command: &mut Command) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    Ok(())
}

impl ProcessTreeGuard {
    pub fn attach(child: &Child) -> Result<Self, String> {
        let process_group = libc::pid_t::try_from(child.id())
            .map_err(|_| "child process ID exceeds pid_t".to_owned())?;
        let root_start_identity = match observe(child.id()) {
            ProcessObservation::Live {
                start_identity: Some(identity),
            } => Some(identity),
            _ => None,
        };
        Ok(Self {
            process_group,
            root_start_identity,
            active: true,
        })
    }

    pub fn terminate(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        let root_id = u32::try_from(self.process_group)
            .map_err(|_| "owned process group is outside the process ID range".to_owned())?;
        let root_observation = observe(root_id);
        let root_is_owned = match (&self.root_start_identity, &root_observation) {
            (
                Some(expected),
                ProcessObservation::Live {
                    start_identity: Some(current),
                },
            ) => *current == *expected,
            _ => false,
        };
        if !root_is_owned {
            // Once the original root is reaped, its PID and process-group ID
            // can be reused by unrelated processes. Only terminate a tree while
            // the exact observed root is still live.
            self.active = false;
            return Ok(());
        }
        let descendants = list()
            .map(|processes| {
                crate::contract::process::transitive_descendant_ids(root_id, &processes)
                    .into_iter()
                    .filter_map(|id| match observe(id) {
                        ProcessObservation::Live {
                            start_identity: Some(identity),
                        } => Some((id, identity)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .map_err(|error| format!("owned process inventory failed: {error}"));
        let group_result =
            if root_is_owned && unsafe { libc::killpg(self.process_group, libc::SIGKILL) } == 0 {
                Ok(())
            } else {
                let error = std::io::Error::last_os_error();
                if !root_is_owned || error.raw_os_error() == Some(libc::ESRCH) {
                    Ok(())
                } else {
                    Err(format!("killpg failed: {error}"))
                }
            };
        let mut failures = Vec::new();
        if let Err(error) = group_result {
            failures.push(error);
        }
        match descendants {
            Ok(descendants) => {
                for (id, identity) in descendants {
                    if !matches!(
                        observe(id),
                        ProcessObservation::Live { start_identity: Some(current) }
                            if current == identity
                    ) {
                        continue;
                    }
                    let Ok(native_id) = libc::pid_t::try_from(id) else {
                        failures.push(format!("descendant process ID {id} exceeds pid_t"));
                        continue;
                    };
                    if unsafe { libc::kill(native_id, libc::SIGKILL) } != 0 {
                        let error = std::io::Error::last_os_error();
                        if error.raw_os_error() != Some(libc::ESRCH) {
                            failures.push(format!("kill descendant {id} failed: {error}"));
                        }
                    }
                }
            }
            Err(error) => failures.push(error),
        }
        if !failures.is_empty() {
            return Err(failures.join("; "));
        }
        self.active = false;
        Ok(())
    }
}

#[cfg(test)]
mod procargs2_tests {
    use super::procargs2_sections;

    fn fixture(argc: libc::c_int, fields: &[&[u8]]) -> Vec<u8> {
        let mut bytes = argc.to_ne_bytes().to_vec();
        for field in fields {
            bytes.extend_from_slice(field);
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn parser_skips_executable_padding_and_exactly_argc_arguments() {
        let bytes = fixture(
            3,
            &[
                b"/bin/demo",
                b"",
                b"",
                b"demo",
                b"",
                b"tail",
                b"A=one",
                b"EMPTY=",
                b"BAD",
                b"NON_UTF8=\xff",
                b"",
            ],
        );
        let (arguments, environment) = procargs2_sections(&bytes).expect("valid procargs2");
        assert_eq!(arguments, vec![&b"demo"[..], &b""[..], &b"tail"[..]]);
        assert_eq!(environment, b"A=one\0EMPTY=\0BAD\0NON_UTF8=\xff\0");
    }

    #[test]
    fn parser_rejects_an_unterminated_environment_entry() {
        let mut bytes = fixture(1, &[b"/bin/demo", b"", b"demo"]);
        bytes.extend_from_slice(b"A=unterminated");
        assert!(procargs2_sections(&bytes).is_err());
    }
}
