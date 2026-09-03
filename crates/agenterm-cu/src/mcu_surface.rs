//! MCU capability groups as a shipped `agenterm-cu` surface.
//!
//! Every MCU command group is either live on this host or typed
//! `unsupported`/`denied` with a reason. Silent absence is a defect.

use serde_json::{Value, json};

/// One MCU CAPABILITY-TREE group this binary must answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Group {
    pub id: &'static str,
    pub verbs: &'static [&'static str],
}

/// MCU groups from `skills/mcu/CAPABILITY-TREE.md` / the replacement plan.
pub const GROUPS: &[Group] = &[
    Group {
        id: "setup",
        verbs: &["setup", "doctor", "permissions"],
    },
    Group {
        id: "discover",
        verbs: &[
            "windows",
            "windows-watch",
            "apps",
            "launch",
            "quit",
            "hide",
            "show",
        ],
    },
    Group {
        id: "snapshot",
        verbs: &[
            "tree",
            "query",
            "focused",
            "observe",
            "screenshot",
            "shot",
            "elements",
            "inspect",
            "snapshot",
            "hit",
            "diff",
            "zoom",
            "find",
            "read",
            "page-text",
        ],
    },
    Group {
        id: "semantic",
        verbs: &[
            "invoke",
            "verify",
            "wait",
            "menu-inspect",
            "menu-invoke",
            "unlock",
            "tab-list",
            "tab-select",
        ],
    },
    Group {
        id: "input-local",
        verbs: &[
            "click",
            "dclick",
            "rclick",
            "send-text",
            "type",
            "send-keys",
            "key",
            "scroll",
            "pointer-move",
            "move",
            "drag",
            "ghost",
        ],
    },
    Group {
        id: "input-global",
        verbs: &["pointer-move"],
    },
    Group {
        id: "page-js",
        verbs: &["page-js", "page", "page-targets"],
    },
    Group {
        id: "geometry",
        verbs: &[
            "window-place",
            "close",
            "orderwin",
            "spaces",
            "displays",
            "movewin",
            "resize",
            "frame",
            "maximize",
            "minimize",
            "restore",
            "raise",
        ],
    },
    Group {
        id: "shell-pty-job",
        verbs: &["pty", "job"],
    },
    Group {
        id: "process",
        verbs: &["process", "ps", "kill", "signal", "exec", "state"],
    },
    Group {
        id: "resource",
        verbs: &["resource", "open", "notify"],
    },
    Group {
        id: "power",
        verbs: &["power"],
    },
    Group {
        id: "login-session",
        verbs: &["login-session"],
    },
    Group {
        id: "storage",
        verbs: &["storage"],
    },
    Group {
        id: "file",
        verbs: &["file"],
    },
    Group {
        id: "network",
        verbs: &["network"],
    },
    Group {
        id: "device",
        verbs: &["device", "audio"],
    },
    Group {
        id: "privilege",
        verbs: &["privilege"],
    },
    Group {
        id: "runtime",
        verbs: &["daemon", "audit", "session", "lock", "service", "term"],
    },
    Group {
        id: "desktop-helper",
        verbs: &["desktop-helper"],
    },
    Group {
        id: "simulator",
        verbs: &["simulator"],
    },
    Group {
        id: "browser",
        verbs: &["browser"],
    },
];

/// CLI verbs that this binary accepts and answers typed (not unknown).
pub const ALIGN_VERBS: &[&str] = &[
    "setup",
    "doctor",
    "permissions",
    "unlock",
    "pty",
    "job",
    "process",
    "resource",
    "power",
    "login-session",
    "storage",
    "file",
    "network",
    "device",
    "audio",
    "privilege",
    "daemon",
    "desktop-helper",
    "simulator",
    "browser",
    "page",
    // MCU leaf spellings that stay typed (not silent unknown). Live
    // aliases (dclick/rclick/shot/type/key/move/launch/quit/hide/show/
    // elements/clipboard/inspect/find/read) have dedicated parse arms and are not listed here.
    "drag",
    "ghost",
    "snapshot",
    "hit",
    "diff",
    "zoom",
    "minimize",
    "restore",
    "raise",
    "ps",
    "kill",
    "signal",
    "exec",
    "state",
    "open",
    "notify",
    "audit",
    "session",
    "lock",
    "service",
    "term",
];

pub fn is_align_verb(verb: &str) -> bool {
    ALIGN_VERBS.contains(&verb)
}

/// Map a CLI verb (`pty`, `windows-watch`) to the MCU group id (`shell-pty-job`).
pub fn group_id_for_verb(verb: &str) -> &'static str {
    GROUPS
        .iter()
        .find(|group| group.verbs.contains(&verb))
        .map(|group| group.id)
        .unwrap_or("unknown")
}

/// Align-CLI verbs that are typed-only (not a dedicated live command).
/// `unlock` / `windows-watch` / `apps` / `orderwin` are dedicated.
pub fn is_typed_only_verb(verb: &str) -> bool {
    is_align_verb(verb) && verb != "unlock"
}

fn typed_only_reason(verb: &str) -> &'static str {
    match verb {
        "page" => {
            "MCU page click/nav stay CDP; page read --js maps to page-js --expression (--target-id/--target-url/--target-title pick a tab); page targets is page-targets; page text is page-text (a11y reading order)"
        }
        "drag" => "pixel drag is not mapped; use invoke press or pointer-move --to desktop",
        "ghost" => "MCU ghost cursor overlay stays MCU; this binary will not draw on the desktop",
        "snapshot" => "MCU snapshot token is not mapped; use tree/query",
        "hit" => "hit-test is not mapped; use query --within or get-extents",
        "diff" => "MCU diff is observe poll-diff; use observe --window",
        "zoom" => "MCU zoom overlay is not mapped; screenshots are last resort",
        "minimize" | "restore" => {
            "MCU minimize/restore is not mapped; linux windows --minimized reads WM hidden state"
        }
        "raise" => {
            "MCU raise is orderwin --relation above; macOS cannot restack another app without activating it"
        }
        "ps" | "kill" | "signal" | "exec" | "state" => {
            "process introspect/kill stays MCU / qjs process.*; typed refuse"
        }
        "open" => "MCU open (non-GUI path) stays MCU; typed refuse",
        "notify" => "host notification stays MCU; typed refuse",
        "audit" | "session" | "lock" | "service" | "term" => {
            "MCU runtime session/lock/audit/term stays MCU; typed refuse"
        }
        _ => group_status(group_id_for_verb(verb), host_os()).1,
    }
}

fn tree_live(os: &str) -> bool {
    matches!(os, "macos" | "linux" | "windows")
}

/// Status of one group on one OS: `available`, `unsupported`, or `denied`.
pub fn group_status(group_id: &str, os: &str) -> (&'static str, &'static str) {
    match group_id {
        "setup" => (
            "unsupported",
            "TCC/setup wizard stays MCU; capabilities reports permission repair",
        ),
        "discover" => {
            if tree_live(os) {
                (
                    "available",
                    "windows / windows-watch (poll-diff) / apps (running-only) live",
                )
            } else {
                ("unsupported", "window enumerate not mapped on this OS")
            }
        }
        "snapshot" => {
            if tree_live(os) {
                ("available", "tree/query live via libagenterm a11y")
            } else {
                ("unsupported", "a11y tree not mapped on this OS")
            }
        }
        "semantic" => {
            if os == "macos" {
                ("available", "invoke/verify/wait/menu/unlock classify-only")
            } else if tree_live(os) {
                // Background menus are no longer macOS-only: AT-SPI2 and
                // UIA both publish a menu bar in the window's own tree, and
                // both have been executed against a real one.
                (
                    "available",
                    "invoke/verify/wait/menu share the CLI; unlock is macOS-only (no poke to map)",
                )
            } else {
                ("unsupported", "semantic a11y not mapped")
            }
        }
        "input-local" => {
            if tree_live(os) {
                (
                    "available",
                    "click/send-text/keys/scroll share CLI; --to required; Wayland raw pointer typed-refused",
                )
            } else {
                ("unsupported", "input inject not mapped")
            }
        }
        "input-global" => {
            if os == "linux" {
                (
                    "unsupported",
                    "global pointer on Wayland is session-global; --to desktop is explicit or refused",
                )
            } else {
                ("available", "pointer-move --to desktop is explicit global")
            }
        }
        "page-js" => (
            "available",
            "CDP Runtime.evaluate when --remote-debugging-port answers; MAIN-world Function constructor refused",
        ),
        "geometry" => {
            if os == "linux" {
                (
                    "available",
                    "window-place live where mapped; displays live; orderwin/close/spaces typed",
                )
            } else if tree_live(os) {
                (
                    "available",
                    "window-place/close/displays; orderwin raise; spaces SkyLight read on macos",
                )
            } else {
                ("unsupported", "window geometry not mapped")
            }
        }
        "shell-pty-job" => (
            "unsupported",
            "PTY/job stay AgenTerm tabs / MCU lab; this binary will not silently shell",
        ),
        "process" => (
            "unsupported",
            "process introspect stays MCU / qjs process.*; typed refuse",
        ),
        "resource" => (
            "unsupported",
            "resource pressure/cgroup stays MCU; typed refuse",
        ),
        "power" => ("unsupported", "power actions are terminal; typed refuse"),
        "login-session" => (
            "unsupported",
            "login-session lock is session-global; typed refuse",
        ),
        "storage" => ("unsupported", "volume inventory stays MCU; typed refuse"),
        "file" => (
            "unsupported",
            "file copy/move plan-apply stays MCU; typed refuse",
        ),
        "network" => (
            "unsupported",
            "network inventory/DNS plan stays MCU; typed refuse",
        ),
        "device" => ("unsupported", "device/audio lease stays MCU; typed refuse"),
        "privilege" => (
            "unsupported",
            "typed privilege broker stays MCU; no root shell",
        ),
        "runtime" => ("unsupported", "user daemon/jobs stay MCU; typed refuse"),
        "desktop-helper" => (
            "unsupported",
            "cu-helper-mac is MCU desktop-helper; this binary uses libagenterm",
        ),
        "simulator" => (
            "unsupported",
            "CoreSimulator guest AX stays MCU; typed refuse",
        ),
        "browser" => (
            "unsupported",
            "MV3/Native Messaging stays MCU; ordinary web is AX query/invoke",
        ),
        _ => ("unsupported", "unknown MCU group"),
    }
}

pub struct AlignmentRow {
    pub group: &'static str,
    pub os: &'static str,
    pub status: &'static str,
    pub reason: &'static str,
    pub verbs: &'static [&'static str],
}

pub fn alignment_rows(os: &str) -> Vec<AlignmentRow> {
    GROUPS
        .iter()
        .map(|group| {
            let (status, reason) = group_status(group.id, os);
            AlignmentRow {
                group: group.id,
                os: match os {
                    "macos" => "macos",
                    "windows" => "windows",
                    _ => "linux",
                },
                status,
                reason,
                verbs: group.verbs,
            }
        })
        .collect()
}

/// TSV: group, os, status, reason, verbs. No empty status.
pub fn alignment_matrix_text() -> String {
    let mut out = String::from("group\tos\tstatus\treason\tverbs\n");
    for os in ["macos", "linux", "windows"] {
        for row in alignment_rows(os) {
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                row.group,
                row.os,
                row.status,
                row.reason,
                row.verbs.join(",")
            ));
        }
    }
    out
}

pub fn typed_reason(group: &str) -> String {
    let os = host_os();
    let (status, reason) = group_status(group, os);
    format!("{status}: {reason}")
}

/// Reason for a CLI Align verb. Looks up the MCU group; never "unknown MCU group"
/// for verbs listed on a Group.
pub fn typed_reason_for_verb(verb: &str) -> String {
    if is_typed_only_verb(verb) {
        format!("unsupported: {}", typed_only_reason(verb))
    } else {
        typed_reason(group_id_for_verb(verb))
    }
}

pub fn host_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

pub fn verb_declaration(verb: &str) -> Value {
    let os = host_os();
    let group = group_id_for_verb(verb);
    if verb == "inspect" || verb == "find" || verb == "read" {
        let reason = match verb {
            "find" => "MCU find HANDLE TEXT is query --window --text",
            "read" => "MCU read HANDLE SELECTOR is query --window --selector",
            _ => "MCU inspect HANDLE is query --window; inspect --app inventory stays MCU",
        };
        return json!({
            "status": if tree_live(os) { "available" } else { "unsupported" },
            "alias_of": "query",
            "reason": reason,
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "page-js" {
        return json!({
            "status": "available",
            "backend": crate::observe::page_js_backend(),
            "mode": "cdp",
            "reason": crate::observe::page_js_unsupported_reason(),
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "page-targets" {
        return json!({
            "status": "available",
            "backend": crate::observe::page_js_backend(),
            "mode": "cdp",
            "reason": "CDP /json target inventory (id, url, title, type, attached, websocket) when --remote-debugging-port answers; no listener is typed unsupported",
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "page-text" {
        let (status, reason) = if tree_live(os) {
            (
                "available",
                "visible text in reading order as {id, role, text, bounds} rows from the a11y tree (web static-text carries its words in AXValue); pick a row then invoke/click --node, never --coords or a screenshot",
            )
        } else {
            ("unsupported", "a11y tree not mapped on this OS")
        };
        return json!({
            "status": status,
            "mode": "a11y-reading-order",
            "grant": "observe",
            "reason": reason,
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "tab-list" || verb == "tab-select" {
        let (status, reason) = if tree_live(os) {
            (
                "available",
                if verb == "tab-list" {
                    "browser tab strip rows (tab-group radio-buttons / page-tab / tab-item) with index, title, selected; background tabs carry no web-area"
                } else {
                    "press one tab-strip row in the background (--title SUB | --index N) and read selected back; never raises the window; the a11y fallback when no CDP port is open"
                },
            )
        } else {
            ("unsupported", "a11y tree not mapped on this OS")
        };
        return json!({
            "status": status,
            "mode": "a11y-tab-strip",
            "grant": if verb == "tab-select" { "actuate" } else { "observe" },
            "reason": reason,
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "displays" {
        let (status, reason) = if tree_live(os) {
            (
                "available",
                "agt_screen_list native frames (top-origin); not MCU scale/rotation",
            )
        } else {
            ("unsupported", "screen enumeration is not mapped on this OS")
        };
        return json!({
            "status": status,
            "reason": reason,
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "spaces" {
        let (status, reason) = if os == "macos" {
            (
                "available",
                "SkyLight managed Space inventory; move is not mapped",
            )
        } else {
            ("unsupported", "spaces inventory is macOS SkyLight only")
        };
        return json!({
            "status": status,
            "provider": if os == "macos" { "skylight-private-read" } else { "none" },
            "reason": reason,
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "windows-watch" {
        let (status, reason) = group_status(group, os);
        return json!({
            "status": status,
            "mode": "poll-diff",
            "reason": if status == "available" {
                "poll-diff over windows inventory; not AXObserver"
            } else {
                reason
            },
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "apps" {
        let (status, reason) = group_status(group, os);
        return json!({
            "status": status,
            "running_only": true,
            "reason": if status == "available" {
                "running apps from windows; installed-not-running is not mapped"
            } else {
                reason
            },
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "orderwin" {
        let (status, reason) = if os == "linux" {
            ("unsupported", "native window raise is not wired on linux")
        } else if tree_live(os) {
            (
                "available",
                "above raises --window, below raises --relative (native show / AXRaise)",
            )
        } else {
            group_status(group, os)
        };
        return json!({
            "status": status,
            "mode": "raise",
            "reason": reason,
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if is_typed_only_verb(verb) {
        return json!({
            "status": "unsupported",
            "reason": typed_only_reason(verb),
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    let (status, reason) = group_status(group, os);
    json!({ "status": status, "reason": reason, "group": group, "os": os, "verb": verb })
}

/// Merge MCU group verbs into a capabilities `verbs` object.
pub fn merge_verbs(mut verbs: Value) -> Value {
    let Some(map) = verbs.as_object_mut() else {
        return verbs;
    };
    for verb in ALIGN_VERBS {
        if !map.contains_key(*verb) {
            map.insert((*verb).to_owned(), verb_declaration(verb));
        }
    }
    for verb in [
        "windows-watch",
        "apps",
        "orderwin",
        "spaces",
        "displays",
        "unlock",
        "inspect",
        "find",
        "read",
        "page-targets",
        "page-text",
        "tab-list",
        "tab-select",
    ] {
        if !map.contains_key(verb) {
            map.insert(verb.to_owned(), verb_declaration(verb));
        }
    }
    json!(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcu_groups_have_no_silent_gap_on_three_os() {
        for os in ["macos", "linux", "windows"] {
            let rows = alignment_rows(os);
            assert_eq!(rows.len(), GROUPS.len());
            for row in &rows {
                assert!(
                    matches!(row.status, "available" | "unsupported" | "denied"),
                    "{} on {}: bad status {}",
                    row.group,
                    os,
                    row.status
                );
                if row.status != "available" {
                    assert!(
                        !row.reason.is_empty(),
                        "{} on {} missing reason",
                        row.group,
                        os
                    );
                }
                assert!(!row.verbs.is_empty());
            }
        }
        let matrix = alignment_matrix_text();
        assert!(matrix.starts_with("group\tos\tstatus\t"));
        assert!(!matrix.contains("\t\t\n"));
        assert!(matrix.contains("page-js\tmacos\tavailable\t"));
        assert!(matrix.contains("shell-pty-job\tlinux\tunsupported\t"));
        assert!(matrix.contains("simulator\twindows\tunsupported\t"));
        assert!(is_align_verb("pty") && is_align_verb("unlock"));
        assert!(!is_align_verb("query"));
        assert_eq!(group_id_for_verb("pty"), "shell-pty-job");
        assert_eq!(group_id_for_verb("windows-watch"), "discover");
        assert_eq!(group_id_for_verb("orderwin"), "geometry");
        assert_eq!(group_id_for_verb("dclick"), "input-local");
        assert_eq!(group_id_for_verb("launch"), "discover");
        assert_eq!(group_id_for_verb("page"), "page-js");
        assert_eq!(group_id_for_verb("page-targets"), "page-js");
        assert_eq!(group_id_for_verb("tab-select"), "semantic");
        assert_eq!(group_id_for_verb("tab-list"), "semantic");
        assert!(!is_align_verb("page-targets") && !is_align_verb("tab-select"));
        assert_eq!(verb_declaration("page-targets")["mode"], "cdp");
        assert_eq!(verb_declaration("page-targets")["status"], "available");
        let select = verb_declaration("tab-select");
        assert_eq!(select["mode"], "a11y-tab-strip");
        assert_eq!(select["grant"], "actuate");
        assert_eq!(select["status"], "available");
        assert_eq!(verb_declaration("tab-list")["grant"], "observe");
        assert_eq!(group_id_for_verb("page-text"), "snapshot");
        assert_eq!(verb_declaration("page-text")["mode"], "a11y-reading-order");
        assert_eq!(verb_declaration("page-text")["grant"], "observe");
        let merged = merge_verbs(json!({}));
        assert_eq!(merged["tab-select"]["group"], "semantic");
        assert_eq!(merged["page-targets"]["group"], "page-js");
        assert_eq!(group_id_for_verb("drag"), "input-local");
        assert!(is_align_verb("drag") && is_align_verb("page"));
        assert!(!is_align_verb("dclick") && !is_align_verb("launch"));
        assert!(!is_align_verb("inspect"));
        assert!(!is_align_verb("find") && !is_align_verb("read"));
        assert_eq!(verb_declaration("inspect")["alias_of"], "query");
        assert_eq!(verb_declaration("find")["alias_of"], "query");
        assert_eq!(verb_declaration("read")["alias_of"], "query");
        assert!(!is_align_verb("movewin") && !is_align_verb("frame") && !is_align_verb("maximize"));
        assert_eq!(group_id_for_verb("movewin"), "geometry");
        let page_reason = typed_reason_for_verb("page");
        assert!(page_reason.starts_with("unsupported:"), "{page_reason}");
        assert!(!page_reason.contains("unknown MCU group"), "{page_reason}");
        let pty = typed_reason_for_verb("pty");
        assert!(!pty.contains("unknown MCU group"), "{pty}");
        assert!(pty.contains("PTY") || pty.contains("job"), "{pty}");
        let watch = verb_declaration("windows-watch");
        assert_eq!(watch["mode"], "poll-diff");
        assert_eq!(watch["group"], "discover");
        assert_ne!(watch["reason"], "");
        assert_eq!(verb_declaration("apps")["running_only"], true);
        let spaces = verb_declaration("spaces");
        assert_eq!(spaces["group"], "geometry");
        if host_os() == "macos" {
            assert_eq!(spaces["status"], "available");
            assert_eq!(spaces["provider"], "skylight-private-read");
        } else {
            assert_eq!(spaces["status"], "unsupported");
        }
        assert!(!is_align_verb("spaces"));
        assert_eq!(group_id_for_verb("displays"), "geometry");
        assert_eq!(verb_declaration("displays")["status"], "available");
        let order = verb_declaration("orderwin");
        assert_eq!(order["mode"], "raise");
        assert_eq!(order["group"], "geometry");
        if host_os() == "linux" {
            assert_eq!(order["status"], "unsupported");
        } else {
            assert_eq!(order["status"], "available");
        }
        assert!(!is_align_verb("windows-watch"));
        assert!(!is_align_verb("apps"));
        assert!(!is_align_verb("orderwin"));
    }
}
