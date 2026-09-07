//! Host-level discovery that is independent of the desktop/window family.

use agenterm_cu::{
    Command, DeviceInventorySelector, PermissionAction, PermissionKind, SetupAction, TargetRef,
    command::{
        DEVICE_INVENTORY_MAX, DEVICE_IO_BYTES_MAX, DEVICE_WATCH_DURATION_MS_MAX,
        DEVICE_WATCH_EVENTS_MAX, DEVICE_WATCH_INTERVAL_MS_MAX, DEVICE_WATCH_INTERVAL_MS_MIN,
        DeviceDataEncoding, DeviceSerialConfiguration, DeviceSerialFlow, DeviceSerialParity,
        JobEnvironment, JobOutputCursor, JobOutputStream, JobPolicyAction, JobPolicyEnforcement,
        JobProcessLimits, JobResourcePolicy, JobStateFilter, ProcessRunState, ProcessSignalKind,
        STORAGE_DEVICES_MAX,
    },
    service_control::{ServiceOperation, ServiceScope},
};

use super::{flag_parsed, flag_text, take_switch, verbs::VerbSpec};

const DEFAULT_SESSION_TTL_SECONDS: u64 = 3_600;
const DEFAULT_LOCK_TTL_SECONDS: u64 = 300;
const DEFAULT_JOB_TTL_SECONDS: u64 = 3_600;
const DEFAULT_JOB_EVENTS_MAX_BYTES: usize = 65_536;
const DEFAULT_DEVICE_TTL_SECONDS: u64 = 300;
const DEFAULT_DEVICE_IO_TIMEOUT_MS: u64 = 1_000;

pub fn parse(
    spec: &VerbSpec,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spec.name == "service" {
        let subcommand = args
            .first()
            .cloned()
            .ok_or_else(|| "service requires list, status, plan or apply".to_owned())?;
        args.remove(0);
        let command = match subcommand.as_str() {
            "list" | "system-list" => Command::ServiceList {
                target,
                scope: if subcommand == "system-list" {
                    ServiceScope::System
                } else {
                    ServiceScope::User
                },
                match_text: flag_text(args, "--match")?,
                max: flag_parsed::<usize>(args, "--max")?.unwrap_or(500),
            },
            "status" | "system-status" => {
                let name = args
                    .first()
                    .cloned()
                    .ok_or_else(|| format!("service {subcommand} requires a name"))?;
                args.remove(0);
                Command::ServiceStatus {
                    target,
                    scope: if subcommand == "system-status" {
                        ServiceScope::System
                    } else {
                        ServiceScope::User
                    },
                    name,
                }
            }
            "plan" => {
                let operation = args
                    .first()
                    .map(String::as_str)
                    .ok_or_else(|| "service plan requires an operation".to_owned())?;
                let operation = match operation {
                    "start" => ServiceOperation::Start,
                    "stop" => ServiceOperation::Stop,
                    "restart" => ServiceOperation::Restart,
                    "bootstrap" => ServiceOperation::Bootstrap,
                    "bootout" => ServiceOperation::Bootout,
                    other => return Err(format!("unknown service plan operation {other:?}")),
                };
                args.remove(0);
                let name = args
                    .first()
                    .cloned()
                    .ok_or_else(|| "service plan requires a service name".to_owned())?;
                args.remove(0);
                let scope = match flag_text(args, "--scope")?.as_deref() {
                    None | Some("user") => ServiceScope::User,
                    Some("system") => ServiceScope::System,
                    Some(_) => return Err("service --scope must be user or system".to_owned()),
                };
                Command::ServicePlan {
                    target,
                    scope,
                    name,
                    operation,
                    definition: flag_text(args, "--definition")?,
                    ttl_seconds: flag_parsed::<u64>(args, "--ttl-seconds")?.unwrap_or(120),
                }
            }
            "apply" => Command::ServiceApply {
                target,
                request: flag_text(args, "--request")?
                    .ok_or_else(|| "service apply requires --request R".to_owned())?,
                approval: flag_text(args, "--approve")?
                    .ok_or_else(|| "service apply requires --approve H".to_owned())?,
            },
            "bootstrap" | "start" | "restart" | "stop" | "bootout" => {
                let operation = match subcommand.as_str() {
                    "bootstrap" => ServiceOperation::Bootstrap,
                    "start" => ServiceOperation::Start,
                    "restart" => ServiceOperation::Restart,
                    "stop" => ServiceOperation::Stop,
                    "bootout" => ServiceOperation::Bootout,
                    _ => unreachable!(),
                };
                let operand = args
                    .first()
                    .cloned()
                    .ok_or_else(|| format!("service {subcommand} requires a name or plist"))?;
                args.remove(0);
                if subcommand == "bootout" && !take_switch(args, "--force") {
                    return Err("service bootout requires --force".to_owned());
                }
                let scope = match flag_text(args, "--scope")?.as_deref() {
                    None | Some("user") => ServiceScope::User,
                    Some("system") => ServiceScope::System,
                    Some(_) => return Err("service --scope must be user or system".to_owned()),
                };
                Command::ServiceTransact {
                    target,
                    scope,
                    operation,
                    name: (subcommand != "bootstrap").then_some(operand.clone()),
                    definition: (subcommand == "bootstrap").then_some(operand),
                    ttl_seconds: flag_parsed::<u64>(args, "--ttl-seconds")?.unwrap_or(120),
                }
            }
            other => return Err(format!("unknown service subcommand {other:?}")),
        };
        if !args.is_empty() {
            return Err(format!(
                "service {subcommand} has unexpected argument {:?}",
                args[0]
            ));
        }
        command.validate().map_err(str::to_owned)?;
        return Ok(command);
    }
    if spec.name == "audio" {
        let subcommand = args
            .first()
            .map(String::as_str)
            .ok_or_else(|| "audio requires status, plan volume|muted or apply".to_owned())?;
        match subcommand {
            "status" => {
                args.remove(0);
                if !args.is_empty() {
                    return Err(format!(
                        "audio status accepts no arguments; unexpected {:?}",
                        args[0]
                    ));
                }
                return Ok(Command::AudioStatus { target });
            }
            "plan" => {
                args.remove(0);
                let property = args
                    .first()
                    .cloned()
                    .ok_or_else(|| "audio plan requires volume or muted".to_owned())?;
                args.remove(0);
                let value = args
                    .first()
                    .cloned()
                    .ok_or_else(|| format!("audio plan {property} requires a value"))?;
                args.remove(0);
                let ttl_seconds = flag_parsed::<u64>(args, "--ttl-seconds")?.unwrap_or(120);
                if !args.is_empty() {
                    return Err(format!(
                        "audio plan accepts only --ttl-seconds N; unexpected {:?}",
                        args[0]
                    ));
                }
                let command = match property.as_str() {
                    "volume" => Command::AudioPlanVolume {
                        target,
                        volume: value
                            .parse::<u8>()
                            .map_err(|_| "audio volume must be an integer in 0..=100".to_owned())?,
                        ttl_seconds,
                    },
                    "muted" => Command::AudioPlanMuted {
                        target,
                        muted: match value.as_str() {
                            "true" => true,
                            "false" => false,
                            _ => return Err("audio muted must be true or false".to_owned()),
                        },
                        ttl_seconds,
                    },
                    _ => {
                        return Err("audio plan currently supports only volume or muted".to_owned());
                    }
                };
                command.validate().map_err(str::to_owned)?;
                return Ok(command);
            }
            "apply" => {
                args.remove(0);
                let request = flag_text(args, "--request")?
                    .ok_or_else(|| "audio apply requires --request R".to_owned())?;
                let approval = flag_text(args, "--approve")?
                    .ok_or_else(|| "audio apply requires --approve H".to_owned())?;
                if !args.is_empty() {
                    return Err(format!(
                        "audio apply accepts only --request R and --approve H; unexpected {:?}",
                        args[0]
                    ));
                }
                let command = Command::AudioApply {
                    target,
                    request,
                    approval,
                };
                command.validate().map_err(str::to_owned)?;
                return Ok(command);
            }
            other => return Err(format!("unknown audio subcommand {other:?}")),
        }
    }
    if spec.name == "login-session" {
        let subcommand = args
            .first()
            .map(String::as_str)
            .ok_or_else(|| "login-session requires status, plan lock or apply".to_owned())?;
        match subcommand {
            "status" => {
                args.remove(0);
                if !args.is_empty() {
                    return Err(format!(
                        "login-session status accepts no arguments; unexpected {:?}",
                        args[0]
                    ));
                }
                return Ok(Command::LoginSessionStatus { target });
            }
            "plan" => {
                args.remove(0);
                if args.first().map(String::as_str) != Some("lock") {
                    return Err("login-session plan currently supports only lock".to_owned());
                }
                args.remove(0);
                let ttl_seconds = flag_parsed::<u64>(args, "--ttl-seconds")?.unwrap_or(120);
                if !args.is_empty() {
                    return Err(format!(
                        "login-session plan lock accepts only --ttl-seconds N; unexpected {:?}",
                        args[0]
                    ));
                }
                let command = Command::LoginSessionPlanLock {
                    target,
                    ttl_seconds,
                };
                command.validate().map_err(str::to_owned)?;
                return Ok(command);
            }
            "apply" => {
                args.remove(0);
                let request = flag_text(args, "--request")?
                    .ok_or_else(|| "login-session apply requires --request R".to_owned())?;
                let approval = flag_text(args, "--approve")?
                    .ok_or_else(|| "login-session apply requires --approve H".to_owned())?;
                if !args.is_empty() {
                    return Err(format!(
                        "login-session apply accepts only --request R and --approve H; unexpected {:?}",
                        args[0]
                    ));
                }
                let command = Command::LoginSessionApplyLock {
                    target,
                    request,
                    approval,
                };
                command.validate().map_err(str::to_owned)?;
                return Ok(command);
            }
            other => return Err(format!("unknown login-session subcommand {other:?}")),
        }
    }
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
    if spec.name == "power-status" {
        if args.first().is_some_and(|arg| arg == "status") {
            args.remove(0);
        }
        if !args.is_empty() {
            return Err(format!(
                "power-status accepts no arguments; unexpected {:?}",
                args[0]
            ));
        }
        return Ok(Command::PowerStatus { target });
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
    if matches!(
        spec.name,
        "device-claims"
            | "device-claim"
            | "device-status"
            | "device-read"
            | "device-write"
            | "device-renew"
            | "device-release"
    ) {
        if args
            .first()
            .is_some_and(|arg| spec.aliases.contains(&format!("device {arg}").as_str()))
        {
            args.remove(0);
        }
        return parse_device_lease(spec.name, target, args);
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

fn parse_device_lease(
    name: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    let command = match name {
        "device-claims" => Command::DeviceClaims {
            target,
            offset: flag_parsed(args, "--offset")?,
            max: flag_parsed(args, "--max")?,
        },
        "device-claim" => {
            let device_id = positional(args, "DEVICE_ID")?;
            let ttl_seconds = ttl_flag(args, DEFAULT_DEVICE_TTL_SECONDS)?;
            let baud = flag_parsed::<u32>(args, "--baud")?;
            let data_bits = flag_parsed::<u8>(args, "--data-bits")?;
            let parity = flag_text(args, "--parity")?;
            let stop_bits = flag_parsed::<u8>(args, "--stop-bits")?;
            let flow = flag_text(args, "--flow")?;
            let serial_requested = baud.is_some()
                || data_bits.is_some()
                || parity.is_some()
                || stop_bits.is_some()
                || flow.is_some();
            let serial = if serial_requested {
                Some(DeviceSerialConfiguration {
                    baud: baud.unwrap_or(9_600),
                    data_bits: data_bits.unwrap_or(8),
                    parity: match parity.as_deref().unwrap_or("none") {
                        "none" => DeviceSerialParity::None,
                        "even" => DeviceSerialParity::Even,
                        "odd" => DeviceSerialParity::Odd,
                        value => {
                            return Err(format!(
                                "--parity must be none, even or odd; got {value:?}"
                            ));
                        }
                    },
                    stop_bits: stop_bits.unwrap_or(1),
                    flow: match flow.as_deref().unwrap_or("none") {
                        "none" => DeviceSerialFlow::None,
                        "software" => DeviceSerialFlow::Software,
                        "hardware" => DeviceSerialFlow::Hardware,
                        value => {
                            return Err(format!(
                                "--flow must be none, software or hardware; got {value:?}"
                            ));
                        }
                    },
                })
            } else {
                None
            };
            Command::DeviceClaim {
                target,
                device_id,
                ttl_seconds,
                serial,
            }
        }
        "device-status" => Command::DeviceStatus {
            target,
            lease_id: positional(args, "LEASE_ID")?,
            generation: generation_flag(args)?,
        },
        "device-read" => Command::DeviceRead {
            target,
            lease_id: positional(args, "LEASE_ID")?,
            generation: generation_flag(args)?,
            lease: positional_or_flag(args, "--lease", "LEASE")?,
            max_bytes: flag_parsed(args, "--max-bytes")?.unwrap_or(DEVICE_IO_BYTES_MAX),
            timeout_ms: flag_parsed(args, "--timeout-ms")?.unwrap_or(DEFAULT_DEVICE_IO_TIMEOUT_MS),
            encoding: parse_data_encoding(
                flag_text(args, "--encoding")?
                    .as_deref()
                    .unwrap_or("base64"),
            )?,
        },
        "device-write" => {
            let lease_id = positional(args, "LEASE_ID")?;
            let generation = generation_flag(args)?;
            let lease = positional_or_flag(args, "--lease", "LEASE")?;
            let data_base64 = flag_text(args, "--data-base64")?;
            let data_hex = flag_text(args, "--hex")?;
            let (data, encoding) = match (data_base64, data_hex) {
                (Some(data), None) => (data, DeviceDataEncoding::Base64),
                (None, Some(data)) => (data, DeviceDataEncoding::Hex),
                _ => {
                    return Err(
                        "device-write requires exactly one of --data-base64 or --hex".into(),
                    );
                }
            };
            Command::DeviceWrite {
                target,
                lease_id,
                generation,
                lease,
                data,
                encoding,
                timeout_ms: flag_parsed(args, "--timeout-ms")?
                    .unwrap_or(DEFAULT_DEVICE_IO_TIMEOUT_MS),
            }
        }
        "device-renew" => Command::DeviceRenew {
            target,
            lease_id: positional(args, "LEASE_ID")?,
            generation: generation_flag(args)?,
            lease: positional_or_flag(args, "--lease", "LEASE")?,
            ttl_seconds: ttl_flag(args, DEFAULT_DEVICE_TTL_SECONDS)?,
        },
        "device-release" => Command::DeviceRelease {
            target,
            lease_id: positional(args, "LEASE_ID")?,
            generation: generation_flag(args)?,
            lease: positional_or_flag(args, "--lease", "LEASE")?,
        },
        other => return Err(format!("unknown device lease command {other:?}")),
    };
    if !args.is_empty() {
        return Err(format!("{name} received unexpected argument {:?}", args[0]));
    }
    command.validate().map_err(str::to_owned)?;
    Ok(command)
}

fn generation_flag(args: &mut Vec<String>) -> Result<u64, String> {
    flag_parsed(args, "--generation")?.ok_or_else(|| "--generation N is required".to_owned())
}

fn parse_data_encoding(value: &str) -> Result<DeviceDataEncoding, String> {
    match value {
        "base64" => Ok(DeviceDataEncoding::Base64),
        "hex" => Ok(DeviceDataEncoding::Hex),
        _ => Err(format!("--encoding must be base64 or hex; got {value:?}")),
    }
}

fn parse_job(name: &str, target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let command = match name {
        "job-spawn" => return parse_job_spawn(target, args),
        "job-adopt" => {
            let pid = positional(args, "PID")?
                .parse()
                .map_err(|_| "PID must be an integer in 2..=4294967295".to_owned())?;
            let start_identity = flag_text(args, "--start-identity")?
                .ok_or_else(|| "job-adopt requires --start-identity ID".to_owned())?;
            let ttl_seconds = ttl_flag(args, DEFAULT_JOB_TTL_SECONDS)?;
            let expiry = flag_text(args, "--expiry")?.unwrap_or_else(|| "detach".to_owned());
            let force = take_switch(args, "--force");
            let stop_on_expiry = match expiry.as_str() {
                "detach" => false,
                "stop" => true,
                _ => return Err("job-adopt --expiry must be detach or stop".to_owned()),
            };
            Command::JobAdopt {
                target,
                pid,
                start_identity,
                ttl_seconds,
                stop_on_expiry,
                force,
            }
        }
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
        "job-prune" => Command::JobPrune {
            target,
            max_age_seconds: flag_parsed(args, "--max-age-seconds")?.unwrap_or(86_400),
            keep_newest: flag_parsed(args, "--keep-newest")?.unwrap_or(128),
            apply: take_switch(args, "--apply"),
        },
        "job-resources" => Command::JobResources {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            watch_ms: flag_parsed(args, "--watch-ms")?,
        },
        "job-priority" => Command::JobPriority {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            nice: positional(args, "NICE")?
                .parse()
                .map_err(|_| "NICE must be an integer in -20..=19".to_owned())?,
        },
        "job-policy" => {
            let job_id = positional(args, "JOB_ID")?;
            let generation = positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?;
            let action = match positional(args, "status|set|clear")?.as_str() {
                "status" => JobPolicyAction::Status,
                "set" => JobPolicyAction::Set,
                "clear" => JobPolicyAction::Clear,
                value => {
                    return Err(format!(
                        "job policy action must be status, set or clear, got {value:?}"
                    ));
                }
            };
            let force = take_switch(args, "--force");
            let policy = if action == JobPolicyAction::Set {
                Some(JobResourcePolicy {
                    max_rss_bytes: flag_parsed(args, "--max-rss-bytes")?,
                    max_cpu_pct: flag_parsed(args, "--max-cpu-pct")?,
                    max_processes: flag_parsed(args, "--max-processes")?,
                    interval_ms: flag_parsed(args, "--interval-ms")?.unwrap_or(1_000),
                    consecutive_samples: flag_parsed(args, "--samples")?.unwrap_or(3),
                    action: match flag_text(args, "--action")?.as_deref().unwrap_or("stop") {
                        "stop" => JobPolicyEnforcement::Stop,
                        "terminate" => JobPolicyEnforcement::Terminate,
                        value => {
                            return Err(format!(
                                "job policy --action must be stop or terminate, got {value:?}"
                            ));
                        }
                    },
                })
            } else {
                None
            };
            Command::JobPolicy {
                target,
                job_id,
                generation,
                action,
                policy,
                force,
            }
        }
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
        "job-set-state" => Command::JobSetState {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            state: match positional(args, "running|stopped")?.as_str() {
                "running" => ProcessRunState::Running,
                "stopped" => ProcessRunState::Stopped,
                value => {
                    return Err(format!(
                        "job state must be running or stopped, got {value:?}"
                    ));
                }
            },
            timeout_ms: flag_parsed(args, "--timeout-ms")?.unwrap_or(5_000),
        },
        "job-signal" => Command::JobSignal {
            target,
            job_id: positional(args, "JOB_ID")?,
            generation: positional(args, "GENERATION")?
                .parse()
                .map_err(|_| "GENERATION must be a positive integer".to_owned())?,
            signal: ProcessSignalKind::parse(&positional(args, "SIGNAL")?).ok_or_else(|| {
                "SIGNAL must be HUP, INT, TERM, KILL, STOP, CONT, USR1 or USR2".to_owned()
            })?,
            timeout_ms: flag_parsed(args, "--timeout-ms")?.unwrap_or(5_000),
            force: take_switch(args, "--force"),
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
    let cpu_seconds = flag_parsed(args, "--cpu-seconds")?;
    let memory_bytes = flag_parsed(args, "--memory-bytes")?;
    let file_size_bytes = flag_parsed(args, "--file-size-bytes")?;
    let open_files = flag_parsed(args, "--max-open-files")?;
    let processes = flag_parsed(args, "--max-processes")?;
    let limits = if cpu_seconds.is_some()
        || memory_bytes.is_some()
        || file_size_bytes.is_some()
        || open_files.is_some()
        || processes.is_some()
    {
        Some(JobProcessLimits {
            cpu_seconds,
            memory_bytes,
            file_size_bytes,
            open_files,
            processes,
        })
    } else {
        None
    };
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
        limits,
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
    fn login_session_status_plan_and_apply_are_closed() {
        assert!(matches!(
            parse("login-session", &["status"]).unwrap(),
            Command::LoginSessionStatus {
                target: TargetRef::Current
            }
        ));
        assert!(matches!(
            parse("login-session", &["plan", "lock", "--ttl-seconds", "60"]).unwrap(),
            Command::LoginSessionPlanLock {
                ttl_seconds: 60,
                ..
            }
        ));
        let request = "00";
        let approval = "a".repeat(64);
        assert!(matches!(
            parse(
                "login-session",
                &["apply", "--request", request, "--approve", &approval]
            )
            .unwrap(),
            Command::LoginSessionApplyLock {
                request: encoded,
                approval: digest,
                ..
            } if encoded == request && digest == approval
        ));
        assert!(parse("login-session", &["plan", "unlock"]).is_err());
        assert!(parse("login-session", &["plan", "lock", "--ttl-seconds", "0"]).is_err());
        assert!(parse("login-session", &["apply", "--request", request]).is_err());
    }

    #[test]
    fn audio_status_plan_and_apply_are_closed() {
        assert!(matches!(
            parse("audio", &["status"]).unwrap(),
            Command::AudioStatus { .. }
        ));
        assert!(matches!(
            parse("audio", &["plan", "volume", "60", "--ttl-seconds", "30"]).unwrap(),
            Command::AudioPlanVolume {
                volume: 60,
                ttl_seconds: 30,
                ..
            }
        ));
        assert!(matches!(
            parse("audio", &["plan", "muted", "true"]).unwrap(),
            Command::AudioPlanMuted { muted: true, .. }
        ));
        assert!(matches!(
            parse(
                "audio",
                &[
                    "apply",
                    "--request",
                    "REQUEST",
                    "--approve",
                    &"a".repeat(64)
                ]
            )
            .unwrap(),
            Command::AudioApply { .. }
        ));
        assert!(parse("audio", &["plan", "volume", "101"]).is_err());
        assert!(parse("audio", &["plan", "muted", "yes"]).is_err());
        assert!(parse("audio", &["apply", "--request", "REQUEST"]).is_err());
    }

    #[test]
    fn service_observation_plan_and_apply_are_closed() {
        assert!(matches!(
            parse("service", &["list", "--match", "agent", "--max", "5000"]).unwrap(),
            Command::ServiceList {
                scope: ServiceScope::User,
                match_text: Some(value),
                max: 5000,
                ..
            } if value == "agent"
        ));
        assert!(matches!(
            parse("service", &["system-status", "example.service"]).unwrap(),
            Command::ServiceStatus {
                scope: ServiceScope::System,
                name,
                ..
            } if name == "example.service"
        ));
        assert!(matches!(
            parse(
                "service",
                &[
                    "plan",
                    "bootstrap",
                    "example.service",
                    "--definition",
                    "fixture.plist",
                    "--ttl-seconds",
                    "30"
                ]
            )
            .unwrap(),
            Command::ServicePlan {
                operation: ServiceOperation::Bootstrap,
                ttl_seconds: 30,
                ..
            }
        ));
        assert!(matches!(
            parse(
                "service",
                &[
                    "apply",
                    "--request",
                    "REQUEST",
                    "--approve",
                    &"a".repeat(64)
                ]
            )
            .unwrap(),
            Command::ServiceApply { .. }
        ));
        assert!(matches!(
            parse("service", &["restart", "example.service", "--ttl-seconds", "30"])
                .unwrap(),
            Command::ServiceTransact {
                operation: ServiceOperation::Restart,
                name: Some(name),
                definition: None,
                ttl_seconds: 30,
                ..
            } if name == "example.service"
        ));
        assert!(matches!(
            parse("service", &["bootstrap", "fixture.plist"]).unwrap(),
            Command::ServiceTransact {
                operation: ServiceOperation::Bootstrap,
                name: None,
                definition: Some(path),
                ..
            } if path == "fixture.plist"
        ));
        assert!(parse("service", &["bootout", "example.service"]).is_err());
        assert!(matches!(
            parse("service", &["bootout", "example.service", "--force"]).unwrap(),
            Command::ServiceTransact {
                operation: ServiceOperation::Bootout,
                name: Some(name),
                ..
            } if name == "example.service"
        ));
        assert!(parse("service", &["list", "--max", "5001"]).is_err());
        assert!(parse("service", &["plan", "bootstrap", "example.service"]).is_err());
        assert!(parse("service", &["apply", "--request", "REQUEST"]).is_err());
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
    fn power_status_flat_and_grouped_shapes_are_closed() {
        assert!(matches!(
            parse("power-status", &[]).unwrap(),
            Command::PowerStatus { .. }
        ));
        assert!(matches!(
            parse("power", &["status"]).unwrap(),
            Command::PowerStatus { .. }
        ));
        assert!(parse("power-status", &["extra"]).is_err());
        assert!(parse("power", &["status", "extra"]).is_err());
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
    fn device_lease_shapes_keep_authority_and_payload_flags_closed() {
        let device_id = format!("agt-device-v1-{}", "a".repeat(64));
        let lease_id = "00000000-0000-4000-8000-000000000001";
        let lease = "b".repeat(64);
        assert!(matches!(
            parse(
                "device",
                &[
                    "claim", &device_id, "--ttl", "60", "--baud", "115200", "--parity", "even",
                ],
            )
            .unwrap(),
            Command::DeviceClaim {
                ttl_seconds: 60,
                serial: Some(DeviceSerialConfiguration {
                    baud: 115_200,
                    parity: DeviceSerialParity::Even,
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            parse(
                "device-read",
                &[
                    lease_id,
                    "--generation",
                    "1",
                    "--lease",
                    &lease,
                    "--max-bytes",
                    "7",
                    "--encoding",
                    "hex",
                ],
            )
            .unwrap(),
            Command::DeviceRead {
                generation: 1,
                max_bytes: 7,
                encoding: DeviceDataEncoding::Hex,
                ..
            }
        ));
        assert!(matches!(
            parse(
                "device",
                &[
                    "write",
                    lease_id,
                    "--generation",
                    "1",
                    "--lease",
                    &lease,
                    "--hex",
                    "00ff",
                ],
            )
            .unwrap(),
            Command::DeviceWrite {
                data,
                encoding: DeviceDataEncoding::Hex,
                ..
            } if data == "00ff"
        ));
        assert!(
            parse(
                "device-write",
                &[
                    lease_id,
                    "--generation",
                    "1",
                    "--lease",
                    &lease,
                    "--hex",
                    "00",
                    "--data-base64",
                    "AA==",
                ],
            )
            .is_err()
        );
        assert!(parse("device-status", &[lease_id]).is_err());
    }

    #[test]
    fn managed_job_spawn_preserves_child_arguments_after_separator() {
        let Command::JobSpawn {
            command,
            environment,
            cwd,
            limits,
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
                "--cpu-seconds",
                "60",
                "--max-processes",
                "32",
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
        assert_eq!(
            limits,
            Some(JobProcessLimits {
                cpu_seconds: Some(60),
                memory_bytes: None,
                file_size_bytes: None,
                open_files: None,
                processes: Some(32),
            })
        );
        assert!(parse("job-spawn", &["program"]).is_err());
    }

    #[test]
    fn managed_job_lifecycle_shapes_are_closed_and_bounded() {
        let id = "123e4567-e89b-42d3-a456-426614174000";
        assert!(matches!(
            parse(
                "job",
                &[
                    "adopt",
                    "42",
                    "--start-identity",
                    "opaque-start",
                    "--ttl-seconds",
                    "12",
                    "--expiry",
                    "detach",
                ],
            )
            .unwrap(),
            Command::JobAdopt {
                pid: 42,
                ttl_seconds: 12,
                stop_on_expiry: false,
                force: false,
                ..
            }
        ));
        assert!(
            parse(
                "job-adopt",
                &["42", "--start-identity", "opaque-start", "--expiry", "stop",],
            )
            .is_err()
        );
        assert!(matches!(
            parse(
                "job-adopt",
                &[
                    "42",
                    "--start-identity",
                    "opaque-start",
                    "--expiry",
                    "stop",
                    "--force",
                ],
            )
            .unwrap(),
            Command::JobAdopt {
                stop_on_expiry: true,
                force: true,
                ..
            }
        ));
        assert!(matches!(
            parse("job", &["status", id]).unwrap(),
            Command::JobStatus { .. }
        ));
        assert!(matches!(
            parse(
                "job",
                &[
                    "prune",
                    "--max-age-seconds",
                    "0",
                    "--keep-newest",
                    "7",
                    "--apply",
                ],
            )
            .unwrap(),
            Command::JobPrune {
                max_age_seconds: 0,
                keep_newest: 7,
                apply: true,
                ..
            }
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
        assert!(matches!(
            parse("job", &["priority", id, "2", "7"]).unwrap(),
            Command::JobPriority {
                generation: 2,
                nice: 7,
                ..
            }
        ));
        assert!(parse("job-priority", &[id, "1", "20"]).is_err());
        assert!(matches!(
            parse("job-policy", &[id, "2", "status"]).unwrap(),
            Command::JobPolicy {
                action: JobPolicyAction::Status,
                policy: None,
                force: false,
                ..
            }
        ));
        let Command::JobPolicy {
            action,
            policy: Some(policy),
            force,
            ..
        } = parse(
            "job",
            &[
                "policy",
                id,
                "2",
                "set",
                "--max-rss-bytes",
                "1048576",
                "--max-cpu-pct",
                "250",
                "--max-processes",
                "8",
                "--interval-ms",
                "250",
                "--samples",
                "2",
                "--action",
                "terminate",
                "--force",
            ],
        )
        .unwrap()
        else {
            panic!("job-policy command")
        };
        assert_eq!(action, JobPolicyAction::Set);
        assert_eq!(policy.max_rss_bytes, Some(1_048_576));
        assert_eq!(policy.max_cpu_pct, Some(250));
        assert_eq!(policy.max_processes, Some(8));
        assert_eq!(policy.interval_ms, 250);
        assert_eq!(policy.consecutive_samples, 2);
        assert_eq!(policy.action, JobPolicyEnforcement::Terminate);
        assert!(force);
        assert!(parse("job-policy", &[id, "2", "set"]).is_err());
        assert!(
            parse(
                "job-policy",
                &[
                    id,
                    "2",
                    "set",
                    "--max-processes",
                    "1",
                    "--action",
                    "terminate",
                ],
            )
            .is_err()
        );
        assert!(parse("job-policy", &[id, "2", "status", "--force"]).is_err());
        assert!(matches!(
            parse("job", &["set-state", id, "2", "stopped"]).unwrap(),
            Command::JobSetState {
                state: ProcessRunState::Stopped,
                timeout_ms: 5_000,
                ..
            }
        ));
        assert!(matches!(
            parse("job-signal", &[id, "2", "CONT"]).unwrap(),
            Command::JobSignal {
                signal: ProcessSignalKind::Continue,
                force: false,
                ..
            }
        ));
        assert!(parse("job-signal", &[id, "2", "KILL"]).is_err());
        assert!(matches!(
            parse("job-signal", &[id, "2", "KILL", "--force"]).unwrap(),
            Command::JobSignal {
                signal: ProcessSignalKind::Kill,
                force: true,
                ..
            }
        ));
        assert!(matches!(
            parse("job-signal", &[id, "2", "USR1"]).unwrap(),
            Command::JobSignal {
                signal: ProcessSignalKind::User1,
                force: false,
                ..
            }
        ));
        assert!(parse("job-signal", &[id, "2", "CONT", "--force"]).is_err());
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
