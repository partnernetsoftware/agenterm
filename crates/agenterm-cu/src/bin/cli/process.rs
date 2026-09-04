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
        other => Err(format!("unknown command '{other}'")),
    }
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
}
