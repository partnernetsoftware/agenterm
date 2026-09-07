//! Background menus: `menu inspect` and `menu invoke`.

use super::*;

// ---------------------------------------------------------------------------
// Background menus, the App-local focused control, and the observation
// stream (slice 3 of plan/design-mcu-absorption.md).
// ---------------------------------------------------------------------------

pub(super) fn menu_budget(
    depth: Option<u32>,
    max_nodes: Option<usize>,
) -> Result<mechanism::TreeBudget, CuError> {
    observe::validate_menu_budget(depth, max_nodes).map_err(invalid_input)?;
    Ok(mechanism::TreeBudget {
        max_depth: Some(observe::menu_node_depth(
            depth.unwrap_or(observe::DEFAULT_MENU_DEPTH),
        )),
        max_nodes: Some(max_nodes.unwrap_or(observe::DEFAULT_MENU_NODE_BUDGET)),
    })
}

/// Background menu inventory: the application's menu bar walked under a
/// menu-level / node budget, flattened to items with exact title paths.
pub(super) fn menu_inspect_payload(
    window: isize,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    filter: observe::MenuFilter,
    offset: Option<usize>,
    max: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "menu inspect requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    let budget = menu_budget(depth, max_nodes)?;
    let page = observe::Page::new(offset, max).map_err(invalid_input)?;
    let tree =
        mechanism::menu_tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let items = observe::menu_items(&tree);
    let (hits, counts) = observe::menu_query(&items, &filter, page, tree.truncated);
    let rows = serde_json::to_value(&hits)
        .map_err(|error| CuError::new("serialize", error.to_string()))?;
    Ok(serde_json::json!({
        "addressing": "menu-path",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "budget": {
            "depth": depth.unwrap_or(observe::DEFAULT_MENU_DEPTH),
            "max_nodes": max_nodes.unwrap_or(observe::DEFAULT_MENU_NODE_BUDGET),
        },
        "filter": {
            "title": filter.title,
            "exact": filter.exact,
            "enabled": filter.enabled,
        },
        "nodes_visited": tree.visited,
        "visited": counts.visited,
        "matched": counts.matched,
        "returned": counts.returned,
        "offset": counts.offset,
        "truncated": counts.truncated,
        "scan_truncated": counts.scan_truncated,
        "page_truncated": counts.page_truncated,
        "items": rows,
    }))
}

/// Resolve the legacy macOS `menu inspect APP` shape without guessing one
/// window across applications or processes. macOS exposes one application
/// menu bar through any of that process's AX windows; other hosts never had
/// this legacy app-global contract and fail typed instead of choosing a window.
pub(super) fn app_menu_inspect_payload(
    app: &str,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    filter: observe::MenuFilter,
    offset: Option<usize>,
    max: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, depth, max_nodes, filter, offset, max);
        return Err(CuError::new(
            "app_menu_platform_unsupported",
            "application-global menu inspection is a macOS capability; use menu-inspect --window on this host",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let app = app.trim();
        if app.is_empty() {
            return Err(invalid_input(
                "app-menu-inspect --app must not be empty".into(),
            ));
        }
        let mut before =
            mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
        let stacking = mechanism::window_enumerate::stacking().unwrap_or_default();
        let focus_before = super::windows::resolve_inventory_focus(&mut before, &stacking);
        let mut matching = before
            .iter()
            .filter(|window| window.app_name.eq_ignore_ascii_case(app))
            .cloned()
            .collect::<Vec<_>>();
        matching.sort_by_key(|window| (window.process_id, window.handle));
        if matching.is_empty() {
            return Err(CuError::new(
                "a11y_app_not_found",
                "no top-level window belongs to the exact requested application",
            )
            .with_detail(serde_json::json!({ "app": app })));
        }
        let pids = matching
            .iter()
            .map(|window| window.process_id)
            .collect::<std::collections::BTreeSet<_>>();
        if pids.len() != 1 {
            return Err(CuError::new(
                "a11y_app_ambiguous",
                "the exact application name belongs to more than one live process",
            )
            .with_detail(serde_json::json!({ "app": app, "processes": pids.len() })));
        }
        let pid = matching[0].process_id;
        let start_identity = match agenterm_platform::process::observe(pid) {
            agenterm_platform::process::ProcessObservation::Live {
                start_identity: Some(identity),
            } => identity,
            _ => {
                return Err(CuError::new(
                    "a11y_app_identity_unavailable",
                    "the application process has no stable live start identity",
                ));
            }
        };
        let identities_before = matching
            .iter()
            .map(|window| (window.handle, window.process_id, window.app_name.clone()))
            .collect::<Vec<_>>();
        let selected = matching[0].handle;
        let mut payload = menu_inspect_payload(selected, depth, max_nodes, filter, offset, max)?;

        let process_stable = matches!(
            agenterm_platform::process::observe(pid),
            agenterm_platform::process::ProcessObservation::Live {
                start_identity: Some(after),
            } if after == start_identity
        );
        let mut after =
            mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
        let stacking_after = mechanism::window_enumerate::stacking().unwrap_or_default();
        let focus_after = super::windows::resolve_inventory_focus(&mut after, &stacking_after);
        let mut identities_after = after
            .iter()
            .filter(|window| window.app_name.eq_ignore_ascii_case(app))
            .map(|window| (window.handle, window.process_id, window.app_name.clone()))
            .collect::<Vec<_>>();
        identities_after.sort();
        if !process_stable
            || identities_before != identities_after
            || super::windows::focus_identity(&focus_before)
                != super::windows::focus_identity(&focus_after)
        {
            return Err(CuError::new(
                "app_menu_inspection_changed",
                "application identity, windows or foreground changed during menu inspection",
            )
            .with_detail(serde_json::json!({
                "app": app,
                "process_stable": process_stable,
                "windows_before": identities_before.len(),
                "windows_after": identities_after.len(),
                "focus_before": focus_before.json(),
                "focus_after": focus_after.json(),
            })));
        }
        if let Some(object) = payload.as_object_mut() {
            object.insert("addressing".into(), serde_json::json!("application-menu"));
            object.insert("app".into(), serde_json::json!(app));
            object.insert("pid".into(), serde_json::json!(pid));
            object.insert("start_identity".into(), serde_json::json!(start_identity));
            object.insert(
                "window_count".into(),
                serde_json::json!(identities_before.len()),
            );
            object.insert("focus_unchanged".into(), serde_json::json!(true));
        }
        Ok(payload)
    }
}

/// Press one menu item by exact title path in the background, verified by
/// the item's mark read-back and a whole-window tree diff.
pub(super) fn menu_invoke_payload(
    window: isize,
    path: &[String],
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "menu invoke requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if path.len() < 2 || path.iter().any(String::is_empty) {
        return Err(invalid_input(
            "menu invoke needs --path with a menu title and at least one non-empty item title"
                .into(),
        ));
    }
    let before = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    // The platform resolves the whole path (and refuses) before pressing,
    // so a refusal there leaves a `failed` receipt with nothing performed.
    let ticket = receipts.reserve(
        "menu-invoke",
        window,
        serde_json::json!({
            "path": path,
            "action": "press",
            "before": { "nodes": before.returned },
        }),
    )?;
    let receipt = match mechanism::invoke_menu_path(Some(window), path) {
        Ok(receipt) => receipt,
        Err(error) => {
            let error = map_mechanism_err(error);
            receipts.complete(
                &ticket,
                "menu-invoke",
                window,
                false,
                serde_json::json!({
                    "performed": false,
                    "verification": { "method": "none", "reason": "mechanism_failed" },
                    "error": error_payload(&error),
                }),
            )?;
            return Err(error.with_detail(serde_json::json!({ "receipt": ticket.json() })));
        }
    };
    let after = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let tree_changed = observe::tree_changed(&before, &after);
    let mark_changed = receipt.mark_before != receipt.mark_after;
    let (method, reason) = if mark_changed {
        ("mark-readback", None)
    } else if tree_changed {
        ("tree-diff", None)
    } else {
        ("tree-diff", Some("no_observable_change"))
    };
    let verification = serde_json::json!({ "method": method, "reason": reason });
    receipts.complete(
        &ticket,
        "menu-invoke",
        window,
        reason.is_none(),
        serde_json::json!({
            "performed": true,
            "after": { "nodes": after.returned },
            "verification": verification,
            "mark_before": receipt.mark_before,
            "mark_after": receipt.mark_after,
            "tree_changed": tree_changed,
        }),
    )?;
    Ok(serde_json::json!({
        "addressing": "menu-path",
        "mechanism": "libagenterm",
        "backend": before.backend,
        "window": window,
        "path": path,
        "action": "press",
        "performed": true,
        "verified": reason.is_none(),
        "verification": verification,
        "mark_before": receipt.mark_before,
        "mark_after": receipt.mark_after,
        "tree_changed": tree_changed,
        "nodes_before": before.returned,
        "nodes_after": after.returned,
        "receipt": ticket.json(),
    }))
}
