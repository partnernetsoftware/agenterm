//! Process inventory commands backed by the shared platform facade.

use agenterm_cu::{Command, TargetRef};

use super::verbs::VerbSpec;
use super::{flag_parsed, flag_text};

pub fn parse(
    spec: &VerbSpec,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match spec.name {
        "ps" => ps(target, args),
        "process-state" => process_state(target, args),
        "process-usage" => process_usage(target, args),
        "process-wait" => process_wait(target, args),
        other => Err(format!("unknown command '{other}'")),
    }
}

fn process_wait(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = required_positive_pid_flag(args, "process-wait")?;
    let start_identity = flag_text(args, "--start-identity")?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "process-wait requires --start-identity ID from process-state".to_owned())?;
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(30_000);
    if !(1..=86_400_000).contains(&timeout_ms) {
        return Err("process-wait --timeout-ms must be in 1..=86400000".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "process-wait accepts only --pid N --start-identity ID --timeout-ms N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::ProcessWait {
        target,
        pid,
        start_identity,
        timeout_ms,
    })
}

fn required_positive_pid_flag(args: &mut Vec<String>, command: &str) -> Result<u32, String> {
    let pid =
        flag_parsed::<u32>(args, "--pid")?.ok_or_else(|| format!("{command} requires --pid N"))?;
    if pid == 0 {
        return Err(format!("{command} --pid must be greater than zero"));
    }
    Ok(pid)
}

fn process_usage(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = required_positive_pid(args, "process-usage")?;
    Ok(Command::ProcessUsage { target, pid })
}

fn required_positive_pid(args: &mut Vec<String>, command: &str) -> Result<u32, String> {
    let pid = required_positive_pid_flag(args, command)?;
    if !args.is_empty() {
        return Err(format!(
            "{command} accepts only --pid N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(pid)
}

fn process_state(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = required_positive_pid(args, "process-state")?;
    Ok(Command::ProcessState { target, pid })
}

fn ps(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = flag_parsed::<u32>(args, "--pid")?;
    let parent = flag_parsed::<u32>(args, "--parent")?;
    let name = flag_text(args, "--name")?;
    if name.as_deref().is_some_and(|value| value.trim().is_empty()) {
        return Err("ps --name must not be empty".into());
    }
    let offset = flag_parsed::<usize>(args, "--offset")?;
    let max = flag_parsed::<usize>(args, "--max")?;
    if !args.is_empty() {
        return Err(format!(
            "ps accepts only --pid N --parent N --name SUB --offset N --max N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::Ps {
        target,
        pid,
        parent,
        name,
        offset,
        max,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::verbs;

    #[test]
    fn ps_parses_the_closed_bounded_shape() {
        let spec = verbs::lookup("ps").expect("ps verb");
        let mut args = vec![
            "--pid".into(),
            "42".into(),
            "--parent".into(),
            "7".into(),
            "--name".into(),
            "worker".into(),
            "--offset".into(),
            "3".into(),
            "--max".into(),
            "9".into(),
        ];
        let command = parse(spec, TargetRef::Current, &mut args).expect("parse");
        assert!(matches!(
            command,
            Command::Ps {
                pid: Some(42),
                parent: Some(7),
                ref name,
                offset: Some(3),
                max: Some(9),
                ..
            } if name.as_deref() == Some("worker")
        ));
    }

    #[test]
    fn ps_rejects_richer_mcu_flags_instead_of_ignoring_them() {
        let spec = verbs::lookup("ps").expect("ps verb");
        let mut args = vec!["--cpu-above".into(), "5".into()];
        let error = parse(spec, TargetRef::Current, &mut args).expect_err("typed usage");
        assert!(error.contains("unexpected"), "{error}");
    }

    #[test]
    fn process_state_requires_one_positive_pid() {
        let spec = verbs::lookup("process-state").expect("process-state verb");
        let mut args = vec!["--pid".into(), "42".into()];
        assert!(matches!(
            parse(spec, TargetRef::Current, &mut args).expect("parse"),
            Command::ProcessState { pid: 42, .. }
        ));

        let mut missing = Vec::new();
        assert!(
            parse(spec, TargetRef::Current, &mut missing)
                .expect_err("missing")
                .contains("requires --pid")
        );
    }

    #[test]
    fn process_usage_parses_the_same_closed_pid_shape() {
        let spec = verbs::lookup("process-usage").expect("process-usage verb");
        let mut args = vec!["--pid".into(), "42".into()];
        assert!(matches!(
            parse(spec, TargetRef::Ssh, &mut args).expect("parse"),
            Command::ProcessUsage {
                target: TargetRef::Ssh,
                pid: 42,
            }
        ));
    }

    #[test]
    fn process_wait_requires_identity_and_a_bounded_timeout() {
        let spec = verbs::lookup("process-wait").expect("process-wait verb");
        let mut args = vec![
            "--pid".into(),
            "42".into(),
            "--start-identity".into(),
            "boot:123".into(),
            "--timeout-ms".into(),
            "250".into(),
        ];
        assert!(matches!(
            parse(spec, TargetRef::Current, &mut args).expect("parse"),
            Command::ProcessWait {
                pid: 42,
                timeout_ms: 250,
                ref start_identity,
                ..
            } if start_identity == "boot:123"
        ));

        let mut missing_identity = vec!["--pid".into(), "42".into()];
        assert!(
            parse(spec, TargetRef::Current, &mut missing_identity)
                .expect_err("identity")
                .contains("--start-identity")
        );
    }
}
