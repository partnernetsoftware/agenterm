//! Host-level discovery that is independent of the desktop/window family.

use agenterm_cu::{Command, TargetRef};

use super::{flag_parsed, flag_text, take_switch, verbs::VerbSpec};

const DEFAULT_SESSION_TTL_SECONDS: u64 = 3_600;
const DEFAULT_LOCK_TTL_SECONDS: u64 = 300;

pub fn parse(
    spec: &VerbSpec,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
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
    if !args.is_empty() {
        return Err(format!(
            "{} accepts no arguments; unexpected {:?}",
            spec.name, args[0]
        ));
    }
    match spec.name {
        "capabilities" => Ok(Command::Capabilities { target }),
        "permissions" => Ok(Command::Permissions { target }),
        "doctor" => Ok(Command::Doctor { target }),
        other => Err(format!("unknown command '{other}'")),
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
        let spec = verbs::lookup(name).expect("runtime verb");
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
}
