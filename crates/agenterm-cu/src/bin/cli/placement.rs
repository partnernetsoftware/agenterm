//! Window placement: the 18-action `window-place` catalog, the MCU
//! shorthands `frame` / `movewin` / `resize` / `maximize`, and `orderwin`.

use agenterm_cu::{Command, OrderRelation, TargetRef};

use super::parse_optional_window;
use super::verbs::VerbSpec;
use super::{flag_handle, flag_parsed, flag_text, flag_value, flag_window, flag_window_opt};

pub fn parse(
    spec: &VerbSpec,
    _spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match spec.name {
        "window-place" => window_place(target, args),
        "frame" | "movewin" | "resize" | "maximize" => shorthand(spec.name, target, args),
        "orderwin" => orderwin(target, args),
        other => Err(format!("unknown command '{other}'")),
    }
}

fn window_place(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let action = flag_value(args, "--action")
        .or_else(|| args.first().cloned())
        .unwrap_or_default();
    if action.is_empty() {
        return Err("window-place requires --action <id>".into());
    }
    let window = flag_window_opt(args);
    // `--action frame` takes the requested rect; the four flags are typed
    // (a bad value is usage, never a silently dropped flag).
    let mut rect = [None; 4];
    for (slot, flag) in ["--x", "--y", "--width", "--height"]
        .into_iter()
        .enumerate()
    {
        rect[slot] = flag_parsed::<i32>(args, flag)?;
    }
    let frame = match rect {
        [None, None, None, None] => None,
        [Some(x), Some(y), Some(width), Some(height)] => Some([x, y, width, height]),
        _ => {
            return Err(
                "window-place --action frame needs all four of --x --y --width --height".into(),
            );
        }
    };
    if action == "frame" && frame.is_none() {
        return Err("window-place --action frame requires --x X --y Y --width W --height H".into());
    }
    Ok(Command::WindowPlace {
        target,
        action,
        window,
        frame,
    })
}

fn take_i32_flag_or_positional(
    args: &mut Vec<String>,
    flag: &'static str,
) -> Result<Option<i32>, String> {
    if let Some(value) = flag_parsed::<i32>(args, flag)? {
        return Ok(Some(value));
    }
    let Some(first) = args.first() else {
        return Ok(None);
    };
    if first.starts_with('-') && first != "-" && first.parse::<i32>().is_err() {
        return Ok(None);
    }
    let raw = args.remove(0);
    raw.parse::<i32>()
        .map(Some)
        .map_err(|_| format!("{flag} value {raw:?} is not an i32"))
}

/// `frame` / `movewin` / `resize` / `maximize`: flags or MCU positionals,
/// all answering as `window-place`.
fn shorthand(verb: &str, target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let window = parse_optional_window(args)?;
    let command = match verb {
        "maximize" => {
            if !args.is_empty() {
                return Err(format!(
                    "maximize accepts --window H; unexpected {:?}",
                    args[0]
                ));
            }
            Command::WindowPlace {
                target,
                action: "fullscreen".into(),
                window,
                frame: None,
            }
        }
        "movewin" => {
            let x = take_i32_flag_or_positional(args, "--x")?
                .ok_or_else(|| "movewin requires --x X --y Y (or HANDLE X Y)".to_owned())?;
            let y = take_i32_flag_or_positional(args, "--y")?
                .ok_or_else(|| "movewin requires --x X --y Y (or HANDLE X Y)".to_owned())?;
            if !args.is_empty() {
                return Err(format!("movewin unexpected {:?}", args[0]));
            }
            Command::WindowPlace {
                target,
                action: "move".into(),
                window,
                frame: Some([x, y, 0, 0]),
            }
        }
        "resize" => {
            let width = take_i32_flag_or_positional(args, "--width")?
                .ok_or_else(|| "resize requires --width W --height H (or HANDLE W H)".to_owned())?;
            let height = take_i32_flag_or_positional(args, "--height")?
                .ok_or_else(|| "resize requires --width W --height H (or HANDLE W H)".to_owned())?;
            if !args.is_empty() {
                return Err(format!("resize unexpected {:?}", args[0]));
            }
            Command::WindowPlace {
                target,
                action: "resize".into(),
                window,
                frame: Some([0, 0, width, height]),
            }
        }
        _ => {
            let missing =
                || "frame requires --x --y --width --height (or HANDLE X Y W H)".to_owned();
            let x = take_i32_flag_or_positional(args, "--x")?.ok_or_else(missing)?;
            let y = take_i32_flag_or_positional(args, "--y")?.ok_or_else(missing)?;
            let width = take_i32_flag_or_positional(args, "--width")?.ok_or_else(missing)?;
            let height = take_i32_flag_or_positional(args, "--height")?.ok_or_else(missing)?;
            if !args.is_empty() {
                return Err(format!("frame unexpected {:?}", args[0]));
            }
            Command::WindowPlace {
                target,
                action: "frame".into(),
                window,
                frame: Some([x, y, width, height]),
            }
        }
    };
    Ok(command)
}

fn orderwin(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let Some(window) = flag_window(args)? else {
        return Err("orderwin requires --window <handle>".into());
    };
    let Some(relative) = flag_handle(args, "--relative")? else {
        return Err("orderwin requires --relative <handle>".into());
    };
    let relation = match flag_text(args, "--relation")? {
        Some(raw) => match OrderRelation::parse(&raw) {
            Some(value) => value,
            None => return Err("orderwin --relation must be above or below".into()),
        },
        None => return Err("orderwin requires --relation above|below".into()),
    };
    if !args.is_empty() {
        return Err(format!(
            "orderwin accepts only --window H --relation above|below --relative H; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::OrderWin {
        target,
        window,
        relation,
        relative,
    })
}
