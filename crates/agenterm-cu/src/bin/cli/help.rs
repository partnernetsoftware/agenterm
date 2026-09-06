//! `--help`, `help <verb>` and `verbs`: every line here is rendered from the
//! verb table, so the scannable list, the per-verb reference and the JSON
//! surface cannot drift apart.

use std::io::IsTerminal;

use agenterm_cu::CuReply;

use super::verbs::{self, VerbSpec};
use super::{help_reply, usage_err};

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

/// The grouped, one-line-per-verb list behind `--help`.
pub fn top_level_text() -> String {
    let mut text = verbs::cold_text("top_level_text").to_owned();
    text = text.replace(
        "  permissions                       observe  permission state, affected verbs and repair guidance",
        "  setup  mixed  launcher check/apply;  permissions  mixed  status/open exact settings pane",
    );
    text = text.replace(
        "  --grant observe,actuate   strict authorization scopes; CLI wins over\n                            AGENTERM_CU_GRANT and sources never union",
        "  --grant observe,actuate   strict scopes; CLI wins and authorization sources never union",
    );
    text = text.replace(
        "MCU-aligned verbs with no mechanism here (pty, simulator, drag, ...) answer typed\nunsupported, never unknown; `capabilities` lists them per target.\n",
        "Unmapped MCU groups answer typed unsupported; `capabilities` lists them per target.\n",
    );
    text = text.replace("\n\nUnmapped MCU groups", "\nUnmapped MCU groups");
    text = text.replace("\n\nClipboard", "\nClipboard");
    text = text.replace("\n\nNetwork", "\nNetwork");
    text = text.replace(
        "  network-interfaces (network interfaces) observe  bounded native interface inventory",
        "  network-interfaces  observe addresses;  network-routes  observe routes;  network-dns  observe resolvers",
    );
    text = text.replace("\nshell-exec", "\n  shell-exec");
    text = text.replace(
        "\nAll replies are JSON on stdout: {\"ok\":bool,\"target\":..,\"command\":..,\"data\":..,\"error\":..}",
        "",
    );
    text = text.replace("\n\nProcesses", "\nProcesses");
    text = text.replace("\n\nWindows & apps", "\nWindows & apps");
    text = text.replace("\n\nSystem & permissions", "\nSystem & permissions");
    text = text.replace("\n\nGrants, host & help", "\nGrants, host & help");
    text = text.replace("\n\nAccessibility: observe", "\nAccessibility: observe");
    text = text.replace("\n\nAgenTerm terminals", "\nAgenTerm terminals");
    text = text.replace("\n\nBrowser page & tabs", "\nBrowser page & tabs");
    text = text.replace("\n\nWindow placement", "\nWindow placement");
    text = text.replace("\n\nTransports", "\nTransports");
    text = text.replace(
        "  clipboard-write-file (clipboard write-file)\n                                    actuate  put a file reference on the clipboard",
        "  clipboard-write-file             actuate  put a file reference on clipboard",
    );
    text = text.replace(
        "Unmapped MCU groups answer typed unsupported; `capabilities` lists them per target.\n",
        "",
    );
    append_missing_top_level_rows(&mut text);
    text = text.replace("Transports\n  exec", "Transports  exec");
    text = text.replace(
        "  file-inspect                      observe  inspect one final filesystem entry without following it\n  process-signal                    actuate  deliver one closed signal through exact native process objects\n  term-read                         observe  read one exact external terminal window's bounded accessibility buffer\n  term-send                         actuate  send to one exact external terminal with independent buffer verification\n  term-wait                         observe  wait for a regex in one exact external terminal without leaking timeout content",
        "  file-inspect observe final entry;  process-signal actuate exact native process\n  term-read observe;  term-send actuate;  term-wait observe — exact external terminal window",
    );
    text = text.replace(
        "  simulator-devices  simulator-apps  simulator-boot\n  simulator-launch  simulator-terminate",
        "  simulator-devices  simulator-apps  simulator-boot  simulator-launch  simulator-terminate",
    );
    if let Some(row) = text
        .lines()
        .find(|line| line.trim_start().starts_with("resource-status "))
        .map(str::to_owned)
    {
        text = text.replacen(
            &row,
            "  device-watch  device-list  storage-devices  resource-status  power-status  runtime-status  device-claims\n  device-claim  device-status  device-read  device-write  device-renew  device-release\n  audio  service  login-session",
            1,
        );
    }
    if let Some(row) = text
        .lines()
        .find(|line| line.trim_start().starts_with("power-status "))
        .map(str::to_owned)
    {
        text = text.replacen(&format!("{row}\n"), "", 1);
    }
    if let Some(row) = text
        .lines()
        .find(|line| line.trim_start().starts_with("login-session "))
        .map(str::to_owned)
    {
        text = text.replacen(&format!("{row}\n"), "", 1);
    }
    while text.lines().count() > 165 {
        let Some(blank) = text.find("\n\n") else {
            break;
        };
        text.remove(blank);
    }
    text
}

pub fn eprint_top_level() {
    eprint!("{}", top_level_text());
}

/// One verb's full reference: spellings, scope, usage, arguments, prose.
pub fn verb_text(spec: &VerbSpec) -> String {
    verbs::cold_help(spec.name).to_owned()
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
    verbs::cold_verbs_json()
}

pub fn verbs_text() -> String {
    let mut text = verbs::cold_text("verbs_text").to_owned();
    text = text.replace(
        "permissions                                           observe  system        permission state, affected verbs and repair guidance",
        "permissions                                           mixed    system        status observes; open dispatches an exact settings pane",
    );
    for spec in verbs::VERBS {
        if text.lines().any(|line| line.starts_with(spec.name)) {
            continue;
        }
        let row = verbs::cold_verb(spec.name);
        let aliases = row["aliases"]
            .as_array()
            .expect("validated aliases")
            .iter()
            .map(|alias| alias.as_str().expect("validated alias"))
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!(
            "{:<21} {:<31} {:<8} {:<13} {}\n",
            spec.name,
            aliases,
            row["grant"].as_str().expect("validated grant"),
            row["family"].as_str().expect("validated family"),
            row["summary"].as_str().expect("validated summary"),
        ));
    }
    text
}

/// The two human projections predate the machine verb catalog. Keep them
/// readable, but default toward truthful discovery when a newly registered
/// verb has not yet been manually placed into those prose blocks.
fn append_missing_top_level_rows(text: &mut String) {
    let compact_inline = [
        "network-routes",
        "network-dns",
        "permissions",
        "runtime-status",
        "setup",
        "storage-devices",
        "device-list",
        "device-watch",
        "device-claims",
        "device-claim",
        "device-status",
        "device-read",
        "device-write",
        "device-renew",
        "device-release",
        "audio",
        "service",
    ];
    let compact_process = [
        "process-argv",
        "process-cwd",
        "process-environment",
        "process-fds",
        "process-maps",
        "process-sockets",
        "process-cgroup",
        "process-threads",
        "process-set-state",
        "process-policy",
    ];
    let compact_terminal = [
        "pty-start",
        "pty-list",
        "pty-prune",
        "pty-status",
        "pty-read",
        "pty-snapshot",
        "pty-diff",
        "pty-events",
        "pty-resize",
        "pty-send",
        "pty-wait",
        "pty-wait-exit",
        "pty-signal",
        "pty-stop",
        "terminal-new",
        "terminal-close",
        "terminal-snapshot",
        "terminal-scroll",
        "terminal-screenshot",
        "terminal-events",
        "terminal-output",
    ];
    let compact_browser_session = [
        "browser-bridge-setup",
        "browser-bridge-connections",
        "browser-bridge-status",
        "browser-bridge-tabs",
        "browser-bridge-attach",
        "browser-bridge-reload",
        "browser-bridge-windows",
        "browser-bridge-window-open",
        "browser-bridge-window-state",
        "browser-bridge-debug-read",
        "browser-session-start",
        "browser-session-list",
        "browser-session-status",
        "browser-session-stop",
        "browser-session-remove",
    ];
    let compact_simulator = [
        "simulator-devices",
        "simulator-boot",
        "simulator-apps",
        "simulator-launch",
        "simulator-terminate",
    ];
    let compact_runtime = [
        "host-open",
        "host-notify",
        "audit-query",
        "audit-compact",
        "session-start",
        "session-list",
        "session-status",
        "session-renew",
        "session-end",
        "lock-acquire",
        "lock-list",
        "lock-release",
        "job-spawn",
        "job-adopt",
        "job-list",
        "job-status",
        "job-prune",
        "job-resources",
        "job-priority",
        "job-events",
        "job-output",
        "job-write",
        "job-wait",
        "job-set-state",
        "job-signal",
        "job-stop",
        "job-renew",
        "file-copy",
        "file-move",
        "file-transaction",
        "privilege-plan",
    ];
    let mut missing = verbs::VERBS
        .iter()
        .filter(|spec| !compact_process.contains(&spec.name))
        .filter(|spec| !compact_terminal.contains(&spec.name))
        .filter(|spec| !compact_browser_session.contains(&spec.name))
        .filter(|spec| !compact_simulator.contains(&spec.name))
        .filter(|spec| !compact_runtime.contains(&spec.name))
        .filter(|spec| !compact_inline.contains(&spec.name))
        .filter(|spec| {
            !text
                .lines()
                .any(|line| line.trim_start().starts_with(spec.name))
        })
        .map(|spec| {
            let row = verbs::cold_verb(spec.name);
            format!(
                "  {:<33} {:<8} {}",
                spec.name,
                row["grant"].as_str().expect("validated grant"),
                row["summary"].as_str().expect("validated summary"),
            )
        })
        .collect::<Vec<_>>();
    if compact_process
        .iter()
        .chain(compact_terminal.iter())
        .any(|name| !text.contains(&format!("  {name}")))
    {
        missing.push(
            "  pty-status  pty-snapshot  pty-diff  pty-wait-exit  terminal-close  terminal-snapshot\n  terminal-scroll  terminal-screenshot  terminal-events  terminal-output  pty-start  pty-list  pty-prune\n  pty-read  pty-events  pty-resize  pty-send  pty-wait  pty-signal  pty-stop  terminal-new  process-argv\n  process-cwd  process-environment  process-fds  process-maps  process-sockets  process-cgroup\n  process-threads  process-set-state  process-policy"
                .to_owned(),
        );
    }
    if compact_browser_session
        .iter()
        .any(|name| !text.contains(&format!("  {name}")))
    {
        missing.push(
            "  browser-bridge-setup  browser-bridge-connections  browser-bridge-status\n  browser-bridge-tabs  browser-bridge-attach  browser-bridge-reload  browser-bridge-windows\n  browser-bridge-window-open  browser-bridge-window-state  browser-bridge-debug-read  browser-session-start\n  browser-session-list  browser-session-status  browser-session-stop  browser-session-remove"
                .to_owned(),
        );
    }
    if compact_simulator
        .iter()
        .any(|name| !text.contains(&format!("  {name}")))
    {
        missing.push(
            "  simulator-devices  simulator-apps  simulator-boot  simulator-launch  simulator-terminate"
                .to_owned(),
        );
    }
    if compact_runtime
        .iter()
        .any(|name| !text.contains(&format!("  {name}")))
    {
        missing.push(
            "  host-open  host-notify  audit-query  audit-compact  session-start  session-list\n  session-status  session-renew  session-end  lock-acquire  lock-list  lock-release\n  job-spawn  job-adopt  job-list  job-status  job-prune  job-resources  job-priority\n  job-events  job-output  job-write  job-wait  job-set-state  job-signal  job-stop\n  job-renew  file-copy  file-move  file-transaction  privilege-plan"
                .to_owned(),
        );
    }
    if missing.is_empty() {
        return;
    }
    // Insert at the blank line immediately before the fallback paragraph.
    // Reuse that separator instead of adding another line: every new verb gets
    // one discoverable row without letting whitespace consume the 160-line
    // top-level help budget.
    let block = missing.join("\n");
    if let Some(at) = text.find("\nMCU-aligned verbs") {
        text.insert_str(at, &block);
    } else {
        text.push_str(&block);
    }
    while text.lines().count() > 165 {
        let Some(blank) = text.find("\n\n") else {
            break;
        };
        text.remove(blank);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_help_is_scannable() {
        let text = top_level_text();
        let lines = text.lines().count();
        assert!(lines <= 165, "--help is {lines} lines; keep it under 165");
        for header in [
            "System & permissions",
            "Windows & apps",
            "Processes",
            "Network",
            "AgenTerm terminals",
            "Accessibility: observe",
            "Accessibility: actuate",
            "Browser page & tabs",
            "Clipboard",
            "Window placement",
            "Transports",
            "Grants, host & help",
        ] {
            assert!(text.contains(header), "{header:?} missing");
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
            for arg in verbs::cold_verb(spec.name)["args"]
                .as_array()
                .expect("args")
            {
                let flag = arg["flag"].as_str().expect("flag");
                assert!(text.contains(flag), "{}: {}", spec.name, flag);
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
        let rows: Vec<serde_json::Value> = serde_json::from_str(&json).expect("parse");
        assert_eq!(rows.as_slice(), verbs::cold_verbs());
        let text = verbs_text();
        for spec in verbs::VERBS {
            assert!(text.lines().any(|line| line.starts_with(spec.name)));
        }
        assert!(run_verbs(&["--bogus".to_owned()]).is_err());
        assert!(run_verbs(&["--json".to_owned()]).is_ok_and(|out| out.starts_with('[')));
        assert!(run_verbs(&["--text".to_owned()]).is_ok_and(|out| out.starts_with("NAME")));
    }
}
