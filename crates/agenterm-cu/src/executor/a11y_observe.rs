//! Accessibility-tree observation verbs: `tree`, `query`, `hit`, `focused`,
//! `observe`, `verify`, plus the tree-budget helpers they share.

use super::*;

pub(super) fn tree_budget(
    depth: Option<u32>,
    max_nodes: Option<usize>,
) -> Result<mechanism::TreeBudget, CuError> {
    observe::validate_budget(depth, max_nodes).map_err(invalid_input)?;
    Ok(mechanism::TreeBudget {
        max_depth: depth,
        max_nodes,
    })
}

pub(super) fn budget_json(depth: Option<u32>, max_nodes: Option<usize>) -> serde_json::Value {
    // `null` means the platform adapter's own default for that dimension.
    serde_json::json!({ "depth": depth, "max_nodes": max_nodes })
}

/// Bounded tree. `flat` returns the same nodes in walk order, each with its
/// flatten `index` and `depth`; the identities are the tree's own ids.
#[derive(serde::Serialize)]
struct ScopedFlatNode<'a> {
    index: usize,
    depth: u32,
    #[serde(flatten)]
    node: &'a mechanism::A11yNode,
}

pub(super) fn tree_payload(
    window: Option<isize>,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    flat: bool,
    selector: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let budget = tree_budget(depth, max_nodes)?;
    let tree = mechanism::tree_for_window_bounded(window, budget).map_err(map_mechanism_err)?;
    scoped_tree_payload(tree, depth, max_nodes, flat, selector)
}

fn scoped_tree_payload(
    tree: mechanism::A11yTree,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    flat: bool,
    selector: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let all_flat = observe::flatten(&tree);
    let scoped = if let Some(selector) = selector {
        let scoped =
            observe::query_selector_scope(&tree, &all_flat, selector).map_err(invalid_input)?;
        if scoped.is_empty() {
            return Err(CuError::new(
                "a11y_node_not_found",
                format!("tree --selector {selector:?} matched no node in the bounded walk"),
            ));
        }
        scoped
    } else {
        all_flat.iter().collect()
    };
    let selected_root_id = scoped
        .first()
        .map(|entry| entry.node.id.as_str())
        .unwrap_or(tree.root_id.as_str());
    let selected_root_depth = scoped.first().map_or(0, |entry| entry.depth);
    let nodes = if flat {
        serde_json::to_value(
            scoped
                .iter()
                .map(|entry| ScopedFlatNode {
                    index: entry.index,
                    depth: entry.depth.saturating_sub(selected_root_depth),
                    node: entry.node,
                })
                .collect::<Vec<_>>(),
        )
    } else {
        serde_json::to_value(scoped.iter().map(|entry| entry.node).collect::<Vec<_>>())
    }
    .map_err(|error| CuError::new("serialize", error.to_string()))?;
    let ax = observe::classify_ax_tree(&tree);
    let app = window_app_name(tree.window_handle);
    let mut payload = serde_json::json!({
        "degraded": false,
        "backend": tree.backend,
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "window": tree.window_handle,
        "root_id": selected_root_id,
        "flat": flat,
        "budget": budget_json(depth, max_nodes),
        "truncated": tree.truncated,
        "visited": tree.visited,
        "returned": scoped.len(),
        "ax": ax.as_str(),
        "next_actions": observe::empty_chrome_next_actions(ax, &app),
        "nodes": nodes,
    });
    if let Some(selector) = selector {
        payload["selector"] = selector.into();
        payload["selector_root_depth"] = selected_root_depth.into();
    }
    Ok(payload)
}

pub(super) fn window_app_name(handle: Option<isize>) -> String {
    let Some(handle) = handle else {
        return String::new();
    };
    mechanism::window_enumerate::enumerate_top_level()
        .ok()
        .and_then(|rows| rows.into_iter().find(|row| row.handle == handle))
        .map(|row| row.app_name)
        .unwrap_or_default()
}

fn query_watch_bounds(
    watch_ms: Option<u64>,
    until: Option<QueryWatchUntil>,
    interval_ms: Option<u64>,
    max_events: Option<usize>,
) -> Result<Option<(u64, u64, usize)>, CuError> {
    let Some(watch_ms) = watch_ms else {
        if until.is_some() || interval_ms.is_some() || max_events.is_some() {
            return Err(invalid_input(
                "query --until/--interval-ms/--max-events requires --watch-ms".into(),
            ));
        }
        return Ok(None);
    };
    if !(100..=crate::command::QUERY_WATCH_DURATION_MS_MAX).contains(&watch_ms) {
        return Err(invalid_input(format!(
            "query --watch-ms must be in 100..={}",
            crate::command::QUERY_WATCH_DURATION_MS_MAX
        )));
    }
    let interval_ms = interval_ms.unwrap_or(250);
    if !(crate::command::QUERY_WATCH_INTERVAL_MS_MIN..=crate::command::QUERY_WATCH_INTERVAL_MS_MAX)
        .contains(&interval_ms)
    {
        return Err(invalid_input(format!(
            "query --interval-ms must be in {}..={}",
            crate::command::QUERY_WATCH_INTERVAL_MS_MIN,
            crate::command::QUERY_WATCH_INTERVAL_MS_MAX
        )));
    }
    let max_events = max_events.unwrap_or(500);
    if !(1..=crate::command::QUERY_WATCH_EVENTS_MAX).contains(&max_events) {
        return Err(invalid_input(format!(
            "query --max-events must be in 1..={}",
            crate::command::QUERY_WATCH_EVENTS_MAX
        )));
    }
    Ok(Some((watch_ms, interval_ms, max_events)))
}

fn query_nodes_by_id(
    payload: &serde_json::Value,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    payload["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| Some((node["id"].as_str()?.to_owned(), node.clone())))
        .collect()
}

fn query_changed_fields(before: &serde_json::Value, after: &serde_json::Value) -> Vec<String> {
    let mut fields = std::collections::BTreeSet::new();
    if let Some(object) = before.as_object() {
        fields.extend(object.keys().cloned());
    }
    if let Some(object) = after.as_object() {
        fields.extend(object.keys().cloned());
    }
    fields
        .into_iter()
        .filter(|field| field != "index" && field != "depth" && before[field] != after[field])
        .collect()
}

fn diff_query_nodes(
    before: &serde_json::Value,
    after: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let before = query_nodes_by_id(before);
    let after = query_nodes_by_id(after);
    let mut events = Vec::new();
    for id in after.keys().filter(|id| !before.contains_key(*id)) {
        events.push(serde_json::json!({ "type": "appeared", "node_id": id }));
    }
    for id in before.keys().filter(|id| !after.contains_key(*id)) {
        events.push(serde_json::json!({ "type": "disappeared", "node_id": id }));
    }
    for (id, next) in &after {
        if let Some(previous) = before.get(id) {
            let changed_fields = query_changed_fields(previous, next);
            if !changed_fields.is_empty() {
                events.push(serde_json::json!({
                    "type": "changed",
                    "node_id": id,
                    "changed_fields": changed_fields,
                }));
            }
        }
    }
    events
}

fn query_watch_satisfied(
    until: Option<QueryWatchUntil>,
    sample: &serde_json::Value,
    changed: bool,
) -> bool {
    match until {
        None => false,
        Some(QueryWatchUntil::Present) => sample["matched"].as_u64().unwrap_or(0) > 0,
        Some(QueryWatchUntil::Absent) => {
            sample["matched"].as_u64() == Some(0)
                && sample["scan_truncated"].as_bool() == Some(false)
        }
        Some(QueryWatchUntil::Change) => changed,
    }
}

/// Bounded, filtered flat node list over the same walk `tree` makes. With a
/// watch budget, every poll repeats this exact acquisition/filter contract;
/// transient later acquisition failures are counted, never turned into an
/// empty sample, and the desktop foreground identity is bracketed.
#[allow(clippy::too_many_arguments)]
pub(super) fn query_payload(
    window: isize,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    filter: observe::NodeFilter,
    text_and_text_exact: bool,
    offset: Option<usize>,
    max: Option<usize>,
    selector: Option<&str>,
    watch_ms: Option<u64>,
    until: Option<QueryWatchUntil>,
    interval_ms: Option<u64>,
    max_events: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    let watch = query_watch_bounds(watch_ms, until, interval_ms, max_events)?;
    let focus_before = if watch.is_some() {
        Some(super::pointer::focused_window_identity()?.ok_or_else(|| {
            CuError::new(
                "focused_window_unavailable",
                "query watch requires one uniquely resolved focused top-level window",
            )
        })?)
    } else {
        None
    };
    let mut sample = query_once_payload(
        window,
        depth,
        max_nodes,
        filter.clone(),
        text_and_text_exact,
        offset,
        max,
        selector,
    )?;
    let Some((watch_ms, interval_ms, max_events)) = watch else {
        return Ok(sample);
    };
    let Some(focus_before) = focus_before else {
        return Err(CuError::new(
            "query_watch_internal",
            "query watch did not retain its foreground baseline",
        ));
    };
    let started = Instant::now();
    let deadline = started + Duration::from_millis(watch_ms);
    let mut events = Vec::new();
    let mut event_seq = 0u64;
    let mut dropped_events = 0usize;
    let mut missing_samples = 0usize;
    let mut polls = 1usize;
    let mut condition_satisfied = query_watch_satisfied(until, &sample, false);
    while !condition_satisfied && Instant::now() < deadline {
        thread::sleep(
            Duration::from_millis(interval_ms)
                .min(deadline.saturating_duration_since(Instant::now())),
        );
        polls += 1;
        let next = match query_once_payload(
            window,
            depth,
            max_nodes,
            filter.clone(),
            text_and_text_exact,
            offset,
            max,
            selector,
        ) {
            Ok(next) => next,
            Err(_) => {
                missing_samples += 1;
                continue;
            }
        };
        let batch = diff_query_nodes(&sample, &next);
        for event in &batch {
            event_seq += 1;
            if events.len() < max_events {
                let mut event = event.clone();
                event["seq"] = event_seq.into();
                event["t_ms"] = (started.elapsed().as_millis() as u64).into();
                events.push(event);
            } else {
                dropped_events += 1;
            }
        }
        sample = next;
        condition_satisfied = query_watch_satisfied(until, &sample, !batch.is_empty());
    }
    let focus_after = super::pointer::focused_window_identity()?.ok_or_else(|| {
        CuError::new(
            "focused_window_unavailable",
            "query watch could not re-read one focused top-level window",
        )
    })?;
    if focus_after != focus_before {
        return Err(CuError::new(
            "focused_window_changed",
            "desktop foreground identity changed while query watch was observing",
        ));
    }
    let observation = serde_json::json!({
        "mode": "poll-diff",
        "duration_ms": watch_ms,
        "interval_ms": interval_ms,
        "polls": polls,
        "missing_samples": missing_samples,
        "until": until,
        "condition_satisfied": condition_satisfied,
        "timed_out": until.is_some() && !condition_satisfied,
        "event_count": events.len(),
        "dropped_event_count": dropped_events,
        "truncated_events": dropped_events > 0,
        "foreground_unchanged": true,
        "events": events,
        "final": sample,
    });
    if until.is_some() && !condition_satisfied {
        return Err(CuError::new(
            "query_watch_timeout",
            "query watch exhausted its deadline before the requested condition",
        )
        .with_detail(observation));
    }
    Ok(observation)
}

#[allow(clippy::too_many_arguments)]
fn query_once_payload(
    window: isize,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    filter: observe::NodeFilter,
    text_and_text_exact: bool,
    offset: Option<usize>,
    max: Option<usize>,
    selector: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "query requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if text_and_text_exact {
        return Err(invalid_input(
            "query accepts --text or --text-exact, not both".into(),
        ));
    }
    let budget = tree_budget(depth, max_nodes)?;
    let page = observe::Page::new(offset, max).map_err(invalid_input)?;
    let tree =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let flat = observe::flatten(&tree);
    let scoped: Vec<&observe::FlatNode<'_>> = if let Some(selector) = selector {
        observe::query_selector_scope(&tree, &flat, selector).map_err(invalid_input)?
    } else {
        flat.iter().collect()
    };
    let owned: Vec<observe::FlatNode<'_>> = scoped.into_iter().cloned().collect();
    let (hits, counts) = observe::query(&owned, &filter, page, tree.truncated);
    let nodes = serde_json::to_value(&hits)
        .map_err(|error| CuError::new("serialize", error.to_string()))?;
    let mut next_actions = observe::empty_chrome_next_actions(
        observe::classify_ax_tree(&tree),
        &window_app_name(Some(window)),
    );
    next_actions.extend(truncation_next_actions(&tree, "query"));
    Ok(serde_json::json!({
        "degraded": false,
        "backend": tree.backend,
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "window": window,
        "root_id": tree.root_id,
        "budget": budget_json(depth, max_nodes),
        "filter": {
            "role": filter.roles,
            "text": filter.text,
            "text_exact": filter.text_exact,
            "identifier": filter.identifier,
            "actionable": filter.actionable,
            "within": filter.within,
            "selector": selector,
        },
        "visited": counts.visited,
        "matched": counts.matched,
        "returned": counts.returned,
        "offset": counts.offset,
        "truncated": counts.truncated,
        "scan_truncated": counts.scan_truncated,
        "page_truncated": counts.page_truncated,
        "ax": observe::classify_ax_tree(&tree).as_str(),
        "next_actions": next_actions,
        "nodes": nodes,
    }))
}

/// `hit --window H --x X --y Y`: screen coordinates -> the node under
/// them, in the shape `query` returns.
///
/// The point is resolved against the window's own bounded walk rather than
/// through a platform point-to-element call (macOS
/// `AXUIElementCopyElementAtPosition`). That is deliberate: the element
/// such a call hands back is a live `AXUIElement` with no address in the
/// id space `tree` / `query` publish, so a caller could not pass it to
/// `invoke --node` or `click --node` — which is the entire point of
/// asking what is under a point. Resolving inside the walk returns an id
/// that is already actionable, and the budget the caller sets is the
/// budget the answer came from (the reply repeats it).
pub(super) fn hit_payload(
    window: isize,
    x: i32,
    y: i32,
    depth: Option<u32>,
    max_nodes: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "hit requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    let budget = tree_budget(depth, max_nodes)?;
    let tree =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let flat = observe::flatten(&tree);
    let Some(hit) = observe::node_at_point(&flat, x, y) else {
        // A miss is typed, and it carries the walk it searched so the
        // caller can tell "nothing is there" from "the walk stopped early".
        return Err(CuError::new(
            "a11y_node_not_found",
            format!("no accessibility node of window {window} contains the point {x},{y}"),
        )
        .with_detail(serde_json::json!({
            "window": window,
            "point": [x, y],
            "budget": budget_json(depth, max_nodes),
            "visited": tree.visited,
            "returned": tree.returned,
            "truncated": tree.truncated,
            "next_actions": if tree.truncated {
                vec!["the walk was truncated: raise --depth / --max-nodes and ask again"]
            } else {
                Vec::new()
            },
        })));
    };
    let node =
        serde_json::to_value(hit).map_err(|error| CuError::new("serialize", error.to_string()))?;
    // Every node whose rectangle contains the point, innermost last: the
    // caller sees what was ranked, not only the winner.
    let containing: Vec<serde_json::Value> = flat
        .iter()
        .filter(|item| observe::node_contains_point(item.node, x, y))
        .map(|item| {
            serde_json::json!({
                "index": item.index,
                "depth": item.depth,
                "id": item.node.id,
                "role": item.node.role,
                "name": item.node.name,
            })
        })
        .collect();
    Ok(serde_json::json!({
        "degraded": false,
        "backend": tree.backend,
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "resolution": "bounded-walk-hit-test",
        "window": window,
        "point": [x, y],
        "root_id": tree.root_id,
        "budget": budget_json(depth, max_nodes),
        "visited": tree.visited,
        "returned": tree.returned,
        "truncated": tree.truncated,
        "ax": observe::classify_ax_tree(&tree).as_str(),
        "containing": containing,
        "node": node,
    }))
}

/// The application's own focused control inside `window`, role-bound when
/// the caller names one (a mismatch is typed `unverified`, never a guess).
pub(super) fn focused_control(
    window: isize,
    role: Option<&str>,
) -> Result<(String, mechanism::A11yNode), CuError> {
    let tree = mechanism::focused_node(Some(window)).map_err(map_mechanism_err)?;
    let backend = tree.backend;
    let Some(node) = tree.nodes.into_iter().next() else {
        return Err(CuError::new(
            "a11y_focus_unavailable",
            "the platform returned no focused control",
        ));
    };
    if let Some(wanted) = role
        && observe::normalize_role(&node.role) != observe::normalize_role(wanted)
    {
        return Err(CuError::new(
            "unverified",
            format!(
                "the focused control is {} {:?} (identifier {}), not role {wanted:?}",
                node.role,
                node.name,
                node.identifier.as_deref().unwrap_or("none")
            ),
        )
        .with_detail(serde_json::json!({ "observed": observe::node_state_json(&node) })));
    }
    Ok((backend, node))
}

/// `focused --window H [--role R] [--max-value-bytes N]`.
pub(super) fn focused_payload(
    window: isize,
    role: Option<&str>,
    max_value_bytes: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "focused requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    observe::validate_max_value_bytes(max_value_bytes).map_err(invalid_input)?;
    let max_value_bytes = max_value_bytes.unwrap_or(observe::DEFAULT_MAX_VALUE_BYTES);
    let (backend, node) = focused_control(window, role)?;
    let full = node.text.clone().unwrap_or_default();
    let (preview, cut) = observe::preview_value(&full, max_value_bytes);
    let adapter_truncated = node.states.iter().any(|state| state == "text-truncated");
    let mut state = observe::node_state_json(&node);
    state["bounds"] = serde_json::to_value(&node.bounds).unwrap_or(serde_json::Value::Null);
    state["actions"] = serde_json::json!(node.actions);
    state["text"] = serde_json::Value::Null;
    Ok(serde_json::json!({
        "addressing": "focused-control",
        "mechanism": "libagenterm",
        "backend": backend,
        "window": window,
        "role_bound": role,
        "node": state,
        "value": preview,
        "value_bytes": full.len(),
        "value_truncated": cut || adapter_truncated,
        "max_value_bytes": max_value_bytes,
    }))
}

/// The reply for a run that used the backend's own notifications.
///
/// It reports `mode: "notifications"` and no `polls` count, because there
/// were none: a caller comparing two runs must be able to tell which
/// mechanism produced the events. `filtered` still applies -- a caller can
/// ask for a subset of the vocabulary either way.
pub(super) fn native_observe_payload(
    window: isize,
    duration_ms: u64,
    max_events: usize,
    wanted: &[String],
    events: Vec<mechanism::A11yEvent>,
) -> serde_json::Value {
    let total = events.len();
    let mut emitted = Vec::new();
    let mut filtered = 0usize;
    for event in events {
        if !wanted.contains(&event.notification) {
            filtered += 1;
            continue;
        }
        let seq = emitted.len() as u64;
        emitted.push(serde_json::json!({
            "seq": seq,
            "t_ms": event.t_ms,
            "notification": event.notification,
            "node": {
                "id": event.node_id,
                "role": event.role,
                "name": event.name,
            },
        }));
    }
    serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": "ax",
        "mode": "notifications",
        "window": window,
        "duration_ms": duration_ms,
        "notifications": wanted,
        "max_events": max_events,
        "received": total,
        "emitted": emitted.len(),
        "filtered": filtered,
        "truncated": total >= max_events,
        "stopped": if total >= max_events { "max-events" } else { "deadline" },
        "events": emitted,
    })
}

/// `observe`: poll the bounded tree and emit the semantic differences
/// between consecutive walks as a monotonic, filtered, bounded stream. AX
/// notifications are not subscribed (the platform crate wires no
/// AXObserver); the reply says `mode: "poll-diff"`.
#[allow(clippy::too_many_arguments)]
pub(super) fn observe_payload(
    window: isize,
    duration_ms: u64,
    ready_path: Option<&str>,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    max_events: Option<usize>,
    notifications: &[String],
    interval_ms: Option<u64>,
    mode: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "observe requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if ready_path.is_some() && mode == Some("notifications") {
        return Err(invalid_input(
            "observe --ready-path currently requires --mode poll-diff; native notifications do not yet expose subscription readiness".into(),
        ));
    }
    observe::validate_observe(duration_ms, max_events, interval_ms).map_err(invalid_input)?;
    let budget = tree_budget(depth, max_nodes)?;
    let max_events = max_events.unwrap_or(observe::DEFAULT_OBSERVE_EVENTS);
    let interval =
        Duration::from_millis(interval_ms.unwrap_or(observe::DEFAULT_OBSERVE_INTERVAL_MS));
    let wanted: Vec<String> = if notifications.is_empty() {
        observe::OBSERVE_NOTIFICATIONS
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    } else {
        let mut merged = Vec::new();
        for raw in notifications {
            for name in observe::parse_notifications(raw).map_err(invalid_input)? {
                if !merged.contains(&name) {
                    merged.push(name);
                }
            }
        }
        merged
    };
    // The two modes see different things and neither subsumes the other, so
    // the caller picks and the reply says which ran. Polling compares two
    // tree walks: every event carries `before` and `after`, but a change
    // that reverts between walks is invisible and an idle interface still
    // costs a walk per interval. The backend's own notifications carry the
    // order and arrival time of every change -- including ones that revert
    // -- and cost nothing while nothing happens, but a notification says
    // "this changed", not what it changed from. Defaulting to notifications
    // would silently drop `before`/`after` from every reply, so poll-diff
    // stays the default and `--mode notifications` is the explicit ask.
    if mode == Some("notifications") {
        return match mechanism::observe_window(window, duration_ms, max_events) {
            Ok(events) => Ok(native_observe_payload(
                window,
                duration_ms,
                max_events,
                &wanted,
                events,
            )),
            Err(error) => Err(map_mechanism_err(error)),
        };
    }
    let mut previous =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let backend = previous.backend.clone();
    if let Some(path) = ready_path {
        publish_observe_ready(path, window, &backend)?;
    }
    // `duration_ms` is the observation window, not baseline acquisition.
    // Starting it after the full baseline also makes slow accessibility
    // backends receive the same advertised window as fast ones.
    let started = Instant::now();
    let deadline = started + Duration::from_millis(duration_ms);
    let mut events: Vec<serde_json::Value> = Vec::new();
    let mut seq = 0u64;
    let mut filtered = 0usize;
    let mut polls = 1usize;
    let mut poll_errors = 0usize;
    let mut last_poll_error: Option<serde_json::Value> = None;
    let mut stopped = "deadline";
    let mut truncated = false;
    loop {
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(interval.min(deadline.saturating_duration_since(Instant::now())));
        polls += 1;
        let current = match mechanism::tree_for_window_bounded(Some(window), budget) {
            Ok(tree) => tree,
            Err(mechanism::MechanismError::Unsupported { reason }) => {
                return Err(map_mechanism_err(mechanism::MechanismError::Unsupported {
                    reason,
                }));
            }
            Err(error) => {
                let error = map_mechanism_err(error);
                if error.code == "denied" {
                    return Err(error);
                }
                poll_errors += 1;
                last_poll_error = Some(error_payload(&error));
                continue;
            }
        };
        let t_ms = started.elapsed().as_millis() as u64;
        for event in observe::diff_events(&previous, &current) {
            if !wanted.iter().any(|name| name == event.notification) {
                filtered += 1;
                continue;
            }
            if events.len() >= max_events {
                truncated = true;
                stopped = "max-events";
                break;
            }
            let mut value = serde_json::to_value(&event)
                .map_err(|error| CuError::new("serialize", error.to_string()))?;
            value["seq"] = serde_json::json!(seq);
            value["t_ms"] = serde_json::json!(t_ms);
            seq += 1;
            events.push(value);
        }
        previous = current;
        if truncated {
            break;
        }
    }
    Ok(serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": backend,
        "mode": "poll-diff",
        "window": window,
        "duration_ms": duration_ms,
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "interval_ms": interval.as_millis() as u64,
        "budget": budget_json(depth, max_nodes),
        "notifications": wanted,
        "max_events": max_events,
        "polls": polls,
        "poll_errors": poll_errors,
        "last_poll_error": last_poll_error,
        "emitted": events.len(),
        "filtered": filtered,
        "truncated": truncated,
        "stopped": stopped,
        "events": events,
    }))
}

/// Publish the point after which mutations are ordered after the complete
/// poll-diff baseline. The temporary file lives beside the destination and
/// is published with a same-filesystem hard link, so publication is atomic,
/// refuses to overwrite an existing caller marker, and a reader can never
/// mistake partial JSON for readiness. The caller owns marker cleanup.
fn publish_observe_ready(path: &str, window: isize, backend: &str) -> Result<(), CuError> {
    use std::io::Write;

    let destination = std::path::Path::new(path);
    let Some(parent) = destination.parent() else {
        return Err(CuError::new(
            "observe_ready_publish_failed",
            "--ready-path has no parent directory",
        ));
    };
    if !parent.as_os_str().is_empty() && !parent.is_dir() {
        return Err(CuError::new(
            "observe_ready_publish_failed",
            format!("--ready-path parent does not exist: {}", parent.display()),
        ));
    }
    if destination.exists() {
        return Err(CuError::new(
            "observe_ready_publish_failed",
            format!("--ready-path already exists: {}", destination.display()),
        ));
    }
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CuError::new(
                "observe_ready_publish_failed",
                "--ready-path needs a UTF-8 file name",
            )
        })?;
    let temporary = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let body = serde_json::to_vec(&serde_json::json!({
        "schema": 1,
        "state": "ready",
        "mode": "poll-diff",
        "window": window,
        "backend": backend,
    }))
    .map_err(|error| CuError::new("serialize", error.to_string()))?;
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&body)?;
        file.sync_all()?;
        drop(file);
        std::fs::hard_link(&temporary, destination)?;
        std::fs::remove_file(&temporary)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(CuError::new(
            "observe_ready_publish_failed",
            format!(
                "could not publish --ready-path {}: {error}",
                destination.display()
            ),
        ));
    }
    Ok(())
}

/// One expectation checked against one flattened tree.
pub(super) struct Verdict {
    pub(super) item: serde_json::Value,
    pub(super) met: bool,
    pub(super) unknown: bool,
}

pub(super) fn check_one(
    flat: &[observe::FlatNode<'_>],
    expectation: &crate::command::Expectation,
) -> Result<Verdict, CuError> {
    if !expectation.has_state() && !expectation.has_page_identity() {
        return Err(invalid_input(
            "every --expect item needs a state (value, checked, expanded, focused) or a title substring (name / titleIncludes)".into(),
        ));
    }
    let spec = observe::TargetSpec::from_expectation(expectation);
    let node = match observe::resolve_target(flat, &spec) {
        Ok(hit) => hit.node,
        Err(observe::TargetError::Missing(message)) => {
            return Ok(Verdict {
                item: serde_json::json!({
                    "target": spec.json(),
                    "node": null,
                    "met": false,
                    "reason": message,
                }),
                met: false,
                unknown: false,
            });
        }
        Err(error) => return Err(target_error(error)),
    };
    if !expectation.has_state() {
        return Ok(Verdict {
            item: serde_json::json!({
                "target": spec.json(),
                "node": observe::node_state_json(node),
                "checks": [],
                "met": true,
                "unknown": false,
                "page_identity": true,
            }),
            met: true,
            unknown: false,
        });
    }
    let checks = observe::check_expectation(node, expectation);
    let unknown = checks.iter().any(|check| check.met.is_none());
    let met = !unknown && checks.iter().all(|check| check.met == Some(true));
    Ok(Verdict {
        item: serde_json::json!({
            "target": spec.json(),
            "node": observe::node_state_json(node),
            "checks": checks,
            "met": met,
            "unknown": unknown,
        }),
        met,
        unknown,
    })
}

pub(super) fn verify_payload(
    window: isize,
    expect: &[crate::command::Expectation],
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "verify requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if expect.is_empty() {
        return Err(invalid_input(
            "verify requires a non-empty --expect array".into(),
        ));
    }
    let tree = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let flat = observe::flatten(&tree);
    let mut results = Vec::with_capacity(expect.len());
    let mut unknown = false;
    let mut unmet = false;
    for expectation in expect {
        let verdict = check_one(&flat, expectation)?;
        unknown |= verdict.unknown;
        unmet |= !verdict.met;
        results.push(verdict.item);
    }
    let observation = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "visited": tree.visited,
        "truncated": tree.truncated,
        "results": results,
    });
    if unknown {
        return Err(CuError::new(
            "unsupported",
            "an expected state is not observable on its node; refusing to call it met",
        )
        .with_detail(
            serde_json::json!({ "reason": "state_unobservable", "observation": observation }),
        ));
    }
    if unmet {
        return Err(CuError::new(
            "unverified",
            "at least one expectation is not met by the current tree",
        )
        .with_detail(serde_json::json!({ "observation": observation })));
    }
    let mut payload = observation;
    payload["verified"] = serde_json::Value::Bool(true);
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector_tree() -> mechanism::A11yTree {
        let node =
            |id: &str, parent_id: Option<&str>, role: &str, name: &str| mechanism::A11yNode {
                id: id.into(),
                parent_id: parent_id.map(str::to_owned),
                role: role.into(),
                name: name.into(),
                states: vec!["showing".into()],
                bounds: mechanism::A11yBounds {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                actions: Vec::new(),
                text: None,
                identifier: None,
            };
        mechanism::A11yTree {
            backend: "fixture".into(),
            window_handle: Some(7),
            root_id: "/0".into(),
            nodes: vec![
                node("/0", None, "window", "root"),
                node("/0/0", Some("/0"), "group", "first"),
                node("/0/0/0", Some("/0/0"), "button", "inside"),
                node("/0/1", Some("/0"), "group", "second"),
            ],
            truncated: true,
            visited: 9,
            returned: 4,
        }
    }

    #[test]
    fn tree_selector_returns_only_the_deterministic_subtree_without_hiding_scan_truth() {
        let payload =
            scoped_tree_payload(selector_tree(), Some(4), Some(20), true, Some("Group[0]"))
                .expect("selected subtree");
        assert_eq!(payload["selector"], "Group[0]");
        assert_eq!(payload["root_id"], "/0/0");
        assert_eq!(payload["returned"], 2);
        assert_eq!(payload["visited"], 9);
        assert_eq!(payload["truncated"], true);
        assert_eq!(payload["nodes"][0]["id"], "/0/0");
        assert_eq!(payload["nodes"][0]["index"], 1);
        assert_eq!(payload["nodes"][0]["depth"], 0);
        assert_eq!(payload["nodes"][1]["id"], "/0/0/0");
        assert_eq!(payload["nodes"][1]["depth"], 1);
        assert!(
            payload["nodes"]
                .as_array()
                .expect("nodes")
                .iter()
                .all(|node| node["id"] != "/0/1")
        );
    }

    #[test]
    fn tree_selector_miss_is_typed_instead_of_an_empty_success() {
        let error = scoped_tree_payload(selector_tree(), None, None, false, Some("Button[9]"))
            .expect_err("selector miss");
        assert_eq!(error.code, "a11y_node_not_found");
    }

    #[test]
    fn query_watch_bounds_are_closed_before_any_platform_read() {
        assert!(
            query_watch_bounds(None, None, None, None)
                .expect("ordinary query")
                .is_none()
        );
        for error in [
            query_watch_bounds(None, Some(QueryWatchUntil::Present), None, None),
            query_watch_bounds(Some(99), None, None, None),
            query_watch_bounds(Some(100), None, Some(49), None),
            query_watch_bounds(Some(100), None, None, Some(0)),
        ] {
            assert_eq!(error.expect_err("closed bounds").code, "invalid_input");
        }
        assert_eq!(
            query_watch_bounds(
                Some(30_000),
                Some(QueryWatchUntil::Change),
                Some(2_000),
                Some(2_000)
            )
            .expect("upper bounds"),
            Some((30_000, 2_000, 2_000))
        );
    }

    #[test]
    fn query_watch_diff_ignores_walk_position_but_reports_semantic_change() {
        let before = serde_json::json!({"nodes": [
            {"id":"/0/a", "index":1, "depth":1, "name":"same"},
            {"id":"/0/gone", "name":"gone"}
        ]});
        let after_position_only = serde_json::json!({"nodes": [
            {"id":"/0/a", "index":9, "depth":7, "name":"same"},
            {"id":"/0/gone", "name":"gone"}
        ]});
        assert!(diff_query_nodes(&before, &after_position_only).is_empty());

        let after = serde_json::json!({"nodes": [
            {"id":"/0/a", "index":9, "depth":7, "name":"changed"},
            {"id":"/0/new", "name":"new"}
        ]});
        let events = diff_query_nodes(&before, &after);
        assert_eq!(events.len(), 3);
        assert!(events.iter().any(|event| event["type"] == "appeared"));
        assert!(events.iter().any(|event| event["type"] == "disappeared"));
        assert!(events.iter().any(|event| {
            event["type"] == "changed" && event["changed_fields"] == serde_json::json!(["name"])
        }));
    }

    #[test]
    fn query_watch_absence_requires_a_complete_scan() {
        assert!(query_watch_satisfied(
            Some(QueryWatchUntil::Absent),
            &serde_json::json!({"matched":0,"scan_truncated":false}),
            false
        ));
        assert!(!query_watch_satisfied(
            Some(QueryWatchUntil::Absent),
            &serde_json::json!({"matched":0,"scan_truncated":true}),
            false
        ));
    }

    #[test]
    fn observe_ready_marker_is_atomic_owned_json_and_never_overwrites() {
        let directory = std::env::temp_dir().join(format!(
            "agenterm-cu-observe-ready-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir(&directory).expect("temporary directory");
        let path = directory.join("ready.json");
        publish_observe_ready(path.to_str().expect("UTF-8 path"), 41, "atspi")
            .expect("publish marker");
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read marker"))
                .expect("parse marker");
        assert_eq!(marker["schema"], 1);
        assert_eq!(marker["state"], "ready");
        assert_eq!(marker["mode"], "poll-diff");
        assert_eq!(marker["window"], 41);
        assert_eq!(marker["backend"], "atspi");
        let error = publish_observe_ready(path.to_str().expect("UTF-8 path"), 42, "ax")
            .expect_err("caller-owned marker is never overwritten");
        assert_eq!(error.code, "observe_ready_publish_failed");
        assert_eq!(
            marker,
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()
        );
        std::fs::remove_file(path).expect("remove marker");
        std::fs::remove_dir(directory).expect("remove temporary directory");
    }

    #[test]
    fn native_observe_refuses_a_ready_marker_before_touching_the_backend() {
        let error = observe_payload(
            1,
            1_000,
            Some("unused-ready.json"),
            None,
            None,
            None,
            &[],
            None,
            Some("notifications"),
        )
        .expect_err("native subscription readiness is not implemented");
        assert_eq!(error.code, "invalid_input");
    }

    #[test]
    fn tree_and_query_budgets_fail_typed_before_any_mechanism_call() {
        let executor = observe_executor();
        let too_deep = executor.execute(&Command::Tree {
            target: TargetRef::Current,
            window: Some(1),
            depth: Some(65),
            max_nodes: None,
            flat: false,
            selector: None,
        });
        assert!(!too_deep.ok);
        assert_eq!(too_deep.error.as_ref().unwrap().code, "invalid_input");
        let zero_nodes = executor.execute(&Command::Tree {
            target: TargetRef::Current,
            window: Some(1),
            depth: None,
            max_nodes: Some(0),
            flat: false,
            selector: None,
        });
        assert_eq!(zero_nodes.error.as_ref().unwrap().code, "invalid_input");
        let query =
            |window: isize, text: Option<&str>, text_exact: Option<&str>, max: Option<usize>| {
                executor.execute(&Command::Query {
                    target: TargetRef::Current,
                    window,
                    depth: None,
                    max_nodes: None,
                    role: Vec::new(),
                    text: text.map(str::to_owned),
                    text_exact: text_exact.map(str::to_owned),
                    identifier: None,
                    actionable: false,
                    within: None,
                    offset: None,
                    max,
                    selector: None,
                    watch_ms: None,
                    until: None,
                    interval_ms: None,
                    max_events: None,
                })
            };
        let no_window = query(0, None, None, None);
        assert_eq!(no_window.command, "query");
        assert_eq!(no_window.error.as_ref().unwrap().code, "invalid_input");
        let both_texts = query(1, Some("a"), Some("b"), None);
        assert_eq!(both_texts.error.as_ref().unwrap().code, "invalid_input");
        let bad_page = query(1, None, None, Some(0));
        assert_eq!(bad_page.error.as_ref().unwrap().code, "invalid_input");
        let bad_windows_page = executor.execute(&Command::Windows {
            target: TargetRef::Current,
            pid: None,
            app: None,
            title: None,
            focused: None,
            minimized: None,
            browser_profile: None,
            offset: None,
            max: Some(0),
        });
        assert_eq!(
            bad_windows_page.error.as_ref().unwrap().code,
            "invalid_input"
        );
    }

    #[test]
    fn hit_refuses_a_missing_window_and_a_bad_budget_before_the_walk() {
        let executor = observe_executor();
        let no_window = executor.execute(&Command::Hit {
            target: TargetRef::Current,
            window: 0,
            x: 1,
            y: 1,
            depth: None,
            max_nodes: None,
        });
        assert!(!no_window.ok);
        assert_eq!(no_window.command, "hit");
        assert_eq!(
            no_window.error.as_ref().expect("typed").code,
            "invalid_input"
        );
        let deep = executor.execute(&Command::Hit {
            target: TargetRef::Current,
            window: 1,
            x: 1,
            y: 1,
            depth: Some(65),
            max_nodes: None,
        });
        assert_eq!(deep.error.as_ref().expect("typed").code, "invalid_input");
    }

    #[test]
    fn check_one_title_includes_heading_matches_webarea_identity() {
        let web = mechanism::A11yNode {
            id: "/0/1".into(),
            parent_id: None,
            role: "AXWebArea".into(),
            name: "Nepal floods latest: Head teacher".into(),
            states: vec!["showing".into()],
            bounds: mechanism::A11yBounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            actions: Vec::new(),
            text: None,
            identifier: None,
        };
        let tree = mechanism::A11yTree {
            backend: "ax".into(),
            window_handle: Some(1),
            root_id: "/0".into(),
            nodes: vec![web],
            truncated: false,
            visited: 1,
            returned: 1,
        };
        let flat = observe::flatten(&tree);
        let expectation: crate::command::Expectation =
            serde_json::from_str(r#"{"role":"AXHeading","titleIncludes":"Nepal"}"#)
                .expect("titleIncludes");
        let verdict = super::check_one(&flat, &expectation).expect("identity-only expect");
        assert!(verdict.met);
        assert_eq!(verdict.item["page_identity"], true);
        assert!(verdict.item["checks"].as_array().unwrap().is_empty());
    }
}
