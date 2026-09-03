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
