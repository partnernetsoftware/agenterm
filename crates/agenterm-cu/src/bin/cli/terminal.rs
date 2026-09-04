//! AgenTerm-owned terminal/session commands.

use agenterm_cu::{Command, TargetRef, TerminalWaitCondition};

use super::{flag_parsed, flag_text, take_switch, verbs::VerbSpec};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spelled == "terminal" || spelled == "pty" {
        let group = spelled;
        let expected = spec
            .aliases
            .iter()
            .find_map(|alias| alias.strip_prefix(&format!("{group} ")))
            .ok_or_else(|| format!("{group} requires a subcommand"))?;
        if args.first().map(String::as_str) != Some(expected) {
            return Err(format!("{group} requires subcommand {expected}"));
        }
        args.remove(0);
    }
    match spec.name {
        "pty-start" => {
            let command = command_tail(args);
            let name = required_name(args, "pty-start")?;
            let cwd = flag_text(args, "--cwd")?;
            if command.is_empty() {
                return Err("pty-start requires PROGRAM ARG... after --".into());
            }
            if command.len() > 256 || command.iter().map(String::len).sum::<usize>() > 1_048_576 {
                return Err("pty-start command exceeds 256 arguments or 1048576 bytes".into());
            }
            empty(args, "pty-start")?;
            Ok(Command::PtyStart {
                target,
                name,
                cwd,
                command,
            })
        }
        "pty-list" => {
            empty(args, "pty-list")?;
            Ok(Command::PtyList { target })
        }
        "pty-prune" => {
            let name = required_name(args, "pty-prune")?;
            let expect = flag_text(args, "--expect")?
                .ok_or_else(|| "pty-prune requires --expect stale".to_owned())?;
            if expect != "stale" {
                return Err("pty-prune --expect must be stale".into());
            }
            empty(args, "pty-prune")?;
            Ok(Command::PtyPrune {
                target,
                name,
                expect_stale: true,
            })
        }
        "pty-status" => {
            let name = required_name(args, "pty-status")?;
            empty(args, "pty-status")?;
            Ok(Command::PtyStatus { target, name })
        }
        "pty-read" => {
            let name = required_name(args, "pty-read")?;
            let cursor = flag_text(args, "--cursor")?.unwrap_or_else(|| "earliest".to_owned());
            if cursor != "earliest" && cursor != "current" && cursor.parse::<u64>().is_err() {
                return Err(
                    "pty-read --cursor must be earliest, current, or a non-negative integer".into(),
                );
            }
            let max_bytes = flag_parsed::<usize>(args, "--max-bytes")?.unwrap_or(65_536);
            if !(1..=1_048_576).contains(&max_bytes) {
                return Err("pty-read --max-bytes must be in 1..=1048576".into());
            }
            empty(args, "pty-read")?;
            Ok(Command::PtyRead {
                target,
                name,
                cursor,
                max_bytes,
            })
        }
        "pty-snapshot" => {
            let name = required_name(args, "pty-snapshot")?;
            empty(args, "pty-snapshot")?;
            Ok(Command::PtySnapshot { target, name })
        }
        "pty-events" => {
            let name = required_name(args, "pty-events")?;
            let epoch = flag_text(args, "--epoch")?
                .ok_or_else(|| "pty-events requires --epoch EPOCH".to_owned())?;
            if epoch.is_empty() || epoch.len() > 128 || epoch.chars().any(char::is_control) {
                return Err("pty-events --epoch must be 1..=128 non-control bytes".into());
            }
            let after = flag_parsed::<u64>(args, "--after")?
                .ok_or_else(|| "pty-events requires --after SEQUENCE".to_owned())?;
            let limit = flag_parsed::<usize>(args, "--limit")?.unwrap_or(64);
            if !(1..=64).contains(&limit) {
                return Err("pty-events --limit must be in 1..=64".into());
            }
            empty(args, "pty-events")?;
            Ok(Command::PtyEvents {
                target,
                name,
                epoch,
                after,
                limit,
            })
        }
        "pty-resize" => {
            let name = required_name(args, "pty-resize")?;
            let rows = flag_parsed::<u16>(args, "--rows")?
                .ok_or_else(|| "pty-resize requires --rows ROWS".to_owned())?;
            let columns = flag_parsed::<u16>(args, "--columns")?
                .ok_or_else(|| "pty-resize requires --columns COLUMNS".to_owned())?;
            if rows == 0 || rows > 512 || columns == 0 || columns > 512 {
                return Err("pty-resize rows and columns must be in 1..=512".into());
            }
            empty(args, "pty-resize")?;
            Ok(Command::PtyResize {
                target,
                name,
                rows,
                columns,
            })
        }
        "pty-send" => {
            let name = required_name(args, "pty-send")?;
            if args.first().map(String::as_str) == Some("--") {
                args.remove(0);
            }
            if args.len() != 1 || args[0].is_empty() {
                return Err(
                    "pty-send requires exactly one non-empty text argument after --; quote text containing spaces"
                        .into(),
                );
            }
            if args[0].len() > 1_048_576 {
                return Err("pty-send text exceeds 1048576 bytes".into());
            }
            let text = args.remove(0);
            Ok(Command::PtySend { target, name, text })
        }
        "pty-wait" => {
            let name = required_name(args, "pty-wait")?;
            let contains = flag_text(args, "--contains")?
                .ok_or_else(|| "pty-wait requires --contains TEXT".to_owned())?;
            if contains.is_empty() || contains.len() > 65_536 {
                return Err("pty-wait --contains must be 1..=65536 bytes".into());
            }
            let cursor = flag_text(args, "--cursor")?.unwrap_or_else(|| "earliest".to_owned());
            if cursor != "earliest" && cursor != "current" && cursor.parse::<u64>().is_err() {
                return Err(
                    "pty-wait --cursor must be earliest, current, or a non-negative integer".into(),
                );
            }
            let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(30_000);
            if !(1..=86_400_000).contains(&timeout_ms) {
                return Err("pty-wait --timeout-ms must be in 1..=86400000".into());
            }
            empty(args, "pty-wait")?;
            Ok(Command::PtyWait {
                target,
                name,
                contains,
                cursor,
                timeout_ms,
            })
        }
        "pty-wait-exit" => {
            let name = required_name(args, "pty-wait-exit")?;
            let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(300_000);
            if !(1..=86_400_000).contains(&timeout_ms) {
                return Err("pty-wait-exit --timeout-ms must be in 1..=86400000".into());
            }
            let expect_status = flag_parsed::<i32>(args, "--expect-status")?;
            if expect_status.is_some_and(|status| !(0..=255).contains(&status)) {
                return Err("pty-wait-exit --expect-status must be in 0..=255".into());
            }
            empty(args, "pty-wait-exit")?;
            Ok(Command::PtyWaitExit {
                target,
                name,
                timeout_ms,
                expect_status,
            })
        }
        "pty-stop" => {
            let name = required_name(args, "pty-stop")?;
            let expect = flag_text(args, "--expect")?
                .ok_or_else(|| "pty-stop requires --expect stopped".to_owned())?;
            if expect != "stopped" {
                return Err("pty-stop --expect must be stopped".into());
            }
            empty(args, "pty-stop")?;
            Ok(Command::PtyStop {
                target,
                name,
                expect_stopped: true,
            })
        }
        "terminal-list" => {
            empty(args, "terminal-list")?;
            Ok(Command::TerminalList { target })
        }
        "terminal-new" => {
            let command = if let Some(separator) = args.iter().position(|arg| arg == "--") {
                let command = args.drain(separator + 1..).collect::<Vec<_>>();
                args.pop();
                command
            } else {
                Vec::new()
            };
            let title = flag_text(args, "--title")?;
            if title.as_ref().is_some_and(|value| value.len() > 4_096) {
                return Err("terminal-new --title exceeds 4096 bytes".into());
            }
            let parent = flag_text(args, "--parent")?;
            if let Some(parent) = parent.as_deref() {
                validate_tab(parent, "terminal-new --parent")?;
            }
            let detached = take_switch(args, "--detached");
            if command.len() > 256 || command.iter().map(String::len).sum::<usize>() > 1_048_576 {
                return Err("terminal-new command exceeds 256 arguments or 1048576 bytes".into());
            }
            empty(args, "terminal-new")?;
            Ok(Command::TerminalNew {
                target,
                title,
                parent,
                detached,
                command,
            })
        }
        "terminal-close" => {
            let tab = required_tab(args)?;
            let expect = flag_text(args, "--expect")?
                .ok_or_else(|| "terminal-close requires --expect closed".to_owned())?;
            if expect != "closed" {
                return Err("terminal-close --expect must be closed".into());
            }
            empty(args, "terminal-close")?;
            Ok(Command::TerminalClose {
                target,
                tab,
                expect_closed: true,
            })
        }
        "terminal-read" => {
            let tab = required_tab(args)?;
            let max_bytes = flag_parsed::<usize>(args, "--max-bytes")?.unwrap_or(262_144);
            if !(1..=1_048_576).contains(&max_bytes) {
                return Err("terminal-read --max-bytes must be in 1..=1048576".into());
            }
            empty(args, "terminal-read")?;
            Ok(Command::TerminalRead {
                target,
                tab,
                max_bytes,
            })
        }
        "terminal-snapshot" => {
            let tab = required_tab(args)?;
            empty(args, "terminal-snapshot")?;
            Ok(Command::TerminalSnapshot { target, tab })
        }
        "terminal-events" => {
            let tab = required_tab(args)?;
            let epoch = flag_text(args, "--epoch")?
                .ok_or_else(|| "terminal-events requires --epoch EPOCH".to_owned())?;
            if epoch.is_empty() || epoch.len() > 128 || epoch.chars().any(char::is_control) {
                return Err("terminal-events --epoch must be 1..=128 non-control bytes".into());
            }
            let after = flag_parsed::<u64>(args, "--after")?
                .ok_or_else(|| "terminal-events requires --after SEQUENCE".to_owned())?;
            let limit = flag_parsed::<usize>(args, "--limit")?.unwrap_or(64);
            if !(1..=64).contains(&limit) {
                return Err("terminal-events --limit must be in 1..=64".into());
            }
            empty(args, "terminal-events")?;
            Ok(Command::TerminalEvents {
                target,
                tab,
                epoch,
                after,
                limit,
            })
        }
        "terminal-output" => {
            let tab = required_tab(args)?;
            let cursor = flag_text(args, "--cursor")?.unwrap_or_else(|| "earliest".to_owned());
            if cursor != "earliest" && cursor != "current" && cursor.parse::<u64>().is_err() {
                return Err(
                    "terminal-output --cursor must be earliest, current, or a non-negative integer"
                        .into(),
                );
            }
            let max_bytes = flag_parsed::<usize>(args, "--max-bytes")?.unwrap_or(65_536);
            if !(1..=1_048_576).contains(&max_bytes) {
                return Err("terminal-output --max-bytes must be in 1..=1048576".into());
            }
            empty(args, "terminal-output")?;
            Ok(Command::TerminalOutput {
                target,
                tab,
                cursor,
                max_bytes,
            })
        }
        "terminal-send" => {
            let tab = required_tab(args)?;
            if args.first().map(String::as_str) == Some("--") {
                args.remove(0);
            }
            if args.is_empty() {
                return Err("terminal-send requires text after --".into());
            }
            if args.len() != 1 {
                return Err(
                    "terminal-send accepts exactly one text argument; quote text containing spaces"
                        .into(),
                );
            }
            let text = args.remove(0);
            Ok(Command::TerminalSend { target, tab, text })
        }
        "terminal-wait" => {
            let tab = required_tab(args)?;
            let contains = flag_text(args, "--contains")?;
            let exited = take_switch(args, "--exited");
            let finalized = take_switch(args, "--finalized");
            let selected =
                usize::from(contains.is_some()) + usize::from(exited) + usize::from(finalized);
            if selected != 1 {
                return Err(
                    "terminal-wait requires exactly one of --contains, --exited or --finalized"
                        .into(),
                );
            }
            let condition = if let Some(text) = contains {
                TerminalWaitCondition::Contains(text)
            } else if exited {
                TerminalWaitCondition::Exited
            } else {
                TerminalWaitCondition::Finalized
            };
            let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(5_000);
            if !(1..=86_400_000).contains(&timeout_ms) {
                return Err("terminal-wait --timeout-ms must be in 1..=86400000".into());
            }
            empty(args, "terminal-wait")?;
            Ok(Command::TerminalWait {
                target,
                tab,
                condition,
                timeout_ms,
            })
        }
        other => Err(format!("unknown command '{other}'")),
    }
}

fn command_tail(args: &mut Vec<String>) -> Vec<String> {
    let Some(separator) = args.iter().position(|arg| arg == "--") else {
        return Vec::new();
    };
    let command = args.drain(separator + 1..).collect::<Vec<_>>();
    args.pop();
    command
}

fn required_name(args: &mut Vec<String>, verb: &str) -> Result<String, String> {
    let Some(first) = args.first() else {
        return Err(format!("{verb} requires NAME"));
    };
    if first.starts_with('-') {
        return Err(format!("{verb} requires NAME before options"));
    }
    let name = args.remove(0);
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    valid
        .then_some(name)
        .ok_or_else(|| format!("{verb} NAME must be 1..=64 ASCII letters, digits, '.', '_' or '-'"))
}

fn required_tab(args: &mut Vec<String>) -> Result<String, String> {
    let tab =
        flag_text(args, "--tab")?.ok_or_else(|| "terminal command requires --tab @N".to_owned())?;
    validate_tab(&tab, "terminal --tab")?;
    Ok(tab)
}

fn validate_tab(tab: &str, label: &str) -> Result<(), String> {
    let valid = tab
        .strip_prefix('@')
        .is_some_and(|value| !value.is_empty() && value.parse::<u64>().is_ok());
    valid
        .then_some(())
        .ok_or_else(|| format!("{label} must be a stable @N id"))
}

fn empty(args: &[String], verb: &str) -> Result<(), String> {
    if let Some(argument) = args.first() {
        Err(format!("{verb} received unexpected {argument:?}"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::verbs::resolve;

    fn parse(name: &str, args: &[&str]) -> Result<Command, String> {
        let spec = resolve(name, None).expect("verb");
        super::parse(
            spec,
            name,
            TargetRef::Current,
            &mut args.iter().map(|value| (*value).to_owned()).collect(),
        )
    }

    #[test]
    fn terminal_shapes_are_closed_and_bounded() {
        assert!(matches!(
            parse("pty-start", &["build", "--cwd", ".", "--", "sh", "-lc", "printf ok"]).unwrap(),
            Command::PtyStart { name, cwd: Some(cwd), command, .. }
                if name == "build" && cwd == "." && command == ["sh", "-lc", "printf ok"]
        ));
        assert!(matches!(
            parse("pty-list", &[]).unwrap(),
            Command::PtyList { .. }
        ));
        assert!(matches!(
            parse("pty-prune", &["build", "--expect", "stale"]).unwrap(),
            Command::PtyPrune { name, expect_stale: true, .. } if name == "build"
        ));
        assert!(matches!(
            parse("pty-status", &["build"]).unwrap(),
            Command::PtyStatus { name, .. } if name == "build"
        ));
        assert!(matches!(
            parse("pty-read", &["build", "--cursor", "12", "--max-bytes", "8"]).unwrap(),
            Command::PtyRead { name, cursor, max_bytes: 8, .. }
                if name == "build" && cursor == "12"
        ));
        assert!(matches!(
            parse("pty-snapshot", &["build"]).unwrap(),
            Command::PtySnapshot { name, .. } if name == "build"
        ));
        assert!(matches!(
            parse(
                "pty-events",
                &["build", "--epoch", "epoch-a", "--after", "12", "--limit", "8"]
            )
            .unwrap(),
            Command::PtyEvents { name, epoch, after: 12, limit: 8, .. }
                if name == "build" && epoch == "epoch-a"
        ));
        assert!(matches!(
            parse(
                "pty-resize",
                &["build", "--rows", "40", "--columns", "120"]
            )
            .unwrap(),
            Command::PtyResize { name, rows: 40, columns: 120, .. } if name == "build"
        ));
        assert!(matches!(
            parse("pty-send", &["build", "--", "hello\r"]).unwrap(),
            Command::PtySend { name, text, .. } if name == "build" && text == "hello\r"
        ));
        assert!(matches!(
            parse(
                "pty-wait",
                &["build", "--contains", "ready", "--cursor", "current", "--timeout-ms", "9"]
            )
            .unwrap(),
            Command::PtyWait { name, contains, cursor, timeout_ms: 9, .. }
                if name == "build" && contains == "ready" && cursor == "current"
        ));
        assert!(matches!(
            parse("pty-wait-exit", &["build", "--timeout-ms", "9", "--expect-status", "0"]).unwrap(),
            Command::PtyWaitExit { name, timeout_ms: 9, expect_status: Some(0), .. }
                if name == "build"
        ));
        assert!(matches!(
            parse("pty-stop", &["build", "--expect", "stopped"]).unwrap(),
            Command::PtyStop { name, expect_stopped: true, .. } if name == "build"
        ));
        assert!(parse("pty-start", &["bad/name", "--", "true"]).is_err());
        assert!(parse("pty-start", &["build"]).is_err());
        assert!(parse("pty-send", &["build", ""]).is_err());
        assert!(parse("pty-wait", &["build"]).is_err());
        assert!(parse("pty-events", &["build", "--epoch", "e"]).is_err());
        assert!(
            parse(
                "pty-events",
                &["build", "--epoch", "e", "--after", "0", "--limit", "65"]
            )
            .is_err()
        );
        assert!(parse("pty-resize", &["build", "--rows", "40"]).is_err());
        assert!(parse("pty-resize", &["build", "--rows", "0", "--columns", "80"]).is_err());
        assert!(parse("pty-prune", &["build"]).is_err());
        assert!(parse("pty-stop", &["build"]).is_err());
        assert!(matches!(
            parse("terminal-list", &[]).unwrap(),
            Command::TerminalList { .. }
        ));
        assert!(matches!(
            parse(
                "terminal-new",
                &["--title", "build", "--parent", "@7", "--detached", "--", "sh", "-lc", "printf ok"]
            )
            .unwrap(),
            Command::TerminalNew { title: Some(title), parent: Some(parent), detached: true, command, .. }
                if title == "build" && parent == "@7" && command == ["sh", "-lc", "printf ok"]
        ));
        assert!(parse("terminal-new", &["--parent", "7"]).is_err());
        assert!(matches!(
            parse("terminal-close", &["--tab", "@7", "--expect", "closed"]).unwrap(),
            Command::TerminalClose { tab, expect_closed: true, .. } if tab == "@7"
        ));
        assert!(parse("terminal-close", &["--tab", "@7"]).is_err());
        assert!(matches!(
            parse("terminal-read", &["--tab", "@7", "--max-bytes", "12"]).unwrap(),
            Command::TerminalRead { tab, max_bytes: 12, .. } if tab == "@7"
        ));
        assert!(matches!(
            parse("terminal-send", &["--tab", "@7", "--", "hello world"]).unwrap(),
            Command::TerminalSend { text, .. } if text == "hello world"
        ));
        assert!(parse("terminal-send", &["--tab", "@7", "--", "hello", "world"]).is_err());
        assert!(matches!(
            parse(
                "terminal-wait",
                &["--tab", "@7", "--finalized", "--timeout-ms", "9"]
            )
            .unwrap(),
            Command::TerminalWait {
                condition: TerminalWaitCondition::Finalized,
                timeout_ms: 9,
                ..
            }
        ));
        assert!(parse("terminal-read", &["--tab", "7"]).is_err());
        assert!(parse("terminal-read", &["--tab", "@7", "--max-bytes", "0"]).is_err());
        assert!(matches!(
            parse("terminal-snapshot", &["--tab", "@7"]).unwrap(),
            Command::TerminalSnapshot { tab, .. } if tab == "@7"
        ));
        assert!(matches!(
            parse(
                "terminal-events",
                &["--tab", "@7", "--epoch", "epoch-a", "--after", "12", "--limit", "8"]
            )
            .unwrap(),
            Command::TerminalEvents { tab, epoch, after: 12, limit: 8, .. }
                if tab == "@7" && epoch == "epoch-a"
        ));
        assert!(parse("terminal-events", &["--tab", "@7", "--epoch", "e"]).is_err());
        assert!(
            parse(
                "terminal-events",
                &[
                    "--tab", "@7", "--epoch", "e", "--after", "0", "--limit", "65"
                ]
            )
            .is_err()
        );
        assert!(matches!(
            parse(
                "terminal-output",
                &["--tab", "@7", "--cursor", "12", "--max-bytes", "8"]
            )
            .unwrap(),
            Command::TerminalOutput { tab, cursor, max_bytes: 8, .. }
                if tab == "@7" && cursor == "12"
        ));
        assert!(matches!(
            parse("terminal-output", &["--tab", "@7"]).unwrap(),
            Command::TerminalOutput { cursor, max_bytes: 65_536, .. }
                if cursor == "earliest"
        ));
        assert!(parse("terminal-output", &["--tab", "@7", "--cursor", "old"]).is_err());
        assert!(parse("terminal-output", &["--tab", "@7", "--max-bytes", "0"]).is_err());
        assert!(parse("terminal-wait", &["--tab", "@7", "--exited", "--finalized"]).is_err());
    }
}
