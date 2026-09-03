//! Accessibility-tree observation verbs: `tree`, `query`, `focused`,
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
pub(super) fn tree_payload(
    window: Option<isize>,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    flat: bool,
) -> Result<serde_json::Value, CuError> {
    let budget = tree_budget(depth, max_nodes)?;
    let tree = mechanism::tree_for_window_bounded(window, budget).map_err(map_mechanism_err)?;
    let nodes = if flat {
        serde_json::to_value(observe::flatten(&tree))
    } else {
        serde_json::to_value(&tree.nodes)
    }
    .map_err(|error| CuError::new("serialize", error.to_string()))?;
    let ax = observe::classify_ax_tree(&tree);
    let app = window_app_name(tree.window_handle);
    Ok(serde_json::json!({
        "degraded": false,
        "backend": tree.backend,
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "window": tree.window_handle,
        "root_id": tree.root_id,
        "flat": flat,
        "budget": budget_json(depth, max_nodes),
        "truncated": tree.truncated,
        "visited": tree.visited,
        "returned": tree.returned,
        "ax": ax.as_str(),
        "next_actions": observe::empty_chrome_next_actions(ax, &app),
        "nodes": nodes,
    }))
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

/// Bounded, filtered flat node list over the same walk `tree` makes.
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
    let started = Instant::now();
    let deadline = started + Duration::from_millis(duration_ms);
    let mut previous =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let backend = previous.backend.clone();
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

    #[test]
    fn tree_and_query_budgets_fail_typed_before_any_mechanism_call() {
        let executor = observe_executor();
        let too_deep = executor.execute(&Command::Tree {
            target: TargetRef::Current,
            window: Some(1),
            depth: Some(65),
            max_nodes: None,
            flat: false,
        });
        assert!(!too_deep.ok);
        assert_eq!(too_deep.error.as_ref().unwrap().code, "invalid_input");
        let zero_nodes = executor.execute(&Command::Tree {
            target: TargetRef::Current,
            window: Some(1),
            depth: None,
            max_nodes: Some(0),
            flat: false,
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
            offset: None,
            max: Some(0),
        });
        assert_eq!(
            bad_windows_page.error.as_ref().unwrap().code,
            "invalid_input"
        );
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
