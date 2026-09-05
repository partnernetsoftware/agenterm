//! Read-only preparation of canonical privileged-operation plans.

use agenterm_cu::{Command, TargetRef};

use super::{flag_parsed, verbs::VerbSpec};

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
    if args.first().map(String::as_str) != Some("process.set-priority") {
        return Err(
            "privilege-plan requires operation process.set-priority PID NICE [--ttl-seconds N]"
                .into(),
        );
    }
    args.remove(0);
    let ttl_seconds = flag_parsed::<u64>(args, "--ttl-seconds")?.unwrap_or(120);
    if !(1..=600).contains(&ttl_seconds) {
        return Err("privilege-plan --ttl-seconds must be in 1..=600".into());
    }
    if args.len() != 2 {
        return Err("privilege-plan process.set-priority requires PID NICE".into());
    }
    let pid = if args[0] == "self" {
        std::process::id()
    } else {
        args[0]
            .parse::<u32>()
            .map_err(|_| "privilege-plan PID must be a positive integer or self".to_owned())?
    };
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
    }
}
