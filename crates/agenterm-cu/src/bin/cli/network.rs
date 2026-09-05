//! Bounded network observations.

use agenterm_cu::{Command, TargetRef};

use super::{flag_parsed, verbs::VerbSpec};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spelled == "network" {
        if !matches!(
            args.first().map(String::as_str),
            Some("probe" | "interfaces")
        ) {
            return Err("network requires subcommand interfaces or probe".into());
        }
        args.remove(0);
    }
    match spec.name {
        "network-interfaces" => network_interfaces(target, args),
        "network-probe" => network_probe(target, args),
        other => Err(format!("unknown command '{other}'")),
    }
}

fn network_interfaces(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let max = flag_parsed::<usize>(args, "--max")?.unwrap_or(1000);
    if !(1..=5000).contains(&max) {
        return Err("network-interfaces --max must be in 1..=5000".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "network-interfaces received unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::NetworkInterfaces { target, max })
}

fn network_probe(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    if args.is_empty() || args[0].starts_with('-') {
        return Err("network-probe requires one HOST positional".into());
    }
    let host = args.remove(0);
    let port = flag_parsed::<u16>(args, "--port")?.unwrap_or(443);
    let attempts = flag_parsed::<u8>(args, "--attempts")?.unwrap_or(3);
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(3000);
    if host.is_empty()
        || host.len() > 253
        || host.trim() != host
        || host.chars().any(char::is_whitespace)
        || port == 0
        || !(1..=20).contains(&attempts)
        || !(100..=60_000).contains(&timeout_ms)
    {
        return Err("network-probe requires a bare 1..=253-byte HOST, port 1..=65535, attempts 1..=20 and timeout-ms 100..=60000".into());
    }
    if !args.is_empty() {
        return Err(format!("network-probe received unexpected {:?}", args[0]));
    }
    Ok(Command::NetworkProbe {
        target,
        host,
        port,
        attempts,
        timeout_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_and_alias() {
        let spec = crate::cli::verbs::lookup("network-probe").unwrap();
        let mut args = vec!["localhost".into()];
        assert!(matches!(
            parse(spec, "network-probe", TargetRef::Current, &mut args).unwrap(),
            Command::NetworkProbe {
                port: 443,
                attempts: 3,
                timeout_ms: 3000,
                ..
            }
        ));
        let mut args = vec!["probe".into(), "127.0.0.1".into()];
        assert!(parse(spec, "network", TargetRef::Current, &mut args).is_ok());
    }

    #[test]
    fn parses_network_interfaces_defaults_alias_and_closed_limit() {
        let spec = crate::cli::verbs::lookup("network-interfaces").unwrap();
        let mut args = Vec::new();
        assert!(matches!(
            parse(spec, "network-interfaces", TargetRef::Current, &mut args).unwrap(),
            Command::NetworkInterfaces { max: 1000, .. }
        ));
        let mut args = vec!["interfaces".into(), "--max".into(), "1".into()];
        assert!(matches!(
            parse(spec, "network", TargetRef::Current, &mut args).unwrap(),
            Command::NetworkInterfaces { max: 1, .. }
        ));
        for value in ["0", "5001"] {
            let mut args = vec!["--max".into(), value.into()];
            assert!(parse(spec, "network-interfaces", TargetRef::Current, &mut args).is_err());
        }
    }

    #[test]
    fn rejects_invalid_limits_before_execution() {
        let spec = crate::cli::verbs::lookup("network-probe").unwrap();
        let mut args = vec!["bad host".into()];
        assert!(parse(spec, "network-probe", TargetRef::Current, &mut args).is_err());
        let mut args = vec!["localhost".into(), "--attempts".into(), "21".into()];
        assert!(parse(spec, "network-probe", TargetRef::Current, &mut args).is_err());
    }
}
