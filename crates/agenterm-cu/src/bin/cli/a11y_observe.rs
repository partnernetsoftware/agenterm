//! Accessibility observation: tree / query / focused / observe / verify, the
//! independent AT-SPI read-backs, `wait`, and the two non-tree observers
//! (`screenshot`, `pointer-position`).

use agenterm_cu::{Command, TargetRef, WaitCondition};

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
            let window = flag_window_opt(args);
            let depth = flag_parsed::<u32>(args, "--depth")?;
            let max_nodes = flag_parsed::<usize>(args, "--max-nodes")?;
            // `elements` is the MCU spelling of `tree --flat`.
            let flat = spelled == "elements" || take_switch(args, "--flat");
            Ok(Command::Tree {
                target,
                window,
                depth,
                max_nodes,
                flat,
            })
        }
        "query" => query(target, spelled, args),
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
    let text = flag_text(args, "--text")?;
    let text_exact = flag_text(args, "--text-exact")?;
    if text.is_some() && text_exact.is_some() {
        return Err("query accepts --text or --text-exact, not both".into());
    }
    let identifier = flag_text(args, "--identifier")?;
    let actionable = take_switch(args, "--actionable");
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
    if !args.is_empty() {
        return Err(format!(
            "{verb} accepts only --window H --depth N --max-nodes N --role R,R \
             --text T | --text-exact T --identifier ID --actionable \
             --within X,Y,W,H --offset N --max N --selector PATH; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::Query {
        target,
        window,
        depth,
        max_nodes,
        role,
        text,
        text_exact,
        identifier,
        actionable,
        within,
        offset,
        max,
        selector,
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
    if let Some(mode) = &mode
        && mode != "poll-diff"
        && mode != "notifications"
    {
        return Err("observe --mode must be poll-diff or notifications".into());
    }
    if !args.is_empty() {
        return Err(format!(
            "observe accepts only --window H --duration S | --duration-ms N --depth N --max-nodes N \
             --max-events N --notification A,B --interval-ms N --mode poll-diff|notifications; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::Observe {
        target,
        window,
        duration_ms,
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
                "wait --expect accepts only --timeout-ms MS --window H --expect JSON; unexpected {:?}",
                args[0]
            ));
        }
        WaitCondition::Expect { window, expect }
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
