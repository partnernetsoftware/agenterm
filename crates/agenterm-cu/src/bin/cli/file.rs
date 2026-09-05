//! Bounded filesystem observations.

use agenterm_cu::{Command, FileTransactionAction, TargetRef};

use super::{take_switch, verbs::VerbSpec};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match spec.name {
        "file-inspect" => {
            consume_group_subcommand(spelled, args, "inspect")?;
            if args.len() != 1 || args[0].is_empty() {
                return Err("file-inspect requires exactly one non-empty PATH".into());
            }
            Ok(Command::FileInspect {
                target,
                path: args.remove(0),
            })
        }
        "file-copy" => {
            consume_group_subcommand(spelled, args, "copy")?;
            let replace = take_switch(args, "--replace");
            let apply = take_switch(args, "--apply");
            if args.len() != 2 || args.iter().any(String::is_empty) {
                return Err("file-copy requires SOURCE DESTINATION [--replace] [--apply]".into());
            }
            Ok(Command::FileCopy {
                target,
                source: args.remove(0),
                destination: args.remove(0),
                replace,
                apply,
            })
        }
        "file-move" => {
            consume_group_subcommand(spelled, args, "move")?;
            let replace = take_switch(args, "--replace");
            let apply = take_switch(args, "--apply");
            if args.len() != 2 || args.iter().any(String::is_empty) {
                return Err("file-move requires SOURCE DESTINATION [--replace] [--apply]".into());
            }
            Ok(Command::FileMove {
                target,
                source: args.remove(0),
                destination: args.remove(0),
                replace,
                apply,
            })
        }
        "file-transaction" => {
            let action = parse_action(args.first().map(String::as_str))?;
            if args.is_empty() {
                return Err("file-transaction requires ACTION TRANSACTION_ID".into());
            }
            args.remove(0);
            if args.len() != 1 || args[0].is_empty() {
                return Err("file-transaction requires ACTION TRANSACTION_ID".into());
            }
            Ok(Command::FileTransaction {
                target,
                action,
                transaction_id: args.remove(0),
            })
        }
        _ => Err(format!("unknown command '{}'", spec.name)),
    }
}

fn consume_group_subcommand(
    spelled: &str,
    args: &mut Vec<String>,
    expected: &str,
) -> Result<(), String> {
    if spelled != "file" {
        return Ok(());
    }
    if args.first().map(String::as_str) != Some(expected) {
        return Err(format!("file requires subcommand {expected}"));
    }
    args.remove(0);
    Ok(())
}

fn parse_action(value: Option<&str>) -> Result<FileTransactionAction, String> {
    match value {
        Some("status") => Ok(FileTransactionAction::Status),
        Some("rollback") => Ok(FileTransactionAction::Rollback),
        Some("recover") => Ok(FileTransactionAction::Recover),
        Some("finalize") => Ok(FileTransactionAction::Finalize),
        _ => Err("file-transaction ACTION must be status, rollback, recover, or finalize".into()),
    }
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

        let spec = crate::cli::verbs::lookup("file-copy").unwrap();
        let mut copy = vec!["source".into(), "destination".into(), "--apply".into()];
        assert!(matches!(
            parse(spec, "file-copy", TargetRef::Current, &mut copy).unwrap(),
            Command::FileCopy {
                apply: true,
                replace: false,
                ..
            }
        ));

        let spec = crate::cli::verbs::resolve("file", Some("move")).unwrap();
        let mut moved = vec![
            "move".into(),
            "source".into(),
            "destination".into(),
            "--replace".into(),
        ];
        assert!(matches!(
            parse(spec, "file", TargetRef::Current, &mut moved).unwrap(),
            Command::FileMove {
                apply: false,
                replace: true,
                ..
            }
        ));
        let spec = crate::cli::verbs::lookup("file-move").unwrap();
        let mut short = vec!["only-source".into()];
        assert!(parse(spec, "file-move", TargetRef::Current, &mut short).is_err());

        let spec = crate::cli::verbs::resolve("file", Some("rollback")).unwrap();
        let mut rollback = vec!["rollback".into(), "fixture-id".into()];
        assert!(matches!(
            parse(spec, "file", TargetRef::Current, &mut rollback).unwrap(),
            Command::FileTransaction {
                action: FileTransactionAction::Rollback,
                transaction_id,
                ..
            } if transaction_id == "fixture-id"
        ));
    }
}
