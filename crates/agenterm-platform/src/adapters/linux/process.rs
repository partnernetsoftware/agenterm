//! Linux implementation of the process facade contract.

use std::process::{Child, ChildStderr, ChildStdout, Command};

use crate::contract::process::{
    PROCESS_ENVIRONMENT_MAX_BYTES, ProcessEnvironmentSnapshot, ProcessError, ProcessErrorKind,
    ProcessFileDescriptor, ProcessInfo, ProcessInspection, ProcessMemoryRegion, ProcessObservation,
    ProcessThreadInfo,
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
    let entries = std::fs::read_dir("/proc")
        .map_err(|error| ProcessError::new(ProcessErrorKind::Inventory, error.to_string()))?;
    let mut processes = Vec::new();
    for entry in entries.flatten() {
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(executable) = crate::selected::process_image::executable_path(id) else {
            continue;
        };
        let Some(executable_name) = executable.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let parent_id = std::fs::read_to_string(entry.path().join("stat"))
            .ok()
            .and_then(|stat| {
                let end = stat.rfind(')')?;
                stat.get(end + 1..)?
                    .split_whitespace()
                    .nth(1)?
                    .parse::<u32>()
                    .ok()
            })
            .unwrap_or_default();
        processes.push(ProcessInfo {
            id,
            parent_id,
            executable_name: executable_name.to_owned(),
        });
    }
    Ok(processes)
}

fn inspection_error(error: std::io::Error) -> ProcessError {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => ProcessErrorKind::NotFound,
        std::io::ErrorKind::PermissionDenied => ProcessErrorKind::PermissionDenied,
        _ => ProcessErrorKind::Inspect,
    };
    ProcessError::new(kind, error.to_string())
}

pub(crate) fn file_descriptors(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessFileDescriptor>, ProcessError> {
    use std::os::unix::ffi::OsStringExt as _;

    let directory = format!("/proc/{pid}/fd");
    let entries = std::fs::read_dir(&directory).map_err(inspection_error)?;
    let mut descriptors = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
                .map(|descriptor| (descriptor, entry.path()))
        })
        .collect::<Vec<_>>();
    descriptors.sort_unstable_by_key(|(descriptor, _)| *descriptor);
    let truncated_scan = descriptors.len() > max_visited;
    descriptors.truncate(max_visited);
    let visited_count = descriptors.len();
    let mut read_errors = 0usize;
    let items = descriptors
        .into_iter()
        .filter_map(|(descriptor, path)| match std::fs::read_link(path) {
            Ok(target) => {
                let target = target.into_os_string().into_vec();
                let kind = if target.starts_with(b"socket:[") {
                    "socket"
                } else if target.starts_with(b"pipe:[") {
                    "pipe"
                } else if target.starts_with(b"anon_inode:") {
                    "anonymous-inode"
                } else if target.starts_with(b"memfd:") {
                    "memory-file"
                } else if target.starts_with(b"/") {
                    "file"
                } else {
                    "other"
                };
                Some(ProcessFileDescriptor {
                    descriptor,
                    kind: kind.to_owned(),
                    target: Some(target),
                    open_flags: None,
                    status_flags: None,
                    offset_bytes: None,
                    file_type: None,
                    guard_flags: None,
                })
            }
            Err(_) => {
                read_errors += 1;
                None
            }
        })
        .collect();
    Ok(ProcessInspection {
        items,
        visited_count,
        read_errors,
        truncated_scan,
    })
}

pub(crate) fn memory_regions(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessMemoryRegion>, ProcessError> {
    use std::io::Read as _;

    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    let file = std::fs::File::open(format!("/proc/{pid}/maps")).map_err(inspection_error)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(inspection_error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ProcessError::new(
            ProcessErrorKind::InventoryTooLarge,
            "process maps exceed 8 MiB",
        ));
    }
    let mut items = Vec::new();
    let mut truncated_scan = false;
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if items.len() == max_visited {
            truncated_scan = true;
            break;
        }
        items.push(parse_linux_map_line(line)?);
    }
    let visited_count = items.len();
    Ok(ProcessInspection {
        items,
        visited_count,
        read_errors: 0,
        truncated_scan,
    })
}

fn parse_linux_map_line(line: &[u8]) -> Result<ProcessMemoryRegion, ProcessError> {
    use std::os::unix::ffi::OsStrExt as _;

    let mut cursor = 0usize;
    let range = take_map_token(line, &mut cursor)?;
    let permissions = take_map_token(line, &mut cursor)?;
    let offset = take_map_token(line, &mut cursor)?;
    let device = take_map_token(line, &mut cursor)?;
    let inode = take_map_token(line, &mut cursor)?;
    while cursor < line.len() && line[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    let path = (cursor < line.len()).then(|| line[cursor..].to_vec());
    let separator = range.iter().position(|byte| *byte == b'-').ok_or_else(|| {
        ProcessError::new(
            ProcessErrorKind::InvalidData,
            "process map range is malformed",
        )
    })?;
    let (start, end_with_separator) = range.split_at(separator);
    let end = &end_with_separator[1..];
    let start_address = parse_hex(start, "map start")?;
    let end_address = parse_hex(end, "map end")?;
    if end_address <= start_address || permissions.len() != 4 {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "process map range or permissions are malformed",
        ));
    }
    let sharing = match permissions[3] {
        b'p' => "private",
        b's' => "shared",
        _ => {
            return Err(ProcessError::new(
                ProcessErrorKind::InvalidData,
                "process map sharing mode is malformed",
            ));
        }
    };
    Ok(ProcessMemoryRegion {
        start_address,
        size_bytes: end_address - start_address,
        offset_bytes: parse_hex(offset, "map offset")?,
        permissions: std::str::from_utf8(&permissions[..3])
            .map_err(|error| ProcessError::new(ProcessErrorKind::InvalidData, error.to_string()))?
            .to_owned(),
        max_permissions: None,
        sharing: sharing.to_owned(),
        path,
        device: Some(
            std::ffi::OsStr::from_bytes(device)
                .to_string_lossy()
                .into_owned(),
        ),
        inode: Some(parse_decimal(inode, "map inode")?),
        flags: None,
        user_tag: None,
        depth: None,
        resident_pages: None,
        private_resident_pages: None,
        shared_resident_pages: None,
        swapped_pages: None,
        dirtied_pages: None,
    })
}

fn take_map_token<'a>(line: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], ProcessError> {
    while *cursor < line.len() && line[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
    let start = *cursor;
    while *cursor < line.len() && !line[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
    if start == *cursor {
        Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "process map row has too few fields",
        ))
    } else {
        Ok(&line[start..*cursor])
    }
}

fn parse_hex(bytes: &[u8], field: &str) -> Result<u64, ProcessError> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .ok_or_else(|| {
            ProcessError::new(ProcessErrorKind::InvalidData, format!("{field} is invalid"))
        })
}

fn parse_decimal(bytes: &[u8], field: &str) -> Result<u64, ProcessError> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            ProcessError::new(ProcessErrorKind::InvalidData, format!("{field} is invalid"))
        })
}

pub(crate) fn threads(
    pid: u32,
    max_visited: usize,
) -> Result<ProcessInspection<ProcessThreadInfo>, ProcessError> {
    let directory = format!("/proc/{pid}/task");
    let entries = std::fs::read_dir(&directory).map_err(inspection_error)?;
    let mut ids = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u64>().ok())
        })
        .collect::<Vec<_>>();
    ids.sort_unstable();
    let truncated_scan = ids.len() > max_visited;
    ids.truncate(max_visited);
    let visited_count = ids.len();
    let mut read_errors = 0usize;
    let items = ids
        .into_iter()
        .filter_map(|id| {
            let stat = match std::fs::read(format!("{directory}/{id}/stat")) {
                Ok(stat) => stat,
                Err(_) => {
                    read_errors += 1;
                    return None;
                }
            };
            match parse_linux_thread_stat(id, &stat) {
                Ok(thread) => Some(thread),
                Err(_) => {
                    read_errors += 1;
                    None
                }
            }
        })
        .collect();
    Ok(ProcessInspection {
        items,
        visited_count,
        read_errors,
        truncated_scan,
    })
}

fn parse_linux_thread_stat(id: u64, stat: &[u8]) -> Result<ProcessThreadInfo, ProcessError> {
    let open = stat.iter().position(|byte| *byte == b'(').ok_or_else(|| {
        ProcessError::new(ProcessErrorKind::InvalidData, "thread stat has no name")
    })?;
    let close = stat.iter().rposition(|byte| *byte == b')').ok_or_else(|| {
        ProcessError::new(
            ProcessErrorKind::InvalidData,
            "thread stat has no closing name",
        )
    })?;
    if close <= open || close + 2 >= stat.len() {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "thread stat name is malformed",
        ));
    }
    let fields = stat[close + 2..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields.len() < 37 {
        return Err(ProcessError::new(
            ProcessErrorKind::InvalidData,
            "thread stat has too few fields",
        ));
    }
    let state_raw = std::str::from_utf8(fields[0])
        .map_err(|error| ProcessError::new(ProcessErrorKind::InvalidData, error.to_string()))?;
    let state = match state_raw {
        "R" => "running",
        "S" => "sleeping",
        "D" => "uninterruptible",
        "T" => "stopped",
        "t" => "tracing-stop",
        "Z" => "zombie",
        "X" | "x" => "dead",
        "I" => "idle",
        "W" => "paging",
        "P" => "parked",
        _ => "unknown",
    };
    Ok(ProcessThreadInfo {
        id,
        name: Some(stat[open + 1..close].to_vec()),
        state: state.to_owned(),
        state_raw: state_raw.to_owned(),
        user_time_raw: parse_decimal(fields[11], "thread user time")?,
        system_time_raw: parse_decimal(fields[12], "thread system time")?,
        time_unit: "linux-clock-ticks",
        cpu_usage_tenths_percent: None,
        policy: None,
        flags: None,
        sleep_seconds: None,
        current_priority: None,
        priority: Some(parse_signed(fields[15], "thread priority")?),
        max_priority: None,
        nice: Some(parse_signed(fields[16], "thread nice")?),
        processor: Some(parse_signed(fields[36], "thread processor")?),
    })
}

fn parse_signed(bytes: &[u8], field: &str) -> Result<i32, ProcessError> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            ProcessError::new(ProcessErrorKind::InvalidData, format!("{field} is invalid"))
        })
}

pub(crate) fn command_line(pid: u32) -> Result<String, ProcessError> {
    Ok(arguments(pid)?.join(" "))
}

pub(crate) fn arguments(pid: u32) -> Result<Vec<String>, ProcessError> {
    use std::io::Read as _;
    const MAX_BYTES: u64 = 1024 * 1024;
    let file = std::fs::File::open(format!("/proc/{pid}/cmdline"))
        .map_err(|error| ProcessError::new(ProcessErrorKind::Inspect, error.to_string()))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ProcessError::new(ProcessErrorKind::Inspect, error.to_string()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ProcessError::new(
            ProcessErrorKind::InventoryTooLarge,
            "process command line exceeds 1 MiB",
        ));
    }
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(ProcessError::new(
            ProcessErrorKind::Inspect,
            "process arguments are unavailable",
        ));
    }
    Ok(bytes
        .split(|byte| *byte == 0)
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect())
}

pub(crate) fn environment_snapshot(pid: u32) -> Result<ProcessEnvironmentSnapshot, ProcessError> {
    use std::io::Read as _;

    const MAX_BYTES: u64 = PROCESS_ENVIRONMENT_MAX_BYTES as u64;
    let file = std::fs::File::open(format!("/proc/{pid}/environ"))
        .map_err(process_environment_io_error)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(process_environment_io_error)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ProcessError::new(
            ProcessErrorKind::InventoryTooLarge,
            "process initial environment exceeds 4 MiB",
        ));
    }
    // proc_pid_environ(5) defines this as the NUL-delimited environment
    // installed by execve, not a reflection of later putenv/setenv changes.
    Ok(ProcessEnvironmentSnapshot::from_nul_delimited(&bytes))
}

fn process_environment_io_error(error: std::io::Error) -> ProcessError {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => ProcessErrorKind::NotFound,
        std::io::ErrorKind::PermissionDenied => ProcessErrorKind::PermissionDenied,
        _ => ProcessErrorKind::Inspect,
    };
    ProcessError::new(kind, error.to_string())
}

pub(crate) fn current_directory(pid: u32) -> Result<std::path::PathBuf, ProcessError> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).map_err(|error| {
        let kind = match error.kind() {
            std::io::ErrorKind::NotFound => ProcessErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied => ProcessErrorKind::PermissionDenied,
            _ => ProcessErrorKind::Inspect,
        };
        ProcessError::new(kind, error.to_string())
    })
}

pub struct ProcessTreeGuard {
    process_group: libc::pid_t,
    root_start_identity: Option<String>,
    root_reference: Option<crate::process_reference::ProcessReference>,
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
        let root_reference = crate::process_reference::ProcessReference::open(child.id()).ok();
        Ok(Self {
            process_group,
            root_start_identity,
            root_reference,
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
        let root_alive = self
            .root_reference
            .as_ref()
            .is_some_and(|reference| reference.is_alive().unwrap_or(false));
        if !root_is_owned || !root_alive {
            // Once the original root is reaped, its PID and process-group ID
            // can be reused by unrelated processes. Only terminate a tree while
            // the exact pidfd-backed root is still live.
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
