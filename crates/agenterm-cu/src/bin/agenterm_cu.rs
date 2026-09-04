//! `agenterm-cu` shell command (PRD_02_29 shell layer).
//!
//! Machine-readable JSON on stdout; human usage on stderr. Verb parsing lives
//! in `cli/`, one module per family, all keyed by the verb table in
//! `cli/verbs.rs`; this file only routes.

mod cli;

use agenterm_cu::{Command, CuReply};

use cli::global::{Globals, authority_environment_flags};
use cli::verbs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.as_slice(), [arg] if arg == "--version" || arg == "-V") {
        println!("agenterm-cu {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match args
        .first()
        .and_then(|first| verbs::lookup(first))
        .map(|spec| spec.name)
    {
        Some("host") => std::process::exit(agenterm_cu::hotkeys::run()),
        Some("verbs") => {
            match cli::help::run_verbs(&args[1..]) {
                Ok(text) => print!("{text}"),
                Err(reply) => print_reply(&reply),
            }
            return;
        }
        _ => {}
    }
    if args.first().map(String::as_str)
        == Some(agenterm_cu::mechanism::clipboard::X11_CLIPBOARD_OWNER_ARG)
    {
        std::process::exit(run_x11_clipboard_owner());
    }
    print_reply(&dispatch(args));
}

fn print_reply(reply: &CuReply) {
    let json = serde_json::to_string(reply).unwrap_or_else(|_| {
        r#"{"ok":false,"target":"","command":"","error":{"code":"serialize","message":"reply serialization failed"}}"#
            .to_string()
    });
    println!("{json}");
}

fn is_help_token(token: &str) -> bool {
    matches!(token, "--help" | "-h") || verbs::lookup(token).is_some_and(|spec| spec.name == "help")
}

/// `<verb> --help` (or `-h`), also after a two-token spelling such as
/// `menu inspect --help`. Nothing else may follow, so a verb whose free text
/// happens to contain `-h` is never mistaken for a help request.
fn wants_verb_help(args: &[String]) -> bool {
    let tail = &args[1..];
    let sub_form = tail.first().is_some_and(|second| {
        verbs::resolve(&args[0], Some(second)).is_some_and(|spec| {
            spec.aliases
                .contains(&format!("{} {second}", args[0]).as_str())
        })
    });
    let tail = if sub_form { &tail[1..] } else { tail };
    tail.len() == 1 && matches!(tail[0].as_str(), "--help" | "-h")
}

fn dispatch(mut args: Vec<String>) -> CuReply {
    let (ambient_authority_present, unsupported_authority_environment) =
        authority_environment_flags();
    // `<verb> --help` for the entry modes too (`grant --help`, `exec --help`).
    if let Some(spec) = args
        .first()
        .and_then(|first| verbs::resolve(first, args.get(1).map(String::as_str)))
        && wants_verb_help(&args)
    {
        return cli::help::verb_help(spec);
    }
    if let Some(reply) = agenterm_cu::grant_management::dispatch(&args, ambient_authority_present) {
        return reply;
    }
    if args.first().is_none_or(|first| is_help_token(first)) {
        return cli::help::run_help(args.get(1..).unwrap_or(&[]));
    }

    let mut globals = match Globals::parse(&mut args) {
        Ok(globals) => globals,
        Err(reply) => return *reply,
    };
    let spec = args
        .first()
        .and_then(|first| verbs::resolve(first, args.get(1).map(String::as_str)));
    match spec.map(|spec| spec.name) {
        // Global flags may precede `exec` so remote workers can be invoked as
        // `agenterm-cu --grant observe exec --json -` as well as `exec` first.
        Some("exec") => {
            return cli::exec::dispatch_json(&globals.exec_args(args.into_iter().skip(1)));
        }
        Some("help") => return cli::help::run_help(&args[1..]),
        _ => {}
    }
    if let Some(spec) = spec
        && wants_verb_help(&args)
    {
        return cli::help::verb_help(spec);
    }

    let target = match globals.resolve_target() {
        Ok(target) => target,
        Err(reply) => return *reply,
    };

    let Some(spelled) = args.first().cloned() else {
        return cli::usage_err("missing command verb");
    };
    args.remove(0);

    let command = match spec {
        Some(spec) => match cli::parse_command(spec, &spelled, target, &mut args) {
            Ok(command) => command,
            Err(message) => return cli::usage_err_for(spec, message),
        },
        None if agenterm_cu::mcu_surface::is_align_verb(&spelled) => Command::Align {
            target,
            group: spelled,
        },
        None => {
            let near = verbs::near_matches(&spelled);
            return cli::usage_err(if near.is_empty() {
                format!("unknown command '{spelled}'")
            } else {
                format!(
                    "unknown command '{spelled}'; near matches: {}",
                    near.join(", ")
                )
            });
        }
    };

    let executor = match globals.executor(
        target,
        &command,
        ambient_authority_present,
        unsupported_authority_environment,
    ) {
        Ok(executor) => executor,
        Err(reply) => return *reply,
    };
    executor.execute(&command)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_cli_carries_the_baseline_ready_path_in_its_closed_shape() {
        let spec = verbs::lookup("observe").expect("observe verb");
        let mut args = [
            "--window",
            "41",
            "--duration-ms",
            "900",
            "--ready-path",
            "observe-ready.json",
        ]
        .map(str::to_owned)
        .to_vec();
        let command =
            cli::parse_command(spec, "observe", agenterm_cu::TargetRef::Current, &mut args)
                .expect("closed observe shape");
        assert!(matches!(
            command,
            Command::Observe {
                window: 41,
                duration_ms: 900,
                ready_path: Some(ref path),
                ..
            } if path == "observe-ready.json"
        ));
        let mut unknown = ["--window", "41", "--duration-ms", "900", "--ready"]
            .map(str::to_owned)
            .to_vec();
        assert!(
            cli::parse_command(
                spec,
                "observe",
                agenterm_cu::TargetRef::Current,
                &mut unknown
            )
            .is_err()
        );
    }

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
    fn page_js_target_selectors_are_exclusive_and_typed() {
        let base = |extra: &[&str]| {
            let mut argv: Vec<String> = [
                "--target",
                "current",
                "--grant",
                "observe",
                "page-js",
                "--expression",
                "document.title",
                "--port",
                "1",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
            argv.extend(extra.iter().map(|s| (*s).to_owned()));
            dispatch(argv)
        };
        let two = base(&["--target-id", "A1", "--target-url", "mail"]);
        let err = two.error.expect("usage");
        assert_eq!(err.code, "usage");
        assert!(err.message.contains("at most one"), "{}", err.message);
        let empty = base(&["--target-title", ""]);
        assert_eq!(empty.error.expect("usage").code, "usage");
        let missing = base(&["--target-title"]);
        assert_eq!(missing.error.expect("usage").code, "usage");
        // One selector parses and reaches the (absent) listener typed.
        let one = base(&["--target-title", "Inbox"]);
        assert_eq!(one.command, "page-js");
        let err = one.error.expect("typed unsupported");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("remote-debugging-port"));
    }

    #[test]
    fn page_targets_is_a_typed_cdp_observe_verb() {
        for argv in [
            vec!["page", "targets", "--port", "1"],
            vec!["page-targets", "--port", "1"],
        ] {
            let mut full: Vec<String> = ["--target", "current", "--grant", "observe"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            full.extend(argv.iter().map(|s| (*s).to_owned()));
            let reply = dispatch(full);
            assert_eq!(reply.command, "page-targets");
            let err = reply.error.expect("typed unsupported");
            assert_eq!(err.code, "unsupported");
            assert!(err.message.contains("remote-debugging-port"));
        }
        let extra = dispatch(
            [
                "--target", "current", "--grant", "observe", "page", "targets", "--bogus",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        );
        assert_eq!(extra.error.expect("usage").code, "usage");
    }

    #[test]
    fn page_text_parses_a_closed_observe_shape() {
        let run = |argv: &[&str]| dispatch(argv.iter().map(|s| (*s).to_owned()).collect());
        let no_window = run(&["--target", "current", "--grant", "observe", "page", "text"]);
        assert_eq!(no_window.error.expect("usage").code, "usage");
        let bad_bytes = run(&[
            "--target",
            "current",
            "--grant",
            "observe",
            "page-text",
            "--window",
            "7",
            "--max-bytes",
            "0",
        ]);
        assert_eq!(bad_bytes.error.expect("usage").code, "usage");
        let bad_within = run(&[
            "--target", "current", "--grant", "observe", "page", "text", "--window", "7",
            "--within", "1,2,3",
        ]);
        assert_eq!(bad_within.error.expect("usage").code, "usage");
        let bad_depth = run(&[
            "--target", "current", "--grant", "observe", "page", "text", "--window", "7",
            "--depth", "65",
        ]);
        assert_eq!(bad_depth.error.expect("usage").code, "usage");
        let extra = run(&[
            "--target", "current", "--grant", "observe", "page", "text", "--window", "7", "--bogus",
        ]);
        assert_eq!(extra.error.expect("usage").code, "usage");
        for argv in [
            vec![
                "page",
                "text",
                "--window",
                "7",
                "--max-bytes",
                "4096",
                "--within",
                "0,0,10,10",
            ],
            vec!["page-text", "--window", "7"],
        ] {
            let mut full: Vec<String> = ["--target", "current", "--grant", "observe"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            full.extend(argv.iter().map(|s| (*s).to_owned()));
            let reply = dispatch(full);
            assert_eq!(reply.command, "page-text");
            // Window 7 is not a real handle: the mechanism answers typed,
            // and it is never a usage error.
            assert_ne!(reply.error.as_ref().map(|e| e.code.as_str()), Some("usage"));
        }
    }

    #[test]
    fn tab_verbs_parse_closed_shapes() {
        let run = |argv: &[&str]| dispatch(argv.iter().map(|s| (*s).to_owned()).collect());
        let no_window = run(&["--target", "current", "--grant", "observe", "tab", "list"]);
        assert_eq!(no_window.error.expect("usage").code, "usage");
        let no_sub = run(&["--target", "current", "--grant", "observe", "tab"]);
        assert_eq!(no_sub.error.expect("usage").code, "usage");
        let bad_sub = run(&[
            "--target", "current", "--grant", "observe", "tab", "bogus", "--window", "7",
        ]);
        assert_eq!(bad_sub.error.expect("usage").code, "usage");
        // `tab close` is live: its shape is closed, and the executor's
        // destructive gate (not the parser) refuses a bare one.
        let close_extra = run(&[
            "--target", "current", "--grant", "actuate", "tab", "close", "--window", "7",
            "--title", "x", "--exact", "--expect", "gone", "--bogus",
        ]);
        assert_eq!(close_extra.error.expect("usage").code, "usage");
        let close_bare = run(&[
            "--target",
            "current",
            "--grant",
            "actuate",
            "tab-close",
            "--window",
            "7",
        ]);
        assert_eq!(close_bare.command, "tab-close");
        let err = close_bare.error.expect("refused");
        assert_eq!(err.code, "refused");
        assert_eq!(err.detail.expect("detail")["reason"], "destructive_gate");
        // `--index N` is the exact selector for same-title duplicates and
        // `--port N` names the CDP listener; both parse and reach the
        // executor (window 7 is not a real handle).
        let close_index = run(&[
            "--target", "current", "--grant", "actuate", "tab", "close", "--window", "7",
            "--index", "2", "--expect", "gone", "--port", "9222",
        ]);
        assert_eq!(close_index.command, "tab-close");
        let err = close_index.error.expect("no such window");
        assert_ne!(err.code, "usage");
        assert_ne!(
            err.detail.as_ref().and_then(|d| d["reason"].as_str()),
            Some("destructive_gate")
        );
        let close_both = run(&[
            "--target", "current", "--grant", "actuate", "tab", "close", "--window", "7",
            "--title", "x", "--exact", "--index", "2", "--expect", "gone",
        ]);
        let err = close_both.error.expect("usage");
        assert_eq!(err.code, "usage");
        assert!(err.message.contains("not both"), "{}", err.message);
        let close_bad_port = run(&[
            "--target",
            "current",
            "--grant",
            "actuate",
            "tab-close",
            "--window",
            "7",
            "--index",
            "2",
            "--expect",
            "gone",
            "--port",
            "x",
        ]);
        assert_eq!(close_bad_port.error.expect("usage").code, "usage");
        // `focused-window` is `windows --focused true`; `--focused false`
        // contradicts it.
        let focused_window = run(&[
            "--target",
            "current",
            "--grant",
            "observe",
            "focused-window",
        ]);
        assert_eq!(focused_window.command, "windows");
        if let Some(data) = focused_window.data.as_ref() {
            assert_eq!(data["filter"]["focused"], true);
            assert!(data.get("focused_app").is_some(), "{data}");
            assert!(data.get("window").is_some(), "{data}");
        }
        let contradiction = run(&[
            "--target",
            "current",
            "--grant",
            "observe",
            "focused-window",
            "--focused",
            "false",
        ]);
        assert_eq!(contradiction.error.expect("usage").code, "usage");
        // `browser` group word and its two sub-commands.
        let no_browser_sub = run(&["--target", "current", "--grant", "observe", "browser"]);
        assert_eq!(no_browser_sub.error.expect("usage").code, "usage");
        let open_no_profile = run(&[
            "--target", "current", "--grant", "actuate", "browser", "open", "--url", "x",
        ]);
        assert_eq!(open_no_profile.error.expect("usage").code, "usage");
        let open_switch_url = run(&[
            "--target",
            "current",
            "--grant",
            "actuate",
            "browser-open",
            "--profile",
            "p",
            "--url",
            "--incognito",
        ]);
        assert_eq!(open_switch_url.error.expect("usage").code, "usage");
        let profiles_extra = run(&[
            "--target", "current", "--grant", "observe", "browser", "profiles", "--bogus",
        ]);
        assert_eq!(profiles_extra.error.expect("usage").code, "usage");
        let profiles = run(&[
            "--target", "current", "--grant", "observe", "browser", "profiles", "--app", "Safari",
        ]);
        assert_eq!(profiles.command, "browser-profiles");
        assert_ne!(
            profiles.error.as_ref().map(|e| e.code.as_str()),
            Some("usage")
        );
        let filtered = run(&[
            "--target",
            "current",
            "--grant",
            "observe",
            "windows",
            "--browser-profile",
            "",
        ]);
        assert_eq!(filtered.error.expect("usage").code, "usage");
        let targets_join = run(&[
            "--target",
            "current",
            "--grant",
            "observe",
            "page",
            "targets",
            "--port",
            "1",
            "--browser-profile",
            "zzz-no-such-profile",
        ]);
        assert_eq!(targets_join.command, "page-targets");
        assert_ne!(
            targets_join.error.as_ref().map(|e| e.code.as_str()),
            Some("usage")
        );
        let both = run(&[
            "--target", "current", "--grant", "actuate", "tab", "select", "--window", "7",
            "--title", "Codex", "--index", "1",
        ]);
        let err = both.error.expect("usage");
        assert_eq!(err.code, "usage");
        assert!(err.message.contains("not both"), "{}", err.message);
        let neither = run(&[
            "--target", "current", "--grant", "actuate", "tab", "select", "--window", "7",
        ]);
        assert_eq!(neither.error.expect("usage").code, "usage");
        let bad_index = run(&[
            "--target",
            "current",
            "--grant",
            "actuate",
            "tab-select",
            "--window",
            "7",
            "--index",
            "x",
        ]);
        assert_eq!(bad_index.error.expect("usage").code, "usage");
        // Well-formed shapes are not usage errors: they reach the
        // authorization / mechanism layer (window 7 is not a real handle).
        let list = run(&[
            "--target", "current", "--grant", "observe", "tab-list", "--window", "7",
        ]);
        assert_eq!(list.command, "tab-list");
        assert_ne!(list.error.as_ref().map(|e| e.code.as_str()), Some("usage"));
        let select = run(&[
            "--target", "current", "--grant", "observe", "tab", "select", "--window", "7",
            "--index", "0",
        ]);
        assert_eq!(select.command, "tab-select");
        let err = select.error.expect("observe cannot actuate");
        assert_ne!(err.code, "usage");
        assert_ne!(err.code, "invalid_input");
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
            "zoom".into(),
        ]);
        assert_eq!(page.command, "page");
        let page_err = page.error.expect("typed page");
        assert_eq!(page_err.code, "unsupported");
        assert!(
            !page_err.message.contains("unknown"),
            "{}",
            page_err.message
        );
        assert!(
            page_err.message.contains("page find/click/hover/scroll"),
            "{}",
            page_err.message
        );
        // The mapped page verbs are live CDP verbs now: a
        // missing listener is typed, never `usage` or "unknown".
        for (argv, command) in [
            (
                vec![
                    "page",
                    "click",
                    "--port",
                    "1",
                    "--target-title",
                    "x",
                    "--text",
                    "Go",
                ],
                "page-click",
            ),
            (
                vec!["page", "hover", "--port", "1", "--x", "10", "--y", "20"],
                "page-hover",
            ),
            (
                vec![
                    "page", "scroll", "--port", "1", "--x", "10", "--y", "20", "--dy", "120",
                ],
                "page-scroll",
            ),
            (
                vec![
                    "page",
                    "nav",
                    "--port",
                    "1",
                    "--target-title",
                    "x",
                    "--url",
                    "https://docs.example/",
                ],
                "page-nav",
            ),
            (
                vec!["page", "read", "--port", "1", "--target-title", "x"],
                "page-text",
            ),
            (
                vec!["page", "find", "--port", "1", "--selector", "#q"],
                "page-find",
            ),
            (
                vec![
                    "page",
                    "fill",
                    "--port",
                    "1",
                    "--selector",
                    "#q",
                    "--text",
                    "hi",
                ],
                "page-fill",
            ),
            (
                vec![
                    "page",
                    "screenshot",
                    "--port",
                    "1",
                    "--out",
                    "cu-smoke-none.png",
                ],
                "page-screenshot",
            ),
        ] {
            let mut full: Vec<String> = ["--target", "current", "--grant", "observe,actuate"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            full.extend(argv.iter().map(|s| (*s).to_owned()));
            let reply = dispatch(full);
            assert_eq!(reply.command, command, "{argv:?}");
            let err = reply.error.expect("typed");
            assert_eq!(err.code, "unsupported", "{argv:?}: {}", err.message);
            assert!(
                err.message.contains("remote-debugging-port"),
                "{}",
                err.message
            );
        }
        // Addressing shape is judged before any socket: usage, by name.
        let two = dispatch(
            [
                "--target",
                "current",
                "--grant",
                "actuate",
                "page",
                "click",
                "--port",
                "1",
                "--selector",
                "#a",
                "--text",
                "b",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        );
        let err = two.error.expect("usage");
        assert_eq!(err.code, "usage");
        assert!(err.message.contains("exactly one"), "{}", err.message);
        let name_alone = dispatch(
            [
                "--target",
                "current",
                "--grant",
                "observe",
                "page",
                "find",
                "--port",
                "1",
                "--selector",
                "#a",
                "--name",
                "b",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        );
        assert_eq!(name_alone.error.expect("usage").code, "usage");
        let no_text = dispatch(
            [
                "--target",
                "current",
                "--grant",
                "actuate",
                "page",
                "fill",
                "--port",
                "1",
                "--selector",
                "#a",
                "--text",
                "",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        );
        let err = no_text.error.expect("usage");
        assert_eq!(err.code, "usage");
        assert!(err.message.contains("--clear"), "{}", err.message);
        let both_backends = dispatch(
            [
                "--target", "current", "--grant", "observe", "page", "text", "--window", "7",
                "--port", "1",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        );
        assert_eq!(both_backends.command, "page-text");
        assert_eq!(both_backends.error.expect("typed").code, "invalid_input");
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
        // `ghost` is the MCU leaf this binary deliberately did NOT absorb
        // (it draws a cursor overlay on the desktop), so it stays typed
        // rather than silently unknown.
        let ghost = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "observe".into(),
            "ghost".into(),
        ]);
        assert_eq!(ghost.command, "ghost");
        let ghost_err = ghost.error.expect("typed ghost");
        assert_eq!(ghost_err.code, "unsupported", "{ghost_err:?}");
        assert!(
            !ghost_err.message.contains("unknown"),
            "{}",
            ghost_err.message
        );
        // `drag` used to answer here as a typed-unsupported MCU leaf; it is
        // a live verb now, so a bare `drag` is a usage error naming its own
        // flags, never "unknown command" and never the align reply.
        let drag = dispatch(vec![
            "--target".into(),
            "current".into(),
            "--grant".into(),
            "actuate".into(),
            "drag".into(),
        ]);
        assert_eq!(drag.command, "usage");
        let drag_err = drag.error.expect("usage");
        assert!(
            drag_err.message.contains("drag requires --window"),
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

/// The verb table is the surface: every spelling dispatches, every verb has
/// help, and the entry modes stay where they were.
#[cfg(test)]
mod surface_tests {
    use super::*;

    fn run(argv: &[&str]) -> CuReply {
        dispatch(argv.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn help_succeeds_for_every_verb_and_spelling() {
        for spec in verbs::VERBS {
            for spelling in spec.spellings() {
                let mut argv = vec!["help"];
                argv.extend(spelling.split(' '));
                let reply = run(&argv);
                assert!(reply.ok, "help {spelling}: {:?}", reply.error);
                assert_eq!(reply.command, "help");
                assert_eq!(reply.data.as_ref().unwrap()["verb"], spec.name);
            }
        }
        let bare = run(&["help"]);
        assert!(bare.ok);
        assert_eq!(bare.data.as_ref().unwrap()["usage"], "see stderr");
        let flag = run(&["--help"]);
        assert!(flag.ok);
        let empty = dispatch(Vec::new());
        assert!(empty.ok);
        assert_eq!(empty.command, "help");
    }

    #[test]
    fn verb_dash_dash_help_is_help_for_every_verb() {
        for spec in verbs::VERBS {
            for spelling in spec.spellings() {
                let mut argv: Vec<&str> = spelling.split(' ').collect();
                argv.push("--help");
                let reply = run(&argv);
                assert!(reply.ok, "{spelling} --help: {:?}", reply.error);
                assert_eq!(reply.command, "help");
                assert_eq!(reply.data.as_ref().unwrap()["verb"], spec.name);
            }
        }
        // With a target the same request still answers help, not usage.
        let targeted = run(&["--target", "current", "page-text", "-h"]);
        assert!(targeted.ok, "{:?}", targeted.error);
        assert_eq!(targeted.command, "help");
        // A literal `-h` inside free text is not a help request.
        let typed = run(&[
            "--target",
            "current",
            "send-text",
            "--window",
            "1",
            "-h",
            "x",
        ]);
        assert_eq!(typed.command, "send-text");
    }

    #[test]
    fn unknown_help_and_unknown_command_are_typed_with_near_matches() {
        let help = run(&["help", "windws"]);
        assert!(!help.ok);
        assert_eq!(help.command, "usage");
        let error = help.error.expect("usage");
        assert_eq!(error.code, "usage");
        assert!(error.message.contains("windows"), "{}", error.message);
        let command = run(&["--target", "current", "clipbord"]);
        assert_eq!(command.command, "usage");
        let error = command.error.expect("usage");
        assert!(
            error.message.contains("unknown command"),
            "{}",
            error.message
        );
        assert!(error.message.contains("clipboard"), "{}", error.message);
    }

    #[test]
    fn entry_modes_are_not_target_commands() {
        for verb in ["verbs", "grant", "host", "hotkeys"] {
            let reply = run(&["--target", "current", verb]);
            assert_eq!(reply.command, "usage", "{verb}");
            assert_eq!(reply.error.expect("usage").code, "usage", "{verb}");
        }
        // `help` after a target is still help.
        let help = run(&["--target", "current", "help", "tree"]);
        assert!(help.ok);
        assert_eq!(help.data.as_ref().unwrap()["verb"], "tree");
    }

    #[test]
    fn every_single_token_alias_dispatches_as_its_canonical_command() {
        // Minimal valid argv per verb so the alias reaches the executor with
        // the canonical `reply.command` (window 1 is never a real handle, so
        // the mechanism answers typed, never usage).
        let argv_for = |name: &str| -> Vec<&'static str> {
            match name {
                "capabilities" | "pointer-position" | "displays" | "spaces" | "clipboard-read"
                | "screenshot" => vec![],
                "tree" => vec!["--window", "1"],
                "query" => vec!["--window", "1"],
                "click" => vec!["--window", "1", "--name", "Ok"],
                "send-text" => vec!["--window", "1", "hello"],
                "send-keys" => vec!["--window", "1", "enter"],
                "pointer-move" => vec!["--to", "desktop", "--x", "0", "--y", "0"],
                "app" => vec!["--window", "1"],
                _ => vec![],
            }
        };
        for spec in verbs::VERBS {
            if matches!(spec.family, verbs::Family::Transports | verbs::Family::Host) {
                continue;
            }
            for alias in spec.aliases.iter().copied() {
                if alias.contains(' ') {
                    continue;
                }
                let mut argv = vec!["--target", "current", "--grant", "observe,actuate", alias];
                if spec.name == "app" && alias == "launch" {
                    argv.push("/Applications/Nonexistent.app");
                } else if spec.name == "query" && alias == "find" {
                    argv.extend(["1", "Save"]);
                } else if spec.name == "query" && alias == "read" {
                    argv.extend(["1", "AXButton[0]"]);
                } else {
                    argv.extend(argv_for(spec.name));
                }
                let reply = run(&argv);
                assert_eq!(reply.command, spec.command, "{alias} -> {}", spec.name);
                assert_ne!(
                    reply.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
                    "usage",
                    "{alias}: {:?}",
                    reply.error
                );
            }
        }
    }

    #[test]
    fn placement_shorthands_answer_as_window_place() {
        for spec in verbs::by_family(verbs::Family::Placement) {
            if spec.command == spec.name {
                continue;
            }
            let mut argv = vec!["--target", "current", "--grant", "actuate", spec.name, "1"];
            match spec.name {
                "frame" => argv.extend(["0", "0", "10", "10"]),
                "movewin" => argv.extend(["0", "0"]),
                "resize" => argv.extend(["10", "10"]),
                _ => {}
            }
            let reply = run(&argv);
            assert_eq!(reply.command, "window-place", "{}", spec.name);
            assert_ne!(
                reply.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
                "usage",
                "{}: {:?}",
                spec.name,
                reply.error
            );
        }
    }

    #[test]
    fn verbs_json_round_trips_through_the_cli_surface() {
        let text = cli::help::run_verbs(&["--json".to_owned()]).expect("json");
        let rows: Vec<verbs::VerbJson> = serde_json::from_str(&text).expect("parse");
        assert_eq!(rows, verbs::table_json());
        assert_eq!(rows.len(), verbs::VERBS.len());
    }

    /// The desktop-ring verbs parse into their own closed shapes, and
    /// every one of them refuses an unknown flag before a target is touched.
    #[test]
    fn desktop_ring_verbs_parse_closed_shapes() {
        use agenterm_cu::TargetRef;
        let parse = |argv: &[&str]| -> Result<Command, String> {
            let spec = verbs::lookup(argv[0]).expect("verb");
            let mut args: Vec<String> = argv[1..].iter().map(|s| (*s).to_owned()).collect();
            cli::parse_command(spec, argv[0], TargetRef::Current, &mut args)
        };
        match parse(&["activate", "--window", "42"]).expect("activate") {
            Command::Activate { window, .. } => assert_eq!(window, 42),
            other => panic!("{other:?}"),
        }
        match parse(&["raise", "--window", "42"]).expect("raise") {
            Command::Raise { window, .. } => assert_eq!(window, 42),
            other => panic!("{other:?}"),
        }
        match parse(&["minimize", "--window", "42", "--expect", "minimized"]).expect("minimize") {
            Command::Minimize { window, expect, .. } => {
                assert_eq!(window, 42);
                assert_eq!(expect.as_deref(), Some("minimized"));
            }
            other => panic!("{other:?}"),
        }
        // A missing gate part is NOT a usage error: it parses, so the
        // executor can name every missing part in one typed refusal.
        match parse(&["restore", "--window", "42"]).expect("restore") {
            Command::Restore { window, expect, .. } => {
                assert_eq!(window, 42);
                assert_eq!(expect, None);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["minimize"]).expect("bare minimize") {
            Command::Minimize { window, expect, .. } => {
                assert_eq!(window, 0);
                assert_eq!(expect, None);
            }
            other => panic!("{other:?}"),
        }
        match parse(&[
            "drag",
            "--window",
            "42",
            "--from",
            "10,20",
            "--to",
            "30,40",
            "--button",
            "right",
            "--steps",
            "5",
            "--degraded",
        ])
        .expect("drag")
        {
            Command::Drag {
                window,
                from,
                to,
                button,
                steps,
                degraded,
                ..
            } => {
                assert_eq!(window, 42);
                assert_eq!(from, [10, 20]);
                assert_eq!(to, [30, 40]);
                assert_eq!(button, agenterm_cu::PointerButton::Right);
                assert_eq!(steps, Some(5));
                assert!(degraded);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["drag", "--window", "42", "--from", "1,2", "--to", "3,4"]).expect("drag") {
            Command::Drag {
                button,
                steps,
                degraded,
                ..
            } => {
                assert_eq!(button, agenterm_cu::PointerButton::Left);
                assert_eq!(steps, None);
                assert!(!degraded, "--degraded is never implied");
            }
            other => panic!("{other:?}"),
        }
        match parse(&["hit", "--window", "42", "--x", "-3", "--y", "7"]).expect("hit") {
            Command::Hit {
                window,
                x,
                y,
                depth,
                ..
            } => {
                assert_eq!((window, x, y), (42, -3, 7));
                assert_eq!(depth, None);
            }
            other => panic!("{other:?}"),
        }
        match parse(&[
            "zoom",
            "--window",
            "42",
            "--region",
            "1,2,30,40",
            "--out",
            "/tmp/z.png",
            "--pad",
            "4",
            "--replace",
        ])
        .expect("zoom")
        {
            Command::Zoom {
                window,
                region,
                out,
                replace,
                pad,
                ..
            } => {
                assert_eq!(window, 42);
                assert_eq!(region, [1, 2, 30, 40]);
                assert_eq!(out, "/tmp/z.png");
                assert!(replace);
                assert_eq!(pad, Some(4));
            }
            other => panic!("{other:?}"),
        }
        match parse(&[
            "snapshot",
            "--window",
            "42",
            "--depth",
            "6",
            "--max-nodes",
            "500",
        ])
        .expect("snapshot")
        {
            Command::Snapshot {
                window,
                depth,
                max_nodes,
                out,
                ..
            } => {
                assert_eq!(window, 42);
                assert_eq!(depth, Some(6));
                assert_eq!(max_nodes, Some(500));
                assert_eq!(out, None);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["diff", "--window", "42", "--base", "1-2-3", "--advance"]).expect("diff") {
            Command::Diff {
                window,
                base,
                advance,
                max,
                ..
            } => {
                assert_eq!(window, 42);
                assert_eq!(base.as_deref(), Some("1-2-3"));
                assert!(advance);
                assert_eq!(max, None);
            }
            other => panic!("{other:?}"),
        }
        // Every one of them is a closed shape: a stray flag is a usage
        // error, never a silently ignored argument.
        for argv in [
            &["raise", "--window", "1", "--force"][..],
            &[
                "minimize",
                "--window",
                "1",
                "--expect",
                "minimized",
                "--now",
            ][..],
            &["restore", "--window", "1", "--expect", "restored", "--now"][..],
            &[
                "drag", "--window", "1", "--from", "1,2", "--to", "3,4", "--fast",
            ][..],
            &["hit", "--window", "1", "--x", "1", "--y", "2", "--deep"][..],
            &[
                "zoom", "--window", "1", "--region", "1,2,3,4", "--out", "z", "--crop",
            ][..],
            &["snapshot", "--window", "1", "--label", "x"][..],
            &["diff", "--window", "1", "--since", "x"][..],
        ] {
            assert!(parse(argv).is_err(), "{argv:?} must be a usage error");
        }
        // A missing required value is a usage error too, and a malformed
        // rectangle never reaches a capture.
        for argv in [
            &["raise"][..],
            &["drag", "--window", "1", "--to", "3,4"][..],
            &["drag", "--window", "1", "--from", "1,2"][..],
            &[
                "drag", "--window", "1", "--from", "1,2", "--to", "3,4", "--button", "back",
            ][..],
            &["hit", "--window", "1", "--y", "2"][..],
            &["zoom", "--window", "1", "--region", "1,2,3", "--out", "z"][..],
            &["zoom", "--window", "1", "--region", "1,2,3,x", "--out", "z"][..],
            &["zoom", "--window", "1", "--region", "1,2,3,4"][..],
        ] {
            assert!(parse(argv).is_err(), "{argv:?} must be a usage error");
        }
    }

    #[test]
    fn allow_browser_chrome_flag_sets_the_field_on_the_text_writers() {
        use agenterm_cu::TargetRef;
        let parse = |argv: &[&str]| {
            let spec = verbs::lookup(argv[0]).expect("verb");
            let mut args: Vec<String> = argv[1..].iter().map(|s| (*s).to_owned()).collect();
            cli::parse_command(spec, argv[0], TargetRef::Current, &mut args).expect("parse")
        };
        match parse(&[
            "send-text",
            "--window",
            "1",
            "--allow-browser-chrome",
            "--",
            "x",
        ]) {
            Command::SendText {
                text,
                window,
                allow_browser_chrome,
                ..
            } => {
                assert_eq!(text, "x");
                assert_eq!(window, Some(1));
                assert!(allow_browser_chrome);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["send-text", "--window", "1", "--", "x"]) {
            Command::SendText {
                allow_browser_chrome,
                ..
            } => assert!(!allow_browser_chrome),
            other => panic!("{other:?}"),
        }
        // With --name the flag is accepted (it is simply not consulted).
        match parse(&[
            "send-text",
            "--window",
            "1",
            "--name",
            "Search",
            "--allow-browser-chrome",
            "hello",
        ]) {
            Command::SendText {
                text,
                name,
                allow_browser_chrome,
                ..
            } => {
                // The lenient `--name` leaves its value in the free text (pre-existing).
                assert!(text.ends_with("hello"), "{text}");
                assert_eq!(name.as_deref(), Some("Search"));
                assert!(allow_browser_chrome);
            }
            other => panic!("{other:?}"),
        }
        match parse(&[
            "send-keys",
            "--window",
            "1",
            "--allow-browser-chrome",
            "--",
            "enter",
        ]) {
            Command::SendKeys {
                keys,
                allow_browser_chrome,
                ..
            } => {
                assert_eq!(keys, "enter");
                assert!(allow_browser_chrome);
            }
            other => panic!("{other:?}"),
        }
        match parse(&["paste", "--window", "1", "--allow-browser-chrome"]) {
            Command::Paste {
                allow_browser_chrome,
                ..
            } => assert!(allow_browser_chrome),
            other => panic!("{other:?}"),
        }
        match parse(&["paste", "--window", "1"]) {
            Command::Paste {
                allow_browser_chrome,
                ..
            } => assert!(!allow_browser_chrome),
            other => panic!("{other:?}"),
        }
        // A literal after `--` is text, never the switch.
        match parse(&["send-text", "--window", "1", "--", "--allow-browser-chrome"]) {
            Command::SendText {
                text,
                allow_browser_chrome,
                ..
            } => {
                assert_eq!(text, "--allow-browser-chrome");
                assert!(!allow_browser_chrome);
            }
            other => panic!("{other:?}"),
        }
    }
}
