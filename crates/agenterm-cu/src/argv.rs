//! Library-owned `argv -> Command -> Executor -> CuReply` adapter.
//!
//! This is the only ordinary command-line parser. It is deliberately silent:
//! terminal presentation remains the binary wrapper's responsibility, while
//! qjswasm and other embedders receive only the typed reply.

use crate::cli::global::{Globals, authority_environment_flags};
use crate::cli::{self, verbs};
use crate::execution_control::ExecutionControl;
use crate::{Command, CuReply};

pub const MAX_ARGV_COUNT: usize = 4_096;
pub const MAX_ARGV_BYTES: usize = 1_048_576;

/// Execute ordinary `agenterm-cu` arguments with ambient authority and target
/// environment resolution, without writing stdout or stderr.
pub fn execute_argv_from_environment(args: impl IntoIterator<Item = String>) -> CuReply {
    execute_argv_from_environment_controlled(args, ExecutionControl::none())
}

/// Controlled form of [`execute_argv_from_environment`] for synchronous
/// embedders. Only mechanisms that explicitly accept the probe observe it.
pub fn execute_argv_from_environment_controlled(
    args: impl IntoIterator<Item = String>,
    control: ExecutionControl<'_>,
) -> CuReply {
    let args: Vec<String> = args.into_iter().collect();
    if let Err(reply) = validate_argv(&args) {
        return *reply;
    }
    execute_argv_with_authority_flags_controlled(args, authority_environment_flags(), control)
}

#[cfg(test)]
fn execute_argv_with_authority_flags(args: Vec<String>, flags: (bool, bool)) -> CuReply {
    execute_argv_with_authority_flags_controlled(args, flags, ExecutionControl::none())
}

fn execute_argv_with_authority_flags_controlled(
    mut args: Vec<String>,
    (ambient_authority_present, unsupported_authority_environment): (bool, bool),
    control: ExecutionControl<'_>,
) -> CuReply {
    if let Err(reply) = validate_argv(&args) {
        return *reply;
    }
    if let Some((spec, _)) = verbs::resolve_spelling(&args)
        && wants_verb_help(&args)
    {
        return cli::help::verb_help_silent(spec);
    }
    if let Some(reply) = crate::grant_management::dispatch(&args, ambient_authority_present) {
        return reply;
    }
    if args.first().is_none_or(|first| is_help_token(first)) {
        return cli::help::run_help_silent(args.get(1..).unwrap_or(&[]));
    }

    let mut globals = match Globals::parse(&mut args) {
        Ok(globals) => globals,
        Err(reply) => return *reply,
    };
    let spec = args
        .first()
        .and_then(|first| verbs::resolve(first, args.get(1).map(String::as_str)));
    match spec.map(|spec| spec.name) {
        Some("exec") => {
            return cli::exec::dispatch_json(&globals.exec_args(args.into_iter().skip(1)));
        }
        Some("help") => return cli::help::run_help_silent(&args[1..]),
        _ => {}
    }
    if let Some((spec, _)) = verbs::resolve_spelling(&args)
        && wants_verb_help(&args)
    {
        return cli::help::verb_help_silent(spec);
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
        None if crate::mcu_surface::is_align_verb(&spelled) => Command::Align {
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
    crate::embedder::execute_command_controlled(&executor, &command, control)
}

fn validate_argv(args: &[String]) -> Result<(), Box<CuReply>> {
    if args.len() > MAX_ARGV_COUNT {
        return Err(Box::new(argv_error(
            "argv_too_large",
            format!("argv count exceeds {MAX_ARGV_COUNT}"),
        )));
    }
    let mut bytes = 0usize;
    for arg in args {
        if arg.as_bytes().contains(&0) {
            return Err(Box::new(argv_error("argv_nul", "argv contains a NUL byte")));
        }
        bytes = match bytes
            .checked_add(arg.len())
            .and_then(|bytes| bytes.checked_add(1))
        {
            Some(bytes) if bytes <= MAX_ARGV_BYTES => bytes,
            _ => {
                return Err(Box::new(argv_error(
                    "argv_too_large",
                    format!("argv bytes exceed {MAX_ARGV_BYTES}"),
                )));
            }
        };
    }
    if is_binary_entry_mode(args) && !binary_entry_help_only(args) {
        return Err(Box::new(argv_error(
            "argv_entry_mode_unsupported",
            "binary-only entry mode is unavailable to library argv callers",
        )));
    }
    Ok(())
}

fn binary_entry_help_only(args: &[String]) -> bool {
    matches!(args, [entry, help] if matches!(entry.as_str(), "host" | "verbs") && matches!(help.as_str(), "--help" | "-h"))
}

fn is_binary_entry_mode(args: &[String]) -> bool {
    let Some(first) = args.first().map(String::as_str) else {
        return false;
    };
    first.starts_with("chrome-extension://")
        || matches!(first, "--version" | "-V" | "host" | "verbs")
        || first == crate::network_probe::WORKER_ARG
        || first == crate::browser_session_owner::OWNER_ARG
        || first == crate::MANAGED_JOB_OWNER_ARG
        || first == crate::DEVICE_LEASE_OWNER_ARG
        || first == crate::PRIVILEGE_BROKER_ARG
        || first == crate::DEVICE_IO_FIXTURE_ARG
        || first == crate::network_probe::FIXTURE_ARG
        || first == crate::mechanism::clipboard::X11_CLIPBOARD_OWNER_ARG
}

fn argv_error(code: &str, message: impl Into<String>) -> CuReply {
    CuReply {
        ok: false,
        target: String::new(),
        command: "argv".to_owned(),
        data: None,
        error: Some(crate::CuError::new(code, message)),
    }
}

/// Human diagnostic matching a silent typed result. Callers that speak a
/// machine protocol should not call this function.
pub fn human_diagnostic(args: &[String], reply: &CuReply) -> Option<String> {
    if reply.command == "help" {
        return cli::help::human_help_text(
            reply
                .data
                .as_ref()
                .and_then(|data| data.get("verb"))
                .and_then(|verb| verb.as_str()),
        );
    }
    if reply
        .error
        .as_ref()
        .is_some_and(|error| error.code == "usage")
    {
        // Parse the globals again only to locate the actual verb. Scanning all
        // tokens would misread a global value such as `--grant observe` as
        // the `observe` verb and print unrelated help.
        let mut command_args = args.to_vec();
        let spec = Globals::parse(&mut command_args).ok().and_then(|_| {
            command_args
                .first()
                .and_then(|first| verbs::resolve(first, command_args.get(1).map(String::as_str)))
        });
        return Some(spec.map_or_else(cli::help::top_level_text, cli::help::verb_text));
    }
    None
}

fn is_help_token(token: &str) -> bool {
    matches!(token, "--help" | "-h") || verbs::lookup(token).is_some_and(|spec| spec.name == "help")
}

/// Whether `args` is exactly one verb spelling followed by ONE help token.
///
/// The spelling comes from the same longest-prefix resolver dispatch uses, so a
/// three-token spelling (`processor topology status`) reaches its help instead of
/// falling through to the global target requirement. Nothing else is tolerated:
/// an extra token or a misplaced help token is a normal command, which keeps the
/// `--target` requirement exactly as strict as it was.
fn wants_verb_help(args: &[String]) -> bool {
    let Some((_, tokens)) = verbs::resolve_spelling(args) else {
        return false;
    };
    args.len() == tokens + 1 && matches!(args[tokens].as_str(), "--help" | "-h")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{Authorization, Executor, Grant, TargetRef};

    fn words(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|word| (*word).to_owned()).collect()
    }

    fn normalize_live_capability_facts(reply: &mut serde_json::Value) {
        let data = reply.get_mut("data").expect("capabilities data");
        assert!(data.get("host_clock").is_some());
        data["host_clock"] = serde_json::json!({ "normalized": true });
        if let Some(uptime) = data.pointer_mut("/host_boot_identity/uptime_milliseconds") {
            *uptime = serde_json::json!(0);
        }
    }

    #[test]
    fn argv_and_typed_command_share_the_same_executor_reply() {
        let argv = execute_argv_with_authority_flags(
            words(&["--target", "current", "--grant", "observe", "capabilities"]),
            (false, false),
        );
        let command = Command::Capabilities {
            target: TargetRef::Current,
        };
        let direct =
            Executor::new(Authorization::new(BTreeSet::from([Grant::Observe]))).execute(&command);
        let mut argv = serde_json::to_value(argv).expect("argv reply");
        let mut direct = serde_json::to_value(direct).expect("direct reply");
        normalize_live_capability_facts(&mut argv);
        normalize_live_capability_facts(&mut direct);
        assert_eq!(argv, direct);
    }

    #[test]
    fn global_target_and_authority_shapes_remain_typed() {
        let missing_identity = execute_argv_with_authority_flags(
            words(&[
                "--target",
                "current",
                "--grant",
                "actuate",
                "--request-id",
                "request-1",
                "clipboard-clear",
            ]),
            (false, false),
        );
        assert_eq!(
            missing_identity.error.expect("identity error").code,
            "request_identity_incomplete"
        );

        let unsupported_environment = execute_argv_with_authority_flags(
            words(&["--target", "current", "capabilities"]),
            (true, true),
        );
        assert_eq!(
            unsupported_environment.error.expect("authority error").code,
            "invalid_authorization"
        );
    }

    #[test]
    fn help_and_usage_expose_presentation_without_printing_it() {
        let help_args = words(&["tree", "--help"]);
        let help = execute_argv_with_authority_flags(help_args.clone(), (false, false));
        assert!(help.ok);
        assert_eq!(help.command, "help");
        assert!(
            human_diagnostic(&help_args, &help)
                .unwrap()
                .contains("tree")
        );

        let bad_args = words(&["--target", "current", "tree", "--bogus"]);
        let bad = execute_argv_with_authority_flags(bad_args.clone(), (false, false));
        assert_eq!(bad.error.as_ref().unwrap().code, "usage");
        assert!(human_diagnostic(&bad_args, &bad).unwrap().contains("tree"));

        let missing_target = words(&["--grant", "observe", "capabilities"]);
        let reply = execute_argv_with_authority_flags(missing_target.clone(), (false, false));
        let diagnostic = human_diagnostic(&missing_target, &reply).unwrap();
        assert!(diagnostic.contains("agenterm-cu capabilities"));
        assert!(!diagnostic.contains("agenterm-cu observe"));
    }

    #[test]
    fn argv_bounds_and_binary_entry_modes_fail_before_dispatch() {
        for argv in [
            words(&[crate::MANAGED_JOB_OWNER_ARG]),
            words(&["host"]),
            words(&["chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"]),
        ] {
            let reply = execute_argv_with_authority_flags(argv, (false, false));
            assert_eq!(
                reply.error.expect("entry-mode refusal").code,
                "argv_entry_mode_unsupported"
            );
        }
        let nul = execute_argv_with_authority_flags(vec!["bad\0arg".to_owned()], (false, false));
        assert_eq!(nul.error.expect("NUL refusal").code, "argv_nul");
        let bytes =
            execute_argv_with_authority_flags(vec!["x".repeat(MAX_ARGV_BYTES + 1)], (false, false));
        assert_eq!(bytes.error.expect("byte refusal").code, "argv_too_large");
        let count = execute_argv_with_authority_flags(
            vec![String::new(); MAX_ARGV_COUNT + 1],
            (false, false),
        );
        assert_eq!(count.error.expect("count refusal").code, "argv_too_large");
    }

    fn run(raw: &[&str]) -> CuReply {
        execute_argv_with_authority_flags(words(raw), (false, false))
    }

    fn help_verb(reply: &CuReply) -> Option<String> {
        reply
            .data
            .as_ref()?
            .get("verb")?
            .as_str()
            .map(str::to_owned)
    }

    /// Every spelling in the catalog -- one, two and three tokens -- reaches its
    /// own verb help through BOTH help tokens, with no `--target` supplied.
    #[test]
    fn every_spelling_reaches_its_own_help_without_a_target() {
        for spec in verbs::VERBS {
            for spelling in spec.spellings() {
                for help in ["--help", "-h"] {
                    let mut raw: Vec<&str> = spelling.split(' ').collect();
                    raw.push(help);
                    let reply = run(&raw);
                    assert!(reply.ok, "{spelling} {help}: {:?}", reply.error);
                    assert_eq!(reply.command, "help", "{spelling} {help}");
                    assert_eq!(
                        help_verb(&reply).as_deref(),
                        Some(spec.name),
                        "{spelling} {help}"
                    );
                }
            }
        }
    }

    /// The four three-token spellings that used to be unreachable.
    #[test]
    fn three_token_spellings_reach_their_help() {
        for (spelling, name) in [
            ("processor topology status", "processor-topology-status"),
            ("cache hierarchy status", "cache-hierarchy-status"),
            ("processor affinity status", "processor-affinity-status"),
            ("screen reader status", "screen-reader"),
        ] {
            for help in ["--help", "-h"] {
                let mut raw: Vec<&str> = spelling.split(' ').collect();
                raw.push(help);
                let reply = run(&raw);
                assert!(reply.ok, "{spelling} {help}: {:?}", reply.error);
                assert_eq!(reply.command, "help", "{spelling} {help}");
                assert_eq!(
                    help_verb(&reply).as_deref(),
                    Some(name),
                    "{spelling} {help}"
                );
            }
        }
    }

    /// Without a help token the same three-token spelling is an ordinary command
    /// and keeps the unchanged `--target` requirement.
    #[test]
    fn a_three_token_spelling_without_help_still_requires_a_target() {
        let reply = run(&["processor", "topology", "status"]);
        assert!(!reply.ok);
        let error = reply.error.as_ref().expect("typed");
        assert_eq!(error.code, "usage");
        assert!(error.message.contains("--target"), "{}", error.message);
    }

    /// The longest spelling wins, so a two-token verb is not stolen by the
    /// one-token verb that shares its first word.
    #[test]
    fn the_longest_spelling_wins_over_a_shorter_prefix() {
        let reply = run(&["page", "read", "--help"]);
        assert!(reply.ok, "{:?}", reply.error);
        assert_eq!(help_verb(&reply).as_deref(), Some("page-js"));
        // The one-token form still resolves to its own verb.
        let shorter = run(&["page", "--help"]);
        assert_eq!(help_verb(&shorter).as_deref(), Some("page"));
    }

    /// Help is never inferred from a shape that is not exactly
    /// `<spelling> <one help token>`: an extra token, a partial spelling, or a
    /// help token in the middle stays an ordinary (target-requiring) command.
    #[test]
    fn help_is_not_inferred_from_extra_or_misplaced_tokens() {
        for raw in [
            vec!["processor", "topology", "status", "extra", "--help"],
            vec!["processor", "topology", "--help"],
            vec!["processor", "--help", "status"],
        ] {
            let reply = run(&raw);
            assert_ne!(reply.command, "help", "{raw:?}");
            assert_eq!(
                reply.error.as_ref().expect("typed").code,
                "usage",
                "{raw:?}"
            );
        }
    }
}
