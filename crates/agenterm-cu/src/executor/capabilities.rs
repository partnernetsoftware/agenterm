//! `capabilities`: the current target's truthful verb / mechanism /
//! permission declaration.

use super::*;

pub(super) fn capabilities_payload() -> serde_json::Value {
    let status = |capability: mechanism::Capability| {
        format!("{:?}", mechanism::capability_status(capability))
    };
    // The *verb* status is one stable word. `status()` above is the
    // capability's Debug form, which for `Available` happens to lowercase
    // into "available" and for anything else lowercases into the whole
    // struct -- `unsupported { reason: "host adapter unavailable" }` was
    // being published as a status value. Nothing on macOS could show that,
    // because every capability there is Available; running on Linux did.
    let verb_status = |capability: mechanism::Capability| -> &'static str {
        match mechanism::capability_status(capability) {
            mechanism::CapabilityStatus::Available => "available",
            mechanism::CapabilityStatus::Unsupported { .. } => "unsupported",
            mechanism::CapabilityStatus::Failed { .. } => "failed",
        }
    };
    // The reason, when there is one, belongs in its own field rather than
    // smuggled into the word a caller matches on.
    let verb_reason = |capability: mechanism::Capability| -> Option<String> {
        match mechanism::capability_status(capability) {
            mechanism::CapabilityStatus::Available => None,
            mechanism::CapabilityStatus::Unsupported { reason } => Some(reason),
            mechanism::CapabilityStatus::Failed { code, message } => {
                Some(format!("{code}: {message}"))
            }
        }
    };
    let capability_verb = |capability: mechanism::Capability, extra: serde_json::Value| {
        let mut declaration = serde_json::json!({ "status": verb_status(capability) });
        if let (Some(object), Some(reason)) = (declaration.as_object_mut(), verb_reason(capability))
        {
            object.insert("reason".into(), serde_json::json!(reason));
        }
        if let (Some(object), Some(extra)) = (declaration.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                object.insert(key.clone(), value.clone());
            }
        }
        declaration
    };
    // ABI 1.12: the a11y capability answers three ways. `Denied` is an OS
    // permission the caller can repair (macOS Accessibility); it is neither
    // "unsupported" (no adapter) nor an empty tree.
    let (tree_status, tree_verb) =
        match mechanism::capability_status(mechanism::Capability::AccessibilityTree) {
            mechanism::CapabilityStatus::Available => {
                ("Available", serde_json::json!({ "status": "available" }))
            }
            mechanism::CapabilityStatus::Failed { code, message }
                if code == "a11y_permission_denied" =>
            {
                (
                    "Denied",
                    serde_json::json!({
                        "status": "denied",
                        "reason": code,
                        "message": message,
                        "permission": "accessibility",
                        "repair": ACCESSIBILITY_REPAIR_PATH,
                    }),
                )
            }
            mechanism::CapabilityStatus::Failed { code, message } => (
                "Failed",
                serde_json::json!({ "status": "failed", "reason": code, "message": message }),
            ),
            mechanism::CapabilityStatus::Unsupported { reason } => (
                "Unsupported",
                serde_json::json!({ "status": "unsupported", "reason": reason }),
            ),
        };
    // The background menu bar is mapped on all three backends now, by two
    // different routes: macOS asks the application for its `AXMenuBar`,
    // while Linux and Windows find the menu-bar node in the window's own
    // bounded tree. The search route is a weaker claim -- a toolkit that
    // populates a closed menu lazily publishes nothing to find -- so those
    // two say which route they took rather than copying the tree status
    // unqualified.
    let menu_verb = if cfg!(target_os = "macos") {
        tree_verb.clone()
    } else {
        let mut declaration = tree_verb.clone();
        if let Some(object) = declaration.as_object_mut() {
            object.insert("mode".into(), serde_json::json!("tree-search"));
        }
        declaration
    };
    // The App-local focused control is read three ways: macOS asks the
    // application for its `AXFocusedUIElement`, while Linux and Windows
    // search the window's own bounded tree for the node the backend marks
    // focused (`STATE_FOCUSED` / `HasKeyboardFocus`). A search is a weaker
    // claim than a toolkit naming its own focus, so those two say which
    // route they took instead of copying the tree status unqualified.
    let focused_verb = if cfg!(target_os = "macos") {
        tree_verb.clone()
    } else {
        let mut declaration = tree_verb.clone();
        if let Some(object) = declaration.as_object_mut() {
            object.insert("mode".into(), serde_json::json!("state-search"));
        }
        declaration
    };
    // The destructive verb rides the platform's own close control on all
    // three hosts now: macOS AX `AXCloseButton`, Windows `WM_CLOSE`, and
    // Linux the EWMH `_NET_CLOSE_WINDOW` request. All three are requests,
    // not kills -- which is exactly why the gate reads the handle back
    // instead of trusting the call.
    let close_verb = capability_verb(mechanism::Capability::WindowOp, serde_json::json!({}));
    // Reading the pointer is an observation on every host, and it stays
    // available even where injection is not: the read never posts an event,
    // so it must not be gated behind the injection capability.
    let pointer_position_verb = serde_json::json!({
        "status": "available",
        "mode": "read-only",
        "group": "pointer",
    });
    // Injection is the opposite: it moves the *user's* real cursor or types
    // into whatever is frontmost, so the declaration says `desktop` scope
    // out loud. macOS has no window-local pointer route at all -- events
    // posted to a pid arrive without a window and no view ever sees them --
    // which is why `pointer-move --to <handle>` is refused rather than
    // approximated.
    // Both observation modes are declared, because they are not
    // interchangeable: polling carries `before` / `after` on every event,
    // notifications carry the order and arrival time of changes polling
    // never sees. `default` names the one a caller gets without asking.
    let observe_verb = {
        let mut declaration = tree_verb.clone();
        if let Some(object) = declaration.as_object_mut() {
            object.insert("default_mode".into(), serde_json::json!("poll-diff"));
            object.insert(
                "baseline_readiness".into(),
                serde_json::json!({
                    "poll-diff": "--ready-path atomically publishes after the complete baseline walk; caller owns cleanup",
                    "notifications": "unavailable until native subscription readiness is ordered",
                }),
            );
            object.insert(
                "modes".into(),
                serde_json::json!({
                    "poll-diff": "available",
                    "notifications": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                }),
            );
        }
        declaration
    };
    // The Screenshot capability covers the PNG *writer*, which every host
    // has. Capturing a window's pixels is a separate mechanism, and every
    // host now has one: Linux X11 GetImage, Windows GDI, and macOS
    // `CGWindowListCreateImage` resolved by dlsym -- removed from the SDK
    // in 15.0 but still in the framework and still capturing, measured.
    // The verb's status therefore comes from the mechanism like every
    // other verb, instead of a hardcoded refusal that outlived its reason.
    let screenshot_verb = capability_verb(
        mechanism::Capability::Screenshot,
        serde_json::json!({ "group": "capture" }),
    );
    let pointer_inject_verb = capability_verb(
        mechanism::Capability::InputInject,
        serde_json::json!({ "scope": "desktop", "group": "pointer" }),
    );
    // `zoom` is a clip of the same window capture `screenshot` takes, so it
    // lives or dies with that mechanism.
    let zoom_verb = {
        let mut declaration = screenshot_verb.clone();
        if let Some(object) = declaration.as_object_mut() {
            object.insert("mode".into(), serde_json::json!("window-capture-clip"));
        }
        declaration
    };
    // `drag` is pointer injection, and it says out loud that no host has a
    // window-local route today: the only path moves the user's real cursor,
    // which is why the verb requires `--degraded`.
    let drag_verb = {
        let mut declaration = pointer_inject_verb.clone();
        if let Some(object) = declaration.as_object_mut() {
            object.insert("grant".into(), serde_json::json!("actuate"));
            object.insert("mode".into(), serde_json::json!("press-move-release"));
            object.insert("window_local".into(), serde_json::json!(false));
            object.insert("requires".into(), serde_json::json!(["--degraded"]));
        }
        declaration
    };
    // `minimize` / `restore` need the window-op WRITE and the per-window
    // minimized READ; the read is what makes the postcondition checkable,
    // so a host without it must not claim the verbs are usable.
    let window_state_verb = {
        let mut declaration = capability_verb(
            mechanism::Capability::WindowOp,
            serde_json::json!({
                "group": "geometry",
                "mode": "window-minimize-affordance",
                "grant": "actuate",
                "activates_application": false,
                "gate": ["--window", "--expect"],
            }),
        );
        let readback = match mechanism::window_op::minimized(0) {
            // Handle 0 is never a window: `bad_handle` proves the export is
            // present and validating, which is what is being probed here.
            Err(mechanism::MechanismError::Failed { code, .. }) if code == "bad_handle" => {
                serde_json::json!("available")
            }
            Err(mechanism::MechanismError::Unsupported { reason }) => serde_json::json!(reason),
            Err(mechanism::MechanismError::Failed { code, message }) => {
                serde_json::json!(format!("{code}: {message}"))
            }
            Ok(_) => serde_json::json!("available"),
        };
        if let Some(object) = declaration.as_object_mut() {
            object.insert("minimized_readback".into(), readback);
        }
        declaration
    };
    let permissions = permissions_declaration();
    let process_state_verb = serde_json::json!({
        "status": "available",
        "group": "process",
        "mode": "agenterm-platform-process-observation",
        "grant": "observe",
        "fields": ["pid", "state", "start_identity", "reason", "verified"],
        "states": ["live", "dead", "unknown"],
    });
    let process_argv_verb = serde_json::json!({
        "status": "available",
        "group": "process",
        "mode": "native-bounded-argv",
        "grant": "observe",
        "identity_bound": true,
        "default_disclosure": "index-byte-length-sha256",
        "values": "explicit-opt-in",
        "max_arguments": 4096,
        "native_buffer_max_bytes": 1048576,
    });
    let process_cwd_verb = serde_json::json!({
        "status": if cfg!(windows) { "unsupported" } else { "available" },
        "group": "process",
        "mode": if cfg!(target_os = "linux") {
            "linux-proc-cwd"
        } else if cfg!(target_os = "macos") {
            "macos-libproc-vnodepath"
        } else {
            "unsupported"
        },
        "grant": "observe",
        "identity_bound": true,
        "path_disclosure": "explicit-command",
        "evidence": "path-byte-length-sha256",
        "windows": "typed-unsupported-no-public-api",
    });
    let process_environment_verb = serde_json::json!({
        "status": if cfg!(windows) { "unsupported" } else { "available" },
        "group": "process",
        "mode": if cfg!(target_os = "linux") {
            "linux-proc-environ"
        } else if cfg!(target_os = "macos") {
            "macos-kern-procargs2"
        } else {
            "unsupported"
        },
        "grant": "observe",
        "identity_bound": true,
        "semantics": "exec-initial",
        "default_disclosure": "name-and-value-byte-length-sha256",
        "values": "explicit-opt-in",
        "native_buffer_max_bytes": 4194304,
        "max_entries": 100001,
        "max_page_size": 5000,
        "windows": "typed-unsupported-no-public-api",
    });
    let process_usage_verb = serde_json::json!({
        "status": "available",
        "group": "process",
        "mode": "agenterm-platform-process-metrics",
        "grant": "observe",
        "identity_bound": true,
        "counter_encoding": "decimal-string",
        "fields": ["cpu_time_ns", "resident_bytes", "page_faults"],
        "watch": {
            "mode": "bounded-series",
            "clock": "monotonic",
            "max_duration_ms": 86400000,
            "max_interval_ms": 60000,
            "max_samples": 4096,
        },
    });
    let process_wait_verb = serde_json::json!({
        "status": "available",
        "group": "process",
        "mode": "agenterm-platform-process-reference",
        "grant": "observe",
        "identity_bound": true,
        "states": ["exited", "timeout"],
        "mechanism": "native-process-reference",
    });
    let process_kill_verb = serde_json::json!({
        "status": "available",
        "group": "process",
        "mode": "exact-native-process-reference",
        "grant": "actuate",
        "identity_bound": true,
        "destructive_gate": ["pid", "start_identity", "expect_exited"],
        "modes": {
            "linux": ["graceful", "forceful"],
            "windows": ["forceful"],
            "macos": [],
        },
        "macos_reason": "no exact-process signal primitive atomic against PID reuse",
        "receipt": "reserved-before-effect + verified-after-exit",
    });
    let process_watch_verb = serde_json::json!({
        "status": "available",
        "group": "process",
        "mode": "bounded-identity-diff",
        "grant": "observe",
        "identity_bound": true,
        "broad_selector_coverage": "explicit",
        "events": ["started", "exited"],
        "max_duration_ms": 86400000,
        "max_interval_ms": 60000,
        "max_events": 4096,
        "max_processes": 5000,
    });
    // Host-specific tree mapping only. Do not list unproven peers (live
    // RDP/UIA-over-RDP) as if this host ships them.
    let tree_mapping = current_tree_mapping();
    let mut payload = serde_json::json!({
        "target": "current",
        "transport": {
            "status": "in_process",
            "available": true,
        },
        "mechanism": "libagenterm",
        "mechanism_target": "current",
        "capabilities": {
            "windows": status(mechanism::Capability::WindowEnumerate),
            "tree": tree_status,
            "screenshot": status(mechanism::Capability::Screenshot),
            "input": status(mechanism::Capability::InputInject),
            "window_place": status(mechanism::Capability::WindowOp),
            "window_placement_inspect": status(mechanism::Capability::WindowPlacementInspect),
            "desktop_host": status(mechanism::Capability::DesktopHost),
        },
        "verbs": {
            "capabilities": { "status": "available" },
            "windows": capability_verb(mechanism::Capability::WindowEnumerate, serde_json::json!({})),
            "windows-watch": capability_verb(
                mechanism::Capability::WindowEnumerate,
                serde_json::json!({ "mode": "poll-diff", "group": "discover" }),
            ),
            "apps": {
                "status": verb_status(mechanism::Capability::WindowEnumerate),
                // `running_only` describes the *default*, not a limit:
                // `--all` adds the installed-but-not-running half where the
                // host can enumerate it.
                "running_only": true,
                "installed": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                "group": "discover",
            },
            "ps": {
                "status": "available",
                "group": "process",
                "mode": "agenterm-platform-process-list",
                "grant": "observe",
                "fields": ["pid", "parent_pid", "executable_name"],
                "filters": ["pid", "parent", "name", "offset", "max"],
                "migration_gaps": ["app", "command", "cpu", "memory", "files", "ports"],
            },
            "tree": tree_verb,
            "query": tree_verb,
            "inspect": crate::mcu_surface::verb_declaration("inspect"),
            "find": crate::mcu_surface::verb_declaration("find"),
            "read": crate::mcu_surface::verb_declaration("read"),
            "invoke": tree_verb,
            "verify": tree_verb,
            "menu-inspect": menu_verb,
            "menu-invoke": menu_verb,
            "focused": focused_verb,
            "observe": observe_verb,
            "close": close_verb,
            "orderwin": capability_verb(
                mechanism::Capability::WindowOp,
                serde_json::json!({ "group": "geometry", "mode": "raise" }),
            ),
            "screenshot": screenshot_verb,
            "click": {
                "status": tree_verb.get("status").cloned().unwrap_or(serde_json::json!("unsupported")),
                "grant": "actuate",
                "backend": "agt_a11y_node_perform",
                "group": "input-local",
            },
            "dclick": {
                "status": tree_verb.get("status").cloned().unwrap_or(serde_json::json!("unsupported")),
                "grant": "actuate",
                "alias_of": "click",
                "group": "input-local",
            },
            "receipts": { "status": "available" },
            // `hide` / `show` need an application-level hidden state, which
            // only macOS has; `quit` needs the application's own Quit menu
            // item, so it rides the menu verb's own status.
            "app": {
                "status": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                "group": "app",
                "actions": {
                    "hide": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                    "show": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                    "quit": if cfg!(target_os = "macos") { "available" } else { "mapped" },
                    "launch": if cfg!(target_os = "macos") { "available" } else { "unsupported" },
                },
                // `launch` cannot report a pid: the launcher service owns
                // the process it starts. Watch for the window instead.
                "launch_returns_pid": false,
                "quit_mechanism": "the application's own Quit menu item, pressed in the background; never a signal",
                "destructive": ["quit"],
            },
            "page-js": {
                "status": "available",
                "backend": observe::page_js_backend(),
                "mode": "cdp",
                "reason": observe::page_js_unsupported_reason(),
                "target_selectors": ["--target-id", "--target-url", "--target-title", "--match"],
                "background_tabs": "Runtime.evaluate reaches a background tab by target; no focus change",
            },
            "spaces": crate::mcu_surface::verb_declaration("spaces"),
            "displays": crate::mcu_surface::verb_declaration("displays"),
            "pointer-position": pointer_position_verb,
            "pointer-move": pointer_inject_verb,
            "send-keys": capability_verb(
                mechanism::Capability::InputInject,
                serde_json::json!({ "scope": "desktop", "group": "input" }),
            ),
            "send-text": capability_verb(
                mechanism::Capability::InputInject,
                serde_json::json!({ "scope": "desktop", "group": "input", "alias_of": "type" }),
            ),
        },
        "permissions": permissions,
        "mcu_groups": crate::mcu_surface::GROUPS.iter().map(|g| g.id).collect::<Vec<_>>(),
        "alignment_tsv": crate::mcu_surface::alignment_matrix_text(),
        "mapping": {
            "windows": "libagenterm agt_window_enumerate",
            "tree": tree_mapping,
            "window_place": "Spectacle catalog via libagenterm agt_native_window_*",
        },
        "gaps": {
            "windows": "none — shared agenterm.dll (milestone 46)",
            "screenshot": "none — shared agenterm.dll (milestone 46)",
            "input_degraded": "none — shared agenterm.dll (milestone 46)",
            "rdp_live": "rdp tier is placeholder; never declared available on current",
            "macos_ax_live": "macOS AX observe (windows / tree / query), semantic actuation (invoke / verify / click / focus), background menus (menu inspect / invoke), the App-local focused control (focused / invoke --focused), the poll-diff observation stream (observe), the destructive close (gate: exact target + snapshot + postcondition) with crash-persistent receipts (receipts), the read-only pointer position and the window-place frame transaction are proven by scripts/qjs/cu-macos-smoke.qjs; invoke offers no quit / delete action; AX notifications are not subscribed (observe is poll-diff)",
        }
    });
    // The desktop-ring verbs are inserted rather than written into the
    // literal above: `serde_json::json!` is one recursive macro expansion
    // and the literal is already at the expander's depth limit.
    if let Some(verbs) = payload["verbs"].as_object_mut() {
        verbs.insert("process-state".into(), process_state_verb);
        verbs.insert("process-argv".into(), process_argv_verb);
        verbs.insert("process-cwd".into(), process_cwd_verb);
        verbs.insert("process-environment".into(), process_environment_verb);
        verbs.insert("process-usage".into(), process_usage_verb);
        verbs.insert("process-wait".into(), process_wait_verb);
        verbs.insert("process-kill".into(), process_kill_verb);
        verbs.insert("process-watch".into(), process_watch_verb);
        verbs.insert(
            "network-probe".into(),
            serde_json::json!({
                "status": "available",
                "group": "network",
                "grant": "observe",
                "mode": "system-resolver-owned-worker",
                "resolution": "once-deduplicated-frozen",
                "attempts": "exact-round-robin",
                "cleanup": "deadline-kill-and-reap",
            }),
        );
        verbs.insert(
            "file-inspect".into(),
            serde_json::json!({
                "status": "available",
                "group": "file",
                "grant": "observe",
                "mode": "final-entry-metadata-plus-stable-object-identity",
                "link_policy": "never-follow-final-link",
                "wide_values": "decimal-strings",
            }),
        );
        for (verb, grant, mode) in [
            ("pty-start", "actuate", "isolated-headless-job-start"),
            (
                "pty-list",
                "observe",
                "durable-job-authority-reconciliation",
            ),
            ("pty-prune", "actuate", "verified-stale-state-reclamation"),
            ("pty-status", "observe", "identity-bound-job-status"),
            ("pty-read", "observe", "loss-aware-retained-raw-byte-cursor"),
            (
                "pty-snapshot",
                "observe",
                "bounded-structured-job-screen-with-event-cursor",
            ),
            ("pty-events", "observe", "loss-aware-job-event-continuation"),
            ("pty-resize", "actuate", "verified-job-terminal-grid-resize"),
            ("pty-send", "actuate", "literal-headless-pty-input"),
            ("pty-wait", "observe", "loss-aware-retained-byte-wait"),
            ("pty-wait-exit", "observe", "drained-exit-status-wait"),
            ("pty-stop", "actuate", "verified-authority-disappearance"),
            ("terminal-list", "observe", "structured-ui-bootstrap"),
            ("terminal-new", "actuate", "verified-owned-tab-create"),
            (
                "terminal-close",
                "actuate",
                "verified-owned-tab-disappearance",
            ),
            ("terminal-read", "observe", "bounded-screen-snapshot"),
            (
                "terminal-snapshot",
                "observe",
                "bounded-structured-screen-with-event-cursor",
            ),
            (
                "terminal-events",
                "observe",
                "loss-aware-ordered-event-cursor",
            ),
            (
                "terminal-output",
                "observe",
                "loss-aware-retained-raw-byte-cursor",
            ),
            ("terminal-send", "actuate", "literal-owned-pty-input"),
            (
                "terminal-wait",
                "observe",
                "bounded-screen-or-lifecycle-wait",
            ),
        ] {
            verbs.insert(
                verb.into(),
                serde_json::json!({
                    "status": "available",
                    "group": if verb.starts_with("pty-") { "headless-pty-job" } else { "agenterm-terminal" },
                    "grant": grant,
                    "mode": mode,
                    "transport": "agenterm-control-protocol",
                    "requires_running_agenterm": !verb.starts_with("pty-"),
                }),
            );
        }
        verbs.insert(
            "activate".into(),
            capability_verb(
                mechanism::Capability::WindowOp,
                serde_json::json!({
                    "group": "geometry",
                    "mode": "desktop-foreground",
                    "grant": "actuate",
                    "activates_application": true,
                    "readback": "window-inventory-focused",
                }),
            ),
        );
        // `raise` is the window-op mechanism the same way `orderwin` is.
        verbs.insert(
            "raise".into(),
            capability_verb(
                mechanism::Capability::WindowOp,
                serde_json::json!({
                    "group": "geometry",
                    "mode": "within-application-raise",
                    "grant": "actuate",
                    "activates_application": false,
                }),
            ),
        );
        // `minimize` / `restore` need the window-op WRITE *and* the
        // per-window minimized READ; the read is what makes the
        // postcondition checkable, so both are declared.
        verbs.insert("minimize".into(), window_state_verb.clone());
        verbs.insert("restore".into(), window_state_verb);
        // Observation over the same bounded walk `tree` / `query` use, so
        // they carry the tree's own status.
        verbs.insert("hit".into(), tree_verb.clone());
        verbs.insert("snapshot".into(), tree_verb.clone());
        verbs.insert("diff".into(), tree_verb.clone());
        verbs.insert("zoom".into(), zoom_verb);
        verbs.insert("drag".into(), drag_verb);
        verbs.insert(
            "doctor".into(),
            serde_json::json!({
                "status": "available",
                "group": "setup",
                "grant": "observe",
                "mode": "bounded-read-only-health-receipt",
                "reason": "live window/display probes plus canonical permissions and capabilities declarations; no setup mutation",
            }),
        );
    }
    if let Some(verbs) = payload.get("verbs").cloned() {
        payload["verbs"] = crate::mcu_surface::merge_verbs(verbs);
    }
    attach_verb_grants(&mut payload);
    attach_invoke_actions(&mut payload);
    payload
}

/// One place to look for "what am I not allowed to do, and how is that
/// fixed". `setup` stays a typed gap; `doctor` and `permissions` provide
/// read-only reporting. The reporting has to be complete, and until now the
/// repair path was buried inside the `tree` verb while input injection
/// depends on the very same grant. A caller should not have to know
/// that to find it.
pub(super) fn permissions_declaration() -> serde_json::Value {
    if cfg!(target_os = "macos") {
        let accessibility =
            match mechanism::capability_status(mechanism::Capability::AccessibilityTree) {
                mechanism::CapabilityStatus::Available => serde_json::json!({
                    "status": "granted",
                }),
                mechanism::CapabilityStatus::Failed { code, message }
                    if code == "a11y_permission_denied" =>
                {
                    serde_json::json!({
                        "status": "denied",
                        "reason": code,
                        "message": message,
                        "repair": ACCESSIBILITY_REPAIR_PATH,
                    })
                }
                other => serde_json::json!({
                    "status": "unknown",
                    "detail": format!("{other:?}"),
                }),
            };
        serde_json::json!({
            "accessibility": {
                "grant": accessibility,
                // Every verb that stops working when this grant is missing,
                // including the input verbs: on macOS the same Accessibility
                // entry gates posting events.
                "gates": [
                    "tree", "query", "invoke", "verify", "wait", "focused",
                    "observe", "menu-inspect", "menu-invoke", "click", "focus",
                    "send-text", "send-keys", "scroll", "get-extents", "select",
                    "get-selection", "set-caret", "get-caret", "get-text",
                    "close", "unlock", "pointer-move", "pointer-position",
                ],
            },
            // Screen Recording really is required now that window capture
            // works, and this does not claim to know whether it is held:
            // the capture API reports "no permission" and "the window is
            // gone" the same way, so the honest report is that the grant
            // gates the verb, with where to fix it -- not a guess at its
            // state dressed as a reading.
            "screen_recording": {
                "grant": {
                    "status": "required",
                    "reason": "window capture reads the window's pixels; macOS gates that on Screen Recording",
                    "repair": SCREEN_RECORDING_REPAIR_PATH,
                },
                "gates": ["screenshot"],
            },
        })
    } else {
        serde_json::json!({
            "model": "none",
            "reason": "this host has no per-application permission gate; a mechanism is available or it is not",
        })
    }
}

pub(super) fn permissions_payload() -> serde_json::Value {
    serde_json::json!({
        "platform": crate::mcu_surface::host_os(),
        "permissions": permissions_declaration(),
        "action": {
            "performed": false,
            "reason": "status-only; operating-system consent remains user controlled",
        },
    })
}

fn doctor_check(result: Result<serde_json::Value, CuError>, count: &str) -> serde_json::Value {
    match result {
        Ok(value) => serde_json::json!({
            "status": "available",
            "count": value.get(count).and_then(serde_json::Value::as_u64),
        }),
        Err(error) => serde_json::json!({
            "status": "failed",
            "error": error_payload(&error),
        }),
    }
}

/// One bounded, read-only answer for an agent deciding whether this host is
/// ready for desktop work. The declarations are embedded rather than
/// reconstructed so `doctor`, `permissions` and `capabilities` cannot drift.
pub(super) fn doctor_payload() -> serde_json::Value {
    let permissions = permissions_declaration();
    let windows = doctor_check(
        windows_payload(observe::WindowFilter::default(), None, Some(0), Some(1)),
        "returned",
    );
    let displays = doctor_check(displays_payload(), "returned");
    let mechanism_degraded = [&windows, &displays]
        .iter()
        .any(|check| check["status"] != "available");
    // Only macOS currently exposes an inspectable desktop-consent state. A
    // missing or unknown required grant is actionable diagnosis, so `doctor`
    // must not call the host ready merely because the underlying API loaded.
    let permission_degraded = permissions
        .pointer("/accessibility/grant/status")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|status| status != "granted");
    let degraded = mechanism_degraded || permission_degraded;
    serde_json::json!({
        "schema": 1,
        "platform": crate::mcu_surface::host_os(),
        "status": if degraded { "degraded" } else { "ready" },
        "checks": {
            "windows": windows,
            "displays": displays,
        },
        "permissions": permissions,
        "capabilities": capabilities_payload(),
        "action": {
            "performed": false,
            "reason": "diagnosis-only; setup, consent and helper lifecycle are separate operations",
        },
    })
}

/// Every verb's `grant` (`observe` / `actuate`), filled in only where the
/// declaration did not already say.
fn attach_verb_grants(payload: &mut serde_json::Value) {
    if let Some(verbs) = payload["verbs"].as_object_mut() {
        // The CDP background-tab verbs: one declaration each, kept out of
        // the literal above (the json! macro has a recursion budget).
        for verb in [
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
        ] {
            verbs.insert(verb.to_owned(), crate::mcu_surface::verb_declaration(verb));
        }
        for (verb, grant) in [
            ("windows", "observe"),
            ("windows-watch", "observe"),
            ("apps", "observe"),
            ("tree", "observe"),
            ("query", "observe"),
            ("inspect", "observe"),
            ("find", "observe"),
            ("read", "observe"),
            ("verify", "observe"),
            ("wait", "observe"),
            ("focused", "observe"),
            ("observe", "observe"),
            ("screenshot", "observe"),
            ("pointer-position", "observe"),
            ("clipboard-read", "observe"),
            ("get-text", "observe"),
            ("capabilities", "observe"),
            ("page-js", "observe"),
            ("page-targets", "observe"),
            ("page-find", "observe"),
            ("page-screenshot", "observe"),
            ("page-click", "actuate"),
            ("page-download", "actuate"),
            ("page-hover", "actuate"),
            ("page-scroll", "actuate"),
            ("page-drag", "actuate"),
            ("page-dialog", "actuate"),
            ("page-files", "actuate"),
            ("page-fill", "actuate"),
            ("page-nav", "actuate"),
            ("tab-list", "observe"),
            ("browser-profiles", "observe"),
            ("click", "actuate"),
            ("tab-select", "actuate"),
            ("tab-close", "actuate"),
            ("browser-open", "actuate"),
            ("dclick", "actuate"),
            ("invoke", "actuate"),
            ("menu-invoke", "actuate"),
            ("send-keys", "actuate"),
            ("send-text", "actuate"),
            ("pointer-move", "actuate"),
            ("unlock", "actuate"),
            ("close", "actuate"),
            ("orderwin", "actuate"),
            ("app", "actuate"),
        ] {
            if let Some(object) = verbs.get_mut(verb).and_then(|value| value.as_object_mut()) {
                object
                    .entry("grant")
                    .or_insert_with(|| serde_json::json!(grant));
            }
        }
    }
}

/// The `invoke` action vocabulary: `mapped` says the ABI carries the
/// action; whether a node offers it is the backend's answer at call time.
fn attach_invoke_actions(payload: &mut serde_json::Value) {
    if let Some(invoke) = payload["verbs"]["invoke"].as_object_mut() {
        invoke.insert(
            "actions".into(),
            serde_json::json!({
                "press": "mapped",
                "set-value": "mapped",
                "select-option": "mapped",
                "set-checked": "mapped",
                "set-expanded": "mapped",
                "increment": "mapped",
                "decrement": "mapped",
                "scroll-to": "mapped",
                "set-selection": "mapped",
                "set-selected": "mapped",
                "cancel": "mapped",
                "show-default-ui": "mapped",
            }),
        );
    }
}

pub(super) fn current_tree_mapping() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "libagenterm agt_a11y_* → Linux AT-SPI2"
    }
    #[cfg(windows)]
    {
        "libagenterm agt_a11y_* → Windows UIA"
    }
    #[cfg(target_os = "macos")]
    {
        "libagenterm agt_a11y_* → macOS AX (observe + invoke live: cu-macos-smoke)"
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        "libagenterm agt_a11y_*"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_capabilities_names_current_target() {
        let reply = observe_executor().execute(&Command::Capabilities {
            target: TargetRef::Current,
        });
        assert!(reply.ok);
        assert_eq!(reply.target, "current");
        assert_eq!(reply.command, "capabilities");
        let data = reply.data.as_ref().expect("data");
        assert_eq!(data["target"], "current");
        assert_eq!(data["transport"]["status"], "in_process");
        assert_eq!(data["transport"]["available"], true);
        assert_eq!(data["verbs"]["capabilities"]["status"], "available");
        assert_eq!(data["verbs"]["pty"]["status"], "unsupported");
        assert_eq!(
            data["verbs"]["page-js"]["backend"],
            "debugger-runtime-evaluate"
        );
        assert!(
            data["mcu_groups"].as_array().map(|g| g.len()).unwrap_or(0)
                >= crate::mcu_surface::GROUPS.len()
        );
        let tsv = data["alignment_tsv"].as_str().unwrap_or("");
        assert!(tsv.contains("shell-pty-job\tlinux\tunsupported\t"));
        assert!(!tsv.contains("still-gap"));
        assert_eq!(data["verbs"]["windows-watch"]["mode"], "poll-diff");
        assert_eq!(data["verbs"]["windows-watch"]["group"], "discover");
        assert_eq!(data["verbs"]["apps"]["running_only"], true);
        assert_eq!(data["verbs"]["apps"]["group"], "discover");
        assert_eq!(data["verbs"]["orderwin"]["mode"], "raise");
        assert_eq!(data["verbs"]["orderwin"]["group"], "geometry");
        assert_ne!(data["verbs"]["orderwin"]["status"], "");
        assert_eq!(data["verbs"]["activate"]["mode"], "desktop-foreground");
        assert_eq!(data["verbs"]["activate"]["activates_application"], true);
        assert_eq!(data["verbs"]["page-js"]["status"], "available");
        assert_eq!(data["verbs"]["page-js"]["mode"], "cdp");
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
            assert_eq!(data["verbs"][verb]["mode"], "cdp", "{verb}");
            assert_eq!(data["verbs"][verb]["status"], "available", "{verb}");
            assert_eq!(data["verbs"][verb]["focus_changed"], false, "{verb}");
        }
        assert_eq!(data["verbs"]["page-click"]["grant"], "actuate");
        assert_eq!(data["verbs"]["page-hover"]["grant"], "actuate");
        assert_eq!(data["verbs"]["page-scroll"]["grant"], "actuate");
        assert_eq!(data["verbs"]["page-drag"]["grant"], "actuate");
        assert_eq!(data["verbs"]["page-dialog"]["grant"], "actuate");
        assert_eq!(data["verbs"]["page-files"]["grant"], "actuate");
        assert_eq!(data["verbs"]["page-find"]["grant"], "observe");
        assert_eq!(
            data["verbs"]["invoke"]["actions"]["set-selection"],
            "mapped"
        );
        // `mapped`, not `available`: the ABI carries these three now, but
        // whether a given node offers AXCancel / AXSelected / AXShowDefaultUI
        // is the backend's answer at call time, not a promise made here.
        assert_eq!(data["verbs"]["invoke"]["actions"]["cancel"], "mapped");
        assert_eq!(data["verbs"]["invoke"]["actions"]["set-selected"], "mapped");
        assert_eq!(
            data["verbs"]["invoke"]["actions"]["show-default-ui"],
            "mapped"
        );
        assert_eq!(data["verbs"]["displays"]["group"], "geometry");
        assert_eq!(data["verbs"]["displays"]["status"], "available");
        assert_eq!(data["verbs"]["spaces"]["group"], "geometry");
        if cfg!(target_os = "macos") {
            assert_eq!(data["verbs"]["spaces"]["status"], "available");
        } else {
            assert_eq!(data["verbs"]["spaces"]["status"], "unsupported");
        }
        assert_ne!(data["verbs"]["windows-watch"]["status"], "");
        assert_ne!(data["verbs"]["apps"]["status"], "");
        // Must not declare live RDP or unproven Mac AX as available.
        assert!(data["gaps"]["rdp_live"].as_str().is_some());
        assert!(data["gaps"]["macos_ax_live"].as_str().is_some());
        let mapping = data["mapping"]["tree"].as_str().unwrap_or("");
        assert!(
            !mapping.contains("RDP") && !mapping.to_lowercase().contains("rdp live"),
            "current mapping must not claim live RDP: {mapping}"
        );
    }

    #[test]
    fn permissions_is_a_live_read_only_facade_over_the_same_declaration() {
        let reply = observe_executor().execute(&Command::Permissions {
            target: TargetRef::Current,
        });
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.command, "permissions");
        let data = reply.data.expect("permission data");
        assert_eq!(data["platform"], crate::mcu_surface::host_os());
        assert_eq!(data["action"]["performed"], false);
        assert_eq!(data["permissions"], permissions_declaration());
    }

    #[test]
    fn doctor_reuses_declarations_and_keeps_probe_failures_in_the_report() {
        let reply = observe_executor().execute(&Command::Doctor {
            target: TargetRef::Current,
        });
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.command, "doctor");
        let data = reply.data.expect("doctor data");
        assert_eq!(data["schema"], 1);
        assert_eq!(data["platform"], crate::mcu_surface::host_os());
        assert!(matches!(
            data["status"].as_str(),
            Some("ready" | "degraded")
        ));
        assert!(data["checks"]["windows"]["status"].is_string());
        assert!(data["checks"]["displays"]["status"].is_string());
        assert_eq!(data["permissions"], permissions_declaration());
        assert_eq!(data["capabilities"], capabilities_payload());
        assert_eq!(
            data["capabilities"]["verbs"]["doctor"]["status"],
            "available"
        );
        assert_eq!(data["action"]["performed"], false);
        let checks_failed = ["windows", "displays"]
            .iter()
            .any(|check| data["checks"][check]["status"] != "available");
        let permission_failed = data["permissions"]
            .pointer("/accessibility/grant/status")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|status| status != "granted");
        assert_eq!(
            data["status"],
            if checks_failed || permission_failed {
                "degraded"
            } else {
                "ready"
            }
        );
    }
}
