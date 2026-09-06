//! macOS implementation of the process facade contract.

use std::process::{Child, ChildStderr, ChildStdout, Command};

use crate::contract::process::{
    PROCESS_ENVIRONMENT_MAX_BYTES, ProcessCgroupError, ProcessCgroupErrorKind,
    ProcessCgroupV2Snapshot, ProcessEnvironmentSnapshot, ProcessError, ProcessErrorKind,
    ProcessFileDescriptor, ProcessInfo, ProcessInspection, ProcessMemoryRegion, ProcessObservation,
    ProcessSocketInfo, ProcessThreadInfo,
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

const NATIVE_PATH_MAX: usize = 1024;
const NATIVE_THREAD_NAME_MAX: usize = 64;
const NATIVE_ENDPOINT_MAX: usize = 1024;

#[derive(Clone, Copy)]
#[repr(C)]
struct NativeFd {
    descriptor: i32,
    kind: u32,
    has_vnode: u32,
    open_flags: u32,
    status_flags: u32,
    offset_bytes: i64,
    file_type: u32,
    guard_flags: u32,
    target_len: u32,
    target: [u8; NATIVE_PATH_MAX],
}

#[derive(Clone, Copy)]
#[repr(C)]
struct NativeRegion {
    start_address: u64,
    size_bytes: u64,
    offset_bytes: u64,
    protection: u32,
    max_protection: u32,
    flags: u32,
    sharing: u32,
    resident_pages: u32,
    private_resident_pages: u32,
    shared_resident_pages: u32,
    swapped_pages: u32,
    dirtied_pages: u32,
    user_tag: u32,
    depth: u32,
    path_len: u32,
    path: [u8; NATIVE_PATH_MAX],
}

#[derive(Clone, Copy)]
#[repr(C)]
struct NativeThread {
    id: u64,
    user_time: u64,
    system_time: u64,
    cpu_usage: i32,
    policy: i32,
    run_state: i32,
    flags: i32,
    sleep_seconds: i32,
    current_priority: i32,
    priority: i32,
    max_priority: i32,
    name_len: u32,
    name: [u8; NATIVE_THREAD_NAME_MAX],
}

#[derive(Clone, Copy)]
#[repr(C)]
struct NativeSocket {
    descriptor: i32,
    family: i32,
    socket_type: i32,
    protocol: i32,
    tcp_state: i32,
    generic_state: u32,
    local_len: u32,
    remote_len: u32,
    local: [u8; NATIVE_ENDPOINT_MAX],
    remote: [u8; NATIVE_ENDPOINT_MAX],
}

unsafe extern "C" {
    fn agt_process_fds(
        pid: u32,
        out: *mut NativeFd,
        capacity: usize,
        visited: *mut usize,
        written: *mut usize,
        read_errors: *mut usize,
        truncated: *mut i32,
    ) -> i32;
    fn agt_process_regions(
        pid: u32,
        out: *mut NativeRegion,
        capacity: usize,
        visited: *mut usize,
        written: *mut usize,
        truncated: *mut i32,
    ) -> i32;
    fn agt_process_threads(
        pid: u32,
        out: *mut NativeThread,
        capacity: usize,
        visited: *mut usize,
        written: *mut usize,
        read_errors: *mut usize,
        truncated: *mut i32,
    ) -> i32;
    fn agt_process_sockets(
        pid: u32,
        out: *mut NativeSocket,
        capacity: usize,
        visited: *mut usize,
        written: *mut usize,
        read_errors: *mut usize,
        truncated: *mut i32,
    ) -> i32;
}

fn native_inspection_error(code: i32, subject: &str) -> ProcessError {
    let kind = match code {
        1 | 4 => ProcessErrorKind::InvalidData,
        2 => ProcessErrorKind::PermissionDenied,
        _ => ProcessErrorKind::Inspect,
    };
    ProcessError::new(
        kind,
        format!("native {subject} provider failed with code {code}"),
    )
}

fn native_bytes(bytes: &[u8], length: u32, subject: &str) -> Result<Option<Vec<u8>>, ProcessError> {
    let length = usize::try_from(length).map_err(|_| {
        ProcessError::new(
            ProcessErrorKind::InvalidData,
            format!("{subject} length overflows"),
        )
    })?;
    if length > bytes.len() {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            format!("{subject} length exceeds native buffer"),
        ));
    }
    Ok((length != 0).then(|| bytes[..length].to_vec()))
}

pub(crate) fn file_descriptors(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessFileDescriptor>, ProcessError> {
    let empty = unsafe { std::mem::zeroed::<NativeFd>() };
    let mut raw = vec![empty; max_visited];
    let (mut visited, mut written, mut read_errors, mut truncated) = (0, 0, 0, 0);
    let code = unsafe {
        agt_process_fds(
            pid,
            raw.as_mut_ptr(),
            raw.len(),
            &raw mut visited,
            &raw mut written,
            &raw mut read_errors,
            &raw mut truncated,
        )
    };
    if code != 0 {
        return Err(native_inspection_error(code, "descriptor"));
    }
    if written > raw.len() || visited > max_visited || written > visited {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "native descriptor counts exceed their buffers",
        ));
    }
    raw.truncate(written);
    let items = raw
        .into_iter()
        .map(|row| {
            let kind = match row.kind {
                0 => "appletalk".to_owned(),
                1 => "vnode".to_owned(),
                2 => "socket".to_owned(),
                3 => "shared-memory".to_owned(),
                4 => "semaphore".to_owned(),
                5 => "kqueue".to_owned(),
                6 => "pipe".to_owned(),
                7 => "fsevents".to_owned(),
                9 => "network-policy".to_owned(),
                10 => "channel".to_owned(),
                11 => "nexus".to_owned(),
                other => format!("unknown-{other}"),
            };
            Ok(ProcessFileDescriptor {
                descriptor: row.descriptor,
                kind,
                target: native_bytes(&row.target, row.target_len, "descriptor target")?,
                open_flags: (row.has_vnode != 0).then_some(row.open_flags),
                status_flags: (row.has_vnode != 0).then_some(row.status_flags),
                offset_bytes: (row.has_vnode != 0).then_some(row.offset_bytes),
                file_type: (row.has_vnode != 0).then_some(row.file_type),
                guard_flags: (row.has_vnode != 0).then_some(row.guard_flags),
            })
        })
        .collect::<Result<Vec<_>, ProcessError>>()?;
    Ok(ProcessInspection {
        items,
        visited_count: visited,
        read_errors,
        truncated_scan: truncated != 0,
    })
}

fn protection(bits: u32) -> String {
    [
        if bits & 1 != 0 { 'r' } else { '-' },
        if bits & 2 != 0 { 'w' } else { '-' },
        if bits & 4 != 0 { 'x' } else { '-' },
    ]
    .into_iter()
    .collect()
}

pub(crate) fn memory_regions(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessMemoryRegion>, ProcessError> {
    let empty = unsafe { std::mem::zeroed::<NativeRegion>() };
    let mut raw = vec![empty; max_visited];
    let (mut visited, mut written, mut truncated) = (0, 0, 0);
    let code = unsafe {
        agt_process_regions(
            pid,
            raw.as_mut_ptr(),
            raw.len(),
            &raw mut visited,
            &raw mut written,
            &raw mut truncated,
        )
    };
    if code != 0 {
        return Err(native_inspection_error(code, "memory-region"));
    }
    if written > raw.len() || visited > max_visited || written != visited {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "native memory-region counts exceed their buffers",
        ));
    }
    raw.truncate(written);
    let items = raw
        .into_iter()
        .map(|row| {
            row.start_address
                .checked_add(row.size_bytes)
                .ok_or_else(|| {
                    ProcessError::new(
                        ProcessErrorKind::InvalidData,
                        "native memory-region range overflows its address space",
                    )
                })?;
            let sharing = match row.sharing {
                1 => "copy-on-write".to_owned(),
                2 => "private".to_owned(),
                3 => "empty".to_owned(),
                4 => "shared".to_owned(),
                5 => "true-shared".to_owned(),
                6 => "private-aliased".to_owned(),
                7 => "shared-aliased".to_owned(),
                8 => "large-page".to_owned(),
                other => format!("unknown-{other}"),
            };
            Ok(ProcessMemoryRegion {
                start_address: row.start_address,
                size_bytes: row.size_bytes,
                offset_bytes: row.offset_bytes,
                permissions: protection(row.protection),
                max_permissions: Some(protection(row.max_protection)),
                sharing,
                path: native_bytes(&row.path, row.path_len, "memory-region path")?,
                device: None,
                inode: None,
                flags: Some(row.flags),
                user_tag: Some(row.user_tag),
                depth: Some(row.depth),
                resident_pages: Some(row.resident_pages),
                private_resident_pages: Some(row.private_resident_pages),
                shared_resident_pages: Some(row.shared_resident_pages),
                swapped_pages: Some(row.swapped_pages),
                dirtied_pages: Some(row.dirtied_pages),
            })
        })
        .collect::<Result<Vec<_>, ProcessError>>()?;
    Ok(ProcessInspection {
        items,
        visited_count: visited,
        read_errors: 0,
        truncated_scan: truncated != 0,
    })
}

pub(crate) fn threads(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessThreadInfo>, ProcessError> {
    let empty = unsafe { std::mem::zeroed::<NativeThread>() };
    let mut raw = vec![empty; max_visited];
    let (mut visited, mut written, mut read_errors, mut truncated) = (0, 0, 0, 0);
    let code = unsafe {
        agt_process_threads(
            pid,
            raw.as_mut_ptr(),
            raw.len(),
            &raw mut visited,
            &raw mut written,
            &raw mut read_errors,
            &raw mut truncated,
        )
    };
    if code != 0 {
        return Err(native_inspection_error(code, "thread"));
    }
    if written > raw.len() || visited > max_visited || written + read_errors != visited {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "native thread counts exceed their buffers",
        ));
    }
    raw.truncate(written);
    let items = raw
        .into_iter()
        .map(|row| {
            let state = match row.run_state {
                1 => "running".to_owned(),
                2 => "stopped".to_owned(),
                3 => "waiting".to_owned(),
                4 => "uninterruptible".to_owned(),
                5 => "halted".to_owned(),
                other => format!("unknown-{other}"),
            };
            Ok(ProcessThreadInfo {
                id: row.id,
                name: native_bytes(&row.name, row.name_len, "thread name")?,
                state,
                state_raw: row.run_state.to_string(),
                user_time_raw: row.user_time,
                system_time_raw: row.system_time,
                time_unit: "darwin-thread-time",
                cpu_usage_tenths_percent: Some(row.cpu_usage),
                policy: Some(row.policy),
                flags: Some(row.flags),
                sleep_seconds: Some(row.sleep_seconds),
                current_priority: Some(row.current_priority),
                priority: Some(row.priority),
                max_priority: Some(row.max_priority),
                nice: None,
                processor: None,
            })
        })
        .collect::<Result<Vec<_>, ProcessError>>()?;
    Ok(ProcessInspection {
        items,
        visited_count: visited,
        read_errors,
        truncated_scan: truncated != 0,
    })
}

pub(crate) fn sockets(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessSocketInfo>, ProcessError> {
    let empty = unsafe { std::mem::zeroed::<NativeSocket>() };
    let mut raw = vec![empty; max_visited];
    let (mut visited, mut written, mut read_errors, mut truncated) = (0, 0, 0, 0);
    let code = unsafe {
        agt_process_sockets(
            pid,
            raw.as_mut_ptr(),
            raw.len(),
            &raw mut visited,
            &raw mut written,
            &raw mut read_errors,
            &raw mut truncated,
        )
    };
    if code != 0 {
        return Err(native_inspection_error(code, "socket"));
    }
    if written > raw.len()
        || visited > max_visited
        || written
            .checked_add(read_errors)
            .is_none_or(|accounted| accounted > visited)
    {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "native socket counts exceed their buffers",
        ));
    }
    raw.truncate(written);
    let items = raw
        .into_iter()
        .map(|row| {
            let local = native_bytes(&row.local, row.local_len, "socket local endpoint")?;
            let remote = native_bytes(&row.remote, row.remote_len, "socket remote endpoint")?;
            let family = match row.family {
                libc::AF_INET => "IPv4".to_owned(),
                libc::AF_INET6 => "IPv6".to_owned(),
                libc::AF_UNIX => "Unix".to_owned(),
                other => format!("family-{other}"),
            };
            let protocol = match (row.family, row.protocol, row.socket_type) {
                (libc::AF_INET | libc::AF_INET6, libc::IPPROTO_TCP, _) => "TCP".to_owned(),
                (libc::AF_INET | libc::AF_INET6, libc::IPPROTO_UDP, _) => "UDP".to_owned(),
                (libc::AF_UNIX, _, libc::SOCK_STREAM) => "UNIX-STREAM".to_owned(),
                (libc::AF_UNIX, _, libc::SOCK_DGRAM) => "UNIX-DGRAM".to_owned(),
                (libc::AF_UNIX, _, _) => "UNIX".to_owned(),
                (_, other, _) => format!("protocol-{other}"),
            };
            let remote = remote.filter(|value| !endpoint_is_unspecified(value));
            let endpoint = match (local.as_deref(), remote.as_deref()) {
                (Some(local), Some(remote)) => [local, b"->", remote].concat(),
                (Some(local), None) => local.to_vec(),
                (None, Some(remote)) => remote.to_vec(),
                (None, None) => format!("socket-fd:{}", row.descriptor).into_bytes(),
            };
            let state = if row.tcp_state >= 0 {
                Some(darwin_tcp_state(row.tcp_state).to_owned())
            } else {
                generic_socket_state(row.generic_state).map(str::to_owned)
            };
            Ok(ProcessSocketInfo {
                descriptor: row.descriptor,
                family,
                protocol,
                local,
                remote,
                endpoint,
                state,
                inode: None,
            })
        })
        .collect::<Result<Vec<_>, ProcessError>>()?;
    Ok(ProcessInspection {
        items,
        visited_count: visited,
        read_errors,
        truncated_scan: truncated != 0,
    })
}

fn endpoint_is_unspecified(endpoint: &[u8]) -> bool {
    endpoint == b"0.0.0.0:0" || endpoint == b"[::]:0"
}

fn darwin_tcp_state(state: i32) -> &'static str {
    match state {
        0 => "closed",
        1 => "listen",
        2 => "syn-sent",
        3 => "syn-received",
        4 => "established",
        5 => "close-wait",
        6 => "fin-wait-1",
        7 => "closing",
        8 => "last-ack",
        9 => "fin-wait-2",
        10 => "time-wait",
        11 => "reserved",
        _ => "unknown",
    }
}

fn generic_socket_state(flags: u32) -> Option<&'static str> {
    if flags & 0x0002 != 0 {
        Some("connected")
    } else if flags & 0x0004 != 0 {
        Some("connecting")
    } else if flags & 0x0008 != 0 {
        Some("disconnecting")
    } else if flags & 0x2000 != 0 {
        Some("disconnected")
    } else {
        None
    }
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

pub(crate) fn cgroup_v2(
    _pid: u32,
    _expected_start_identity: Option<&str>,
) -> Result<ProcessCgroupV2Snapshot, ProcessCgroupError> {
    Err(ProcessCgroupError::new(
        ProcessCgroupErrorKind::NotApplicable,
        "Linux cgroup v2 process observation does not apply to macOS",
    ))
}

pub struct ProcessTreeGuard {
    process_group: libc::pid_t,
    root_start_identity: Option<String>,
    adopted: bool,
    adopted_termination: Option<Vec<AdoptedTerminationMember>>,
    active: bool,
}

struct AdoptedTerminationMember {
    process_id: u32,
    start_identity: String,
    was_stopped: bool,
    reference: crate::process_reference::ProcessReference,
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
            adopted: false,
            adopted_termination: None,
            active: true,
        })
    }

    /// Retain an already-running, current-user process group for bounded
    /// inventory. This does not mutate or terminate the group on drop.
    pub fn adopt_group_leader(
        process_id: u32,
        expected_start_identity: &str,
        max_members: usize,
    ) -> Result<Self, String> {
        if process_id <= 1 || expected_start_identity.is_empty() || max_members == 0 {
            return Err("adopted process-group parameters are invalid".to_owned());
        }
        let native_id = libc::pid_t::try_from(process_id)
            .map_err(|_| "adopted process ID exceeds pid_t".to_owned())?;
        if unsafe { libc::getpgid(native_id) } != native_id {
            return Err("adopted process must be its process-group leader".to_owned());
        }
        if !matches!(
            observe(process_id),
            ProcessObservation::Live { start_identity: Some(identity) }
                if identity == expected_start_identity
        ) {
            return Err("adopted process identity is not live and exact".to_owned());
        }
        let guard = Self {
            process_group: native_id,
            root_start_identity: Some(expected_start_identity.to_owned()),
            adopted: true,
            adopted_termination: None,
            active: true,
        };
        validate_adopted_group_owner(&guard, max_members)?;
        Ok(guard)
    }

    /// Retain audit-token-bound mutation references for every stable member of
    /// an adopted process group. A host that cannot obtain that exact native
    /// authority refuses `expiry=stop` before durable adoption begins.
    pub fn adopt_group_leader_for_termination(
        process_id: u32,
        expected_start_identity: &str,
        max_members: usize,
    ) -> Result<Self, String> {
        let mut guard = Self::adopt_group_leader(process_id, expected_start_identity, max_members)?;
        let before = guard.process_ids(max_members)?;
        let mut members = Vec::with_capacity(before.len());
        for member_id in &before {
            let start_identity = match observe(*member_id) {
                ProcessObservation::Live {
                    start_identity: Some(identity),
                } => identity,
                _ => return Err("adopted group member identity is unavailable".to_owned()),
            };
            let reference =
                crate::process_reference::ProcessReference::open_for_termination(*member_id)
                    .map_err(|error| format!("retain adopted group member failed: {error}"))?;
            let was_stopped = crate::process_metrics::is_stopped(*member_id).map_err(|error| {
                format!("observe adopted group scheduler state failed: {error}")
            })?;
            if !matches!(
                observe(*member_id),
                ProcessObservation::Live { start_identity: Some(current) }
                    if current == start_identity
            ) || !reference.is_alive().unwrap_or(false)
            {
                return Err("adopted group member changed while authority was retained".to_owned());
            }
            members.push(AdoptedTerminationMember {
                process_id: *member_id,
                start_identity,
                was_stopped,
                reference,
            });
        }
        let mut after = guard.process_ids(max_members)?;
        let mut before = before;
        before.sort_unstable();
        after.sort_unstable();
        if before != after {
            return Err(
                "adopted process-group membership changed while authority was retained".to_owned(),
            );
        }
        guard.adopted_termination = Some(members);
        Ok(guard)
    }

    pub fn process_ids(&self, max_members: usize) -> Result<Vec<u32>, String> {
        use std::{ffi::c_void, mem::size_of};
        const PROC_PGRP_ONLY: u32 = 2;
        #[link(name = "proc")]
        unsafe extern "C" {
            fn proc_listpids(
                process_type: u32,
                type_info: u32,
                buffer: *mut c_void,
                buffer_size: libc::c_int,
            ) -> libc::c_int;
        }
        if !self.active || !self.root_is_owned() {
            return Err("owned process-group root identity is no longer live".to_owned());
        }
        let expected_group = u32::try_from(self.process_group)
            .map_err(|_| "owned process-group ID is invalid".to_owned())?;
        let required =
            unsafe { proc_listpids(PROC_PGRP_ONLY, expected_group, std::ptr::null_mut(), 0) };
        if required <= 0 {
            return Err("owned process-group inventory sizing failed".to_owned());
        }
        let capacity =
            usize::try_from(required).unwrap_or_default() / size_of::<libc::c_int>() + 32;
        let mut ids = vec![0 as libc::c_int; capacity];
        let buffer_size = libc::c_int::try_from(ids.len().saturating_mul(size_of::<libc::c_int>()))
            .map_err(|_| "owned process-group inventory buffer overflow".to_owned())?;
        let bytes = unsafe {
            proc_listpids(
                PROC_PGRP_ONLY,
                expected_group,
                ids.as_mut_ptr().cast(),
                buffer_size,
            )
        };
        if bytes <= 0 || bytes >= buffer_size {
            return Err("owned process-group inventory was truncated".to_owned());
        }
        ids.truncate(usize::try_from(bytes).unwrap_or_default() / size_of::<libc::c_int>());
        let mut process_ids = Vec::new();
        for pid in ids.into_iter().filter(|pid| *pid > 0) {
            let pid = u32::try_from(pid)
                .map_err(|_| "owned process-group member ID is invalid".to_owned())?;
            process_ids.push(pid);
            if process_ids.len() > max_members {
                return Err("owned process group exceeds the member bound".to_owned());
            }
        }
        if !self.root_is_owned() || !process_ids.contains(&expected_group) {
            return Err("owned process-group root changed during inventory".to_owned());
        }
        Ok(process_ids)
    }

    fn root_is_owned(&self) -> bool {
        let Ok(root_id) = u32::try_from(self.process_group) else {
            return false;
        };
        matches!(
            (&self.root_start_identity, observe(root_id)),
            (
                Some(expected),
                ProcessObservation::Live {
                    start_identity: Some(current),
                },
            ) if current == *expected
        )
    }

    pub fn terminate(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        if self.adopted_termination.is_some() {
            return self.terminate_adopted_group();
        }
        if self.adopted {
            return Err(
                "adopted process group was retained for observation and cannot be terminated"
                    .to_owned(),
            );
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

    fn terminate_adopted_group(&mut self) -> Result<(), String> {
        let current =
            self.process_ids(self.adopted_termination.as_ref().map_or(0, Vec::len).max(1))?;
        let members = self.adopted_termination.as_ref().expect("checked above");
        let mut expected = members
            .iter()
            .map(|member| member.process_id)
            .collect::<Vec<_>>();
        let mut current = current;
        expected.sort_unstable();
        current.sort_unstable();
        if current != expected
            || members.iter().any(|member| {
                !matches!(
                    observe(member.process_id),
                    ProcessObservation::Live { start_identity: Some(identity) }
                        if identity == member.start_identity
                ) || !member.reference.is_alive().unwrap_or(false)
            })
        {
            return Err(
                "adopted process-group membership or identity changed before termination"
                    .to_owned(),
            );
        }

        let mut frozen = Vec::new();
        for (index, member) in members.iter().enumerate() {
            if member.was_stopped {
                continue;
            }
            if let Err(error) = member.reference.set_suspended(true) {
                resume_adopted_members(members, &frozen);
                return Err(format!("freeze adopted group member failed: {error}"));
            }
            frozen.push(index);
        }
        let stable = self.process_ids(members.len()).and_then(|mut ids| {
            ids.sort_unstable();
            if ids == expected
                && members.iter().all(|member| {
                    matches!(
                        observe(member.process_id),
                        ProcessObservation::Live { start_identity: Some(identity) }
                            if identity == member.start_identity
                    )
                })
            {
                Ok(())
            } else {
                Err("adopted process-group changed while frozen".to_owned())
            }
        });
        if let Err(error) = stable {
            resume_adopted_members(members, &frozen);
            return Err(error);
        }

        let root_id = u32::try_from(self.process_group)
            .map_err(|_| "owned process group is outside the process ID range".to_owned())?;
        for member in members.iter().filter(|member| member.process_id != root_id) {
            member
                .reference
                .terminate(crate::process_control::TerminationMode::Forceful)
                .map_err(|error| format!("terminate adopted group member failed: {error}"))?;
        }
        members
            .iter()
            .find(|member| member.process_id == root_id)
            .ok_or_else(|| "adopted process-group root reference is missing".to_owned())?
            .reference
            .terminate(crate::process_control::TerminationMode::Forceful)
            .map_err(|error| format!("terminate adopted group root failed: {error}"))?;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        for member in members {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if member
                .reference
                .wait_for_exit(Some(remaining))
                .map_err(|error| format!("wait for adopted group member exit failed: {error}"))?
                != crate::process_reference::ProcessWait::Exited
            {
                return Err("adopted process-group did not exit before the deadline".to_owned());
            }
        }
        self.active = false;
        Ok(())
    }
}

fn resume_adopted_members(members: &[AdoptedTerminationMember], frozen: &[usize]) {
    for index in frozen.iter().rev() {
        let _ = members[*index].reference.set_suspended(false);
    }
}

fn validate_adopted_group_owner(
    guard: &ProcessTreeGuard,
    max_members: usize,
) -> Result<(), String> {
    use crate::process_security::ProcessPrincipal;

    let current = crate::process_security::current_process()
        .map_err(|error| format!("current process principal unavailable: {error}"))?;
    let ProcessPrincipal::Posix {
        effective_user_id, ..
    } = current.principal()
    else {
        return Err("current process principal is not POSIX".to_owned());
    };
    let mut before = guard.process_ids(max_members)?;
    before.sort_unstable();
    if before.contains(&std::process::id()) {
        return Err("refusing to adopt the controller's own process group".to_owned());
    }
    for process_id in &before {
        let facts = crate::process_security::process(*process_id)
            .map_err(|error| format!("adopted group principal unavailable: {error}"))?;
        if !matches!(
            facts.principal(),
            ProcessPrincipal::Posix { effective_user_id: member_user_id, .. }
                if member_user_id == effective_user_id
        ) {
            return Err(
                "every adopted process-group member must belong to the current user".to_owned(),
            );
        }
    }
    let mut after = guard.process_ids(max_members)?;
    after.sort_unstable();
    if before != after {
        return Err("adopted process-group membership changed during validation".to_owned());
    }
    Ok(())
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
