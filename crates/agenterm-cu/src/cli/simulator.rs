use agenterm_cu::{
    Command, TargetRef,
    command::{
        SIMULATOR_RESULTS_MAX, SIMULATOR_TIMEOUT_MS_MAX, validate_simulator_bundle_id,
        validate_simulator_udid,
    },
};

use super::{flag_parsed, flag_text, verbs::VerbSpec};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    let action = spec
        .name
        .strip_prefix("simulator-")
        .ok_or_else(|| format!("unknown command '{}'", spec.name))?;
    if spelled == "simulator" {
        if args.first().map(String::as_str) != Some(action) {
            return Err("simulator requires devices | boot | apps | launch | terminate".to_owned());
        }
        args.remove(0);
    }
    match action {
        "devices" => devices(target, args),
        "boot" => boot(target, args),
        "apps" => apps(target, args),
        "launch" => lifecycle(target, args, true),
        "terminate" => lifecycle(target, args, false),
        _ => Err(format!("unknown simulator action {action:?}")),
    }
}

fn devices(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let max = bounded_max(args)?;
    no_extra("simulator devices", args)?;
    Ok(Command::SimulatorDevices { target, max })
}

fn boot(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let timeout_ms = bounded_timeout(args)?;
    require_expect(args, "booted", "simulator boot")?;
    let udid = one_positional("simulator boot", args)?;
    validate_simulator_udid(&udid).map_err(str::to_owned)?;
    Ok(Command::SimulatorBoot {
        target,
        udid,
        timeout_ms,
        expect_booted: true,
    })
}

fn apps(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let max = bounded_max(args)?;
    let udid = one_positional("simulator apps", args)?;
    validate_simulator_udid(&udid).map_err(str::to_owned)?;
    Ok(Command::SimulatorApps { target, udid, max })
}

fn lifecycle(target: TargetRef, args: &mut Vec<String>, launch: bool) -> Result<Command, String> {
    let verb = if launch {
        "simulator launch"
    } else {
        "simulator terminate"
    };
    let timeout_ms = bounded_timeout(args)?;
    require_expect(args, "accepted", verb)?;
    if args.len() != 2 || args.iter().any(|arg| arg.starts_with('-')) {
        return Err(format!(
            "{verb} requires exact UDID and BUNDLE_ID positionals"
        ));
    }
    let udid = args.remove(0);
    let bundle_id = args.remove(0);
    validate_simulator_udid(&udid).map_err(str::to_owned)?;
    validate_simulator_bundle_id(&bundle_id).map_err(str::to_owned)?;
    if launch {
        Ok(Command::SimulatorLaunch {
            target,
            udid,
            bundle_id,
            timeout_ms,
            expect_accepted: true,
        })
    } else {
        Ok(Command::SimulatorTerminate {
            target,
            udid,
            bundle_id,
            timeout_ms,
            expect_accepted: true,
        })
    }
}

fn bounded_max(args: &mut Vec<String>) -> Result<usize, String> {
    let max = flag_parsed::<usize>(args, "--max")?.unwrap_or(SIMULATOR_RESULTS_MAX);
    if !(1..=SIMULATOR_RESULTS_MAX).contains(&max) {
        return Err("simulator --max must be in 1..=200".to_owned());
    }
    Ok(max)
}

fn bounded_timeout(args: &mut Vec<String>) -> Result<u64, String> {
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(DEFAULT_TIMEOUT_MS);
    if !(1..=SIMULATOR_TIMEOUT_MS_MAX).contains(&timeout_ms) {
        return Err("simulator --timeout-ms must be in 1..=600000".to_owned());
    }
    Ok(timeout_ms)
}

fn require_expect(args: &mut Vec<String>, expected: &str, verb: &str) -> Result<(), String> {
    match flag_text(args, "--expect")?.as_deref() {
        Some(value) if value == expected => Ok(()),
        Some(other) => Err(format!(
            "{verb} --expect must be the literal {expected:?}, got {other:?}"
        )),
        None => Err(format!("{verb} requires --expect {expected}")),
    }
}

fn one_positional(verb: &str, args: &mut Vec<String>) -> Result<String, String> {
    if args.len() != 1 || args[0].starts_with('-') {
        return Err(format!("{verb} requires exactly one exact UDID positional"));
    }
    Ok(args.remove(0))
}

fn no_extra(verb: &str, args: &[String]) -> Result<(), String> {
    if let Some(unexpected) = args.first() {
        Err(format!("{verb} received unexpected {unexpected:?}"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UDID: &str = "12345678-1234-1234-1234-123456789ABC";

    fn parse_words(name: &str, spelled: &str, words: &[&str]) -> Result<Command, String> {
        let spec = super::super::verbs::lookup(name).unwrap();
        let mut args = words.iter().map(|word| (*word).to_owned()).collect();
        parse(spec, spelled, TargetRef::Current, &mut args)
    }

    #[test]
    fn grouped_and_flat_forms_materialize_closed_defaults() {
        assert!(matches!(
            parse_words("simulator-devices", "simulator-devices", &[]),
            Ok(Command::SimulatorDevices { max: 200, .. })
        ));
        assert!(matches!(
            parse_words(
                "simulator-apps",
                "simulator",
                &["apps", UDID, "--max", "25"]
            ),
            Ok(Command::SimulatorApps { max: 25, .. })
        ));
        assert!(matches!(
            parse_words(
                "simulator-launch",
                "simulator",
                &["launch", UDID, "com.example.app", "--expect", "accepted"]
            ),
            Ok(Command::SimulatorLaunch {
                timeout_ms: DEFAULT_TIMEOUT_MS,
                expect_accepted: true,
                ..
            })
        ));
    }

    #[test]
    fn mutation_shapes_require_exact_expectation_identity_and_bounds() {
        for (name, words) in [
            ("simulator-boot", vec![UDID]),
            ("simulator-boot", vec![UDID, "--expect", "accepted"]),
            (
                "simulator-launch",
                vec![UDID, "com.example.app", "--expect", "running"],
            ),
            (
                "simulator-terminate",
                vec![UDID, "not dotted", "--expect", "accepted"],
            ),
            (
                "simulator-launch",
                vec![
                    UDID,
                    "com.example.app",
                    "--expect",
                    "accepted",
                    "--timeout-ms",
                    "600001",
                ],
            ),
        ] {
            assert!(
                parse_words(name, name, &words).is_err(),
                "accepted {name} {words:?}"
            );
        }
    }
}
