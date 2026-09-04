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
    let valid = tab
        .strip_prefix('@')
        .is_some_and(|value| !value.is_empty() && value.parse::<u64>().is_ok());
    valid
        .then_some(tab)
        .ok_or_else(|| "terminal --tab must be a stable @N id".to_owned())
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
