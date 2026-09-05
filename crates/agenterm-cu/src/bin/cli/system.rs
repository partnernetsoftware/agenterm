//! Host-level discovery that is independent of the desktop/window family.

use agenterm_cu::{
    Command, DeviceInventorySelector, PermissionAction, PermissionKind, SetupAction, TargetRef,
    command::{
        DEVICE_INVENTORY_MAX, DEVICE_WATCH_DURATION_MS_MAX, DEVICE_WATCH_EVENTS_MAX,
        DEVICE_WATCH_INTERVAL_MS_MAX, DEVICE_WATCH_INTERVAL_MS_MIN, JobEnvironment,
        JobOutputCursor, JobOutputStream, JobStateFilter, STORAGE_DEVICES_MAX,
    },
};

use super::{flag_parsed, flag_text, take_switch, verbs::VerbSpec};

const DEFAULT_SESSION_TTL_SECONDS: u64 = 3_600;
const DEFAULT_LOCK_TTL_SECONDS: u64 = 300;
const DEFAULT_JOB_TTL_SECONDS: u64 = 3_600;
const DEFAULT_JOB_EVENTS_MAX_BYTES: usize = 65_536;

pub fn parse(
    spec: &VerbSpec,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spec.name == "setup" {
        let check = take_switch(args, "--check");
        let bin_dir = flag_text(args, "--bin-dir")?;
        if !args.is_empty() {
            return Err(format!(
                "setup accepts only --check and --bin-dir PATH; unexpected {:?}",
                args[0]
            ));
        }
        let command = Command::Setup {
            target,
            action: if check {
                SetupAction::Check
            } else {
                SetupAction::Apply
            },
            bin_dir,
        };
        command.validate().map_err(str::to_owned)?;
        return Ok(command);
    }
    if spec.name == "resource-status" {
        if args.first().is_some_and(|arg| arg == "status") {
            args.remove(0);
        }
        if !args.is_empty() {
            return Err(format!(
                "resource-status accepts no arguments; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::ResourceStatus { target });
    }
    if spec.name == "runtime-status" {
        if args.first().is_some_and(|arg| arg == "status") {
            args.remove(0);
        }
        if !args.is_empty() {
            return Err(format!(
                "runtime-status accepts no arguments; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::RuntimeStatus { target });
    }
    if spec.name == "storage-devices" {
        if args.first().is_some_and(|arg| arg == "devices") {
            args.remove(0);
        }
        let max = flag_parsed::<usize>(args, "--max")?.unwrap_or(500);
        if !(1..=STORAGE_DEVICES_MAX).contains(&max) {
            return Err("storage devices --max must be in 1..=5000".to_owned());
        }
        if !args.is_empty() {
            return Err(format!(
                "storage-devices accepts only --max N; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::StorageDevices { target, max });
    }
    if spec.name == "device-list" {
        if args.first().is_some_and(|arg| arg == "list") {
            args.remove(0);
        }
        let selector =
            DeviceInventorySelector::parse(flag_text(args, "--type")?.as_deref().unwrap_or("all"))?;
        let max = flag_parsed::<usize>(args, "--max")?.unwrap_or(500);
        if !(1..=DEVICE_INVENTORY_MAX).contains(&max) {
            return Err("device list --max must be in 1..=5000".to_owned());
        }
        if !args.is_empty() {
            return Err(format!(
                "device-list accepts only --type KIND and --max N; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::DeviceList {
            target,
            selector,
            max,
        });
    }
    if spec.name == "device-watch" {
        if args.first().is_some_and(|arg| arg == "watch") {
            args.remove(0);
        }
        let selector =
            DeviceInventorySelector::parse(flag_text(args, "--type")?.as_deref().unwrap_or("all"))?;
        let max = flag_parsed::<usize>(args, "--max")?.unwrap_or(500);
        let duration_ms = flag_parsed::<u64>(args, "--duration-ms")?.unwrap_or(10_000);
        let interval_ms = flag_parsed::<u64>(args, "--interval-ms")?.unwrap_or(2_000);
        let event_max = flag_parsed::<usize>(args, "--event-max")?.unwrap_or(1_000);
        if !(1..=DEVICE_INVENTORY_MAX).contains(&max)
            || !(1_000..=DEVICE_WATCH_DURATION_MS_MAX).contains(&duration_ms)
            || !(DEVICE_WATCH_INTERVAL_MS_MIN..=DEVICE_WATCH_INTERVAL_MS_MAX).contains(&interval_ms)
            || !(1..=DEVICE_WATCH_EVENTS_MAX).contains(&event_max)
        {
            return Err("device watch requires --max 1..=5000, --duration-ms 1000..=3600000, --interval-ms 250..=60000 and --event-max 1..=5000".to_owned());
        }
        if !args.is_empty() {
            return Err(format!("device-watch received unexpected {:?}", args[0]));
        }
        let command = Command::DeviceWatch {
            target,
            selector,
            max,
            duration_ms,
            interval_ms,
            event_max,
        };
        command.validate().map_err(str::to_owned)?;
        return Ok(command);
    }
    if spec.name == "audit-compact" && args.first().is_some_and(|arg| arg == "compact") {
        args.remove(0);
    }
    if spec.name.starts_with("job-") {
        if args
            .first()
            .is_some_and(|arg| spec.aliases.contains(&format!("job {arg}").as_str()))
        {
            args.remove(0);
        }
        return parse_job(spec.name, target, args);
    }
    if spec.name.starts_with("session-") || spec.name.starts_with("lock-") {
        if args.first().is_some_and(|arg| {
            spec.aliases.contains(
                &format!(
                    "{} {arg}",
                    if spec.name.starts_with("session-") {
                        "session"
                    } else {
                        "lock"
                    }
                )
                .as_str(),
            )
        }) {
            args.remove(0);
        }
        return parse_runtime(spec.name, target, args);
    }
    if spec.name == "audit-query" {
        let verb_filter = flag_text(args, "--verb")?;
        let outcome = flag_text(args, "--outcome")?;
        let since_ms = flag_parsed::<u128>(args, "--since-ms")?;
        let offset = flag_parsed::<usize>(args, "--offset")?;
        let max = flag_parsed::<usize>(args, "--max")?;
        let scan_max = flag_parsed::<usize>(args, "--scan-max")?;
        let byte_max = flag_parsed::<usize>(args, "--byte-max")?;
        if !args.is_empty() {
            return Err(format!(
                "audit-query accepts only --verb/--outcome/--since-ms/--offset/--max/--scan-max/--byte-max; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::AuditQuery {
            target,
            verb_filter,
            outcome,
            since_ms,
            offset,
            max,
            scan_max,
            byte_max,
        });
    }
    if spec.name == "audit-compact" {
        let max_age_days = flag_parsed::<u64>(args, "--max-age-days")?;
        let max_events = flag_parsed::<usize>(args, "--max-events")?;
        let max_bytes = flag_parsed::<usize>(args, "--max-bytes")?;
        let apply = take_switch(args, "--apply");
        if !args.is_empty() {
            return Err(format!(
                "audit-compact accepts only --max-age-days/--max-events/--max-bytes/--apply; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::AuditCompact {
            target,
            max_age_days,
            max_events,
            max_bytes,
            apply,
        });
    }
    if spec.name == "host-open" {
        let value = positional(args, "TARGET")?;
        let application = flag_text(args, "--app")?;
        let background = take_switch(args, "--background");
        if !args.is_empty() {
            return Err(format!(
                "host-open accepts TARGET [--app APPLICATION] [--background]; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::HostOpen {
            target,
            value,
            application,
            background,
        });
    }
    if spec.name == "host-notify" {
        let subtitle = flag_text(args, "--subtitle")?;
        let sound = take_switch(args, "--sound");
        let title = positional(args, "TITLE")?;
        let body = if args.is_empty() {
            String::new()
        } else {
            args.remove(0)
        };
        if !args.is_empty() {
            return Err(format!(
                "host-notify accepts TITLE [BODY] [--subtitle TEXT] [--sound]; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::HostNotify {
            target,
            title,
            body,
            subtitle,
            sound,
        });
    }
    if spec.name == "permissions" {
        let action = match args.first().map(String::as_str) {
            None | Some("status") => {
                if !args.is_empty() {
                    args.remove(0);
                }
                PermissionAction::Status
            }
            Some("open") => {
                args.remove(0);
                PermissionAction::Open
            }
            Some(other) => {
                return Err(format!(
                    "permissions expects status or open [accessibility|screen-capture]; unexpected {other:?}"
                ));
            }
        };
        let permission = if action == PermissionAction::Open && !args.is_empty() {
            Some(match args.remove(0).as_str() {
                "accessibility" => PermissionKind::Accessibility,
                "screen-capture" | "screen-recording" => PermissionKind::ScreenCapture,
                other => {
                    return Err(format!(
                        "permission must be accessibility or screen-capture; got {other:?}"
                    ));
                }
            })
        } else {
            None
        };
        if !args.is_empty() {
            return Err(format!(
                "permissions accepts status or open [accessibility|screen-capture]; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::Permissions {
            target,
            action,
            permission,
        });
    }
    if !args.is_empty() {
        return Err(format!(
            "{} accepts no arguments; unexpected {:?}",
            spec.name, args[0]
        ));
    }
    match spec.name {
        "capabilities" => Ok(Command::Capabilities { target }),
        "doctor" => Ok(Command::Doctor { target }),
        other => Err(format!("unknown command '{other}'")),
    }
}

fn parse_job(name: &str, target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let command = match name {
        "job-spawn" => return parse_job_spawn(target, args),
        "job-list" => Command::JobList {
            target,
            state: flag_text(args, "--state")?
                .map(|value| parse_job_state(&value))
                .transpose()?,
            offset: flag_parsed(args, "--offset")?,
            max: flag_parsed(args, "--max")?,
        },
        "job-status" => Command::JobStatus {
            target,
            job_id: positional(args, "JOB_ID")?,
        },
        "job-resources" => Command::JobResources {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            watch_ms: flag_parsed(args, "--watch-ms")?,
        },
        "job-events" => Command::JobEvents {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            stdout_cursor: cursor_flag(args, "--stdout-cursor")?,
            stderr_cursor: cursor_flag(args, "--stderr-cursor")?,
            timeout_ms: flag_parsed(args, "--timeout-ms")?.unwrap_or(0),
            max_bytes: flag_parsed(args, "--max-bytes")?.unwrap_or(DEFAULT_JOB_EVENTS_MAX_BYTES),
        },
        "job-output" => Command::JobOutput {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            stream: match flag_text(args, "--stream")?.as_deref().unwrap_or("stdout") {
                "stdout" => JobOutputStream::Stdout,
                "stderr" => JobOutputStream::Stderr,
                value => return Err(format!("--stream must be stdout or stderr, got {value:?}")),
            },
            cursor: cursor_flag(args, "--cursor")?,
            max_bytes: flag_parsed(args, "--max-bytes")?.unwrap_or(DEFAULT_JOB_EVENTS_MAX_BYTES),
        },
        "job-write" => {
            let job_id = positional(args, "JOB_ID")?;
            let generation = positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?;
            let data_base64 = flag_text(args, "--data-base64")?.unwrap_or_default();
            let close_stdin = take_switch(args, "--close-stdin");
            if data_base64.is_empty() && !close_stdin {
                return Err("job-write requires --data-base64 DATA or --close-stdin".into());
            }
            Command::JobWrite {
                target,
                job_id,
                generation,
                data_base64,
                close_stdin,
            }
        }
        "job-wait" => Command::JobWait {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            timeout_ms: flag_parsed(args, "--timeout-ms")?.unwrap_or(0),
            expect_exit: flag_parsed(args, "--expect-exit")?,
        },
        "job-stop" => Command::JobStop {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            grace_ms: flag_parsed(args, "--grace-ms")?.unwrap_or(0),
            expect_stopped: take_switch(args, "--expect-stopped"),
        },
        "job-renew" => Command::JobRenew {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            ttl_seconds: ttl_flag(args, DEFAULT_JOB_TTL_SECONDS)?,
        },
        other => return Err(format!("unknown managed-job command '{other}'")),
    };
    if !args.is_empty() {
        return Err(format!("{name} received unexpected argument {:?}", args[0]));
    }
    command.validate().map_err(str::to_owned)?;
    Ok(command)
}

fn parse_job_spawn(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let separator = args
        .iter()
        .position(|arg| arg == "--")
        .ok_or_else(|| "job-spawn requires `-- PROGRAM [ARG...]`".to_owned())?;
    let command = args.split_off(separator + 1);
    args.pop();
    if command.is_empty() {
        return Err("job-spawn requires PROGRAM after `--`".into());
    }
    let mut environment = Vec::new();
    while let Some(value) = take_repeated_flag(args, "--env")? {
        let (name, value) = value
            .split_once('=')
            .ok_or_else(|| "--env requires NAME=VALUE".to_owned())?;
        environment.push(JobEnvironment {
            name: name.to_owned(),
            value: Some(value.to_owned()),
        });
    }
    while let Some(name) = take_repeated_flag(args, "--unset-env")? {
        environment.push(JobEnvironment { name, value: None });
    }
    let cwd = flag_text(args, "--cwd")?;
    let ttl_seconds = ttl_flag(args, DEFAULT_JOB_TTL_SECONDS)?;
    if !args.is_empty() {
        return Err(format!(
            "job-spawn received unexpected option {:?} before `--`",
            args[0]
        ));
    }
    let command = Command::JobSpawn {
        target,
        command,
        environment,
        cwd,
        ttl_seconds,
    };
    command.validate().map_err(str::to_owned)?;
    Ok(command)
}

fn take_repeated_flag(
    args: &mut Vec<String>,
    flag: &'static str,
) -> Result<Option<String>, String> {
    flag_text(args, flag)
}

fn cursor_flag(args: &mut Vec<String>, flag: &'static str) -> Result<JobOutputCursor, String> {
    JobOutputCursor::new(flag_text(args, flag)?.unwrap_or_else(|| "0".into()))
        .map_err(str::to_owned)
}

fn parse_job_state(value: &str) -> Result<JobStateFilter, String> {
    match value {
        "start-intent" => Ok(JobStateFilter::StartIntent),
        "starting" => Ok(JobStateFilter::Starting),
        "running" => Ok(JobStateFilter::Running),
        "start-failed" => Ok(JobStateFilter::StartFailed),
        "exited" => Ok(JobStateFilter::Exited),
        "signaled" => Ok(JobStateFilter::Signaled),
        "detached" => Ok(JobStateFilter::Detached),
        "orphaned-uncertain" => Ok(JobStateFilter::OrphanedUncertain),
        _ => Err(format!("unknown managed-job state {value:?}")),
    }
}

fn parse_runtime(name: &str, target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let command = match name {
        "session-start" => {
            let label = flag_text(args, "--label")?;
            let ttl_seconds = ttl_flag(args, DEFAULT_SESSION_TTL_SECONDS)?;
            Command::SessionStart {
                target,
                label,
                ttl_seconds,
            }
        }
        "session-list" => Command::SessionList { target },
        "session-status" => Command::SessionStatus {
            target,
            session_id: positional(args, "SESSION_ID")?,
        },
        "session-renew" => {
            let session_id = positional(args, "SESSION_ID")?;
            let lease = lease_value(args)?;
            let ttl_seconds = ttl_flag(args, DEFAULT_SESSION_TTL_SECONDS)?;
            Command::SessionRenew {
                target,
                session_id,
                lease,
                ttl_seconds,
            }
        }
        "session-end" => {
            let session_id = positional(args, "SESSION_ID")?;
            let lease = lease_value(args)?;
            let confirm = take_switch(args, "--confirm") || take_switch(args, "--force");
            Command::SessionEnd {
                target,
                session_id,
                lease,
                confirm,
            }
        }
        "lock-acquire" => {
            let session_id = positional(args, "SESSION_ID")?;
            let lease = positional_or_flag(args, "--lease", "LEASE")?;
            let lock_target = positional(args, "NAMESPACE:TARGET")?;
            let ttl_seconds = ttl_flag(args, DEFAULT_LOCK_TTL_SECONDS)?;
            Command::LockAcquire {
                target,
                session_id,
                lease,
                lock_target,
                ttl_seconds,
            }
        }
        "lock-list" => Command::LockList { target },
        "lock-release" => Command::LockRelease {
            target,
            lock_id: positional(args, "LOCK_ID")?,
            lease: lease_value(args)?,
        },
        other => return Err(format!("unknown runtime command '{other}'")),
    };
    if !args.is_empty() {
        return Err(format!("{name} received unexpected argument {:?}", args[0]));
    }
    Ok(command)
}

fn ttl_flag(args: &mut Vec<String>, default: u64) -> Result<u64, String> {
    let long = flag_parsed::<u64>(args, "--ttl-seconds")?;
    let short = flag_parsed::<u64>(args, "--ttl")?;
    if long.is_some() && short.is_some() {
        return Err("use only one of --ttl-seconds or --ttl".into());
    }
    Ok(long.or(short).unwrap_or(default))
}

fn lease_value(args: &mut Vec<String>) -> Result<String, String> {
    positional_or_flag(args, "--lease", "LEASE")
}

fn positional_or_flag(
    args: &mut Vec<String>,
    flag: &'static str,
    name: &str,
) -> Result<String, String> {
    if let Some(value) = flag_text(args, flag)? {
        return Ok(value);
    }
    positional(args, name)
}

fn positional(args: &mut Vec<String>, name: &str) -> Result<String, String> {
    if args.is_empty() || args[0].starts_with('-') {
        return Err(format!("{name} is required"));
    }
    Ok(args.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::verbs;

    fn parse(name: &str, args: &[&str]) -> Result<Command, String> {
        let spec = verbs::resolve(name, args.first().copied()).expect("runtime verb");
        let mut args = args.iter().map(|value| (*value).to_owned()).collect();
        super::parse(spec, TargetRef::Current, &mut args)
    }

    #[test]
    fn runtime_session_and_lock_shapes_are_closed() {
        let Command::SessionStart {
            label, ttl_seconds, ..
        } = parse("session-start", &["--label", "court", "--ttl", "12"]).unwrap()
        else {
            panic!("session-start command")
        };
        assert_eq!(label.as_deref(), Some("court"));
        assert_eq!(ttl_seconds, 12);

        let Command::LockAcquire {
            session_id,
            lease,
            lock_target,
            ttl_seconds,
            ..
        } = parse(
            "lock",
            &["acquire", "s1", "secret", "window:42", "--ttl", "9"],
        )
        .unwrap()
        else {
            panic!("lock-acquire command")
        };
        assert_eq!((session_id.as_str(), lease.as_str()), ("s1", "secret"));
        assert_eq!(lock_target, "window:42");
        assert_eq!(ttl_seconds, 9);

        assert!(parse("session-start", &["--ttl", "1", "extra"]).is_err());
        assert!(parse("session-renew", &["s1", "--lease"]).is_err());
        assert!(parse("lock-acquire", &["s1", "lease"]).is_err());
    }

    #[test]
    fn runtime_status_flat_and_daemon_alias_are_closed() {
        assert!(matches!(
            parse("runtime-status", &[]).unwrap(),
            Command::RuntimeStatus {
                target: TargetRef::Current
            }
        ));
        assert!(matches!(
            parse("daemon", &["status"]).unwrap(),
            Command::RuntimeStatus {
                target: TargetRef::Current
            }
        ));
        assert!(parse("daemon", &["status", "extra"]).is_err());
    }

    #[test]
    fn setup_apply_check_and_legacy_alias_are_closed() {
        assert!(matches!(
            parse("setup", &[]).unwrap(),
            Command::Setup {
                action: SetupAction::Apply,
                bin_dir: None,
                ..
            }
        ));
        assert!(matches!(
            parse("setup", &["--check", "--bin-dir", "fixture-bin"]).unwrap(),
            Command::Setup {
                action: SetupAction::Check,
                bin_dir: Some(path),
                ..
            } if path == "fixture-bin"
        ));
        assert!(matches!(
            parse("path-install", &[]).unwrap(),
            Command::Setup {
                action: SetupAction::Apply,
                ..
            }
        ));
        assert!(parse("setup", &["--check", "extra"]).is_err());
    }

    #[test]
    fn permissions_status_and_open_shapes_are_closed() {
        assert!(matches!(
            parse("permissions", &[]).unwrap(),
            Command::Permissions {
                action: PermissionAction::Status,
                permission: None,
                ..
            }
        ));
        assert!(matches!(
            parse("permissions", &["status"]).unwrap(),
            Command::Permissions {
                action: PermissionAction::Status,
                permission: None,
                ..
            }
        ));
        assert!(matches!(
            parse("permissions", &["open", "screen-capture"]).unwrap(),
            Command::Permissions {
                action: PermissionAction::Open,
                permission: Some(PermissionKind::ScreenCapture),
                ..
            }
        ));
        assert!(matches!(
            parse("permissions", &["open"]).unwrap(),
            Command::Permissions {
                action: PermissionAction::Open,
                permission: None,
                ..
            }
        ));
        assert!(parse("permissions", &["open", "camera"]).is_err());
        assert!(parse("permissions", &["status", "accessibility"]).is_err());
    }

    #[test]
    fn resource_status_flat_and_grouped_shapes_are_closed() {
        assert!(matches!(
            parse("resource-status", &[]).unwrap(),
            Command::ResourceStatus { .. }
        ));
        assert!(matches!(
            parse("resource", &["status"]).unwrap(),
            Command::ResourceStatus { .. }
        ));
        assert!(parse("resource-status", &["extra"]).is_err());
        assert!(parse("resource", &["status", "extra"]).is_err());
    }

    #[test]
    fn storage_devices_flat_and_grouped_shapes_are_closed() {
        assert!(matches!(
            parse("storage-devices", &["--max", "7"]).unwrap(),
            Command::StorageDevices { max: 7, .. }
        ));
        assert!(matches!(
            parse("storage", &["devices"]).unwrap(),
            Command::StorageDevices { max: 500, .. }
        ));
        assert!(parse("storage-devices", &["--max", "0"]).is_err());
        assert!(parse("storage", &["devices", "extra"]).is_err());
    }

    #[test]
    fn device_list_flat_and_grouped_shapes_are_closed() {
        assert!(matches!(
            parse("device-list", &["--type", "camera", "--max", "7"]).unwrap(),
            Command::DeviceList {
                selector: DeviceInventorySelector::Camera,
                max: 7,
                ..
            }
        ));
        assert!(matches!(
            parse("device", &["list"]).unwrap(),
            Command::DeviceList {
                selector: DeviceInventorySelector::All,
                max: 500,
                ..
            }
        ));
        assert!(parse("device-list", &["--type", "serial"]).is_err());
        assert!(parse("device", &["list", "--max", "0"]).is_err());
        assert!(parse("device", &["list", "extra"]).is_err());
    }

    #[test]
    fn device_watch_flat_and_grouped_shapes_are_closed() {
        assert!(matches!(
            parse(
                "device-watch",
                &[
                    "--type",
                    "usb",
                    "--max",
                    "7",
                    "--duration-ms",
                    "1000",
                    "--interval-ms",
                    "250",
                    "--event-max",
                    "9",
                ],
            )
            .unwrap(),
            Command::DeviceWatch {
                target: TargetRef::Current,
                selector: DeviceInventorySelector::Usb,
                max: 7,
                duration_ms: 1_000,
                interval_ms: 250,
                event_max: 9,
            }
        ));
        assert!(matches!(
            parse("device", &["watch"]).unwrap(),
            Command::DeviceWatch {
                selector: DeviceInventorySelector::All,
                max: 500,
                duration_ms: 10_000,
                interval_ms: 2_000,
                event_max: 1_000,
                ..
            }
        ));
        assert!(parse("device-watch", &["--duration-ms", "999"]).is_err());
        assert!(parse("device-watch", &["--interval-ms", "249"]).is_err());
        assert!(parse("device-watch", &["--event-max", "5001"]).is_err());
    }

    #[test]
    fn managed_job_spawn_preserves_child_arguments_after_separator() {
        let Command::JobSpawn {
            command,
            environment,
            cwd,
            ttl_seconds,
            ..
        } = parse(
            "job",
            &[
                "spawn",
                "--env",
                "A=1",
                "--unset-env",
                "B",
                "--cwd",
                ".",
                "--ttl",
                "12",
                "--",
                "program",
                "--env",
                "child-value",
            ],
        )
        .unwrap()
        else {
            panic!("job-spawn command")
        };
        assert_eq!(command, ["program", "--env", "child-value"]);
        assert_eq!(
            environment,
            [
                JobEnvironment {
                    name: "A".into(),
                    value: Some("1".into())
                },
                JobEnvironment {
                    name: "B".into(),
                    value: None
                }
            ]
        );
        assert_eq!(cwd.as_deref(), Some("."));
        assert_eq!(ttl_seconds, 12);
        assert!(parse("job-spawn", &["program"]).is_err());
    }

    #[test]
    fn managed_job_lifecycle_shapes_are_closed_and_bounded() {
        let id = "123e4567-e89b-42d3-a456-426614174000";
        assert!(matches!(
            parse("job", &["status", id]).unwrap(),
            Command::JobStatus { .. }
        ));
        let Command::JobEvents {
            stdout_cursor,
            stderr_cursor,
            max_bytes,
            ..
        } = parse(
            "job-events",
            &[
                id,
                "1",
                "--stdout-cursor",
                "7",
                "--stderr-cursor",
                "9",
                "--max-bytes",
                "128",
            ],
        )
        .unwrap()
        else {
            panic!("job-events command")
        };
        assert_eq!(stdout_cursor.as_str(), "7");
        assert_eq!(stderr_cursor.as_str(), "9");
        assert_eq!(max_bytes, 128);
        let Command::JobOutput {
            stream,
            cursor,
            max_bytes,
            ..
        } = parse(
            "job",
            &[
                "output",
                id,
                "1",
                "--stream",
                "stderr",
                "--cursor",
                "11",
                "--max-bytes",
                "1",
            ],
        )
        .unwrap()
        else {
            panic!("job-output command")
        };
        assert_eq!(stream, JobOutputStream::Stderr);
        assert_eq!(cursor.as_str(), "11");
        assert_eq!(max_bytes, 1);
        let Command::JobResources {
            generation,
            watch_ms,
            ..
        } = parse("job", &["resources", id, "2", "--watch-ms", "25"]).unwrap()
        else {
            panic!("job-resources command")
        };
        assert_eq!(generation, 2);
        assert_eq!(watch_ms, Some(25));
        assert!(parse("job-resources", &[id, "1", "--watch-ms", "0"]).is_err());
        assert!(parse("job-resources", &[id, "1", "--watch-ms", "300001"]).is_err());
        assert!(parse("job-output", &[id, "1", "--stream", "merged"]).is_err());
        assert!(parse("job-events", &[id, "0"]).is_err());
        assert!(parse("job-write", &[id, "1"]).is_err());
        assert!(matches!(
            parse("job-write", &[id, "1", "--close-stdin"]).unwrap(),
            Command::JobWrite {
                close_stdin: true,
                ..
            }
        ));
    }
}
