//! Linux `/proc` cumulative process resource counters.

use std::time::Duration;

use crate::contract::process_metrics::{
    ProcessBackgroundPolicy, ProcessMetrics, ProcessMetricsError, ProcessMetricsErrorKind,
    checked_page_faults,
};

pub(crate) fn metrics(pid: u32) -> Result<ProcessMetrics, ProcessMetricsError> {
    if pid == 0 {
        return Err(error(
            ProcessMetricsErrorKind::InvalidId,
            "process ID zero does not identify one process",
        ));
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|source| {
        error(
            if source.kind() == std::io::ErrorKind::NotFound {
                ProcessMetricsErrorKind::NotFound
            } else {
                ProcessMetricsErrorKind::Open
            },
            source.to_string(),
        )
    })?;
    let close = stat.rfind(')').ok_or_else(|| {
        error(
            ProcessMetricsErrorKind::Parse,
            "process stat has no closing command delimiter",
        )
    })?;
    let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    let minor_faults = parse_field(&fields, 7, "minor page faults")?;
    let major_faults = parse_field(&fields, 9, "major page faults")?;
    let user_ticks = parse_field(&fields, 11, "user CPU ticks")?;
    let system_ticks = parse_field(&fields, 12, "system CPU ticks")?;
    let clock_hz = sysconf(libc::_SC_CLK_TCK, "clock ticks")?;
    let page_size = sysconf(libc::_SC_PAGESIZE, "page size")?;
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).map_err(|source| {
        error(
            if source.kind() == std::io::ErrorKind::NotFound {
                ProcessMetricsErrorKind::NotFound
            } else {
                ProcessMetricsErrorKind::Read
            },
            source.to_string(),
        )
    })?;
    let resident_pages = statm
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| {
            error(
                ProcessMetricsErrorKind::Parse,
                "process statm has no RSS field",
            )
        })?
        .parse::<u64>()
        .map_err(|source| error(ProcessMetricsErrorKind::Parse, source.to_string()))?;
    let ticks = user_ticks.saturating_add(system_ticks);
    let total_faults = minor_faults.checked_add(major_faults).ok_or_else(|| {
        error(
            ProcessMetricsErrorKind::Overflow,
            "total page-fault count overflows u64",
        )
    })?;
    Ok(ProcessMetrics {
        cpu_time: Duration::from_secs(ticks / clock_hz).saturating_add(Duration::from_nanos(
            (ticks % clock_hz).saturating_mul(1_000_000_000) / clock_hz,
        )),
        resident_bytes: resident_pages.saturating_mul(page_size),
        page_faults: checked_page_faults(total_faults, Some(minor_faults), Some(major_faults))?,
    })
}

pub(crate) fn nice(pid: u32) -> Result<i32, ProcessMetricsError> {
    let stat = read_stat(pid)?;
    let close = stat.rfind(')').ok_or_else(|| {
        error(
            ProcessMetricsErrorKind::Parse,
            "process stat has no closing command delimiter",
        )
    })?;
    let fields = stat[close + 1..].split_whitespace().collect::<Vec<_>>();
    let nice = fields
        .get(16)
        .ok_or_else(|| error(ProcessMetricsErrorKind::Parse, "missing nice value"))?
        .parse::<i32>()
        .map_err(|source| error(ProcessMetricsErrorKind::Parse, source.to_string()))?;
    if !(-20..=20).contains(&nice) {
        return Err(error(
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
        Err(error(
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
        return Err(error(
            ProcessMetricsErrorKind::InvalidId,
            "process group ID zero means the caller's group and is not an exact target",
        ));
    }
    if !(-20..=19).contains(&value) {
        return Err(error(
            ProcessMetricsErrorKind::InvalidValue,
            "nice value must be in -20..=19",
        ));
    }
    Ok(())
}

pub(crate) fn is_stopped(pid: u32) -> Result<bool, ProcessMetricsError> {
    let stat = read_stat(pid)?;
    let close = stat.rfind(')').ok_or_else(|| {
        error(
            ProcessMetricsErrorKind::Parse,
            "process stat has no closing command delimiter",
        )
    })?;
    let state = stat[close + 1..]
        .split_whitespace()
        .next()
        .ok_or_else(|| error(ProcessMetricsErrorKind::Parse, "missing process state"))?;
    Ok(matches!(state, "T" | "t"))
}

pub(crate) fn background_policy(pid: u32) -> Result<ProcessBackgroundPolicy, ProcessMetricsError> {
    if pid == 0 {
        return Err(ProcessMetricsError::new(
            ProcessMetricsErrorKind::InvalidId,
            "process ID zero does not identify one process",
        ));
    }
    Err(ProcessMetricsError::new(
        ProcessMetricsErrorKind::Unsupported,
        "Linux has no Darwin process-background flag model",
    ))
}

fn read_stat(pid: u32) -> Result<String, ProcessMetricsError> {
    if pid == 0 {
        return Err(error(
            ProcessMetricsErrorKind::InvalidId,
            "process ID zero does not identify one process",
        ));
    }
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|source| {
        error(
            if source.kind() == std::io::ErrorKind::NotFound {
                ProcessMetricsErrorKind::NotFound
            } else {
                ProcessMetricsErrorKind::Open
            },
            source.to_string(),
        )
    })
}

fn parse_field(fields: &[&str], index: usize, name: &str) -> Result<u64, ProcessMetricsError> {
    fields
        .get(index)
        .ok_or_else(|| error(ProcessMetricsErrorKind::Parse, format!("missing {name}")))?
        .parse::<u64>()
        .map_err(|source| error(ProcessMetricsErrorKind::Parse, source.to_string()))
}

fn sysconf(key: libc::c_int, name: &str) -> Result<u64, ProcessMetricsError> {
    let value = unsafe { libc::sysconf(key) };
    u64::try_from(value).map_err(|_| {
        error(
            ProcessMetricsErrorKind::Clock,
            format!("host reported invalid {name}: {value}"),
        )
    })
}

fn error(kind: ProcessMetricsErrorKind, detail: impl Into<String>) -> ProcessMetricsError {
    ProcessMetricsError::new(kind, detail)
}
