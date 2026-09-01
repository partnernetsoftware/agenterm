//! `agenterm-cu` shell command (PRD_02_29 shell layer).
//!
//! Machine-readable JSON on stdout; human usage on stderr.

use std::path::PathBuf;

use agenterm_cu::{
    Authorization, Command, Executor, PointerButton, RdpEndpoint, SshEndpoint, TargetRef,
    VncEndpoint, WaitCondition,
    command::{Expectation, InvokeAction},
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.as_slice(), [arg] if arg == "--version" || arg == "-V") {
        println!("agenterm-cu {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if matches!(args.first().map(String::as_str), Some("host" | "hotkeys")) {
        std::process::exit(agenterm_cu::hotkeys::run());
    }
    if args.first().map(String::as_str)
        == Some(agenterm_cu::mechanism::clipboard::X11_CLIPBOARD_OWNER_ARG)
    {
        std::process::exit(run_x11_clipboard_owner());
    }
    let reply = dispatch(args);
    let json = serde_json::to_string(&reply).unwrap_or_else(|_| {
        r#"{"ok":false,"target":"","command":"","error":{"code":"serialize","message":"reply serialization failed"}}"#
            .to_string()
    });
    println!("{json}");
}

fn dispatch(mut args: Vec<String>) -> agenterm_cu::CuReply {
    let (ambient_authority_present, unsupported_authority_environment) =
        authority_environment_flags();
    if let Some(reply) = agenterm_cu::grant_management::dispatch(&args, ambient_authority_present) {
        return reply;
    }
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        eprint_usage();
        return help_reply(true);
    }

    let mut grant: Option<String> = None;
    let mut grant_id: Option<String> = None;
    let mut grant_store: Option<PathBuf> = None;
    let mut target: Option<TargetRef> = None;
    let mut ssh_dest: Option<String> = None;
    let mut ssh_port: Option<u16> = None;
    let mut ssh_identity: Option<PathBuf> = None;
    let mut ssh_cu: Option<PathBuf> = None;
    let mut ssh_env: Vec<(String, String)> = Vec::new();
    let mut vnc_dest: Option<String> = None;
    let mut vnc_port: Option<u16> = None;
    let mut vnc_cu: Option<PathBuf> = None;
    let mut vnc_env: Vec<(String, String)> = Vec::new();
    let mut rdp_dest: Option<String> = None;
    while let Some(flag) = args.first() {
        match flag.as_str() {
            "--target" => {
                let value = take_value(&mut args, "--target");
                target = TargetRef::parse(&value).or_else(|| {
                    eprint_usage();
                    None
                });
                if target.is_none() {
                    return usage_err(
                        "unknown --target value; supported: 'current', 'ssh', 'vnc', and 'rdp'",
                    );
                }
            }
            "--ssh" => {
                let value = take_value(&mut args, "--ssh");
                if value.is_empty() {
                    return usage_err("--ssh requires <user@host>");
                }
                ssh_dest = Some(value);
                if target.is_none() {
                    target = Some(TargetRef::Ssh);
                }
            }
            "--ssh-port" => {
                let value = take_value(&mut args, "--ssh-port");
                match value.parse::<u16>() {
                    Ok(port) => ssh_port = Some(port),
                    Err(_) => return usage_err("--ssh-port requires a TCP port number"),
                }
            }
            "--ssh-identity" => {
                let value = take_value(&mut args, "--ssh-identity");
                if value.is_empty() {
                    return usage_err("--ssh-identity requires a private-key path");
                }
                ssh_identity = Some(PathBuf::from(value));
            }
            "--ssh-cu" => {
                let value = take_value(&mut args, "--ssh-cu");
                if value.is_empty() {
                    return usage_err("--ssh-cu requires a remote agenterm-cu path");
                }
                ssh_cu = Some(PathBuf::from(value));
            }
            "--ssh-env" => {
                let value = take_value(&mut args, "--ssh-env");
                let Some((key, val)) = value.split_once('=') else {
                    return usage_err("--ssh-env requires KEY=VAL");
                };
                if key.is_empty() {
                    return usage_err("--ssh-env requires a non-empty KEY");
                }
                ssh_env.push((key.to_owned(), val.to_owned()));
            }
            "--vnc" => {
                let value = take_value(&mut args, "--vnc");
                if value.is_empty() {
                    return usage_err("--vnc requires <host[:port]>");
                }
                vnc_dest = Some(value);
                if target.is_none() {
                    target = Some(TargetRef::Vnc);
                }
            }
            "--vnc-port" => {
                let value = take_value(&mut args, "--vnc-port");
                match value.parse::<u16>() {
                    Ok(port) => vnc_port = Some(port),
                    Err(_) => return usage_err("--vnc-port requires a TCP port number"),
                }
            }
            "--vnc-cu" => {
                let value = take_value(&mut args, "--vnc-cu");
                if value.is_empty() {
                    return usage_err(
                        "--vnc-cu requires an agenterm-cu path for the session worker",
                    );
                }
                vnc_cu = Some(PathBuf::from(value));
            }
            "--vnc-env" => {
                let value = take_value(&mut args, "--vnc-env");
                let Some((key, val)) = value.split_once('=') else {
                    return usage_err("--vnc-env requires KEY=VAL");
                };
                if key.is_empty() {
                    return usage_err("--vnc-env requires a non-empty KEY");
                }
                vnc_env.push((key.to_owned(), val.to_owned()));
            }
            "--rdp" => {
                let value = take_value(&mut args, "--rdp");
                if value.is_empty() {
                    return usage_err("--rdp requires <host[:port]>");
                }
                rdp_dest = Some(value);
                if target.is_none() {
                    target = Some(TargetRef::Rdp);
                }
            }
            "--grant" => {
                if grant.is_some() {
                    return usage_err("duplicate --grant");
                }
                grant = Some(take_value(&mut args, "--grant"));
            }
            "--grant-id" => {
                if grant_id.is_some() {
                    return usage_err("duplicate --grant-id");
                }
                let value = take_value(&mut args, "--grant-id");
                if !agenterm_cu::grant_management::valid_grant_id(&value) {
                    return usage_err("--grant-id is invalid");
                }
                grant_id = Some(value);
            }
            "--grant-store" => {
                if grant_store.is_some() {
                    return usage_err("duplicate --grant-store");
                }
                let value = take_value(&mut args, "--grant-store");
                if value.is_empty() {
                    return usage_err("--grant-store requires a path");
                }
                grant_store = Some(PathBuf::from(value));
            }
            _ if flag.starts_with('-') => {
                return usage_err(format!("unknown global flag '{flag}'"));
            }
            _ => break,
        }
    }

    // Allow global flags before `exec` so remote workers can be invoked as
    // `agenterm-cu --grant observe exec --json -` as well as `exec` first.
    if args.first().map(String::as_str) == Some("exec") {
        let mut exec_args: Vec<String> = Vec::new();
        if let Some(raw) = grant.as_ref() {
            exec_args.push(format!("--grant={raw}"));
        }
        if let Some(id) = grant_id.as_ref() {
            exec_args.push(format!("--grant-id={id}"));
        }
        if let Some(path) = grant_store.as_ref() {
            exec_args.push(format!("--grant-store={}", path.display()));
        }
        exec_args.extend(args.into_iter().skip(1));
        return dispatch_json(&exec_args);
    }

    if target.is_none()
        && let Ok(dest) = std::env::var("AGENTERM_CU_SSH")
        && !dest.is_empty()
    {
        ssh_dest = Some(dest);
        target = Some(TargetRef::Ssh);
    }
    if target.is_none()
        && let Ok(dest) = std::env::var("AGENTERM_CU_VNC")
        && !dest.is_empty()
    {
        vnc_dest = Some(dest);
        target = Some(TargetRef::Vnc);
    }

    let Some(target) = target else {
        eprint_usage();
        return usage_err(
            "--target current, --ssh <user@host>, --vnc <host[:port]>, or --rdp <host[:port]> is required on every command",
        );
    };

    if target == TargetRef::Current && ssh_dest.is_some() {
        return usage_err("--ssh cannot be combined with --target current");
    }
    if target == TargetRef::Current && vnc_dest.is_some() {
        return usage_err("--vnc cannot be combined with --target current");
    }
    if target == TargetRef::Current && rdp_dest.is_some() {
        return usage_err("--rdp cannot be combined with --target current");
    }
    if target == TargetRef::Ssh && vnc_dest.is_some() {
        return usage_err("--vnc cannot be combined with --target ssh / --ssh");
    }
    if target == TargetRef::Ssh && rdp_dest.is_some() {
        return usage_err("--rdp cannot be combined with --target ssh / --ssh");
    }
    if target == TargetRef::Vnc && ssh_dest.is_some() {
        return usage_err("--ssh cannot be combined with --target vnc / --vnc");
    }
    if target == TargetRef::Vnc && rdp_dest.is_some() {
        return usage_err("--rdp cannot be combined with --target vnc / --vnc");
    }
    if target == TargetRef::Rdp && ssh_dest.is_some() {
        return usage_err("--ssh cannot be combined with --target rdp / --rdp");
    }
    if target == TargetRef::Rdp && vnc_dest.is_some() {
        return usage_err("--vnc cannot be combined with --target rdp / --rdp");
    }
    if target == TargetRef::Ssh
        && ssh_dest.is_none()
        && let Ok(dest) = std::env::var("AGENTERM_CU_SSH")
        && !dest.is_empty()
    {
        ssh_dest = Some(dest);
    }
    if target == TargetRef::Ssh && ssh_dest.is_none() {
        return usage_err("ssh target requires --ssh <user@host> (or AGENTERM_CU_SSH)");
    }
    if target == TargetRef::Vnc
        && vnc_dest.is_none()
        && let Ok(dest) = std::env::var("AGENTERM_CU_VNC")
        && !dest.is_empty()
    {
        vnc_dest = Some(dest);
    }
    if target == TargetRef::Vnc && vnc_dest.is_none() {
        return usage_err("vnc target requires --vnc <host[:port]> (or AGENTERM_CU_VNC)");
    }
    // `--target rdp` without `--rdp` is not a usage error: the executor returns
    // typed `rdp_unavailable` with command/target preserved (cut 3.46).

    let Some(verb) = args.first().cloned() else {
        eprint_usage();
        return usage_err("missing command verb");
    };
    args.remove(0);

    let command = match verb.as_str() {
        "capabilities" | "caps" => Command::Capabilities { target },
        "windows" => {
            let pid = match flag_parsed::<u32>(&mut args, "--pid") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let app = match flag_text(&mut args, "--app") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let title = match flag_text(&mut args, "--title") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let focused = flag_tristate(&mut args, "--focused");
            let minimized = flag_tristate(&mut args, "--minimized");
            let offset = match flag_parsed::<usize>(&mut args, "--offset") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let max = match flag_parsed::<usize>(&mut args, "--max") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err(format!(
                    "windows accepts only --pid N --app SUB --title SUB --focused [BOOL] \
                     --minimized [BOOL] --offset N --max N; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Windows {
                target,
                pid,
                app,
                title,
                focused,
                minimized,
                offset,
                max,
            }
        }
        "windows-watch" => {
            let pid = match flag_parsed::<u32>(&mut args, "--pid") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let app = match flag_text(&mut args, "--app") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let title = match flag_text(&mut args, "--title") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let duration_ms = match flag_parsed::<u64>(&mut args, "--duration-ms") {
                Ok(value) => value.unwrap_or(0),
                Err(message) => return usage_err(message),
            };
            let interval_ms = match flag_parsed::<u64>(&mut args, "--interval-ms") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let max_events = match flag_parsed::<usize>(&mut args, "--max-events") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err(format!(
                    "windows-watch accepts only --pid N --app SUB --title SUB \
                     --duration-ms N --interval-ms N --max-events N; unexpected {:?}",
                    args[0]
                ));
            }
            Command::WindowsWatch {
                target,
                pid,
                app,
                title,
                duration_ms,
                interval_ms,
                max_events,
            }
        }
        "apps" => {
            let running = take_switch(&mut args, "--running");
            let all = take_switch(&mut args, "--all");
            if !args.is_empty() {
                return usage_err(format!(
                    "apps accepts only --running / --all; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Apps {
                target,
                running,
                all,
            }
        }
        "tree" | "elements" => {
            let window = flag_window_opt(&mut args);
            let depth = match flag_parsed::<u32>(&mut args, "--depth") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let max_nodes = match flag_parsed::<usize>(&mut args, "--max-nodes") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let flat = verb == "elements" || take_switch(&mut args, "--flat");
            Command::Tree {
                target,
                window,
                depth,
                max_nodes,
                flat,
            }
        }
        "query" | "inspect" | "find" | "read" => match parse_query(target, &verb, &mut args) {
            Ok(command) => command,
            Err(message) => return usage_err(message),
        },
        "invoke" => {
            // Closed shape: flags first, then exactly `<action> [VALUE]`.
            let window = match flag_window(&mut args) {
                Ok(Some(value)) => value,
                Ok(None) => return usage_err("invoke requires --window <handle>"),
                Err(message) => return usage_err(message),
            };
            let node = match flag_text(&mut args, "--node") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let index = match flag_parsed::<usize>(&mut args, "--index") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let name = match flag_text(&mut args, "--name") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let identifier = match flag_text(&mut args, "--identifier") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let role = match flag_text(&mut args, "--role") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let focused = take_switch(&mut args, "--focused");
            let selector = match flag_text(&mut args, "--selector") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            if let Some(raw) = selector.as_deref()
                && let Err(message) = agenterm_cu::observe::parse_selector(raw)
            {
                return usage_err(message);
            }
            if node.is_none()
                && index.is_none()
                && name.is_none()
                && identifier.is_none()
                && !focused
                && selector.is_none()
            {
                return usage_err(
                    "invoke requires one of --node PATH, --index N, --name PAT [--role ROLE], --identifier ID, --focused [--role ROLE], --selector PATH",
                );
            }
            if focused
                && (node.is_some()
                    || index.is_some()
                    || name.is_some()
                    || identifier.is_some()
                    || selector.is_some())
            {
                return usage_err(
                    "invoke --focused addresses the focused control; combine it only with --role",
                );
            }
            if selector.is_some()
                && (node.is_some() || index.is_some() || name.is_some() || identifier.is_some())
            {
                return usage_err(
                    "invoke --selector cannot mix with --node/--index/--name/--identifier",
                );
            }
            if let Some(stray) = args.iter().find(|arg| arg.starts_with("--")) {
                return usage_err(format!(
                    "invoke accepts only --window H --node PATH | --index N | --name PAT [--role ROLE] | --identifier ID | --focused [--role ROLE], then <action> [VALUE]; unexpected {stray:?}"
                ));
            }
            let Some(action_raw) = args.first().cloned() else {
                return usage_err(format!(
                    "invoke requires an action: {}",
                    InvokeAction::ALL
                        .iter()
                        .map(|action| action.as_str())
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            };
            let Some(action) = InvokeAction::parse(&action_raw) else {
                return usage_err(format!(
                    "unknown invoke action {action_raw:?}; expected one of {}",
                    InvokeAction::ALL
                        .iter()
                        .map(|action| action.as_str())
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            };
            let value = args.get(1).cloned();
            if args.len() > 2 {
                return usage_err(format!(
                    "invoke takes at most one VALUE after the action; unexpected {:?}",
                    args[2]
                ));
            }
            Command::Invoke {
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
            }
        }
        "menu" => {
            // `menu inspect` / `menu invoke`: closed shapes, background only.
            let Some(sub) = args.first().cloned() else {
                return usage_err("menu requires a subcommand: inspect | invoke");
            };
            args.remove(0);
            let window = match flag_window(&mut args) {
                Ok(Some(value)) => value,
                Ok(None) => return usage_err(format!("menu {sub} requires --window <handle>")),
                Err(message) => return usage_err(message),
            };
            match sub.as_str() {
                "inspect" => {
                    let depth = match flag_parsed::<u32>(&mut args, "--depth") {
                        Ok(value) => value,
                        Err(message) => return usage_err(message),
                    };
                    let max_nodes = match flag_parsed::<usize>(&mut args, "--max-nodes") {
                        Ok(value) => value,
                        Err(message) => return usage_err(message),
                    };
                    let title = match flag_text(&mut args, "--title") {
                        Ok(value) => value,
                        Err(message) => return usage_err(message),
                    };
                    let exact = take_switch(&mut args, "--exact");
                    if exact && title.is_none() {
                        return usage_err("menu inspect --exact requires --title");
                    }
                    let enabled = match flag_text(&mut args, "--enabled") {
                        Ok(Some(raw)) => match raw.as_str() {
                            "true" => Some(true),
                            "false" => Some(false),
                            _ => return usage_err("menu inspect --enabled takes true or false"),
                        },
                        Ok(None) => None,
                        Err(message) => return usage_err(message),
                    };
                    let offset = match flag_parsed::<usize>(&mut args, "--offset") {
                        Ok(value) => value,
                        Err(message) => return usage_err(message),
                    };
                    let max = match flag_parsed::<usize>(&mut args, "--max") {
                        Ok(value) => value,
                        Err(message) => return usage_err(message),
                    };
                    if !args.is_empty() {
                        return usage_err(format!(
                            "menu inspect accepts only --window H --depth N --max-nodes N --title T [--exact] \
                             --enabled true|false --offset N --max N; unexpected {:?}",
                            args[0]
                        ));
                    }
                    Command::MenuInspect {
                        target,
                        window,
                        depth,
                        max_nodes,
                        title,
                        exact,
                        enabled,
                        offset,
                        max,
                    }
                }
                "invoke" => {
                    let path = match flag_text(&mut args, "--path") {
                        Ok(Some(raw)) => match agenterm_cu::observe::parse_menu_path(&raw) {
                            Ok(path) => path,
                            Err(message) => return usage_err(message),
                        },
                        Ok(None) => {
                            return usage_err(
                                "menu invoke requires --path 'Menu/Item' (or a JSON array of titles)",
                            );
                        }
                        Err(message) => return usage_err(message),
                    };
                    if !args.is_empty() {
                        return usage_err(format!(
                            "menu invoke accepts only --window H --path PATH; unexpected {:?}",
                            args[0]
                        ));
                    }
                    Command::MenuInvoke {
                        target,
                        window,
                        path,
                    }
                }
                other => {
                    return usage_err(format!(
                        "unknown menu subcommand {other:?}; expected inspect | invoke"
                    ));
                }
            }
        }
        "focused" => {
            let window = match flag_window(&mut args) {
                Ok(Some(value)) => value,
                Ok(None) => return usage_err("focused requires --window <handle>"),
                Err(message) => return usage_err(message),
            };
            let role = match flag_text(&mut args, "--role") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let max_value_bytes = match flag_parsed::<usize>(&mut args, "--max-value-bytes") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err(format!(
                    "focused accepts only --window H --role R --max-value-bytes N; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Focused {
                target,
                window,
                role,
                max_value_bytes,
            }
        }
        "observe" => {
            let window = match flag_window(&mut args) {
                Ok(Some(value)) => value,
                Ok(None) => return usage_err("observe requires --window <handle>"),
                Err(message) => return usage_err(message),
            };
            // `--duration` is seconds (fractions allowed); `--duration-ms` is exact.
            let seconds = match flag_parsed::<f64>(&mut args, "--duration") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let millis = match flag_parsed::<u64>(&mut args, "--duration-ms") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let duration_ms = match (seconds, millis) {
                (Some(_), Some(_)) => {
                    return usage_err("observe accepts --duration or --duration-ms, not both");
                }
                (Some(seconds), None) => {
                    if !seconds.is_finite() || seconds <= 0.0 || seconds > 120.0 {
                        return usage_err("observe --duration must be within (0, 120] seconds");
                    }
                    (seconds * 1000.0).round() as u64
                }
                (None, Some(millis)) => millis,
                (None, None) => {
                    return usage_err("observe requires --duration S (or --duration-ms N)");
                }
            };
            let depth = match flag_parsed::<u32>(&mut args, "--depth") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let max_nodes = match flag_parsed::<usize>(&mut args, "--max-nodes") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let max_events = match flag_parsed::<usize>(&mut args, "--max-events") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let notifications = match flag_text(&mut args, "--notification") {
                Ok(Some(raw)) => match agenterm_cu::observe::parse_notifications(&raw) {
                    Ok(list) => list,
                    Err(message) => return usage_err(message),
                },
                Ok(None) => Vec::new(),
                Err(message) => return usage_err(message),
            };
            let interval_ms = match flag_parsed::<u64>(&mut args, "--interval-ms") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let mode = match flag_text(&mut args, "--mode") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            if let Some(mode) = &mode
                && mode != "poll-diff"
                && mode != "notifications"
            {
                return usage_err("observe --mode must be poll-diff or notifications");
            }
            if !args.is_empty() {
                return usage_err(format!(
                    "observe accepts only --window H --duration S | --duration-ms N --depth N --max-nodes N \
                     --max-events N --notification A,B --interval-ms N --mode poll-diff|notifications; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Observe {
                target,
                window,
                duration_ms,
                depth,
                max_nodes,
                max_events,
                notifications,
                interval_ms,
                mode,
            }
        }
        "verify" => {
            let window = match flag_window(&mut args) {
                Ok(Some(value)) => value,
                Ok(None) => return usage_err("verify requires --window <handle>"),
                Err(message) => return usage_err(message),
            };
            let expect = match flag_text(&mut args, "--expect") {
                Ok(Some(raw)) => match parse_expectations(&raw) {
                    Ok(expect) => expect,
                    Err(message) => return usage_err(message),
                },
                Ok(None) => return usage_err("verify requires --expect '<json array>'"),
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err(format!(
                    "verify accepts only --window H --expect JSON; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Verify {
                target,
                window,
                expect,
            }
        }
        "screenshot" | "shot" => match parse_screenshot(target, &mut args) {
            Ok(command) => command,
            Err(message) => return usage_err(message),
        },
        "pointer-move" | "move" => {
            let to = match flag_text(&mut args, "--to") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let Some(to) = to else {
                return usage_err(
                    "pointer-move requires --to desktop (explicit global) or --to <handle>",
                );
            };
            if to != "desktop" && agenterm_cu::observe::parse_window_token(&to).is_err() {
                return usage_err(
                    "pointer-move --to must be desktop or App#N/handle; window-local pointer is not mapped",
                );
            }
            if to != "desktop" {
                return usage_err(
                    "pointer-move --to <handle> is typed unsupported; use --to desktop or click --window",
                );
            }
            let x = match required_i32_flag(&mut args, "--x") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let y = match required_i32_flag(&mut args, "--y") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err("pointer-move accepts only --to desktop --x <i32> --y <i32>");
            }
            Command::PointerMove { target, x, y }
        }
        "pointer-position" => {
            if !args.is_empty() {
                return usage_err("pointer-position accepts no command arguments");
            }
            Command::PointerPosition { target }
        }
        "click" | "dclick" | "rclick" => {
            let mut window = flag_window_opt(&mut args);
            match flag_text(&mut args, "--to") {
                Ok(Some(to)) if to == "desktop" => {
                    return usage_err(
                        "click --to desktop is not mapped; use invoke or pointer-move --to desktop",
                    );
                }
                Ok(Some(to)) => {
                    if window.is_some() {
                        return usage_err("click accepts --window or --to, not both");
                    }
                    window = match agenterm_cu::observe::parse_window_token(&to) {
                        Ok(handle) => Some(handle),
                        Err(message) => return usage_err(message.replace("--window", "--to")),
                    };
                }
                Ok(None) => {}
                Err(message) => return usage_err(message),
            }
            let node = flag_value(&mut args, "--node");
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            let coords = flag_coords(&mut args, "--coords");
            let degraded = args.iter().any(|arg| arg == "--degraded");
            args.retain(|arg| arg != "--degraded");
            let clicks = if verb == "dclick" {
                2
            } else {
                flag_u32(&mut args, "--clicks").unwrap_or(1)
            };
            let button = if verb == "rclick" {
                PointerButton::Right
            } else {
                match flag_value(&mut args, "--button").as_deref() {
                    Some("right") => PointerButton::Right,
                    Some("middle") => PointerButton::Middle,
                    _ => PointerButton::Left,
                }
            };
            Command::Click {
                target,
                window,
                node,
                name,
                role,
                coords,
                degraded,
                clicks,
                button,
            }
        }
        "focus" => {
            let window = flag_window_opt(&mut args);
            let node = flag_value(&mut args, "--node");
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            if node.as_ref().is_none_or(|value| value.is_empty())
                && name.as_ref().is_none_or(|value| value.is_empty())
            {
                return usage_err(
                    "focus requires --node <path-id> or --window <handle> --name <pattern>",
                );
            }
            Command::Focus {
                target,
                window,
                node,
                name,
                role,
            }
        }
        "send-text" | "type" => {
            // `--` ends flag parsing so the text may itself start with a dash.
            let literal_text = match args.iter().position(|arg| arg == "--") {
                Some(index) => Some(args.split_off(index)[1..].join(" ")),
                None => None,
            };
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            Command::SendText {
                target,
                text: literal_text.unwrap_or_else(|| args.join(" ")),
                window,
                name,
                role,
            }
        }
        "clipboard-read" => match parse_clipboard_read(target, &mut args) {
            Ok(command) => command,
            Err(message) => return usage_err(message),
        },
        "clipboard-write" => match parse_clipboard_write(target, &mut args) {
            Ok(command) => command,
            Err(message) => return usage_err(message),
        },
        "clipboard-write-file" => match parse_clipboard_write_file(target, &mut args) {
            Ok(command) => command,
            Err(message) => return usage_err(message),
        },
        "clipboard-clear" => match parse_clipboard_clear(target, &mut args) {
            Ok(command) => command,
            Err(message) => return usage_err(message),
        },
        "clipboard" => {
            let sub = args
                .first()
                .cloned()
                .filter(|first| !first.starts_with('-'));
            if let Some(sub) = sub {
                args.remove(0);
                match parse_clipboard_subcommand(target, &sub, &mut args) {
                    Ok(command) => command,
                    Err(message) => return usage_err(message),
                }
            } else {
                match parse_clipboard_read(target, &mut args) {
                    Ok(command) => command,
                    Err(message) => return usage_err(message),
                }
            }
        }
        "copy" => {
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            if window.is_none() {
                return usage_err("copy requires --window <handle> [--name <pattern>]");
            }
            if name.as_ref().is_none_or(|value| value.is_empty())
                && role.as_ref().is_some_and(|value| !value.is_empty())
            {
                return usage_err("copy --role requires --name <pattern>");
            }
            Command::Copy {
                target,
                window,
                name,
                role,
            }
        }
        "paste" => {
            // `--` ends flag parsing so --text may itself start with a dash.
            let literal_text = match args.iter().position(|arg| arg == "--") {
                Some(index) => Some(args.split_off(index)[1..].join(" ")),
                None => None,
            };
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            let text = flag_value(&mut args, "--text").or(literal_text);
            if window.is_none() {
                return usage_err(
                    "paste requires --window <handle> [--name <pattern>] [--text TEXT]",
                );
            }
            if name.as_ref().is_none_or(|value| value.is_empty())
                && role.as_ref().is_some_and(|value| !value.is_empty())
            {
                return usage_err("paste --role requires --name <pattern>");
            }
            Command::Paste {
                target,
                text,
                window,
                name,
                role,
            }
        }
        "send-keys" | "key" => {
            // `--` ends flag parsing so a chord may itself start with a dash.
            let literal_keys = match args.iter().position(|arg| arg == "--") {
                Some(index) => Some(args.split_off(index)[1..].join("+")),
                None => None,
            };
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            Command::SendKeys {
                target,
                keys: literal_keys.unwrap_or_else(|| args.join("+")),
                window,
                name,
                role,
            }
        }
        "scroll" => {
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            if name.as_ref().is_none_or(|value| value.is_empty()) {
                return usage_err("scroll requires --window <handle> --name <pattern>");
            }
            Command::Scroll {
                target,
                window,
                name,
                role,
            }
        }
        "get-extents" => {
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            if name.as_ref().is_none_or(|value| value.is_empty()) {
                return usage_err("get-extents requires --window <handle> --name <pattern>");
            }
            Command::GetExtents {
                target,
                window,
                name,
                role,
            }
        }
        "select" => {
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            let start = flag_i32(&mut args, "--start");
            let end = flag_i32(&mut args, "--end");
            if name.as_ref().is_none_or(|value| value.is_empty())
                || start.is_none()
                || end.is_none()
            {
                return usage_err(
                    "select requires --window <handle> --name <pattern> --start N --end M",
                );
            }
            Command::Select {
                target,
                start: start.unwrap_or(0),
                end: end.unwrap_or(0),
                window,
                name,
                role,
            }
        }
        "get-selection" => {
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            if name.as_ref().is_none_or(|value| value.is_empty()) {
                return usage_err("get-selection requires --window <handle> --name <pattern>");
            }
            Command::GetSelection {
                target,
                window,
                name,
                role,
            }
        }
        "set-caret" => {
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            let offset = flag_i32(&mut args, "--offset");
            if name.as_ref().is_none_or(|value| value.is_empty()) || offset.is_none() {
                return usage_err(
                    "set-caret requires --window <handle> --name <pattern> --offset N",
                );
            }
            Command::SetCaret {
                target,
                offset: offset.unwrap_or(0),
                window,
                name,
                role,
            }
        }
        "get-caret" => {
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            if name.as_ref().is_none_or(|value| value.is_empty()) {
                return usage_err("get-caret requires --window <handle> --name <pattern>");
            }
            Command::GetCaret {
                target,
                window,
                name,
                role,
            }
        }
        "get-text" => {
            let window = flag_window_opt(&mut args);
            let name = flag_value(&mut args, "--name");
            let role = flag_value(&mut args, "--role");
            if window.is_none() && name.as_ref().is_none_or(|value| value.is_empty()) {
                return usage_err("get-text requires --window <handle> [--name <pattern>]");
            }
            Command::GetText {
                target,
                window,
                name,
                role,
            }
        }
        "app" | "launch" | "quit" | "hide" | "show" => {
            if verb != "app" {
                if verb == "launch"
                    && !args.iter().any(|arg| arg == "--path")
                    && args.first().is_some_and(|first| !first.starts_with('-'))
                {
                    let path = args.remove(0);
                    args.insert(0, path);
                    args.insert(0, "--path".into());
                }
                args.insert(0, verb.to_string());
            }
            let action_text = flag_value(&mut args, "--action")
                .or_else(|| {
                    args.first()
                        .cloned()
                        .filter(|first| !first.starts_with("--"))
                })
                .unwrap_or_default();
            if !action_text.is_empty() && args.first() == Some(&action_text) {
                args.remove(0);
            }
            let Some(action) = agenterm_cu::command::AppAction::parse(&action_text) else {
                return usage_err("app requires hide | show | quit (or --action <one of them>)");
            };
            let window = match flag_window(&mut args) {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let pid = match flag_parsed::<u32>(&mut args, "--pid") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            // `launch` names a path rather than a running thing; `show`
            // has no window to name because hiding removed them, so the pid
            // stands in. Everything else still wants a handle.
            let launching = action == agenterm_cu::command::AppAction::Launch;
            if !launching && window.is_none() && pid.is_none() {
                return usage_err("app requires --window <handle> or --pid <n>");
            }
            let window = window.unwrap_or(0);
            let snapshot = take_switch(&mut args, "--snapshot");
            let expect = match flag_text(&mut args, "--expect") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let path = match flag_text(&mut args, "--path") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err(format!(
                    "app accepts only <hide|show|quit|launch> --window H | --pid N | --path P [--snapshot --expect gone]; unexpected {:?}",
                    args[0]
                ));
            }
            Command::App {
                target,
                window,
                action,
                snapshot,
                expect,
                pid,
                path,
            }
        }
        "orderwin" => {
            let window = match flag_window(&mut args) {
                Ok(Some(value)) => value,
                Ok(None) => return usage_err("orderwin requires --window <handle>"),
                Err(message) => return usage_err(message),
            };
            let relative = match flag_handle(&mut args, "--relative") {
                Ok(Some(value)) => value,
                Ok(None) => return usage_err("orderwin requires --relative <handle>"),
                Err(message) => return usage_err(message),
            };
            let relation = match flag_text(&mut args, "--relation") {
                Ok(Some(raw)) => match agenterm_cu::OrderRelation::parse(&raw) {
                    Some(value) => value,
                    None => {
                        return usage_err("orderwin --relation must be above or below");
                    }
                },
                Ok(None) => return usage_err("orderwin requires --relation above|below"),
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err(format!(
                    "orderwin accepts only --window H --relation above|below --relative H; unexpected {:?}",
                    args[0]
                ));
            }
            Command::OrderWin {
                target,
                window,
                relation,
                relative,
            }
        }
        "frame" | "movewin" | "resize" | "maximize" => {
            match parse_mcu_place_alias(target, &verb, &mut args) {
                Ok(command) => command,
                Err(message) => return usage_err(message),
            }
        }
        "window-place" => {
            let action = flag_value(&mut args, "--action")
                .or_else(|| args.first().cloned())
                .unwrap_or_default();
            if action.is_empty() {
                return usage_err("window-place requires --action <id>");
            }
            let window = flag_window_opt(&mut args);
            // `--action frame` takes the requested rect; the four flags are
            // typed (a bad value is usage, never a silently dropped flag).
            let mut rect = [None; 4];
            for (slot, flag) in ["--x", "--y", "--width", "--height"]
                .into_iter()
                .enumerate()
            {
                rect[slot] = match flag_parsed::<i32>(&mut args, flag) {
                    Ok(value) => value,
                    Err(message) => return usage_err(message),
                };
            }
            let frame = match rect {
                [None, None, None, None] => None,
                [Some(x), Some(y), Some(width), Some(height)] => Some([x, y, width, height]),
                _ => {
                    return usage_err(
                        "window-place --action frame needs all four of --x --y --width --height",
                    );
                }
            };
            if action == "frame" && frame.is_none() {
                return usage_err(
                    "window-place --action frame requires --x X --y Y --width W --height H",
                );
            }
            Command::WindowPlace {
                target,
                action,
                window,
                frame,
            }
        }
        "close" => {
            // The destructive verb: closed shape, every part of the gate is
            // a flag the executor checks before touching anything.
            let window = match flag_window(&mut args) {
                Ok(Some(value)) => value,
                // Window 0 lets the executor name `target` among the missing
                // gate parts in one typed refusal.
                Ok(None) => 0,
                Err(message) => return usage_err(message),
            };
            let pid = match flag_parsed::<u32>(&mut args, "--pid") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let title = match flag_text(&mut args, "--title") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let snapshot = take_switch(&mut args, "--snapshot");
            let expect = match flag_text(&mut args, "--expect") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err(format!(
                    "close accepts only --window H [--pid N] [--title T] --snapshot --expect gone; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Close {
                target,
                window,
                pid,
                title,
                snapshot,
                expect,
            }
        }
        "receipts" => {
            let window = match flag_window(&mut args) {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            let max = match flag_parsed::<usize>(&mut args, "--max") {
                Ok(value) => value,
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err(format!(
                    "receipts accepts only [--window H] [--max N]; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Receipts {
                target,
                window,
                max,
            }
        }
        "wait" => {
            // `--` ends flag parsing so --text-equals / --text-contains may start with a dash.
            let literal_text = match args.iter().position(|arg| arg == "--") {
                Some(index) => Some(args.split_off(index)[1..].join(" ")),
                None => None,
            };
            let expect_present = args.iter().any(|arg| arg == "--expect");
            // `--expect` is a closed shape, so its timeout value is consumed
            // (the older conditions' lenient `flag_u64` leaves it in place).
            let timeout_ms = if expect_present {
                match flag_parsed::<u64>(&mut args, "--timeout-ms") {
                    Ok(value) => value.unwrap_or(5_000),
                    Err(message) => return usage_err(message),
                }
            } else {
                flag_u64(&mut args, "--timeout-ms").unwrap_or(5_000)
            };
            let text_equals_present = args
                .iter()
                .any(|arg| arg == "--text-equals" || arg == "--node-text-equals");
            let text_contains_present = args
                .iter()
                .any(|arg| arg == "--text-contains" || arg == "--node-text-contains");
            let condition = if text_equals_present && text_contains_present {
                return usage_err("wait accepts one of --text-equals or --text-contains, not both");
            } else if expect_present {
                let expect = match flag_text(&mut args, "--expect") {
                    Ok(Some(raw)) => match parse_expectations(&raw) {
                        Ok(expect) => expect,
                        Err(message) => return usage_err(message),
                    },
                    Ok(None) => return usage_err("wait --expect requires a JSON array"),
                    Err(message) => return usage_err(message),
                };
                let window = match flag_window(&mut args) {
                    Ok(Some(value)) => value,
                    Ok(None) => return usage_err("wait --expect requires --window <handle>"),
                    Err(message) => return usage_err(message),
                };
                if !args.is_empty() {
                    return usage_err(format!(
                        "wait --expect accepts only --timeout-ms MS --window H --expect JSON; unexpected {:?}",
                        args[0]
                    ));
                }
                WaitCondition::Expect { window, expect }
            } else if text_equals_present {
                let expected = flag_value(&mut args, "--text-equals")
                    .or_else(|| flag_value(&mut args, "--node-text-equals"))
                    .filter(|value| value != "--")
                    .or(literal_text);
                let Some(expected) = expected else {
                    return usage_err(
                        "wait --text-equals / --node-text-equals requires the expected text",
                    );
                };
                let name = flag_value(&mut args, "--name")
                    .or_else(|| flag_value(&mut args, "--node-name-contains"))
                    .filter(|value| !value.is_empty());
                let Some(name) = name else {
                    return usage_err("wait --text-equals requires --name <pattern>");
                };
                WaitCondition::NodeTextEquals {
                    expected,
                    name,
                    role: flag_value(&mut args, "--role")
                        .or_else(|| flag_value(&mut args, "--node-role")),
                    window: flag_window_opt(&mut args),
                }
            } else if text_contains_present {
                let substring = flag_value(&mut args, "--text-contains")
                    .or_else(|| flag_value(&mut args, "--node-text-contains"))
                    .filter(|value| value != "--")
                    .or(literal_text);
                let Some(substring) = substring else {
                    return usage_err(
                        "wait --text-contains / --node-text-contains requires the substring",
                    );
                };
                let name = flag_value(&mut args, "--name")
                    .or_else(|| flag_value(&mut args, "--node-name-contains"))
                    .filter(|value| !value.is_empty());
                let Some(name) = name else {
                    return usage_err("wait --text-contains requires --name <pattern>");
                };
                WaitCondition::NodeTextContains {
                    substring,
                    name,
                    role: flag_value(&mut args, "--role")
                        .or_else(|| flag_value(&mut args, "--node-role")),
                    window: flag_window_opt(&mut args),
                }
            } else if let Some(count) = flag_usize(&mut args, "--window-count-gte") {
                WaitCondition::WindowCountGte { count }
            } else if let Some(pattern) = flag_value(&mut args, "--window-title-contains") {
                WaitCondition::WindowTitleContains { pattern }
            } else if let Some(handle) = flag_isize(&mut args, "--focused-handle") {
                WaitCondition::FocusedHandle { handle }
            } else if let Some(pattern) = flag_value(&mut args, "--node-name-contains") {
                WaitCondition::NodeNameContains {
                    pattern,
                    role: flag_value(&mut args, "--node-role"),
                    window: flag_window_opt(&mut args),
                }
            } else {
                return usage_err(
                    "wait requires one of --window-count-gte, --window-title-contains, --focused-handle, --node-name-contains, --text-equals, or --text-contains",
                );
            };
            Command::Wait {
                target,
                timeout_ms,
                condition,
            }
        }
        "unlock" => {
            let window = match flag_window(&mut args) {
                Ok(Some(value)) => value,
                Ok(None) => return usage_err("unlock requires --window <handle>"),
                Err(message) => return usage_err(message),
            };
            if !args.is_empty() {
                return usage_err(format!(
                    "unlock accepts only --window H; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Unlock { target, window }
        }
        "spaces" => {
            if !args.is_empty() {
                return usage_err(format!(
                    "spaces accepts no arguments; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Spaces { target }
        }
        "displays" => {
            if !args.is_empty() {
                return usage_err(format!(
                    "displays accepts no arguments; unexpected {:?}",
                    args[0]
                ));
            }
            Command::Displays { target }
        }
        "cursor" => {
            if !args.is_empty() {
                return usage_err(format!(
                    "cursor accepts no arguments; unexpected {:?}",
                    args[0]
                ));
            }
            Command::PointerPosition { target }
        }
        "clip" => {
            if !args.is_empty() {
                return usage_err(format!(
                    "clip with no text is clipboard-read; unexpected {:?}",
                    args[0]
                ));
            }
            Command::ClipboardRead {
                target,
                type_name: None,
                max_bytes: None,
                out: None,
                replace: false,
            }
        }
        "page-js" | "page" => {
            if verb == "page" {
                if args.first().map(String::as_str) == Some("read") {
                    args.remove(0);
                }
                if let Some(index) = args.iter().position(|arg| arg == "--js") {
                    args[index] = "--expression".into();
                }
                if !args.iter().any(|arg| arg == "--expression") {
                    Command::Align {
                        target,
                        group: "page".into(),
                    }
                } else {
                    match parse_page_js(target, &mut args) {
                        Ok(command) => command,
                        Err(message) => return usage_err(message),
                    }
                }
            } else {
                match parse_page_js(target, &mut args) {
                    Ok(command) => command,
                    Err(message) => return usage_err(message),
                }
            }
        }
        other if agenterm_cu::mcu_surface::is_align_verb(other) => Command::Align {
            target,
            group: other.to_owned(),
        },
        other => return usage_err(format!("unknown command '{other}'")),
    };

    if grant_id.is_some() && (grant.is_some() || ambient_authority_present) {
        return agenterm_cu::CuReply {
            ok: false,
            target: command.target().as_str().into(),
            command: command.verb(),
            data: None,
            error: Some(agenterm_cu::CuError::new(
                "invalid_authorization",
                "--grant-id cannot be combined with another authorization source",
            )),
        };
    }
    if grant_id.is_none() && grant_store.is_some() {
        return usage_err("--grant-store requires --grant-id for command execution");
    }
    if grant_id.is_none() && unsupported_authority_environment {
        return agenterm_cu::CuReply {
            ok: false,
            target: command.target().as_str().into(),
            command: command.verb(),
            data: None,
            error: Some(agenterm_cu::CuError::new(
                "invalid_authorization",
                "unsupported authorization environment selector is present",
            )),
        };
    }
    let auth = match if grant_id.is_some() {
        Ok(Authorization::new(Default::default()))
    } else {
        resolve_authorization(grant.as_deref())
    } {
        Ok(auth) => auth,
        Err(error) => {
            return agenterm_cu::CuReply {
                ok: false,
                target: command.target().as_str().into(),
                command: command.verb(),
                data: None,
                error: Some(error),
            };
        }
    };
    let mut executor = Executor::new(auth);
    if let Some(grant_id) = grant_id {
        let store_path =
            match grant_store.map_or_else(agenterm_cu::auth_store::AuthStore::default_path, Ok) {
                Ok(path) => path,
                Err(_) => {
                    return agenterm_cu::CuReply {
                        ok: false,
                        target: command.target().as_str().into(),
                        command: command.verb(),
                        data: None,
                        error: Some(agenterm_cu::CuError::new(
                            "grant_store_unavailable",
                            "grant store is unavailable",
                        )),
                    };
                }
            };
        executor = executor.with_persisted_grant(grant_id, store_path);
    }
    if target == TargetRef::Ssh {
        let dest = ssh_dest.expect("ssh destination checked above");
        match SshEndpoint::from_parts(dest, ssh_port, ssh_identity, ssh_cu, ssh_env) {
            Ok(endpoint) => executor = executor.with_ssh(endpoint),
            Err(error) => {
                return agenterm_cu::CuReply {
                    ok: false,
                    target: "ssh".into(),
                    command: "usage".into(),
                    data: None,
                    error: Some(error),
                };
            }
        }
    }
    if target == TargetRef::Vnc {
        let dest = vnc_dest.expect("vnc destination checked above");
        match VncEndpoint::from_parts(dest, vnc_port, vnc_cu, vnc_env) {
            Ok(endpoint) => executor = executor.with_vnc(endpoint),
            Err(error) => {
                return agenterm_cu::CuReply {
                    ok: false,
                    target: "vnc".into(),
                    command: "usage".into(),
                    data: None,
                    error: Some(error),
                };
            }
        }
    }
    if target == TargetRef::Rdp
        && let Some(dest) = rdp_dest
    {
        match RdpEndpoint::from_parts(dest) {
            Ok(endpoint) => executor = executor.with_rdp(endpoint),
            Err(error) => {
                return agenterm_cu::CuReply {
                    ok: false,
                    target: "rdp".into(),
                    command: "usage".into(),
                    data: None,
                    error: Some(error),
                };
            }
        }
    }
    // No endpoint: Executor::execute_rdp returns rdp_unavailable.
    executor.execute(&command)
}

fn dispatch_json(args: &[String]) -> agenterm_cu::CuReply {
    let mut grant: Option<String> = None;
    let mut grant_id: Option<String> = None;
    let mut grant_store: Option<PathBuf> = None;
    let mut payload = None;
    let mut read_stdin = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(value) = arg.strip_prefix("--grant=") {
            if grant.is_some() {
                return usage_err("duplicate --grant");
            }
            grant = Some(value.to_owned());
            i += 1;
        } else if let Some(value) = arg.strip_prefix("--grant-id=") {
            if grant_id.is_some() {
                return usage_err("duplicate --grant-id");
            }
            if !agenterm_cu::grant_management::valid_grant_id(value) {
                return usage_err("--grant-id is invalid");
            }
            grant_id = Some(value.to_owned());
            i += 1;
        } else if let Some(value) = arg.strip_prefix("--grant-store=") {
            if grant_store.is_some() {
                return usage_err("duplicate --grant-store");
            }
            if value.is_empty() {
                return usage_err("--grant-store requires a path");
            }
            grant_store = Some(PathBuf::from(value));
            i += 1;
        } else if arg == "--grant" {
            if grant.is_some() {
                return usage_err("duplicate --grant");
            }
            i += 1;
            if let Some(value) = args.get(i) {
                grant = Some(value.clone());
                i += 1;
            }
        } else if arg == "--json" {
            i += 1;
            if let Some(value) = args.get(i) {
                if value == "-" {
                    read_stdin = true;
                } else {
                    payload = Some(value.clone());
                }
                i += 1;
            } else {
                read_stdin = true;
            }
        } else if arg == "--json-stdin" || arg == "-" {
            read_stdin = true;
            i += 1;
        } else if payload.is_none() {
            if arg == "-" {
                read_stdin = true;
            } else {
                payload = Some(arg.clone());
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    let raw = if read_stdin {
        let mut buf = String::new();
        if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf) {
            return usage_err(format!("could not read JSON command from stdin: {error}"));
        }
        buf
    } else {
        let Some(raw) = payload else {
            return usage_err(
                "exec requires a JSON command payload argument, --json '-', or --json-stdin",
            );
        };
        raw
    };
    let command: Command = match serde_json::from_str(&raw) {
        Ok(command) => command,
        Err(error) => return usage_err(format!("invalid JSON command: {error}")),
    };
    let (ambient, unsupported_authority_environment) = authority_environment_flags();
    if grant_id.is_some() && (grant.is_some() || ambient) {
        return agenterm_cu::CuReply {
            ok: false,
            target: command.target().as_str().into(),
            command: command.verb(),
            data: None,
            error: Some(agenterm_cu::CuError::new(
                "invalid_authorization",
                "--grant-id cannot be combined with another authorization source",
            )),
        };
    }
    if grant_id.is_none() && grant_store.is_some() {
        return usage_err("--grant-store requires --grant-id for command execution");
    }
    if grant_id.is_none() && unsupported_authority_environment {
        return agenterm_cu::CuReply {
            ok: false,
            target: command.target().as_str().into(),
            command: command.verb(),
            data: None,
            error: Some(agenterm_cu::CuError::new(
                "invalid_authorization",
                "unsupported authorization environment selector is present",
            )),
        };
    }
    let auth = match if grant_id.is_some() {
        Ok(Authorization::new(Default::default()))
    } else {
        resolve_authorization(grant.as_deref())
    } {
        Ok(auth) => auth,
        Err(error) => {
            return agenterm_cu::CuReply {
                ok: false,
                target: command.target().as_str().into(),
                command: command.verb(),
                data: None,
                error: Some(error),
            };
        }
    };
    let mut executor = Executor::new(auth);
    if let Some(grant_id) = grant_id {
        let store_path =
            match grant_store.map_or_else(agenterm_cu::auth_store::AuthStore::default_path, Ok) {
                Ok(path) => path,
                Err(_) => {
                    return agenterm_cu::CuReply {
                        ok: false,
                        target: command.target().as_str().into(),
                        command: command.verb(),
                        data: None,
                        error: Some(agenterm_cu::CuError::new(
                            "grant_store_unavailable",
                            "grant store is unavailable",
                        )),
                    };
                }
            };
        executor = executor.with_persisted_grant(grant_id, store_path);
    }
    executor.execute(&command)
}

fn authority_environment_flags() -> (bool, bool) {
    let mut any = false;
    let mut unsupported = false;
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        let reserved = ["AGENTERM_CU_GRANT", "AGENTERM_CU_AUTH"]
            .iter()
            .any(|prefix| {
                key.get(..prefix.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
            });
        if reserved {
            any = true;
            if !key.eq_ignore_ascii_case("AGENTERM_CU_GRANT") {
                unsupported = true;
            }
        }
    }
    (any, unsupported)
}

fn resolve_authorization(cli_grant: Option<&str>) -> Result<Authorization, agenterm_cu::CuError> {
    let environment_grant = if cli_grant.is_none() {
        std::env::var("AGENTERM_CU_GRANT").ok()
    } else {
        None
    };
    Authorization::try_from_sources(cli_grant, environment_grant.as_deref()).map_err(|error| {
        let problem = match error.kind {
            agenterm_cu::auth::GrantParseErrorKind::EmptyToken => "is empty",
            agenterm_cu::auth::GrantParseErrorKind::UnknownToken => "is unknown",
        };
        agenterm_cu::CuError::new(
            "invalid_authorization",
            format!("grant scope token {} {problem}", error.token_index),
        )
    })
}

fn take_value(args: &mut Vec<String>, flag: &str) -> String {
    args.remove(0);
    if args.is_empty() {
        eprintln!("missing value for {flag}");
        return String::new();
    }
    args.remove(0)
}

fn flag_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.remove(index);
    args.get(index).cloned()
}

fn flag_isize(args: &mut Vec<String>, flag: &str) -> Option<isize> {
    flag_value(args, flag)?.parse().ok()
}

/// A typed flag that must parse when present: `Ok(None)` when absent,
/// `Err(usage)` when present without a value or with a value that is not a
/// `T`. Unlike `flag_isize`, a bad value never silently drops the flag.
fn flag_window(args: &mut Vec<String>) -> Result<Option<isize>, String> {
    flag_handle(args, "--window")
}

fn flag_handle(args: &mut Vec<String>, flag: &'static str) -> Result<Option<isize>, String> {
    let Some(index) = args.iter().position(|arg| arg == flag) else {
        return Ok(None);
    };
    args.remove(index);
    if index >= args.len() {
        return Err(format!("{flag} requires a value"));
    }
    let raw = args.remove(index);
    agenterm_cu::observe::parse_window_token(&raw)
        .map(Some)
        .map_err(|message| message.replace("--window", flag))
}

fn flag_window_opt(args: &mut Vec<String>) -> Option<isize> {
    let raw = flag_value(args, "--window")?;
    agenterm_cu::observe::parse_window_token(&raw).ok()
}

fn flag_parsed<T: std::str::FromStr>(
    args: &mut Vec<String>,
    flag: &'static str,
) -> Result<Option<T>, String> {
    let Some(index) = args.iter().position(|arg| arg == flag) else {
        return Ok(None);
    };
    args.remove(index);
    if index >= args.len() {
        return Err(format!("{flag} requires a value"));
    }
    let raw = args.remove(index);
    raw.parse::<T>().map(Some).map_err(|_| {
        format!(
            "{flag} value {raw:?} is not a {}",
            std::any::type_name::<T>()
        )
    })
}

/// A string flag whose value is consumed with it (unlike `flag_value`, which
/// leaves the value in `args` for the older verbs' lenient parsing). Present
/// without a value is a usage error.
fn flag_text(args: &mut Vec<String>, flag: &'static str) -> Result<Option<String>, String> {
    let Some(index) = args.iter().position(|arg| arg == flag) else {
        return Ok(None);
    };
    args.remove(index);
    if index >= args.len() {
        return Err(format!("{flag} requires a value"));
    }
    Ok(Some(args.remove(index)))
}

/// A presence switch (`--flat`, `--actionable`); every occurrence is removed.
fn take_switch(args: &mut Vec<String>, flag: &str) -> bool {
    let present = args.iter().any(|arg| arg == flag);
    args.retain(|arg| arg != flag);
    present
}

/// `--focused` alone means `true`; `--focused true|false` is explicit.
fn flag_tristate(args: &mut Vec<String>, flag: &str) -> Option<bool> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.remove(index);
    match args.get(index).map(String::as_str) {
        Some("true") => {
            args.remove(index);
            Some(true)
        }
        Some("false") => {
            args.remove(index);
            Some(false)
        }
        _ => Some(true),
    }
}

fn flag_i32(args: &mut Vec<String>, flag: &str) -> Option<i32> {
    flag_value(args, flag)?.parse().ok()
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

fn flag_u32(args: &mut Vec<String>, flag: &str) -> Option<u32> {
    flag_value(args, flag)?.parse().ok()
}

fn flag_u64(args: &mut Vec<String>, flag: &str) -> Option<u64> {
    flag_value(args, flag)?.parse().ok()
}

fn flag_usize(args: &mut Vec<String>, flag: &str) -> Option<usize> {
    flag_value(args, flag)?.parse().ok()
}

fn flag_coords(args: &mut Vec<String>, flag: &str) -> Option<[i32; 2]> {
    let raw = flag_value(args, flag)?;
    let mut parts = raw.split(',');
    let x = parts.next()?.trim().parse().ok()?;
    let y = parts.next()?.trim().parse().ok()?;
    Some([x, y])
}

fn parse_screenshot(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
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

fn parse_query(target: TargetRef, verb: &str, args: &mut Vec<String>) -> Result<Command, String> {
    // Closed CLI shape (mcu lesson): an unknown flag, a missing value, or a
    // stray positional fails here, before any tree is read.
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

fn parse_page_js(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let window = flag_window(args)?;
    let expression = flag_text(args, "--expression")?;
    let port = flag_parsed::<u16>(args, "--port")?;
    if !args.is_empty() {
        return Err(format!(
            "page-js accepts only --window H --expression EXPR --port N; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::PageJs {
        target,
        window,
        expression,
        port,
    })
}

fn parse_clipboard_read(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let type_name = flag_text(args, "--type")?;
    let type_name = type_name.or_else(|| {
        args.first()
            .cloned()
            .filter(|first| !first.starts_with('-'))
            .inspect(|_| {
                args.remove(0);
            })
    });
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

fn parse_clipboard_write(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let type_name = flag_text(args, "--type")?;
    let path = flag_text(args, "--path")?;
    let type_name = type_name.or_else(|| {
        args.first()
            .cloned()
            .filter(|first| !first.starts_with('-'))
            .inspect(|_| {
                args.remove(0);
            })
    });
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

fn parse_clipboard_write_file(
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    let path = flag_text(args, "--path")?;
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
            "clipboard-write-file accepts --path P; unexpected {:?}",
            args[0]
        ));
    }
    let Some(path) = path else {
        return Err("clipboard-write-file requires --path P".into());
    };
    Ok(Command::ClipboardWriteFile { target, path })
}

fn parse_clipboard_subcommand(
    target: TargetRef,
    sub: &str,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match sub {
        "read" => parse_clipboard_read(target, args),
        "write" => parse_clipboard_write(target, args),
        "write-file" => parse_clipboard_write_file(target, args),
        "clear" => parse_clipboard_clear(target, args),
        other => Err(format!(
            "unknown clipboard subcommand {other:?}; expected read|write|write-file|clear"
        )),
    }
}

fn parse_optional_window(args: &mut Vec<String>) -> Result<Option<isize>, String> {
    if let Some(window) = flag_window(args)? {
        return Ok(Some(window));
    }
    let Some(first) = args.first() else {
        return Ok(None);
    };
    if first.starts_with('-') {
        return Ok(None);
    }
    let token = args.remove(0);
    agenterm_cu::observe::parse_window_token(&token)
        .map(Some)
        .map_err(|message| message.replace("--window", "window handle"))
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

fn parse_mcu_place_alias(
    target: TargetRef,
    verb: &str,
    args: &mut Vec<String>,
) -> Result<Command, String> {
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
            let x = take_i32_flag_or_positional(args, "--x")?.ok_or_else(|| {
                "frame requires --x --y --width --height (or HANDLE X Y W H)".to_owned()
            })?;
            let y = take_i32_flag_or_positional(args, "--y")?.ok_or_else(|| {
                "frame requires --x --y --width --height (or HANDLE X Y W H)".to_owned()
            })?;
            let width = take_i32_flag_or_positional(args, "--width")?.ok_or_else(|| {
                "frame requires --x --y --width --height (or HANDLE X Y W H)".to_owned()
            })?;
            let height = take_i32_flag_or_positional(args, "--height")?.ok_or_else(|| {
                "frame requires --x --y --width --height (or HANDLE X Y W H)".to_owned()
            })?;
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

fn parse_clipboard_clear(target: TargetRef, args: &mut Vec<String>) -> Result<Command, String> {
    let apply = take_switch(args, "--apply");
    if !args.is_empty() {
        return Err(format!(
            "clipboard-clear accepts only --apply; unexpected {:?}",
            args[0]
        ));
    }
    Ok(Command::ClipboardClear { target, apply })
}

/// `--expect` is a JSON array of closed-shape items; an unknown key or a
/// non-array is a usage error before any tree is read.
fn parse_expectations(raw: &str) -> Result<Vec<Expectation>, String> {
    let items: Vec<Expectation> = serde_json::from_str(raw)
        .map_err(|error| format!("--expect must be a JSON array of {{node|index|name|identifier|role, value?, checked?, expanded?, focused?}} items: {error}"))?;
    if items.is_empty() {
        return Err("--expect needs at least one item".to_owned());
    }
    for (position, item) in items.iter().enumerate() {
        if !item.has_target() {
            return Err(format!(
                "--expect item {position} needs a target (node, index, name, identifier or role)"
            ));
        }
        if !item.has_state() && !item.has_page_identity() {
            return Err(format!(
                "--expect item {position} needs a state (value, checked, expanded or focused) or a page identity (name/titleIncludes)"
            ));
        }
    }
    Ok(items)
}

fn usage_err(message: impl Into<String>) -> agenterm_cu::CuReply {
    eprint_usage();
    agenterm_cu::CuReply {
        ok: false,
        target: String::new(),
        command: "usage".into(),
        data: None,
        error: Some(agenterm_cu::CuError::new("usage", message)),
    }
}

fn help_reply(ok: bool) -> agenterm_cu::CuReply {
    agenterm_cu::CuReply {
        ok,
        target: String::new(),
        command: "help".into(),
        data: Some(serde_json::json!({ "usage": "see stderr" })),
        error: None,
    }
}

fn run_x11_clipboard_owner() -> i32 {
    use std::io::Read;
    let mut text = String::new();
    if std::io::stdin().read_to_string(&mut text).is_err() {
        return 1;
    }
    match agenterm_cu::mechanism::clipboard::own_text(&text) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn eprint_usage() {
    eprintln!(
        r#"usage: agenterm-cu --target <current|ssh|vnc|rdp> [--grant observe,actuate] <command> [args...]
       agenterm-cu --ssh <user@host> [--ssh-port N] [--ssh-identity PATH] [--ssh-cu PATH]
                   [--ssh-env KEY=VAL]... [--grant observe,actuate] <command> [args...]
       agenterm-cu --vnc <host[:port]> [--vnc-port N] [--vnc-cu PATH]
                   [--vnc-env KEY=VAL]... [--grant observe,actuate] <command> [args...]
       agenterm-cu --rdp <host[:port]> [--grant observe,actuate] <command> [args...]
       agenterm-cu exec [--grant observe,actuate] --json '<command-json>'
       agenterm-cu exec [--grant observe,actuate] --json -   # JSON command on stdin
       agenterm-cu grant create --target current --scopes S --ttl-ms N
                         (--one-shot|--max-uses N) [--grant-store PATH]
       agenterm-cu grant list [--grant-store PATH]
       agenterm-cu grant revoke --grant-id ID [--grant-store PATH]
       agenterm-cu host                        desktop menu and global shortcuts
       agenterm-cu hotkeys                     compatibility alias for host

Global:
  --target current|ssh|vnc|rdp  explicit target reference (required unless --ssh/--vnc/--rdp)
  --ssh <user@host>         ssh target destination (implies --target ssh)
  --ssh-port N              OpenSSH -p (or AGENTERM_CU_SSH_PORT)
  --ssh-identity PATH       OpenSSH -i (or AGENTERM_CU_SSH_IDENTITY)
  --ssh-cu PATH             remote agenterm-cu path (or AGENTERM_CU_SSH_CU; default: this exe)
  --ssh-env KEY=VAL         remote env for the worker (repeatable; also AGENTERM_CU_SSH_ENV)
  --vnc <host[:port]>       vnc/RFB endpoint (implies --target vnc; or AGENTERM_CU_VNC)
  --vnc-port N              RFB TCP port when --vnc omits :port (or AGENTERM_CU_VNC_PORT; default 5900)
  --vnc-cu PATH             session worker agenterm-cu path (or AGENTERM_CU_VNC_CU; default: this exe)
  --vnc-env KEY=VAL         session env for the worker (repeatable; also AGENTERM_CU_VNC_ENV)
  --rdp <host[:port]>       rdp endpoint syntax only (implies --target rdp; PLACEHOLDER —
                            no connect / TLS / CredSSP; always rdp_unavailable)
  --grant observe,actuate   strict authorization scopes; CLI wins over
                            AGENTERM_CU_GRANT and sources never union
  --grant-id ID             bounded persisted current-target grant selector;
                            mutually exclusive with every other auth source
  --grant-store PATH        explicit store override; valid only with --grant-id

  grant management is local/current only. It refuses ambient AGENTERM_CU_GRANT*
  and AGENTERM_CU_AUTH* selectors; --grant-store is an explicit test/admin seam.

  ssh transport runs the same verbs on a remote agenterm-cu --target current
  worker over OpenSSH stdio (no new verb). Get-selection evidence: loopback
  sshd + second agenterm-con, host send-text seed into Command, host select a
  range, host independent get-selection --name Command returns that range
  (via=get-selection; native AT-SPI GetNSelections+GetSelection; never
  screenshot / --coords / mouse-drag).

  vnc transport handshakes RFB (security type None / x11vnc -nopw), then runs
  the same verbs (observe and actuate) on a local agenterm-cu --target current
  worker against the shared session (DISPLAY/AT-SPI env; no new verb). Get-selection
  evidence: gate-owned loopback x11vnc + second agenterm-con, Command holds a
  known ASCII seed with a known non-empty selection START..END (gate
  precondition), host independent get-selection --window H --name Command
  returns that range (via=get-selection; native AT-SPI GetNSelections +
  GetSelection(0); n==1 start/end equal precondition range; never screenshot /
  --coords / mouse-drag / RFB framebuffer OCR / cached setter reply).

  rdp is a PLACEHOLDER (cut 3.46): --rdp HOST[:PORT] and --target rdp parse,
  authorize, then fail closed with error.code=rdp_unavailable. No socket
  connect, no TLS/CredSSP, no screenshot/--coords, no silent ssh/vnc/current
  reuse. Reserved first observe argv for a later Windows agent:
    agenterm-cu --rdp "WINDOWS_HOST:3389" --grant observe tree --window HANDLE
  Live RDP session + UIA-over-RDP evidence is not claimed on this cut.

Commands:
  capabilities
  windows [--pid N] [--app SUB] [--title SUB] [--focused [BOOL]] [--minimized [BOOL]]
          [--offset N] [--max N]
                              bare: the window array; with any filter/page flag:
                              {{windows, visited, matched, returned, offset, truncated}}
  windows-watch [--pid N] [--app SUB] [--title SUB] [--duration-ms N] [--interval-ms N]
                [--max-events N]
                              poll-diff over the windows inventory (appeared /
                              disappeared / changed + field list). Not AXObserver.
                              --duration-ms 0 (default) takes one extra sample.
  apps [--running] [--all]
                              running apps from top-level windows (pids + window
                              count). --all also lists the applications installed
                              on this host that no window can reveal, each marked
                              running or not.
  app <hide|show|quit|launch> [--window HANDLE] [--pid N] [--path P]
      [--snapshot --expect gone]
                              steps a whole application, not one of its windows.
                              hide/show take it aside and back (show takes --pid:
                              hiding removed the windows the handle named); quit
                              is destructive and carries the same three-part gate
                              as close -- it presses the application's own Quit
                              item and reads the process back. launch --path
                              starts an installed application; the reply says
                              pid: null because the launcher owns the process,
                              so wait for its window if a pid is needed.
  unlock --window HANDLE      asks the owning application to build its full
                              accessibility tree (macOS AXManualAccessibility),
                              reading the bounded tree before and after. Reports
                              poked (the request was delivered), grew and
                              returned_before separately, because the poke's own
                              status is not the outcome: AppKit calls the
                              attribute unsupported even when it lands, so only
                              the re-read can claim anything about the tree.
  tree [--window HANDLE] [--depth N] [--max-nodes N] [--flat]
                              depth (root=0, <=64) and node budget (1..20000) apply
                              while the platform walks; reply carries truncated /
                              visited / returned. --flat numbers nodes (index, depth)
                              in walk order — the same identity query reports
  query | inspect | find | read --window HANDLE|App#N | HANDLE [--depth N] [--max-nodes N]
        [--role R,R] [--text T | --text-exact T] [--identifier ID] [--actionable]
        [--within X,Y,W,H] [--offset N] [--max N] [--selector PATH]
                              inspect is query (MCU `inspect HANDLE`; `--app` stays MCU).
                              find HANDLE TEXT is query --text; read HANDLE SELECTOR is
                              query --selector. MCU Role[idx] / Role@title / *@title / #desc.
                              bounded, filtered flat node list with visited /
                              matched / returned / truncated; roles accept AXTextArea
                              or text-area; an unknown flag fails before the walk
  invoke --window HANDLE (--node PATH | --index N | --name PAT [--role ROLE] | --identifier ID
                          | --focused [--role ROLE])
         <press | set-value TEXT | select-option NAME | set-checked true|false
          | set-expanded true|false | increment | decrement | scroll-to
          | set-selection START:LENGTH>
                              one semantic a11y action; never activates or raises
                              the window. Two showing matches -> "ambiguous", none
                              -> "a11y_node_not_found", an action the node does not
                              offer -> "unsupported". set-checked / set-expanded are
                              desired states (already there = verified no-op). The
                              reply carries verified true|false with the reason and a
                              receipt (target, node, action, before / after state).
                              --focused acts on the application's own focused control
                              (what `focused` reports), bound by id / role / identifier
                              in the same tree read; --role narrows it ("unverified"
                              when the focused control has another role)
  menu inspect --window HANDLE [--depth N] [--max-nodes N] [--title T [--exact]]
               [--enabled true|false] [--offset N] [--max N]
                              the application's menu bar read in the background (never
                              opens a menu, never activates): items with exact title
                              paths, depth (0 = bar items, default 1, <= 8), enabled /
                              checked / has_submenu; node budget 1..5000; counts
                              visited / matched / returned / truncated
  menu invoke --window HANDLE --path 'Menu/Item' | '["Menu","Item"]'
                              press one menu item by exact path in the background; every
                              segment must be exactly one enabled item before anything is
                              pressed ("a11y_menu_item_not_found" / "..._ambiguous" /
                              "..._disabled"), the last must be a leaf ("..._not_leaf"),
                              a bare menu is "invalid_input"; verified by mark read-back
                              / tree diff
  focused --window HANDLE [--role ROLE] [--max-value-bytes N]
                              the application's own focused control inside the window
                              (id / role / name / identifier / states / value preview),
                              read without the foreground; --role binds the expected
                              role ("unverified" on mismatch); default preview 4096
                              bytes, 0 keeps only value_bytes
  observe --window HANDLE (--duration S | --duration-ms N) [--depth N] [--max-nodes N]
          [--max-events N] [--notification A,B] [--interval-ms N]
                              bounded event stream by poll-diff over the same bounded
                              tree: ValueChanged / TitleChanged / StateChanged /
                              FocusChanged / Created / Destroyed with monotonic seq and
                              t_ms; stops at --max-events (<= 5000, default 200) with
                              truncated true; reports polls / emitted / filtered / stopped
  verify --window HANDLE --expect '[{{"node"|"index"|"name"|"titleIncludes"[+"role"]|"identifier"|"role",
                                     "value"?, "checked"?, "expanded"?, "focused"?}}, ...]'
                              one tree read; all met -> ok + verified, a mismatch ->
                              "unverified", a state the node does not expose ->
                              "unsupported" (fail closed), an unknown key -> usage.
                              name/titleIncludes alone is page identity (WebArea title)
  page-js [--window HANDLE] --expression EXPR [--port N]
                              second knife: CDP Runtime.evaluate on
                              127.0.0.1:N (default 9222). MAIN-world Function
                              constructor is refused. No listener -> typed
                              unsupported with backend debugger-runtime-evaluate.
  spaces                      macOS SkyLight managed Space inventory (read-only).
                              linux/windows typed unsupported.
  displays                    native screen frames via agt_screen_list (MCU 系统).
  cursor                      alias of pointer-position
  clip / clipboard            alias of clipboard-read (observe text only)
  caps                        alias of capabilities
  dclick / rclick             aliases of click (--clicks 2 / --button right)
  shot                        alias of screenshot
  type / key / move           aliases of send-text / send-keys / pointer-move
  elements                    alias of tree --flat
  launch / quit / hide / show aliases of app <action>
  page                        MCU page: `page read --js` → page-js; other page verbs typed
  screenshot --out PATH [--window HANDLE]
  pointer-move --x X --y Y moves to absolute screen coordinates without any
                              press/release/click/drag/wheel side effect
  pointer-position            observes absolute screen coordinates without
                              injecting any pointer event (macOS: a read-only
                              CGEvent sample; the journey reads it around every
                              click / close to prove the real pointer stayed put)
  close --window HANDLE [--pid N] [--title T] --snapshot --expect gone
                              the destructive verb: closes one top-level window in
                              the background through the platform's own close
                              control (macOS AXCloseButton + AXPress). The gate is
                              three parts, all checked before anything is touched:
                              an exact target (--window, bound to --pid / exact
                              --title in the same inventory read), a prior
                              snapshot (--snapshot: the bounded tree written to
                              the receipt) and a checkable postcondition (--expect
                              gone: the handle read back as absent). Missing any ->
                              "refused" (detail.reason destructive_gate, missing
                              [...]) with nothing performed
  receipts [--window HANDLE] [--max N]
                              the target's crash-persistent receipt file
                              (<audit dir>/cu-receipts/<target>.jsonl) read back
                              in order: every invoke / menu invoke / click / focus
                              / close appends a "reserved" line before the
                              mechanism and a "completed" / "failed" line after
                              the read-back; a "reserved" line with no partner is
                              the crash signature (uncertain, never "did not
                              happen"). Default 50, at most 1000
  click (--window HANDLE --node ID | --window HANDLE --name PAT [--role ROLE] | --coords X,Y --degraded)
        [--button left|right|middle] [--clicks N]
                              --name reuses wait NodeNameContains matching, then the --node AT-SPI path
  focus [--window HANDLE] (--node ID | --window HANDLE --name PAT [--role ROLE])
  send-text [--window HANDLE [--name PAT [--role ROLE]]] [--] <text...>
                              --name writes via AT-SPI EditableText (SetTextContents /
                              InsertText) or AT-SPI Text + toolkit set-value when
                              EditableText is absent (Chrome renderer AX; WebKitGTK
                              AT-SPI id + eval helper); a node with no
                              writeable text interface typed-fails (never XTest).
                              --window without --name writes that same path on
                              the showing focused node (same innermost Text
                              candidate as get-text --window). Never XTest when
                              --window is set. Without --window stays the
                              plain type-into-focused inject.
                              `--` ends flag parsing
  clipboard-read [--type T] [--max-bytes N] [--out PATH [--replace]]
                              no --type: Unicode text plus host type names.
                              --type T: one native type as bounded bytes
                              (default 1 MiB, max 16 MiB), sha256, utf8 or
                              base64. --out writes the bytes (0600; --replace
                              overwrites). Requires observe.
  clipboard-write --type T --path P
  clipboard write T P         publish one native type from a regular file
                              (≤16 MiB) and read it back (actuate).
  clipboard-write-file --path P
  clipboard write-file P      put a file reference on the clipboard
                              (macOS POSIX file / Linux text/uri-list /
                              Windows CF_HDROP), not the file bytes (actuate).
  clipboard-clear [--apply]
  clipboard clear [--apply]   empty the clipboard. Without --apply this is
                              planned and performs nothing (actuate).
  copy --window HANDLE [--name PAT [--role ROLE]]
                              copies AT-SPI Text.GetText onto the native
                              clipboard (Linux X11: SetSelectionOwner, not
                              xclip). addressing=accessibility-tree via=gettext.
                              --name targets the unique showing named node.
                              --window without --name copies that same path on
                              the showing focused node (same innermost Text
                              candidate as get-text --window; con Command
                              via=gettext on a second con that never steals the
                              resident control socket; Chrome GetTextField;
                              Reasonix Message Reasonix… under
                              scripts/reasonix-desktop-a11y.sh). Never XTest when
                              --window is set. A node with no Text interface
                              typed-fails (never XTest / --coords / screenshot).
                              Close the circuit with paste --window (no --text /
                              no --name) then get-text --window /
                              wait --text-equals; copy matched.text does not
                              count.
  paste --window HANDLE [--name PAT [--role ROLE]] [--text TEXT]
                              writes clipboard text via native AT-SPI EditableText
                              / Text (addressing=accessibility-tree). --text only
                              seeds the clipboard; the field write always reads
                              the clipboard. --name targets the unique showing
                              named field. --window without --name writes that
                              same path on the showing focused node (same
                              innermost Text candidate as get-text --window;
                              con Command via=editable-text on a second con
                              that never steals the resident control socket;
                              Chrome GetTextField; Reasonix Message Reasonix…).
                              Never XTest when --window is set. A node with no
                              writeable text interface typed-fails (never XTest
                              / --coords / screenshot). Close the circuit with
                              get-text --window / wait --text-equals; paste
                              matched.text does not count. `--` ends flag
                              parsing
  send-keys [--window HANDLE [--name PAT [--role ROLE]]] [--] <keys...>
                              --name delivers AT-SPI Device/key events
                              (DeviceEventListener NotifyEvent); a node with no
                              key interface typed-fails (never XTest).
                              --window without --name targets the showing
                              focused node (same innermost Text candidate as
                              get-text --window). Prefers DeviceEventListener;
                              plain typeable text falls back to the AT-SPI
                              EditableText/Text write path when that interface
                              is absent (con Command; Chrome; Reasonix).
                              Never XTest when --window is set.
                              Without --window stays the plain focused inject.
                              `--` ends flag parsing. e.g. ctrl+c / enter / k
  scroll --window HANDLE --name PAT [--role ROLE]
                              one-shot AT-SPI Component.ScrollTo(TopEdge).
                              addressing=accessibility-tree via=scroll-to.
                              Missing / false / UnknownMethod typed-fails
                              (a11y_scroll_unavailable). Never Action scroll*,
                              XTest wheel, --coords, or screenshot.
  get-extents --window HANDLE --name PAT [--role ROLE]
                              independent AT-SPI Component.GetExtents(Screen).
                              Snapshot node.bounds do not count. Empty extents
                              typed-fail (a11y_extents_unavailable).
  select --window HANDLE --name PAT --start N --end M [--role ROLE]
                              one-shot AT-SPI Text.SetSelection(0, start, end).
                              addressing=accessibility-tree via=set-selection.
                              Missing Text / UnknownMethod typed-fails
                              (a11y_selection_unavailable). SetSelection false
                              typed-fails (a11y_selection_no_effect). Never
                              XTest, mouse-drag, --coords, or screenshot. The
                              reply is not proof; observe with get-selection.
  get-selection --window HANDLE --name PAT [--role ROLE]
                              independent AT-SPI Text.GetNSelections +
                              GetSelection(0). Not the select reply payload.
                              Missing Text typed-fails
                              (a11y_selection_unavailable). n=0 is empty
                              success.
  set-caret --window HANDLE --name PAT --offset N [--role ROLE]
                              one-shot AT-SPI Text.SetCaretOffset.
                              addressing=accessibility-tree via=set-caret-offset.
                              Missing Text / UnknownMethod typed-fails
                              (a11y_caret_unavailable). SetCaretOffset false
                              typed-fails (a11y_caret_no_effect). Never
                              XTest, --coords, or screenshot. The reply is
                              not proof; observe with get-caret.
  get-caret --window HANDLE --name PAT [--role ROLE]
                              independent AT-SPI Text.CaretOffset /
                              GetCaretOffset. Not the set-caret reply payload.
                              Missing Text typed-fails
                              (a11y_caret_unavailable).
  get-text --window HANDLE [--name PAT] [--role ROLE]
                              one-shot independent AT-SPI Text.GetText on
                              the unique showing named node, or with no
                              --name on the node carrying the AT-SPI
                              focused state — the same
                              text authority wait --text-equals polls,
                              without a timeout. Not send-text / paste /
                              copy matched.text, last_text_write_via, the
                              WebKit eval helper queued-job OK, or a tree
                              snapshot text. Missing Text typed-fails
                              (a11y_text_unavailable). Never XTest /
                              --coords / screenshot.
  wait --timeout-ms MS (--window-count-gte N | --window-title-contains PAT | --focused-handle HANDLE
                        | --window HANDLE --expect JSON   (same matcher as verify; polls
                          until every item is met, ambiguity / unobservable state fail
                          at once, timeout is typed with the last observation)
                        | --node-name-contains PAT [--node-role ROLE] [--window HANDLE]
                        | --text-equals TEXT --name PAT [--role ROLE] --window HANDLE
                        | --text-contains SUB --name PAT [--role ROLE] --window HANDLE)
                              --text-equals / --node-text-equals and --text-contains /
                              --node-text-contains poll AT-SPI Text.GetText on the unique
                              showing named node until that independent text equals TEXT
                              or contains SUB. send-text / paste / copy matched.text,
                              last_text_write_via, and the WebKit eval helper's queued-job
                              OK are not this condition. Timeout is typed ("timeout")
                              and reports the last GetText. Never screenshot / XTest /
                              --coords. `--` ends flag parsing.
  frame HANDLE|--window H --x X --y Y --width W --height H
                              alias of window-place --action frame
  movewin HANDLE|--window H --x X --y Y
                              alias of window-place move (keeps current size)
  resize HANDLE|--window H --width W --height H
                              alias of window-place resize (keeps current origin)
  maximize HANDLE|--window H  alias of window-place --action fullscreen
  window-place --action <id> [--window HANDLE]
      ids: center|fullscreen|left-half|right-half|top-half|bottom-half
           upper-left|lower-left|upper-right|lower-right
           next-third|previous-third|next-display|previous-display
           larger|smaller|undo|redo
           (or SpectacleWindowAction* constants)
  orderwin --window HANDLE --relation above|below --relative HANDLE
                              MCU relative z-order: above raises --window, below
                              raises --relative, then reads the order back and
                              answers from what it read: window_order_not_applied
                              when the raise did not take. Linux raises with the
                              EWMH _NET_RESTACK_WINDOW (no focus change); macOS
                              AXRaise cannot reorder a background application's
                              windows, so it refuses rather than activating it.

All replies are JSON on stdout: {{"ok":bool,"target":..,"command":..,"data":..,"error":..}}
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_move_cli_parses_explicit_signed_coordinates_before_authorization() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "pointer-move".into(),
            "--to".into(),
            "desktop".into(),
            "--x".into(),
            "-320".into(),
            "--y".into(),
            "1440".into(),
        ]);
        assert!(!reply.ok);
        assert_eq!(reply.target, "current");
        assert_eq!(reply.command, "pointer-move");
        assert_eq!(reply.error.expect("actuate refusal").code, "refused");
    }

    #[test]
    fn pointer_position_cli_requires_observe_before_native_dispatch() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "pointer-position".into(),
        ]);
        assert!(!reply.ok);
        assert_eq!(reply.command, "pointer-position");
        assert_eq!(reply.error.expect("observe refusal").code, "refused");
    }

    #[test]
    fn pointer_move_cli_rejects_missing_overflow_duplicate_and_extra_values() {
        for tail in [
            vec!["--x", "1"],
            vec!["--to", "desktop", "--x", "2147483648", "--y", "0"],
            vec!["--to", "desktop", "--x", "1", "--x", "2", "--y", "3"],
            vec!["--to", "desktop", "--x", "1", "--y", "2", "unexpected"],
        ] {
            let mut args = vec![
                "--target".to_owned(),
                "current".to_owned(),
                "pointer-move".to_owned(),
            ];
            args.extend(tail.into_iter().map(str::to_owned));
            let reply = dispatch(args);
            assert!(!reply.ok);
            assert_eq!(reply.command, "usage");
            assert_eq!(reply.error.expect("typed usage error").code, "usage");
        }
    }

    #[test]
    fn clipboard_read_cli_requires_observe_before_native_dispatch() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "clipboard-read".into(),
        ]);
        assert!(!reply.ok);
        assert_eq!(reply.target, "current");
        assert_eq!(reply.command, "clipboard-read");
        assert_eq!(reply.error.expect("typed refusal").code, "refused");
    }

    #[test]
    fn clipboard_read_type_requires_observe_and_is_not_unknown() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "clipboard-read".into(),
            "--type".into(),
            "«class PNGf»".into(),
        ]);
        assert_eq!(reply.command, "clipboard-read");
        assert_eq!(reply.error.expect("typed refusal").code, "refused");
        let mcu = dispatch(vec![
            "--target".into(),
            "current".into(),
            "clipboard".into(),
            "read".into(),
            "«class PNGf»".into(),
        ]);
        assert_eq!(mcu.command, "clipboard-read");
        assert_ne!(
            mcu.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
            "usage"
        );
    }

    #[test]
    fn clipboard_write_and_clear_are_typed_not_unknown() {
        let write = dispatch(vec![
            "--target".into(),
            "current".into(),
            "clipboard".into(),
            "write".into(),
            "string".into(),
            "/tmp/cu-clip-write.bin".into(),
        ]);
        assert_eq!(write.command, "clipboard-write");
        assert_eq!(write.error.expect("refused").code, "refused");
        let missing = dispatch(vec![
            "--target".into(),
            "current".into(),
            "clipboard-write".into(),
        ]);
        assert_eq!(missing.command, "usage");
        let planned = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "clipboard-clear".into(),
        ]);
        assert_eq!(planned.command, "clipboard-clear");
        assert!(planned.ok, "{:?}", planned.error);
        assert_eq!(planned.data.as_ref().unwrap()["status"], "planned");
        assert_eq!(planned.data.as_ref().unwrap()["applyRequired"], true);
    }

    #[test]
    fn clipboard_read_cli_rejects_extra_arguments() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "clipboard-read".into(),
            "unexpected".into(),
            "extra".into(),
        ]);
        assert!(!reply.ok);
        assert_eq!(reply.command, "usage");
        assert_eq!(reply.error.expect("typed usage error").code, "usage");
    }

    #[test]
    fn malformed_grant_is_typed_without_echoing_its_value() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "credential-shaped-value".into(),
            "clipboard-read".into(),
        ]);
        assert!(!reply.ok);
        assert_eq!(reply.target, "current");
        assert_eq!(reply.command, "clipboard-read");
        let error = reply.error.expect("typed authorization error");
        assert_eq!(error.code, "invalid_authorization");
        assert!(!error.message.contains("credential-shaped-value"));
    }

    #[test]
    fn page_js_cli_is_typed_unsupported_without_eval() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "page-js".into(),
            "--expression".into(),
            "document.title".into(),
            "--port".into(),
            "1".into(),
        ]);
        assert!(!reply.ok);
        assert_eq!(reply.command, "page-js");
        let error = reply.error.expect("typed unsupported");
        assert_eq!(error.code, "unsupported");
        assert!(error.message.contains("remote-debugging-port"));
        let backend = error
            .detail
            .as_ref()
            .and_then(|value| value.get("backend"))
            .and_then(|value| value.as_str())
            .expect("backend");
        assert_eq!(backend, "debugger-runtime-evaluate");
        assert!(!error.message.contains("eval("));
    }

    #[test]
    fn expect_title_includes_without_state_is_not_usage() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "verify".into(),
            "--window".into(),
            "1".into(),
            "--expect".into(),
            r#"[{"role":"AXWebArea","titleIncludes":"Exact Reply"}]"#.into(),
        ]);
        assert_eq!(reply.command, "verify");
        let error = reply.error.expect("window or unverified, not usage");
        assert_ne!(error.code, "usage");
    }

    #[test]
    fn wait_expect_title_includes_is_not_usage() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "wait".into(),
            "--timeout-ms".into(),
            "1".into(),
            "--window".into(),
            "1".into(),
            "--expect".into(),
            r#"[{"role":"AXHeading","titleIncludes":"Nepal"}]"#.into(),
        ]);
        assert_eq!(reply.command, "wait");
        let error = reply.error.expect("timeout or missing window, not usage");
        assert_ne!(error.code, "usage");
    }

    #[test]
    fn query_window_accepts_mcu_app_hash_handle() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "query".into(),
            "--window".into(),
            "Brave Origin#14278".into(),
        ]);
        assert_eq!(reply.command, "query");
        if let Some(error) = reply.error {
            assert_ne!(error.code, "usage");
        }
    }

    #[test]
    fn screenshot_window_flag_is_not_eaten_as_path() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "screenshot".into(),
            "--window".into(),
            "16784".into(),
        ]);
        assert_eq!(reply.command, "screenshot");
        let code = reply
            .error
            .as_ref()
            .map(|error| error.code.as_str())
            .unwrap_or("");
        assert_ne!(code, "usage");
        if let Some(error) = &reply.error {
            assert!(
                !error.message.contains("handle must be non-zero"),
                "screenshot --window 16784 must not drop the handle: {:?}",
                error
            );
        }
    }

    #[test]
    fn inspect_is_live_alias_of_query() {
        let positional = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "inspect".into(),
            "Brave Origin#14278".into(),
        ]);
        assert_eq!(positional.command, "query");
        assert_ne!(
            positional
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or(""),
            "usage",
            "inspect HANDLE must dispatch as query: {:?}",
            positional.error
        );
        let flagged = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "inspect".into(),
            "--window".into(),
            "1".into(),
        ]);
        assert_eq!(flagged.command, "query");
        assert_ne!(
            flagged
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or(""),
            "usage"
        );
        let app = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "inspect".into(),
            "--app".into(),
            "ChatGPT".into(),
        ]);
        assert_eq!(app.command, "usage");
        assert_eq!(app.error.expect("usage").code, "usage");
        let query_positional = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "query".into(),
            "Brave Origin#14278".into(),
        ]);
        assert_eq!(query_positional.command, "query");
        assert_ne!(
            query_positional
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or(""),
            "usage"
        );
    }

    #[test]
    fn find_and_read_are_live_aliases_of_query() {
        let find = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "find".into(),
            "App#1".into(),
            "Save".into(),
            "--role".into(),
            "Button".into(),
        ]);
        assert_eq!(find.command, "query");
        assert_ne!(
            find.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
            "usage",
            "find HANDLE TEXT must dispatch as query: {:?}",
            find.error
        );
        let read = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "read".into(),
            "App#1".into(),
            "AXButton[0]".into(),
        ]);
        assert_eq!(read.command, "query");
        assert_ne!(
            read.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
            "usage",
            "read HANDLE SELECTOR must dispatch as query: {:?}",
            read.error
        );
        let find_missing = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "find".into(),
            "--window".into(),
            "1".into(),
        ]);
        assert_eq!(find_missing.command, "usage");
    }

    #[test]
    fn query_selector_invalid_is_usage() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "query".into(),
            "--window".into(),
            "1".into(),
            "--selector".into(),
            "!!!".into(),
        ]);
        assert_eq!(reply.command, "usage");
        assert_eq!(reply.error.expect("usage").code, "usage");
    }

    #[test]
    fn mcu_align_verbs_are_typed_not_unknown() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "pty".into(),
        ]);
        assert_eq!(reply.command, "pty");
        let err = reply.error.expect("typed");
        assert_eq!(err.code, "unsupported");
        assert!(
            !err.message.contains("unknown MCU group"),
            "{}",
            err.message
        );
        assert!(
            err.message.contains("PTY") || err.message.contains("job"),
            "{}",
            err.message
        );
        let sim = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "simulator".into(),
        ]);
        assert_eq!(sim.command, "simulator");
        assert_eq!(sim.error.expect("typed").code, "unsupported");
        let watch = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "windows-watch".into(),
            "--duration-ms".into(),
            "0".into(),
            "--interval-ms".into(),
            "0".into(),
        ]);
        assert_eq!(watch.command, "windows-watch");
        assert_ne!(
            watch.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
            "usage"
        );
        if watch.ok {
            assert_eq!(watch.data.as_ref().unwrap()["mode"], "poll-diff");
        }
        let apps = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "apps".into(),
            "--running".into(),
        ]);
        assert_eq!(apps.command, "apps");
        assert_ne!(
            apps.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
            "usage"
        );
        let order = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "orderwin".into(),
            "--window".into(),
            "1".into(),
            "--relation".into(),
            "above".into(),
            "--relative".into(),
            "1".into(),
        ]);
        assert_eq!(order.command, "orderwin");
        assert_eq!(order.error.expect("same handle").code, "invalid_input");
        let missing = dispatch(vec![
            "--target".into(),
            "current".into(),
            "orderwin".into(),
            "--window".into(),
            "1".into(),
        ]);
        assert_eq!(missing.command, "usage");
        for (verb, command) in [
            ("caps", "capabilities"),
            ("displays", "displays"),
            ("cursor", "pointer-position"),
            ("clip", "clipboard-read"),
            ("clipboard", "clipboard-read"),
            ("shot", "screenshot"),
            ("elements", "tree"),
            ("move", "pointer-move"),
            ("inspect", "query"),
        ] {
            let mut argv = vec![
                "--target".into(),
                "current".into(),
                "--grant".into(),
                "observe".into(),
                verb.into(),
            ];
            if verb == "move" {
                argv.extend([
                    "--to".into(),
                    "desktop".into(),
                    "--x".into(),
                    "0".into(),
                    "--y".into(),
                    "0".into(),
                ]);
            }
            if verb == "inspect" {
                argv.extend(["--window".into(), "1".into()]);
            }
            let reply = dispatch(argv);
            assert_eq!(reply.command, command, "{verb}");
            assert_ne!(
                reply.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
                "usage",
                "{verb} must not be unknown: {:?}",
                reply.error
            );
        }
        let dclick = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "dclick".into(),
            "--window".into(),
            "1".into(),
            "--name".into(),
            "Ok".into(),
        ]);
        assert_eq!(dclick.command, "click");
        assert_ne!(
            dclick.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
            "usage",
            "dclick must not be unknown: {:?}",
            dclick.error
        );
        let launch = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "launch".into(),
            "/Applications/Nonexistent.app".into(),
        ]);
        assert_eq!(launch.command, "app");
        assert_ne!(
            launch.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
            "usage",
            "launch must not be unknown: {:?}",
            launch.error
        );
        let page = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "page".into(),
            "targets".into(),
        ]);
        assert_eq!(page.command, "page");
        let page_err = page.error.expect("typed page");
        assert_eq!(page_err.code, "unsupported");
        assert!(
            !page_err.message.contains("unknown"),
            "{}",
            page_err.message
        );
        let page_js = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "page".into(),
            "read".into(),
            "--js".into(),
            "1+1".into(),
        ]);
        assert_eq!(page_js.command, "page-js");
        assert_ne!(
            page_js
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or(""),
            "usage"
        );
        let drag = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "drag".into(),
        ]);
        assert_eq!(drag.command, "drag");
        let drag_err = drag.error.expect("typed drag");
        assert_eq!(drag_err.code, "unsupported", "{drag_err:?}");
        assert!(
            !drag_err.message.contains("unknown"),
            "{}",
            drag_err.message
        );
        let frame = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "frame".into(),
            "1".into(),
            "10".into(),
            "20".into(),
            "300".into(),
            "200".into(),
        ]);
        assert_eq!(frame.command, "window-place");
        assert_ne!(
            frame.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
            "usage",
            "frame must not be unknown: {:?}",
            frame.error
        );
        let movewin = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "movewin".into(),
            "--window".into(),
            "1".into(),
            "--x".into(),
            "40".into(),
            "--y".into(),
            "50".into(),
        ]);
        assert_eq!(movewin.command, "window-place");
        assert_ne!(
            movewin
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or(""),
            "usage",
            "movewin must not be unknown: {:?}",
            movewin.error
        );
        let maximize = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "maximize".into(),
            "--window".into(),
            "1".into(),
        ]);
        assert_eq!(maximize.command, "window-place");
        assert_ne!(
            maximize
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or(""),
            "usage"
        );
        let write = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "clipboard".into(),
            "write".into(),
            "--type".into(),
            "string".into(),
            "--path".into(),
            "/tmp/cu-clip-write.bin".into(),
        ]);
        assert_eq!(write.command, "clipboard-write");
        assert_ne!(
            write.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
            "usage"
        );
    }

    #[test]
    fn pointer_move_without_to_is_usage() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "pointer-move".into(),
            "--x".into(),
            "1".into(),
            "--y".into(),
            "2".into(),
        ]);
        assert_eq!(reply.command, "usage");
    }

    #[test]
    fn invoke_extra_mcu_actions_parse() {
        let reply = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "invoke".into(),
            "--window".into(),
            "1".into(),
            "--selector".into(),
            "AXButton[0]".into(),
            "scroll-to".into(),
        ]);
        assert_eq!(reply.command, "invoke");
        assert_ne!(reply.error.expect("live or unsupported").code, "usage");
    }
}
