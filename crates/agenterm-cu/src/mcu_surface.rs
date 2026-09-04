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
        verbs: &[
            "page-js",
            "page",
            "page-targets",
            "page-find",
            "page-click",
            "page-download",
            "page-hover",
            "page-scroll",
            "page-drag",
            "page-dialog",
            "page-files",
            "page-fill",
            "page-type",
            "page-nav",
            "page-screenshot",
        ],
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
        verbs: &["network", "network-probe"],
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
        verbs: &["browser-profiles", "browser-open", "tab-close"],
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
    "page",
    // MCU leaf spellings that stay typed (not silent unknown). Live
    // aliases (dclick/rclick/shot/type/key/move/launch/quit/hide/show/
    // elements/clipboard/inspect/find/read) have dedicated parse arms and are not listed here.
    // The desktop-ring absorption made drag / hit / zoom / snapshot / diff /
    // raise / minimize / restore live verbs with their own parse arms, so
    // they left this list; `ghost` (a cursor overlay drawn on the desktop)
    // is deliberately not absorbed and stays typed.
    "ghost",
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
/// `permissions` / `unlock` / `windows-watch` / `apps` / `orderwin` are dedicated.
pub fn is_typed_only_verb(verb: &str) -> bool {
    is_align_verb(verb) && !matches!(verb, "unlock" | "permissions")
}

fn typed_only_reason(verb: &str) -> &'static str {
    match verb {
        "page" => {
            "MCU page read --js maps to page-js --expression, page read to the CDP page-text, page targets to page-targets, page text to page-text (a11y with --window, CDP with --target-*), page find/click/hover/scroll/drag/dialog/files/fill/nav/screenshot map to typed CDP verbs (background tabs, no focus change); any other page sub-verb is typed unsupported"
        }
        "ghost" => "ACU migration gap: the ghost cursor overlay has no typed facade yet",
        "ps" | "signal" | "exec" | "state" => {
            "ACU migration gap: delegate through the bounded process/qjswasm facade; typed refuse"
        }
        "open" => "ACU migration gap: typed host-open facade pending",
        "notify" => "ACU migration gap: typed host-notification facade pending",
        "network" => {
            "network probe is live as network-probe; interfaces/routes/DNS inventory/sockets remain typed gaps"
        }
        "audit" | "session" | "lock" | "service" | "term" => {
            "ACU migration gap: delegate through the AgenTerm runtime facade; typed refuse"
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
            "ACU migration gap: setup/repair workflow pending; capabilities reports permission repair",
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
            } else if os == "linux" {
                // Background menus are no longer macOS-only: AT-SPI2 and
                // UIA both publish a menu bar in the window's own tree, and
                // both have been executed against a real one. Neither is
                // `unlock` macOS-only any more: the poke exists on Linux as
                // a bus property instead of an element attribute.
                (
                    "available",
                    "invoke/verify/wait/menu share the CLI; unlock pokes org.a11y.Status (IsEnabled + ScreenReaderEnabled), the AT-SPI analogue of macOS AXManualAccessibility",
                )
            } else if os == "windows" {
                (
                    "available",
                    "invoke/verify/wait/menu share the CLI; unlock has no poke to map because the UIA walk is the poke (Chromium enables accessibility answering WM_GETOBJECT)",
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
            "CDP when --remote-debugging-port answers: Runtime.evaluate (page-js), page text / find / click / fill / nav / screenshot on one page target (background tabs, no focus change); MAIN-world Function constructor refused",
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
            "ACU migration gap: delegate PTY/job through the AgenTerm runtime; this command will not silently shell",
        ),
        "process" => (
            "unsupported",
            "ACU partial migration: ps/process state/argv/cwd/environment/usage/watch/wait are live where the OS has a stable provider; fds/maps/threads/sockets/policy/cgroup and richer mutation remain typed gaps",
        ),
        "resource" => (
            "unsupported",
            "ACU migration gap: typed resource facade pending",
        ),
        "power" => (
            "unsupported",
            "ACU migration gap: terminal power actions need an explicit typed facade",
        ),
        "login-session" => (
            "unsupported",
            "ACU migration gap: session-global login actions need an explicit typed facade",
        ),
        "storage" => (
            "unsupported",
            "ACU migration gap: typed volume inventory facade pending",
        ),
        "file" => (
            "unsupported",
            "ACU migration gap: typed file plan/apply facade pending",
        ),
        "network" => (
            "available",
            "network-probe is live; typed interface/route/DNS inventory and socket-table facades remain gaps",
        ),
        "device" => (
            "unsupported",
            "ACU migration gap: typed device/audio lease facade pending",
        ),
        "privilege" => (
            "unsupported",
            "ACU migration gap: typed privilege-broker facade pending; no root shell",
        ),
        "runtime" => (
            "unsupported",
            "ACU migration gap: AgenTerm daemon/session facade pending",
        ),
        "desktop-helper" => (
            "unsupported",
            "cu-helper-mac is MCU desktop-helper; this binary uses libagenterm",
        ),
        "simulator" => (
            "unsupported",
            "ACU migration gap: CoreSimulator guest accessibility facade pending",
        ),
        "browser" => {
            if os == "macos" {
                (
                    "available",
                    "browser profiles (Local State + window inventory), browser open (open -na --profile-directory on the running instance), tab close (row close button); MV3/Native Messaging is an ACU migration gap, ordinary web is AX query/invoke",
                )
            } else if tree_live(os) {
                (
                    "available",
                    "browser profiles reads ~/.config Local State; browser open needs macOS open -na (typed unsupported); tab close is the a11y row close button",
                )
            } else {
                (
                    "unsupported",
                    "Chromium profile user data is not mapped on this OS; MV3/Native Messaging is an ACU migration gap",
                )
            }
        }
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
    if verb == "permissions" {
        return json!({
            "status": "available",
            "mode": "read-only-status-and-guidance",
            "grant": "observe",
            "reason": "reports the host permission model, affected verbs and repair guidance without changing consent",
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "inspect" || verb == "find" || verb == "read" {
        let reason = match verb {
            "find" => "MCU find HANDLE TEXT is query --window --text",
            "read" => "MCU read HANDLE SELECTOR is query --window --selector",
            _ => {
                "MCU inspect HANDLE is query --window; inspect --app inventory is an ACU migration gap"
            }
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
            "reason": "CDP /json target inventory (id, url, title, description, type, attached, websocket) when --remote-debugging-port answers; no listener is typed unsupported",
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if let Some((grant, reason)) = match verb {
        "page-find" => Some((
            "observe",
            "nodes of one CDP page target by --selector CSS (DOM.querySelectorAll) | --text SUB | --role R [--name SUB] (Accessibility.getFullAXTree): backend node id, path, role, name, text, box; zero -> cdp_node_not_found",
        )),
        "page-click" => Some((
            "actuate",
            "one node (typed ambiguity) or one frozen viewport point: Input.dispatchMouseEvent pressed + released on that target; node path verifies document/node change, point path verifies trusted down/up and attempts release cleanup; receipt",
        )),
        "page-download" => Some((
            "actuate",
            "one background page node click under an explicit Browser download policy; waits for correlated start/completion events and stats the GUID-named regular file without reading content; typed canceled/not-started/timeout/blocked; receipt",
        )),
        "page-hover" => Some((
            "actuate",
            "Input.dispatchMouseEvent mouseMoved at bounded viewport coordinates; verified by trusted mousemove target/coordinates versus elementFromPoint (:hover auxiliary); background target remains background; receipt",
        )),
        "page-scroll" => Some((
            "actuate",
            "Input.dispatchMouseEvent mouseWheel at bounded viewport coordinates; nearest scroll-container offsets read back, with an edge honestly performed but unverified; receipt",
        )),
        "page-drag" => Some((
            "actuate",
            "Input.dispatchMouseEvent left-button down/held-move/up between two bounded viewport points; trusted target-page event sequence read-back; release always attempted after press; receipt",
        )),
        "page-dialog" => Some((
            "actuate",
            "wait for Page.javascriptDialogOpening, accept/dismiss with Page.handleJavaScriptDialog, verify Page.javascriptDialogClosed; message and prompt contents redacted; receipt",
        )),
        "page-files" => Some((
            "actuate",
            "DOM.setFileInputFiles on one enabled input[type=file]; bounded regular local files, exact FileList basename/size read-back, no persisted local paths; receipt",
        )),
        "page-fill" => Some((
            "actuate",
            "one editable node: DOM.focus, optional select-all (--clear), Input.insertText, .value read-back (== TEXT), --submit sends Enter; focus emulation on for the write, off after; receipt",
        )),
        "page-type" => Some((
            "actuate",
            "the already-focused editable page element: Input.insertText, same-focus/value-growth read-back, plaintext redacted from replies and receipts; background target remains background; receipt",
        )),
        "page-nav" => Some((
            "actuate",
            "Page.navigate on that target (a background tab stays background), wait for Page.loadEventFired or --wait-ms; verified with the final url / title; receipt",
        )),
        "page-screenshot" => Some((
            "observe",
            "Page.captureScreenshot PNG to --out; a background / occluded tab may be refused as cdp_screenshot_unavailable and is never activated for it (--activate is the explicit actuate opt-in)",
        )),
        _ => None,
    } {
        return json!({
            "status": "available",
            "backend": crate::observe::page_js_backend(),
            "mode": "cdp",
            "grant": grant,
            "reason": reason,
            "target_selectors": ["--target-id", "--target-url", "--target-title", "--match"],
            "focus_changed": false,
            "group": group,
            "os": os,
            "verb": verb,
        });
    }
    if verb == "page-text" {
        let (status, reason) = if tree_live(os) {
            (
                "available",
                "visible text in reading order as {id, role, text} rows: --window reads the a11y tree (bounds; the active tab's web-area on macOS Chromium), --target-id/--target-url/--target-title/--match reads the CDP page target (backend cdp, background tabs, no focus change; id = backend DOM node id for page click/fill --node); pick a row then act on the node, never --coords or a screenshot",
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
    if verb == "browser-profiles" || verb == "browser-open" || verb == "tab-close" {
        let (status, mode, grant, reason) = match verb {
            "browser-profiles" => (
                if matches!(os, "macos" | "linux") {
                    "available"
                } else {
                    "unsupported"
                },
                "local-state+window-inventory",
                "observe",
                "profiles of the running Chromium-family browser (Local State profile.info_cache + last_used) joined to inventory windows by browser_profile; --app Brave Origin | Brave Browser | Google Chrome",
            ),
            "browser-open" => (
                if os == "macos" {
                    "available"
                } else {
                    "unsupported"
                },
                "open-na-profile-directory",
                "actuate",
                "open -na <app> --args --profile-directory=<dir> [URL] on the running instance (never a restart), verified by a new / retitled window of that browser_profile in the inventory; no CDP port needed",
            ),
            _ => (
                if tree_live(os) {
                    "available"
                } else {
                    "unsupported"
                },
                "a11y-tab-strip",
                "actuate",
                "press the tab row's own close button (button child of the tab radio-button) after the close gate (--title --exact --expect gone), verified by the strip read-back; no close button -> unsupported, never a keyboard shortcut",
            ),
        };
        return json!({
            "status": status,
            "mode": mode,
            "grant": grant,
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
        "raise",
        "minimize",
        "restore",
        "drag",
        "hit",
        "zoom",
        "snapshot",
        "diff",
        "spaces",
        "displays",
        "unlock",
        "inspect",
        "find",
        "read",
        "page-targets",
        "page-text",
        "page-find",
        "page-click",
        "page-hover",
        "page-scroll",
        "page-drag",
        "page-dialog",
        "page-files",
        "page-fill",
        "page-nav",
        "page-screenshot",
        "tab-list",
        "tab-select",
        "tab-close",
        "browser-profiles",
        "browser-open",
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

    /// The `semantic` row used to tell Linux and Windows callers that
    /// `unlock is macOS-only (no poke to map)`. That is wrong on Linux --
    /// the poke is the `org.a11y.Status` toggle -- and on Windows the
    /// reason is not "macOS-only" but "the UIA walk is the poke".
    #[test]
    fn semantic_row_names_each_hosts_unlock_mechanism() {
        let (status, macos) = group_status("semantic", "macos");
        assert_eq!(status, "available");
        assert!(macos.contains("unlock"), "{macos}");
        let (status, linux) = group_status("semantic", "linux");
        assert_eq!(status, "available");
        assert!(linux.contains("org.a11y.Status"), "{linux}");
        assert!(linux.contains("AXManualAccessibility"), "{linux}");
        let (status, windows) = group_status("semantic", "windows");
        assert_eq!(status, "available");
        assert!(windows.contains("WM_GETOBJECT"), "{windows}");
        for reason in [linux, windows] {
            assert!(
                !reason.contains("macOS-only"),
                "unlock is not macOS-only any more: {reason}"
            );
        }
    }

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
        for verb in [
            "page-find",
            "page-click",
            "page-hover",
            "page-scroll",
            "page-drag",
            "page-dialog",
            "page-files",
            "page-fill",
            "page-nav",
            "page-screenshot",
        ] {
            assert_eq!(group_id_for_verb(verb), "page-js", "{verb}");
            assert!(!is_align_verb(verb), "{verb} is live, not typed-only");
            let declaration = verb_declaration(verb);
            assert_eq!(declaration["mode"], "cdp", "{verb}");
            assert_eq!(declaration["status"], "available", "{verb}");
            assert_eq!(declaration["focus_changed"], false, "{verb}");
        }
        assert_eq!(verb_declaration("page-click")["grant"], "actuate");
        assert_eq!(verb_declaration("page-hover")["grant"], "actuate");
        assert_eq!(verb_declaration("page-scroll")["grant"], "actuate");
        assert_eq!(verb_declaration("page-drag")["grant"], "actuate");
        assert_eq!(verb_declaration("page-dialog")["grant"], "actuate");
        assert_eq!(verb_declaration("page-files")["grant"], "actuate");
        assert_eq!(verb_declaration("page-fill")["grant"], "actuate");
        assert_eq!(verb_declaration("page-nav")["grant"], "actuate");
        assert_eq!(verb_declaration("page-find")["grant"], "observe");
        assert_eq!(verb_declaration("page-screenshot")["grant"], "observe");
        assert!(
            typed_reason_for_verb("page")
                .contains("page find/click/hover/scroll/drag/dialog/files")
        );
        assert_eq!(group_id_for_verb("tab-select"), "semantic");
        assert_eq!(group_id_for_verb("tab-list"), "semantic");
        assert!(!is_align_verb("page-targets") && !is_align_verb("tab-select"));
        assert!(!is_align_verb("browser") && !is_align_verb("tab-close"));
        assert_eq!(group_id_for_verb("browser-profiles"), "browser");
        assert_eq!(group_id_for_verb("browser-open"), "browser");
        assert_eq!(group_id_for_verb("tab-close"), "browser");
        assert_eq!(verb_declaration("browser-profiles")["grant"], "observe");
        assert_eq!(verb_declaration("browser-open")["grant"], "actuate");
        assert_eq!(verb_declaration("tab-close")["grant"], "actuate");
        assert_eq!(verb_declaration("tab-close")["mode"], "a11y-tab-strip");
        if host_os() == "macos" {
            assert_eq!(verb_declaration("browser-open")["status"], "available");
            assert_eq!(group_status("browser", "macos").0, "available");
        } else {
            assert_eq!(verb_declaration("browser-open")["status"], "unsupported");
        }
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
        // The desktop-ring absorption turned these eight into live verbs
        // with their own parse arms, so they must NOT be typed-only any
        // more -- and `capabilities` must still declare every one of them.
        for absorbed in [
            "drag", "hit", "zoom", "snapshot", "diff", "raise", "minimize", "restore",
        ] {
            assert!(!is_align_verb(absorbed), "{absorbed} is a live verb now");
            assert_eq!(
                merged[absorbed]["status"], "available",
                "{absorbed} must stay declared in capabilities"
            );
        }
        // `ghost` (a cursor overlay drawn on the desktop) is still a declared
        // migration gap and stays typed rather than silently unknown.
        assert!(is_align_verb("ghost") && is_align_verb("page"));
        assert!(
            typed_reason_for_verb("ghost").contains("ACU migration gap"),
            "{}",
            typed_reason_for_verb("ghost")
        );
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
