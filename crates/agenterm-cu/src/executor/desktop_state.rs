//! Bounded desktop-state composition.  This does not add a platform API: it
//! joins the existing top-level window inventory, one bounded a11y tree, and
//! the read-only pointer observer into one fail-closed reply.

use super::*;

/// A desktop with more visible windows than this has no complete bounded
/// inventory reply.  Refuse rather than silently page a state snapshot.
const MAX_DESKTOP_WINDOWS: usize = 512;

pub(super) fn desktop_state_payload(
    requested_window: Option<isize>,
    depth: Option<u32>,
    max_nodes: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    tree_budget(depth, max_nodes)?;

    let (mut windows, focus) = inventory()?;
    let selected = select_window(&windows, requested_window, &focus)?.clone();
    let selection = if requested_window.is_some() {
        "explicit"
    } else {
        "focused"
    };
    let tree = tree_payload(Some(selected.handle), depth, max_nodes, true)?;
    let pointer = pointer_position()?;

    // A handle is only valid at an observation instant.  Re-read the exact
    // inventory after the tree, normalize focus the same way, and require the
    // complete selected row to agree.  There is no exposed generation token,
    // so this is intentionally stricter than a handle-only comparison.
    let (after, _) = inventory()?;
    let Some(rechecked) = after.iter().find(|row| row.handle == selected.handle) else {
        return Err(changed(
            &selected,
            "window disappeared while desktop-state was captured",
        ));
    };
    if rechecked != &selected {
        return Err(changed(
            &selected,
            "window changed while desktop-state was captured",
        ));
    }

    let windows_json: Vec<serde_json::Value> = windows
        .drain(..)
        .map(|row| observe::window_row_json(&row))
        .collect();
    Ok(serde_json::json!({
        "snapshot_version": 1,
        "mechanism": "libagenterm",
        "addressing": "window-inventory+accessibility-tree+absolute-screen-coordinates",
        "selection": selection,
        "window": observe::window_row_json(&selected),
        "active_window": focus.json(),
        "windows": windows_json,
        "tree": tree,
        "pointer": pointer,
        "complete": { "windows": true, "tree": !tree["truncated"].as_bool().unwrap_or(true) },
    }))
}

fn inventory() -> Result<
    (
        Vec<mechanism::window_enumerate::WindowInfo>,
        observe::FocusResolution,
    ),
    CuError,
> {
    let mut windows =
        mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    if windows.len() > MAX_DESKTOP_WINDOWS {
        return Err(CuError::new(
            "desktop_state_window_limit",
            format!(
                "desktop-state observed {} windows; limit is {MAX_DESKTOP_WINDOWS}",
                windows.len()
            ),
        ));
    }
    let stacking = mechanism::window_enumerate::stacking().unwrap_or_default();
    let focus = resolve_inventory_focus(&mut windows, &stacking);
    Ok((windows, focus))
}

fn select_window<'a>(
    windows: &'a [mechanism::window_enumerate::WindowInfo],
    requested: Option<isize>,
    focus: &observe::FocusResolution,
) -> Result<&'a mechanism::window_enumerate::WindowInfo, CuError> {
    if let Some(handle) = requested {
        return windows
            .iter()
            .find(|row| row.handle == handle)
            .ok_or_else(|| {
                CuError::new(
                    "desktop_state_window_not_found",
                    format!("no top-level window with handle {handle}"),
                )
            });
    }
    let Some(handle) = focus.handle else {
        return Err(CuError::new(
            "desktop_state_ambiguous",
            "desktop-state needs --window because no unique focused inventory window was resolved",
        ));
    };
    windows
        .iter()
        .find(|row| row.handle == handle)
        .ok_or_else(|| {
            CuError::new(
                "desktop_state_ambiguous",
                "focused window was not present in the same inventory",
            )
        })
}

fn changed(window: &mechanism::window_enumerate::WindowInfo, message: &str) -> CuError {
    CuError::new("desktop_state_changed", message).with_detail(serde_json::json!({
        "handle": window.handle,
        "ref": observe::window_stable_ref(window),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(handle: isize, focused: bool) -> mechanism::window_enumerate::WindowInfo {
        mechanism::window_enumerate::WindowInfo {
            handle,
            title: "fixture".into(),
            process_id: 7,
            app_name: "Fixture".into(),
            bounds: mechanism::window_enumerate::WindowBounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            focused,
            minimized: false,
        }
    }

    #[test]
    fn explicit_handle_must_be_in_the_same_inventory() {
        let rows = vec![row(7, true)];
        let focus = observe::FocusResolution {
            app: None,
            handle: Some(7),
            via: Some("inventory-mark"),
            reason: None,
        };
        assert_eq!(select_window(&rows, Some(7), &focus).unwrap().handle, 7);
        assert_eq!(
            select_window(&rows, Some(8), &focus).unwrap_err().code,
            "desktop_state_window_not_found"
        );
    }

    #[test]
    fn omitted_handle_requires_resolved_focus() {
        let rows = vec![row(7, false)];
        let focus = observe::FocusResolution {
            app: None,
            handle: None,
            via: None,
            reason: Some("no_frontmost_app"),
        };
        assert_eq!(
            select_window(&rows, None, &focus).unwrap_err().code,
            "desktop_state_ambiguous"
        );
    }

    #[test]
    fn invalid_tree_budget_refuses_before_any_desktop_read() {
        let error =
            desktop_state_payload(None, Some(observe::MAX_DEPTH_BUDGET + 1), None).unwrap_err();
        assert_eq!(error.code, "invalid_input");
    }
}
