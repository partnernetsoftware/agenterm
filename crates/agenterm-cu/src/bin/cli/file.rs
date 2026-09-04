//! Bounded filesystem observations.

use agenterm_cu::{Command, TargetRef};

use super::verbs::VerbSpec;

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spelled == "file" {
        if args.first().map(String::as_str) != Some("inspect") {
            return Err("file requires subcommand inspect".into());
        }
        args.remove(0);
    }
    if spec.name != "file-inspect" {
        return Err(format!("unknown command '{}'", spec.name));
    }
    if args.len() != 1 || args[0].is_empty() {
        return Err("file-inspect requires exactly one non-empty PATH".into());
    }
    Ok(Command::FileInspect {
        target,
        path: args.remove(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_and_mcu_spellings_without_a_shell() {
        let spec = crate::cli::verbs::lookup("file-inspect").unwrap();
        let mut native = vec!["a path".into()];
        assert!(matches!(
            parse(spec, "file-inspect", TargetRef::Current, &mut native).unwrap(),
            Command::FileInspect { path, .. } if path == "a path"
        ));
        let mut mcu = vec!["inspect".into(), "item".into()];
        assert!(parse(spec, "file", TargetRef::Current, &mut mcu).is_ok());
    }
}
