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

fn bounded_node_json(
    node: &mechanism::A11yNode,
    max_value_bytes: usize,
) -> Result<serde_json::Value, CuError> {
    use sha2::{Digest, Sha256};

    let mut value =
        serde_json::to_value(node).map_err(|error| CuError::new("serialize", error.to_string()))?;
    let Some(full) = node.text.as_deref() else {
        return Ok(value);
    };
    let (preview, bounded_truncated) = observe::preview_value(full, max_value_bytes);
    let adapter_truncated = node.states.iter().any(|state| state == "text-truncated");
    let object = value
        .as_object_mut()
        .expect("A11yNode serializes as an object");
    object.insert("text".into(), preview.into());
    object.insert("value_bytes".into(), full.len().into());
    object.insert(
        "value_truncated".into(),
        (bounded_truncated || adapter_truncated).into(),
    );
    object.insert("value_complete".into(), (!adapter_truncated).into());
    if !adapter_truncated {
        let digest = Sha256::digest(full.as_bytes());
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        object.insert("value_sha256".into(), hex.into());
    }
    Ok(value)
}

fn selector_segment_has_index(raw: &str) -> bool {
    raw.split_once('@')
        .map_or(raw, |(before_title, _)| before_title)
        .contains('[')
}

fn selector_node_matches(node: &mechanism::A11yNode, segment: &observe::SelectorSegment) -> bool {
    if let Some(role) = segment.role.as_deref()
        && observe::normalize_role(&node.role) != observe::normalize_role(role)
    {
        return false;
    }
    if let Some(title) = segment.title.as_deref()
        && !node.name.contains(title)
        && !node
            .identifier
            .as_deref()
            .is_some_and(|identifier| identifier.contains(title))
    {
        return false;
    }
    if let Some(description) = segment.description.as_deref()
        && !node.name.contains(description)
        && !node
            .identifier
            .as_deref()
            .is_some_and(|identifier| identifier.contains(description))
    {
        return false;
    }
    true
}

struct TreeIndex<'a> {
    by_id: std::collections::HashMap<&'a str, &'a mechanism::A11yNode>,
    children: std::collections::HashMap<&'a str, Vec<&'a mechanism::A11yNode>>,
}

fn index_tree(tree: &mechanism::A11yTree) -> Result<TreeIndex<'_>, CuError> {
    let mut by_id = std::collections::HashMap::with_capacity(tree.nodes.len());
    let mut children = std::collections::HashMap::new();
    for node in &tree.nodes {
        if by_id.insert(node.id.as_str(), node).is_some() {
            return Err(CuError::new(
                "a11y_tree_invalid",
                format!("accessibility tree repeats node id {:?}", node.id),
            ));
        }
        if let Some(parent_id) = node.parent_id.as_deref() {
            children
                .entry(parent_id)
                .or_insert_with(Vec::new)
                .push(node);
        }
    }
    Ok(TreeIndex { by_id, children })
}

fn resolve_tree_selector<'a>(
    tree: &'a mechanism::A11yTree,
    index: &TreeIndex<'a>,
    root_id: &str,
    selector: &str,
) -> Result<&'a mechanism::A11yNode, CuError> {
    let path = observe::parse_selector(selector).map_err(invalid_input)?;
    let raw_segments = selector
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty());
    let mut current = index.by_id.get(root_id).copied().ok_or_else(|| {
        CuError::new(
            "a11y_node_not_found",
            "the bounded accessibility tree did not contain its declared root",
        )
    })?;
    for (position, (segment, raw)) in path.iter().zip(raw_segments).enumerate() {
        let matches = index
            .children
            .get(current.id.as_str())
            .into_iter()
            .flatten()
            .copied()
            .filter(|node| selector_node_matches(node, segment))
            .collect::<Vec<_>>();
        if matches.is_empty() || segment.index >= matches.len() {
            return Err(CuError::new(
                "a11y_node_not_found",
                format!(
                    "tree --selector {selector:?} matched no node at segment {}",
                    position + 1
                ),
            )
            .with_detail(serde_json::json!({
                "selector": selector,
                "segment": position + 1,
                "matches": matches.len(),
            })));
        }
        if !selector_segment_has_index(raw) && matches.len() != 1 {
            return Err(CuError::new(
                "a11y_node_ambiguous",
                format!(
                    "tree --selector {selector:?} matched {} sibling nodes at segment {}; add an explicit [index]",
                    matches.len(),
                    position + 1
                ),
            )
            .with_count(matches.len())
            .with_detail(serde_json::json!({
                "selector": selector,
                "segment": position + 1,
                "matches": matches.len(),
            })));
        }
        current = matches[segment.index];
    }
    let resolved = observe::walk_selector(tree, selector)
        .map_err(invalid_input)?
        .ok_or_else(|| {
            CuError::new(
                "a11y_node_not_found",
                format!("tree --selector {selector:?} matched no node"),
            )
        })?;
    if resolved.id != current.id {
        return Err(CuError::new(
            "a11y_tree_invalid",
            "selector uniqueness check disagreed with the shared selector resolver",
        ));
    }
    Ok(resolved)
}

fn subtree_ids(
    index: &TreeIndex<'_>,
    root_id: &str,
) -> Result<std::collections::HashSet<String>, CuError> {
    let mut ids = std::collections::HashSet::from([root_id.to_owned()]);
    let mut pending = vec![root_id.to_owned()];
    while let Some(parent_id) = pending.pop() {
        for child in index.children.get(parent_id.as_str()).into_iter().flatten() {
            if !ids.insert(child.id.clone()) {
                return Err(CuError::new(
                    "a11y_tree_invalid",
                    format!(
                        "accessibility tree repeats or cycles through node {:?}",
                        child.id
                    ),
                ));
            }
            pending.push(child.id.clone());
        }
    }
    Ok(ids)
}

fn nested_subtree_node(
    index: &TreeIndex<'_>,
    node: &mechanism::A11yNode,
    remaining: &mut std::collections::HashSet<String>,
    max_value_bytes: usize,
) -> Result<serde_json::Value, CuError> {
    if !remaining.remove(&node.id) {
        return Err(CuError::new(
            "a11y_tree_invalid",
            format!("accessibility subtree repeats node {:?}", node.id),
        ));
    }
    let children = index
        .children
        .get(node.id.as_str())
        .into_iter()
        .flatten()
        .map(|child| nested_subtree_node(index, child, remaining, max_value_bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let mut value = bounded_node_json(node, max_value_bytes)?;
    value
        .as_object_mut()
        .expect("A11yNode serializes as an object")
        .insert("children".into(), serde_json::Value::Array(children));
    Ok(value)
}

pub(super) fn tree_payload(
    window: Option<isize>,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    max_value_bytes: Option<usize>,
    flat: bool,
    selector: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    observe::validate_max_value_bytes(max_value_bytes).map_err(invalid_input)?;
    let max_value_bytes = max_value_bytes.unwrap_or(observe::DEFAULT_MAX_VALUE_BYTES);
    let budget = tree_budget(depth, max_nodes)?;
    let tree = mechanism::tree_for_window_bounded(window, budget).map_err(map_mechanism_err)?;
    scoped_tree_payload(tree, depth, max_nodes, max_value_bytes, flat, selector)
}

fn scoped_tree_payload(
    tree: mechanism::A11yTree,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    max_value_bytes: usize,
    flat: bool,
    selector: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let all_flat = observe::flatten(&tree);
    let (scoped, nested_root) = if let Some(selector) = selector {
        // Parse first so malformed input remains invalid_input even if the
        // provider also hit a walk budget.
        observe::parse_selector(selector).map_err(invalid_input)?;
        if tree.truncated {
            return Err(CuError::new(
                "a11y_tree_truncated",
                "tree --selector requires a complete bounded window-root walk; increase --depth or --max-nodes",
            )
            .with_detail(serde_json::json!({
                "selector": selector,
                "budget": budget_json(depth, max_nodes),
                "visited": tree.visited,
                "returned": tree.returned,
            })));
        }
        let index = index_tree(&tree)?;
        let selected = resolve_tree_selector(&tree, &index, &tree.root_id, selector)?;
        let mut ids = subtree_ids(&index, &selected.id)?;
        let scoped = all_flat
            .iter()
            .filter(|entry| ids.contains(&entry.node.id))
            .collect::<Vec<_>>();
        let nested = if flat {
            None
        } else {
            let root = nested_subtree_node(&index, selected, &mut ids, max_value_bytes)?;
            if !ids.is_empty() {
                return Err(CuError::new(
                    "a11y_tree_invalid",
                    "accessibility subtree contains nodes unreachable from its selected root",
                ));
            }
            Some(root)
        };
        (scoped, nested)
    } else {
        (all_flat.iter().collect(), None)
    };
    let selected_root_id = scoped
        .first()
        .map(|entry| entry.node.id.as_str())
        .unwrap_or(tree.root_id.as_str());
    let selected_root_depth = scoped.first().map_or(0, |entry| entry.depth);
    let nodes = if flat {
        scoped
            .iter()
            .map(|entry| {
                let mut value = bounded_node_json(entry.node, max_value_bytes)?;
                let object = value.as_object_mut().expect("bounded node is an object");
                object.insert("index".into(), entry.index.into());
                object.insert(
                    "depth".into(),
                    entry.depth.saturating_sub(selected_root_depth).into(),
                );
                Ok(value)
            })
            .collect::<Result<Vec<_>, CuError>>()
            .map(serde_json::Value::Array)
    } else if selector.is_none() {
        scoped
            .iter()
            .map(|entry| bounded_node_json(entry.node, max_value_bytes))
            .collect::<Result<Vec<_>, CuError>>()
            .map(serde_json::Value::Array)
    } else {
        Ok(serde_json::Value::Null)
    }?;
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
        "max_value_bytes": max_value_bytes,
        "truncated": tree.truncated,
        "visited": tree.visited,
        "returned": scoped.len(),
        "ax": ax.as_str(),
        "next_actions": observe::empty_chrome_next_actions(ax, &app),
    });
    if flat || selector.is_none() {
        payload["nodes"] = nodes;
    } else if let Some(root) = nested_root {
        payload["root"] = root;
    }
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
    control: crate::execution_control::ExecutionControl<'_>,
) -> Result<serde_json::Value, CuError> {
    filter.validate().map_err(invalid_input)?;
    let watch = query_watch_bounds(watch_ms, until, interval_ms, max_events)?;
    // The one-shot path is unchanged: no cancellation point is introduced when
    // there is no watch, and nothing below the watch branch is consulted.
    let Some((watch_ms, interval_ms, max_events)) = watch else {
        return query_once_payload(
            window,
            depth,
            max_nodes,
            filter,
            text_and_text_exact,
            offset,
            max,
            selector,
        );
    };
    // The pre-effect check lives INSIDE `query_watch_with_providers`, before its
    // first authority call, so production and the injected test seam share the exact
    // same decision and a pre-cancelled watch issues zero focus reads and zero
    // samples.
    query_watch_with_providers(
        QueryWatchArgs {
            window,
            depth,
            max_nodes,
            filter,
            text_and_text_exact,
            offset,
            max,
            selector,
            watch_ms,
            interval_ms,
            max_events,
            until,
        },
        control,
        super::pointer::focused_window_identity,
        &|args| {
            query_once_payload(
                args.window,
                args.depth,
                args.max_nodes,
                args.filter.clone(),
                args.text_and_text_exact,
                args.offset,
                args.max,
                args.selector,
            )
        },
    )
}

/// The bounded watch request, so the loop and its provider closure can share one
/// description instead of repeating eight acquisition parameters.
struct QueryWatchArgs<'a> {
    window: isize,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    filter: observe::NodeFilter,
    text_and_text_exact: bool,
    offset: Option<usize>,
    max: Option<usize>,
    selector: Option<&'a str>,
    watch_ms: u64,
    interval_ms: u64,
    max_events: usize,
    until: Option<QueryWatchUntil>,
}

/// Everything the observation builder needs about ONE bounded watch, whether it
/// ended normally, on its deadline, or was cancelled. All three paths publish
/// through `build_observation`, so a cancelled watch cannot bypass the same field
/// set or invent a different one.
struct QueryWatchState {
    polls: usize,
    missing_samples: usize,
    dropped_events: usize,
    events: Vec<serde_json::Value>,
    condition_satisfied: bool,
    final_sample: serde_json::Value,
}

/// The bounded query-watch loop, GENERIC over its two providers and its identity.
///
/// The identity parameter `I` is what makes the bracketing rule testable without
/// exposing a native handle type: production infers the real private pointer
/// identity, while a test can inject a plain tuple, and equality of the bracketing
/// pair is the only operation the loop performs on it.
///
/// These are generic `Fn` parameters rather than trait objects or a type alias, so
/// a caller's closure can borrow its own locals and the higher-ranked borrow stays
/// intact. Production passes the real `focused_window_identity` and the real
/// `query_once_payload` acquisition, so the tested loop is the shipped loop.
fn query_watch_with_providers<F, S, I>(
    args: QueryWatchArgs<'_>,
    control: crate::execution_control::ExecutionControl<'_>,
    focus: F,
    acquire: &S,
) -> Result<serde_json::Value, CuError>
where
    F: Fn() -> Result<Option<I>, CuError>,
    S: Fn(&QueryWatchArgs<'_>) -> Result<serde_json::Value, CuError>,
    I: PartialEq,
{
    // PRE-FIRST-AUTHORITY: the ONLY direct `check_observe` in this verb, and the only
    // valid place for an `effect: not_performed` claim. It precedes the foreground
    // comparison and the baseline sample, so a pre-cancelled watch issues zero focus
    // reads and zero samples. Every later cancellation is a private signal handled
    // by the loop owner below.
    control.check_observe()?;
    let QueryWatchArgs {
        watch_ms,
        interval_ms,
        max_events,
        until,
        ..
    } = args;
    // The foreground identity is bracketed around the WHOLE observation, so a
    // cancelled observation can only claim `foreground_unchanged: true` after the
    // same revalidation a normal one performs.
    let focus_before = focus()?.ok_or_else(|| {
        CuError::new(
            "focused_window_unavailable",
            "query watch requires one uniquely resolved focused top-level window",
        )
    })?;
    let mut sample = (*acquire)(&args)?;
    let started = Instant::now();
    let deadline = started + Duration::from_millis(watch_ms);
    let mut events = Vec::new();
    let mut event_seq = 0u64;
    let mut dropped_events = 0usize;
    let mut missing_samples = 0usize;
    let mut polls = 1usize;
    let mut condition_satisfied = query_watch_satisfied(until, &sample, false);

    while !condition_satisfied && Instant::now() < deadline {
        // POST-BASELINE PAUSE: a valid cancellation POINT, but only a private
        // signal. The deadline is re-checked first, so a bound already reached stays
        // the authoritative outcome, and the pause is sliced so a long interval does
        // not delay the observation.
        let pause_deadline = Instant::now()
            + Duration::from_millis(interval_ms)
                .min(deadline.saturating_duration_since(Instant::now()));
        if control.sleep_until_cancelled(pause_deadline) {
            if Instant::now() >= deadline {
                break;
            }
            return query_watch_cancelled(
                QueryWatchState {
                    polls,
                    missing_samples,
                    dropped_events,
                    events,
                    condition_satisfied,
                    final_sample: sample,
                },
                &args,
                focus_before,
                &focus,
            );
        }
        if Instant::now() >= deadline {
            break;
        }
        // LAST-MOMENT CHECK before the next authority call, same private signal.
        if control.is_cancelled() {
            return query_watch_cancelled(
                QueryWatchState {
                    polls,
                    missing_samples,
                    dropped_events,
                    events,
                    condition_satisfied,
                    final_sample: sample,
                },
                &args,
                focus_before,
                &focus,
            );
        }
        polls += 1;
        let next = match (*acquire)(&args) {
            Ok(next) => next,
            // Unchanged policy: a transient later acquisition failure is COUNTED,
            // never turned into an empty sample nor promoted to a hard error. The
            // cancellation check sits at the loop boundary, not here, so a miss stays
            // indistinguishable from any other miss regardless of token timing.
            Err(_) => {
                missing_samples += 1;
                continue;
            }
        };
        // The sample is consumed UNCONDITIONALLY once it returns: this round's diff,
        // events, overflow count and condition update all happen before any later
        // token is consulted, so a token flipped inside the authority call cannot
        // discard the round.
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

    // FINAL FOREGROUND AUTHORITY: runs on the normal and timeout paths too, so its
    // failure outranks nothing here. On the cancelled path it runs BEFORE the
    // cancellation outcome is built, where it does outrank cancellation.
    query_watch_focus_after(focus_before, &focus)?;
    let observation = QueryWatchState {
        polls,
        missing_samples,
        dropped_events,
        events,
        condition_satisfied,
        final_sample: sample,
    }
    .build_observation(&args, false);
    if until.is_some() && !condition_satisfied {
        return Err(CuError::new(
            "query_watch_timeout",
            "query watch exhausted its deadline before the requested condition",
        )
        .with_detail(observation));
    }
    Ok(observation)
}

/// The post-baseline foreground revalidation, shared by the normal and cancelled
/// paths so both apply the same two named refusals.
fn query_watch_focus_after<F, I>(focus_before: I, focus: &F) -> Result<(), CuError>
where
    F: Fn() -> Result<Option<I>, CuError>,
    I: PartialEq,
{
    let focus_after = focus()?.ok_or_else(|| {
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
    Ok(())
}

/// The post-baseline cancellation outcome.
///
/// ORDERING IS THE POINT: the foreground comparison runs FIRST and its named
/// refusals win, because they are authoritative statements about whether the
/// observation is still attributable to one unchanged window, while cancellation
/// is only a request to stop. Publishing `foreground_unchanged: true` without that
/// revalidation would be a false attribution. Only afterwards is the SAME
/// observation object built and attached as structured detail.
fn query_watch_cancelled<F, I>(
    state: QueryWatchState,
    args: &QueryWatchArgs<'_>,
    focus_before: I,
    focus: &F,
) -> Result<serde_json::Value, CuError>
where
    F: Fn() -> Result<Option<I>, CuError>,
    I: PartialEq,
{
    query_watch_focus_after(focus_before, focus)?;
    let partial = state.build_observation(args, true);
    Err(CuError::new(
        "cancelled",
        "the query watch was cancelled after observation began",
    )
    .with_detail(serde_json::json!({
        "effect": "partially_performed",
        "phase": "observe_wait",
        "partial_observation": partial,
    })))
}

impl QueryWatchState {
    /// The ONE observation builder for the normal, timeout and cancelled paths.
    ///
    /// `cancelled` selects only the field whose truth depends on why the watch
    /// stopped: `timed_out` must be false for a cancellation, because a cancellation
    /// is not a timeout. Every other field keeps its existing name and meaning, and
    /// no new field is added, because the outer error code is already the
    /// termination carrier. `foreground_unchanged` is hardcoded true because every
    /// caller has already completed the foreground revalidation successfully.
    fn build_observation(self, args: &QueryWatchArgs<'_>, cancelled: bool) -> serde_json::Value {
        serde_json::json!({
            "mode": "poll-diff",
            "duration_ms": args.watch_ms,
            "interval_ms": args.interval_ms,
            "polls": self.polls,
            "missing_samples": self.missing_samples,
            "until": args.until,
            "condition_satisfied": self.condition_satisfied,
            "timed_out": !cancelled && args.until.is_some() && !self.condition_satisfied,
            "event_count": self.events.len(),
            "dropped_event_count": self.dropped_events,
            "truncated_events": self.dropped_events > 0,
            "foreground_unchanged": true,
            "events": self.events,
            "final": self.final_sample,
        })
    }
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
            "subrole": filter.subroles,
            "action": filter.actions,
            "min_depth": filter.min_depth,
            "max_depth": filter.max_depth,
            "text": filter.text,
            "text_exact": filter.text_exact,
            "identifier": filter.identifier,
            "actionable": filter.actionable,
            "enabled": filter.enabled,
            "focused": filter.focused,
            "selected": filter.selected,
            "checked": filter.checked,
            "expanded": filter.expanded,
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
                "actions": null,
                "bounds": null,
                "depth": null,
                "states": null,
                "text": null,
                "facts_complete": false,
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

fn observe_node_index(
    tree: &mechanism::A11yTree,
) -> std::collections::HashMap<&str, &mechanism::A11yNode> {
    tree.nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect()
}

fn project_observe_event_facts(
    event: &observe::ObserveEvent,
    previous: &std::collections::HashMap<&str, &mechanism::A11yNode>,
    current: &std::collections::HashMap<&str, &mechanism::A11yNode>,
    value: &mut serde_json::Value,
) {
    let id = event.node.get("id").and_then(serde_json::Value::as_str);
    let node = if event.notification == "Destroyed" {
        id.and_then(|id| previous.get(id).or_else(|| current.get(id)).copied())
    } else {
        id.and_then(|id| current.get(id).or_else(|| previous.get(id)).copied())
    };
    let Some(object) = value
        .get_mut("node")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    if let Some(node) = node {
        object.insert("actions".into(), serde_json::json!(node.actions));
        object.insert("bounds".into(), serde_json::json!(node.bounds));
        object.insert(
            "depth".into(),
            serde_json::json!(observe::node_depth(&node.id)),
        );
        object.insert("states".into(), serde_json::json!(node.states));
        object.insert("text".into(), serde_json::json!(node.text));
        object.insert("facts_complete".into(), serde_json::Value::Bool(true));
    } else {
        for name in ["actions", "bounds", "depth", "states", "text"] {
            object.insert(name.into(), serde_json::Value::Null);
        }
        object.insert("facts_complete".into(), serde_json::Value::Bool(false));
    }
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
        publish_ready_marker(path, window, &backend, "poll-diff")?;
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
        let previous_nodes = observe_node_index(&previous);
        let current_nodes = observe_node_index(&current);
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
            project_observe_event_facts(&event, &previous_nodes, &current_nodes, &mut value);
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

/// Read one schema-1 readiness marker. Partial or non-ready JSON returns
/// `None` so callers keep polling instead of treating a torn write as met.
pub(super) fn read_ready_marker(path: &str) -> Option<serde_json::Value> {
    let bytes = std::fs::read(path).ok()?;
    let marker = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    if marker.get("schema") != Some(&serde_json::json!(1)) {
        return None;
    }
    if marker.get("state") != Some(&serde_json::json!("ready")) {
        return None;
    }
    Some(marker)
}

/// Publish the point after which mutations are ordered after the complete
/// poll-diff baseline. The temporary file lives beside the destination and
/// is published with a same-filesystem hard link, so publication is atomic,
/// refuses to overwrite an existing caller marker, and a reader can never
/// mistake partial JSON for readiness. The caller owns marker cleanup.
pub(super) fn publish_ready_marker(
    path: &str,
    window: isize,
    backend: &str,
    mode: &str,
) -> Result<(), CuError> {
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
        "mode": mode,
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

    // ---- cooperative query-watch cancellation ---------------------------------

    /// A synthetic focused-window identity. The loop is generic over the identity
    /// type, so the test injects a plain tuple and never touches the private native
    /// type.
    type TestFocus = (isize, u32);

    const fn focus_id(handle: isize, process_id: u32) -> TestFocus {
        (handle, process_id)
    }

    fn watch_args(
        watch_ms: u64,
        interval_ms: u64,
        until: Option<QueryWatchUntil>,
    ) -> QueryWatchArgs<'static> {
        QueryWatchArgs {
            window: 7,
            depth: None,
            max_nodes: None,
            filter: observe::NodeFilter::from_parts(&[], None, None, None, false, None),
            text_and_text_exact: false,
            offset: None,
            max: None,
            selector: None,
            watch_ms,
            interval_ms,
            max_events: 8,
            until,
        }
    }

    /// An acquisition row shaped like the real filtered sample: `matched` drives
    /// `until: Present/Absent`, and an extra field drives diff detection.
    fn sample_json(matched: u64, marker: u64) -> serde_json::Value {
        serde_json::json!({
            "matched": matched,
            "scan_truncated": false,
            "nodes": [{ "id": "/0", "name": format!("n{marker}") }],
        })
    }

    #[test]
    fn pre_cancel_reads_no_focus_and_takes_no_sample() {
        // The injected providers count and PANIC, proving no authority is reachable
        // and that the shared pre-effect check lives inside the loop helper.
        let focus_calls = std::cell::Cell::new(0usize);
        let sample_calls = std::cell::Cell::new(0usize);
        let focus = || -> Result<Option<TestFocus>, CuError> {
            focus_calls.set(focus_calls.get() + 1);
            unreachable!("a pre-effect cancel must not read the foreground identity")
        };
        let acquire = |_: &QueryWatchArgs<'_>| -> Result<serde_json::Value, CuError> {
            sample_calls.set(sample_calls.get() + 1);
            unreachable!("a pre-effect cancel must not take a sample")
        };
        let error = query_watch_with_providers(
            watch_args(30_000, 50, Some(QueryWatchUntil::Present)),
            crate::execution_control::ExecutionControl::with_cancel_probe(&|| true),
            focus,
            &acquire,
        )
        .expect_err("a pre-effect cancel must refuse the watch");
        assert_eq!(error.code, "cancelled");
        assert_eq!(focus_calls.get(), 0, "no focus read on a pre-effect cancel");
        assert_eq!(sample_calls.get(), 0, "no sample on a pre-effect cancel");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "not_performed");
        assert!(
            detail.get("partial_observation").is_none(),
            "nothing was observed, so no partial may be claimed"
        );
    }

    #[test]
    fn a_post_baseline_pause_cancel_returns_a_truthful_partial_and_takes_no_second_sample() {
        // The token is raised from a test thread while the watch sits in a long
        // pause, i.e. AFTER the baseline exists. The outcome must be a partial
        // observation whose `timed_out` is false, because a cancellation is not a
        // timeout.
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        let token = Arc::new(AtomicBool::new(false));
        let raised = Arc::clone(&token);
        let focus_calls = std::cell::Cell::new(0usize);
        let samples = std::cell::Cell::new(0usize);
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            raised.store(true, Ordering::Release);
        });
        let probe = || token.load(Ordering::Acquire);
        let focus = || {
            focus_calls.set(focus_calls.get() + 1);
            Ok(Some(focus_id(7, 42)))
        };
        let acquire = |_: &QueryWatchArgs<'_>| -> Result<serde_json::Value, CuError> {
            samples.set(samples.get() + 1);
            Ok(sample_json(0, 1))
        };
        let error = query_watch_with_providers(
            watch_args(30_000, 2_000, Some(QueryWatchUntil::Present)),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            focus,
            &acquire,
        )
        .expect_err("a post-baseline cancel must refuse the watch");
        trigger.join().expect("cancel trigger");

        assert_eq!(error.code, "cancelled");
        assert_eq!(samples.get(), 1, "the token must stop the second sample");
        // Two focus reads: the bracketing pair a publishable observation requires.
        assert_eq!(focus_calls.get(), 2, "focus must be bracketed");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "partially_performed");
        assert_eq!(detail["phase"], "observe_wait");
        let partial = &detail["partial_observation"];
        assert_eq!(partial["mode"], "poll-diff");
        assert_eq!(partial["polls"], 1);
        assert_eq!(partial["missing_samples"], 0);
        assert_eq!(partial["event_count"], 0);
        assert_eq!(partial["dropped_event_count"], 0);
        assert_eq!(partial["truncated_events"], false);
        // A cancellation is NOT a timeout.
        assert_eq!(partial["timed_out"], false);
        assert_eq!(partial["condition_satisfied"], false);
        // Only true after the revalidation that just ran.
        assert_eq!(partial["foreground_unchanged"], true);
        assert_eq!(partial["duration_ms"], 30_000);
        assert_eq!(partial["interval_ms"], 2_000);
        // No parallel termination/completed vocabulary was introduced.
        assert!(partial.get("termination").is_none());
        assert!(partial.get("completed").is_none());
    }

    #[test]
    fn a_same_round_satisfied_condition_win_over_a_token_that_round_flipped() {
        // Round 2 flips the token AND returns a sample satisfying `until: Present`.
        // The normal matched result must win, with its events intact.
        let token = std::cell::Cell::new(false);
        let samples = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let focus = || Ok(Some(focus_id(7, 42)));
        let acquire = |_: &QueryWatchArgs<'_>| -> Result<serde_json::Value, CuError> {
            let n = samples.get() + 1;
            samples.set(n);
            let value = sample_json(if n == 1 { 0 } else { 1 }, n as u64);
            if n == 2 {
                token.set(true);
            }
            Ok(value)
        };
        let value = query_watch_with_providers(
            watch_args(30_000, 50, Some(QueryWatchUntil::Present)),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            focus,
            &acquire,
        )
        .expect("the same-round satisfied condition must win");
        assert!(token.get(), "the provider really did flip the token");
        assert_eq!(samples.get(), 2);
        assert_eq!(value["condition_satisfied"], true);
        assert_eq!(value["timed_out"], false);
        assert_eq!(value["foreground_unchanged"], true);
        assert!(
            value["event_count"].as_u64().unwrap_or(0) >= 1,
            "the same-round diff must be reported"
        );
        assert!(value.get("termination").is_none());
    }

    #[test]
    fn a_swallowed_sample_error_stays_swallowed_and_is_then_reported_as_a_partial() {
        // Round 2 flips the token and FAILS. Current policy counts the miss, never
        // promoting it to a provider failure; the token is then observed in the
        // following pause and surfaces as a partial cancellation.
        let token = std::cell::Cell::new(false);
        let samples = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let focus = || Ok(Some(focus_id(7, 42)));
        let acquire = |_: &QueryWatchArgs<'_>| -> Result<serde_json::Value, CuError> {
            let n = samples.get() + 1;
            samples.set(n);
            if n == 1 {
                return Ok(sample_json(0, 1));
            }
            token.set(true);
            Err(CuError::new(
                "query_watch_fixture_transient",
                "the fixture acquisition failed transiently",
            ))
        };
        let error = query_watch_with_providers(
            watch_args(30_000, 50, Some(QueryWatchUntil::Present)),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            focus,
            &acquire,
        )
        .expect_err("the pending token must surface as a partial cancellation");
        // NOT the fixture provider error: the miss stayed swallowed.
        assert_eq!(error.code, "cancelled");
        assert_eq!(samples.get(), 2);
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "partially_performed");
        let partial = &detail["partial_observation"];
        // The miss was COUNTED, not turned into an empty sample or a hard error.
        assert_eq!(partial["missing_samples"], 1);
        assert_eq!(partial["timed_out"], false);
        assert_eq!(partial["polls"], 2);
    }

    #[test]
    fn foreground_drift_wins_over_a_pending_cancellation() {
        // The token becomes pending after the baseline, but the foreground identity
        // changed. Drift is authoritative about attribution, so it must win and no
        // partial cancellation may be published.
        let token = std::cell::Cell::new(false);
        let focus_calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let focus = || {
            let n = focus_calls.get() + 1;
            focus_calls.set(n);
            if n == 1 {
                Ok(Some(focus_id(7, 42)))
            } else {
                // A different window took the foreground.
                Ok(Some(focus_id(9, 99)))
            }
        };
        let acquire = |_: &QueryWatchArgs<'_>| -> Result<serde_json::Value, CuError> {
            token.set(true);
            Ok(sample_json(0, 1))
        };
        let error = query_watch_with_providers(
            watch_args(30_000, 2_000, Some(QueryWatchUntil::Present)),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            focus,
            &acquire,
        )
        .expect_err("drift must win over the pending cancellation");
        assert!(token.get(), "the token really did become pending");
        assert_eq!(error.code, "focused_window_changed");
        assert_eq!(focus_calls.get(), 2, "the bracketing pair really ran");
        let detail = error.detail.unwrap_or(serde_json::Value::Null);
        assert_ne!(detail["effect"], "partially_performed");
    }

    #[test]
    fn an_uncancelled_watch_returns_the_existing_success_shape() {
        // The normal field set is unchanged: no termination, no completed.
        let focus = || Ok(Some(focus_id(7, 42)));
        let acquire = |_: &QueryWatchArgs<'_>| -> Result<serde_json::Value, CuError> {
            Ok(sample_json(1, 1))
        };
        let value = query_watch_with_providers(
            watch_args(30_000, 50, Some(QueryWatchUntil::Present)),
            crate::execution_control::ExecutionControl::none(),
            focus,
            &acquire,
        )
        .expect("a satisfied condition succeeds immediately");
        assert_eq!(value["mode"], "poll-diff");
        assert_eq!(value["polls"], 1);
        assert_eq!(value["condition_satisfied"], true);
        assert_eq!(value["timed_out"], false);
        assert_eq!(value["foreground_unchanged"], true);
        assert_eq!(value["missing_samples"], 0);
        assert_eq!(value["event_count"], 0);
        assert_eq!(value["dropped_event_count"], 0);
        assert_eq!(value["truncated_events"], false);
        assert!(value.get("termination").is_none());
        assert!(value.get("completed").is_none());
        assert!(value["final"].is_object());
    }

    #[test]
    fn a_same_round_deadline_win_over_a_token_that_round_flipped() {
        // Deterministic deadline precedence, using only publicly valid bounds (watch
        // at least 100ms, interval 50..=2000ms). The token starts false. Round 1 is
        // the baseline. Round 2 sleeps past the remaining watch deadline, THEN sets
        // the token and returns an unmatched but valid sample. The loop therefore
        // exits on the deadline, not on the token, and the ordinary timeout outcome
        // must win with `timed_out: true`.
        let token = std::cell::Cell::new(false);
        let samples = std::cell::Cell::new(0usize);
        let focus_calls = std::cell::Cell::new(0usize);
        let probe = || token.get();
        let focus = || {
            focus_calls.set(focus_calls.get() + 1);
            Ok(Some(focus_id(7, 42)))
        };
        let acquire = |_: &QueryWatchArgs<'_>| -> Result<serde_json::Value, CuError> {
            let n = samples.get() + 1;
            samples.set(n);
            if n == 2 {
                // Burn past the whole 150ms watch inside the authority call.
                std::thread::sleep(Duration::from_millis(200));
                token.set(true);
            }
            Ok(sample_json(0, n as u64))
        };
        let error = query_watch_with_providers(
            watch_args(150, 50, Some(QueryWatchUntil::Present)),
            crate::execution_control::ExecutionControl::with_cancel_probe(&probe),
            focus,
            &acquire,
        )
        .expect_err("an unsatisfied condition must time out");
        assert!(token.get(), "the provider really did flip the token");
        assert_eq!(samples.get(), 2, "round 2 really happened");
        assert_eq!(focus_calls.get(), 2, "focus was bracketed");
        assert_eq!(
            error.code, "query_watch_timeout",
            "the reached deadline must win over the token flipped in that round"
        );
        let detail = error.detail.expect("the timeout carries the observation");
        assert_eq!(detail["timed_out"], true);
        assert_eq!(detail["condition_satisfied"], false);
    }

    #[test]
    fn an_uncancelled_timeout_keeps_the_existing_error_detail_shape() {
        // The pre-existing timeout carrier is unchanged, and `timed_out` is true
        // there because it really is a timeout. Bounds are publicly valid.
        let focus = || Ok(Some(focus_id(7, 42)));
        let acquire = |_: &QueryWatchArgs<'_>| -> Result<serde_json::Value, CuError> {
            Ok(sample_json(0, 1))
        };
        let error = query_watch_with_providers(
            watch_args(120, 50, Some(QueryWatchUntil::Present)),
            crate::execution_control::ExecutionControl::none(),
            focus,
            &acquire,
        )
        .expect_err("an unsatisfied condition must time out");
        assert_eq!(error.code, "query_watch_timeout");
        let detail = error.detail.expect("the timeout carries the observation");
        assert_eq!(detail["timed_out"], true);
        assert_eq!(detail["condition_satisfied"], false);
        assert_eq!(detail["mode"], "poll-diff");
        assert_eq!(detail["foreground_unchanged"], true);
        assert!(detail.get("termination").is_none());
        assert!(detail.get("completed").is_none());
    }

    fn selector_tree() -> mechanism::A11yTree {
        let node =
            |id: &str, parent_id: Option<&str>, role: &str, name: &str| mechanism::A11yNode {
                id: id.into(),
                parent_id: parent_id.map(str::to_owned),
                role: role.into(),
                subrole: None,
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
            truncated: false,
            visited: 4,
            returned: 4,
        }
    }

    #[test]
    fn observe_event_facts_use_current_nodes_and_previous_nodes_for_destroyed_events() {
        let previous = selector_tree();
        let mut current = selector_tree();
        current.nodes[2].actions = vec!["press".into()];
        current.nodes[2].states = vec!["enabled".into(), "focused".into()];
        current.nodes[2].text = Some("current value".into());
        current.nodes[2].bounds.x = 41;
        let event = observe::ObserveEvent {
            notification: "StateChanged",
            node: serde_json::json!({"id":"/0/0/0","role":"button","name":"inside"}),
            field: Some("state"),
            before: serde_json::Value::Null,
            after: serde_json::Value::Null,
        };
        let mut value = serde_json::to_value(&event).expect("serialize event");
        project_observe_event_facts(
            &event,
            &observe_node_index(&previous),
            &observe_node_index(&current),
            &mut value,
        );
        assert_eq!(value["node"]["actions"], serde_json::json!(["press"]));
        assert_eq!(
            value["node"]["states"],
            serde_json::json!(["enabled", "focused"])
        );
        assert_eq!(value["node"]["bounds"]["x"], 41);
        assert_eq!(value["node"]["depth"], 2);
        assert_eq!(value["node"]["text"], "current value");
        assert_eq!(value["node"]["facts_complete"], true);

        let destroyed = observe::ObserveEvent {
            notification: "Destroyed",
            node: serde_json::json!({"id":"/0/0/0","role":"button","name":"inside"}),
            field: None,
            before: serde_json::Value::Null,
            after: serde_json::Value::Null,
        };
        current.nodes.remove(2);
        let mut value = serde_json::to_value(&destroyed).expect("serialize event");
        project_observe_event_facts(
            &destroyed,
            &observe_node_index(&previous),
            &observe_node_index(&current),
            &mut value,
        );
        assert_eq!(value["node"]["bounds"]["x"], 0);
        assert_eq!(value["node"]["actions"], serde_json::json!([]));
        assert_eq!(value["node"]["facts_complete"], true);
    }

    #[test]
    fn notification_events_publish_explicitly_unavailable_tree_facts() {
        let data = native_observe_payload(
            7,
            50,
            2,
            &["Created".into()],
            vec![mechanism::A11yEvent {
                notification: "Created".into(),
                node_id: "/0/1".into(),
                role: "button".into(),
                name: "new".into(),
                t_ms: 1,
            }],
        );
        assert_eq!(data["events"][0]["node"]["facts_complete"], false);
        assert!(data["events"][0]["node"]["actions"].is_null());
        assert!(data["events"][0]["node"]["bounds"].is_null());
        assert!(data["events"][0]["node"]["depth"].is_null());
    }

    #[test]
    fn tree_selector_returns_a_real_nested_subtree_and_a_flat_projection_on_request() {
        let nested = scoped_tree_payload(
            selector_tree(),
            Some(4),
            Some(20),
            4096,
            false,
            Some("Group[0]"),
        )
        .expect("selected nested subtree");
        assert_eq!(nested["selector"], "Group[0]");
        assert_eq!(nested["root_id"], "/0/0");
        assert_eq!(nested["returned"], 2);
        assert!(nested.get("nodes").is_none());
        assert_eq!(nested["root"]["id"], "/0/0");
        assert_eq!(nested["root"]["children"][0]["id"], "/0/0/0");
        assert_eq!(
            nested["root"]["children"][0]["children"],
            serde_json::json!([])
        );

        let payload = scoped_tree_payload(
            selector_tree(),
            Some(4),
            Some(20),
            4096,
            true,
            Some("Group[0]"),
        )
        .expect("selected subtree");
        assert_eq!(payload["selector"], "Group[0]");
        assert_eq!(payload["root_id"], "/0/0");
        assert_eq!(payload["returned"], 2);
        assert_eq!(payload["visited"], 4);
        assert_eq!(payload["truncated"], false);
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
    fn tree_value_budget_keeps_complete_length_and_digest_without_splitting_utf8() {
        let mut tree = selector_tree();
        tree.nodes[2].text = Some("hello".into());
        let bounded = scoped_tree_payload(tree.clone(), None, None, 3, true, None)
            .expect("bounded flat tree");
        let node = &bounded["nodes"][2];
        assert_eq!(node["text"], "hel");
        assert_eq!(node["value_bytes"], 5);
        assert_eq!(node["value_truncated"], true);
        assert_eq!(node["value_complete"], true);
        assert_eq!(
            node["value_sha256"],
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(bounded["max_value_bytes"], 3);

        tree.nodes[2].text = Some("你好".into());
        let metadata = scoped_tree_payload(tree.clone(), None, None, 0, false, None)
            .expect("metadata-only nested tree");
        assert_eq!(metadata["nodes"][2]["text"], "");
        assert_eq!(metadata["nodes"][2]["value_bytes"], 6);
        assert_eq!(metadata["nodes"][2]["value_truncated"], true);

        tree.nodes[2].states.push("text-truncated".into());
        let incomplete = scoped_tree_payload(tree, None, None, 3, false, None)
            .expect("adapter-truncated tree remains truthful");
        assert_eq!(incomplete["nodes"][2]["value_complete"], false);
        assert!(incomplete["nodes"][2].get("value_sha256").is_none());
    }

    #[test]
    fn tree_selector_requires_a_unique_unindexed_match() {
        let error = scoped_tree_payload(selector_tree(), None, None, 4096, false, Some("Group"))
            .expect_err("unindexed repeated role must be ambiguous");
        assert_eq!(error.code, "a11y_node_ambiguous");
        assert_eq!(error.count, Some(2));
        let payload =
            scoped_tree_payload(selector_tree(), None, None, 4096, false, Some("Group[1]"))
                .expect("explicit sibling index is deterministic");
        assert_eq!(payload["root"]["id"], "/0/1");
    }

    #[test]
    fn tree_selector_miss_and_incomplete_walk_are_typed_failures() {
        let error =
            scoped_tree_payload(selector_tree(), None, None, 4096, false, Some("Button[9]"))
                .expect_err("selector miss");
        assert_eq!(error.code, "a11y_node_not_found");

        let mut truncated = selector_tree();
        truncated.truncated = true;
        truncated.visited = 9;
        let error = scoped_tree_payload(truncated, Some(1), Some(4), 4096, false, Some("Group[0]"))
            .expect_err("a partial acquisition cannot prove a complete subtree");
        assert_eq!(error.code, "a11y_tree_truncated");
        assert_eq!(error.detail.as_ref().unwrap()["visited"], 9);
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
        publish_ready_marker(path.to_str().expect("UTF-8 path"), 41, "atspi", "poll-diff")
            .expect("publish marker");
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read marker"))
                .expect("parse marker");
        assert_eq!(marker["schema"], 1);
        assert_eq!(marker["state"], "ready");
        assert_eq!(marker["mode"], "poll-diff");
        assert_eq!(marker["window"], 41);
        assert_eq!(marker["backend"], "atspi");
        let error = publish_ready_marker(path.to_str().expect("UTF-8 path"), 42, "ax", "poll-diff")
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
            max_value_bytes: None,
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
            max_value_bytes: None,
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
                    subrole: Vec::new(),
                    action: Vec::new(),
                    min_depth: None,
                    max_depth: None,
                    text: text.map(str::to_owned),
                    text_exact: text_exact.map(str::to_owned),
                    identifier: None,
                    actionable: false,
                    enabled: None,
                    focused: None,
                    selected: None,
                    checked: None,
                    expanded: None,
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
            space: None,
            onscreen: None,
            occluded: None,
            all: false,
            meta: false,
            browser_profile: None,
            ax_meta: false,
            ax_role: None,
            ax_subrole: None,
            ax_identifier: None,
            ax_scan_max: None,
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
            subrole: None,
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
