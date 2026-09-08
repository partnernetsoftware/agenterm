//! Read-only preparation of canonical privileged-operation plans.

use agenterm_cu::{
    Command, TargetRef,
    command::{PrivilegeProviderAction, ProcessSignalKind},
    privilege_apply::{DEFAULT_PROVIDER_TIMEOUT_MS, MAX_PROVIDER_TIMEOUT_MS, decode_plan_request},
    privilege_plan::{PROCESS_SIGNAL_TREE_MAX_DESCENDANTS, PowerAction},
};

use super::{flag_parsed, flag_text, take_switch, verbs::VerbSpec};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spec.name == "privilege-provider" {
        if spelled == "privilege" {
            if args.first().map(String::as_str) != Some("provider") {
                return Err("privilege requires subcommand provider".into());
            }
            args.remove(0);
        }
        return parse_provider(target, args);
    }
    if spec.name == "privilege-apply" {
        if spelled == "privilege" {
            if args.first().map(String::as_str) != Some("apply") {
                return Err("privilege requires subcommand apply".into());
            }
            args.remove(0);
        }
        return parse_apply(target, args);
    }
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
        "system.power-action" => parse_power_action(target, args, ttl_seconds),
        _ => Err(
            "privilege-plan operation must be process.set-priority, process.signal or system.power-action"
                .into(),
        ),
    }
}

fn parse_power_action(
    target: TargetRef,
    args: &[String],
    ttl_seconds: u64,
) -> Result<Command, String> {
    if args.len() != 1 {
        return Err(
            "privilege-plan system.power-action requires exactly sleep, restart or shutdown".into(),
        );
    }
    let action = PowerAction::parse(&args[0]).ok_or_else(|| {
        "privilege-plan system.power-action must be sleep, restart or shutdown".to_owned()
    })?;
    Ok(Command::PrivilegePlanPowerAction {
        target,
        action,
        ttl_seconds,
    })
}

fn parse_provider(target: TargetRef, args: &mut [String]) -> Result<Command, String> {
    if args.len() != 1 {
        return Err("privilege-provider requires exactly status, register or unregister".into());
    }
    let action = PrivilegeProviderAction::parse(&args[0]).ok_or_else(|| {
        "privilege-provider action must be status, register or unregister".to_owned()
    })?;
    Ok(Command::PrivilegeProvider { target, action })
}

fn parse_apply(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let request = flag_text(args, "--request")?
        .ok_or_else(|| "privilege apply requires --request".to_owned())?;
    let approval_digest = flag_text(args, "--approve")?
        .ok_or_else(|| "privilege apply requires --approve".to_owned())?;
    let provider_timeout_ms =
        flag_parsed::<u64>(args, "--provider-timeout-ms")?.unwrap_or(DEFAULT_PROVIDER_TIMEOUT_MS);
    if !args.is_empty() {
        return Err(format!(
            "privilege apply has unexpected arguments: {}",
            args.join(" ")
        ));
    }
    if !(1..=MAX_PROVIDER_TIMEOUT_MS).contains(&provider_timeout_ms) {
        return Err("privilege apply --provider-timeout-ms must be in 1..=600000".into());
    }
    let plan = decode_plan_request(&request).map_err(|error| error.message)?;
    if approval_digest != plan.approval_digest() {
        return Err("privilege apply --approve does not match the encoded plan".into());
    }
    Ok(Command::PrivilegeApply {
        target,
        plan,
        approval_digest,
        provider_timeout_ms,
    })
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

        let mut power = vec![
            "plan".into(),
            "system.power-action".into(),
            "shutdown".into(),
            "--ttl-seconds".into(),
            "1".into(),
        ];
        assert!(matches!(
            parse(spec, "privilege", TargetRef::Current, &mut power).unwrap(),
            Command::PrivilegePlanPowerAction {
                action: PowerAction::Shutdown,
                ttl_seconds: 1,
                ..
            }
        ));

        for args in [
            vec!["system.power-action".into(), "hibernate".into()],
            vec![
                "system.power-action".into(),
                "sleep".into(),
                "--ttl-seconds".into(),
                "0".into(),
            ],
            vec![
                "system.power-action".into(),
                "restart".into(),
                "--ttl-seconds".into(),
                "601".into(),
            ],
        ] {
            let mut args = args;
            assert!(parse(spec, "privilege-plan", TargetRef::Current, &mut args).is_err());
        }
    }

    #[test]
    fn parses_typed_apply_without_retaining_the_opaque_encoding() {
        let plan = agenterm_cu::privilege_plan::process_priority_plan(
            std::process::id(),
            0,
            60,
            1_000_000,
        )
        .unwrap();
        let plan = agenterm_cu::privilege_apply::PrivilegePlanV1::ProcessPriority(plan);
        let request = agenterm_cu::privilege_apply::encode_plan_request(&plan).unwrap();
        let approval = plan.approval_digest().to_owned();
        let spec = crate::cli::verbs::lookup("privilege-apply").unwrap();
        let mut args = vec![
            "apply".into(),
            "--request".into(),
            request,
            "--approve".into(),
            approval.clone(),
            "--provider-timeout-ms".into(),
            "3000".into(),
        ];
        let command = parse(spec, "privilege", TargetRef::Current, &mut args).unwrap();
        assert_eq!(command.required_grant(), agenterm_cu::Grant::Actuate);
        assert_eq!(
            command.authorization_operation().as_deref(),
            Some("privilege.apply.process.set-priority")
        );
        assert!(matches!(
            command,
            Command::PrivilegeApply {
                plan: ref actual,
                approval_digest: ref actual_approval,
                provider_timeout_ms: 3_000,
                ..
            } if actual == &plan && actual_approval == &approval
        ));
        assert!(args.is_empty());
    }

    #[test]
    fn parses_provider_lifecycle_without_caller_selected_identity() {
        let spec = crate::cli::verbs::lookup("privilege-provider").unwrap();
        let mut status = vec!["status".into()];
        assert!(matches!(
            parse(spec, "privilege-provider", TargetRef::Current, &mut status).unwrap(),
            Command::PrivilegeProvider {
                action: PrivilegeProviderAction::Status,
                ..
            }
        ));

        let mut grouped = vec!["provider".into(), "register".into()];
        let command = parse(spec, "privilege", TargetRef::Current, &mut grouped).unwrap();
        assert!(matches!(
            command,
            Command::PrivilegeProvider {
                action: PrivilegeProviderAction::Register,
                ..
            }
        ));
        assert_eq!(command.required_grant(), agenterm_cu::Grant::Actuate);

        for bad in [
            vec![],
            vec!["other".into()],
            vec!["status".into(), "extra".into()],
        ] {
            let mut bad = bad;
            assert!(parse(spec, "privilege-provider", TargetRef::Current, &mut bad).is_err());
        }
    }
}
