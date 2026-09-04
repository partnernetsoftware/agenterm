//! Shell parsing for `agenterm-cu`: one module per verb family, all driven
//! by the static verb table in [`verbs`]. The binary's `dispatch` resolves
//! the first token through that table and hands the rest to
//! [`parse_command`]; every usage failure is a `String` here and becomes the
//! typed `usage` reply in one place.

pub mod a11y_actuate;
pub mod a11y_observe;
pub mod browser;
pub mod clipboard;
pub mod exec;
pub mod global;
pub mod help;
pub mod menu;
pub mod placement;
pub mod process;
pub mod system;
pub mod verbs;
pub mod windows;

use agenterm_cu::{Command, CuError, CuReply, TargetRef, command::Expectation};

use verbs::{Family, VerbSpec};

/// Parse `spelled args…` into a `Command` through the family module the
/// verb table names. `spelled` is the token the caller typed (a canonical
/// name, an alias, or a group word such as `menu`); the family module uses
/// it only where the alias changes the shape (`elements` is `tree --flat`).
pub fn parse_command(
    spec: &VerbSpec,
    spelled: &str,
    target: TargetRef,
    args: &mut Vec<String>,
) -> Result<Command, String> {
    match spec.family {
        Family::System => system::parse(spec, target, args),
        Family::Windows => windows::parse(spec, spelled, target, args),
        Family::Process => process::parse(spec, target, args),
        Family::A11yObserve => a11y_observe::parse(spec, spelled, target, args),
        Family::A11yActuate => a11y_actuate::parse(spec, spelled, target, args),
        Family::Browser => browser::parse(spec, spelled, target, args),
        Family::Clipboard => clipboard::parse(spec, spelled, target, args),
        Family::Placement => placement::parse(spec, spelled, target, args),
        Family::Transports | Family::Host => Err(format!(
            "{} is an entry mode, not a target command; see `agenterm-cu help {}`",
            spec.name, spec.name
        )),
    }
}

/// The typed usage reply; the grouped command list goes to stderr.
pub fn usage_err(message: impl Into<String>) -> CuReply {
    help::eprint_top_level();
    usage_reply(message)
}

/// The typed usage reply for one verb; that verb's reference goes to stderr
/// instead of the whole list.
pub fn usage_err_for(spec: &VerbSpec, message: impl Into<String>) -> CuReply {
    eprint!("{}", help::verb_text(spec));
    usage_reply(message)
}

fn usage_reply(message: impl Into<String>) -> CuReply {
    CuReply {
        ok: false,
        target: String::new(),
        command: "usage".into(),
        data: None,
        error: Some(CuError::new("usage", message)),
    }
}

pub fn help_reply(verb: Option<&str>) -> CuReply {
    let data = match verb {
        Some(verb) => serde_json::json!({ "usage": "see stderr", "verb": verb }),
        None => serde_json::json!({ "usage": "see stderr" }),
    };
    CuReply {
        ok: true,
        target: String::new(),
        command: "help".into(),
        data: Some(data),
        error: None,
    }
}

/// A typed failure that still names the command and target it refused.
pub fn command_error(command: &Command, code: &str, message: &str) -> CuReply {
    CuReply {
        ok: false,
        target: command.target().as_str().into(),
        command: command.verb(),
        data: None,
        error: Some(CuError::new(code, message)),
    }
}

// ---------------------------------------------------------------- flag helpers

/// Lenient string flag: the value stays in `args` (older verbs' shape).
pub fn flag_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.remove(index);
    args.get(index).cloned()
}

pub fn flag_isize(args: &mut Vec<String>, flag: &str) -> Option<isize> {
    flag_value(args, flag)?.parse().ok()
}

/// A typed flag that must parse when present: `Ok(None)` when absent,
/// `Err(usage)` when present without a value or with a value that is not a
/// window token. Unlike `flag_isize`, a bad value never silently drops the
/// flag.
pub fn flag_window(args: &mut Vec<String>) -> Result<Option<isize>, String> {
    flag_handle(args, "--window")
}

pub fn flag_handle(args: &mut Vec<String>, flag: &'static str) -> Result<Option<isize>, String> {
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

pub fn flag_window_opt(args: &mut Vec<String>) -> Option<isize> {
    let raw = flag_value(args, "--window")?;
    agenterm_cu::observe::parse_window_token(&raw).ok()
}

pub fn flag_parsed<T: std::str::FromStr>(
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
pub fn flag_text(args: &mut Vec<String>, flag: &'static str) -> Result<Option<String>, String> {
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
pub fn take_switch(args: &mut Vec<String>, flag: &str) -> bool {
    let present = args.iter().any(|arg| arg == flag);
    args.retain(|arg| arg != flag);
    present
}

/// `--focused` alone means `true`; `--focused true|false` is explicit.
pub fn flag_tristate(args: &mut Vec<String>, flag: &str) -> Option<bool> {
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

pub fn flag_i32(args: &mut Vec<String>, flag: &str) -> Option<i32> {
    flag_value(args, flag)?.parse().ok()
}

pub fn flag_u32(args: &mut Vec<String>, flag: &str) -> Option<u32> {
    flag_value(args, flag)?.parse().ok()
}

pub fn flag_u64(args: &mut Vec<String>, flag: &str) -> Option<u64> {
    flag_value(args, flag)?.parse().ok()
}

pub fn flag_usize(args: &mut Vec<String>, flag: &str) -> Option<usize> {
    flag_value(args, flag)?.parse().ok()
}

pub fn flag_coords(args: &mut Vec<String>, flag: &str) -> Option<[i32; 2]> {
    let raw = flag_value(args, flag)?;
    let mut parts = raw.split(',');
    let x = parts.next()?.trim().parse().ok()?;
    let y = parts.next()?.trim().parse().ok()?;
    Some([x, y])
}

/// `--window H`, or the MCU positional `HANDLE` when the first token is not
/// a flag.
pub fn parse_optional_window(args: &mut Vec<String>) -> Result<Option<isize>, String> {
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

/// Everything after a literal `--`, joined with `separator`; the tail is
/// removed from `args` so flag parsing never sees it.
pub fn split_literal_tail(args: &mut Vec<String>, separator: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == "--")?;
    Some(args.split_off(index)[1..].join(separator))
}

/// `--expect` is a JSON array of closed-shape items; an unknown key or a
/// non-array is a usage error before any tree is read.
pub fn parse_expectations(raw: &str) -> Result<Vec<Expectation>, String> {
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

/// `(window, name, role)` of a name-addressed node.
pub type NamedNode = (Option<isize>, Option<String>, Option<String>);

/// Named-node shape shared by the AT-SPI text verbs: `--window H --name PAT
/// [--role ROLE]`, with `--name` required.
pub fn named_node(args: &mut Vec<String>, missing: &str) -> Result<NamedNode, String> {
    let window = flag_window_opt(args);
    let name = flag_value(args, "--name");
    let role = flag_value(args, "--role");
    if name.as_ref().is_none_or(|value| value.is_empty()) {
        return Err(missing.to_owned());
    }
    Ok((window, name, role))
}
