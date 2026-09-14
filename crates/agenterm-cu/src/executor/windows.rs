//! Window inventory and geometry verbs: `windows`, `windows-watch`,
//! `apps`, `orderwin`, `displays`, `spaces`, and the window captures
//! `screenshot` / `zoom`.

use super::*;

use std::collections::{BTreeMap, BTreeSet};

use crate::command::WindowWatchEventKind;
use crate::observe::FrontmostApp;

const WINDOWS_WATCH_MAX_DURATION_MS: u64 = 300_000;
const WINDOWS_WATCH_MAX_EVENTS: usize = 10_000;
const WINDOWS_WATCH_MAX_WINDOWS: usize = 10_000;
const WINDOWS_WATCH_DEFAULT_MAX_WINDOWS: usize = 2_000;
const WINDOWS_INVENTORY_DEFAULT_MAX: usize = 1_000;
const WINDOWS_AX_SCAN_DEFAULT: usize = 200;
const WINDOWS_AX_SCAN_MAX: usize = 1_000;

/// Window inventory. The bare verb keeps its array reply; any filter or page
/// field switches to the inventory object with counts. `browser_profile`
/// is the one filter the plain `WindowFilter` cannot judge from the
/// inventory row alone (a Chromium window's profile sits in its AX root
/// name when the title does not carry it), so it is applied here after
/// the row filter and before paging.
// One argument per `windows` flag the dispatcher already destructured; a
// wrapper struct would only move the same fields one layer down.
#[allow(clippy::too_many_arguments)]
pub(super) fn windows_payload(
    filter: observe::WindowFilter,
    space: Option<u64>,
    onscreen: Option<bool>,
    occluded: Option<bool>,
    all: bool,
    meta: bool,
    browser_profile: Option<String>,
    ax_meta: bool,
    ax_role: Option<String>,
    ax_subrole: Option<String>,
    ax_identifier: Option<String>,
    ax_scan_max: Option<usize>,
    offset: Option<usize>,
    max: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    let page = observe::Page::new(offset, Some(max.unwrap_or(WINDOWS_INVENTORY_DEFAULT_MAX)))
        .map_err(invalid_input)?;
    let wants_ax = ax_meta || ax_role.is_some() || ax_subrole.is_some() || ax_identifier.is_some();
    let ax_scan_max = ax_scan_max.unwrap_or(WINDOWS_AX_SCAN_DEFAULT);
    if !(1..=WINDOWS_AX_SCAN_MAX).contains(&ax_scan_max) {
        return Err(invalid_input(format!(
            "windows --ax-scan-max must be in 1..={WINDOWS_AX_SCAN_MAX}"
        )));
    }
    let visible = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let visible_handles: BTreeSet<_> = visible.iter().map(|window| window.handle).collect();
    let mut windows = if all {
        mechanism::window_enumerate::enumerate_top_level_all().map_err(map_mechanism_err)?
    } else {
        visible
    };
    let visited = windows.len();
    // Stacking is an additional read, and a host without one is not an
    // error: the rows simply carry no z_index / occluded_percent, and the
    // envelope says why.
    let (stacking, stacking_reason) = match mechanism::window_enumerate::stacking() {
        Ok(rows) => (rows, None),
        Err(mechanism::MechanismError::Unsupported { reason }) => (Vec::new(), Some(reason)),
        // A reason a caller reads, not a Debug rendering of the enum.
        Err(mechanism::MechanismError::Failed { code, message }) => {
            (Vec::new(), Some(format!("{code}: {message}")))
        }
    };
    let focus = resolve_inventory_focus(&mut windows, &stacking);
    if filter.is_empty()
        && space.is_none()
        && onscreen.is_none()
        && occluded.is_none()
        && !all
        && !meta
        && browser_profile.is_none()
        && !wants_ax
        && offset.is_none()
        && max.is_none()
    {
        return Ok(serde_json::Value::Array(
            windows
                .iter()
                .map(|window| {
                    static_window_row_json(window, &stacking, !window.minimized, None, None)
                })
                .collect(),
        ));
    }
    let wanted_profile = browser_profile.as_deref().map(str::to_lowercase);
    let cheap_filter = observe::WindowFilter {
        focused: None,
        minimized: None,
        ..filter.clone()
    };
    windows.retain(|window| cheap_filter.matches(window));
    windows.retain(|window| {
        wanted_profile.as_deref().is_none_or(|wanted| {
            window_browser_profile(window)
                .is_some_and(|profile| profile.to_lowercase().contains(wanted))
        })
    });
    #[cfg(target_os = "macos")]
    if all {
        for window in &mut windows {
            window.minimized = mechanism::window_op::minimized(window.handle).map_err(|error| {
                CuError::new(
                    "windows_filter_unavailable",
                    "windows --all could not read one all-inventory window",
                )
                .with_detail(serde_json::json!({
                    "filter": "all",
                    "window": window.handle,
                    "reason": map_mechanism_err(error).message,
                }))
            })?;
        }
    }
    windows.retain(|window| filter.matches(window));
    if let Some(wanted) = space {
        #[cfg(target_os = "macos")]
        {
            let mut selected = Vec::new();
            for window in windows {
                let memberships =
                    crate::macos_spaces::spaces_for_window(window.handle).map_err(|error| {
                        CuError::new("windows_filter_unavailable", error.reason).with_detail(
                            serde_json::json!({
                                "filter": "space",
                                "window": window.handle,
                                "space": wanted,
                            }),
                        )
                    })?;
                let Some(memberships) = memberships else {
                    return Err(CuError::new(
                        "windows_filter_unavailable",
                        "managed Space membership provider is unavailable",
                    )
                    .with_detail(serde_json::json!({ "filter": "space", "space": wanted })));
                };
                if memberships.contains(&wanted) {
                    selected.push(window);
                }
            }
            windows = selected;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = windows;
            return Err(CuError::new(
                "windows_filter_unavailable",
                "managed Space filtering is macOS only",
            )
            .with_detail(serde_json::json!({ "filter": "space", "space": wanted })));
        }
    }
    if occluded.is_some() && stacking_reason.is_some() {
        return Err(CuError::new(
            "windows_filter_unavailable",
            "windows --occluded requires a native stacking inventory",
        )
        .with_detail(serde_json::json!({
            "filter": "occluded",
            "reason": stacking_reason,
        })));
    }
    let mut selected = Vec::new();
    for window in windows {
        let is_onscreen = visible_handles.contains(&window.handle) && !window.minimized;
        if onscreen.is_some_and(|wanted| wanted != is_onscreen) {
            continue;
        }
        let occluded_percent = stacking
            .iter()
            .find(|row| row.handle == window.handle)
            .map(|row| row.occluded_percent);
        if let Some(wanted) = occluded {
            let Some(percent) = occluded_percent else {
                return Err(CuError::new(
                    "windows_filter_unavailable",
                    "the stacking provider did not describe a filtered window",
                )
                .with_detail(serde_json::json!({
                    "filter": "occluded",
                    "window": window.handle,
                })));
            };
            if (percent > 0) != wanted {
                continue;
            }
        }
        selected.push(WindowWatchSample {
            window,
            onscreen: is_onscreen,
            occluded_percent,
        });
    }

    let candidate_count = selected.len();
    let mut scanned = Vec::new();
    let mut unavailable_count = 0usize;
    for row in selected
        .into_iter()
        .take(if wants_ax { ax_scan_max } else { usize::MAX })
    {
        let ax_root = wants_ax.then(|| window_ax_root_metadata(&row.window));
        if ax_root
            .as_ref()
            .is_some_and(|value| value["status"] == "unavailable")
        {
            unavailable_count += 1;
        }
        if ax_root.as_ref().is_none_or(|value| {
            ax_root_matches(
                value,
                ax_role.as_deref(),
                ax_subrole.as_deref(),
                ax_identifier.as_deref(),
            )
        }) {
            scanned.push((row, ax_root));
        }
    }
    let matched = scanned.len();
    let ax_truncated = wants_ax && candidate_count > ax_scan_max;
    let focused_window = focus.handle.and_then(|handle| {
        scanned
            .iter()
            .find(|(row, _)| row.window.handle == handle)
            .map(|(row, ax_root)| {
                static_window_row_json(
                    &row.window,
                    &stacking,
                    row.onscreen,
                    row.occluded_percent,
                    ax_root.as_ref(),
                )
            })
    });
    let (hits, page_truncated) = page.apply(&scanned);
    let rows = serde_json::Value::Array(
        hits.iter()
            .map(|(row, ax_root)| {
                static_window_row_json(
                    &row.window,
                    &stacking,
                    row.onscreen,
                    row.occluded_percent,
                    ax_root.as_ref(),
                )
            })
            .collect(),
    );
    let mut payload = serde_json::json!({
        "mechanism": "libagenterm",
        "focus": focus.json(),
        "stacking": match &stacking_reason {
            None => serde_json::json!({ "status": "available", "order": "front-to-back" }),
            Some(reason) => serde_json::json!({ "status": "unsupported", "reason": reason }),
        },
        "filter": {
            "pid": filter.pid,
            "app": filter.app,
            "title": filter.title,
            "focused": filter.focused,
            "minimized": filter.minimized,
            "space": space,
            "onscreen": onscreen,
            "occluded": occluded,
            "all": all,
            "browser_profile": browser_profile,
            "ax_role": ax_role,
            "ax_subrole": ax_subrole,
            "ax_identifier": ax_identifier,
        },
        "visited": visited,
        "matched": matched,
        "returned": hits.len(),
        "offset": page.offset,
        "truncated": page_truncated || ax_truncated,
        "windows": rows,
    });
    if wants_ax && let Some(object) = payload.as_object_mut() {
        object.insert(
            "ax_scan".into(),
            serde_json::json!({
                "scanned": candidate_count.min(ax_scan_max),
                "candidates": candidate_count,
                "matched": matched,
                "unavailable": unavailable_count,
                "max": ax_scan_max,
                "truncated": ax_truncated,
            }),
        );
    }
    // `--focused true` is a question with one answer: the focused window,
    // or an explicit "the frontmost app has no window here" -- never a
    // bare empty list that reads as "nothing is focused".
    if filter.focused == Some(true)
        && let Some(object) = payload.as_object_mut()
    {
        object.insert(
            "focused_app".into(),
            focus
                .app
                .as_ref()
                .map(FrontmostApp::json)
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "window".into(),
            focused_window.unwrap_or(serde_json::Value::Null),
        );
    }
    Ok(payload)
}

fn static_window_row_json(
    window: &WindowInfo,
    stacking: &[mechanism::window_enumerate::WindowStacking],
    onscreen: bool,
    occluded_percent: Option<u32>,
    ax_root: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut row = observe::window_row_json_with_stacking(window, stacking);
    let missing_browser_profile = row
        .get("browser_profile")
        .and_then(|value| value.as_str())
        .is_none();
    if let Some(object) = row.as_object_mut() {
        object.insert("onscreen".into(), serde_json::json!(onscreen));
        if let Some(percent) = occluded_percent {
            object.insert("occluded_percent".into(), serde_json::json!(percent));
        }
        if missing_browser_profile
            && observe::looks_like_browser_app(&window.app_name)
            && let Some(profile) = ax_root_browser_profile(window)
        {
            object.insert("browser_profile".into(), serde_json::json!(profile));
        }
        if let Some(ax_root) = ax_root {
            object.insert("ax_root".into(), ax_root.clone());
        }
    }
    row
}

fn bounded_ax_text(value: &str) -> String {
    value.chars().take(512).collect()
}

fn window_ax_root_metadata(window: &WindowInfo) -> serde_json::Value {
    let budget = mechanism::TreeBudget {
        max_depth: Some(0),
        max_nodes: Some(1),
    };
    match mechanism::tree_for_window_bounded(Some(window.handle), budget) {
        Ok(tree) => {
            let root = tree
                .nodes
                .iter()
                .find(|node| node.id == tree.root_id)
                .or_else(|| tree.nodes.first());
            let Some(root) = root else {
                return serde_json::json!({
                    "status": "unavailable",
                    "error": "accessibility root returned no node",
                });
            };
            serde_json::json!({
                "status": "ok",
                "role": bounded_ax_text(&root.role),
                "subrole": root.subrole.as_deref().map(bounded_ax_text),
                "identifier": root.identifier.as_deref().map(bounded_ax_text),
                "title": bounded_ax_text(&root.name),
                "actions": root.actions.iter().take(64).map(|value| bounded_ax_text(value)).collect::<Vec<_>>(),
            })
        }
        Err(error) => serde_json::json!({
            "status": "unavailable",
            "error": bounded_ax_text(&map_mechanism_err(error).message),
        }),
    }
}

fn ax_root_matches(
    metadata: &serde_json::Value,
    role: Option<&str>,
    subrole: Option<&str>,
    identifier: Option<&str>,
) -> bool {
    metadata["status"] == "ok"
        && role.is_none_or(|wanted| {
            metadata["role"].as_str().is_some_and(|have| {
                observe::normalize_role(have) == observe::normalize_role(wanted)
            })
        })
        && subrole.is_none_or(|wanted| {
            metadata["subrole"].as_str().is_some_and(|have| {
                observe::normalize_role(have) == observe::normalize_role(wanted)
            })
        })
        && identifier.is_none_or(|wanted| metadata["identifier"].as_str() == Some(wanted))
}

/// Decide the inventory's focused window and write it into the rows.
/// The mechanism's own mark is kept when it made one; otherwise the
/// frontmost application (NSWorkspace on macOS) and its own focused
/// window / topmost window decide (`observe::resolve_focus`).
pub(super) fn resolve_inventory_focus(
    windows: &mut [WindowInfo],
    stacking: &[mechanism::window_enumerate::WindowStacking],
) -> observe::FocusResolution {
    // The frontmost app is always reported; its AX read is only needed
    // when the mechanism left no mark.
    let app = frontmost_app();
    let already_marked = windows.iter().any(|window| window.focused);
    let ax_window = if already_marked {
        None
    } else {
        app.as_ref().and_then(|app| focused_window_of(app.pid))
    };
    resolve_inventory_focus_from_facts(windows, stacking, app, ax_window)
}

/// Pure core shared by public inventory and actuation read-back. Keeping the
/// host facts injectable makes the fallback order independently testable
/// without touching the real desktop.
fn resolve_inventory_focus_from_facts(
    windows: &mut [WindowInfo],
    stacking: &[mechanism::window_enumerate::WindowStacking],
    app: Option<FrontmostApp>,
    ax_window: Option<isize>,
) -> observe::FocusResolution {
    let focus = observe::resolve_focus(windows, stacking, app, ax_window);
    observe::apply_focus(windows, &focus);
    focus
}

#[cfg(target_os = "macos")]
fn frontmost_app() -> Option<FrontmostApp> {
    crate::macos_focus::frontmost_app()
}

#[cfg(not(target_os = "macos"))]
fn frontmost_app() -> Option<FrontmostApp> {
    None
}

#[cfg(target_os = "macos")]
fn focused_window_of(pid: u32) -> Option<isize> {
    crate::macos_focus::focused_window_of(pid)
}

#[cfg(not(target_os = "macos"))]
fn focused_window_of(_pid: u32) -> Option<isize> {
    None
}

/// The Chromium profile name a browser window belongs to: parsed from the
/// inventory title's ` - <App> - <profile>` suffix when it carries one,
/// otherwise from the AX root name (Brave keeps the suffix there while the
/// inventory title is the bare tab title). `None` for windows that are
/// not a browser's or carry no profile.
pub(super) fn window_browser_profile(window: &WindowInfo) -> Option<String> {
    observe::browser_profile_from_identity(&window.app_name, &window.title).or_else(|| {
        observe::looks_like_browser_app(&window.app_name)
            .then(|| ax_root_browser_profile(window))
            .flatten()
    })
}

pub(super) fn ax_root_browser_profile(window: &WindowInfo) -> Option<String> {
    let budget = mechanism::TreeBudget {
        max_depth: Some(0),
        max_nodes: Some(8),
    };
    let tree = mechanism::tree_for_window_bounded(Some(window.handle), budget).ok()?;
    let root = tree
        .nodes
        .iter()
        .find(|node| node.id == tree.root_id)
        .or(tree.nodes.first())?;
    observe::browser_profile_from_identity(&window.app_name, &root.name)
}

/// `apps`: the applications with a window, and with `--all` the ones that
/// are merely installed.
///
/// The two halves answer different questions from different mechanisms: a
/// running application is one the window inventory can see, an installed
/// one is a bundle on disk that may never have been started.
/// `installed_available: false` says this host cannot enumerate installed
/// applications, which is not the same as having none.
pub(super) fn apps_payload(all: bool) -> Result<serde_json::Value, CuError> {
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "mechanism": "libagenterm",
        "running_only": !all,
        "installed": false,
        "apps": observe::running_apps_json(&windows),
    });
    if !all {
        return Ok(payload);
    }
    let (installed, truncated, reason) = match mechanism::list_installed_apps() {
        Ok((apps, truncated)) => (apps, truncated, None),
        Err(mechanism::MechanismError::Unsupported { reason }) => (Vec::new(), false, Some(reason)),
        Err(error) => return Err(map_mechanism_err(error)),
    };
    // Which installed ones are up right now, matched by the name the window
    // inventory reports, so a caller asking "installed but not running?"
    // gets the answer in one read instead of joining two lists itself.
    //
    // This is a name join and the reply says so (`running_match`). It is
    // exact on macOS, where the bundle name is the name the window
    // inventory reports. It is weaker on Linux, where a desktop entry's
    // `Name` is a display name and the window reports the executable --
    // an application started through an interpreter reports `python3`, and
    // no name join can see through that. So `false` means "no running
    // window reports this name", which is what was measured, not "this
    // application is not running".
    let running_names: Vec<&str> = windows
        .iter()
        .map(|window| window.app_name.as_str())
        .collect();
    let rows: Vec<serde_json::Value> = installed
        .iter()
        .map(|app| {
            serde_json::json!({
                "name": app.name,
                "path": app.path,
                "running": running_names.contains(&app.name.as_str()),
            })
        })
        .collect();
    if let Some(object) = payload.as_object_mut() {
        object.insert("installed".into(), serde_json::json!(reason.is_none()));
        object.insert(
            "installed_available".into(),
            serde_json::json!(reason.is_none()),
        );
        object.insert("installed_apps".into(), serde_json::json!(rows));
        object.insert("installed_truncated".into(), serde_json::json!(truncated));
        object.insert("running_match".into(), serde_json::json!("window-app-name"));
        if let Some(reason) = reason {
            object.insert("installed_reason".into(), serde_json::json!(reason));
        }
    }
    Ok(payload)
}

const DEFAULT_APP_INSPECT_DEPTH: u32 = 12;
const DEFAULT_APP_INSPECT_NODES: usize = 6_000;
const DEFAULT_APP_INSPECT_WINDOWS: usize = 64;
const MAX_APP_INSPECT_WINDOWS: usize = 256;

fn app_name_matches(have: &str, wanted: &str) -> bool {
    have.to_lowercase().contains(&wanted.to_lowercase())
}

fn app_page_status(pid: u32, app_name: &str) -> serde_json::Value {
    let port = match resolve_cdp_port(None, Some(pid)) {
        Ok(port) => port,
        Err(error) if error.code == "cdp_debug_port_not_found" => {
            return serde_json::json!({
                "state": "no-debug-port",
                "cdp_scope": "process",
            });
        }
        Err(error) => {
            return serde_json::json!({
                "state": "inspect-unavailable",
                "cdp_scope": "process",
                "error": error_payload(&error),
            });
        }
    };
    match crate::cdp::targets::list_targets(port) {
        Ok(targets) => {
            let wanted = app_name.to_lowercase();
            let selected = targets.iter().find(|target| {
                target.is_page()
                    && (target.title.to_lowercase().contains(&wanted)
                        || target.url.to_lowercase().contains(&wanted)
                        || target.description.to_lowercase().contains(&wanted))
            });
            serde_json::json!({
                "state": "available",
                "cdp_scope": "process",
                "debug_port": port,
                "targets": targets.len(),
                "selected": selected.map(crate::cdp::targets::PageTarget::identity_json),
            })
        }
        Err(error) => serde_json::json!({
            "state": "unreachable",
            "cdp_scope": "process",
            "debug_port": port,
            "error": { "code": error.code, "message": error.message },
        }),
    }
}

fn matching_app_identity_set(windows: &[WindowInfo], app: &str) -> Vec<(isize, u32, String)> {
    let mut identities = windows
        .iter()
        .filter(|window| app_name_matches(&window.app_name, app))
        .map(|window| (window.handle, window.process_id, window.app_name.clone()))
        .collect::<Vec<_>>();
    identities.sort();
    identities
}

pub(super) fn focus_identity(focus: &observe::FocusResolution) -> (Option<isize>, Option<u32>) {
    (
        focus.handle,
        focus.app.as_ref().map(|application| application.pid),
    )
}

/// MCU `inspect --app`: classify every matching top-level window in one
/// bounded native call. A per-window acquisition failure remains a row rather
/// than erasing the other windows, while a reused handle/process identity is
/// never reported as a successful observation.
pub(super) fn app_inspect_payload(
    app: &str,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    max_windows: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    let app = app.trim();
    if app.is_empty() {
        return Err(invalid_input("app-inspect --app must not be empty".into()));
    }
    let depth = depth.unwrap_or(DEFAULT_APP_INSPECT_DEPTH);
    let max_nodes = max_nodes.unwrap_or(DEFAULT_APP_INSPECT_NODES);
    let max_windows = max_windows.unwrap_or(DEFAULT_APP_INSPECT_WINDOWS);
    if !(1..=32).contains(&depth) {
        return Err(invalid_input(
            "app-inspect --depth must be in 1..=32".into(),
        ));
    }
    if !(1..=observe::MAX_NODE_BUDGET).contains(&max_nodes) {
        return Err(invalid_input(format!(
            "app-inspect --max-nodes must be in 1..={}",
            observe::MAX_NODE_BUDGET
        )));
    }
    if !(1..=MAX_APP_INSPECT_WINDOWS).contains(&max_windows) {
        return Err(invalid_input(format!(
            "app-inspect --max-windows must be in 1..={MAX_APP_INSPECT_WINDOWS}"
        )));
    }
    let mut inventory =
        mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let stacking = mechanism::window_enumerate::stacking().unwrap_or_default();
    let focus_before = resolve_inventory_focus(&mut inventory, &stacking);
    let identities_before = matching_app_identity_set(&inventory, app);
    let mut matches: Vec<WindowInfo> = inventory
        .into_iter()
        .filter(|window| app_name_matches(&window.app_name, app))
        .collect();
    matches.sort_by_key(|window| (window.process_id, window.handle));
    if matches.is_empty() {
        return Err(CuError::new(
            "a11y_app_not_found",
            "no top-level window belongs to the requested application",
        )
        .with_detail(serde_json::json!({ "app": app })));
    }
    let matched = matches.len();
    matches.truncate(max_windows);
    let budget = mechanism::TreeBudget {
        max_depth: Some(depth),
        max_nodes: Some(max_nodes),
    };
    let mut rows = Vec::with_capacity(matches.len());
    let mut page_by_pid = std::collections::BTreeMap::<u32, serde_json::Value>::new();
    for window in matches {
        let process_before = agenterm_platform::process::observe(window.process_id);
        let row = match mechanism::tree_for_window_bounded(Some(window.handle), budget) {
            Ok(tree) => {
                let process_after = agenterm_platform::process::observe(window.process_id);
                let stable_process = matches!(
                    (&process_before, &process_after),
                    (
                        agenterm_platform::process::ProcessObservation::Live {
                            start_identity: Some(before),
                        },
                        agenterm_platform::process::ProcessObservation::Live {
                            start_identity: Some(after),
                        }
                    ) if before == after
                );
                if !stable_process {
                    serde_json::json!({
                        "ok": false,
                        "handle": window.handle,
                        "pid": window.process_id,
                        "app": window.app_name,
                        "title": window.title,
                        "error": {
                            "code": "a11y_app_identity_changed",
                            "message": "window or owning process identity changed during inspection"
                        }
                    })
                } else {
                    let ax = observe::classify_ax_tree(&tree);
                    let inconclusive_truncation =
                        tree.truncated && ax != observe::AxAvailability::Content;
                    let page = if let Some(page) = page_by_pid.get(&window.process_id) {
                        page.clone()
                    } else {
                        let page = app_page_status(window.process_id, &window.app_name);
                        page_by_pid.insert(window.process_id, page.clone());
                        page
                    };
                    serde_json::json!({
                        "ok": true,
                        "handle": window.handle,
                        "pid": window.process_id,
                        "app": window.app_name,
                        "title": window.title,
                        "ax": if inconclusive_truncation {
                            "inconclusive-truncated"
                        } else {
                            ax.as_str()
                        },
                        "backend": tree.backend,
                        "visited": tree.visited,
                        "returned": tree.returned,
                        "truncated": tree.truncated,
                        "page": page,
                        "page_accessibility": if ax == observe::AxAvailability::Content {
                            "native-accessibility"
                        } else if inconclusive_truncation {
                            "inconclusive-truncated"
                        } else { "not-visible-in-accessibility" },
                        "next_actions": if inconclusive_truncation {
                            Vec::<String>::new()
                        } else {
                            observe::empty_chrome_next_actions(ax, &window.app_name)
                        },
                        "identity_verified": true,
                    })
                }
            }
            Err(error) => {
                let error = map_mechanism_err(error);
                serde_json::json!({
                    "ok": false,
                    "handle": window.handle,
                    "pid": window.process_id,
                    "app": window.app_name,
                    "title": window.title,
                    "error": error_payload(&error),
                })
            }
        };
        rows.push(row);
    }
    let mut inventory_after =
        mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let stacking_after = mechanism::window_enumerate::stacking().unwrap_or_default();
    let focus_after = resolve_inventory_focus(&mut inventory_after, &stacking_after);
    let identities_after = matching_app_identity_set(&inventory_after, app);
    if identities_before != identities_after
        || focus_identity(&focus_before) != focus_identity(&focus_after)
    {
        return Err(CuError::new(
            "app_inspection_changed",
            "application windows or foreground focus changed during inspection",
        )
        .with_detail(serde_json::json!({
            "app": app,
            "windows_before": identities_before.len(),
            "windows_after": identities_after.len(),
            "focus_before": focus_before.json(),
            "focus_after": focus_after.json(),
        })));
    }
    Ok(serde_json::json!({
        "addressing": "application-window-inventory",
        "mechanism": "libagenterm",
        "app": app,
        "budget": {
            "depth": depth,
            "max_nodes": max_nodes,
            "max_windows": max_windows,
        },
        "matched": matched,
        "returned": rows.len(),
        "truncated": matched > rows.len(),
        "focus_unchanged": true,
        "windows": rows,
    }))
}

/// One argument per `windows-watch` flag the dispatcher already destructured.
#[allow(clippy::too_many_arguments)]
pub(super) fn windows_watch_payload(
    filter: observe::WindowFilter,
    space: Option<u64>,
    onscreen: Option<bool>,
    occluded: Option<bool>,
    all: bool,
    event_types: &[WindowWatchEventKind],
    duration_ms: u64,
    interval_ms: Option<u64>,
    max_events: Option<usize>,
    max_windows: Option<usize>,
    control: crate::execution_control::ExecutionControl<'_>,
) -> Result<serde_json::Value, CuError> {
    validate_windows_watch_bounds(duration_ms, max_events, interval_ms, max_windows)?;
    validate_windows_watch_space_provider(space)?;
    let request = WindowsWatchRequest {
        filter,
        space,
        onscreen,
        occluded,
        all,
        event_types,
        duration_ms,
        // The bounds above already resolved the interval, so the loop owns the single
        // resolved value and no later stage re-derives it.
        interval: Duration::from_millis(observe::windows_watch_interval_ms(
            duration_ms,
            interval_ms,
        )),
        max_events: max_events.unwrap_or(observe::DEFAULT_OBSERVE_EVENTS),
        max_windows: max_windows.unwrap_or(WINDOWS_WATCH_DEFAULT_MAX_WINDOWS),
    };
    windows_watch_with_sample(request, control, &windows_watch_sample)
}

/// The immutable per-watch request, resolved once so the normal path and the
/// cancellation partial encode from exactly the same values.
struct WindowsWatchRequest<'a> {
    filter: observe::WindowFilter,
    space: Option<u64>,
    onscreen: Option<bool>,
    occluded: Option<bool>,
    all: bool,
    event_types: &'a [WindowWatchEventKind],
    duration_ms: u64,
    interval: Duration,
    max_events: usize,
    max_windows: usize,
}

/// Everything a bounded windows watch accumulates. It is moved into the encoder,
/// so the normal and cancelled paths cannot diverge and no event row is cloned.
struct WindowsWatchState {
    previous: Vec<WindowWatchSample>,
    events: Vec<serde_json::Value>,
    polls: usize,
    inventory_count: usize,
    max_inventory_count: usize,
    truncated: bool,
}

/// The bounded windows-watch loop, GENERIC over its sample provider.
///
/// The provider is a generic `Fn` reference rather than a trait object or a type
/// alias, so production passes the real `windows_watch_sample` and a test can pass
/// a counting closure without either side imposing a `'static` bound.
fn windows_watch_with_sample<S>(
    request: WindowsWatchRequest<'_>,
    control: crate::execution_control::ExecutionControl<'_>,
    sample: &S,
) -> Result<serde_json::Value, CuError>
where
    S: Fn(
        &observe::WindowFilter,
        Option<u64>,
        Option<bool>,
        Option<bool>,
        bool,
        usize,
    ) -> Result<Vec<WindowWatchSample>, CuError>,
{
    let take_sample = || {
        sample(
            &request.filter,
            request.space,
            request.onscreen,
            request.occluded,
            request.all,
            request.max_windows,
        )
    };
    // PRE-FIRST-AUTHORITY: the ONLY direct `check_observe` in this verb, and the only
    // valid place for an `effect: not_performed` claim. It runs before the baseline
    // sample, so a pre-cancelled watch issues zero samples. It does NOT cover the
    // whole extra-once mode: when `duration_ms == 0` and the interval is non-zero,
    // the pause after the baseline can still cancel and produce a shaped partial, and
    // a token flipped inside the single extra sample still ends in the ordinary
    // payload. Those are two distinct extra-once boundaries with their own tests.
    // Every later cancellation is a private signal handled by the loop owner below.
    control.check_observe()?;
    let started = Instant::now();
    let previous = take_sample()?;
    let mut state = WindowsWatchState {
        inventory_count: previous.len(),
        max_inventory_count: previous.len(),
        previous,
        events: Vec::new(),
        polls: 1,
        truncated: false,
    };
    let extra_once = request.duration_ms == 0;
    let deadline = started + Duration::from_millis(request.duration_ms);
    let mut seq = 0u64;
    loop {
        if extra_once {
            // This mode has exactly ONE authority call left. A token is therefore
            // only observable through the sliced pause, and there is no later round
            // for it to win in: the ordinary payload is still produced below.
            let pause_deadline = Instant::now() + request.interval;
            if !request.interval.is_zero() && control.sleep_until_cancelled(pause_deadline) {
                return windows_watch_cancelled(state, &request);
            }
        } else {
            if Instant::now() >= deadline {
                break;
            }
            let pause_deadline = Instant::now()
                + request
                    .interval
                    .min(deadline.saturating_duration_since(Instant::now()));
            if control.sleep_until_cancelled(pause_deadline) {
                // DEADLINE FIRST: a bound that is already reached stays the
                // authoritative outcome even when the final slice saw the token.
                if Instant::now() >= deadline {
                    break;
                }
                return windows_watch_cancelled(state, &request);
            }
        }
        state.polls += 1;
        let current = take_sample()?;
        state.inventory_count = current.len();
        state.max_inventory_count = state.max_inventory_count.max(state.inventory_count);
        let batch = diff_windows_watch_samples(&state.previous, &current);
        let t_ms = started.elapsed().as_millis() as u64;
        for event in batch {
            if !request.event_types.is_empty()
                && !request
                    .event_types
                    .iter()
                    .any(|wanted| wanted.as_str() == event.kind)
            {
                continue;
            }
            // A full buffer is not proof of loss. Keep polling until one
            // more retained event exists; only that max+1 observation makes
            // truncation true.
            if state.events.len() == request.max_events {
                state.truncated = true;
                break;
            }
            seq += 1;
            state
                .events
                .push(window_watch_sample_event_json(seq, t_ms, &event));
        }
        state.previous = current;
        if state.truncated || extra_once {
            break;
        }
    }
    windows_watch_into_value(state, &request, None)
}

/// The post-baseline cancellation outcome: a named `cancelled` failure whose
/// structured detail carries the COMPLETE public partial watch payload.
///
/// The payload comes from the same `windows_watch_into_value` encoder as a normal
/// watch, so the partial has passed the same filter projection, the same
/// `--max-windows` behavior, the same event ceiling and the same
/// `emitted == events.length` invariant before it is attached. No raw sample is
/// ever serialized on this path. `effect` deliberately says an observation was
/// partially performed, because claiming `not_performed` after a baseline sample
/// would be false.
///
/// A cancellation is neither a timeout nor a completed watch, so the nested payload
/// reports `completed: false` while `truncated` keeps its real observed value. No
/// `termination` field is introduced: this verb never had one, two public courts
/// assert its `completed`/`truncated` pair, and the outer error code is already the
/// termination carrier.
fn windows_watch_cancelled(
    state: WindowsWatchState,
    request: &WindowsWatchRequest<'_>,
) -> Result<serde_json::Value, CuError> {
    let partial = windows_watch_into_value(state, request, Some(false))?;
    Err(CuError::new(
        "cancelled",
        "the windows watch was cancelled after observation began",
    )
    .with_detail(serde_json::json!({
        "effect": "partially_performed",
        "phase": "observe_wait",
        "partial_observation": partial,
    })))
}

/// The SOLE encoder for this verb: the ordinary return and the cancellation partial
/// both come from here, so neither path can drift from the other in key set,
/// filtering, truncation truth or the `emitted`/`events` invariant.
///
/// `completed_override` exists because completion is the ONE field whose truth
/// depends on why the watch stopped: an ordinary watch is completed exactly when the
/// event ceiling was not proven, while a cancelled watch is not completed even when
/// `truncated` is false. Passing `None` therefore keeps the ordinary
/// `completed == !truncated` rule byte-identical, and the cancelled path must pass
/// `Some(false)`. `truncated` itself is never overridden: it always keeps its real
/// observed value.
fn windows_watch_into_value(
    state: WindowsWatchState,
    request: &WindowsWatchRequest<'_>,
    completed_override: Option<bool>,
) -> Result<serde_json::Value, CuError> {
    let payload = serde_json::json!({
        "mechanism": "libagenterm",
        "mode": "poll-diff",
        "polls": state.polls,
        "emitted": state.events.len(),
        "truncated": state.truncated,
        // Unchanged ordinary truth unless a non-completion reason was supplied.
        "completed": completed_override.unwrap_or(!state.truncated),
        "duration_ms": request.duration_ms,
        "interval_ms": request.interval.as_millis() as u64,
        "max_events": request.max_events,
        "max_windows": request.max_windows,
        "inventory_count": state.inventory_count,
        "max_inventory_count": state.max_inventory_count,
        "events": state.events,
        "windows": state.previous.iter().map(window_watch_sample_row_json).collect::<Vec<_>>(),
        "filter": {
            "pid": request.filter.pid,
            "app": request.filter.app,
            "title": request.filter.title,
            "space": request.space,
            "focused": request.filter.focused,
            "minimized": request.filter.minimized,
            "onscreen": request.onscreen,
            "occluded": request.occluded,
            "all": request.all,
            "types": request.event_types.iter().map(|kind| kind.as_str()).collect::<Vec<_>>(),
        },
    });
    Ok(payload)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WindowWatchSample {
    window: mechanism::window_enumerate::WindowInfo,
    onscreen: bool,
    occluded_percent: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WindowWatchSampleEvent<'a> {
    kind: &'static str,
    row: &'a WindowWatchSample,
    fields: Vec<&'static str>,
}

fn window_watch_sample_row_json(row: &WindowWatchSample) -> serde_json::Value {
    let mut value = observe::window_row_json(&row.window);
    if let Some(object) = value.as_object_mut() {
        object.insert("onscreen".into(), serde_json::json!(row.onscreen));
        if let Some(percent) = row.occluded_percent {
            object.insert("occluded_percent".into(), serde_json::json!(percent));
        }
    }
    value
}

fn window_watch_sample_event_json(
    seq: u64,
    t_ms: u64,
    event: &WindowWatchSampleEvent<'_>,
) -> serde_json::Value {
    let mut value = window_watch_sample_row_json(event.row);
    let row = value.as_object_mut().expect("window row is an object");
    row.insert("seq".into(), serde_json::json!(seq));
    row.insert("t_ms".into(), serde_json::json!(t_ms));
    row.insert("kind".into(), serde_json::json!(event.kind));
    row.insert("fields".into(), serde_json::json!(event.fields));
    serde_json::Value::Object(std::mem::take(row))
}

fn diff_windows_watch_samples<'a>(
    before: &'a [WindowWatchSample],
    after: &'a [WindowWatchSample],
) -> Vec<WindowWatchSampleEvent<'a>> {
    let previous: BTreeMap<_, _> = before.iter().map(|row| (row.window.handle, row)).collect();
    let current: BTreeMap<_, _> = after.iter().map(|row| (row.window.handle, row)).collect();
    let mut events = Vec::new();
    for (handle, row) in &previous {
        if !current.contains_key(handle) {
            events.push(WindowWatchSampleEvent {
                kind: "disappeared",
                row,
                fields: Vec::new(),
            });
        }
    }
    for (handle, row) in &current {
        let Some(was) = previous.get(handle) else {
            events.push(WindowWatchSampleEvent {
                kind: "appeared",
                row,
                fields: Vec::new(),
            });
            continue;
        };
        let mut fields = Vec::new();
        if was.window.title != row.window.title {
            fields.push("title");
        }
        if was.window.app_name != row.window.app_name {
            fields.push("app_name");
        }
        if was.window.process_id != row.window.process_id {
            fields.push("process_id");
        }
        if was.window.bounds != row.window.bounds {
            fields.push("bounds");
        }
        if was.window.focused != row.window.focused {
            fields.push("focused");
        }
        if was.window.minimized != row.window.minimized {
            fields.push("minimized");
        }
        if was.onscreen != row.onscreen {
            fields.push("onscreen");
        }
        if was.occluded_percent != row.occluded_percent {
            fields.push("occluded_percent");
        }
        if !fields.is_empty() {
            events.push(WindowWatchSampleEvent {
                kind: "changed",
                row,
                fields,
            });
        }
    }
    events
}

/// Apply watch filters at the sample boundary. In particular, a managed
/// Space filter may not be applied after diffing: doing so loses the moment a
/// stable window enters or leaves the selected Space.
fn windows_watch_sample(
    filter: &observe::WindowFilter,
    space: Option<u64>,
    onscreen: Option<bool>,
    occluded: Option<bool>,
    all: bool,
    max_windows: usize,
) -> Result<Vec<WindowWatchSample>, CuError> {
    let visible = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let visible_handles: BTreeSet<_> = visible.iter().map(|window| window.handle).collect();
    let mut windows = if all {
        mechanism::window_enumerate::enumerate_top_level_all().map_err(map_mechanism_err)?
    } else {
        visible
    };
    if filter.focused.is_some() {
        let stacking = mechanism::window_enumerate::stacking().unwrap_or_default();
        resolve_inventory_focus(&mut windows, &stacking);
    }
    // Narrow by fields already present in the native inventory before any
    // per-window read. An unrelated all-inventory row must not make an exact
    // pid/title watch fail merely because that row's accessibility state is
    // unreadable.
    let cheap_filter = observe::WindowFilter {
        focused: None,
        minimized: None,
        ..filter.clone()
    };
    windows.retain(|window| cheap_filter.matches(window));
    #[cfg(target_os = "macos")]
    if all {
        for window in &mut windows {
            window.minimized = mechanism::window_op::minimized(window.handle).map_err(|error| {
                CuError::new(
                    "windows_watch_filter_unavailable",
                    "windows-watch --all could not read one all-inventory window",
                )
                .with_detail(serde_json::json!({
                    "filter": "all",
                    "window": window.handle,
                    "reason": map_mechanism_err(error).message,
                }))
            })?;
        }
    }
    windows.retain(|window| filter.matches(window));
    if let Some(wanted) = space {
        #[cfg(target_os = "macos")]
        {
            let mut selected = Vec::new();
            for window in windows {
                let memberships =
                    crate::macos_spaces::spaces_for_window(window.handle).map_err(|error| {
                        CuError::new("unsupported", error.reason).with_detail(serde_json::json!({
                            "group": "geometry",
                            "os": "macos",
                            "provider": "skylight-private-read",
                            "window": window.handle,
                            "filter": { "space": wanted },
                        }))
                    })?;
                let Some(memberships) = memberships else {
                    return Err(CuError::new(
                        "unsupported",
                        "managed Space membership provider is unavailable",
                    )
                    .with_detail(serde_json::json!({
                        "group": "geometry",
                        "os": "macos",
                        "provider": "none",
                        "window": window.handle,
                        "filter": { "space": wanted },
                    })));
                };
                if memberships.contains(&wanted) {
                    selected.push(window);
                }
            }
            windows = selected;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = windows;
            return Err(CuError::new(
                "unsupported",
                "managed Space filtering is macOS SkyLight only",
            )
            .with_detail(serde_json::json!({
                "group": "geometry",
                "os": crate::mcu_surface::host_os(),
                "provider": "none",
                "filter": { "space": wanted },
            })));
        }
    }
    let stacking = if occluded.is_some() {
        mechanism::window_enumerate::stacking().map_err(|error| {
            CuError::new(
                "windows_watch_filter_unavailable",
                "windows-watch --occluded requires a native stacking inventory",
            )
            .with_detail(serde_json::json!({
                "filter": "occluded",
                "reason": map_mechanism_err(error).message,
            }))
        })?
    } else {
        Vec::new()
    };
    let mut selected = Vec::new();
    for window in windows {
        let is_onscreen = visible_handles.contains(&window.handle) && !window.minimized;
        if onscreen.is_some_and(|wanted| wanted != is_onscreen) {
            continue;
        }
        let occluded_percent = stacking
            .iter()
            .find(|row| row.handle == window.handle)
            .map(|row| row.occluded_percent);
        if let Some(wanted) = occluded {
            let Some(percent) = occluded_percent else {
                return Err(CuError::new(
                    "windows_watch_filter_unavailable",
                    "the stacking provider did not describe a filtered window",
                )
                .with_detail(serde_json::json!({
                    "filter": "occluded",
                    "window": window.handle,
                })));
            };
            if (percent > 0) != wanted {
                continue;
            }
        }
        selected.push(WindowWatchSample {
            window,
            onscreen: is_onscreen,
            occluded_percent,
        });
        if selected.len() > max_windows {
            return Err(CuError::new(
                "windows_watch_inventory_truncated",
                "filtered window inventory exceeds --max-windows",
            )
            .with_detail(serde_json::json!({
                "max_windows": max_windows,
                "observed_at_least": selected.len(),
            })));
        }
    }
    Ok(selected)
}

fn validate_windows_watch_bounds(
    duration_ms: u64,
    max_events: Option<usize>,
    interval_ms: Option<u64>,
    max_windows: Option<usize>,
) -> Result<(), CuError> {
    if duration_ms > WINDOWS_WATCH_MAX_DURATION_MS {
        return Err(invalid_input(format!(
            "--duration-ms must be 0..={WINDOWS_WATCH_MAX_DURATION_MS}, got {duration_ms}"
        )));
    }
    if max_events.is_some_and(|value| value == 0 || value > WINDOWS_WATCH_MAX_EVENTS) {
        return Err(invalid_input(format!(
            "--max-events must be 1..={WINDOWS_WATCH_MAX_EVENTS}"
        )));
    }
    if max_windows.is_some_and(|value| value == 0 || value > WINDOWS_WATCH_MAX_WINDOWS) {
        return Err(invalid_input(format!(
            "--max-windows must be 1..={WINDOWS_WATCH_MAX_WINDOWS}"
        )));
    }
    if let Some(interval_ms) = interval_ms {
        if duration_ms == 0 {
            if interval_ms > WINDOWS_WATCH_MAX_DURATION_MS {
                return Err(invalid_input(format!(
                    "--interval-ms must be 0..={WINDOWS_WATCH_MAX_DURATION_MS}"
                )));
            }
        } else if interval_ms < observe::MIN_OBSERVE_INTERVAL_MS || interval_ms > duration_ms {
            return Err(invalid_input(format!(
                "--interval-ms must be {}..=duration",
                observe::MIN_OBSERVE_INTERVAL_MS
            )));
        }
    }
    Ok(())
}

fn validate_windows_watch_space_provider(space: Option<u64>) -> Result<(), CuError> {
    let Some(wanted) = space else {
        return Ok(());
    };
    if wanted == 0 {
        return Err(invalid_input(
            "windows-watch --space must be a positive managed Space id".into(),
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let inventory = crate::macos_spaces::inventory().map_err(|error| {
            CuError::new("unsupported", error.reason).with_detail(serde_json::json!({
                "group": "geometry",
                "os": "macos",
                "provider": "skylight-private-read",
                "filter": { "space": wanted },
            }))
        })?;
        if inventory["provider"] == "skylight-private-read" {
            return Ok(());
        }
        Err(CuError::new(
            "unsupported",
            "managed Space membership is unavailable on this macOS host",
        )
        .with_detail(serde_json::json!({
            "group": "geometry",
            "os": "macos",
            "provider": inventory["provider"],
            "filter": { "space": wanted },
        })))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(CuError::new(
            "unsupported",
            "managed Space filtering is macOS SkyLight only",
        )
        .with_detail(serde_json::json!({
            "group": "geometry",
            "os": crate::mcu_surface::host_os(),
            "provider": "none",
            "filter": { "space": wanted },
        })))
    }
}

pub(super) fn displays_payload() -> Result<serde_json::Value, CuError> {
    let screens = mechanism::window_enumerate::list_screens().map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "mechanism": "libagenterm",
        "via": "agt_screen_list",
        "displays": screens.iter().enumerate().map(|(index, screen)| serde_json::json!({
            "index": index,
            "primary": screen.primary,
            "frame": screen.frame,
            "workArea": screen.visible,
            "widthMm": screen.physical.width_mm,
            "heightMm": screen.physical.height_mm,
            "dpiX": screen.physical.dpi_x,
            "dpiY": screen.physical.dpi_y,
            "scaleFactor": screen.physical.scale_factor,
        })).collect::<Vec<_>>(),
        "returned": screens.len(),
    }))
}

pub(super) fn spaces_payload() -> Result<serde_json::Value, CuError> {
    #[cfg(target_os = "macos")]
    {
        crate::macos_spaces::inventory().map_err(|error| {
            CuError::new("unsupported", error.reason).with_detail(serde_json::json!({
                "group": "geometry",
                "os": "macos",
                "provider": "skylight-private-read",
            }))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(crate::host_limit::spaces_unsupported())
    }
}

pub(super) fn screenshot(path: &str, window: Option<isize>) -> Result<serde_json::Value, CuError> {
    if path.is_empty() {
        return Err(CuError::new("invalid_input", "screenshot path is required"));
    }
    let path_ref = std::path::Path::new(path);
    let (result, via) = match window {
        Some(0) => {
            return Err(CuError::new(
                "invalid_input",
                "screenshot window handle must be non-zero",
            ));
        }
        Some(raw) => (
            mechanism::screenshot::capture_native_window_png(raw, path_ref)
                .map_err(map_mechanism_err)?,
            "window-capture",
        ),
        None => (
            mechanism::screenshot::capture_native_display_png(path_ref)
                .map_err(map_mechanism_err)?,
            "display-capture",
        ),
    };
    Ok(serde_json::json!({
        "path": path,
        "window": window,
        "via": via,
        "output_width": result.output_width,
        "output_height": result.output_height,
        "output_pixels": result.output_pixels,
    }))
}

/// Default and ceiling for `zoom --pad`: a little context around the
/// region, because a crop with no margin is often unreadable.
pub(super) const DEFAULT_ZOOM_PAD: u32 = 8;
pub(super) const MAX_ZOOM_PAD: u32 = crate::command::ZOOM_PAD_MAX;

/// A screen rectangle intersected with a window's rectangle, in the
/// window's own top-left-origin coordinates. `None` when they do not
/// overlap at all.
///
/// Pure so the refusal can be tested without a display: a region that
/// misses the window must be a typed error, never an empty PNG.
pub(super) fn window_local_region(
    window_bounds: (i32, i32, i32, i32),
    region: [i32; 4],
) -> Option<(i32, i32, i32, i32)> {
    let (wx, wy, ww, wh) = window_bounds;
    if ww <= 0 || wh <= 0 || region[2] <= 0 || region[3] <= 0 {
        return None;
    }
    let left = region[0].max(wx);
    let top = region[1].max(wy);
    let right = region[0]
        .saturating_add(region[2])
        .min(wx.saturating_add(ww));
    let bottom = region[1]
        .saturating_add(region[3])
        .min(wy.saturating_add(wh));
    if right <= left || bottom <= top {
        return None;
    }
    Some((left - wx, top - wy, right - left, bottom - top))
}

/// Scale a window-local rectangle from points into the capture's pixel
/// space. A Retina window is captured at 2x, so a clip expressed in the
/// point coordinates the inventory reports would land in the top-left
/// quadrant without this.
pub(super) fn scale_region(
    region: (i32, i32, i32, i32),
    scale_x: f64,
    scale_y: f64,
) -> (i32, i32, i32, i32) {
    let scaled = |value: i32, scale: f64| {
        ((f64::from(value) * scale).round() as i64).clamp(0, i32::MAX as i64) as i32
    };
    (
        scaled(region.0, scale_x),
        scaled(region.1, scale_y),
        scaled(region.2, scale_x).max(1),
        scaled(region.3, scale_y).max(1),
    )
}

/// Translate one window-local rectangle into screen coordinates without
/// saturating a caller value into a plausible but different rectangle.
fn screen_region_from_local(
    window_origin: (i32, i32),
    local: [i32; 4],
) -> Result<[i32; 4], CuError> {
    let x = i32::try_from(i64::from(window_origin.0) + i64::from(local[0])).map_err(|_| {
        invalid_input("zoom --local-region x cannot be represented in screen coordinates".into())
    })?;
    let y = i32::try_from(i64::from(window_origin.1) + i64::from(local[1])).map_err(|_| {
        invalid_input("zoom --local-region y cannot be represented in screen coordinates".into())
    })?;
    Ok([x, y, local[2], local[3]])
}

/// `zoom --window H --region X,Y,W,H --out PATH`: one crop of one window
/// capture, so a caller can look at a detail without a full-screen image.
pub(super) fn zoom_payload(
    window: isize,
    region: Option<[i32; 4]>,
    requested_local_region: Option<[i32; 4]>,
    out: &str,
    replace: bool,
    pad: Option<u32>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "zoom requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if out.trim().is_empty() || out.contains('\0') {
        return Err(invalid_input(
            "zoom requires --out PATH (a writable PNG path)".into(),
        ));
    }
    let requested = match (region, requested_local_region) {
        (Some(value), None) | (None, Some(value)) => value,
        _ => {
            return Err(invalid_input(
                "zoom requires exactly one screen region or window-local region".into(),
            ));
        }
    };
    if requested[2] <= 0 || requested[3] <= 0 {
        return Err(invalid_input(format!(
            "zoom region X,Y,W,H needs a positive width and height, got {}x{}",
            requested[2], requested[3]
        )));
    }
    let pad = match pad {
        None => DEFAULT_ZOOM_PAD,
        Some(value) if value > MAX_ZOOM_PAD => {
            return Err(invalid_input(format!(
                "zoom --pad must be at most {MAX_ZOOM_PAD}, got {value}"
            )));
        }
        Some(value) => value,
    };
    if !replace && std::path::Path::new(out).exists() {
        return Err(invalid_input(format!(
            "zoom --out {out}: the file exists; pass --replace to overwrite it"
        )));
    }
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let Some(row) = windows.iter().find(|item| item.handle == window) else {
        return Err(CuError::new(
            "window_not_found",
            format!("no top-level window with handle {window}"),
        ));
    };
    let bounds = row.bounds;
    let window_rect = (
        bounds.x,
        bounds.y,
        bounds.width as i32,
        bounds.height as i32,
    );
    let region = if let Some(local) = requested_local_region {
        screen_region_from_local((bounds.x, bounds.y), local)?
    } else {
        requested
    };
    // The refusal is judged on the region the caller asked for: padding is
    // context around a region that already intersects, never a way for a
    // region that misses the window to be rescued into one that does not.
    if window_local_region(window_rect, region).is_none() {
        return Err(CuError::new(
            "region_outside_window",
            format!(
                "region {},{} {}x{} does not intersect window {window} ({}x{} at {},{}); nothing was written",
                region[0], region[1], region[2], region[3],
                bounds.width, bounds.height, bounds.x, bounds.y
            ),
        )
        .with_detail(serde_json::json!({
            "region": region,
            "window_bounds": bounds,
            "out": out,
            "written": false,
        })));
    }
    let pad = pad as i32;
    let padded = [
        region[0].saturating_sub(pad),
        region[1].saturating_sub(pad),
        region[2].saturating_add(pad.saturating_mul(2)),
        region[3].saturating_add(pad.saturating_mul(2)),
    ];
    let local = window_local_region(window_rect, padded)
        .or_else(|| window_local_region(window_rect, region))
        .ok_or_else(|| {
            CuError::new(
                "region_outside_window",
                format!("region {region:?} does not intersect window {window}"),
            )
        })?;
    // One full-window capture first, to learn the capture's pixel space:
    // the inventory reports points and the capture is in backing-store
    // pixels, and only their ratio can convert between them. It goes to
    // the caller's own path, so the crop that follows overwrites it and no
    // temporary file is left behind.
    let full = mechanism::screenshot::capture_native_window_png(window, std::path::Path::new(out))
        .map_err(map_mechanism_err)?;
    let scale_x = if bounds.width > 0 {
        f64::from(full.output_width) / f64::from(bounds.width)
    } else {
        1.0
    };
    let scale_y = if bounds.height > 0 {
        f64::from(full.output_height) / f64::from(bounds.height)
    } else {
        1.0
    };
    let (left, top, width, height) = scale_region(local, scale_x, scale_y);
    let cropped = mechanism::screenshot::capture_native_window_region_png(
        window,
        std::path::Path::new(out),
        left,
        top,
        width,
        height,
    )
    .map_err(map_mechanism_err)?;
    let mut reply = serde_json::json!({
        "addressing": "window-handle",
        "mechanism": "libagenterm",
        "via": "window-capture-clip",
        "path": out,
        "window": window,
        "window_bounds": bounds,
        "region": region,
        "pad": pad,
        "padded_region": padded,
        "window_local_region": { "x": local.0, "y": local.1, "width": local.2, "height": local.3 },
        "capture": {
            "width": full.output_width,
            "height": full.output_height,
            "scale_x": scale_x,
            "scale_y": scale_y,
        },
        "clip_pixels": { "x": left, "y": top, "width": width, "height": height },
        "output_width": cropped.output_width,
        "output_height": cropped.output_height,
        "output_pixels": cropped.output_pixels,
    });
    if let Some(local) = requested_local_region {
        reply["region_space"] = serde_json::json!("window-local");
        reply["requested_local_region"] = serde_json::json!(local);
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- cooperative windows-watch cancellation --------------------------------

    /// A request whose bounds are all publicly valid, so the injected seam is never
    /// exercised through a shape the public CLI would have refused.
    fn watch_request<'a>(
        duration_ms: u64,
        interval_ms: u64,
        max_events: usize,
        event_types: &'a [WindowWatchEventKind],
    ) -> WindowsWatchRequest<'a> {
        WindowsWatchRequest {
            filter: observe::WindowFilter {
                pid: None,
                app: None,
                title: None,
                focused: None,
                minimized: None,
            },
            space: None,
            onscreen: None,
            occluded: None,
            all: false,
            event_types,
            duration_ms,
            interval: Duration::from_millis(interval_ms),
            max_events,
            max_windows: WINDOWS_WATCH_DEFAULT_MAX_WINDOWS,
        }
    }

    /// One sample row. `title` is what the differ reads, so varying it is what
    /// produces a real changed event.
    fn sample_row(handle: isize, title: &str) -> WindowWatchSample {
        let mut window = focus_window(handle, 4242, true);
        window.title = title.into();
        WindowWatchSample {
            window,
            onscreen: true,
            occluded_percent: None,
        }
    }

    fn sample_of(rows: Vec<WindowWatchSample>) -> Result<Vec<WindowWatchSample>, CuError> {
        Ok(rows)
    }

    #[test]
    fn pre_cancel_issues_zero_samples_and_claims_not_performed() {
        // The provider counts and PANICS, proving no authority is reachable and that
        // the shared pre-effect check really lives inside the loop helper.
        let calls = std::cell::Cell::new(0usize);
        let sample = |_: &observe::WindowFilter,
                      _: Option<u64>,
                      _: Option<bool>,
                      _: Option<bool>,
                      _: bool,
                      _: usize|
         -> Result<Vec<WindowWatchSample>, CuError> {
            calls.set(calls.get() + 1);
            unreachable!("a pre-effect cancel must not take a sample")
        };
        let error = windows_watch_with_sample(
            watch_request(30_000, 50, 8, &[]),
            crate::execution_control::ExecutionControl::with_cancel_probe(&|| true),
            &sample,
        )
        .expect_err("a pre-effect cancel must refuse the watch");
        assert_eq!(error.code, "cancelled");
        assert_eq!(calls.get(), 0, "no sample on a pre-effect cancel");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "not_performed");
        assert!(
            detail.get("partial_observation").is_none(),
            "nothing was observed, so no partial may be claimed"
        );
    }

    #[test]
    fn a_post_baseline_pause_cancel_reports_a_shaped_partial() {
        // The token is raised from a test thread while the watch sits in a long
        // pause, i.e. AFTER the baseline exists. The nested payload must be the same
        // shape the ordinary path produces, with `completed` false because a
        // cancellation is not a completed watch.
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let token = Arc::new(AtomicBool::new(false));
        let raised = Arc::clone(&token);
        let calls = std::cell::Cell::new(0usize);
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            raised.store(true, Ordering::Release);
        });
        let probe = || token.load(Ordering::Acquire);
        let sample = |_: &observe::WindowFilter,
                      _: Option<u64>,
                      _: Option<bool>,
                      _: Option<bool>,
                      _: bool,
                      _: usize|
         -> Result<Vec<WindowWatchSample>, CuError> {
            calls.set(calls.get() + 1);
            sample_of(vec![sample_row(1, "a")])
        };
        let error = windows_watch_with_sample(
            watch_request(30_000, 2_000, 8, &[]),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            &sample,
        )
        .expect_err("a post-baseline cancel must refuse the watch");
        trigger.join().expect("cancel trigger");

        assert_eq!(error.code, "cancelled");
        assert_eq!(calls.get(), 1, "the token must stop the second sample");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "partially_performed");
        assert_eq!(detail["phase"], "observe_wait");
        let partial = &detail["partial_observation"];
        assert_eq!(partial["mechanism"], "libagenterm");
        assert_eq!(partial["mode"], "poll-diff");
        assert_eq!(partial["polls"], 1);
        assert_eq!(partial["completed"], false);
        assert_eq!(partial["truncated"], false);
        assert_eq!(partial["duration_ms"], 30_000);
        assert_eq!(partial["interval_ms"], 2_000);
        assert_eq!(partial["max_events"], 8);
        assert_eq!(partial["emitted"], 0);
        assert_eq!(partial["inventory_count"], 1);
        assert_eq!(partial["max_inventory_count"], 1);
        assert_eq!(partial["events"].as_array().map(Vec::len), Some(0));
        assert_eq!(partial["windows"].as_array().map(Vec::len), Some(1));
        // No parallel termination vocabulary was introduced on this verb.
        assert!(partial.get("termination").is_none());
    }

    #[test]
    fn a_same_round_event_ceiling_win_over_a_token_flipped_that_round() {
        // Round 2 flips the token AND proves the event ceiling. The ceiling is
        // authoritative, so the ordinary payload must be returned with truncated true
        // and no partial cancellation published.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let sample = |_: &observe::WindowFilter,
                      _: Option<u64>,
                      _: Option<bool>,
                      _: Option<bool>,
                      _: bool,
                      _: usize|
         -> Result<Vec<WindowWatchSample>, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 1 {
                return sample_of(vec![sample_row(1, "t1"), sample_row(2, "t1")]);
            }
            // Round 2 changes BOTH rows, so two events are retained at once. With a
            // ceiling of 1 the max+1 event is really observed, which is the only
            // thing that makes truncation true.
            token.set(true);
            sample_of(vec![sample_row(1, "u2"), sample_row(2, "u2")])
        };
        let value = windows_watch_with_sample(
            watch_request(30_000, 50, 1, &[]),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            &sample,
        )
        .expect("the same-round event ceiling must win");
        assert!(token.get(), "the provider really did flip the token");
        assert_eq!(calls.get(), 2);
        // The ceiling really was proven this round, so truncation is true and the
        // ordinary payload reports a non-completed watch rather than a cancellation.
        assert_eq!(value["truncated"], true);
        assert_eq!(value["completed"], false);
        assert_eq!(
            value["emitted"], 1,
            "only the ceiling-reaching event is retained"
        );
        assert_eq!(value["polls"], 2);
        assert!(value.get("termination").is_none());
    }

    #[test]
    fn a_same_round_sample_error_win_over_a_token_flipped_that_round() {
        // Round 2 flips the token and FAILS. The typed provider error is
        // authoritative and must not be rewritten as a cancellation.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let sample = |_: &observe::WindowFilter,
                      _: Option<u64>,
                      _: Option<bool>,
                      _: Option<bool>,
                      _: bool,
                      _: usize|
         -> Result<Vec<WindowWatchSample>, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 1 {
                return sample_of(vec![sample_row(1, "a")]);
            }
            token.set(true);
            Err(CuError::new(
                "windows_watch_fixture_transient",
                "the fixture inventory read failed",
            ))
        };
        let error = windows_watch_with_sample(
            watch_request(30_000, 50, 8, &[]),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            &sample,
        )
        .expect_err("the provider error must win");
        assert!(token.get(), "the provider really did flip the token");
        assert_eq!(error.code, "windows_watch_fixture_transient");
        let detail = error.detail.unwrap_or(serde_json::Value::Null);
        assert_ne!(detail["effect"], "partially_performed");
    }

    #[test]
    fn a_same_round_deadline_win_over_a_token_flipped_that_round() {
        // Deterministic deadline precedence with publicly valid bounds. The token
        // starts false. Round 1 is the baseline. Round 2 sleeps past the remaining
        // duration, THEN flips the token and returns a valid sample, so the loop must
        // exit on the reached bound rather than publishing a cancellation.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let sample = |_: &observe::WindowFilter,
                      _: Option<u64>,
                      _: Option<bool>,
                      _: Option<bool>,
                      _: bool,
                      _: usize|
         -> Result<Vec<WindowWatchSample>, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 2 {
                std::thread::sleep(Duration::from_millis(200));
                token.set(true);
            }
            sample_of(vec![sample_row(1, "a")])
        };
        let value = windows_watch_with_sample(
            watch_request(150, 50, 8, &[]),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            &sample,
        )
        .expect("the reached deadline must win over the late token");
        assert!(token.get(), "the provider really did flip the token");
        assert_eq!(calls.get(), 2, "round 2 really happened");
        assert_eq!(value["completed"], true);
        assert_eq!(value["truncated"], false);
        assert_eq!(value["polls"], 2);
    }

    #[test]
    fn an_extra_once_watch_is_not_turned_into_a_multi_round_cancel() {
        // duration_ms == 0 is the default extra-once mode: baseline plus exactly ONE
        // extra sample. A token flipped inside that extra sample has no later round to
        // win in, so the ordinary payload with polls == 2 must be returned.
        let token = std::cell::Cell::new(false);
        let calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let sample = |_: &observe::WindowFilter,
                      _: Option<u64>,
                      _: Option<bool>,
                      _: Option<bool>,
                      _: bool,
                      _: usize|
         -> Result<Vec<WindowWatchSample>, CuError> {
            let n = calls.get() + 1;
            calls.set(n);
            if n == 2 {
                token.set(true);
            }
            sample_of(vec![sample_row(1, &format!("t{n}"))])
        };
        let value = windows_watch_with_sample(
            watch_request(0, 20, 8, &[]),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            &sample,
        )
        .expect("extra-once must still produce its ordinary payload");
        assert!(token.get(), "the provider really did flip the token");
        assert_eq!(calls.get(), 2, "baseline plus exactly one extra sample");
        assert_eq!(value["polls"], 2);
        assert_eq!(value["duration_ms"], 0);
        assert_eq!(value["completed"], true);
        assert_eq!(value["truncated"], false);
    }

    #[test]
    fn an_extra_once_pause_cancel_still_produces_a_shaped_partial() {
        // The distinct extra-once boundary: duration_ms == 0 with a non-zero interval
        // can still be cancelled in the pause after the baseline, because that pause is
        // an observation point. The token is raised from a thread while the watch is in
        // that pause, so the partial provably covers a real baseline and no extra sample.
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let token = Arc::new(AtomicBool::new(false));
        let raised = Arc::clone(&token);
        let calls = std::cell::Cell::new(0usize);
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            raised.store(true, Ordering::Release);
        });
        let probe = || token.load(Ordering::Acquire);
        let sample = |_: &observe::WindowFilter,
                      _: Option<u64>,
                      _: Option<bool>,
                      _: Option<bool>,
                      _: bool,
                      _: usize|
         -> Result<Vec<WindowWatchSample>, CuError> {
            calls.set(calls.get() + 1);
            sample_of(vec![sample_row(1, "a")])
        };
        let error = windows_watch_with_sample(
            watch_request(0, 2_000, 8, &[]),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            &sample,
        )
        .expect_err("the extra-once pause cancel must refuse the watch");
        trigger.join().expect("cancel trigger");

        assert_eq!(error.code, "cancelled");
        assert_eq!(calls.get(), 1, "only the baseline was taken");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "partially_performed");
        assert_eq!(detail["phase"], "observe_wait");
        let partial = &detail["partial_observation"];
        assert_eq!(partial["duration_ms"], 0);
        assert_eq!(partial["polls"], 1);
        assert_eq!(partial["completed"], false);
        assert_eq!(partial["truncated"], false);
        assert!(partial.get("termination").is_none());
    }

    #[test]
    fn an_uncancelled_watch_keeps_its_normal_shape_and_has_no_termination() {
        // The ordinary field set is unchanged: no termination key, and completed
        // remains the negation of truncated.
        let sample = |_: &observe::WindowFilter,
                      _: Option<u64>,
                      _: Option<bool>,
                      _: Option<bool>,
                      _: bool,
                      _: usize|
         -> Result<Vec<WindowWatchSample>, CuError> {
            sample_of(vec![sample_row(1, "a")])
        };
        let value = windows_watch_with_sample(
            watch_request(120, 50, 8, &[]),
            crate::execution_control::ExecutionControl::none(),
            &sample,
        )
        .expect("an ordinary watch succeeds");
        assert_eq!(value["mechanism"], "libagenterm");
        assert_eq!(value["mode"], "poll-diff");
        assert_eq!(value["completed"], true);
        assert_eq!(value["truncated"], false);
        assert_eq!(value["duration_ms"], 120);
        assert_eq!(value["interval_ms"], 50);
        assert_eq!(value["max_events"], 8);
        assert_eq!(
            value["emitted"],
            value["events"].as_array().map(Vec::len).unwrap_or(0)
        );
        assert!(value.get("termination").is_none());
    }

    fn focus_window(handle: isize, pid: u32, focused: bool) -> WindowInfo {
        WindowInfo {
            handle,
            title: format!("window-{handle}"),
            process_id: pid,
            app_name: format!("app-{pid}"),
            bounds: mechanism::window_enumerate::WindowBounds {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            focused,
            minimized: false,
            maximized: false,
            fullscreen: false,
            above: false,
        }
    }

    fn focus_app(pid: u32) -> FrontmostApp {
        FrontmostApp {
            name: format!("app-{pid}"),
            pid,
            bundle_id: None,
        }
    }

    #[test]
    fn resolved_focus_keeps_a_mechanism_mark() {
        let mut windows = vec![focus_window(1, 10, true), focus_window(2, 20, false)];
        let focus =
            resolve_inventory_focus_from_facts(&mut windows, &[], Some(focus_app(20)), Some(2));
        assert_eq!(focus.handle, Some(1));
        assert_eq!(focus.via, Some("inventory-mark"));
        assert!(windows[0].focused);
        assert!(!windows[1].focused);
    }

    #[test]
    fn resolved_focus_accepts_only_the_frontmost_apps_ax_window() {
        let mut windows = vec![focus_window(1, 10, false), focus_window(2, 20, false)];
        let focus =
            resolve_inventory_focus_from_facts(&mut windows, &[], Some(focus_app(20)), Some(2));
        assert_eq!(focus.handle, Some(2));
        assert_eq!(focus.via, Some("ax-focused-window"));
        assert_eq!(windows.iter().filter(|row| row.focused).count(), 1);
        assert!(windows[1].focused);
    }

    #[test]
    fn resolved_focus_falls_back_within_the_frontmost_app() {
        let mut windows = vec![
            focus_window(1, 10, false),
            focus_window(2, 20, false),
            focus_window(3, 20, false),
        ];
        let stacking = [
            mechanism::window_enumerate::WindowStacking {
                handle: 1,
                z_index: 0,
                occluded_percent: 0,
            },
            mechanism::window_enumerate::WindowStacking {
                handle: 3,
                z_index: 1,
                occluded_percent: 0,
            },
            mechanism::window_enumerate::WindowStacking {
                handle: 2,
                z_index: 2,
                occluded_percent: 0,
            },
        ];
        let focus =
            resolve_inventory_focus_from_facts(&mut windows, &stacking, Some(focus_app(20)), None);
        assert_eq!(focus.handle, Some(3));
        assert_eq!(focus.via, Some("frontmost-app-front-window"));
        assert!(windows.iter().find(|row| row.handle == 3).unwrap().focused);
        assert!(!windows.iter().find(|row| row.handle == 1).unwrap().focused);
    }

    #[test]
    fn resolved_focus_refuses_foreign_ax_and_missing_frontmost_windows() {
        let mut windows = vec![focus_window(1, 10, false)];
        let focus =
            resolve_inventory_focus_from_facts(&mut windows, &[], Some(focus_app(20)), Some(1));
        assert_eq!(focus.handle, None);
        assert_eq!(focus.reason, Some("frontmost_app_has_no_inventory_window"));
        assert!(!windows[0].focused);

        let focus = resolve_inventory_focus_from_facts(&mut windows, &[], None, None);
        assert_eq!(focus.handle, None);
        assert_eq!(focus.reason, Some("no_frontmost_app"));
    }

    #[test]
    fn app_inspection_name_matching_is_case_insensitive_but_not_exact_only() {
        assert!(app_name_matches(
            "Agenterm Save Panel Fixture",
            "save panel"
        ));
        assert!(app_name_matches("EDITOR.EXE", "editor"));
        assert!(!app_name_matches("Terminal", "Editor"));
    }

    #[test]
    fn displays_lists_native_screens() {
        let reply = observe_executor().execute(&Command::Displays {
            target: TargetRef::Current,
        });
        assert_eq!(reply.command, "displays");
        if reply.ok {
            let data = reply.data.as_ref().expect("displays");
            assert_eq!(data["via"], "agt_screen_list");
            assert!(data["displays"].is_array());
        } else {
            assert_ne!(reply.error.as_ref().unwrap().code, "usage");
        }
    }

    /// The crop geometry is pure, so the "does not intersect" refusal and
    /// the point -> pixel conversion are provable without a display.
    #[test]
    fn a_region_that_misses_the_window_has_no_local_rectangle() {
        let window = (100, 50, 400, 300);
        // Fully inside.
        assert_eq!(
            window_local_region(window, [150, 100, 40, 30]),
            Some((50, 50, 40, 30))
        );
        // Straddling the left/top edges is clipped, not refused.
        assert_eq!(
            window_local_region(window, [80, 30, 40, 40]),
            Some((0, 0, 20, 20))
        );
        // Straddling the right/bottom edges is clipped too.
        assert_eq!(
            window_local_region(window, [480, 330, 100, 100]),
            Some((380, 280, 20, 20))
        );
        // Entirely outside on each side, and a touching-but-empty edge.
        for miss in [
            [0, 0, 50, 50],
            [600, 400, 10, 10],
            [500, 100, 10, 10],
            [100, 350, 10, 10],
            [150, 100, 0, 30],
            [150, 100, 40, -1],
        ] {
            assert_eq!(window_local_region(window, miss), None, "{miss:?}");
        }
        // A window with no area cannot be cropped.
        assert_eq!(window_local_region((0, 0, 0, 0), [0, 0, 10, 10]), None);
    }

    #[test]
    fn a_retina_clip_is_scaled_into_the_capture_pixel_space() {
        assert_eq!(scale_region((10, 20, 30, 40), 2.0, 2.0), (20, 40, 60, 80));
        assert_eq!(scale_region((10, 20, 30, 40), 1.0, 1.0), (10, 20, 30, 40));
        // A sub-pixel region still asks for at least one pixel.
        assert_eq!(scale_region((0, 0, 1, 1), 0.25, 0.25), (0, 0, 1, 1));
    }

    #[test]
    fn a_window_local_region_translates_without_saturation() {
        assert_eq!(
            screen_region_from_local((100, -50), [-20, 30, 40, 50]).expect("translated"),
            [80, -20, 40, 50]
        );
        let error = screen_region_from_local((i32::MAX, 0), [1, 0, 1, 1])
            .expect_err("overflow must fail typed");
        assert_eq!(error.code, "invalid_input");
    }

    #[test]
    fn zoom_refuses_its_bad_inputs_before_any_capture() {
        let executor = observe_executor();
        let zoom =
            |window: isize, region: Option<[i32; 4]>, local_region, out: &str, pad: Option<u32>| {
                executor.execute(&Command::Zoom {
                    target: TargetRef::Current,
                    window,
                    region,
                    local_region,
                    out: out.into(),
                    replace: true,
                    pad,
                })
            };
        for (reply, what) in [
            (
                zoom(0, Some([0, 0, 10, 10]), None, "/dev/null", None),
                "no window",
            ),
            (
                zoom(7, Some([0, 0, 0, 10]), None, "/dev/null", None),
                "zero width",
            ),
            (
                zoom(7, None, Some([0, 0, 10, 0]), "/dev/null", None),
                "zero height",
            ),
            (
                zoom(7, Some([0, 0, 10, 10]), None, "  ", None),
                "empty path",
            ),
            (
                zoom(
                    7,
                    Some([0, 0, 10, 10]),
                    None,
                    "/dev/null",
                    Some(MAX_ZOOM_PAD + 1),
                ),
                "pad too large",
            ),
            (zoom(7, None, None, "/dev/null", None), "no region"),
            (
                zoom(
                    7,
                    Some([0, 0, 10, 10]),
                    Some([0, 0, 10, 10]),
                    "/dev/null",
                    None,
                ),
                "two regions",
            ),
        ] {
            assert_eq!(reply.command, "zoom", "{what}");
            assert_eq!(
                reply.error.as_ref().expect("typed").code,
                "invalid_input",
                "{what}"
            );
        }
    }

    #[test]
    fn align_pty_and_windows_watch_use_group_reason_not_unknown() {
        let exec = observe_executor();
        let pty = exec.execute(&Command::Align {
            target: TargetRef::Current,
            group: "pty".into(),
        });
        assert!(!pty.ok);
        assert_eq!(pty.command, "pty");
        let err = pty.error.as_ref().expect("typed");
        assert_eq!(err.code, "unsupported");
        assert!(
            !err.message.contains("unknown MCU group"),
            "{}",
            err.message
        );
        assert_eq!(err.detail.as_ref().unwrap()["group"], "shell-pty-job");
        assert_eq!(err.detail.as_ref().unwrap()["verb"], "pty");
        let zero_space = exec.execute(&Command::WindowsWatch {
            target: TargetRef::Current,
            pid: None,
            app: None,
            title: None,
            space: Some(0),
            focused: None,
            minimized: None,
            onscreen: None,
            occluded: None,
            all: false,
            event_types: Vec::new(),
            duration_ms: 0,
            interval_ms: Some(0),
            max_events: Some(10),
            max_windows: None,
        });
        assert_eq!(
            zero_space.error.as_ref().expect("typed space id").code,
            "invalid_input"
        );
        #[cfg(not(target_os = "macos"))]
        {
            let unavailable = exec.execute(&Command::WindowsWatch {
                target: TargetRef::Current,
                pid: None,
                app: None,
                title: None,
                space: Some(1),
                focused: None,
                minimized: None,
                onscreen: None,
                occluded: None,
                all: false,
                event_types: Vec::new(),
                duration_ms: 0,
                interval_ms: Some(0),
                max_events: Some(10),
                max_windows: None,
            });
            assert_eq!(
                unavailable
                    .error
                    .as_ref()
                    .expect("typed platform limit")
                    .code,
                "unsupported"
            );
        }
        let watch = exec.execute(&Command::WindowsWatch {
            target: TargetRef::Current,
            pid: None,
            app: None,
            title: None,
            space: None,
            focused: None,
            minimized: None,
            onscreen: None,
            occluded: None,
            all: false,
            event_types: Vec::new(),
            duration_ms: 0,
            interval_ms: Some(0),
            max_events: Some(10),
            max_windows: None,
        });
        if watch.ok {
            let data = watch.data.as_ref().expect("watch data");
            assert_eq!(data["mode"], "poll-diff");
            assert!(data["events"].is_array());
            assert!(data["windows"].is_array());
        } else {
            let werr = watch.error.as_ref().expect("typed");
            assert_ne!(werr.code, "usage");
            assert!(!werr.message.contains("unknown MCU group"));
        }
        let apps = exec.execute(&Command::Apps {
            target: TargetRef::Current,
            running: true,
            all: false,
        });
        if apps.ok {
            let data = apps.data.as_ref().expect("apps data");
            assert_eq!(data["running_only"], true);
            assert_eq!(data["installed"], false);
            assert!(data["apps"].is_array());
        } else {
            assert_ne!(apps.error.as_ref().unwrap().code, "usage");
        }
        let order_same = actuate_executor().execute(&Command::OrderWin {
            target: TargetRef::Current,
            window: 1,
            relation: OrderRelation::Above,
            relative: 1,
        });
        assert!(!order_same.ok);
        assert_eq!(order_same.command, "orderwin");
        assert_eq!(order_same.error.as_ref().unwrap().code, "invalid_input");
        let order_zero = actuate_executor().execute(&Command::OrderWin {
            target: TargetRef::Current,
            window: 0,
            relation: OrderRelation::Below,
            relative: 2,
        });
        assert_eq!(order_zero.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn windows_watch_bounds_and_enriched_diff_are_loss_visible() {
        assert!(
            validate_windows_watch_bounds(300_000, Some(10_000), Some(50), Some(10_000)).is_ok()
        );
        assert!(validate_windows_watch_bounds(300_001, None, None, None).is_err());
        assert!(validate_windows_watch_bounds(1_000, Some(10_001), None, None).is_err());
        assert!(validate_windows_watch_bounds(1_000, None, None, Some(10_001)).is_err());

        let before = WindowWatchSample {
            window: WindowInfo {
                handle: 7,
                title: "before".into(),
                process_id: 42,
                app_name: "Fixture".into(),
                bounds: mechanism::window_enumerate::WindowBounds {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                focused: false,
                minimized: false,
                maximized: false,
                fullscreen: false,
                above: false,
            },
            onscreen: true,
            occluded_percent: Some(0),
        };
        let mut after = before.clone();
        after.onscreen = false;
        after.occluded_percent = Some(100);
        let previous = [before];
        let current = [after];
        let events = diff_windows_watch_samples(&previous, &current);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "changed");
        assert_eq!(events[0].fields, ["onscreen", "occluded_percent"]);
    }

    #[test]
    fn ax_root_filters_are_exact_and_unavailable_never_matches() {
        let root = serde_json::json!({
            "status": "ok",
            "role": "AXWindow",
            "subrole": "AXDialog",
            "identifier": "fixture-dialog",
        });
        assert!(ax_root_matches(
            &root,
            Some("AXWindow"),
            Some("AXDialog"),
            Some("fixture-dialog")
        ));
        assert!(!ax_root_matches(
            &root,
            Some("AXButton"),
            Some("AXDialog"),
            Some("fixture-dialog")
        ));
        assert!(!ax_root_matches(
            &serde_json::json!({ "status": "unavailable" }),
            None,
            None,
            None
        ));
        assert_eq!(bounded_ax_text(&"x".repeat(600)).chars().count(), 512);
    }
}
