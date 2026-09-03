//! Application lifecycle and the destructive gate: `app launch|hide|show|quit`
//! and `close`, each read back through the window inventory.

use super::*;

// ---------------------------------------------------------------------------
// The destructive verb and the receipt read-back (slice 4 of
// plan/design-mcu-absorption.md).
// ---------------------------------------------------------------------------

/// Bounds of the prior snapshot a destructive action writes to its receipt.
pub(super) const CLOSE_SNAPSHOT_DEPTH: u32 = 6;

pub(super) const CLOSE_SNAPSHOT_NODES: usize = 500;

/// How long the postcondition read-back polls the window inventory.
pub(super) const CLOSE_READBACK: Duration = Duration::from_millis(2_500);

pub(super) const CLOSE_READBACK_POLL: Duration = Duration::from_millis(50);

/// The three-part destructive gate (PRD_02_31), checked before any
/// inventory or tree read: every missing part is named in one refusal.
pub(super) fn destructive_gate(
    window: isize,
    snapshot: bool,
    expect: Option<&str>,
) -> Result<(), CuError> {
    let mut missing = Vec::new();
    if window == 0 {
        missing.push("target");
    }
    if !snapshot {
        missing.push("snapshot");
    }
    match expect {
        Some("gone") => {}
        Some(other) => {
            return Err(invalid_input(format!(
                "close --expect accepts only 'gone' (the window is read back as absent), got {other:?}"
            )));
        }
        None => missing.push("postcondition"),
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(CuError::new(
        "refused",
        "close is destructive: it needs an exact target (--window HANDLE), a prior snapshot \
         (--snapshot) and a checkable postcondition (--expect gone); nothing was performed",
    )
    .with_detail(serde_json::json!({
        "reason": "destructive_gate",
        "missing": missing,
        "required": {
            "target": "--window HANDLE [--pid N] [--title T]",
            "snapshot": "--snapshot",
            "postcondition": "--expect gone",
        },
        "effect": "not_performed",
    })))
}

/// A compact node record for the snapshot a receipt carries.
pub(super) fn snapshot_node_json(node: &mechanism::A11yNode) -> serde_json::Value {
    let text = node.text.as_deref().map(|text| {
        let mut end = text.len().min(200);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_owned()
    });
    serde_json::json!({
        "id": node.id,
        "role": node.role,
        "name": node.name,
        "identifier": node.identifier,
        "text": text,
        "states": node.states,
    })
}

pub(super) fn window_identity_json(row: &WindowInfo) -> serde_json::Value {
    serde_json::json!({
        "handle": row.handle,
        "ref": observe::window_stable_ref(row),
        "pid": row.process_id,
        "app": row.app_name,
        "title": row.title,
        "bounds": row.bounds,
        "focused": row.focused,
    })
}

/// Close one top-level window through the platform's close control, in the
/// background. Order: gate → exact target bound in one inventory read →
/// prior snapshot → receipt reserved → close → postcondition read back
/// (absent from the inventory) → receipt completed → reply.
/// `app launch --path P`: ask the host to start an application.
///
/// The reply says the request was **accepted**, not that the application
/// is up, and says it in a field rather than in prose. Every host route
/// hands the new process to a launcher service that owns it, so no pid
/// comes back and none is invented: the caller watches for the window,
/// which is also the only evidence the application really started rather
/// than merely being asked to.
pub(super) fn launch_payload(path: Option<&str>) -> Result<serde_json::Value, CuError> {
    let Some(path) = path else {
        return Err(invalid_input(
            "app launch requires --path <application> (as `apps --all` lists it)".into(),
        ));
    };
    mechanism::launch_app(path).map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "addressing": "application-path",
        "mechanism": "libagenterm",
        "action": "launch",
        "path": path,
        "requested": true,
        // Deliberately not `performed` / `verified`: the launcher owns the
        // process, so this call cannot know either one. Watch for the window.
        "pid": serde_json::Value::Null,
        "pid_source": "none: the launcher service owns the process; watch for its window",
    }))
}

/// `app hide|show|quit` on the application owning `window`.
///
/// `hide` / `show` are the application stepping aside and back: nothing is
/// closed, and asking for the state it is already in performs nothing.
/// `quit` ends an application, so it carries the same three-part gate as
/// `close` -- an exact target, a prior snapshot, and a checkable
/// postcondition -- and its mechanism is the application's **own Quit menu
/// item**, pressed in the background. A signal would be a kill, not a
/// quit: the application would lose its chance to run its shutdown path
/// and ask about unsaved work.
pub(super) fn app_payload(
    window: isize,
    action: crate::command::AppAction,
    snapshot: bool,
    expect: Option<&str>,
    pid: Option<u32>,
    path: Option<&str>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    use crate::command::AppAction;
    if matches!(action, AppAction::Launch) {
        return launch_payload(path);
    }
    if window == 0 && pid.is_none() {
        return Err(invalid_input(
            "app requires --window <handle> (a non-zero handle from `windows`) or --pid <n>".into(),
        ));
    }
    if action.is_destructive() && window == 0 {
        return Err(invalid_input(
            "app quit requires --window <handle>: the gate needs an exact target and a prior snapshot of it".into(),
        ));
    }
    if !action.is_destructive() {
        if snapshot || expect.is_some() {
            return Err(invalid_input(format!(
                "app {} takes no --snapshot / --expect; those belong to the destructive quit",
                action.as_str()
            )));
        }
        let hidden = matches!(action, AppAction::Hide);
        // Hiding takes the application's windows out of the inventory, so
        // `show` cannot be addressed by a handle that no longer resolves:
        // it needs the pid, which outlives the hide. `hide` accepts either
        // and looks the pid up while the window is still there.
        let process_id = match (pid, hidden) {
            (Some(pid), _) => pid,
            (None, true) => {
                let windows = mechanism::window_enumerate::enumerate_top_level()
                    .map_err(map_mechanism_err)?;
                let Some(row) = windows.iter().find(|row| row.handle == window) else {
                    return Err(CuError::new(
                        "window_not_found",
                        format!("no top-level window with handle {window}"),
                    ));
                };
                row.process_id
            }
            (None, false) => {
                return Err(invalid_input(
                    "app show needs --pid: hiding removed the application's windows, so a window handle no longer names it".into(),
                ));
            }
        };
        mechanism::set_application_hidden(process_id, hidden).map_err(map_mechanism_err)?;
        // Read the inventory back, because what "put away" looks like is
        // not the same on every host and neither shape is the definition.
        // macOS stops enumerating a hidden application's windows at all;
        // X11 keeps them listed and marks them `_NET_WM_STATE_HIDDEN`.
        // Checking only for "gone" reported a working Linux hide as
        // unverified, so a window still on screen is what fails this --
        // absent and minimized both count as away.
        //
        // Polled, not sampled once: the adapter already waited for the
        // state to settle, but the window server updates its own list a
        // beat later, and a single read right after the write catches the
        // old inventory.
        let started = Instant::now();
        let mut listed = usize::MAX;
        let mut on_screen = usize::MAX;
        while started.elapsed() < CLOSE_READBACK {
            let windows =
                mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
            let mine: Vec<_> = windows
                .iter()
                .filter(|row| row.process_id == process_id)
                .collect();
            listed = mine.len();
            on_screen = mine.iter().filter(|row| !row.minimized).count();
            if (on_screen == 0) == hidden {
                break;
            }
            thread::sleep(CLOSE_READBACK_POLL);
        }
        return Ok(serde_json::json!({
            "addressing": "process-id",
            "mechanism": "libagenterm",
            "process_id": process_id,
            "action": action.as_str(),
            "performed": true,
            "windows_listed": listed,
            "windows_on_screen": on_screen,
            "verified": (on_screen == 0) == hidden,
            "verification": {
                "method": "window-inventory-by-pid",
                "elapsed_ms": started.elapsed().as_millis(),
            },
        }));
    }
    destructive_gate(window, snapshot, expect)?;
    let not_performed = |error: CuError| {
        let mut detail = error.detail.clone().unwrap_or(serde_json::json!({}));
        detail["effect"] = serde_json::json!("not_performed");
        error.with_detail(detail)
    };
    let windows = mechanism::window_enumerate::enumerate_top_level()
        .map_err(map_mechanism_err)
        .map_err(not_performed)?;
    let Some(row) = windows.iter().find(|item| item.handle == window) else {
        return Err(not_performed(CuError::new(
            "window_not_found",
            format!("no top-level window with handle {window}"),
        )));
    };
    if let Some(pid) = pid
        && row.process_id != pid
    {
        return Err(not_performed(
            CuError::new(
                "window_identity_mismatch",
                format!(
                    "window {window} belongs to pid {} not {pid}; refusing to quit another process",
                    row.process_id
                ),
            )
            .with_detail(
                serde_json::json!({ "expected": { "pid": pid }, "observed": window_identity_json(row) }),
            ),
        ));
    }
    let identity = window_identity_json(row);
    let application = row.app_name.clone();
    let target_pid = row.process_id;
    let tree = mechanism::tree_for_window_bounded(
        Some(window),
        mechanism::TreeBudget {
            max_depth: Some(CLOSE_SNAPSHOT_DEPTH),
            max_nodes: Some(CLOSE_SNAPSHOT_NODES),
        },
    )
    .map_err(map_mechanism_err)
    .map_err(not_performed)?;
    let snapshot_json = serde_json::json!({
        "backend": tree.backend,
        "budget": { "depth": CLOSE_SNAPSHOT_DEPTH, "max_nodes": CLOSE_SNAPSHOT_NODES },
        "visited": tree.visited,
        "returned": tree.returned,
        "truncated": tree.truncated,
        "nodes": tree.nodes.iter().map(snapshot_node_json).collect::<Vec<_>>(),
    });
    // The application's own Quit item, by the two spellings a menu bar
    // uses. Resolved through the same background menu path `menu invoke`
    // uses, so a missing / duplicated / disabled item refuses there with
    // nothing pressed.
    let candidates = [
        vec![application.clone(), format!("Quit {application}")],
        vec![application.clone(), "Quit".to_owned()],
    ];
    let ticket = receipts.reserve(
        "app-quit",
        window,
        serde_json::json!({
            "action": "quit",
            "window_identity": identity,
            "postcondition": "gone",
            "before": { "present": true, "nodes": tree.returned },
            "snapshot": snapshot_json,
        }),
    )?;
    let started = Instant::now();
    let mut mechanism_error = None;
    let mut pressed_path: Option<Vec<String>> = None;
    for path in &candidates {
        match mechanism::invoke_menu_path(Some(window), path) {
            Ok(_) => {
                pressed_path = Some(path.clone());
                mechanism_error = None;
                break;
            }
            Err(error) => mechanism_error = Some(map_mechanism_err(error)),
        }
    }
    // Postcondition: no window of that process is left in the inventory.
    let mut polls = 0usize;
    let mut present = true;
    let mut readback_error = None;
    while started.elapsed() < CLOSE_READBACK {
        polls += 1;
        match mechanism::window_enumerate::enumerate_top_level() {
            Ok(now) => present = now.iter().any(|item| item.process_id == target_pid),
            Err(error) => {
                readback_error = Some(map_mechanism_err(error));
                break;
            }
        }
        if !present || mechanism_error.is_some() {
            break;
        }
        thread::sleep(CLOSE_READBACK_POLL);
    }
    let verified = !present && mechanism_error.is_none() && readback_error.is_none();
    let reason = if mechanism_error.is_some() {
        Some("mechanism_failed")
    } else if readback_error.is_some() {
        Some("readback_failed")
    } else if present {
        Some("application_still_present")
    } else {
        None
    };
    let verification = serde_json::json!({
        "method": "window-inventory-by-pid",
        "reason": reason,
        "polls": polls,
        "elapsed_ms": started.elapsed().as_millis(),
        "menu_path": pressed_path,
    });
    receipts.complete(
        &ticket,
        "app-quit",
        window,
        verified,
        serde_json::json!({
            "performed": mechanism_error.is_none(),
            "after": { "present": present },
            "verification": verification,
            "error": mechanism_error.as_ref().or(readback_error.as_ref()).map(error_payload),
        }),
    )?;
    if let Some(error) = mechanism_error.or(readback_error) {
        return Err(error);
    }
    Ok(serde_json::json!({
        "addressing": "window-handle",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "target": identity,
        "action": "quit",
        "postcondition": "gone",
        "performed": true,
        "verified": verified,
        "verification": verification,
        "snapshot": snapshot_json,
    }))
}

pub(super) fn close_payload(
    window: isize,
    pid: Option<u32>,
    title: Option<&str>,
    snapshot: bool,
    expect: Option<&str>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    destructive_gate(window, snapshot, expect)?;
    let not_performed = |error: CuError| {
        let mut detail = error.detail.clone().unwrap_or(serde_json::json!({}));
        detail["effect"] = serde_json::json!("not_performed");
        error.with_detail(detail)
    };
    let windows = mechanism::window_enumerate::enumerate_top_level()
        .map_err(map_mechanism_err)
        .map_err(not_performed)?;
    let Some(row) = windows.iter().find(|item| item.handle == window) else {
        return Err(not_performed(CuError::new(
            "window_not_found",
            format!("no top-level window with handle {window}"),
        )));
    };
    if let Some(pid) = pid
        && row.process_id != pid
    {
        return Err(not_performed(
            CuError::new(
                "window_identity_mismatch",
                format!(
                    "window {window} belongs to pid {} not {pid}; refusing to close another process's window",
                    row.process_id
                ),
            )
            .with_detail(serde_json::json!({ "expected": { "pid": pid }, "observed": window_identity_json(row) })),
        ));
    }
    if let Some(title) = title
        && row.title != title
    {
        return Err(not_performed(
            CuError::new(
                "window_identity_mismatch",
                format!(
                    "window {window} is titled {:?} not {title:?}; refusing to close it",
                    row.title
                ),
            )
            .with_detail(serde_json::json!({ "expected": { "title": title }, "observed": window_identity_json(row) })),
        ));
    }
    let identity = window_identity_json(row);
    let tree = mechanism::tree_for_window_bounded(
        Some(window),
        mechanism::TreeBudget {
            max_depth: Some(CLOSE_SNAPSHOT_DEPTH),
            max_nodes: Some(CLOSE_SNAPSHOT_NODES),
        },
    )
    .map_err(map_mechanism_err)
    .map_err(not_performed)?;
    let snapshot_json = serde_json::json!({
        "backend": tree.backend,
        "budget": { "depth": CLOSE_SNAPSHOT_DEPTH, "max_nodes": CLOSE_SNAPSHOT_NODES },
        "visited": tree.visited,
        "returned": tree.returned,
        "truncated": tree.truncated,
        "nodes": tree.nodes.iter().map(snapshot_node_json).collect::<Vec<_>>(),
    });
    let ticket = receipts.reserve(
        "close",
        window,
        serde_json::json!({
            "action": "close",
            "window_identity": identity,
            "postcondition": "gone",
            "before": { "present": true, "nodes": tree.returned },
            "snapshot": snapshot_json,
        }),
    )?;
    let started = Instant::now();
    let mechanism_error = mechanism::window_op::close(window)
        .err()
        .map(map_mechanism_err);
    // Postcondition: the handle (bound to its pid) leaves the inventory.
    let mut polls = 0usize;
    let mut present = true;
    let mut readback_error = None;
    while started.elapsed() < CLOSE_READBACK {
        polls += 1;
        match mechanism::window_enumerate::enumerate_top_level() {
            Ok(now) => {
                present = now
                    .iter()
                    .any(|item| item.handle == window && item.process_id == row.process_id);
            }
            Err(error) => {
                readback_error = Some(map_mechanism_err(error));
                break;
            }
        }
        if !present || mechanism_error.is_some() {
            break;
        }
        thread::sleep(CLOSE_READBACK_POLL);
    }
    let verified = !present && mechanism_error.is_none() && readback_error.is_none();
    let reason = if mechanism_error.is_some() {
        Some("mechanism_failed")
    } else if readback_error.is_some() {
        Some("readback_failed")
    } else if present {
        Some("window_still_present")
    } else {
        None
    };
    let verification = serde_json::json!({
        "method": "window-inventory",
        "reason": reason,
        "polls": polls,
        "elapsed_ms": started.elapsed().as_millis(),
    });
    let after = serde_json::json!({ "present": present });
    receipts.complete(
        &ticket,
        "close",
        window,
        verified,
        serde_json::json!({
            "performed": mechanism_error.is_none(),
            "after": after,
            "verification": verification,
            "error": mechanism_error.as_ref().or(readback_error.as_ref()).map(error_payload),
        }),
    )?;
    let payload = serde_json::json!({
        "addressing": "window-handle",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "target": identity,
        "action": "close",
        "postcondition": "gone",
        "performed": mechanism_error.is_none(),
        "verified": verified,
        "verification": verification,
        "before": { "present": true, "nodes": tree.returned },
        "after": after,
        "snapshot": {
            "visited": tree.visited,
            "returned": tree.returned,
            "truncated": tree.truncated,
            "in_receipt": true,
        },
        "receipt": ticket.json(),
    });
    if let Some(error) = mechanism_error.or(readback_error) {
        return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
    }
    if present {
        return Err(CuError::new(
            "unverified",
            format!(
                "close was delivered to window {window} but it is still in the inventory after {} polls",
                polls
            ),
        )
        .with_detail(serde_json::json!({ "reason": "window_still_present", "receipt": payload })));
    }
    Ok(payload)
}
