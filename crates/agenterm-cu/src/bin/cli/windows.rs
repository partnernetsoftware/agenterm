//! Windows & apps: inventory, watch, application steps, the destructive
//! `close`, receipts, spaces and displays.

use agenterm_cu::{Command, TargetRef, command::AppAction};

use super::verbs::VerbSpec;
use super::{flag_parsed, flag_text, flag_tristate, flag_value, flag_window, take_switch};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match spec.name {
        "capabilities" => Ok(Command::Capabilities { target }),
        "windows" => windows(target, args),
        "windows-watch" => windows_watch(target, args),
        "apps" => apps(target, args),
        "app" => app(spelled, target, args),
        "unlock" => {
            let Some(window) = flag_window(args)? else {
                return Err("unlock requires --window <handle>".into());
            };
            if !args.is_empty() {
                return Err(format!(
                    "unlock accepts only --window H; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Unlock { target, window })
        }
        "close" => close(target, args),
        "receipts" => {
            let window = flag_window(args)?;
            let max = flag_parsed::<usize>(args, "--max")?;
            if !args.is_empty() {
                return Err(format!(
                    "receipts accepts only [--window H] [--max N]; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Receipts {
                target,
                window,
                max,
            })
        }
        "spaces" => {
            if !args.is_empty() {
                return Err(format!(
                    "spaces accepts no arguments; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Spaces { target })
        }
        "displays" => {
            if !args.is_empty() {
                return Err(format!(
                    "displays accepts no arguments; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Displays { target })
        }
        other => Err(format!("unknown command '{other}'")),
    }
}

fn windows(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = flag_parsed::<u32>(args, "--pid")?;
    let app = flag_text(args, "--app")?;
    let title = flag_text(args, "--title")?;
    let focused = flag_tristate(args, "--focused");
    let minimized = flag_tristate(args, "--minimized");
    let offset = flag_parsed::<usize>(args, "--offset")?;
    let max = flag_parsed::<usize>(args, "--max")?;
    if !args.is_empty() {
        return Err(format!(
            "windows accepts only --pid N --app SUB --title SUB --focused [BOOL] \
             --minimized [BOOL] --offset N --max N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::Windows {
        target,
        pid,
        app,
        title,
        focused,
        minimized,
        offset,
        max,
    })
}

fn windows_watch(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = flag_parsed::<u32>(args, "--pid")?;
    let app = flag_text(args, "--app")?;
    let title = flag_text(args, "--title")?;
    let duration_ms = flag_parsed::<u64>(args, "--duration-ms")?.unwrap_or(0);
    let interval_ms = flag_parsed::<u64>(args, "--interval-ms")?;
    let max_events = flag_parsed::<usize>(args, "--max-events")?;
    if !args.is_empty() {
        return Err(format!(
            "windows-watch accepts only --pid N --app SUB --title SUB \
             --duration-ms N --interval-ms N --max-events N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::WindowsWatch {
        target,
        pid,
        app,
        title,
        duration_ms,
        interval_ms,
        max_events,
    })
}

fn apps(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let running = take_switch(args, "--running");
    let all = take_switch(args, "--all");
    if !args.is_empty() {
        return Err(format!(
            "apps accepts only --running / --all; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::Apps {
        target,
        running,
        all,
    })
}

/// `app <action> …`; the MCU spellings `launch PATH`, `quit`, `hide` and
/// `show` put their own name back as the action.
fn app(spelled: &str, target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    if spelled != "app" {
        if AppAction::parse(spelled) == Some(AppAction::Launch)
            && !args.iter().any(|arg| arg == "--path")
            && args.first().is_some_and(|first| !first.starts_with('-'))
        {
            args.insert(0, "--path".into());
        }
        args.insert(0, spelled.to_string());
    }
    let action_text = flag_value(args, "--action")
        .or_else(|| {
            args.first()
                .cloned()
                .filter(|first| !first.starts_with("--"))
        })
        .unwrap_or_default();
    if !action_text.is_empty() && args.first() == Some(&action_text) {
        args.remove(0);
    }
    let Some(action) = AppAction::parse(&action_text) else {
        return Err("app requires hide | show | quit (or --action <one of them>)".into());
    };
    let window = flag_window(args)?;
    let pid = flag_parsed::<u32>(args, "--pid")?;
    // `launch` names a path rather than a running thing; `show` has no
    // window to name because hiding removed them, so the pid stands in.
    // Everything else still wants a handle.
    let launching = action == AppAction::Launch;
    if !launching && window.is_none() && pid.is_none() {
        return Err("app requires --window <handle> or --pid <n>".into());
    }
    let window = window.unwrap_or(0);
    let snapshot = take_switch(args, "--snapshot");
    let expect = flag_text(args, "--expect")?;
    let path = flag_text(args, "--path")?;
    if !args.is_empty() {
        return Err(format!(
            "app accepts only <hide|show|quit|launch> --window H | --pid N | --path P [--snapshot --expect gone]; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::App {
        target,
        window,
        action,
        snapshot,
        expect,
        pid,
        path,
    })
}

/// The destructive verb: closed shape, every part of the gate is a flag the
/// executor checks before touching anything.
fn close(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    // Window 0 lets the executor name `target` among the missing gate parts
    // in one typed refusal.
    let window = flag_window(args)?.unwrap_or(0);
    let pid = flag_parsed::<u32>(args, "--pid")?;
    let title = flag_text(args, "--title")?;
    let snapshot = take_switch(args, "--snapshot");
    let expect = flag_text(args, "--expect")?;
    if !args.is_empty() {
        return Err(format!(
            "close accepts only --window H [--pid N] [--title T] --snapshot --expect gone; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::Close {
        target,
        window,
        pid,
        title,
        snapshot,
        expect,
    })
}
