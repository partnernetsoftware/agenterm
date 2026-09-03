//! Node-addressed actuation and readback: `click`, `focus`, `scroll`,
//! `get-extents`, `select` / `get-selection`, `set-caret` / `get-caret`,
//! `get-text`, and the shared `--node` / `--name` / focused-node resolver.

use super::*;

pub(super) fn click_command(
    command: &Command,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let Command::Click {
        window,
        node,
        name,
        role,
        coords,
        degraded,
        clicks,
        button,
        ..
    } = command
    else {
        return Err(CuError::new(
            "invalid_input",
            "internal: expected click command",
        ));
    };
    let window = *window;
    let node = node.as_deref();
    let name = name.as_deref();
    let role = role.as_deref();
    let coords = *coords;
    let degraded = *degraded;
    let clicks = *clicks;
    let button = *button;
    if name.filter(|value| !value.is_empty()).is_some() && coords.is_some() {
        return Err(CuError::new(
            "invalid_input",
            "click --name is accessibility-tree addressing; do not pass --coords",
        ));
    }
    if let Some(resolved) = resolve_actuation_node(window, node, name, role, "click")? {
        // Receipt (reserved before the press) and read-back: the window
        // tree before and after, the same `tree-diff` proof `invoke press`
        // uses. Without a window scope there is nothing to diff, which the
        // reply says instead of claiming a verified click.
        let before = window
            .map(|handle| mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err))
            .transpose()?;
        let before_node = before
            .as_ref()
            .and_then(|tree| observe::node_by_id(tree, &resolved.node_id))
            .map(observe::node_state_json);
        let mut payload = click_tree_payload(&resolved, window, clicks, button);
        let ticket = receipts.reserve(
            "click",
            window.unwrap_or(0),
            serde_json::json!({
                "action": "click",
                "node": { "id": resolved.node_id, "name": resolved.matched.as_ref().map(|node| node.name.clone()), "role": resolved.matched.as_ref().map(|node| node.role.clone()) },
                "clicks": clicks.max(1),
                "before": before_node,
            }),
        )?;
        let mut mechanism_error = None;
        for _ in 0..clicks.max(1) {
            if let Err(error) = mechanism::perform_node_action(
                window,
                &resolved.node_id,
                mechanism::NodeAction::Click,
            ) {
                mechanism_error = Some(map_mechanism_err(error));
                break;
            }
        }
        let after = window
            .map(|handle| mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err))
            .transpose()?;
        let (verified, method, reason) = match (&before, &after, &resolved.matched) {
            (Some(was), Some(is), Some(hit)) => {
                let now = observe::node_by_id(is, &resolved.node_id);
                let proof = observe::verify_press(hit, now, was, is);
                (proof.verified, proof.method, proof.reason)
            }
            (Some(was), Some(is), None) => {
                if observe::tree_changed_semantically(was, is) {
                    (true, "tree-diff", None)
                } else {
                    (false, "tree-diff", Some("no_observable_change"))
                }
            }
            _ => (false, "none", Some("no_window_scope")),
        };
        let verified = verified && mechanism_error.is_none();
        let after_node = after
            .as_ref()
            .and_then(|tree| observe::node_by_id(tree, &resolved.node_id))
            .map(observe::node_state_json);
        payload["performed"] = serde_json::json!(true);
        payload["verified"] = serde_json::json!(verified);
        payload["verification"] = serde_json::json!({
            "method": method,
            "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { reason },
        });
        if reason == Some("checked_unchanged") {
            payload["next_actions"] = serde_json::json!([
                "AX did not flip checked; Chromium custom switch is not a native checkbox",
                "re-query then retry, or mcu browser/CDP click on the DOM control",
            ]);
        }
        payload["before"] = before_node.unwrap_or(serde_json::Value::Null);
        payload["after"] = after_node.clone().unwrap_or(serde_json::Value::Null);
        payload["receipt"] = ticket.json();
        receipts.complete(
            &ticket,
            "click",
            window.unwrap_or(0),
            verified,
            serde_json::json!({
                "after": after_node,
                "verification": payload["verification"].clone(),
                "error": mechanism_error.as_ref().map(error_payload),
            }),
        )?;
        if let Some(error) = mechanism_error {
            return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
        }
        return Ok(payload);
    }
    let Some([x, y]) = coords else {
        return Err(CuError::new(
            "invalid_input",
            "click requires --window + --node, --window + --name, or --coords with --degraded",
        ));
    };
    if !degraded {
        return Err(CuError::new(
            "invalid_input",
            "coordinate click requires --degraded so callers can see pixel addressing explicitly",
        ));
    }
    let inject_button = match button {
        PointerButton::Left => mechanism::input_inject::PointerButton::Left,
        PointerButton::Right => mechanism::input_inject::PointerButton::Right,
        PointerButton::Middle => mechanism::input_inject::PointerButton::Middle,
    };
    mechanism::input_inject::pointer_click(x, y, inject_button, clicks)
        .map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "addressing": "degraded-coordinates",
        "coords": [x, y],
        "window": window,
        "button": button,
        "clicks": clicks,
    }))
}

pub(super) fn focus(
    window: Option<isize>,
    node: Option<&str>,
    name: Option<&str>,
    role: Option<&str>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let resolved = resolve_actuation_node(window, node, name, role, "focus")?.ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "focus requires --node <path-id> or --window + --name",
        )
    })?;
    // Receipt reserved before the focus move; read back as the node's own
    // `focused` state in the window tree (no window scope: unverifiable).
    let before_node = window
        .map(|handle| mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err))
        .transpose()?
        .as_ref()
        .and_then(|tree| observe::node_by_id(tree, &resolved.node_id))
        .map(observe::node_state_json);
    let ticket = receipts.reserve(
        "focus",
        window.unwrap_or(0),
        serde_json::json!({
            "action": "focus",
            "node": { "id": resolved.node_id, "name": resolved.matched.as_ref().map(|node| node.name.clone()), "role": resolved.matched.as_ref().map(|node| node.role.clone()) },
            "before": before_node,
        }),
    )?;
    let mechanism_error =
        mechanism::perform_node_action(window, &resolved.node_id, mechanism::NodeAction::Focus)
            .err()
            .map(map_mechanism_err);
    let after_node = window
        .map(|handle| mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err))
        .transpose()?
        .as_ref()
        .and_then(|tree| observe::node_by_id(tree, &resolved.node_id))
        .cloned();
    let (verified, method, reason) = match &after_node {
        Some(node) => match observe::focused_state(node) {
            observe::Tri::True => (true, "focused-readback", None),
            observe::Tri::False | observe::Tri::Mixed => {
                (false, "focused-readback", Some("state_mismatch"))
            }
            observe::Tri::Unknown => (false, "focused-readback", Some("state_unobservable")),
        },
        None if window.is_some() => (false, "node-readback", Some("node_gone")),
        None => (false, "none", Some("no_window_scope")),
    };
    let verified = verified && mechanism_error.is_none();
    let after_state = after_node.as_ref().map(observe::node_state_json);
    let mut payload = focus_tree_payload(&resolved, window);
    payload["performed"] = serde_json::json!(true);
    payload["verified"] = serde_json::json!(verified);
    payload["verification"] = serde_json::json!({
        "method": method,
        "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { reason },
    });
    payload["before"] = before_node.unwrap_or(serde_json::Value::Null);
    payload["after"] = after_state.clone().unwrap_or(serde_json::Value::Null);
    payload["receipt"] = ticket.json();
    receipts.complete(
        &ticket,
        "focus",
        window.unwrap_or(0),
        verified,
        serde_json::json!({
            "after": after_state,
            "verification": payload["verification"].clone(),
            "error": mechanism_error.as_ref().map(error_payload),
        }),
    )?;
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
    }
    Ok(payload)
}

/// `scroll --name` is one-shot AT-SPI `Component.ScrollTo(TopEdge)`
/// (`agt_a11y_node_scroll`). Missing / false / `UnknownMethod` typed-fails
/// (`a11y_scroll_unavailable`). Never Action `scroll*`, XTest wheel,
/// `GenerateMouseEvent`, or `--coords`. `matched.extents` / snapshot
/// bounds do not count as proof.
pub(super) fn scroll(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "scroll requires --window <handle> --name <pattern>",
        )
    })?;
    let resolved =
        resolve_actuation_node(window, None, Some(name), role, "scroll")?.ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "scroll requires --window <handle> --name <pattern>",
            )
        })?;
    mechanism::scroll_node(window, &resolved.node_id).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "scroll",
        "via": "scroll-to",
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `get-extents --name` reads independent AT-SPI `Component.GetExtents(Screen)`
/// (`agt_a11y_node_get_extents`). Snapshot `node.bounds` do not count.
/// Empty extents typed-fail (`a11y_extents_unavailable`).
pub(super) fn get_extents(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "get-extents requires --window <handle> --name <pattern>",
        )
    })?;
    let resolved = resolve_actuation_node(window, None, Some(name), role, "get-extents")?
        .ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "get-extents requires --window <handle> --name <pattern>",
            )
        })?;
    let extents =
        mechanism::get_node_extents(window, &resolved.node_id).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "get-extents",
        "via": "get-extents",
        "extents": {
            "x": extents.x,
            "y": extents.y,
            "width": extents.width,
            "height": extents.height,
        },
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `select --name` is one-shot AT-SPI `Text.SetSelection`
/// (`agt_a11y_node_set_selection`). Missing Text / `UnknownMethod`
/// typed-fails (`a11y_selection_unavailable`). SetSelection false
/// typed-fails (`a11y_selection_no_effect`). Never XTest, mouse-drag,
/// `--coords`, or screenshot. The reply is not proof — `get-selection`
/// is the independent `GetNSelections` / `GetSelection` readback.
pub(super) fn select(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
    start: i32,
    end: i32,
) -> Result<serde_json::Value, CuError> {
    if start < 0 || end < start {
        return Err(CuError::new(
            "invalid_input",
            format!("select requires 0 <= --start <= --end; got {start}..{end}"),
        ));
    }
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "select requires --window <handle> --name <pattern> --start N --end M",
        )
    })?;
    let resolved =
        resolve_actuation_node(window, None, Some(name), role, "select")?.ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "select requires --window <handle> --name <pattern> --start N --end M",
            )
        })?;
    mechanism::set_node_selection(window, &resolved.node_id, start, end)
        .map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "select",
        "via": "set-selection",
        "start": start,
        "end": end,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `get-selection --name` reads independent AT-SPI `Text.GetNSelections`
/// + `GetSelection(0)` (`agt_a11y_node_get_selection`). The `select`
///
/// The reply payload does not count. Missing Text typed-fails
/// (`a11y_selection_unavailable`). `n == 0` is empty success.
pub(super) fn get_selection(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "get-selection requires --window <handle> --name <pattern>",
        )
    })?;
    let resolved = resolve_actuation_node(window, None, Some(name), role, "get-selection")?
        .ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "get-selection requires --window <handle> --name <pattern>",
            )
        })?;
    let selection =
        mechanism::get_node_selection(window, &resolved.node_id).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "get-selection",
        "via": "get-selection",
        "n": selection.n,
        "start": selection.start,
        "end": selection.end,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `set-caret --name` is one-shot AT-SPI `Text.SetCaretOffset`
/// (`agt_a11y_node_set_caret_offset`). Missing Text / `UnknownMethod`
/// typed-fails (`a11y_caret_unavailable`). SetCaretOffset false
/// typed-fails (`a11y_caret_no_effect`). Never XTest, `--coords`, or
/// screenshot. The reply is not proof — `get-caret` is the independent
/// `CaretOffset` readback.
pub(super) fn set_caret(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
    offset: i32,
) -> Result<serde_json::Value, CuError> {
    if offset < 0 {
        return Err(CuError::new(
            "invalid_input",
            format!("set-caret requires --offset >= 0; got {offset}"),
        ));
    }
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "set-caret requires --window <handle> --name <pattern> --offset N",
        )
    })?;
    let resolved = resolve_actuation_node(window, None, Some(name), role, "set-caret")?
        .ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "set-caret requires --window <handle> --name <pattern> --offset N",
            )
        })?;
    mechanism::set_node_caret_offset(window, &resolved.node_id, offset)
        .map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "set-caret",
        "via": "set-caret-offset",
        "offset": offset,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `get-caret --name` reads independent AT-SPI `Text.CaretOffset`
/// (`agt_a11y_node_get_caret_offset`). The `set-caret` reply payload
/// does not count. Missing Text typed-fails (`a11y_caret_unavailable`).
pub(super) fn get_caret(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty()).ok_or_else(|| {
        CuError::new(
            "invalid_input",
            "get-caret requires --window <handle> --name <pattern>",
        )
    })?;
    let resolved = resolve_actuation_node(window, None, Some(name), role, "get-caret")?
        .ok_or_else(|| {
            CuError::new(
                "invalid_input",
                "get-caret requires --window <handle> --name <pattern>",
            )
        })?;
    let offset =
        mechanism::get_node_caret_offset(window, &resolved.node_id).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "get-caret",
        "via": "get-caret-offset",
        "offset": offset,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `get-text --name` reads independent AT-SPI `Text.GetText`
/// (`agt_a11y_node_get_text`) once for the unique showing named node.
/// Without `--name` it reads the focused node instead: toolkits may mark
/// a whole ancestor chain `focused` (Reasonix marks a container that has
/// no Text interface), so the candidates are every showing node carrying
/// the AT-SPI `focused` state, probed innermost-first, and the winner is
/// the innermost one that actually exposes `Text.GetText`. So
/// `focus --name X` then `get-text --window H` closes the loop on
/// whatever holds focus. This is the same text authority
/// `wait --text-equals` polls, exposed as a first-class one-shot readback
/// so an independent observation does not need a wait timeout. Not
/// `send-text` / `paste` / `copy` `matched.text`, `last_text_write_via`,
/// the WebKit eval helper's queued-job `OK`, or a tree snapshot `text`.
/// No focused candidate with Text typed-fails (`a11y_text_unavailable`).
/// Never XTest / `--coords` / screenshot.
pub(super) fn get_text(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let name = name.filter(|value| !value.is_empty());
    let (resolved, text) = match name {
        Some(name) => {
            let resolved = resolve_actuation_node(window, None, Some(name), role, "get-text")?
                .ok_or_else(|| {
                    CuError::new(
                        "invalid_input",
                        "get-text requires --window <handle> [--name <pattern>]",
                    )
                })?;
            let text =
                mechanism::get_node_text(window, &resolved.node_id).map_err(map_mechanism_err)?;
            (resolved, text)
        }
        None => {
            if role.is_some() {
                return Err(CuError::new(
                    "invalid_input",
                    "get-text --role requires --name <pattern>",
                ));
            }
            get_text_focused(window)?
        }
    };
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "get-text",
        "via": "gettext",
        "text": text,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

pub(super) struct ResolvedNode {
    pub(super) node_id: String,
    pub(super) matched: Option<mechanism::A11yNode>,
    pub(super) backend: Option<String>,
}

/// Focused-node text readback: no name pattern, no coordinates — the
/// toolkit's own focus report picks the node. Probes every showing
/// `focused` node innermost-first with independent `Text.GetText` and
/// returns the first that exposes it. A `focused` ancestor without the
/// Text interface (`a11y_text_unavailable`) falls through to the next
/// candidate; any other mechanism failure aborts. All candidates missing
/// Text re-raises the innermost candidate's `a11y_text_unavailable`.
pub(super) fn get_text_focused(window: Option<isize>) -> Result<(ResolvedNode, String), CuError> {
    let (_tree, resolved, text) = get_text_focused_in_tree(window)?;
    Ok((resolved, text))
}

/// `get_text_focused` plus the tree snapshot the node was resolved in, so
/// a caller can judge the node's place in that same walk (the focused
/// text writers ask whether it sits inside a browser's web-area).
pub(super) fn get_text_focused_in_tree(
    window: Option<isize>,
) -> Result<(mechanism::A11yTree, ResolvedNode, String), CuError> {
    let Some(handle) = window else {
        return Err(CuError::new(
            "invalid_input",
            "get-text without --name requires --window <handle>",
        ));
    };
    let tree = mechanism::tree_for_window(Some(handle)).map_err(map_mechanism_err)?;
    let (node, text) = {
        let candidates = focused_candidates_innermost_first(&tree.nodes);
        if candidates.is_empty() {
            return Err(CuError::new(
                "a11y_node_not_found",
                "no showing focused accessibility node in window tree",
            ));
        }
        let mut text_unavailable: Option<CuError> = None;
        let mut hit = None;
        for node in candidates {
            match mechanism::get_node_text(window, &node.id) {
                Ok(text) => {
                    hit = Some((node.clone(), text));
                    break;
                }
                Err(mechanism::MechanismError::Failed { code, message })
                    if code == "a11y_text_unavailable" =>
                {
                    text_unavailable.get_or_insert(CuError::new(code, message));
                }
                Err(other) => return Err(map_mechanism_err(other)),
            }
        }
        match hit {
            Some(hit) => hit,
            None => {
                return Err(
                    text_unavailable.expect("non-empty candidates yield Ok or a stored error")
                );
            }
        }
    };
    let resolved = ResolvedNode {
        node_id: node.id.clone(),
        matched: Some(node),
        backend: Some(tree.backend.clone()),
    };
    Ok((tree, resolved, text))
}

/// Every showing node carrying the AT-SPI `focused` state, deepest child
/// path first, so an innermost real widget wins over a `focused` ancestor
/// container. Depth is the child-index path length; the stable sort keeps
/// snapshot pre-order between equal depths.
pub(super) fn focused_candidates_innermost_first(
    nodes: &[mechanism::A11yNode],
) -> Vec<&mechanism::A11yNode> {
    let mut candidates: Vec<&mechanism::A11yNode> = nodes
        .iter()
        .filter(|node| node_is_showing(node))
        .filter(|node| node.states.iter().any(|state| state == "focused"))
        .collect();
    candidates.sort_by_key(|node| std::cmp::Reverse(node.id.matches('/').count()));
    candidates
}

/// Shared addressing gate for structured click/focus: `--node` or `--name`,
/// never both, and `--name` never opens a coordinate/screenshot path.
/// `--name` requires exactly one showing/visible match.
pub(super) fn resolve_actuation_node(
    window: Option<isize>,
    node: Option<&str>,
    name: Option<&str>,
    role: Option<&str>,
    verb: &str,
) -> Result<Option<ResolvedNode>, CuError> {
    let node = node.filter(|value| !value.is_empty());
    let name = name.filter(|value| !value.is_empty());
    if node.is_some() && name.is_some() {
        return Err(CuError::new(
            "invalid_input",
            format!("{verb} accepts --node or --name, not both"),
        ));
    }
    if let Some(pattern) = name {
        let (tree, matched) = resolve_named_node(window, pattern, role)?;
        return Ok(Some(ResolvedNode {
            node_id: matched.id.clone(),
            matched: Some(matched),
            backend: Some(tree.backend),
        }));
    }
    let Some(node_id) = node else {
        return Ok(None);
    };
    let Some(window) = window else {
        return Err(CuError::new(
            "invalid_input",
            format!(
                "{verb} --node requires --window <handle> so the path is resolved against one tree snapshot"
            ),
        ));
    };
    let tree = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let Some(matched) = observe::node_by_id(&tree, node_id).cloned() else {
        return Err(CuError::new(
            "a11y_node_not_found",
            format!(
                "{verb} --node {node_id:?} is not in the current window tree; re-query and use the new path (Chromium AX ids are not stable)"
            ),
        ));
    };
    Ok(Some(ResolvedNode {
        node_id: matched.id.clone(),
        matched: Some(matched),
        backend: Some(tree.backend),
    }))
}

pub(super) fn resolve_named_node(
    window: Option<isize>,
    pattern: &str,
    role: Option<&str>,
) -> Result<(mechanism::A11yTree, mechanism::A11yNode), CuError> {
    let Some(window) = window else {
        return Err(CuError::new(
            "invalid_input",
            "name addressing requires --window <handle>",
        ));
    };
    let tree = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let node = require_unique_showing_node(&tree.nodes, pattern, role)?.clone();
    Ok((tree, node))
}

pub(super) fn click_tree_payload(
    resolved: &ResolvedNode,
    window: Option<isize>,
    clicks: u32,
    button: PointerButton,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "click",
        "clicks": clicks,
        "button": button,
    });
    attach_name_match(&mut payload, resolved);
    payload
}

pub(super) fn focus_tree_payload(
    resolved: &ResolvedNode,
    window: Option<isize>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "focus",
    });
    attach_name_match(&mut payload, resolved);
    payload
}

pub(super) fn attach_name_match(payload: &mut serde_json::Value, resolved: &ResolvedNode) {
    let Some(matched) = &resolved.matched else {
        return;
    };
    if let Some(backend) = &resolved.backend {
        payload["backend"] = serde_json::json!(backend);
    }
    payload["matched"] = serde_json::to_value(matched).unwrap_or(serde_json::Value::Null);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_click_requires_degraded_marker() {
        let auth = Authorization::new([Grant::Observe, Grant::Actuate].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Click {
            target: TargetRef::Current,
            window: None,
            node: None,
            name: None,
            role: None,
            coords: Some([1, 2]),
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn node_click_uses_accessibility_tree_when_node_is_set() {
        let auth = Authorization::new([Grant::Actuate].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Click {
            target: TargetRef::Current,
            window: None,
            node: Some("/0/999999".into()),
            name: None,
            role: None,
            coords: None,
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "invalid_input"
                    | "a11y_invalid_node_id"
                    | "a11y_node_not_found"
                    | "a11y_backend_failed"
                    | "dylib_load"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn focused_candidates_order_innermost_widget_before_focused_ancestor() {
        // Reasonix shape: a focused container without Text sits above the
        // focused composer textarea; the composer must be probed first.
        let panel = node_at("/0/0/0/0/0/0/0", "", "filler", &["showing", "focused"]);
        let composer = node_at(
            "/0/0/0/0/0/0/0/0/5/1/0",
            "Message Reasonix…",
            "text",
            &["showing", "editable", "focused"],
        );
        let hidden = node_at("/0/0/0/0/0/0/0/0/9", "", "text", &["focused"]);
        let unfocused = node_at("/0/1", "Send", "push button", &["showing"]);
        let nodes = vec![panel.clone(), composer.clone(), hidden, unfocused];
        let candidates = focused_candidates_innermost_first(&nodes);
        let ids: Vec<&str> = candidates.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(ids, vec![composer.id.as_str(), panel.id.as_str()]);
    }

    #[test]
    fn name_click_requires_window() {
        let command = Command::Click {
            target: TargetRef::Current,
            window: None,
            node: None,
            name: Some("Reload".into()),
            role: None,
            coords: None,
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_and_node_are_exclusive() {
        let command = Command::Click {
            target: TargetRef::Current,
            window: Some(1),
            node: Some("/0/1".into()),
            name: Some("Reload".into()),
            role: None,
            coords: None,
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_and_coords_are_exclusive() {
        let command = Command::Click {
            target: TargetRef::Current,
            window: Some(1),
            node: None,
            name: Some("Reload".into()),
            role: None,
            coords: Some([1, 2]),
            degraded: true,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_click_missing_node_is_typed() {
        let command = Command::Click {
            target: TargetRef::Current,
            window: Some(-1),
            node: None,
            name: Some("agenterm-no-such-node".into()),
            role: None,
            coords: None,
            degraded: false,
            clicks: 1,
            button: PointerButton::Left,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn name_focus_missing_node_is_typed() {
        let command = Command::Focus {
            target: TargetRef::Current,
            window: Some(-1),
            node: None,
            name: Some("agenterm-no-such-node".into()),
            role: Some("button".into()),
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn name_scroll_requires_name() {
        let command = Command::Scroll {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_scroll_requires_window() {
        let command = Command::Scroll {
            target: TargetRef::Current,
            window: None,
            name: Some("OffscreenField".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_scroll_missing_node_is_typed_and_scrolls_nothing() {
        let command = Command::Scroll {
            target: TargetRef::Current,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not scroll");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn scroll_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::Scroll {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("OffscreenField".into()),
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn name_get_extents_requires_name() {
        let command = Command::GetExtents {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_get_extents_requires_window() {
        let command = Command::GetExtents {
            target: TargetRef::Current,
            window: None,
            name: Some("OffscreenField".into()),
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_get_extents_missing_node_is_typed() {
        let command = Command::GetExtents {
            target: TargetRef::Current,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok, "missing name must not invent extents");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn scroll_and_get_extents_verbs_are_named() {
        let scroll = Command::Scroll {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("OffscreenField".into()),
            role: None,
        };
        let extents = Command::GetExtents {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("ScrollViewport".into()),
            role: None,
        };
        assert_eq!(scroll.verb(), "scroll");
        assert_eq!(extents.verb(), "get-extents");
        assert_eq!(scroll.required_grant(), Grant::Actuate);
        assert_eq!(extents.required_grant(), Grant::Observe);
    }

    #[test]
    fn name_select_requires_name() {
        let command = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_select_requires_window() {
        let command = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: None,
            name: Some("SelectField".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_select_rejects_inverted_range() {
        let command = Command::Select {
            target: TargetRef::Current,
            start: 4,
            end: 0,
            window: Some(1),
            name: Some("SelectField".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_select_missing_node_is_typed_and_selects_nothing() {
        let command = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not select");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn select_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: Some(1),
            name: Some("SelectField".into()),
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn name_get_selection_requires_name() {
        let command = Command::GetSelection {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_get_selection_requires_window() {
        let command = Command::GetSelection {
            target: TargetRef::Current,
            window: None,
            name: Some("SelectField".into()),
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_get_selection_missing_node_is_typed() {
        let command = Command::GetSelection {
            target: TargetRef::Current,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok, "missing name must not invent a selection");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn select_and_get_selection_verbs_are_named() {
        let select = Command::Select {
            target: TargetRef::Current,
            start: 0,
            end: 4,
            window: Some(1),
            name: Some("SelectField".into()),
            role: None,
        };
        let get_selection = Command::GetSelection {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("SelectField".into()),
            role: None,
        };
        assert_eq!(select.verb(), "select");
        assert_eq!(get_selection.verb(), "get-selection");
        assert_eq!(select.required_grant(), Grant::Actuate);
        assert_eq!(get_selection.required_grant(), Grant::Observe);
    }

    #[test]
    fn set_caret_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::SetCaret {
            target: TargetRef::Current,
            offset: 2,
            window: Some(1),
            name: Some("Command".into()),
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn name_get_caret_requires_name() {
        let command = Command::GetCaret {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn set_caret_and_get_caret_verbs_are_named() {
        let set_caret = Command::SetCaret {
            target: TargetRef::Current,
            offset: 2,
            window: Some(1),
            name: Some("Command".into()),
            role: None,
        };
        let get_caret = Command::GetCaret {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("Command".into()),
            role: None,
        };
        assert_eq!(set_caret.verb(), "set-caret");
        assert_eq!(get_caret.verb(), "get-caret");
        assert_eq!(set_caret.required_grant(), Grant::Actuate);
        assert_eq!(get_caret.required_grant(), Grant::Observe);
    }

    #[test]
    fn get_text_without_name_requires_window() {
        let command = Command::GetText {
            target: TargetRef::Current,
            window: None,
            name: None,
            role: None,
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("requires --window <handle>"),
            "missing-window message should name the addressing contract"
        );
    }

    #[test]
    fn get_text_role_without_name_is_typed() {
        let command = Command::GetText {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: Some("text".into()),
        };
        let reply = observe_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("--role requires --name"),
            "role-without-name message should name the addressing contract"
        );
    }

    #[test]
    fn get_text_verb_is_named_and_observe() {
        let get_text = Command::GetText {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("Command".into()),
            role: None,
        };
        assert_eq!(get_text.verb(), "get-text");
        assert_eq!(get_text.required_grant(), Grant::Observe);
    }
}
