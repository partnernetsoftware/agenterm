//! Clipboard: the native read / write / clear verbs, the MCU `clipboard`
//! group word and `clip`, plus the a11y-addressed `copy` / `paste`.

use agenterm_cu::{
    Command, TargetRef,
    command::{CLIPBOARD_UTF8_TEXT_TYPE, ClipboardWriteSource},
};

use super::verbs::VerbSpec;
use super::{flag_parsed, flag_text, flag_value, flag_window_opt, split_literal_tail, take_switch};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match spelled {
        "clipboard" => {
            let sub = args
                .first()
                .cloned()
                .filter(|first| !first.starts_with('-'));
            match sub {
                Some(sub) => {
                    args.remove(0);
                    subcommand(target, &sub, args)
                }
                None => read(target, args),
            }
        }
        "clip" => {
            if args.len() == 1 && args[0] == "--" {
                args.clear();
            }
            if !args.is_empty() {
                let text = if args.first().is_some_and(|arg| arg == "--") {
                    args.remove(0);
                    args.join(" ")
                } else {
                    std::mem::take(args).join(" ")
                };
                return Ok(Command::ClipboardWrite {
                    target,
                    type_name: CLIPBOARD_UTF8_TEXT_TYPE.to_owned(),
                    path: ClipboardWriteSource::Text { text },
                });
            }
            Ok(Command::ClipboardRead {
                target,
                metadata_only: false,
                type_name: None,
                max_bytes: None,
                out: None,
                replace: false,
            })
        }
        _ => match spec.name {
            "clipboard-read" => read(target, args),
            "clipboard-write" => write(target, args),
            "clipboard-write-file" => write_file(target, args),
            "clipboard-clear" => clear(target, args),
            "copy" => {
                let window = flag_window_opt(args);
                let name = flag_value(args, "--name");
                let role = flag_value(args, "--role");
                if window.is_none() {
                    return Err("copy requires --window <handle> [--name <pattern>]".into());
                }
                if name.as_ref().is_none_or(|value| value.is_empty())
                    && role.as_ref().is_some_and(|value| !value.is_empty())
                {
                    return Err("copy --role requires --name <pattern>".into());
                }
                Ok(Command::Copy {
                    target,
                    window,
                    name,
                    role,
                })
            }
            "paste" => {
                // `--` ends flag parsing so --text may itself start with a dash.
                let literal_text = split_literal_tail(args, " ");
                let window = flag_window_opt(args);
                let name = flag_value(args, "--name");
                let role = flag_value(args, "--role");
                let text = flag_value(args, "--text").or(literal_text);
                let allow_browser_chrome = take_switch(args, "--allow-browser-chrome");
                if window.is_none() {
                    return Err(
                        "paste requires --window <handle> [--name <pattern>] [--text TEXT]".into(),
                    );
                }
                if name.as_ref().is_none_or(|value| value.is_empty())
                    && role.as_ref().is_some_and(|value| !value.is_empty())
                {
                    return Err("paste --role requires --name <pattern>".into());
                }
                Ok(Command::Paste {
                    target,
                    text,
                    window,
                    name,
                    role,
                    allow_browser_chrome,
                })
            }
            other => Err(format!("unknown command '{other}'")),
        },
    }
}

fn subcommand(target: TargetRef, sub: &str, args: &mut Vec<String>) -> Result<Command, String> {
    match sub {
        "read" => read(target, args),
        "write" => write(target, args),
        "write-file" => write_file(target, args),
        "clear" => clear(target, args),
        other => Err(format!(
            "unknown clipboard subcommand {other:?}; expected read|write|write-file|clear"
        )),
    }
}

/// The first non-flag token, consumed (MCU positional form).
fn positional(args: &mut Vec<String>) -> Option<String> {
    args.first()
        .cloned()
        .filter(|first| !first.starts_with('-'))
        .inspect(|_| {
            args.remove(0);
        })
}

fn read(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let metadata_only = take_switch(args, "--metadata-only");
    let type_name = flag_text(args, "--type")?;
    let type_name = type_name.or_else(|| positional(args));
    let max_bytes = flag_parsed::<usize>(args, "--max-bytes")?;
    let out = flag_text(args, "--out")?;
    let replace = take_switch(args, "--replace");
    if !args.is_empty() {
        return Err(format!(
            "clipboard-read accepts only --metadata-only or --type T --max-bytes N --out PATH [--replace]; unexpected {:?}",
            args[0]
        ));
    }
    if metadata_only && (type_name.is_some() || max_bytes.is_some() || out.is_some() || replace) {
        return Err("clipboard-read --metadata-only cannot be combined with --type/--max-bytes/--out/--replace".into());
    }
    if type_name.is_none() && (max_bytes.is_some() || out.is_some() || replace) {
        return Err("clipboard-read --max-bytes/--out/--replace require --type".into());
    }
    Ok(Command::ClipboardRead {
        target,
        metadata_only,
        type_name,
        max_bytes,
        out,
        replace,
    })
}

fn write(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let type_name = flag_text(args, "--type")?;
    let path = flag_text(args, "--path")?;
    let text = flag_text(args, "--text")?;
    let type_name = type_name.or_else(|| positional(args));
    let path = if path.is_none() && text.is_none() {
        positional(args)
    } else {
        path
    };
    if !args.is_empty() {
        return Err(format!(
            "clipboard-write accepts --type T with exactly one of --path P or --text TEXT; unexpected {:?}",
            args[0]
        ));
    }
    let Some(type_name) = type_name else {
        return Err(
            "clipboard-write requires --type T and exactly one of --path P or --text TEXT".into(),
        );
    };
    let source = match (path, text) {
        (Some(path), None) => ClipboardWriteSource::Path(path),
        (None, Some(text)) => ClipboardWriteSource::Text { text },
        (Some(_), Some(_)) => {
            return Err("clipboard-write --path and --text are mutually exclusive".into());
        }
        (None, None) => {
            return Err("clipboard-write requires exactly one of --path P or --text TEXT".into());
        }
    };
    Ok(Command::ClipboardWrite {
        target,
        type_name,
        path: source,
    })
}

fn write_file(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let path = flag_text(args, "--path")?;
    let path = path.or_else(|| positional(args));
    if !args.is_empty() {
        return Err(format!(
            "clipboard-write-file accepts --path P; unexpected {:?}",
            args[0]
        ));
    }
    let Some(path) = path else {
        return Err("clipboard-write-file requires --path P".into());
    };
    Ok(Command::ClipboardWriteFile { target, path })
}

fn clear(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let apply = take_switch(args, "--apply");
    if !args.is_empty() {
        return Err(format!(
            "clipboard-clear accepts only --apply; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::ClipboardClear { target, apply })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenterm_cu::verb_catalog::resolve;

    fn parse_clip(args: &[&str]) -> Command {
        let spec = resolve("clip", None).expect("clip spec");
        let mut args = args.iter().map(|arg| (*arg).to_owned()).collect();
        parse(spec, "clip", TargetRef::Current, &mut args).expect("parse clip")
    }

    #[test]
    fn clip_with_text_preserves_mcu_space_join_and_utf8() {
        let command = parse_clip(&["alpha", "中🚀", "omega"]);
        match command {
            Command::ClipboardWrite {
                type_name,
                path: ClipboardWriteSource::Text { text },
                ..
            } => {
                assert_eq!(type_name, CLIPBOARD_UTF8_TEXT_TYPE);
                assert_eq!(text, "alpha 中🚀 omega");
            }
            _ => panic!("clip TEXT must become a direct clipboard write"),
        }
    }

    #[test]
    fn clip_empty_argument_is_a_write_but_no_arguments_is_a_read() {
        assert!(matches!(parse_clip(&[]), Command::ClipboardRead { .. }));
        assert!(matches!(parse_clip(&["--"]), Command::ClipboardRead { .. }));
        assert!(matches!(
            parse_clip(&[""]),
            Command::ClipboardWrite {
                path: ClipboardWriteSource::Text { text },
                ..
            } if text.is_empty()
        ));
    }

    #[test]
    fn clipboard_write_text_and_path_are_strictly_exclusive() {
        let spec = resolve("clipboard-write", None).expect("clipboard-write spec");
        let mut args = [
            "--type",
            CLIPBOARD_UTF8_TEXT_TYPE,
            "--path",
            "payload.bin",
            "--text",
            "private",
        ]
        .map(str::to_owned)
        .to_vec();
        let error = parse(spec, "clipboard-write", TargetRef::Current, &mut args)
            .expect_err("mixed sources must fail");
        assert_eq!(
            error,
            "clipboard-write --path and --text are mutually exclusive"
        );
        assert!(!error.contains("private"));
    }
}
