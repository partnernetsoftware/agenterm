//! Windows & apps: inventory, watch, application steps, the destructive
//! `close`, receipts, spaces and displays.

use agenterm_cu::{Command, TargetRef};

use super::verbs::VerbSpec;
use super::{flag_parsed, flag_text, flag_tristate, flag_window, take_switch};

pub fn parse(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match spec.name {
        "windows" => windows(spelled, target, args),
        "windows-watch" => windows_watch(target, args),
        "apps" => apps(target, args),
        "app-watch" => app_watch(target, args),
        "app-facts" => app_facts(target, args),
        "app-inspect" => app_inspect(target, args),
        "app" => super::app::parse(spelled, target, args),
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
        "activate" => {
            let Some(window) = flag_window(args)? else {
                return Err("activate requires --window <handle>".into());
            };
            if !args.is_empty() {
                return Err(format!(
                    "activate accepts only --window H; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Activate { target, window })
        }
        "raise" => {
            let Some(window) = flag_window(args)? else {
                return Err("raise requires --window <handle>".into());
            };
            if !args.is_empty() {
                return Err(format!(
                    "raise accepts only --window H; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Raise { target, window })
        }
        // Window 0 lets the executor name `target` among the missing gate
        // parts in one typed refusal, exactly as `close` does.
        "minimize" => {
            let (window, expect) = window_state(spec.name, args)?;
            Ok(Command::Minimize {
                target,
                window,
                expect,
            })
        }
        "restore" => {
            let (window, expect) = window_state(spec.name, args)?;
            Ok(Command::Restore {
                target,
                window,
                expect,
            })
        }
        "window-opacity" => {
            let Some(window) = flag_window(args)? else {
                return Ok(Command::WindowOpacity {
                    target,
                    window: 0,
                    opacity_permille: 0,
                    expect: flag_text(args, "--expect")?,
                });
            };
            let opacity_text = flag_text(args, "--opacity")?;
            let expect = flag_text(args, "--expect")?;
            if !args.is_empty() {
                return Err(format!(
                    "window-opacity accepts only --window H --opacity 0..1 --expect <same>; unexpected {:?}",
                    args[0]
                ));
            }
            let opacity_permille = match opacity_text.as_deref() {
                Some(value) => parse_opacity_permille(value)?,
                None => 0,
            };
            Ok(Command::WindowOpacity {
                target,
                window,
                opacity_permille,
                expect,
            })
        }
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

/// `windows`, and its alias `focused-window` (= `windows --focused true`:
/// the one focused window, or the explicit `{focused_app, window: null}`).
fn windows(spelled: &str, target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = flag_parsed::<u32>(args, "--pid")?;
    let app = flag_text(args, "--app")?;
    let title = flag_text(args, "--title")?;
    let mut focused = flag_tristate(args, "--focused");
    if spelled == "focused-window" {
        if focused == Some(false) {
            return Err(
                "focused-window is windows --focused true; it does not take --focused false".into(),
            );
        }
        focused = Some(true);
    }
    let minimized = flag_tristate(args, "--minimized");
    let browser_profile = flag_text(args, "--browser-profile")?;
    if browser_profile
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err("windows --browser-profile must not be empty".into());
    }
    let offset = flag_parsed::<usize>(args, "--offset")?;
    let max = flag_parsed::<usize>(args, "--max")?;
    if !args.is_empty() {
        return Err(format!(
            "windows accepts only --pid N --app SUB --title SUB --focused [BOOL] \
             --minimized [BOOL] --browser-profile SUB --offset N --max N; unexpected {:?}",
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
        browser_profile,
        offset,
        max,
    })
}

fn windows_watch(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let pid = flag_parsed::<u32>(args, "--pid")?;
    let app = flag_text(args, "--app")?;
    let title = flag_text(args, "--title")?;
    let space = flag_parsed::<u64>(args, "--space")?;
    if space == Some(0) {
        return Err("windows-watch --space must be a positive managed Space id".into());
    }
    let duration_ms = flag_parsed::<u64>(args, "--duration-ms")?.unwrap_or(0);
    let interval_ms = flag_parsed::<u64>(args, "--interval-ms")?;
    let max_events = flag_parsed::<usize>(args, "--max-events")?;
    if !args.is_empty() {
        return Err(format!(
            "windows-watch accepts only --pid N --app SUB --title SUB --space ID \
             --duration-ms N --interval-ms N --max-events N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::WindowsWatch {
        target,
        pid,
        app,
        title,
        space,
        duration_ms,
        interval_ms,
        max_events,
    })
}

fn apps(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    if args.first().is_some_and(|argument| argument == "watch") {
        return app_watch(target, args);
    }
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

fn app_watch(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    // A catalog alias such as `apps watch` is resolved before parsing, but the
    // argv cursor deliberately still contains the subcommand token.
    let alias = args.first().is_some_and(|argument| argument == "watch");
    if alias {
        args.remove(0);
    }
    let mut selectors = Vec::new();
    if alias
        && args
            .first()
            .is_some_and(|argument| !argument.starts_with("--"))
    {
        selectors.push(args.remove(0));
    }
    let positional_selector = !selectors.is_empty();
    while let Some(selector) = flag_text(args, "--app")? {
        if positional_selector {
            return Err("apps watch SELECTOR cannot be combined with --app".into());
        }
        selectors.push(selector);
    }
    let duration_ms = flag_parsed::<u64>(args, "--duration-ms")?.unwrap_or(30_000);
    let interval_ms = flag_parsed::<u64>(args, "--interval-ms")?;
    let max_events = flag_parsed::<usize>(args, "--max-events")?;
    let max_processes = flag_parsed::<usize>(args, "--max-processes")?;
    if selectors.is_empty()
        || selectors.len() > 16
        || selectors.iter().any(|selector| {
            selector.trim().is_empty()
                || selector.len() > agenterm_platform::app_facts::MAX_APP_FACTS_SELECTOR_BYTES
                || selector.as_bytes().contains(&0)
        })
        || !(1..=86_400_000).contains(&duration_ms)
        || interval_ms.is_some_and(|value| !(1..=60_000).contains(&value))
        || max_events.is_some_and(|value| !(1..=4_096).contains(&value))
        || max_processes.is_some_and(|value| !(1..=5_000).contains(&value))
    {
        return Err("app-watch requires 1..=16 non-empty --app selectors, duration-ms in 1..=86400000, interval-ms in 1..=60000, max-events in 1..=4096 and max-processes in 1..=5000".into());
    }
    if !args.is_empty() {
        return Err(format!("app-watch received unexpected {:?}", args[0]));
    }
    Ok(Command::AppWatch {
        target,
        selectors,
        duration_ms,
        interval_ms,
        max_events,
        max_processes,
    })
}

fn app_facts(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let selector = flag_text(args, "--selector")?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "app-facts requires --selector VALUE".to_owned())?;
    let signing = take_switch(args, "--signing");
    let verify = take_switch(args, "--verify");
    let entitlements = take_switch(args, "--entitlements");
    if !args.is_empty() {
        return Err(format!(
            "app-facts accepts only --selector VALUE [--signing] [--verify] [--entitlements]; unexpected {:?}",
            args[0]
        ));
    }
    let command = Command::AppFacts {
        target,
        selector,
        signing,
        verify,
        entitlements,
    };
    command.validate().map_err(str::to_owned)?;
    Ok(command)
}

pub(crate) fn app_inspect(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let app = flag_text(args, "--app")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "app-inspect requires --app <name>".to_owned())?;
    let depth = flag_parsed::<u32>(args, "--depth")?;
    let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
    let max_windows = flag_parsed::<usize>(args, "--max-windows")?;
    if !args.is_empty() {
        return Err(format!(
            "app-inspect accepts only --app NAME --depth N --max-nodes N --max-windows N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::AppInspect {
        target,
        app,
        depth,
        max_nodes,
        max_windows,
    })
}

/// `minimize` / `restore`: `--window H --expect <word>`, both parts of the
/// gate parsed leniently so the executor can name every missing one in a
/// single typed refusal instead of a usage error per flag.
fn window_state(verb: &str, args: &mut Vec<String>) -> Result<(isize, Option<String>), String> {
    let window = flag_window(args)?.unwrap_or(0);
    let expect = flag_text(args, "--expect")?;
    if !args.is_empty() {
        return Err(format!(
            "{verb} accepts only --window H --expect <postcondition>; unexpected {:?}",
            args[0]
        ));
    }
    Ok((window, expect))
}

fn parse_opacity_permille(text: &str) -> Result<u32, String> {
    let parsed: f64 = text
        .parse()
        .map_err(|_| format!("opacity must be a number 0..1: {text}"))?;
    if !(0.0..=1.0).contains(&parsed) {
        return Err(format!("opacity must be between 0 and 1: {text}"));
    }
    Ok(((parsed * 1000.0).round() as u32).min(1000))
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

#[cfg(test)]
mod app_inspect_tests {
    use super::*;

    #[test]
    fn parses_closed_bounded_app_inspect_shape() {
        let mut args = vec![
            "--app".into(),
            "Editor".into(),
            "--depth".into(),
            "12".into(),
            "--max-nodes".into(),
            "6000".into(),
            "--max-windows".into(),
            "8".into(),
        ];
        assert!(matches!(
            app_inspect(TargetRef::Current, &mut args).unwrap(),
            Command::AppInspect {
                app,
                depth: Some(12),
                max_nodes: Some(6000),
                max_windows: Some(8),
                ..
            } if app == "Editor"
        ));
        assert!(app_inspect(TargetRef::Current, &mut Vec::new()).is_err());
        assert!(
            app_inspect(
                TargetRef::Current,
                &mut vec!["--app".into(), "Editor".into(), "--unknown".into()]
            )
            .is_err()
        );
    }

    #[test]
    fn parses_closed_app_facts_shape() {
        let mut args = vec![
            "--selector".into(),
            "org.example.Editor.desktop".into(),
            "--signing".into(),
            "--verify".into(),
            "--entitlements".into(),
        ];
        assert!(matches!(
            app_facts(TargetRef::Current, &mut args).unwrap(),
            Command::AppFacts {
                selector,
                signing: true,
                verify: true,
                entitlements: true,
                ..
            } if selector == "org.example.Editor.desktop"
        ));
        assert!(app_facts(TargetRef::Current, &mut Vec::new()).is_err());
    }

    #[test]
    fn parses_repeated_app_watch_selectors_and_alias_token() {
        let mut canonical = vec![
            "--app".into(),
            "Editor".into(),
            "--app".into(),
            "Viewer".into(),
            "--duration-ms".into(),
            "4000".into(),
            "--interval-ms".into(),
            "100".into(),
        ];
        assert!(matches!(
            app_watch(TargetRef::Current, &mut canonical).unwrap(),
            Command::AppWatch {
                selectors,
                duration_ms: 4000,
                interval_ms: Some(100),
                ..
            } if selectors == ["Editor", "Viewer"]
        ));

        let mut args = vec![
            "watch".into(),
            "Editor".into(),
            "--duration-ms".into(),
            "4000".into(),
            "--interval-ms".into(),
            "100".into(),
        ];
        assert!(matches!(
            apps(TargetRef::Current, &mut args).unwrap(),
            Command::AppWatch {
                selectors,
                duration_ms: 4000,
                interval_ms: Some(100),
                ..
            } if selectors == ["Editor"]
        ));

        assert!(
            app_watch(
                TargetRef::Current,
                &mut vec![
                    "watch".into(),
                    "Editor".into(),
                    "--app".into(),
                    "Viewer".into()
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn app_watch_rejects_missing_and_out_of_range_fields() {
        assert!(app_watch(TargetRef::Current, &mut Vec::new()).is_err());
        assert!(
            app_watch(
                TargetRef::Current,
                &mut vec![
                    "--app".into(),
                    "Editor".into(),
                    "--interval-ms".into(),
                    "0".into()
                ]
            )
            .is_err()
        );
        let mut too_many = Vec::new();
        for index in 0..17 {
            too_many.extend(["--app".into(), format!("App {index}")]);
        }
        assert!(app_watch(TargetRef::Current, &mut too_many).is_err());
    }
}
