//! macOS cumulative process resource counters.

use std::time::Duration;

use crate::contract::process_metrics::{
    ProcessBackgroundPolicy, ProcessMetrics, ProcessMetricsError, ProcessMetricsErrorKind,
    checked_page_faults,
};

pub(crate) fn metrics(pid: u32) -> Result<ProcessMetrics, ProcessMetricsError> {
    if pid == 0 {
        return Err(ProcessMetricsError::new(
            ProcessMetricsErrorKind::InvalidId,
            "process ID zero does not identify one process",
        ));
    }
    let pid = libc::pid_t::try_from(pid).map_err(|source| {
        ProcessMetricsError::new(ProcessMetricsErrorKind::InvalidId, source.to_string())
    })?;
    let mut task = unsafe { std::mem::zeroed::<libc::proc_taskinfo>() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let read =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, (&raw mut task).cast(), size) };
    if read != size {
        let error = std::io::Error::last_os_error();
        let kind = if error.raw_os_error() == Some(libc::ESRCH) {
            ProcessMetricsErrorKind::NotFound
        } else {
            ProcessMetricsErrorKind::Read
        };
        return Err(ProcessMetricsError::new(kind, error.to_string()));
    }
    let total_faults = nonnegative_counter(task.pti_faults, "page faults")?;
    let page_ins = nonnegative_counter(task.pti_pageins, "page-ins")?;
    Ok(ProcessMetrics {
        cpu_time: Duration::from_nanos(task.pti_total_user.saturating_add(task.pti_total_system)),
        resident_bytes: task.pti_resident_size,
        page_faults: checked_page_faults(total_faults, None, Some(page_ins))?,
    })
}

pub(crate) fn nice(pid: u32) -> Result<i32, ProcessMetricsError> {
    let info = read_bsd_info(pid)?;
    let nice = info.pbi_nice;
    if !(-20..=20).contains(&nice) {
        return Err(ProcessMetricsError::new(
            ProcessMetricsErrorKind::InvalidValue,
            format!("host reported invalid nice value: {nice}"),
        ));
    }
    Ok(nice)
}

pub(crate) fn set_group_nice(group_id: u32, value: i32) -> Result<(), ProcessMetricsError> {
    validate_group_nice(group_id, value)?;
    let result = unsafe { libc::setpriority(libc::PRIO_PGRP, group_id, value) };
    if result == 0 {
        Ok(())
    } else {
        let source = std::io::Error::last_os_error();
        Err(ProcessMetricsError::new(
            if source.raw_os_error() == Some(libc::ESRCH) {
                ProcessMetricsErrorKind::NotFound
            } else {
                ProcessMetricsErrorKind::Open
            },
            source.to_string(),
        ))
    }
}

pub(crate) fn set_group_suspended(
    group_id: u32,
    suspended: bool,
) -> Result<(), ProcessMetricsError> {
    let group = libc::pid_t::try_from(group_id).map_err(|_| {
        ProcessMetricsError::new(
            ProcessMetricsErrorKind::InvalidId,
            "process group ID exceeds pid_t",
        )
    })?;
    if group <= 1 {
        return Err(ProcessMetricsError::new(
            ProcessMetricsErrorKind::InvalidId,
            "process group ID must identify an owned non-system group",
        ));
    }
    let signal = if suspended {
        libc::SIGSTOP
    } else {
        libc::SIGCONT
    };
    if unsafe { libc::kill(-group, signal) } == 0 {
        Ok(())
    } else {
        let source = std::io::Error::last_os_error();
        Err(ProcessMetricsError::new(
            if source.raw_os_error() == Some(libc::ESRCH) {
                ProcessMetricsErrorKind::NotFound
            } else {
                ProcessMetricsErrorKind::Open
            },
            source.to_string(),
        ))
    }
}

fn validate_group_nice(group_id: u32, value: i32) -> Result<(), ProcessMetricsError> {
    if group_id == 0 {
        return Err(ProcessMetricsError::new(
            ProcessMetricsErrorKind::InvalidId,
            "process group ID zero means the caller's group and is not an exact target",
        ));
    }
    if !(-20..=19).contains(&value) {
        return Err(ProcessMetricsError::new(
            ProcessMetricsErrorKind::InvalidValue,
            "nice value must be in -20..=19",
        ));
    }
    Ok(())
}

pub(crate) fn is_stopped(pid: u32) -> Result<bool, ProcessMetricsError> {
    Ok(read_bsd_info(pid)?.pbi_status == libc::SSTOP)
}

pub(crate) fn background_policy(pid: u32) -> Result<ProcessBackgroundPolicy, ProcessMetricsError> {
    // Public macOS SDK values from <mach/task_policy.h>. libc does not expose
    // these two process flags, so keep the values beside the one native read.
    const PROC_FLAG_DARWINBG: u32 = 0x8000;
    const PROC_FLAG_EXT_DARWINBG: u32 = 0x10000;
    let flags = read_bsd_info(pid)?.pbi_flags;
    Ok(ProcessBackgroundPolicy {
        raw_flags: flags,
        darwin_background: flags & PROC_FLAG_DARWINBG != 0,
        external_background: flags & PROC_FLAG_EXT_DARWINBG != 0,
    })
}

fn read_bsd_info(pid: u32) -> Result<libc::proc_bsdinfo, ProcessMetricsError> {
    if pid == 0 {
        return Err(ProcessMetricsError::new(
            ProcessMetricsErrorKind::InvalidId,
            "process ID zero does not identify one process",
        ));
    }
    let pid = libc::pid_t::try_from(pid).map_err(|source| {
        ProcessMetricsError::new(ProcessMetricsErrorKind::InvalidId, source.to_string())
    })?;
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if read != size {
        let source = std::io::Error::last_os_error();
        let kind = if source.raw_os_error() == Some(libc::ESRCH) {
            ProcessMetricsErrorKind::NotFound
        } else {
            ProcessMetricsErrorKind::Read
        };
        return Err(ProcessMetricsError::new(kind, source.to_string()));
    }
    Ok(info)
}

fn nonnegative_counter(value: i32, name: &str) -> Result<u64, ProcessMetricsError> {
    u64::try_from(value).map_err(|_| {
        ProcessMetricsError::new(
            ProcessMetricsErrorKind::InvalidValue,
            format!("host reported negative {name}: {value}"),
        )
    })
}
