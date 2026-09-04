//! AgenTerm-owned terminal/session commands.

use agenterm_cu::{Command, TargetRef, TerminalWaitCondition};

use super::{flag_parsed, flag_text, take_switch, verbs::VerbSpec};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spelled == "terminal" {
        let expected = spec
            .aliases
            .iter()
            .find_map(|alias| alias.strip_prefix("terminal "))
            .ok_or_else(|| "terminal requires a subcommand".to_owned())?;
        if args.first().map(String::as_str) != Some(expected) {
            return Err(format!("terminal requires subcommand {expected}"));
        }
        args.remove(0);
    }
    match spec.name {
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
        assert!(parse("terminal-wait", &["--tab", "@7", "--exited", "--finalized"]).is_err());
    }
}
