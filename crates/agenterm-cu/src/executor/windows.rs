//! Window inventory and geometry verbs: `windows`, `windows-watch`,
//! `apps`, `orderwin`, `displays`, `spaces`, and the window captures
//! `screenshot` / `zoom`.

use super::*;

use crate::observe::FrontmostApp;

/// Window inventory. The bare verb keeps its array reply; any filter or page
/// field switches to the inventory object with counts. `browser_profile`
/// is the one filter the plain `WindowFilter` cannot judge from the
/// inventory row alone (a Chromium window's profile sits in its AX root
/// name when the title does not carry it), so it is applied here after
/// the row filter and before paging.
pub(super) fn windows_payload(
    filter: observe::WindowFilter,
    browser_profile: Option<String>,
    offset: Option<usize>,
    max: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    let page = observe::Page::new(offset, max).map_err(invalid_input)?;
    let mut windows =
        mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
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
    let row_json = |window: &WindowInfo| {
        let mut row = observe::window_row_json_with_stacking(window, &stacking);
        if row
            .get("browser_profile")
            .and_then(|value| value.as_str())
            .is_none()
            && observe::looks_like_browser_app(&window.app_name)
            && let Some(profile) = ax_root_browser_profile(window)
            && let Some(object) = row.as_object_mut()
        {
            object.insert("browser_profile".into(), serde_json::json!(profile));
        }
        row
    };
    if filter.is_empty() && browser_profile.is_none() && offset.is_none() && max.is_none() {
        return Ok(serde_json::Value::Array(
            windows.iter().map(row_json).collect(),
        ));
    }
    let wanted_profile = browser_profile.as_deref().map(str::to_lowercase);
    let matched: Vec<&WindowInfo> = windows
        .iter()
        .filter(|window| filter.matches(window))
        .filter(|window| {
            wanted_profile.as_deref().is_none_or(|wanted| {
                window_browser_profile(window)
                    .is_some_and(|profile| profile.to_lowercase().contains(wanted))
            })
        })
        .collect();
    let (hits, page_truncated) = page.apply(&matched);
    let rows = serde_json::Value::Array(hits.iter().copied().map(row_json).collect());
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
            "browser_profile": browser_profile,
        },
        "visited": windows.len(),
        "matched": matched.len(),
        "returned": hits.len(),
        "offset": page.offset,
        "truncated": page_truncated,
        "windows": rows,
    });
    // `--focused true` is a question with one answer: the focused window,
    // or an explicit "the frontmost app has no window here" -- never a
    // bare empty list that reads as "nothing is focused".
    if filter.focused == Some(true)
        && let Some(object) = payload.as_object_mut()
    {
        let window = focus
            .handle
            .and_then(|handle| windows.iter().find(|window| window.handle == handle))
            .map(row_json);
        object.insert(
            "focused_app".into(),
            focus
                .app
                .as_ref()
                .map(FrontmostApp::json)
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert("window".into(), window.unwrap_or(serde_json::Value::Null));
    }
    Ok(payload)
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

pub(super) fn filtered_windows(filter: &observe::WindowFilter) -> Result<Vec<WindowInfo>, CuError> {
    let mut windows =
        mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    if filter.focused.is_some() {
        let stacking = mechanism::window_enumerate::stacking().unwrap_or_default();
        resolve_inventory_focus(&mut windows, &stacking);
    }
    Ok(windows
        .into_iter()
        .filter(|window| filter.matches(window))
        .collect())
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

pub(super) fn windows_watch_payload(
    filter: observe::WindowFilter,
    duration_ms: u64,
    interval_ms: Option<u64>,
    max_events: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    observe::validate_windows_watch(duration_ms, max_events, interval_ms).map_err(invalid_input)?;
    let max_events = max_events.unwrap_or(observe::DEFAULT_OBSERVE_EVENTS);
    let interval =
        Duration::from_millis(observe::windows_watch_interval_ms(duration_ms, interval_ms));
    let started = Instant::now();
    let mut previous = filtered_windows(&filter)?;
    let mut events = Vec::new();
    let mut seq = 0u64;
    let mut polls = 1usize;
    let mut truncated = false;
    let extra_once = duration_ms == 0;
    let deadline = started + Duration::from_millis(duration_ms);
    loop {
        if extra_once {
            if !interval.is_zero() {
                thread::sleep(interval);
            }
        } else {
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(interval.min(deadline.saturating_duration_since(Instant::now())));
        }
        polls += 1;
        let current = filtered_windows(&filter)?;
        let batch = observe::diff_window_inventory(&previous, &current);
        let t_ms = started.elapsed().as_millis() as u64;
        for event in batch {
            seq += 1;
            events.push(observe::window_watch_event_json(seq, t_ms, &event));
            if events.len() >= max_events {
                truncated = true;
                break;
            }
        }
        previous = current;
        if truncated || extra_once {
            break;
        }
    }
    Ok(serde_json::json!({
        "mechanism": "libagenterm",
        "mode": "poll-diff",
        "polls": polls,
        "emitted": events.len(),
        "truncated": truncated,
        "duration_ms": duration_ms,
        "interval_ms": interval.as_millis() as u64,
        "events": events,
        "windows": previous.iter().map(observe::window_row_json).collect::<Vec<_>>(),
    }))
}

/// MCU `orderwin`: `above` raises `window`, `below` raises `relative`.
pub(super) fn orderwin_payload(
    window: isize,
    relation: OrderRelation,
    relative: isize,
) -> Result<serde_json::Value, CuError> {
    if window == 0 || relative == 0 {
        return Err(invalid_input(
            "orderwin requires --window H --relative H (non-zero handles from windows)".into(),
        ));
    }
    if window == relative {
        return Err(invalid_input(
            "orderwin --window and --relative must be distinct handles".into(),
        ));
    }
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let target = windows.iter().find(|item| item.handle == window);
    let other = windows.iter().find(|item| item.handle == relative);
    if target.is_none() {
        return Err(CuError::new(
            "a11y_window_gone",
            format!("orderwin --window {window} is not in the current inventory"),
        ));
    }
    if other.is_none() {
        return Err(CuError::new(
            "a11y_window_gone",
            format!("orderwin --relative {relative} is not in the current inventory"),
        ));
    }
    let raised = match relation {
        OrderRelation::Above => window,
        OrderRelation::Below => relative,
    };
    // Snapshot the order first: the reply has to be able to show what
    // actually moved, and a window manager that declines to restack has to
    // be distinguishable from one that had nothing to do.
    let before = order_snapshot(window, relative);
    mechanism::window_op::show(raised, mechanism::window_op::SHOW).map_err(map_mechanism_err)?;
    // Restacking is a *request* to the window manager, applied (or refused)
    // asynchronously -- the same reason `app hide` has to poll before the
    // windows are gone. Sending it is not evidence that it happened, so
    // read the order back rather than reporting success from the send.
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut after = order_snapshot(window, relative);
    loop {
        if after.holds(relation) || Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(25));
        after = order_snapshot(window, relative);
    }
    let payload = serde_json::json!({
        "mechanism": "libagenterm",
        "via": "native-window-show",
        "relation": relation.as_str(),
        "window": window,
        "relative": relative,
        "raised": raised,
        "before": before.json(),
        "after": after.json(),
    });
    match after.verdict(relation) {
        OrderVerdict::Holds => Ok(payload),
        // The host cannot report a stacking order at all, so nothing here
        // can confirm or deny the move. Say that, rather than letting the
        // absence of a contradiction read as success.
        OrderVerdict::Unverifiable(reason) => {
            let mut payload = payload;
            if let Some(object) = payload.as_object_mut() {
                object.insert("verified".into(), serde_json::json!(false));
                object.insert(
                    "verification".into(),
                    serde_json::json!({
                        "method": "stacking-readback",
                        "reason": reason,
                    }),
                );
            }
            Ok(payload)
        }
        OrderVerdict::Refused => Err(CuError::new(
            "window_order_not_applied",
            format!(
                "the window manager did not place {window} {} {relative}; the order is unchanged",
                relation.as_str()
            ),
        )
        .with_detail(payload)),
    }
}

/// The two handles' places in the front-to-back order, or why they are not
/// readable. `z_index` 0 is frontmost.
pub(super) struct OrderSnapshot {
    window: Option<u32>,
    relative: Option<u32>,
    reason: Option<String>,
}

pub(super) enum OrderVerdict {
    Holds,
    Refused,
    Unverifiable(String),
}

impl OrderSnapshot {
    fn holds(&self, relation: OrderRelation) -> bool {
        let (Some(window), Some(relative)) = (self.window, self.relative) else {
            return false;
        };
        match relation {
            OrderRelation::Above => window < relative,
            OrderRelation::Below => window > relative,
        }
    }

    fn verdict(&self, relation: OrderRelation) -> OrderVerdict {
        if self.holds(relation) {
            return OrderVerdict::Holds;
        }
        if let Some(reason) = &self.reason {
            return OrderVerdict::Unverifiable(reason.clone());
        }
        match (self.window, self.relative) {
            (Some(_), Some(_)) => OrderVerdict::Refused,
            _ => OrderVerdict::Unverifiable(
                "one of the two windows is no longer in the stacking order".to_owned(),
            ),
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({ "window_z": self.window, "relative_z": self.relative })
    }
}

pub(super) fn order_snapshot(window: isize, relative: isize) -> OrderSnapshot {
    match mechanism::window_enumerate::stacking() {
        Ok(rows) => {
            let z = |handle: isize| {
                rows.iter()
                    .find(|row| row.handle == handle)
                    .map(|row| row.z_index)
            };
            OrderSnapshot {
                window: z(window),
                relative: z(relative),
                reason: None,
            }
        }
        Err(mechanism::MechanismError::Unsupported { reason }) => OrderSnapshot {
            window: None,
            relative: None,
            reason: Some(reason),
        },
        Err(mechanism::MechanismError::Failed { code, message }) => OrderSnapshot {
            window: None,
            relative: None,
            reason: Some(format!("{code}: {message}")),
        },
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
        Err(
            CuError::new("unsupported", "spaces inventory is macOS SkyLight only").with_detail(
                serde_json::json!({
                    "group": "geometry",
                    "os": crate::mcu_surface::host_os(),
                    "provider": "none",
                }),
            ),
        )
    }
}

pub(super) fn screenshot(path: &str, window: Option<isize>) -> Result<serde_json::Value, CuError> {
    if path.is_empty() {
        return Err(CuError::new("invalid_input", "screenshot path is required"));
    }
    let raw = window.unwrap_or(0);
    if raw == 0 {
        return Err(CuError::new(
            "invalid_input",
            "screenshot window handle must be non-zero",
        ));
    }
    let result = mechanism::screenshot::capture_native_window_png(raw, std::path::Path::new(path))
        .map_err(map_mechanism_err)?;
    Ok(serde_json::json!({
        "path": path,
        "window": window,
        "output_width": result.output_width,
        "output_height": result.output_height,
        "output_pixels": result.output_pixels,
    }))
}

/// Default and ceiling for `zoom --pad`: a little context around the
/// region, because a crop with no margin is often unreadable.
pub(super) const DEFAULT_ZOOM_PAD: u32 = 8;
pub(super) const MAX_ZOOM_PAD: u32 = 512;

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

/// `zoom --window H --region X,Y,W,H --out PATH`: one crop of one window
/// capture, so a caller can look at a detail without a full-screen image.
pub(super) fn zoom_payload(
    window: isize,
    region: [i32; 4],
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
    if region[2] <= 0 || region[3] <= 0 {
        return Err(invalid_input(format!(
            "zoom --region X,Y,W,H needs a positive width and height, got {}x{}",
            region[2], region[3]
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
    Ok(serde_json::json!({
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
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn zoom_refuses_its_bad_inputs_before_any_capture() {
        let executor = observe_executor();
        let zoom = |window: isize, region: [i32; 4], out: &str, pad: Option<u32>| {
            executor.execute(&Command::Zoom {
                target: TargetRef::Current,
                window,
                region,
                out: out.into(),
                replace: true,
                pad,
            })
        };
        for (reply, what) in [
            (zoom(0, [0, 0, 10, 10], "/dev/null", None), "no window"),
            (zoom(7, [0, 0, 0, 10], "/dev/null", None), "zero width"),
            (zoom(7, [0, 0, 10, 0], "/dev/null", None), "zero height"),
            (zoom(7, [0, 0, 10, 10], "  ", None), "empty path"),
            (
                zoom(7, [0, 0, 10, 10], "/dev/null", Some(MAX_ZOOM_PAD + 1)),
                "pad too large",
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
        let watch = exec.execute(&Command::WindowsWatch {
            target: TargetRef::Current,
            pid: None,
            app: None,
            title: None,
            duration_ms: 0,
            interval_ms: Some(0),
            max_events: Some(10),
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
}
