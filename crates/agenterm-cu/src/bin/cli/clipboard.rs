//! Clipboard: the native read / write / clear verbs, the MCU `clipboard`
//! group word and `clip`, plus the a11y-addressed `copy` / `paste`.

use agenterm_cu::{Command, TargetRef};

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
            if !args.is_empty() {
                return Err(format!(
                    "clip with no text is clipboard-read; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::ClipboardRead {
                target,
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
    let type_name = flag_text(args, "--type")?;
    let type_name = type_name.or_else(|| positional(args));
    let max_bytes = flag_parsed::<usize>(args, "--max-bytes")?;
    let out = flag_text(args, "--out")?;
    let replace = take_switch(args, "--replace");
    if !args.is_empty() {
        return Err(format!(
            "clipboard-read accepts only --type T --max-bytes N --out PATH [--replace]; unexpected {:?}",
            args[0]
        ));
    }
    if type_name.is_none() && (max_bytes.is_some() || out.is_some() || replace) {
        return Err("clipboard-read --max-bytes/--out/--replace require --type".into());
    }
    Ok(Command::ClipboardRead {
        target,
        type_name,
        max_bytes,
        out,
        replace,
    })
}

fn write(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let type_name = flag_text(args, "--type")?;
    let path = flag_text(args, "--path")?;
    let type_name = type_name.or_else(|| positional(args));
    let path = path.or_else(|| positional(args));
    if !args.is_empty() {
        return Err(format!(
            "clipboard-write accepts --type T --path P; unexpected {:?}",
            args[0]
        ));
    }
    let (Some(type_name), Some(path)) = (type_name, path) else {
        return Err("clipboard-write requires --type T --path P".into());
    };
    Ok(Command::ClipboardWrite {
        target,
        type_name,
        path,
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
