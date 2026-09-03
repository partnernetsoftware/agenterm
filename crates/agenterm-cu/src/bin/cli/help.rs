//! `--help`, `help <verb>` and `verbs`: every line here is rendered from the
//! verb table, so the scannable list, the per-verb reference and the JSON
//! surface cannot drift apart.

use std::fmt::Write as _;
use std::io::IsTerminal;

use agenterm_cu::CuReply;

use super::verbs::{self, Family, Scope, VerbSpec};
use super::{help_reply, usage_err};

const USAGE_HEAD: &str = r"usage: agenterm-cu --target <current|ssh|vnc|rdp> [--grant observe,actuate] <command> [args...]
       agenterm-cu --ssh <user@host> [--ssh-port N] [--ssh-identity PATH] [--ssh-cu PATH]
                   [--ssh-env KEY=VAL]... [--grant observe,actuate] <command> [args...]
       agenterm-cu --vnc <host[:port]> [--vnc-port N] [--vnc-cu PATH]
                   [--vnc-env KEY=VAL]... [--grant observe,actuate] <command> [args...]
       agenterm-cu --rdp <host[:port]> [--grant observe,actuate] <command> [args...]
       agenterm-cu exec [--grant observe,actuate] --json '<command-json>' | --json -
       agenterm-cu grant create|list|revoke ...     bounded persisted grants (help grant)
       agenterm-cu host | hotkeys                   desktop menu and global shortcuts
       agenterm-cu help <verb> | <verb> --help      one verb's full reference
       agenterm-cu verbs [--json]                   the verb table (JSON when piped)

Global flags:
  --target current|ssh|vnc|rdp  explicit target reference (required unless --ssh/--vnc/--rdp)
  --ssh <user@host>         ssh target destination (implies --target ssh; or AGENTERM_CU_SSH)
  --ssh-port N              OpenSSH -p (or AGENTERM_CU_SSH_PORT)
  --ssh-identity PATH       OpenSSH -i (or AGENTERM_CU_SSH_IDENTITY)
  --ssh-cu PATH             remote agenterm-cu path (or AGENTERM_CU_SSH_CU; default: this exe)
  --ssh-env KEY=VAL         remote env for the worker (repeatable; also AGENTERM_CU_SSH_ENV)
  --vnc <host[:port]>       vnc/RFB endpoint (implies --target vnc; or AGENTERM_CU_VNC)
  --vnc-port N              RFB TCP port when --vnc omits :port (or AGENTERM_CU_VNC_PORT; default 5900)
  --vnc-cu PATH             session worker agenterm-cu path (or AGENTERM_CU_VNC_CU; default: this exe)
  --vnc-env KEY=VAL         session env for the worker (repeatable; also AGENTERM_CU_VNC_ENV)
  --rdp <host[:port]>       rdp endpoint syntax only (implies --target rdp; PLACEHOLDER --
                            no connect / TLS / CredSSP; always rdp_unavailable)
  --grant observe,actuate   strict authorization scopes; CLI wins over
                            AGENTERM_CU_GRANT and sources never union
  --grant-id ID             bounded persisted current-target grant selector;
                            mutually exclusive with every other auth source
  --grant-store PATH        explicit store override; valid only with --grant-id

Transports: ssh and vnc run the same verbs on an agenterm-cu --target current
  worker (OpenSSH stdio / the shared RFB session); rdp parses, authorizes and
  fails closed with rdp_unavailable. `help ssh|vnc|rdp` carry the evidence notes.

Commands  (scope = required --grant; `help <verb>` prints arguments and behaviour)
";

const FOOTER: &str = r#"
MCU-aligned verbs with no mechanism here (pty, simulator, drag, ...) answer typed
unsupported, never unknown; `capabilities` lists them per target.
All replies are JSON on stdout: {"ok":bool,"target":..,"command":..,"data":..,"error":..}
"#;

/// Topics that are not verbs but deserve the long transport prose.
const TOPICS: &[(&str, &str)] = &[
    (
        "ssh",
        r"ssh transport runs the same verbs on a remote agenterm-cu --target current
worker over OpenSSH stdio (no new verb). Get-selection evidence: loopback
sshd + second agenterm-con, host send-text seed into Command, host select a
range, host independent get-selection --name Command returns that range
(via=get-selection; native AT-SPI GetNSelections+GetSelection; never
screenshot / --coords / mouse-drag).

flags: --ssh <user@host> [--ssh-port N] [--ssh-identity PATH] [--ssh-cu PATH]
       [--ssh-env KEY=VAL]...   (or AGENTERM_CU_SSH / _SSH_PORT / _SSH_IDENTITY /
       _SSH_CU / _SSH_ENV)
",
    ),
    (
        "vnc",
        r"vnc transport handshakes RFB (security type None / x11vnc -nopw), then runs
the same verbs (observe and actuate) on a local agenterm-cu --target current
worker against the shared session (DISPLAY/AT-SPI env; no new verb).
Get-selection evidence: gate-owned loopback x11vnc + second agenterm-con,
Command holds a known ASCII seed with a known non-empty selection START..END
(gate precondition), host independent get-selection --window H --name Command
returns that range (via=get-selection; native AT-SPI GetNSelections +
GetSelection(0); n==1 start/end equal precondition range; never screenshot /
--coords / mouse-drag / RFB framebuffer OCR / cached setter reply).

flags: --vnc <host[:port]> [--vnc-port N] [--vnc-cu PATH] [--vnc-env KEY=VAL]...
       (or AGENTERM_CU_VNC / _VNC_PORT / _VNC_CU / _VNC_ENV)
",
    ),
    (
        "rdp",
        r#"rdp is a PLACEHOLDER (cut 3.46): --rdp HOST[:PORT] and --target rdp parse,
authorize, then fail closed with error.code=rdp_unavailable. No socket
connect, no TLS/CredSSP, no screenshot/--coords, no silent ssh/vnc/current
reuse. Reserved first observe argv for a later Windows agent:
  agenterm-cu --rdp "WINDOWS_HOST:3389" --grant observe tree --window HANDLE
Live RDP session + UIA-over-RDP evidence is not claimed on this cut.
"#,
    ),
];

const NAME_COLUMN: usize = 36;

fn spelled_with_aliases(spec: &VerbSpec) -> String {
    if spec.aliases.is_empty() {
        spec.name.to_owned()
    } else {
        format!("{} ({})", spec.name, spec.aliases.join(", "))
    }
}

/// The grouped, one-line-per-verb list behind `--help`.
pub fn top_level_text() -> String {
    let mut out = String::from(USAGE_HEAD);
    for family in Family::ALL {
        let _ = writeln!(out, "\n{}", family.header());
        for spec in verbs::by_family(family) {
            let spelled = spelled_with_aliases(spec);
            if spelled.len() + 2 >= NAME_COLUMN {
                let _ = writeln!(out, "  {spelled}");
                let _ = writeln!(
                    out,
                    "{:width$}{:<9}{}",
                    "",
                    spec.scope.as_str(),
                    spec.summary,
                    width = NAME_COLUMN
                );
            } else {
                let _ = writeln!(
                    out,
                    "  {:<width$}{:<9}{}",
                    spelled,
                    spec.scope.as_str(),
                    spec.summary,
                    width = NAME_COLUMN - 2
                );
            }
        }
    }
    out.push_str(FOOTER);
    out
}

pub fn eprint_top_level() {
    eprint!("{}", top_level_text());
}

/// One verb's full reference: spellings, scope, usage, arguments, prose.
pub fn verb_text(spec: &VerbSpec) -> String {
    let mut out = String::new();
    let _ = write!(out, "agenterm-cu {}", spec.name);
    if !spec.aliases.is_empty() {
        let _ = write!(out, "    (also: {})", spec.aliases.join(", "));
    }
    out.push('\n');
    let _ = write!(
        out,
        "  scope: {:<10} family: {}",
        spec.scope.as_str(),
        spec.family.header()
    );
    if spec.command != spec.name {
        let _ = write!(out, "    reply.command: {}", spec.command);
    }
    out.push_str("\n\n");
    match spec.scope {
        Scope::Unscoped => {
            out.push_str("usage:\n");
            for line in spec.usage.lines() {
                let _ = writeln!(out, "  agenterm-cu {line}");
            }
        }
        Scope::Observe | Scope::Actuate => {
            let _ = writeln!(
                out,
                "usage (after the global flags, e.g. agenterm-cu --target current --grant {}):",
                spec.scope.as_str()
            );
            for line in spec.usage.lines() {
                let _ = writeln!(out, "  {line}");
            }
        }
    }
    if !spec.args.is_empty() {
        out.push_str("\narguments:\n");
        for arg in spec.args {
            let head = if arg.value.is_empty() {
                arg.flag.to_owned()
            } else {
                format!("{} {}", arg.flag, arg.value)
            };
            let _ = writeln!(out, "  {head:<30}{}", arg.help);
        }
    }
    if !spec.details.is_empty() {
        out.push('\n');
        out.push_str(spec.details.trim_end());
        out.push('\n');
    }
    out
}

/// `help [name…]`: the grouped list, one verb, one transport topic, or a
/// typed usage error naming the near matches.
pub fn run_help(args: &[String]) -> CuReply {
    let Some(first) = args.first() else {
        eprint_top_level();
        return help_reply(None);
    };
    if matches!(first.as_str(), "--help" | "-h") {
        eprint_top_level();
        return help_reply(None);
    }
    let name = args.join(" ");
    if let Some((topic, text)) = TOPICS.iter().find(|(topic, _)| *topic == name) {
        eprint!("{text}");
        return help_reply(Some(topic));
    }
    if let Some(spec) = verbs::VERBS
        .iter()
        .find(|spec| spec.spellings().any(|spelling| spelling == name))
    {
        return verb_help(spec);
    }
    if args.len() == 1 && agenterm_cu::mcu_surface::is_align_verb(first) {
        eprintln!(
            "agenterm-cu {first}: MCU-aligned verb with no mechanism in this binary; it answers typed unsupported.\n  {}",
            agenterm_cu::mcu_surface::typed_reason_for_verb(first)
        );
        return help_reply(Some(first));
    }
    let near = verbs::near_matches(&name);
    let message = if near.is_empty() {
        format!("unknown verb '{name}'; `agenterm-cu verbs` lists every verb")
    } else {
        format!("unknown verb '{name}'; near matches: {}", near.join(", "))
    };
    usage_err(message)
}

pub fn verb_help(spec: &VerbSpec) -> CuReply {
    eprint!("{}", verb_text(spec));
    help_reply(Some(spec.name))
}

/// `verbs [--json | --text]`: `Ok` is the text to print on stdout.
pub fn run_verbs(args: &[String]) -> Result<String, Box<CuReply>> {
    let mut json = !std::io::stdout().is_terminal();
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "--text" => json = false,
            other => {
                return Err(Box::new(usage_err(format!(
                    "verbs accepts only --json or --text; unexpected {other:?}"
                ))));
            }
        }
    }
    Ok(if json {
        format!("{}\n", verbs_json())
    } else {
        verbs_text()
    })
}

pub fn verbs_json() -> String {
    serde_json::to_string_pretty(&verbs::table_json()).unwrap_or_else(|_| "[]".into())
}

pub fn verbs_text() -> String {
    let mut out = format!(
        "{:<22}{:<32}{:<9}{:<14}{}\n",
        "NAME", "ALIASES", "SCOPE", "FAMILY", "SUMMARY"
    );
    for spec in verbs::VERBS {
        let _ = writeln!(
            out,
            "{:<22}{:<32}{:<9}{:<14}{}",
            spec.name,
            spec.aliases.join(", "),
            spec.scope.as_str(),
            spec.family.id(),
            spec.summary
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_help_is_scannable() {
        let text = top_level_text();
        let lines = text.lines().count();
        assert!(lines <= 150, "--help is {lines} lines; keep it under 150");
        for family in Family::ALL {
            assert!(text.contains(family.header()), "{family:?} header missing");
        }
        for spec in verbs::VERBS {
            assert!(
                text.contains(&format!("  {}", spec.name)),
                "{} missing from --help",
                spec.name
            );
        }
        for line in text.lines() {
            assert!(line.len() <= 110, "line too wide: {line:?}");
        }
    }

    #[test]
    fn every_verb_renders_a_reference() {
        for spec in verbs::VERBS {
            let text = verb_text(spec);
            assert!(text.starts_with(&format!("agenterm-cu {}", spec.name)));
            assert!(text.contains("usage"), "{}", spec.name);
            for arg in spec.args {
                assert!(text.contains(arg.flag), "{}: {}", spec.name, arg.flag);
            }
        }
    }

    #[test]
    fn help_resolves_every_spelling_and_topic() {
        for spec in verbs::VERBS {
            for spelling in spec.spellings() {
                let args: Vec<String> = spelling.split(' ').map(str::to_owned).collect();
                let reply = run_help(&args);
                assert!(reply.ok, "help {spelling}: {:?}", reply.error);
                assert_eq!(reply.command, "help");
                assert_eq!(reply.data.as_ref().unwrap()["verb"], spec.name);
            }
        }
        for (topic, _) in TOPICS {
            let reply = run_help(&[(*topic).to_owned()]);
            assert!(reply.ok, "help {topic}");
        }
        let bare = run_help(&[]);
        assert!(bare.ok);
        assert_eq!(bare.data.as_ref().unwrap()["usage"], "see stderr");
    }

    #[test]
    fn unknown_help_verb_is_typed_with_near_matches() {
        let reply = run_help(&["windws".to_owned()]);
        assert!(!reply.ok);
        assert_eq!(reply.command, "usage");
        let error = reply.error.expect("usage");
        assert_eq!(error.code, "usage");
        assert!(error.message.contains("windows"), "{}", error.message);
        let group = run_help(&["menu".to_owned()]);
        let error = group.error.expect("usage");
        assert!(error.message.contains("menu inspect"), "{}", error.message);
        let far = run_help(&["qqqqqqqq".to_owned()]);
        let error = far.error.expect("usage");
        assert!(
            error.message.contains("agenterm-cu verbs"),
            "{}",
            error.message
        );
    }

    #[test]
    fn align_verb_help_is_a_typed_note() {
        let reply = run_help(&["pty".to_owned()]);
        assert!(reply.ok);
        assert_eq!(reply.data.as_ref().unwrap()["verb"], "pty");
    }

    #[test]
    fn verbs_json_round_trips_and_text_lists_every_verb() {
        let json = verbs_json();
        let rows: Vec<verbs::VerbJson> = serde_json::from_str(&json).expect("parse");
        assert_eq!(rows, verbs::table_json());
        let text = verbs_text();
        for spec in verbs::VERBS {
            assert!(text.lines().any(|line| line.starts_with(spec.name)));
        }
        assert!(run_verbs(&["--bogus".to_owned()]).is_err());
        assert!(run_verbs(&["--json".to_owned()]).is_ok_and(|out| out.starts_with('[')));
        assert!(run_verbs(&["--text".to_owned()]).is_ok_and(|out| out.starts_with("NAME")));
    }
}
