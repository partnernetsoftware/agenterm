//! Bounded filesystem observations.

use agenterm_cu::{Command, FileTransactionAction, TargetRef};

use super::{flag_parsed, take_switch, verbs::VerbSpec};

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
        "file-watch" => {
            consume_group_subcommand(spelled, args, "observe")?;
            let duration_ms = flag_parsed::<u64>(args, "--duration-ms")?.unwrap_or(30_000);
            let max_events = flag_parsed::<usize>(args, "--max-events")?;
            if args.len() != 1 || args[0].is_empty() {
                return Err(
                    "file-watch requires exactly one non-empty directory PATH [--duration-ms N] [--max-events N]"
                        .into(),
                );
            }
            if !(1..=86_400_000).contains(&duration_ms)
                || max_events.is_some_and(|value| !(1..=4_096).contains(&value))
            {
                return Err(
                    "file-watch requires duration-ms in 1..=86400000 and max-events in 1..=4096"
                        .into(),
                );
            }
            let path = args.remove(0);
            if !args.is_empty() {
                return Err(format!("file-watch received unexpected {:?}", args[0]));
            }
            Ok(Command::FileWatch {
                target,
                path,
                duration_ms,
                max_events,
            })
        }
        "file-attributes" => {
            consume_group_subcommand(spelled, args, "attributes")?;
            let include_values = take_switch(args, "--include-values");
            if args.len() != 1 || args[0].is_empty() {
                return Err("file-attributes requires PATH [--include-values]".into());
            }
            Ok(Command::FileAttributes {
                target,
                path: args.remove(0),
                include_values,
            })
        }
        "file-mode" => {
            consume_group_subcommand(spelled, args, "chmod")?;
            let apply = take_switch(args, "--apply");
            if args.len() != 2 || args.iter().any(String::is_empty) {
                return Err("file-mode requires PATH OCTAL [--apply]".into());
            }
            let path = args.remove(0);
            let mode = parse_octal_mode(&args.remove(0))?;
            Ok(Command::FileMode {
                target,
                path,
                mode,
                apply,
            })
        }
        "file-xattr-set" => {
            consume_group_subcommand(spelled, args, "xattr-set")?;
            let apply = take_switch(args, "--apply");
            let value_hex = take_required_value(args, "--value-hex")?;
            if args.len() != 2 || args.iter().any(String::is_empty) {
                return Err("file-xattr-set requires PATH NAME --value-hex HEX [--apply]".into());
            }
            validate_hex(&value_hex)?;
            Ok(Command::FileXattrSet {
                target,
                path: args.remove(0),
                name: args.remove(0),
                value_hex,
                apply,
            })
        }
        "file-xattr-remove" => {
            consume_group_subcommand(spelled, args, "xattr-remove")?;
            let apply = take_switch(args, "--apply");
            if args.len() != 2 || args.iter().any(String::is_empty) {
                return Err("file-xattr-remove requires PATH NAME [--apply]".into());
            }
            Ok(Command::FileXattrRemove {
                target,
                path: args.remove(0),
                name: args.remove(0),
                apply,
            })
        }
        "file-quarantine-clear" => {
            consume_group_subcommand(spelled, args, "quarantine-clear")?;
            let apply = take_switch(args, "--apply");
            if args.len() != 1 || args[0].is_empty() {
                return Err("file-quarantine-clear requires PATH [--apply]".into());
            }
            Ok(Command::FileQuarantineClear {
                target,
                path: args.remove(0),
                apply,
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

fn parse_octal_mode(value: &str) -> Result<u32, String> {
    if value.is_empty()
        || value.len() > 4
        || value.bytes().any(|byte| !(b'0'..=b'7').contains(&byte))
    {
        return Err("file-mode OCTAL must contain one to four octal digits".into());
    }
    u32::from_str_radix(value, 8).map_err(|_| "file-mode OCTAL is invalid".into())
}

fn take_required_value(args: &mut Vec<String>, flag: &str) -> Result<String, String> {
    let Some(index) = args.iter().position(|item| item == flag) else {
        return Err(format!("{flag} is required"));
    };
    args.remove(index);
    if index >= args.len() {
        return Err(format!("{flag} requires a value"));
    }
    Ok(args.remove(index))
}

fn validate_hex(value: &str) -> Result<(), String> {
    const MAX_HEX_BYTES: usize = 8 * 1024 * 1024;
    if value.len() > MAX_HEX_BYTES {
        return Err("file-xattr-set --value-hex exceeds the 4 MiB decoded limit".into());
    }
    if !value.len().is_multiple_of(2) || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
        return Err(
            "file-xattr-set --value-hex must contain an even number of hexadecimal digits".into(),
        );
    }
    Ok(())
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

        let spec = crate::cli::verbs::lookup("file-attributes").unwrap();
        let mut attributes = vec!["item".into(), "--include-values".into()];
        assert!(matches!(
            parse(spec, "file-attributes", TargetRef::Current, &mut attributes).unwrap(),
            Command::FileAttributes {
                path,
                include_values: true,
                ..
            } if path == "item"
        ));

        let spec = crate::cli::verbs::resolve("file", Some("chmod")).unwrap();
        let mut mode = vec![
            "chmod".into(),
            "item".into(),
            "0640".into(),
            "--apply".into(),
        ];
        assert!(matches!(
            parse(spec, "file", TargetRef::Current, &mut mode).unwrap(),
            Command::FileMode {
                path,
                mode: 0o640,
                apply: true,
                ..
            } if path == "item"
        ));
        let spec = crate::cli::verbs::lookup("file-mode").unwrap();
        let mut bad_mode = vec!["item".into(), "888".into()];
        assert!(parse(spec, "file-mode", TargetRef::Current, &mut bad_mode).is_err());

        let spec = crate::cli::verbs::resolve("file", Some("xattr-set")).unwrap();
        let mut xattr_set = vec![
            "xattr-set".into(),
            "item".into(),
            "user.example".into(),
            "--value-hex".into(),
            "00ff".into(),
            "--apply".into(),
        ];
        assert!(matches!(
            parse(spec, "file", TargetRef::Current, &mut xattr_set).unwrap(),
            Command::FileXattrSet { path, name, value_hex, apply: true, .. }
                if path == "item" && name == "user.example" && value_hex == "00ff"
        ));
        let spec = crate::cli::verbs::lookup("file-xattr-set").unwrap();
        let mut empty_xattr = vec![
            "item".into(),
            "user.empty".into(),
            "--value-hex".into(),
            String::new(),
        ];
        assert!(matches!(
            parse(
                spec,
                "file-xattr-set",
                TargetRef::Current,
                &mut empty_xattr
            )
            .unwrap(),
            Command::FileXattrSet { value_hex, apply: false, .. } if value_hex.is_empty()
        ));
        let spec = crate::cli::verbs::lookup("file-xattr-remove").unwrap();
        let mut xattr_remove = vec!["item".into(), "user.example".into()];
        assert!(matches!(
            parse(
                spec,
                "file-xattr-remove",
                TargetRef::Current,
                &mut xattr_remove
            )
            .unwrap(),
            Command::FileXattrRemove { apply: false, .. }
        ));
        let spec = crate::cli::verbs::resolve("file", Some("quarantine-clear")).unwrap();
        let mut quarantine = vec!["quarantine-clear".into(), "item".into(), "--apply".into()];
        assert!(matches!(
            parse(spec, "file", TargetRef::Current, &mut quarantine).unwrap(),
            Command::FileQuarantineClear { apply: true, .. }
        ));
        let spec = crate::cli::verbs::lookup("file-xattr-set").unwrap();
        let mut odd_hex = vec![
            "item".into(),
            "user.example".into(),
            "--value-hex".into(),
            "0".into(),
        ];
        assert!(parse(spec, "file-xattr-set", TargetRef::Current, &mut odd_hex).is_err());

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
