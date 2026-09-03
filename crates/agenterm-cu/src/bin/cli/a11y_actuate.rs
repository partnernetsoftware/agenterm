//! Accessibility actuation: invoke / menu invoke / click / focus, the text
//! and key writers, the AT-SPI setters, and `pointer-move`.

use agenterm_cu::{Command, PointerButton, TargetRef, command::InvokeAction};

use super::verbs::VerbSpec;
use super::{
    flag_coords, flag_i32, flag_parsed, flag_text, flag_u32, flag_value, flag_window,
    flag_window_opt, menu, named_node, split_literal_tail, take_switch,
};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    if spelled == "menu" {
        return menu::parse(target, args);
    }
    match spec.name {
        "invoke" => invoke(target, args),
        "menu-invoke" => menu::parse(target, args),
        "click" => click(spelled, target, args),
        "focus" => {
            let window = flag_window_opt(args);
            let node = flag_value(args, "--node");
            let name = flag_value(args, "--name");
            let role = flag_value(args, "--role");
            if node.as_ref().is_none_or(|value| value.is_empty())
                && name.as_ref().is_none_or(|value| value.is_empty())
            {
                return Err(
                    "focus requires --node <path-id> or --window <handle> --name <pattern>".into(),
                );
            }
            Ok(Command::Focus {
                target,
                window,
                node,
                name,
                role,
            })
        }
        "send-text" => {
            // `--` ends flag parsing so the text may itself start with a dash.
            let literal_text = split_literal_tail(args, " ");
            let window = flag_window_opt(args);
            let name = flag_value(args, "--name");
            let role = flag_value(args, "--role");
            let allow_browser_chrome = take_switch(args, "--allow-browser-chrome");
            Ok(Command::SendText {
                target,
                text: literal_text.unwrap_or_else(|| args.join(" ")),
                window,
                name,
                role,
                allow_browser_chrome,
            })
        }
        "send-keys" => {
            // `--` ends flag parsing so a chord may itself start with a dash.
            let literal_keys = split_literal_tail(args, "+");
            let window = flag_window_opt(args);
            let name = flag_value(args, "--name");
            let role = flag_value(args, "--role");
            let allow_browser_chrome = take_switch(args, "--allow-browser-chrome");
            Ok(Command::SendKeys {
                target,
                keys: literal_keys.unwrap_or_else(|| args.join("+")),
                window,
                name,
                role,
                allow_browser_chrome,
            })
        }
        "scroll" => {
            let (window, name, role) =
                named_node(args, "scroll requires --window <handle> --name <pattern>")?;
            Ok(Command::Scroll {
                target,
                window,
                name,
                role,
            })
        }
        "select" => {
            let window = flag_window_opt(args);
            let name = flag_value(args, "--name");
            let role = flag_value(args, "--role");
            let start = flag_i32(args, "--start");
            let end = flag_i32(args, "--end");
            if name.as_ref().is_none_or(|value| value.is_empty())
                || start.is_none()
                || end.is_none()
            {
                return Err(
                    "select requires --window <handle> --name <pattern> --start N --end M".into(),
                );
            }
            Ok(Command::Select {
                target,
                start: start.unwrap_or(0),
                end: end.unwrap_or(0),
                window,
                name,
                role,
            })
        }
        "set-caret" => {
            let window = flag_window_opt(args);
            let name = flag_value(args, "--name");
            let role = flag_value(args, "--role");
            let offset = flag_i32(args, "--offset");
            if name.as_ref().is_none_or(|value| value.is_empty()) || offset.is_none() {
                return Err(
                    "set-caret requires --window <handle> --name <pattern> --offset N".into(),
                );
            }
            Ok(Command::SetCaret {
                target,
                offset: offset.unwrap_or(0),
                window,
                name,
                role,
            })
        }
        "pointer-move" => pointer_move(target, args),
        other => Err(format!("unknown command '{other}'")),
    }
}

/// Closed shape: flags first, then exactly `<action> [VALUE]`.
fn invoke(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let Some(window) = flag_window(args)? else {
        return Err("invoke requires --window <handle>".into());
    };
    let node = flag_text(args, "--node")?;
    let index = flag_parsed::<usize>(args, "--index")?;
    let name = flag_text(args, "--name")?;
    let identifier = flag_text(args, "--identifier")?;
    let role = flag_text(args, "--role")?;
    let focused = take_switch(args, "--focused");
    let selector = flag_text(args, "--selector")?;
    if let Some(raw) = selector.as_deref() {
        agenterm_cu::observe::parse_selector(raw)?;
    }
    if node.is_none()
        && index.is_none()
        && name.is_none()
        && identifier.is_none()
        && !focused
        && selector.is_none()
    {
        return Err(
            "invoke requires one of --node PATH, --index N, --name PAT [--role ROLE], --identifier ID, --focused [--role ROLE], --selector PATH".into(),
        );
    }
    if focused
        && (node.is_some()
            || index.is_some()
            || name.is_some()
            || identifier.is_some()
            || selector.is_some())
    {
        return Err(
            "invoke --focused addresses the focused control; combine it only with --role".into(),
        );
    }
    if selector.is_some()
        && (node.is_some() || index.is_some() || name.is_some() || identifier.is_some())
    {
        return Err("invoke --selector cannot mix with --node/--index/--name/--identifier".into());
    }
    if let Some(stray) = args.iter().find(|arg| arg.starts_with("--")) {
        return Err(format!(
            "invoke accepts only --window H --node PATH | --index N | --name PAT [--role ROLE] | --identifier ID | --focused [--role ROLE], then <action> [VALUE]; unexpected {stray:?}"
        ));
    }
    let actions = || {
        InvokeAction::ALL
            .iter()
            .map(|action| action.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    };
    let Some(action_raw) = args.first().cloned() else {
        return Err(format!("invoke requires an action: {}", actions()));
    };
    let Some(action) = InvokeAction::parse(&action_raw) else {
        return Err(format!(
            "unknown invoke action {action_raw:?}; expected one of {}",
            actions()
        ));
    };
    let value = args.get(1).cloned();
    if args.len() > 2 {
        return Err(format!(
            "invoke takes at most one VALUE after the action; unexpected {:?}",
            args[2]
        ));
    }
    Ok(Command::Invoke {
        target,
        window,
        node,
        index,
        name,
        identifier,
        role,
        action,
        value,
        focused,
        selector,
    })
}

/// `click`, with the MCU presets `dclick` (two clicks) and `rclick` (right
/// button) keyed on the spelling.
fn click(spelled: &str, target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let mut window = flag_window_opt(args);
    match flag_text(args, "--to")? {
        Some(to) if to == "desktop" => {
            return Err(
                "click --to desktop is not mapped; use invoke or pointer-move --to desktop".into(),
            );
        }
        Some(to) => {
            if window.is_some() {
                return Err("click accepts --window or --to, not both".into());
            }
            window = match agenterm_cu::observe::parse_window_token(&to) {
                Ok(handle) => Some(handle),
                Err(message) => return Err(message.replace("--window", "--to")),
            };
        }
        None => {}
    }
    let node = flag_value(args, "--node");
    let name = flag_value(args, "--name");
    let role = flag_value(args, "--role");
    let coords = flag_coords(args, "--coords");
    let degraded = take_switch(args, "--degraded");
    let clicks = if spelled == "dclick" {
        2
    } else {
        flag_u32(args, "--clicks").unwrap_or(1)
    };
    let button = if spelled == "rclick" {
        PointerButton::Right
    } else {
        match flag_value(args, "--button").as_deref() {
            Some("right") => PointerButton::Right,
            Some("middle") => PointerButton::Middle,
            _ => PointerButton::Left,
        }
    };
    Ok(Command::Click {
        target,
        window,
        node,
        name,
        role,
        coords,
        degraded,
        clicks,
        button,
    })
}

fn pointer_move(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let Some(to) = flag_text(args, "--to")? else {
        return Err("pointer-move requires --to desktop (explicit global) or --to <handle>".into());
    };
    if to != "desktop" && agenterm_cu::observe::parse_window_token(&to).is_err() {
        return Err(
            "pointer-move --to must be desktop or App#N/handle; window-local pointer is not mapped"
                .into(),
        );
    }
    if to != "desktop" {
        return Err(
            "pointer-move --to <handle> is typed unsupported; use --to desktop or click --window"
                .into(),
        );
    }
    let x = required_i32_flag(args, "--x")?;
    let y = required_i32_flag(args, "--y")?;
    if !args.is_empty() {
        return Err("pointer-move accepts only --to desktop --x <i32> --y <i32>".into());
    }
    Ok(Command::PointerMove { target, x, y })
}

fn required_i32_flag(args: &mut Vec<String>, flag: &'static str) -> Result<i32, String> {
    let Some(index) = args.iter().position(|arg| arg == flag) else {
        return Err(format!("pointer-move requires {flag} <i32>"));
    };
    args.remove(index);
    if index >= args.len() {
        return Err(format!("pointer-move requires {flag} <i32>"));
    }
    let raw = args.remove(index);
    raw.parse::<i32>()
        .map_err(|_| format!("pointer-move {flag} must be a signed 32-bit integer"))
}
