//! Read-only preparation of canonical privileged-operation plans.

use agenterm_cu::{
    Command, TargetRef, command::ProcessSignalKind,
    privilege_plan::PROCESS_SIGNAL_TREE_MAX_DESCENDANTS,
};

use super::{flag_parsed, take_switch, verbs::VerbSpec};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spec.name != "privilege-plan" {
        return Err(format!("unknown command '{}'", spec.name));
    }
    if spelled == "privilege" {
        if args.first().map(String::as_str) != Some("plan") {
            return Err("privilege requires subcommand plan".into());
        }
        args.remove(0);
    }
    let operation = args
        .first()
        .cloned()
        .ok_or_else(|| "privilege-plan requires a closed operation".to_owned())?;
    args.remove(0);
    let ttl_seconds = flag_parsed::<u64>(args, "--ttl-seconds")?.unwrap_or(120);
    if !(1..=600).contains(&ttl_seconds) {
        return Err("privilege-plan --ttl-seconds must be in 1..=600".into());
    }
    match operation.as_str() {
        "process.set-priority" => parse_priority(target, args, ttl_seconds),
        "process.signal" => parse_signal(target, args, ttl_seconds),
        _ => Err("privilege-plan operation must be process.set-priority or process.signal".into()),
    }
}

fn parse_pid(raw: &str) -> Result<u32, String> {
    let pid = if raw == "self" {
        std::process::id()
    } else {
        raw.parse::<u32>()
            .map_err(|_| "privilege-plan PID must be a positive integer or self".to_owned())?
    };
    Ok(pid)
}

fn parse_priority(target: TargetRef, args: &[String], ttl_seconds: u64) -> Result<Command, String> {
    if args.len() != 2 {
        return Err("privilege-plan process.set-priority requires PID NICE".into());
    }
    let pid = parse_pid(&args[0])?;
    let nice = args[1]
        .parse::<i32>()
        .map_err(|_| "privilege-plan NICE must be an integer in -20..=20".to_owned())?;
    if pid == 0 {
        return Err("privilege-plan PID must be greater than zero".into());
    }
    if !(-20..=20).contains(&nice) {
        return Err("privilege-plan NICE must be in -20..=20".into());
    }
    Ok(Command::PrivilegePlanProcessPriority {
        target,
        pid,
        nice,
        ttl_seconds,
    })
}

fn parse_signal(
    target: TargetRef,
    args: &mut Vec<String>,
    ttl_seconds: u64,
) -> Result<Command, String> {
    let force = take_switch(args, "--force");
    let tree = take_switch(args, "--tree");
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(5_000);
    let explicit_max = flag_parsed::<u32>(args, "--max")?;
    if explicit_max.is_some() && !tree {
        return Err("privilege-plan process.signal --max requires --tree".into());
    }
    let max_descendants = explicit_max.unwrap_or(PROCESS_SIGNAL_TREE_MAX_DESCENDANTS);
    if args.len() != 2 {
        return Err("privilege-plan process.signal requires PID SIGNAL".into());
    }
    let pid = parse_pid(&args[0])?;
    if pid <= 1 {
        return Err("privilege-plan process.signal PID must be greater than one".into());
    }
    let signal = ProcessSignalKind::parse(&args[1]).ok_or_else(|| {
        "privilege-plan process.signal SIGNAL must be HUP, INT, TERM, KILL, STOP, CONT, USR1 or USR2"
            .to_owned()
    })?;
    if (signal == ProcessSignalKind::Kill) != force {
        return Err("privilege-plan process.signal requires --force exactly for KILL".into());
    }
    if !(1..=60_000).contains(&timeout_ms) {
        return Err("privilege-plan process.signal --timeout-ms must be in 1..=60000".into());
    }
    if !(1..=PROCESS_SIGNAL_TREE_MAX_DESCENDANTS).contains(&max_descendants) {
        return Err(format!(
            "privilege-plan process.signal --max must be in 1..={PROCESS_SIGNAL_TREE_MAX_DESCENDANTS}"
        ));
    }
    Ok(Command::PrivilegePlanProcessSignal {
        target,
        pid,
        signal,
        force,
        tree,
        timeout_ms,
        max_descendants,
        ttl_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_and_mcu_spellings_as_observation_only_plan() {
        let spec = crate::cli::verbs::lookup("privilege-plan").unwrap();
        let mut native = vec![
            "process.set-priority".into(),
            "self".into(),
            "10".into(),
            "--ttl-seconds".into(),
            "60".into(),
        ];
        assert!(matches!(
            parse(spec, "privilege-plan", TargetRef::Current, &mut native).unwrap(),
            Command::PrivilegePlanProcessPriority {
                nice: 10,
                ttl_seconds: 60,
                ..
            }
        ));

        let mut grouped = vec![
            "plan".into(),
            "process.set-priority".into(),
            "42".into(),
            "0".into(),
        ];
        assert!(matches!(
            parse(spec, "privilege", TargetRef::Current, &mut grouped).unwrap(),
            Command::PrivilegePlanProcessPriority {
                pid: 42,
                nice: 0,
                ..
            }
        ));

        let mut signal = vec![
            "process.signal".into(),
            "42".into(),
            "STOP".into(),
            "--tree".into(),
            "--max".into(),
            "128".into(),
            "--timeout-ms".into(),
            "3000".into(),
            "--ttl-seconds".into(),
            "30".into(),
        ];
        assert!(matches!(
            parse(spec, "privilege-plan", TargetRef::Current, &mut signal).unwrap(),
            Command::PrivilegePlanProcessSignal {
                pid: 42,
                signal: ProcessSignalKind::Stop,
                tree: true,
                max_descendants: 128,
                timeout_ms: 3_000,
                ttl_seconds: 30,
                ..
            }
        ));

        let mut kill_without_force = vec!["process.signal".into(), "42".into(), "KILL".into()];
        assert!(
            parse(
                spec,
                "privilege-plan",
                TargetRef::Current,
                &mut kill_without_force
            )
            .unwrap_err()
            .contains("--force")
        );
    }
}
