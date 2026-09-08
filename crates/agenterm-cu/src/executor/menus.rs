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

const LINUX_MENU_UNAVAILABLE_ALTERNATIVES: &[&str] = &[
    "launch the target with accessibility enabled (QT_ACCESSIBILITY=1; Chrome via scripts/box-chrome-a11y.sh --force-renderer-accessibility)",
    "menu-inspect --window HANDLE on a GTK application that publishes an AT-SPI menu bar before menu-invoke",
    "invoke --name on in-window controls when the application does not publish a menu bar",
];

fn linux_menu_unavailable_detail() -> serde_json::Value {
    serde_json::json!({
        "os": "linux",
        "mechanism": "at-spi2-menu-bar",
        "alternatives": LINUX_MENU_UNAVAILABLE_ALTERNATIVES,
    })
}

fn menu_leaf_is_unsafe(title: &str) -> bool {
    let normalized = title.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "quit"
            | "exit"
            | "close"
            | "close window"
            | "close tab"
            | "kill"
            | "delete"
            | "remove"
            | "log out"
            | "logout"
            | "sign out"
            | "signout"
            | "shut down"
            | "shutdown"
            | "restart"
            | "power off"
            | "discard"
            | "empty trash"
    )
}

fn enabled_leaf_paths(window: isize) -> Vec<String> {
    let budget = mechanism::TreeBudget {
        max_depth: Some(observe::menu_node_depth(observe::DEFAULT_MENU_DEPTH)),
        max_nodes: Some(observe::DEFAULT_MENU_NODE_BUDGET),
    };
    let tree = mechanism::menu_tree_for_window_bounded(Some(window), budget);
    let Ok(tree) = tree else {
        return Vec::new();
    };
    observe::menu_items(&tree)
        .into_iter()
        .filter(|item| item.enabled && !item.has_submenu && !menu_leaf_is_unsafe(&item.title))
        .map(|item| item.path.join("/"))
        .collect()
}

fn enrich_linux_menu_err(error: CuError, window: Option<isize>) -> CuError {
    if crate::mcu_surface::host_os() != "linux" {
        return error;
    }
    match error.code.as_str() {
        "a11y_menu_unavailable" => error.with_detail(linux_menu_unavailable_detail()),
        "a11y_menu_item_not_found" | "a11y_menu_item_ambiguous" => {
            let mut detail = error.detail.unwrap_or_else(|| serde_json::json!({}));
            if let Some(object) = detail.as_object_mut() {
                object.insert("os".into(), serde_json::json!("linux"));
                if let Some(window) = window.filter(|handle| *handle != 0) {
                    let alternatives = enabled_leaf_paths(window);
                    if !alternatives.is_empty() {
                        object.insert("alternatives".into(), serde_json::json!(alternatives));
                        object.insert(
                            "hint".into(),
                            serde_json::json!(
                                "no exact enabled leaf matched; choose one of the published paths in alternatives"
                            ),
                        );
                    }
                }
            }
            CuError::new(error.code, error.message).with_detail(detail)
        }
        _ => error,
    }
}

fn menu_invoke_unsupported(window: isize, path: &[String]) -> CuError {
    let leaf = path.last().map(String::as_str).unwrap_or_default();
    let alternatives = enabled_leaf_paths(window);
    CuError::new(
        "menu_invoke_unsupported",
        format!(
            "menu invoke refuses destructive or session-ending menu path {leaf:?}; choose a safe enabled leaf instead"
        ),
    )
    .with_detail(serde_json::json!({
        "os": "linux",
        "effect": "not_performed",
        "path": path,
        "unsafe_segment": leaf,
        "alternatives": alternatives,
        "hint": "menu invoke only presses non-destructive enabled leaves; run menu-inspect --window HANDLE to inventory safe paths",
    }))
}

#[cfg(any(target_os = "macos", test))]
fn app_menu_effect_verification(
    target_disappeared: bool,
    foreground_unchanged: bool,
    window_set_unchanged: bool,
    mark_changed: bool,
    tree_changed: bool,
    source_addressable: bool,
) -> (bool, &'static str) {
    if target_disappeared {
        (true, "application-disappeared-after-press")
    } else if foreground_unchanged && window_set_unchanged && mark_changed {
        (true, "mark-readback")
    } else if foreground_unchanged && window_set_unchanged && tree_changed {
        (true, "tree-diff")
    } else if !source_addressable {
        (false, "source-no-longer-addressable")
    } else {
        (false, "no-observable-change")
    }
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
    let tree = mechanism::menu_tree_for_window_bounded(Some(window), budget)
        .map_err(map_mechanism_err)
        .map_err(|error| enrich_linux_menu_err(error, Some(window)))?;
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

/// Press one exact path in a uniquely resolved macOS application menu.
/// Delivery and business-effect verification are deliberately separate: once
/// AXPress succeeds, a menu action may rebuild or remove the source window, so
/// a missing post-action tree is evidence availability, not retroactive proof
/// that the action failed.
pub(super) fn app_menu_invoke_payload(
    app: &str,
    path: &[String],
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let app = app.trim();
    if app.is_empty() {
        return Err(invalid_input(
            "app-menu-invoke --app must not be empty".into(),
        ));
    }
    if path.len() < 2 || path.iter().any(String::is_empty) {
        return Err(invalid_input(
            "app-menu-invoke needs --path with a menu title and at least one non-empty item title"
                .into(),
        ));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = receipts;
        return Err(CuError::new(
            "app_menu_platform_unsupported",
            "application-global menu invocation is a macOS capability; use menu-invoke --window on this host",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let mut inventory_before =
            mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
        let stacking_before = mechanism::window_enumerate::stacking().unwrap_or_default();
        let focus_before =
            super::windows::resolve_inventory_focus(&mut inventory_before, &stacking_before);
        let mut matching = inventory_before
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
        let windows_before = matching
            .iter()
            .map(|window| (window.handle, window.process_id, window.app_name.clone()))
            .collect::<Vec<_>>();
        let window = matching[0].handle;
        let tree_before = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
        let ticket = receipts.reserve(
            "app-menu-invoke",
            window,
            serde_json::json!({
                "addressing": "application-menu",
                "app": app,
                "app_pid": pid,
                "app_start_identity": start_identity,
                "windows_before": windows_before,
                "foreground_before": focus_before.json(),
                "path": path,
                "action": "press",
                "before": { "nodes": tree_before.returned },
            }),
        )?;
        let native = match mechanism::invoke_menu_path(Some(window), path) {
            Ok(receipt) => receipt,
            Err(error) => {
                let error = map_mechanism_err(error);
                receipts.complete(
                    &ticket,
                    "app-menu-invoke",
                    window,
                    false,
                    serde_json::json!({
                        "performed": false,
                        "delivery_verified": false,
                        "effect_verified": false,
                        "error": error_payload(&error),
                    }),
                )?;
                return Err(error.with_detail(serde_json::json!({ "receipt": ticket.json() })));
            }
        };

        // Everything below is post-observation. None of it may turn the
        // already accepted AXPress into an error response.
        let tree_after = mechanism::tree_for_window(Some(window)).ok();
        let tree_changed = tree_after
            .as_ref()
            .is_some_and(|after| observe::tree_changed(&tree_before, after));
        let mark_changed = native.mark_before != native.mark_after;
        let process_state = match agenterm_platform::process::observe(pid) {
            agenterm_platform::process::ProcessObservation::Live {
                start_identity: Some(after),
            } if after == start_identity => "same-identity",
            agenterm_platform::process::ProcessObservation::Dead { .. } => "exited",
            _ => "changed-or-unavailable",
        };
        let mut windows_after = None;
        let mut focus_after_json = serde_json::Value::Null;
        let mut foreground_unchanged = false;
        if let Ok(mut inventory_after) = mechanism::window_enumerate::enumerate_top_level() {
            let stacking_after = mechanism::window_enumerate::stacking().unwrap_or_default();
            let focus_after =
                super::windows::resolve_inventory_focus(&mut inventory_after, &stacking_after);
            foreground_unchanged = focus_before.app.as_ref().map(|value| value.pid)
                == focus_after.app.as_ref().map(|value| value.pid);
            focus_after_json = focus_after.json();
            let mut identities = inventory_after
                .iter()
                .filter(|entry| entry.app_name.eq_ignore_ascii_case(app))
                .map(|entry| (entry.handle, entry.process_id, entry.app_name.clone()))
                .collect::<Vec<_>>();
            identities.sort();
            windows_after = Some(identities);
        }
        let target_disappeared = process_state == "exited"
            && windows_after
                .as_ref()
                .is_some_and(|identities| identities.is_empty());
        let window_set_unchanged = windows_after
            .as_ref()
            .is_some_and(|identities| identities == &windows_before);
        let (effect_verified, verification_method) = app_menu_effect_verification(
            target_disappeared,
            foreground_unchanged,
            window_set_unchanged,
            mark_changed,
            tree_changed,
            tree_after.is_some(),
        );
        let close_body = serde_json::json!({
            "performed": true,
            "delivery_verified": true,
            "effect_verified": effect_verified,
            "verification": { "method": verification_method },
            "mark_before": native.mark_before,
            "mark_after": native.mark_after,
            "tree_changed": tree_changed,
            "nodes_after": tree_after.as_ref().map(|tree| tree.returned),
            "process_after": process_state,
            "windows_after": windows_after,
            "window_set_unchanged": window_set_unchanged,
            "target_disappeared": target_disappeared,
            "foreground_after": focus_after_json,
            "foreground_unchanged": foreground_unchanged,
        });
        if effect_verified {
            receipts.complete(&ticket, "app-menu-invoke", window, true, close_body.clone())?;
        } else {
            receipts.delivered(&ticket, "app-menu-invoke", window, close_body.clone())?;
        }
        Ok(serde_json::json!({
            "addressing": "application-menu",
            "mechanism": "libagenterm",
            "backend": tree_before.backend,
            "app": app,
            "pid": pid,
            "start_identity": start_identity,
            "window": window,
            "window_count_before": windows_before.len(),
            "path": path,
            "action": "press",
            "performed": true,
            "delivery_verified": true,
            "verified": effect_verified,
            "effect_verified": effect_verified,
            "verification": { "method": verification_method },
            "mark_before": native.mark_before,
            "mark_after": native.mark_after,
            "tree_changed": tree_changed,
            "process_after": process_state,
            "window_set_unchanged": window_set_unchanged,
            "target_disappeared": target_disappeared,
            "foreground_unchanged": foreground_unchanged,
            "post_observation_complete": tree_after.is_some() && windows_after.is_some(),
            "receipt": ticket.json(),
        }))
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
    if crate::mcu_surface::host_os() == "linux" {
        let leaf = path.last().map(String::as_str).unwrap_or_default();
        if menu_leaf_is_unsafe(leaf) {
            return Err(menu_invoke_unsupported(window, path));
        }
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
            let error = enrich_linux_menu_err(map_mechanism_err(error), Some(window));
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

#[cfg(test)]
mod tests {
    use super::{
        app_menu_effect_verification, enrich_linux_menu_err, menu_invoke_unsupported,
        menu_leaf_is_unsafe,
    };
    use crate::reply::CuError;

    #[test]
    fn app_menu_effect_never_confuses_delivery_with_readback() {
        assert_eq!(
            app_menu_effect_verification(true, false, false, false, false, false),
            (true, "application-disappeared-after-press")
        );
        assert_eq!(
            app_menu_effect_verification(false, true, true, true, false, true),
            (true, "mark-readback")
        );
        assert_eq!(
            app_menu_effect_verification(false, true, true, false, true, true),
            (true, "tree-diff")
        );
        assert_eq!(
            app_menu_effect_verification(false, true, true, false, false, false),
            (false, "source-no-longer-addressable")
        );
        assert_eq!(
            app_menu_effect_verification(false, false, true, true, true, true),
            (false, "no-observable-change")
        );
    }

    #[test]
    fn destructive_menu_leaf_is_refused_on_linux() {
        assert!(menu_leaf_is_unsafe("Quit"));
        assert!(menu_leaf_is_unsafe("Close Window"));
        assert!(!menu_leaf_is_unsafe("Do Thing"));
        assert!(!menu_leaf_is_unsafe("Minimize"));
    }

    #[test]
    fn linux_menu_unavailable_includes_os_and_alternatives() {
        let error = enrich_linux_menu_err(
            CuError::new("a11y_menu_unavailable", "no menu bar"),
            Some(7),
        );
        assert_eq!(error.code, "a11y_menu_unavailable");
        if crate::mcu_surface::host_os() == "linux" {
            let detail = error.detail.expect("detail");
            assert_eq!(detail["os"], "linux");
            assert_eq!(detail["mechanism"], "at-spi2-menu-bar");
            assert!(detail["alternatives"].as_array().is_some_and(|items| !items.is_empty()));
        }
    }

    #[test]
    fn linux_menu_invoke_unsupported_names_alternatives() {
        let error = menu_invoke_unsupported(7, &["File".into(), "Quit".into()]);
        assert_eq!(error.code, "menu_invoke_unsupported");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["os"], "linux");
        assert_eq!(detail["effect"], "not_performed");
        assert_eq!(detail["unsafe_segment"], "Quit");
        assert!(detail["alternatives"].is_array());
        assert!(detail["hint"].as_str().unwrap().contains("menu-inspect"));
    }
}
