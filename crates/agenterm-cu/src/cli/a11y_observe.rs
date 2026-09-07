//! Accessibility observation: tree / query / hit / focused / observe /
//! verify, the incremental pair snapshot / diff, the independent AT-SPI
//! read-backs, `wait`, and the non-tree observers (`screenshot`, `zoom`,
//! `device-screenshot`, `pointer-position`, and `desktop-state`).

use agenterm_cu::{Command, QueryWatchUntil, TargetRef, WaitCondition};

use super::verbs::VerbSpec;
use super::{
    flag_isize, flag_parsed, flag_text, flag_u64, flag_usize, flag_value, flag_window,
    flag_window_opt, menu, named_node, parse_expectations, parse_optional_window,
    split_literal_tail, take_switch,
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
        "tree" => {
            let window = flag_window(args)?;
            let depth = flag_parsed::<u32>(args, "--depth")?;
            let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
            let selector = flag_text(args, "--selector")?;
            if let Some(raw) = selector.as_deref() {
                agenterm_cu::observe::parse_selector(raw)?;
            }
            // `elements` is the MCU spelling of `tree --flat`.
            let flat = spelled == "elements" || take_switch(args, "--flat");
            if !args.is_empty() {
                return Err(format!(
                    "tree accepts only [--window H] [--depth N] [--max-nodes N] [--flat] [--selector PATH]; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Tree {
                target,
                window,
                depth,
                max_nodes,
                flat,
                selector,
            })
        }
        "desktop-state" => {
            let window = flag_window(args)?;
            let depth = flag_parsed::<u32>(args, "--depth")?;
            let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
            if !args.is_empty() {
                return Err(format!(
                    "desktop-state accepts only [--window HANDLE] [--depth N] [--max-nodes N]; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::DesktopState {
                target,
                window,
                depth,
                max_nodes,
            })
        }
        "query" => query(target, spelled, args),
        "hit" => {
            let Some(window) = flag_window(args)? else {
                return Err("hit requires --window <handle>".into());
            };
            let Some(x) = flag_parsed::<i32>(args, "--x")? else {
                return Err("hit requires --x <screen x>".into());
            };
            let Some(y) = flag_parsed::<i32>(args, "--y")? else {
                return Err("hit requires --y <screen y>".into());
            };
            let depth = flag_parsed::<u32>(args, "--depth")?;
            let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
            if !args.is_empty() {
                return Err(format!(
                    "hit accepts only --window H --x X --y Y [--depth N] [--max-nodes N]; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Hit {
                target,
                window,
                x,
                y,
                depth,
                max_nodes,
            })
        }
        "snapshot" => {
            let Some(window) = flag_window(args)? else {
                return Err("snapshot requires --window <handle>".into());
            };
            let depth = flag_parsed::<u32>(args, "--depth")?;
            let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
            let out = flag_text(args, "--out")?;
            let shot = take_switch(args, "--shot");
            if !args.is_empty() {
                return Err(format!(
                    "snapshot accepts only --window H [--depth N] [--max-nodes N] [--out PATH] [--shot]; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Snapshot {
                target,
                window,
                depth,
                max_nodes,
                out,
                shot,
            })
        }
        "diff" => {
            let Some(window) = flag_window(args)? else {
                return Err("diff requires --window <handle>".into());
            };
            let base = flag_text(args, "--base")?;
            let advance = take_switch(args, "--advance");
            let max = flag_parsed::<usize>(args, "--max")?;
            if !args.is_empty() {
                return Err(format!(
                    "diff accepts only --window H [--base ID] [--advance] [--max N]; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Diff {
                target,
                window,
                base,
                advance,
                max,
            })
        }
        "focused" => {
            let Some(window) = flag_window(args)? else {
                return Err("focused requires --window <handle>".into());
            };
            let role = flag_text(args, "--role")?;
            let max_value_bytes = flag_parsed::<usize>(args, "--max-value-bytes")?;
            if !args.is_empty() {
                return Err(format!(
                    "focused accepts only --window H --role R --max-value-bytes N; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Focused {
                target,
                window,
                role,
                max_value_bytes,
            })
        }
        "observe" => observe(target, args),
        "verify" => {
            let Some(window) = flag_window(args)? else {
                return Err("verify requires --window <handle>".into());
            };
            let expect = match flag_text(args, "--expect")? {
                Some(raw) => parse_expectations(&raw)?,
                None => return Err("verify requires --expect '<json array>'".into()),
            };
            if !args.is_empty() {
                return Err(format!(
                    "verify accepts only --window H --expect JSON; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Verify {
                target,
                window,
                expect,
            })
        }
        "menu-inspect" => menu::parse(target, args),
        "get-text" => {
            let window = flag_window_opt(args);
            let name = flag_value(args, "--name");
            let role = flag_value(args, "--role");
            if window.is_none() && name.as_ref().is_none_or(|value| value.is_empty()) {
                return Err("get-text requires --window <handle> [--name <pattern>]".into());
            }
            Ok(Command::GetText {
                target,
                window,
                name,
                role,
            })
        }
        "get-extents" => {
            let (window, name, role) = named_node(
                args,
                "get-extents requires --window <handle> --name <pattern>",
            )?;
            Ok(Command::GetExtents {
                target,
                window,
                name,
                role,
            })
        }
        "get-selection" => {
            let (window, name, role) = named_node(
                args,
                "get-selection requires --window <handle> --name <pattern>",
            )?;
            Ok(Command::GetSelection {
                target,
                window,
                name,
                role,
            })
        }
        "get-caret" => {
            let (window, name, role) = named_node(
                args,
                "get-caret requires --window <handle> --name <pattern>",
            )?;
            Ok(Command::GetCaret {
                target,
                window,
                name,
                role,
            })
        }
        "wait" => wait(target, args),
        "screenshot" => screenshot(target, args),
        "device-screenshot" => device_screenshot(target, args),
        "zoom" => {
            let Some(window) = flag_window(args)? else {
                return Err("zoom requires --window <handle>".into());
            };
            let Some(region) = flag_text(args, "--region")? else {
                return Err("zoom requires --region X,Y,W,H".into());
            };
            let region = parse_rect(&region)?;
            let Some(out) = flag_text(args, "--out")? else {
                return Err("zoom requires --out <PATH>".into());
            };
            let replace = take_switch(args, "--replace");
            let pad = flag_parsed::<u32>(args, "--pad")?;
            if !args.is_empty() {
                return Err(format!(
                    "zoom accepts only --window H --region X,Y,W,H --out PATH [--replace] [--pad N]; unexpected {:?}",
                    args[0]
                ));
            }
            Ok(Command::Zoom {
                target,
                window,
                region,
                out,
                replace,
                pad,
            })
        }
        "pointer-position" => {
            if !args.is_empty() {
                // `cursor` is the MCU spelling; its refusal names it.
                return Err(if spelled == "cursor" {
                    format!("cursor accepts no arguments; unexpected {:?}", args[0])
                } else {
                    "pointer-position accepts no command arguments".to_owned()
                });
            }
            Ok(Command::PointerPosition { target })
        }
        other => Err(format!("unknown command '{other}'")),
    }
}

/// `X,Y,W,H`, the same four-field spelling `query --within` takes. A
/// malformed rectangle is a usage error before anything is captured.
fn parse_rect(raw: &str) -> Result<[i32; 4], String> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return Err(format!(
            "--region must be X,Y,W,H (four comma-separated integers), got {raw:?}"
        ));
    }
    let mut rect = [0i32; 4];
    for (slot, part) in rect.iter_mut().zip(parts) {
        *slot = part
            .parse()
            .map_err(|_| format!("--region field {part:?} is not an integer"))?;
    }
    Ok(rect)
}

/// Closed CLI shape (mcu lesson): an unknown flag, a missing value, or a
/// stray positional fails here, before any tree is read. `verb` is the
/// spelling (`query`, `inspect`, `find`, `read`) because the MCU forms take
/// their needle / selector positionally.
fn query(target: TargetRef, verb: &str, args: &mut Vec<String>) -> Result<Command, String> {
    if verb == "inspect" && args.iter().any(|arg| arg == "--app") {
        return Err(
            "inspect --app is MCU window inventory; use mcu inspect --app, or query --window"
                .into(),
        );
    }
    let window = match parse_optional_window(args)? {
        Some(value) => value,
        None => {
            return Err(format!(
                "{verb} requires --window <handle> (MCU `{verb} HANDLE` is also accepted)"
            ));
        }
    };
    if verb == "find" && !args.iter().any(|arg| arg == "--text") {
        let Some(needle) = args
            .first()
            .cloned()
            .filter(|first| !first.starts_with('-'))
        else {
            return Err("find requires a text needle (MCU `find HANDLE TEXT`)".into());
        };
        args.remove(0);
        args.insert(0, needle);
        args.insert(0, "--text".into());
    }
    if verb == "read" && !args.iter().any(|arg| arg == "--selector") {
        let Some(selector) = args
            .first()
            .cloned()
            .filter(|first| !first.starts_with('-'))
        else {
            return Err("read requires a selector (MCU `read HANDLE SELECTOR`)".into());
        };
        args.remove(0);
        args.insert(0, selector);
        args.insert(0, "--selector".into());
    }
    let depth = flag_parsed::<u32>(args, "--depth")?;
    let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
    let role = flag_text(args, "--role")?
        .map(|raw| agenterm_cu::observe::parse_roles(&raw))
        .unwrap_or_default();
    let subrole = flag_text(args, "--subrole")?
        .map(|raw| agenterm_cu::observe::parse_roles(&raw))
        .unwrap_or_default();
    let action = flag_text(args, "--action")?
        .map(|raw| agenterm_cu::observe::parse_roles(&raw))
        .unwrap_or_default();
    let min_depth = flag_parsed::<u32>(args, "--min-depth")?;
    let max_depth = flag_parsed::<u32>(args, "--max-depth")?;
    let text = flag_text(args, "--text")?;
    let text_exact = flag_text(args, "--text-exact")?;
    if text.is_some() && text_exact.is_some() {
        return Err("query accepts --text or --text-exact, not both".into());
    }
    let identifier = flag_text(args, "--identifier")?;
    let actionable = take_switch(args, "--actionable");
    let explicit_bool =
        |args: &mut Vec<String>, flag: &'static str| -> Result<Option<bool>, String> {
            match flag_text(args, flag)?.as_deref() {
                None => Ok(None),
                Some("true") => Ok(Some(true)),
                Some("false") => Ok(Some(false)),
                Some(_) => Err(format!("query {flag} takes true or false")),
            }
        };
    let enabled = explicit_bool(args, "--enabled")?;
    let focused = explicit_bool(args, "--focused")?;
    let selected = explicit_bool(args, "--selected")?;
    let checked = explicit_bool(args, "--checked")?;
    let expanded = explicit_bool(args, "--expanded")?;
    let within = match flag_text(args, "--within")? {
        Some(raw) => Some(agenterm_cu::observe::parse_within(&raw)?),
        None => None,
    };
    let offset = flag_parsed::<usize>(args, "--offset")?;
    let max = flag_parsed::<usize>(args, "--max")?;
    let selector = flag_text(args, "--selector")?;
    if let Some(raw) = selector.as_deref() {
        agenterm_cu::observe::parse_selector(raw)?;
    }
    let watch_ms = flag_parsed::<u64>(args, "--watch-ms")?;
    let until = match flag_text(args, "--until")?.as_deref() {
        None => None,
        Some("present") => Some(QueryWatchUntil::Present),
        Some("absent") => Some(QueryWatchUntil::Absent),
        Some("change") => Some(QueryWatchUntil::Change),
        Some(value) => {
            return Err(format!(
                "query --until must be present|absent|change, got {value:?}"
            ));
        }
    };
    let interval_ms = flag_parsed::<u64>(args, "--interval-ms")?;
    let max_events = flag_parsed::<usize>(args, "--max-events")?;
    if !args.is_empty() {
        return Err(format!(
            "{verb} accepts only --window H --depth N --max-nodes N --role R,R --subrole S,S \
             --action A,A --min-depth N --max-depth N \
             --text T | --text-exact T --identifier ID --actionable \
             --enabled true|false --focused true|false --selected true|false \
             --checked true|false --expanded true|false \
             --within X,Y,W,H --offset N --max N --selector PATH --watch-ms N \
             --until present|absent|change --interval-ms N --max-events N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::Query {
        target,
        window,
        depth,
        max_nodes,
        role,
        subrole,
        action,
        min_depth,
        max_depth,
        text,
        text_exact,
        identifier,
        actionable,
        enabled,
        focused,
        selected,
        checked,
        expanded,
        within,
        offset,
        max,
        selector,
        watch_ms,
        until,
        interval_ms,
        max_events,
    })
}

fn observe(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let Some(window) = flag_window(args)? else {
        return Err("observe requires --window <handle>".into());
    };
    // `--duration` is seconds (fractions allowed); `--duration-ms` is exact.
    let seconds = flag_parsed::<f64>(args, "--duration")?;
    let millis = flag_parsed::<u64>(args, "--duration-ms")?;
    let duration_ms = match (seconds, millis) {
        (Some(_), Some(_)) => {
            return Err("observe accepts --duration or --duration-ms, not both".into());
        }
        (Some(seconds), None) => {
            if !seconds.is_finite() || seconds <= 0.0 || seconds > 120.0 {
                return Err("observe --duration must be within (0, 120] seconds".into());
            }
            (seconds * 1000.0).round() as u64
        }
        (None, Some(millis)) => millis,
        (None, None) => {
            return Err("observe requires --duration S (or --duration-ms N)".into());
        }
    };
    let depth = flag_parsed::<u32>(args, "--depth")?;
    let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
    let max_events = flag_parsed::<usize>(args, "--max-events")?;
    let notifications = match flag_text(args, "--notification")? {
        Some(raw) => agenterm_cu::observe::parse_notifications(&raw)?,
        None => Vec::new(),
    };
    let interval_ms = flag_parsed::<u64>(args, "--interval-ms")?;
    let mode = flag_text(args, "--mode")?;
    let ready_path = flag_text(args, "--ready-path")?;
    if let Some(mode) = &mode
        && mode != "poll-diff"
        && mode != "notifications"
    {
        return Err("observe --mode must be poll-diff or notifications".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "observe accepts only --window H --duration S | --duration-ms N --depth N --max-nodes N \
             --max-events N --notification A,B --interval-ms N --mode poll-diff|notifications \
             --ready-path PATH; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::Observe {
        target,
        window,
        duration_ms,
        ready_path,
        depth,
        max_nodes,
        max_events,
        notifications,
        interval_ms,
        mode,
    })
}

fn wait(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    // `--` ends flag parsing so --text-equals / --text-contains may start with a dash.
    let literal_text = split_literal_tail(args, " ");
    let expect_present = args.iter().any(|arg| arg == "--expect");
    let absent = take_switch(args, "--absent");
    if absent && !expect_present {
        return Err("wait --absent requires --expect JSON".into());
    }
    // `--expect` is a closed shape, so its timeout value is consumed (the
    // older conditions' lenient `flag_u64` leaves it in place).
    let timeout_ms = if expect_present {
        flag_parsed::<u64>(args, "--timeout-ms")?.unwrap_or(5_000)
    } else {
        flag_u64(args, "--timeout-ms").unwrap_or(5_000)
    };
    let text_equals_present = args
        .iter()
        .any(|arg| arg == "--text-equals" || arg == "--node-text-equals");
    let text_contains_present = args
        .iter()
        .any(|arg| arg == "--text-contains" || arg == "--node-text-contains");
    let condition = if text_equals_present && text_contains_present {
        return Err("wait accepts one of --text-equals or --text-contains, not both".into());
    } else if expect_present {
        let expect = match flag_text(args, "--expect")? {
            Some(raw) => parse_expectations(&raw)?,
            None => return Err("wait --expect requires a JSON array".into()),
        };
        let Some(window) = flag_window(args)? else {
            return Err("wait --expect requires --window <handle>".into());
        };
        if !args.is_empty() {
            return Err(format!(
                "wait --expect accepts only --timeout-ms MS --window H --expect JSON [--absent]; unexpected {:?}",
                args[0]
            ));
        }
        WaitCondition::Expect {
            window,
            expect,
            absent,
        }
    } else if text_equals_present {
        let expected = flag_value(args, "--text-equals")
            .or_else(|| flag_value(args, "--node-text-equals"))
            .filter(|value| value != "--")
            .or(literal_text);
        let Some(expected) = expected else {
            return Err(
                "wait --text-equals / --node-text-equals requires the expected text".into(),
            );
        };
        let name = flag_value(args, "--name")
            .or_else(|| flag_value(args, "--node-name-contains"))
            .filter(|value| !value.is_empty());
        let Some(name) = name else {
            return Err("wait --text-equals requires --name <pattern>".into());
        };
        WaitCondition::NodeTextEquals {
            expected,
            name,
            role: flag_value(args, "--role").or_else(|| flag_value(args, "--node-role")),
            window: flag_window_opt(args),
        }
    } else if text_contains_present {
        let substring = flag_value(args, "--text-contains")
            .or_else(|| flag_value(args, "--node-text-contains"))
            .filter(|value| value != "--")
            .or(literal_text);
        let Some(substring) = substring else {
            return Err(
                "wait --text-contains / --node-text-contains requires the substring".into(),
            );
        };
        let name = flag_value(args, "--name")
            .or_else(|| flag_value(args, "--node-name-contains"))
            .filter(|value| !value.is_empty());
        let Some(name) = name else {
            return Err("wait --text-contains requires --name <pattern>".into());
        };
        WaitCondition::NodeTextContains {
            substring,
            name,
            role: flag_value(args, "--role").or_else(|| flag_value(args, "--node-role")),
            window: flag_window_opt(args),
        }
    } else if let Some(count) = flag_usize(args, "--window-count-gte") {
        WaitCondition::WindowCountGte { count }
    } else if let Some(pattern) = flag_value(args, "--window-title-contains") {
        WaitCondition::WindowTitleContains { pattern }
    } else if let Some(handle) = flag_isize(args, "--focused-handle") {
        WaitCondition::FocusedHandle { handle }
    } else if let Some(pattern) = flag_value(args, "--node-name-contains") {
        WaitCondition::NodeNameContains {
            pattern,
            role: flag_value(args, "--node-role"),
            window: flag_window_opt(args),
        }
    } else {
        return Err(
            "wait requires one of --window-count-gte, --window-title-contains, --focused-handle, --node-name-contains, --text-equals, or --text-contains".into(),
        );
    };
    Ok(Command::Wait {
        target,
        timeout_ms,
        condition,
    })
}

fn screenshot(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    // Closed flags: `--window` is never a positional path. The old parser
    // treated argv[0] as `--out`, so `screenshot --window 16784` stored
    // path="--window" and then failed "handle must be non-zero".
    let window = flag_window(args)?;
    let path = flag_text(args, "--out")?;
    let path = path.or_else(|| {
        args.first()
            .cloned()
            .filter(|first| !first.starts_with('-'))
            .inspect(|_| {
                args.remove(0);
            })
    });
    if !args.is_empty() {
        return Err(format!(
            "screenshot accepts --out PATH --window HANDLE; unexpected {:?}",
            args[0]
        ));
    }
    let path = path.unwrap_or_else(|| {
        std::env::temp_dir()
            .join(format!("agenterm-cu-{}.png", std::process::id()))
            .to_string_lossy()
            .into_owned()
    });
    Ok(Command::Screenshot {
        target,
        path,
        window,
    })
}

fn device_screenshot(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let list = take_switch(args, "--list");
    let path = flag_text(args, "--out")?;
    let device = flag_text(args, "--device")?;
    let timeout_ms = flag_parsed::<u64>(args, "--timeout-ms")?;
    if !args.is_empty() {
        return Err(format!(
            "device-screenshot accepts only --list or --out PATH [--device NAME|UID] [--timeout-ms N]; unexpected {:?}",
            args[0]
        ));
    }
    if list && (path.is_some() || device.is_some() || timeout_ms.is_some()) {
        return Err("device-screenshot --list cannot be combined with capture flags".into());
    }
    if !list && path.as_deref().is_none_or(str::is_empty) {
        return Err("device-screenshot requires --out PATH (or --list)".into());
    }
    Ok(Command::DeviceScreenshot {
        target,
        path,
        device,
        timeout_ms,
        list,
    })
}

#[cfg(test)]
mod tests {
    use super::super::verbs;
    use super::*;

    #[test]
    fn device_screenshot_keeps_inventory_and_capture_shapes_disjoint() {
        let spec = verbs::lookup("device-screenshot").expect("device verb");
        let mut list = vec!["--list".into()];
        assert!(matches!(
            parse(spec, "device-screenshot", TargetRef::Current, &mut list).expect("inventory"),
            Command::DeviceScreenshot { list: true, .. }
        ));

        let mut capture = vec![
            "--out".into(),
            "shot.png".into(),
            "--device".into(),
            "fixture-device".into(),
            "--timeout-ms".into(),
            "2500".into(),
        ];
        assert!(matches!(
            parse(
                spec,
                "device-screenshot",
                TargetRef::Current,
                &mut capture
            )
            .expect("capture"),
            Command::DeviceScreenshot {
                path: Some(path),
                device: Some(device),
                timeout_ms: Some(2500),
                list: false,
                ..
            } if path == "shot.png" && device == "fixture-device"
        ));

        for mut invalid in [
            vec!["--list".into(), "--out".into(), "shot.png".into()],
            Vec::new(),
            vec!["--unknown".into()],
        ] {
            assert!(parse(spec, "device-screenshot", TargetRef::Current, &mut invalid).is_err());
        }
    }

    #[test]
    fn desktop_state_accepts_the_mcu_state_alias_with_closed_flags() {
        let spec = verbs::lookup("state").expect("state alias");
        let mut args = vec![
            "--window".into(),
            "Fixture#7".into(),
            "--depth".into(),
            "2".into(),
        ];
        assert!(matches!(
            parse(spec, "state", TargetRef::Current, &mut args).unwrap(),
            Command::DesktopState {
                window: Some(7),
                depth: Some(2),
                max_nodes: None,
                ..
            }
        ));
        let mut stray = vec!["--unknown".into()];
        assert!(parse(spec, "state", TargetRef::Current, &mut stray).is_err());
    }

    #[test]
    fn query_watch_parses_only_the_closed_bounded_shape() {
        let spec = verbs::lookup("query").expect("query verb");
        let mut args = vec![
            "--window".into(),
            "Fixture#7".into(),
            "--role".into(),
            "button".into(),
            "--subrole".into(),
            "AXDialog".into(),
            "--action".into(),
            "Press,Focus".into(),
            "--min-depth".into(),
            "1".into(),
            "--max-depth".into(),
            "4".into(),
            "--enabled".into(),
            "false".into(),
            "--checked".into(),
            "true".into(),
            "--watch-ms".into(),
            "1500".into(),
            "--until".into(),
            "change".into(),
            "--interval-ms".into(),
            "100".into(),
            "--max-events".into(),
            "12".into(),
        ];
        assert!(matches!(
            parse(spec, "query", TargetRef::Current, &mut args).expect("query watch"),
            Command::Query {
                window: 7,
                ref subrole,
                ref action,
                min_depth: Some(1),
                max_depth: Some(4),
                enabled: Some(false),
                checked: Some(true),
                watch_ms: Some(1500),
                until: Some(QueryWatchUntil::Change),
                interval_ms: Some(100),
                max_events: Some(12),
                ..
            } if subrole == &["AXDialog"] && action == &["Press", "Focus"]
        ));
        let mut invalid = vec![
            "--window".into(),
            "7".into(),
            "--until".into(),
            "forever".into(),
        ];
        assert!(parse(spec, "query", TargetRef::Current, &mut invalid).is_err());
        let mut invalid_bool = vec![
            "--window".into(),
            "7".into(),
            "--focused".into(),
            "unknown".into(),
        ];
        assert!(parse(spec, "query", TargetRef::Current, &mut invalid_bool).is_err());
    }

    #[test]
    fn wait_absent_is_closed_and_requires_expect() {
        let spec = verbs::lookup("wait").expect("wait verb");
        let mut args = vec![
            "--window".into(),
            "Fixture#7".into(),
            "--expect".into(),
            r#"[{"identifier":"gone","name":"Gone"}]"#.into(),
            "--absent".into(),
            "--timeout-ms".into(),
            "25".into(),
        ];
        assert!(matches!(
            parse(spec, "wait", TargetRef::Current, &mut args).unwrap(),
            Command::Wait {
                timeout_ms: 25,
                condition: WaitCondition::Expect {
                    window: 7,
                    absent: true,
                    ..
                },
                ..
            }
        ));

        let mut absent_only = vec!["--absent".into(), "--window-count-gte".into(), "1".into()];
        assert_eq!(
            parse(spec, "wait", TargetRef::Current, &mut absent_only).unwrap_err(),
            "wait --absent requires --expect JSON"
        );
    }
}
