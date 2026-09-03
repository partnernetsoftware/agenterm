//! Window inventory and geometry verbs: `windows`, `windows-watch`,
//! `apps`, `orderwin`, `displays`, `spaces`, and window `screenshot`.

use super::*;

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
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
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
    Ok(serde_json::json!({
        "mechanism": "libagenterm",
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
    }))
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
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
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
