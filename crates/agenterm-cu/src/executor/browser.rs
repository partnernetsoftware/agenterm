//! Browser-specific verbs and helpers: `unlock` (web-tree poke), `page js`
//! / `page targets` (CDP, optionally joined to one profile's tab titles),
//! `page text` (visible words), the a11y tab strip (`tab list` / `tab
//! select` / the gated `tab close`), and the browser-chrome guard the
//! focused text writers consult. Profiles (`browser profiles` / `browser
//! open`) live in `profiles.rs`.

use super::*;

/// `unlock`: read the window's tree, ask the owning application to build
/// its full accessibility tree, read the tree again, and report what
/// actually changed.
///
/// A browser engine leaves its web tree unbuilt until an assistive client
/// asks for it, so a walk of an idle Chromium or WebKit window returns
/// chrome and no page -- "empty chrome" is not an empty page. macOS spells
/// the request `AXManualAccessibility`.
///
/// **The poke's own status is not the outcome.** AppKit reports the
/// attribute as unsupported even when the poke lands (measured on a
/// WKWebView: three nodes before, fourteen after, the same AXError both
/// times), so this reads the tree again and reports `grew` from the node
/// counts. A host with no such mechanism reports `poked: false` with the
/// backend's own reason and still returns the classification, because
/// knowing the tree is empty chrome is useful either way.
/// Nodes that live below a `web-area` (Chromium / WebKit page content).
pub(super) fn web_content_nodes(tree: &mechanism::A11yTree) -> usize {
    let roots: Vec<String> = tree
        .nodes
        .iter()
        .filter(|node| observe::normalize_role(&node.role) == "webarea")
        .map(|node| format!("{}/", node.id))
        .collect();
    tree.nodes
        .iter()
        .filter(|node| roots.iter().any(|root| node.id.starts_with(root)))
        .count()
}

/// How many bounded re-reads `unlock` makes while the tree has not grown,
/// and how long it waits between them (at most ~1 s in total).
pub(super) const UNLOCK_REREADS: usize = 5;

pub(super) const UNLOCK_REREAD_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(200);

/// What `unlock` actually did on this host, for the reply's `poke` field.
///
/// macOS sets `AXManualAccessibility`; Linux flips the desktop-wide
/// `org.a11y.Status` flags (`IsEnabled` + `ScreenReaderEnabled` on the
/// session-bus name `org.a11y.Bus`) that a Chromium-family browser watches
/// before it builds a renderer tree; Windows has no separate poke, because
/// a Chromium process turns accessibility on when it answers `WM_GETOBJECT`
/// for its window and the UIA tree walk sends that itself.
pub(super) fn unlock_poke_description() -> &'static str {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "other"
    };
    poke_description_for(os)
}

/// The `poke` sentence for one OS name. Split out of
/// [`unlock_poke_description`] so every host's wording is checkable from
/// any host: `cfg!` alone would hide two thirds of it from the test run.
fn poke_description_for(os: &str) -> &'static str {
    match os {
        "macos" => {
            "AXManualAccessibility + AXEnhancedUserInterface on the application, AXManualAccessibility on the window, then a renderer wake (hit-test + children reads)"
        }
        "linux" => {
            "org.a11y.Status IsEnabled + ScreenReaderEnabled on the session-bus name org.a11y.Bus (the desktop-wide switch a Chromium renderer watches before it builds a web tree), then a re-read"
        }
        "windows" => {
            "no separate poke on Windows: a Chromium process enables accessibility when it answers WM_GETOBJECT for its window, which the UIA tree walk itself sends, so the walk is the poke"
        }
        _ => "no accessibility poke is mapped on this OS",
    }
}

pub(super) fn unlock_payload(window: isize) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "unlock requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    // The before / after reads must be able to see web content at all:
    // the old depth-12 read stopped above a Chromium web-area's children
    // (Brave: 68 nodes, `truncated: true`, both times), so `grew` was
    // false whether or not the poke landed. Read as deep and wide as
    // `page text` does.
    let budget = tree_budget(
        Some(crate::page_text::DEFAULT_DEPTH),
        Some(crate::page_text::DEFAULT_MAX_NODES),
    )?;
    let before =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let (poked, poke_reason) = match mechanism::poke_manual_accessibility(window) {
        Ok(()) => (true, None),
        Err(error) => {
            let reason = match &error {
                mechanism::MechanismError::Unsupported { reason } => reason.clone(),
                other => format!("{other:?}"),
            };
            (false, Some(reason))
        }
    };
    // The renderer bridges its tree asynchronously after the poke, so one
    // immediate re-read can miss it. Poll a few bounded rounds while the
    // tree has not grown; the last read is what is reported either way.
    let mut rereads = 0usize;
    let after = if poked {
        let mut after =
            mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
        rereads = 1;
        while after.returned <= before.returned
            && web_content_nodes(&after) <= web_content_nodes(&before)
            && rereads < UNLOCK_REREADS
        {
            std::thread::sleep(UNLOCK_REREAD_INTERVAL);
            after = mechanism::tree_for_window_bounded(Some(window), budget)
                .map_err(map_mechanism_err)?;
            rereads += 1;
        }
        after
    } else {
        before.clone()
    };
    let ax = observe::classify_ax_tree(&after);
    let app = window_app_name(Some(window));
    let web_before = web_content_nodes(&before);
    let web_after = web_content_nodes(&after);
    let mut payload = serde_json::json!({
        "ax": ax.as_str(),
        "poked": poked,
        "grew": after.returned > before.returned || web_after > web_before,
        "returned_before": before.returned,
        // Nodes below a web-area: the part of the tree the poke is for.
        "web_nodes_before": web_before,
        "web_nodes_after": web_after,
        "rereads": rereads,
        // The mechanism differs per host, so the reply must not describe
        // the macOS one everywhere: Linux flips the `org.a11y.Status`
        // session-bus switch a Chromium browser watches, and Windows has
        // no separate poke at all because the UIA walk itself sends
        // WM_GETOBJECT, which is what turns Chromium accessibility on.
        "poke": unlock_poke_description(),
        "next_actions": observe::empty_chrome_next_actions(ax, &app),
        "window": window,
        "visited": after.visited,
        "returned": after.returned,
        "truncated": after.truncated,
    });
    if let Some(reason) = poke_reason
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("reason".into(), serde_json::json!(reason));
    }
    Ok(payload)
}

pub(super) fn page_js_payload(
    expression: Option<&str>,
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
) -> Result<serde_json::Value, CuError> {
    let expression = expression.unwrap_or("");
    let port = port.unwrap_or(crate::cdp::DEFAULT_PORT);
    crate::cdp::evaluate(port, expression, &selector)
        .map_err(|error| CuError::new(error.code, error.message).with_detail(error.detail))
}

/// `page targets`: the CDP `/json` inventory, so a caller can pick a
/// `--target-id` for a background tab without touching the desktop.
///
/// With `browser_profile`, the inventory is joined to that profile's
/// windows: only the targets whose `title` equals (exactly) a tab title
/// read from the tab strip of a window whose `browser_profile` contains
/// the substring are kept, each marked `profile_match: "title"`. This is
/// a heuristic and the reply says so: one CDP port serves every profile
/// of an instance and a target carries no profile field, so a title
/// shared by two profiles' tabs is attributed to both, and a target whose
/// title the strip spells differently (memory-saver suffixes, an unset
/// document title) is left out.
pub(super) fn page_targets_payload(
    port: Option<u16>,
    browser_profile: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let port = port.unwrap_or(crate::cdp::DEFAULT_PORT);
    let Some(wanted) = browser_profile.map(str::trim).filter(|s| !s.is_empty()) else {
        if browser_profile.is_some() {
            return Err(invalid_input(
                "page targets --browser-profile must not be empty".into(),
            ));
        }
        return crate::cdp::targets(port)
            .map_err(|error| CuError::new(error.code, error.message).with_detail(error.detail));
    };
    // The windows first: a profile with no window is a typed miss before
    // any socket is opened.
    let wanted_lower = wanted.to_lowercase();
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let mut strips: Vec<(isize, String, Vec<String>)> = Vec::new();
    for window in windows
        .iter()
        .filter(|window| observe::looks_like_browser_app(&window.app_name))
    {
        let Some(profile) = window_browser_profile(window) else {
            continue;
        };
        if !profile.to_lowercase().contains(&wanted_lower) {
            continue;
        }
        let tree = mechanism::tree_for_window(Some(window.handle)).map_err(map_mechanism_err)?;
        let titles: Vec<String> = crate::tab_strip::tab_strip_entries(&tree)
            .iter()
            .map(|entry| entry.title().to_owned())
            .collect();
        strips.push((window.handle, profile, titles));
    }
    if strips.is_empty() {
        return Err(CuError::new(
            "browser_window_not_found",
            format!(
                "no browser window whose browser_profile contains {wanted:?} is in the inventory"
            ),
        )
        .with_detail(serde_json::json!({ "browser_profile": wanted })));
    }
    let list = crate::cdp::targets(port)
        .map_err(|error| CuError::new(error.code, error.message).with_detail(error.detail))?;
    let all: Vec<serde_json::Value> = list["targets"].as_array().cloned().unwrap_or_default();
    let mut matched = Vec::new();
    for target in &all {
        let title = target["title"].as_str().unwrap_or_default();
        for (handle, profile, titles) in &strips {
            if titles.iter().any(|tab| tab == title) {
                let mut row = target.clone();
                if let Some(object) = row.as_object_mut() {
                    object.insert("profile_match".into(), serde_json::json!("title"));
                    object.insert("window".into(), serde_json::json!(handle));
                    object.insert("browser_profile".into(), serde_json::json!(profile));
                }
                matched.push(row);
            }
        }
    }
    let pages = matched.iter().filter(|row| row["type"] == "page").count();
    Ok(serde_json::json!({
        "backend": list["backend"],
        "port": port,
        "via": "/json + a11y tab strip",
        "browser_profile": wanted,
        "profile_match": "title",
        "heuristic": "CDP targets carry no profile field (one port serves every profile of the instance); a target is attributed to a profile only when its title equals a tab title of that profile's window exactly, so a shared title matches every such window and a differently spelled title is left out",
        "windows": strips.iter().map(|(handle, profile, titles)| serde_json::json!({
            "handle": handle,
            "browser_profile": profile,
            "tabs": titles.len(),
        })).collect::<Vec<_>>(),
        "total": all.len(),
        "returned": matched.len(),
        "pages": pages,
        "targets": matched,
    }))
}

/// What a caller should do when the platform walk stopped at its budget.
/// The macOS walk is breadth-first: with the platform's own 1000-node /
/// 32-level defaults the budget is spent on browser chrome (tab strip,
/// toolbar, bookmarks) before deep web content is reached, so "truncated"
/// on a browser window usually means "the page is not in this reply".
pub(super) fn truncation_next_actions(tree: &mechanism::A11yTree, verb: &str) -> Vec<String> {
    if !tree.truncated {
        return Vec::new();
    }
    vec![
        format!(
            "the walk stopped at its budget after {} nodes (breadth-first: deep web content is what gets cut); rerun {verb} with --max-nodes {} --depth {} (limits {} / {}), or narrow with --within X,Y,W,H / --selector",
            tree.visited,
            crate::page_text::DEFAULT_MAX_NODES,
            observe::MAX_DEPTH_BUDGET,
            observe::MAX_NODE_BUDGET,
            observe::MAX_DEPTH_BUDGET,
        ),
        "page text --window HANDLE reads the visible words with those larger defaults; never a screenshot".to_owned(),
    ]
}

/// The mutually exclusive CDP target flags as one selector.
pub(super) fn cdp_selector(
    id: &Option<String>,
    url: &Option<String>,
    title: &Option<String>,
    match_any: &Option<String>,
) -> crate::cdp::TargetSelector {
    crate::cdp::TargetSelector {
        id: id.clone(),
        url: url.clone(),
        title: title.clone(),
        match_any: match_any.clone(),
    }
}

/// Resolve the selector on `port` and open one session to that target.
/// Nothing here activates a tab or a window.
fn cdp_connect(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
) -> Result<
    (
        crate::cdp::page::Ctx,
        crate::cdp::Session<crate::cdp::ws::TcpTransport>,
    ),
    CuError,
> {
    let port = port.unwrap_or(crate::cdp::DEFAULT_PORT);
    let (target, session) = crate::cdp::targets::connect_target(port, &selector)?;
    Ok((
        crate::cdp::page::Ctx {
            port,
            target,
            selector,
        },
        session,
    ))
}

/// `page text`: visible words in reading order, each with the node that
/// carries them, so the caller's next step is `invoke --node` /
/// `click --node`. The walk budget defaults to depth 64 / 6000 nodes.
///
/// Two backends, one row shape: with `--window` the a11y tree of that
/// window (the active tab's web-area on macOS Chromium); with a CDP
/// selector (`--target-*` / `--port`) the `Accessibility.getFullAXTree`
/// of that page target, which reaches a background tab in a background
/// window without touching what is active (`backend: "cdp"`).
pub(super) fn page_text_payload(
    window: Option<isize>,
    max_bytes: Option<usize>,
    within: Option<[i32; 4]>,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
) -> Result<serde_json::Value, CuError> {
    let cdp = port.is_some() || !selector.is_empty();
    let window = match (window, cdp) {
        (Some(_), true) => {
            return Err(invalid_input(
                "page text reads one backend per call: --window HANDLE (a11y tree) or a CDP target (--target-id | --target-url | --target-title | --match [--port N]), not both".into(),
            ));
        }
        (None, false) => {
            return Err(invalid_input(
                "page text needs --window HANDLE (a11y tree of the active tab) or a CDP target (--target-id | --target-url | --target-title | --match [--port N]; reaches background tabs)".into(),
            ));
        }
        (None, true) => {
            if within.is_some() || depth.is_some() || max_nodes.is_some() {
                return Err(invalid_input(
                    "page text over CDP takes only --max-bytes; --within / --depth / --max-nodes are a11y-walk budgets".into(),
                ));
            }
            let max_bytes =
                crate::page_text::validate_max_bytes(max_bytes).map_err(invalid_input)?;
            let (ctx, mut session) = cdp_connect(port, selector)?;
            return crate::cdp::page::text(&mut session, &ctx, max_bytes).map_err(CuError::from);
        }
        (Some(window), false) => window,
    };
    if window == 0 {
        return Err(invalid_input(
            "page text requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    let max_bytes = crate::page_text::validate_max_bytes(max_bytes).map_err(invalid_input)?;
    let depth = Some(depth.unwrap_or(crate::page_text::DEFAULT_DEPTH));
    let max_nodes = Some(max_nodes.unwrap_or(crate::page_text::DEFAULT_MAX_NODES));
    let budget = tree_budget(depth, max_nodes)?;
    let tree =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let reading = crate::page_text::read(&tree, within, max_bytes);
    let ax = observe::classify_ax_tree(&tree);
    let mut next_actions = observe::empty_chrome_next_actions(ax, &window_app_name(Some(window)));
    next_actions.extend(truncation_next_actions(&tree, "page text"));
    if reading.rows.is_empty() && tree.truncated {
        next_actions.push(
            "no visible words inside the budget; raise --max-nodes / --depth before concluding the page is empty"
                .to_owned(),
        );
    }
    if reading.truncated {
        next_actions.push(format!(
            "text cut at --max-bytes {max_bytes}; raise it (<= {}) or narrow with --within",
            crate::page_text::MAX_MAX_BYTES
        ));
    }
    Ok(serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "order": "reading (child-index path, document order)",
        "budget": budget_json(depth, max_nodes),
        "within": within,
        "max_bytes": max_bytes,
        "visited": tree.visited,
        "tree_truncated": tree.truncated,
        "candidates": reading.candidates,
        "merged": reading.merged,
        "returned": reading.rows.len(),
        "bytes": reading.bytes,
        "truncated": reading.truncated,
        "ax": ax.as_str(),
        "next_actions": next_actions,
        "rows": reading.rows.iter().map(crate::page_text::TextRow::json).collect::<Vec<_>>(),
    }))
}

// ---------------------------------------------------------------------------
// CDP background-tab verbs (`page find` / `page click` / `page fill` /
// `page nav` / `page screenshot`). Every one of them addresses a page
// target through the same selector, runs on that target's own websocket,
// and never activates a tab or a window; the actuators reserve a receipt
// between the read-only plan and the dispatch (receipt window 0: a CDP
// target is not a window handle).
// ---------------------------------------------------------------------------

/// Exactly one of `--selector` / `--text` / `--role [--name]` / `--node`.
pub(super) fn cdp_node_query(
    verb: &str,
    selector: Option<&str>,
    text: Option<&str>,
    role: Option<&str>,
    name: Option<&str>,
    node: Option<u64>,
) -> Result<crate::cdp::page::NodeQuery, CuError> {
    use crate::cdp::page::NodeQuery;
    let given = usize::from(selector.is_some())
        + usize::from(text.is_some())
        + usize::from(role.is_some())
        + usize::from(node.is_some());
    if given != 1 {
        return Err(invalid_input(format!(
            "{verb} names one node with exactly one of --selector CSS | --text SUB | --role R [--name SUB] | --node ID (got {given})"
        )));
    }
    if name.is_some() && role.is_none() {
        return Err(invalid_input(format!(
            "{verb} --name SUB only narrows --role R"
        )));
    }
    for (flag, value) in [
        ("--selector", selector),
        ("--text", text),
        ("--role", role),
        ("--name", name),
    ] {
        if value.is_some_and(|value| value.trim().is_empty()) {
            return Err(invalid_input(format!("{verb} {flag} must not be empty")));
        }
    }
    Ok(if let Some(css) = selector {
        NodeQuery::Css(css.to_owned())
    } else if let Some(text) = text {
        NodeQuery::Text(text.to_owned())
    } else if let Some(role) = role {
        NodeQuery::Role {
            role: role.to_owned(),
            name: name.map(str::to_owned),
        }
    } else {
        NodeQuery::Node(node.unwrap_or_default())
    })
}

pub(super) fn page_find_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    css: Option<&str>,
    text: Option<&str>,
    role: Option<&str>,
    name: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let query = cdp_node_query("page find", css, text, role, name, None)?;
    let (ctx, mut session) = cdp_connect(port, selector)?;
    crate::cdp::page::find(&mut session, &ctx, &query).map_err(CuError::from)
}

/// Close a CDP actuation receipt from its outcome (or its failure) and
/// attach the ticket to the reply.
fn complete_cdp_receipt(
    receipts: &mut ReceiptLog,
    ticket: &crate::receipt::ReceiptTicket,
    verb: &str,
    outcome: Result<crate::cdp::page::ActuationOutcome, crate::cdp::CdpError>,
) -> Result<serde_json::Value, CuError> {
    match outcome {
        Ok(outcome) => {
            receipts.complete(
                ticket,
                verb,
                0,
                outcome.verified,
                serde_json::json!({
                    "performed": outcome.performed,
                    "verification": outcome.payload["verification"],
                    "after": outcome.payload["after"],
                }),
            )?;
            let mut payload = outcome.payload;
            payload["receipt"] = ticket.json();
            Ok(payload)
        }
        Err(error) => {
            let error = CuError::from(error);
            receipts.complete(
                ticket,
                verb,
                0,
                false,
                serde_json::json!({
                    "performed": false,
                    "error": error_payload(&error),
                }),
            )?;
            Err(error.with_detail(serde_json::json!({ "receipt": ticket.json() })))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn page_click_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    css: Option<&str>,
    text: Option<&str>,
    node: Option<u64>,
    button: Option<&str>,
    clicks: Option<u32>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let query = cdp_node_query("page click", css, text, None, None, node)?;
    let button = button.unwrap_or("left");
    let clicks = clicks.unwrap_or(1);
    let (ctx, mut session) = cdp_connect(port, selector)?;
    let plan = crate::cdp::page::plan_click(&mut session, &query, button, clicks)?;
    let ticket = receipts.reserve(
        "page-click",
        0,
        serde_json::json!({
            "cdp_target": ctx.target.identity_json(),
            "query": query.json(),
            "action": "click",
            "plan": plan.json(),
        }),
    )?;
    let outcome = crate::cdp::page::perform_click(&mut session, &ctx, &plan);
    complete_cdp_receipt(receipts, &ticket, "page-click", outcome)
}

pub(super) fn page_hover_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    x: f64,
    y: f64,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let (ctx, mut session) = cdp_connect(port, selector)?;
    let plan = crate::cdp::page::plan_point(&mut session, x, y)?;
    let ticket = receipts.reserve(
        "page-hover",
        0,
        serde_json::json!({
            "cdp_target": ctx.target.identity_json(),
            "action": "hover",
            "plan": plan.json(),
        }),
    )?;
    let outcome = crate::cdp::page::perform_hover(&mut session, &ctx, &plan);
    complete_cdp_receipt(receipts, &ticket, "page-hover", outcome)
}

pub(super) fn page_scroll_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    x: f64,
    y: f64,
    delta_x: f64,
    delta_y: f64,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    crate::cdp::page::validate_scroll_delta("page scroll --dx", delta_x)
        .and_then(|_| crate::cdp::page::validate_scroll_delta("page scroll --dy", delta_y))
        .map_err(invalid_input)?;
    let (ctx, mut session) = cdp_connect(port, selector)?;
    let plan = crate::cdp::page::plan_point(&mut session, x, y)?;
    let ticket = receipts.reserve(
        "page-scroll",
        0,
        serde_json::json!({
            "cdp_target": ctx.target.identity_json(),
            "action": "scroll",
            "delta": { "x": delta_x, "y": delta_y },
            "plan": plan.json(),
        }),
    )?;
    let outcome = crate::cdp::page::perform_scroll(&mut session, &ctx, &plan, delta_x, delta_y);
    complete_cdp_receipt(receipts, &ticket, "page-scroll", outcome)
}

pub(super) fn page_drag_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let (ctx, mut session) = cdp_connect(port, selector)?;
    let plan = crate::cdp::page::plan_drag(&mut session, x1, y1, x2, y2)?;
    let ticket = receipts.reserve(
        "page-drag",
        0,
        serde_json::json!({
            "cdp_target": ctx.target.identity_json(),
            "action": "drag",
            "plan": plan.json(),
        }),
    )?;
    let outcome = crate::cdp::page::perform_drag(&mut session, &ctx, &plan);
    complete_cdp_receipt(receipts, &ticket, "page-drag", outcome)
}

pub(super) fn page_dialog_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    dismiss: bool,
    text: Option<&str>,
    wait_ms: Option<u64>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let (ctx, mut session) = cdp_connect(port, selector)?;
    let plan = crate::cdp::page::plan_dialog(&mut session, !dismiss, text, wait_ms)?;
    let ticket = receipts.reserve(
        "page-dialog",
        0,
        serde_json::json!({
            "cdp_target": ctx.target.identity_json(),
            "action": "dialog",
            "plan": plan.json(),
        }),
    )?;
    let outcome = crate::cdp::page::perform_dialog(&mut session, &ctx, &plan);
    complete_cdp_receipt(receipts, &ticket, "page-dialog", outcome)
}

pub(super) fn page_files_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    css: Option<&str>,
    node: Option<u64>,
    files: &[String],
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let query = cdp_node_query("page files", css, None, None, None, node)?;
    let (ctx, mut session) = cdp_connect(port, selector)?;
    let plan = crate::cdp::page::plan_files(&mut session, &query, files)?;
    let ticket = receipts.reserve(
        "page-files",
        0,
        serde_json::json!({
            "cdp_target": ctx.target.identity_json(),
            "query": query.json(),
            "action": "files",
            "plan": plan.json(),
        }),
    )?;
    let outcome = crate::cdp::page::perform_files(&mut session, &ctx, &plan);
    complete_cdp_receipt(receipts, &ticket, "page-files", outcome)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn page_fill_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    css: Option<&str>,
    node: Option<u64>,
    text: &str,
    clear: bool,
    submit: bool,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let query = cdp_node_query("page fill", css, None, None, None, node)?;
    let (ctx, mut session) = cdp_connect(port, selector)?;
    let plan = crate::cdp::page::plan_fill(&mut session, &query, text, clear, submit)?;
    let ticket = receipts.reserve(
        "page-fill",
        0,
        serde_json::json!({
            "cdp_target": ctx.target.identity_json(),
            "query": query.json(),
            "action": "fill",
            "plan": plan.json(),
        }),
    )?;
    let outcome = crate::cdp::page::perform_fill(&mut session, &ctx, &plan);
    complete_cdp_receipt(receipts, &ticket, "page-fill", outcome)
}

pub(super) fn page_nav_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    url: &str,
    wait_ms: Option<u64>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    crate::cdp::page::validate_nav_url(url).map_err(invalid_input)?;
    crate::cdp::page::validate_nav_wait(wait_ms).map_err(invalid_input)?;
    let (ctx, mut session) = cdp_connect(port, selector)?;
    let plan = crate::cdp::page::plan_nav(&mut session, url, wait_ms)?;
    let ticket = receipts.reserve(
        "page-nav",
        0,
        serde_json::json!({
            "cdp_target": ctx.target.identity_json(),
            "action": "nav",
            "plan": plan.json(),
        }),
    )?;
    let outcome = crate::cdp::page::perform_nav(&mut session, &ctx, &plan);
    complete_cdp_receipt(receipts, &ticket, "page-nav", outcome)
}

pub(super) fn page_screenshot_payload(
    port: Option<u16>,
    selector: crate::cdp::TargetSelector,
    out: &str,
    replace: bool,
    activate: bool,
    receipts: Option<&mut ReceiptLog>,
) -> Result<serde_json::Value, CuError> {
    if out.trim().is_empty() || out.contains('\0') {
        return Err(invalid_input(
            "page screenshot requires --out PATH (a writable PNG path)".into(),
        ));
    }
    if !replace && std::path::Path::new(out).exists() {
        return Err(invalid_input(format!(
            "page screenshot --out {out}: the file exists; pass --replace to overwrite it"
        )));
    }
    let (ctx, mut session) = cdp_connect(port, selector)?;
    let ticket = match receipts {
        Some(receipts) if activate => Some((
            receipts.reserve(
                "page-screenshot",
                0,
                serde_json::json!({
                    "cdp_target": ctx.target.identity_json(),
                    "action": "bring-to-front",
                    "out": out,
                }),
            )?,
            receipts,
        )),
        _ => None,
    };
    let captured = crate::cdp::page::screenshot(&mut session, &ctx, activate);
    let (bytes, mut meta) = match captured {
        Ok(captured) => captured,
        Err(error) => {
            let error = CuError::from(error);
            if let Some((ticket, receipts)) = ticket {
                receipts.complete(
                    &ticket,
                    "page-screenshot",
                    0,
                    false,
                    serde_json::json!({ "performed": false, "error": error_payload(&error) }),
                )?;
            }
            return Err(error);
        }
    };
    let sha256 = clipboard_sha256_hex(&bytes);
    let written = write_png(out, &bytes, replace);
    if let Some((ticket, receipts)) = ticket {
        receipts.complete(
            &ticket,
            "page-screenshot",
            0,
            written.is_ok(),
            serde_json::json!({ "performed": true, "out": out, "bytes": bytes.len(), "sha256": sha256 }),
        )?;
        meta["receipt"] = ticket.json();
    }
    written?;
    meta["out"] = serde_json::json!(out);
    meta["sha256"] = serde_json::json!(sha256);
    Ok(meta)
}

fn write_png(path: &str, bytes: &[u8], replace: bool) -> Result<(), CuError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if replace {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(path).map_err(|error| {
        CuError::new(
            "screenshot_write_failed",
            format!("page screenshot --out {path}: {error}"),
        )
    })?;
    use std::io::Write;
    file.write_all(bytes)
        .and_then(|_| file.flush())
        .map_err(|error| {
            CuError::new(
                "screenshot_write_failed",
                format!("page screenshot --out {path}: {error}"),
            )
        })
}

// ---------------------------------------------------------------------------
// Browser tab strip through the a11y tree (`tab list` / `tab select`): the
// fallback for choosing a tab when no CDP port is open. The tree read is
// the same bounded walk `tree` uses; the press is the same `AXPress` path
// `invoke press` uses. Nothing here raises or activates the window.
// ---------------------------------------------------------------------------

pub(super) fn tab_window_arg(verb: &str, window: isize) -> Result<(), CuError> {
    if window == 0 {
        return Err(invalid_input(format!(
            "{verb} requires --window <handle> (a non-zero handle from `windows`)"
        )));
    }
    Ok(())
}

pub(super) fn tab_rows(tree: &mechanism::A11yTree) -> Vec<serde_json::Value> {
    crate::tab_strip::tab_strip_entries(tree)
        .iter()
        .map(crate::tab_strip::TabEntry::json)
        .collect()
}

pub(super) fn tab_list_payload(window: isize) -> Result<serde_json::Value, CuError> {
    tab_window_arg("tab list", window)?;
    let tree = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let entries = crate::tab_strip::tab_strip_entries(&tree);
    let selected: Vec<usize> = entries
        .iter()
        .filter(|entry| entry.selected() == observe::Tri::True)
        .map(|entry| entry.index)
        .collect();
    Ok(serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "returned": entries.len(),
        "selected": selected,
        "tabs": entries.iter().map(crate::tab_strip::TabEntry::json).collect::<Vec<_>>(),
        "visited": tree.visited,
        "truncated": tree.truncated,
        "note": if entries.is_empty() {
            "no tab strip in this window's tree; Chromium lists tabs as tab-group radio-buttons only when the strip is rendered"
        } else {
            "background tabs have no web-area in the tree; select one to read it, or use page-js --target-title through CDP"
        },
    }))
}

pub(super) fn tab_match_error(
    error: crate::tab_strip::TabMatchError,
    rows: &[serde_json::Value],
) -> CuError {
    match error {
        crate::tab_strip::TabMatchError::NotFound { reason, message } => {
            CuError::new("a11y_tab_not_found", message).with_detail(serde_json::json!({
                "reason": reason,
                "candidates": rows,
            }))
        }
        crate::tab_strip::TabMatchError::Ambiguous { count, message } => {
            CuError::new("a11y_tab_ambiguous", message)
                .with_count(count)
                .with_detail(serde_json::json!({
                    "matches": count,
                    "candidates": rows,
                }))
        }
    }
}

/// Press one tab-strip row in the background and read `selected` back.
/// An already-selected tab is a verified no-op (nothing pressed).
pub(super) fn tab_select_payload(
    window: isize,
    title: Option<&str>,
    index: Option<usize>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    tab_window_arg("tab select", window)?;
    let spec = crate::tab_strip::TabSpec::from_parts(title, index).map_err(invalid_input)?;
    let before = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let entries = crate::tab_strip::tab_strip_entries(&before);
    let rows_before: Vec<serde_json::Value> = entries
        .iter()
        .map(crate::tab_strip::TabEntry::json)
        .collect();
    let hit = crate::tab_strip::match_tab(&entries, &spec)
        .map_err(|error| tab_match_error(error, &rows_before))?;
    let target = hit.node.clone();
    let tab_json = hit.json();
    // The same honesty rule as `invoke press`: where the backend publishes
    // action names, a row that offers no click is refused before anything
    // is touched; AT-SPI skips actions during the walk and judges at press.
    let backend_publishes_actions = before.backend != "at-spi2";
    if backend_publishes_actions
        && !target
            .actions
            .iter()
            .any(|offered| offered.eq_ignore_ascii_case("click"))
    {
        return Err(CuError::new(
            "unsupported",
            format!(
                "tab {} ({:?}) does not offer press (actions: {})",
                hit.index,
                target.name,
                if target.actions.is_empty() {
                    "none".to_owned()
                } else {
                    target.actions.join(", ")
                }
            ),
        )
        .with_detail(serde_json::json!({
            "reason": "node_action_missing",
            "required": "click",
            "offered": target.actions,
        })));
    }
    let already = hit.selected() == observe::Tri::True;
    let performed = !already;
    let ticket = receipts.reserve(
        "tab-select",
        window,
        serde_json::json!({
            "spec": spec.json(),
            "tab": tab_json,
            "node": observe::node_state_json(&target),
            "action": "press",
            "performed": performed,
            "before": rows_before,
        }),
    )?;
    let mut mechanism_error = None;
    if performed
        && let Err(error) =
            mechanism::perform_node_action(Some(window), &target.id, mechanism::NodeAction::Press)
    {
        mechanism_error = Some(map_mechanism_err(error));
    }
    let after = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let after_node = observe::node_by_id(&after, &target.id).cloned();
    let rows_after = tab_rows(&after);
    let (verified, reason) = match &after_node {
        Some(now) => match observe::selected_state(now) {
            observe::Tri::True => (true, None),
            observe::Tri::False => (false, Some("still_unselected")),
            _ => (false, Some("selected_unobservable")),
        },
        None => (false, Some("node_gone")),
    };
    let verified = verified && mechanism_error.is_none();
    let verification = serde_json::json!({
        "method": "selected-readback",
        "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { reason },
    });
    let after_state = after_node.as_ref().map(observe::node_state_json);
    let receipt = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": before.backend,
        "window": window,
        "target": spec.json(),
        "tab": tab_json,
        "node": observe::node_state_json(&target),
        "action": "press",
        "via": "tab-select",
        "performed": performed,
        "verified": verified,
        "verification": verification,
        "focus_changed": false,
        "before": rows_before,
        "after": rows_after,
        "after_node": after_state,
        "tree_changed": observe::tree_changed(&before, &after),
        "receipt": ticket.json(),
    });
    receipts.complete(
        &ticket,
        "tab-select",
        window,
        verified,
        serde_json::json!({
            "after": rows_after,
            "verification": verification,
            "error": mechanism_error.as_ref().map(error_payload),
        }),
    )?;
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(serde_json::json!({ "receipt": receipt })));
    }
    Ok(receipt)
}

// ---------------------------------------------------------------------------
// `tab close`: the destructive tab verb. Gated like `close` (exact
// identity, prior snapshot, checkable postcondition), performed through
// the tab row's own close button in the tree -- never a keyboard shortcut
// -- and verified by reading the strip back without that title.
// ---------------------------------------------------------------------------

/// How long the tab-strip read-back polls after the close press.
pub(super) const TAB_CLOSE_READBACK: Duration = Duration::from_millis(2_500);

pub(super) const TAB_CLOSE_READBACK_POLL: Duration = Duration::from_millis(50);

/// The three-part gate for `tab close`: a target (`--window H` plus
/// `--title T --exact` or `--index N`), the strip snapshot the receipt
/// always carries, and `--expect gone` (postcondition). Every missing
/// part is named in one refusal; nothing is read before it passes.
pub(super) fn tab_close_gate(
    window: isize,
    title: Option<&str>,
    index: Option<usize>,
    exact: bool,
    expect: Option<&str>,
) -> Result<(), CuError> {
    match expect {
        Some("gone") | None => {}
        Some(other) => {
            return Err(invalid_input(format!(
                "tab close --expect accepts only 'gone' (one fewer strip row with the tab's title is read back), got {other:?}"
            )));
        }
    }
    let has_title = title.is_some_and(|title| !title.trim().is_empty());
    if has_title && index.is_some() {
        return Err(invalid_input(
            "tab close takes --title T --exact or --index N, not both".into(),
        ));
    }
    let mut missing = Vec::new();
    if window == 0 {
        missing.push("target");
    }
    if !has_title && index.is_none() {
        missing.push("selector");
    }
    if has_title && !exact {
        missing.push("exact");
    }
    if expect.is_none() {
        missing.push("postcondition");
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(CuError::new(
        "refused",
        "tab close is destructive: it needs an exact tab (--window HANDLE with --title T --exact or \
         --index N) and a checkable postcondition (--expect gone); the strip snapshot is written to \
         the receipt before the press; nothing was performed",
    )
    .with_detail(serde_json::json!({
        "reason": "destructive_gate",
        "missing": missing,
        "required": {
            "target": "--window HANDLE",
            "selector": "--title T --exact | --index N",
            "snapshot": "tab strip rows (always written to the receipt)",
            "postcondition": "--expect gone",
        },
        "effect": "not_performed",
    })))
}

/// The close control of one tab row: a `button` child of the row that
/// offers a click. macOS Chromium exposes it on the active tab (and on a
/// hovered one); a background tab's row has no such child.
pub(super) fn tab_close_button<'a>(
    tree: &'a mechanism::A11yTree,
    tab_id: &str,
) -> Option<&'a mechanism::A11yNode> {
    tree.nodes.iter().find(|node| {
        node.parent_id.as_deref() == Some(tab_id)
            && observe::normalize_role(&node.role) == "button"
            && (tree.backend == "at-spi2"
                || node
                    .actions
                    .iter()
                    .any(|offered| offered.eq_ignore_ascii_case("click")))
    })
}

/// What the a11y close path has to do for one strip entry, decided from
/// one tree read: press the row's button directly, or -- a background tab
/// with no button -- select the row first (in the window; nothing is
/// raised), close, and press the previously selected row again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TabClosePlan {
    pub target_id: String,
    pub target_index: usize,
    pub target_selected: bool,
    pub button_id: Option<String>,
    /// `(index, title)` of the selected row when it is not the target.
    pub previously_selected: Option<(usize, String)>,
    pub select_first: bool,
}

impl TabClosePlan {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "button": self.button_id,
            "select_first": self.select_first,
            "restore_to": self.previously_selected.as_ref().map(|(index, title)| {
                serde_json::json!({ "index": index, "title": title })
            }),
        })
    }
}

pub(super) fn tab_close_plan(
    tree: &mechanism::A11yTree,
    entries: &[crate::tab_strip::TabEntry<'_>],
    hit: &crate::tab_strip::TabEntry<'_>,
) -> TabClosePlan {
    let button_id = tab_close_button(tree, &hit.node.id).map(|node| node.id.clone());
    let target_selected = hit.selected() == observe::Tri::True;
    let previously_selected = entries
        .iter()
        .find(|entry| entry.index != hit.index && entry.selected() == observe::Tri::True)
        .map(|entry| (entry.index, entry.title().to_owned()));
    TabClosePlan {
        target_id: hit.node.id.clone(),
        target_index: hit.index,
        target_selected,
        select_first: button_id.is_none() && !target_selected,
        button_id,
        previously_selected,
    }
}

/// The strip row (after the close) that the previously selected tab
/// became: the row at its shifted index when the title still agrees, else
/// the one row carrying that title. `None` when it cannot be told.
pub(super) fn restore_row<'a>(
    rows_after: &'a [serde_json::Value],
    previously_selected: &(usize, String),
    closed_index: usize,
) -> Option<&'a serde_json::Value> {
    let (index, title) = previously_selected;
    let expected = crate::tab_strip::index_after_close(*index, closed_index)?;
    if let Some(row) = rows_after
        .iter()
        .find(|row| row["index"] == expected && row["title"] == title.as_str())
    {
        return Some(row);
    }
    let titled: Vec<&serde_json::Value> = rows_after
        .iter()
        .filter(|row| row["title"] == title.as_str())
        .collect();
    match titled.as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

/// How many rows of `rows` carry `title`.
fn titled_rows(rows: &[serde_json::Value], title: &str) -> usize {
    rows.iter().filter(|row| row["title"] == title).count()
}

/// How long `tab close` waits for a background tab to become the selected
/// one (and grow its close button) after pressing its row.
pub(super) const TAB_CLOSE_SELECT_WAIT: Duration = Duration::from_millis(1_500);

struct CloseReadback {
    present: bool,
    window_present: bool,
    rows_after: Vec<serde_json::Value>,
    polls: usize,
    error: Option<CuError>,
}

/// Poll the strip after the close until `title` appears `expected` times
/// (one fewer than before), the window is gone, or the budget runs out.
fn tab_close_readback(
    window: isize,
    title: &str,
    expected: usize,
    rows_before: &[serde_json::Value],
    started: Instant,
    stop_early: bool,
) -> CloseReadback {
    let mut out = CloseReadback {
        present: true,
        window_present: true,
        rows_after: rows_before.to_vec(),
        polls: 0,
        error: None,
    };
    while started.elapsed() < TAB_CLOSE_READBACK {
        out.polls += 1;
        match mechanism::window_enumerate::enumerate_top_level() {
            Ok(now) if !now.iter().any(|item| item.handle == window) => {
                out.window_present = false;
                out.present = false;
                out.rows_after = Vec::new();
            }
            Ok(_) => match mechanism::tree_for_window(Some(window)) {
                Ok(after) => {
                    out.rows_after = tab_rows(&after);
                    out.present = titled_rows(&out.rows_after, title) > expected;
                }
                Err(error) => {
                    out.error = Some(map_mechanism_err(error));
                    break;
                }
            },
            Err(error) => {
                out.error = Some(map_mechanism_err(error));
                break;
            }
        }
        if !out.present || stop_early {
            break;
        }
        thread::sleep(TAB_CLOSE_READBACK_POLL);
    }
    out
}

/// One row press plus a bounded re-read; `Ok(tree)` is the tree after
/// the press (the caller judges it).
fn press_row(window: isize, node_id: &str) -> Result<(), CuError> {
    mechanism::perform_node_action(Some(window), node_id, mechanism::NodeAction::Press)
        .map_err(map_mechanism_err)
}

pub(super) fn tab_close_payload(
    window: isize,
    title: Option<&str>,
    index: Option<usize>,
    exact: bool,
    expect: Option<&str>,
    port: Option<u16>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    tab_close_gate(window, title, index, exact, expect)?;
    let spec = crate::tab_strip::TabCloseSpec::from_parts(title, index).map_err(invalid_input)?;
    let not_performed = |error: CuError| {
        let mut detail = error.detail.clone().unwrap_or(serde_json::json!({}));
        detail["effect"] = serde_json::json!("not_performed");
        error.with_detail(detail)
    };
    let before = mechanism::tree_for_window(Some(window))
        .map_err(map_mechanism_err)
        .map_err(not_performed)?;
    let entries = crate::tab_strip::tab_strip_entries(&before);
    let rows_before: Vec<serde_json::Value> = entries
        .iter()
        .map(crate::tab_strip::TabEntry::json)
        .collect();
    let hit = crate::tab_strip::match_tab_exact(&entries, &spec)
        .map_err(|error| not_performed(tab_match_error(error, &rows_before)))?;
    let tab_json = hit.json();
    let title = hit.title().to_owned();
    let plan = tab_close_plan(&before, &entries, hit);
    let expected_titled = titled_rows(&rows_before, &title).saturating_sub(1);
    let snapshot = serde_json::json!({ "tabs": rows_before.len(), "nodes": before.returned });

    // CDP first when a port is named: the tab is closed by the browser
    // itself, no row is pressed and no selection moves -- but only when
    // the title names exactly one page target of the whole instance (one
    // port serves every profile); otherwise the a11y path below.
    let mut cdp_fallback = None;
    let mut cdp_target: Option<crate::cdp::PageTarget> = None;
    if let Some(port) = port {
        match crate::cdp::targets::list_targets(port) {
            Ok(targets) => {
                let titled = crate::cdp::targets::page_targets_titled(&targets, &title);
                match titled.as_slice() {
                    [one] => cdp_target = Some((*one).clone()),
                    [] => {
                        cdp_fallback = Some(serde_json::json!({
                            "reason": "title_not_listed",
                            "port": port,
                            "matched": 0,
                        }));
                    }
                    many => {
                        cdp_fallback = Some(serde_json::json!({
                            "reason": "title_not_unique",
                            "port": port,
                            "matched": many.len(),
                            "candidates": many.iter().map(|t| t.identity_json()).collect::<Vec<_>>(),
                        }));
                    }
                }
            }
            Err(error) => {
                cdp_fallback = Some(serde_json::json!({
                    "reason": if error.code == "unsupported" { "no_listener" } else { error.code },
                    "port": port,
                    "message": error.message,
                }));
            }
        }
    }

    if let Some(target) = cdp_target {
        let port = port.unwrap_or(crate::cdp::DEFAULT_PORT);
        let ticket = receipts.reserve(
            "tab-close",
            window,
            serde_json::json!({
                "action": "Target.closeTarget",
                "via": "cdp-close-target",
                "tab": tab_json,
                "target": target.identity_json(),
                "port": port,
                "postcondition": "gone",
                "before": rows_before,
                "snapshot": snapshot,
            }),
        )?;
        let started = Instant::now();
        let closed: Result<bool, CuError> = crate::cdp::targets::browser_ws_url(port)
            .and_then(|url| crate::cdp::ws::connect(&url))
            .and_then(|mut session| crate::cdp::targets::close_target(&mut session, &target.id))
            .map_err(CuError::from);
        let mechanism_error = match closed {
            Ok(true) => None,
            Ok(false) => Some(CuError::new(
                "cdp_method_failed",
                "Target.closeTarget answered success: false",
            )),
            Err(error) => Some(error),
        };
        let readback = tab_close_readback(
            window,
            &title,
            expected_titled,
            &rows_before,
            started,
            mechanism_error.is_some(),
        );
        let selection_restored = plan.previously_selected.as_ref().map(|previous| {
            restore_row(&readback.rows_after, previous, plan.target_index)
                .is_some_and(|row| row["selected"] == true)
        });
        return finish_tab_close(
            window,
            &before,
            tab_json,
            serde_json::json!({
                "via": "cdp-close-target",
                "action": "Target.closeTarget",
                "node": serde_json::Value::Null,
                "cdp": { "port": port, "target": target.identity_json(), "matched": 1 },
                "select_first": false,
                "selected_before": plan.previously_selected.as_ref().map(|(index, _)| index),
                "selection_restored": selection_restored,
            }),
            rows_before,
            readback,
            mechanism_error,
            ticket,
            receipts,
            &title,
        );
    }

    // The a11y path: the row's own close button, after selecting a
    // background row when Chromium shows the button on the selected tab
    // only. Selecting a row switches the tab inside its window and never
    // brings the window forward.
    if plan.button_id.is_none() && !plan.select_first {
        return Err(CuError::new(
            "unsupported",
            format!(
                "tab {} ({:?}) is selected but exposes no close button in the accessibility tree; a keyboard shortcut is never substituted",
                hit.index,
                title
            ),
        )
        .with_detail(serde_json::json!({
            "reason": "tab_close_button_missing",
            "tab": tab_json,
            "cdp_fallback": cdp_fallback,
            "effect": "not_performed",
        })));
    }
    let ticket = receipts.reserve(
        "tab-close",
        window,
        serde_json::json!({
            "action": "press",
            "via": "tab-close",
            "tab": tab_json,
            "plan": plan.json(),
            "cdp_fallback": cdp_fallback,
            "postcondition": "gone",
            "before": rows_before,
            "snapshot": snapshot,
        }),
    )?;
    let started = Instant::now();
    let mut select_step = serde_json::Value::Null;
    let mut button_id = plan.button_id.clone();
    let mut mechanism_error = None;
    if plan.select_first {
        let mut polls = 0usize;
        let mut selected = false;
        match press_row(window, &plan.target_id) {
            Ok(()) => {
                let select_started = Instant::now();
                loop {
                    polls += 1;
                    match mechanism::tree_for_window(Some(window)) {
                        Ok(now) => {
                            selected =
                                observe::node_by_id(&now, &plan.target_id).is_some_and(|node| {
                                    observe::selected_state(node) == observe::Tri::True
                                });
                            button_id =
                                tab_close_button(&now, &plan.target_id).map(|node| node.id.clone());
                        }
                        Err(error) => {
                            mechanism_error = Some(map_mechanism_err(error));
                            break;
                        }
                    }
                    if button_id.is_some() || select_started.elapsed() >= TAB_CLOSE_SELECT_WAIT {
                        break;
                    }
                    thread::sleep(TAB_CLOSE_READBACK_POLL);
                }
            }
            Err(error) => mechanism_error = Some(error),
        }
        select_step = serde_json::json!({
            "performed": true,
            "selected": selected,
            "button_found": button_id.is_some(),
            "polls": polls,
        });
        if mechanism_error.is_none() && button_id.is_none() {
            // The row took the selection but grew no button: put the
            // selection back and refuse, performed nothing destructive.
            let restored = plan.previously_selected.as_ref().and_then(|(_, _)| {
                entries
                    .iter()
                    .find(|entry| entry.selected() == observe::Tri::True)
                    .map(|entry| entry.node.id.clone())
            });
            let mut selection_restored = None;
            if let Some(previous_id) = restored {
                selection_restored = Some(
                    press_row(window, &previous_id).is_ok()
                        && mechanism::tree_for_window(Some(window)).is_ok_and(|now| {
                            observe::node_by_id(&now, &previous_id).is_some_and(|node| {
                                observe::selected_state(node) == observe::Tri::True
                            })
                        }),
                );
            }
            let payload = serde_json::json!({
                "tab": tab_json,
                "select_first": select_step,
                "selection_restored": selection_restored,
                "receipt": ticket.json(),
            });
            receipts.complete(
                &ticket,
                "tab-close",
                window,
                false,
                serde_json::json!({
                    "performed": false,
                    "select_first": payload["select_first"],
                    "selection_restored": selection_restored,
                    "verification": { "method": "tab-strip-readback", "reason": "tab_close_button_missing" },
                }),
            )?;
            return Err(CuError::new(
                "unsupported",
                format!(
                    "tab {} ({:?}) was selected but still exposes no close button; a keyboard shortcut is never substituted",
                    hit.index, title
                ),
            )
            .with_detail(serde_json::json!({
                "reason": "tab_close_button_missing",
                "effect": "not_performed",
                "receipt": payload,
            })));
        }
    }
    let button_id = button_id.unwrap_or_default();
    if mechanism_error.is_none()
        && let Err(error) = press_row(window, &button_id)
    {
        mechanism_error = Some(error);
    }
    let mut readback = tab_close_readback(
        window,
        &title,
        expected_titled,
        &rows_before,
        started,
        mechanism_error.is_some(),
    );
    // Restore: the row that was selected before is pressed again when the
    // close moved the selection off it (Chromium selects a neighbour of
    // the closed tab when the closed one was selected).
    let mut restore_step = serde_json::Value::Null;
    let mut selection_restored = None;
    if let Some(previous) = plan.previously_selected.as_ref()
        && readback.window_present
        && mechanism_error.is_none()
        && readback.error.is_none()
    {
        match restore_row(&readback.rows_after, previous, plan.target_index) {
            Some(row) if row["selected"] == true => {
                selection_restored = Some(true);
                restore_step = serde_json::json!({ "performed": false, "already_selected": true });
            }
            Some(row) => {
                let row_id = row["id"].as_str().unwrap_or_default().to_owned();
                let pressed = press_row(window, &row_id);
                let verified = pressed.is_ok()
                    && match mechanism::tree_for_window(Some(window)) {
                        Ok(now) => {
                            readback.rows_after = tab_rows(&now);
                            observe::node_by_id(&now, &row_id).is_some_and(|node| {
                                observe::selected_state(node) == observe::Tri::True
                            })
                        }
                        Err(_) => false,
                    };
                selection_restored = Some(verified);
                restore_step = serde_json::json!({
                    "performed": true,
                    "row": row_id,
                    "verified": verified,
                    "error": pressed.err().as_ref().map(error_payload),
                });
            }
            None => {
                selection_restored = Some(false);
                restore_step =
                    serde_json::json!({ "performed": false, "reason": "previous_tab_not_found" });
            }
        }
    }
    finish_tab_close(
        window,
        &before,
        tab_json,
        serde_json::json!({
            "via": "tab-close",
            "action": "press",
            "node": button_id,
            "cdp_fallback": cdp_fallback,
            "select_first": if plan.select_first { select_step } else { serde_json::json!(false) },
            "selected_before": plan.previously_selected.as_ref().map(|(index, _)| index),
            "restore": restore_step,
            "selection_restored": selection_restored,
        }),
        rows_before,
        readback,
        mechanism_error,
        ticket,
        receipts,
        &title,
    )
}

/// Complete the receipt and shape the reply / error of either close path.
#[allow(clippy::too_many_arguments)]
fn finish_tab_close(
    window: isize,
    before: &mechanism::A11yTree,
    tab_json: serde_json::Value,
    extra: serde_json::Value,
    rows_before: Vec<serde_json::Value>,
    readback: CloseReadback,
    mechanism_error: Option<CuError>,
    ticket: receipt::ReceiptTicket,
    receipts: &mut ReceiptLog,
    title: &str,
) -> Result<serde_json::Value, CuError> {
    let CloseReadback {
        present,
        window_present,
        rows_after,
        polls,
        error: readback_error,
    } = readback;
    let verified = !present && mechanism_error.is_none() && readback_error.is_none();
    let reason = if mechanism_error.is_some() {
        Some("mechanism_failed")
    } else if readback_error.is_some() {
        Some("readback_failed")
    } else if present {
        Some("tab_still_present")
    } else {
        None
    };
    let verification = serde_json::json!({
        "method": "tab-strip-readback",
        "reason": reason,
        "polls": polls,
    });
    let after = serde_json::json!({
        "present": present,
        "window_present": window_present,
        "tabs": rows_after,
    });
    receipts.complete(
        &ticket,
        "tab-close",
        window,
        verified,
        serde_json::json!({
            "performed": mechanism_error.is_none(),
            "after": after,
            "verification": verification,
            "selection_restored": extra["selection_restored"],
            "error": mechanism_error.as_ref().or(readback_error.as_ref()).map(error_payload),
        }),
    )?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": before.backend,
        "window": window,
        "tab": tab_json,
        "postcondition": "gone",
        "performed": mechanism_error.is_none(),
        "verified": verified,
        "verification": verification,
        "focus_changed": false,
        "before": rows_before,
        "after": after,
        "receipt": ticket.json(),
    });
    if let (Some(object), Some(more)) = (payload.as_object_mut(), extra.as_object()) {
        for (key, value) in more {
            object.insert(key.clone(), value.clone());
        }
    }
    if let Some(error) = mechanism_error.or(readback_error) {
        return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
    }
    if present {
        return Err(CuError::new(
            "unverified",
            format!(
                "tab {title:?} was closed ({}) but the strip still lists it after {polls} polls",
                payload["via"].as_str().unwrap_or("tab-close")
            ),
        )
        .with_detail(serde_json::json!({ "reason": "tab_still_present", "receipt": payload })));
    }
    Ok(payload)
}

// ---------------------------------------------------------------------------
// Browser-chrome guard for the focused text writers (`send-text`, `paste`,
// `send-keys` with `--window` and no `--name`). In a browser the node the
// toolkit reports focused is the omnibox whenever the address bar holds
// focus, so an unaddressed write lands in the URL field -- a device-login
// code did exactly that. The page is everything below a web-area; anything
// else in a browser window is chrome and is refused unless the caller
// says `--allow-browser-chrome`.
// ---------------------------------------------------------------------------

/// Typed code of the focused-write refusal.
pub(super) const BROWSER_CHROME_CODE: &str = "focused_node_is_browser_chrome";

/// What a refused caller does next, quoted in `detail.hint`.
pub(super) const BROWSER_CHROME_HINT: &str = "pass --name to address a page control, or --allow-browser-chrome to write the omnibox deliberately";

/// Where a focused node sits, as far as the browser-chrome guard cares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FocusedNodeSite {
    /// Inside (or the root of) a web-area: page content.
    Page,
    /// In a browser window but outside every web-area: omnibox, toolbar,
    /// tab strip, find bar.
    BrowserChrome,
    /// The window is not a browser's; the guard has no opinion.
    NotBrowser,
}

fn is_web_area(node: &mechanism::A11yNode) -> bool {
    observe::normalize_role(&node.role) == "webarea"
}

/// `true` when `node_id` is a web-area or lies below one in `tree`.
pub(super) fn node_inside_web_area(tree: &mechanism::A11yTree, node_id: &str) -> bool {
    tree.nodes
        .iter()
        .filter(|node| is_web_area(node))
        .any(|root| node_id == root.id || node_id.starts_with(&format!("{}/", root.id)))
}

/// Classify the focused node `node_id` of the window `tree` was read from.
/// A window is a browser's when its tree carries a web-area or, failing
/// that, when the owning application's name says so (`app_name` is only
/// consulted in that second case: a page walk already settles it).
pub(super) fn classify_focused_site(
    tree: &mechanism::A11yTree,
    node_id: &str,
    app_name: impl FnOnce() -> String,
) -> FocusedNodeSite {
    if node_inside_web_area(tree, node_id) {
        return FocusedNodeSite::Page;
    }
    let browser =
        tree.nodes.iter().any(is_web_area) || observe::looks_like_browser_app(&app_name());
    if browser {
        FocusedNodeSite::BrowserChrome
    } else {
        FocusedNodeSite::NotBrowser
    }
}

/// The typed refusal for a focused write that would land in browser chrome.
pub(super) fn browser_chrome_refusal(verb: &str, node: &mechanism::A11yNode) -> CuError {
    CuError::new(
        BROWSER_CHROME_CODE,
        format!(
            "{verb} --window without --name would write browser chrome: the focused node is {} {:?}, not a control inside the page's web-area",
            node.role, node.name
        ),
    )
    .with_detail(serde_json::json!({
        "node": node.id,
        "role": node.role,
        "name": node.name,
        "hint": BROWSER_CHROME_HINT,
        "effect": "not_performed",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `poke` field is what the caller reads to know what was done to
    /// their browser. It used to be the macOS sentence on every host, so a
    /// Linux reply named an attribute Linux has no notion of and a Windows
    /// reply claimed a poke that never happens there.
    #[test]
    fn unlock_poke_names_each_hosts_own_mechanism() {
        let macos = poke_description_for("macos");
        assert!(macos.contains("AXManualAccessibility"), "{macos}");
        let linux = poke_description_for("linux");
        assert!(linux.contains("org.a11y.Status"), "{linux}");
        assert!(linux.contains("ScreenReaderEnabled"), "{linux}");
        assert!(linux.contains("org.a11y.Bus"), "{linux}");
        let windows = poke_description_for("windows");
        assert!(windows.contains("WM_GETOBJECT"), "{windows}");
        assert!(windows.contains("no separate poke"), "{windows}");
        // No host may be told it got the macOS attribute set on it.
        for poke in [linux, windows, poke_description_for("other")] {
            assert!(
                !poke.contains("AXManualAccessibility"),
                "the macOS attribute must not be claimed off macOS: {poke}"
            );
        }
        // The live reply uses the running host's wording.
        assert_eq!(
            unlock_poke_description(),
            poke_description_for(crate::mcu_surface::host_os()),
        );
    }

    #[test]
    fn spaces_and_page_js_are_mapped_verbs() {
        let spaces = observe_executor().execute(&Command::Spaces {
            target: TargetRef::Current,
        });
        assert_eq!(spaces.command, "spaces");
        if cfg!(target_os = "macos") {
            if spaces.ok {
                let data = spaces.data.as_ref().expect("spaces");
                assert!(data["displays"].is_array());
                assert_eq!(data["moveProvider"]["available"], false);
            } else {
                assert_eq!(spaces.error.as_ref().unwrap().code, "unsupported");
            }
        } else {
            assert!(!spaces.ok);
            assert_eq!(spaces.error.as_ref().unwrap().code, "unsupported");
        }
        let page = observe_executor().execute(&Command::PageJs {
            target: TargetRef::Current,
            window: None,
            expression: Some("1+1".into()),
            port: Some(1),
            target_id: None,
            target_url: None,
            target_title: None,
            target_match: None,
        });
        assert!(!page.ok);
        assert_eq!(page.command, "page-js");
        let err = page.error.as_ref().expect("typed");
        assert_eq!(err.code, "unsupported");
        assert_eq!(
            err.detail.as_ref().unwrap()["backend"],
            "debugger-runtime-evaluate"
        );
        assert!(err.message.contains("remote-debugging-port"));
        let targets = observe_executor().execute(&Command::PageTargets {
            target: TargetRef::Current,
            port: Some(1),
            browser_profile: None,
        });
        assert!(!targets.ok);
        assert_eq!(targets.command, "page-targets");
        let err = targets.error.as_ref().expect("typed");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("remote-debugging-port"));
    }

    #[test]
    fn page_js_selector_exclusivity_is_typed_before_any_socket() {
        let page = observe_executor().execute(&Command::PageJs {
            target: TargetRef::Current,
            window: None,
            expression: Some("1+1".into()),
            port: Some(1),
            target_id: Some("A1".into()),
            target_url: Some("mail".into()),
            target_title: None,
            target_match: None,
        });
        assert!(!page.ok);
        // Port 1 has no listener; the selector shape is still judged first
        // by the CLI, and the executor reports whichever failure the
        // library reaches first as a typed code, never `usage`.
        let err = page.error.as_ref().expect("typed");
        assert!(matches!(err.code.as_str(), "unsupported" | "invalid_input"));
    }

    #[test]
    fn page_text_requires_a_window_and_validates_its_bounds() {
        let page_text = |window: Option<isize>, max_bytes, depth, port, title: Option<&str>| {
            observe_executor().execute(&Command::PageText {
                target: TargetRef::Current,
                window,
                max_bytes,
                within: None,
                depth,
                max_nodes: None,
                port,
                target_id: None,
                target_url: None,
                target_title: title.map(str::to_owned),
                target_match: None,
            })
        };
        let none = page_text(Some(0), None, None, None, None);
        assert!(!none.ok);
        assert_eq!(none.command, "page-text");
        assert_eq!(none.error.as_ref().unwrap().code, "invalid_input");
        let bytes = page_text(Some(7), Some(0), None, None, None);
        assert_eq!(bytes.error.as_ref().unwrap().code, "invalid_input");
        let depth = page_text(Some(7), None, Some(99), None, None);
        assert_eq!(depth.error.as_ref().unwrap().code, "invalid_input");
        // Neither backend named, or both: invalid before any socket / tree.
        let neither = page_text(None, None, None, None, None);
        let err = neither.error.as_ref().unwrap();
        assert_eq!(err.code, "invalid_input");
        assert!(err.message.contains("--window") && err.message.contains("--target-"));
        let both = page_text(Some(7), None, None, Some(1), None);
        assert_eq!(both.error.as_ref().unwrap().code, "invalid_input");
        // The CDP backend with no listener is typed like page-js.
        let cdp = page_text(None, None, None, Some(1), Some("Inbox"));
        let err = cdp.error.as_ref().unwrap();
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("remote-debugging-port"));
        // A CDP read refuses the a11y walk budgets by name.
        let budget = page_text(None, None, Some(3), Some(1), Some("Inbox"));
        assert_eq!(budget.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn cdp_node_query_is_exactly_one_addressing_form() {
        use crate::cdp::page::NodeQuery;
        assert_eq!(
            cdp_node_query("page find", Some("#q"), None, None, None, None).unwrap(),
            NodeQuery::Css("#q".into())
        );
        assert_eq!(
            cdp_node_query("page find", None, None, Some("button"), Some("Go"), None).unwrap(),
            NodeQuery::Role {
                role: "button".into(),
                name: Some("Go".into())
            }
        );
        assert_eq!(
            cdp_node_query("page click", None, None, None, None, Some(17)).unwrap(),
            NodeQuery::Node(17)
        );
        let none = cdp_node_query("page find", None, None, None, None, None).unwrap_err();
        assert_eq!(none.code, "invalid_input");
        assert!(none.message.contains("exactly one"));
        let two =
            cdp_node_query("page click", Some("#q"), Some("Go"), None, None, None).unwrap_err();
        assert_eq!(two.code, "invalid_input");
        let name_alone =
            cdp_node_query("page find", Some("#q"), None, None, Some("x"), None).unwrap_err();
        assert!(name_alone.message.contains("--name"));
        let empty = cdp_node_query("page find", Some("  "), None, None, None, None).unwrap_err();
        assert!(empty.message.contains("must not be empty"));
    }

    #[test]
    fn cdp_actuators_are_grant_gated_and_typed_without_a_listener() {
        let click = Command::PageClick {
            target: TargetRef::Current,
            port: Some(1),
            target_id: None,
            target_url: None,
            target_title: Some("Inbox".into()),
            target_match: None,
            selector: Some("#go".into()),
            text: None,
            node: None,
            button: None,
            clicks: None,
        };
        let denied = observe_executor().execute(&click);
        assert!(!denied.ok);
        assert_eq!(denied.command, "page-click");
        assert_eq!(denied.error.as_ref().unwrap().code, "refused");
        let scratch = audit_scratch("cdp-click");
        let executor = actuate_executor().with_audit_path(scratch.clone());
        let reply = executor.execute(&click);
        assert!(!reply.ok);
        let err = reply.error.as_ref().unwrap();
        assert_eq!(err.code, "unsupported", "{}", err.message);
        assert!(err.message.contains("remote-debugging-port"));
        // The addressing shape is judged before any socket is opened.
        let shapeless = executor.execute(&Command::PageClick {
            target: TargetRef::Current,
            port: Some(1),
            target_id: None,
            target_url: None,
            target_title: Some("Inbox".into()),
            target_match: None,
            selector: None,
            text: None,
            node: None,
            button: None,
            clicks: None,
        });
        assert_eq!(shapeless.error.as_ref().unwrap().code, "invalid_input");
        let fill = executor.execute(&Command::PageFill {
            target: TargetRef::Current,
            port: Some(1),
            target_id: None,
            target_url: None,
            target_title: Some("Inbox".into()),
            target_match: None,
            selector: Some("#q".into()),
            node: None,
            text: "hi".into(),
            clear: false,
            submit: false,
        });
        assert_eq!(fill.command, "page-fill");
        assert_eq!(fill.error.as_ref().unwrap().code, "unsupported");
        let nav = executor.execute(&Command::PageNav {
            target: TargetRef::Current,
            port: Some(1),
            target_id: None,
            target_url: None,
            target_title: Some("Inbox".into()),
            target_match: None,
            url: "no-scheme".into(),
            wait_ms: None,
        });
        assert_eq!(nav.command, "page-nav");
        assert_eq!(nav.error.as_ref().unwrap().code, "invalid_input");
        let shot_dir = scratch.parent().unwrap().to_path_buf();
        let existing = shot_dir.join("exists.png");
        std::fs::write(&existing, b"x").unwrap();
        let shot = observe_executor().execute(&Command::PageScreenshot {
            target: TargetRef::Current,
            port: Some(1),
            target_id: None,
            target_url: None,
            target_title: None,
            target_match: None,
            out: existing.to_string_lossy().into_owned(),
            replace: false,
            activate: false,
        });
        assert_eq!(shot.command, "page-screenshot");
        let err = shot.error.as_ref().unwrap();
        assert_eq!(err.code, "invalid_input");
        assert!(err.message.contains("--replace"));
        // --activate is actuation: observe-only is refused before anything.
        let raised = observe_executor().execute(&Command::PageScreenshot {
            target: TargetRef::Current,
            port: Some(1),
            target_id: None,
            target_url: None,
            target_title: None,
            target_match: None,
            out: shot_dir.join("new.png").to_string_lossy().into_owned(),
            replace: false,
            activate: true,
        });
        assert_eq!(raised.error.as_ref().unwrap().code, "refused");
        let find = observe_executor().execute(&Command::PageFind {
            target: TargetRef::Current,
            port: Some(1),
            target_id: None,
            target_url: None,
            target_title: None,
            target_match: None,
            selector: None,
            text: Some("Go".into()),
            role: None,
            name: None,
        });
        assert_eq!(find.command, "page-find");
        assert_eq!(find.error.as_ref().unwrap().code, "unsupported");
        remove_audit_scratch(&scratch);
    }

    #[test]
    fn truncated_walk_names_the_larger_budget_not_a_screenshot() {
        let tree = mechanism::A11yTree {
            backend: "ax".into(),
            window_handle: Some(1),
            root_id: "/0".into(),
            nodes: Vec::new(),
            truncated: true,
            visited: 1000,
            returned: 1000,
        };
        let actions = truncation_next_actions(&tree, "query");
        assert_eq!(actions.len(), 2);
        assert!(
            actions[0].contains("--max-nodes 6000 --depth 64"),
            "{}",
            actions[0]
        );
        assert!(actions[0].contains("breadth-first"));
        assert!(
            actions
                .iter()
                .all(|a| !a.to_lowercase().contains("screenshot")
                    || a.contains("never a screenshot"))
        );
        let whole = mechanism::A11yTree {
            truncated: false,
            ..tree
        };
        assert!(truncation_next_actions(&whole, "query").is_empty());
    }

    #[test]
    fn tab_verbs_require_a_window_and_a_single_spec() {
        let list = observe_executor().execute(&Command::TabList {
            target: TargetRef::Current,
            window: 0,
        });
        assert!(!list.ok);
        assert_eq!(list.command, "tab-list");
        assert_eq!(list.error.as_ref().unwrap().code, "invalid_input");
        let select = actuate_executor().execute(&Command::TabSelect {
            target: TargetRef::Current,
            window: 0,
            title: Some("Codex".into()),
            index: None,
        });
        assert!(!select.ok);
        assert_eq!(select.command, "tab-select");
        assert_eq!(select.error.as_ref().unwrap().code, "invalid_input");
        let both = actuate_executor().execute(&Command::TabSelect {
            target: TargetRef::Current,
            window: 7,
            title: Some("Codex".into()),
            index: Some(1),
        });
        assert!(!both.ok);
        let err = both.error.as_ref().unwrap();
        assert_eq!(err.code, "invalid_input");
        assert!(err.message.contains("not both"), "{}", err.message);
        // Observe-only authorization never reaches the tab press.
        let denied = observe_executor().execute(&Command::TabSelect {
            target: TargetRef::Current,
            window: 7,
            title: None,
            index: Some(0),
        });
        assert!(!denied.ok);
        assert_ne!(denied.error.as_ref().unwrap().code, "invalid_input");
    }

    fn tree_of(nodes: Vec<mechanism::A11yNode>) -> mechanism::A11yTree {
        mechanism::A11yTree {
            backend: "ax".into(),
            window_handle: Some(1),
            root_id: "/0".into(),
            nodes,
            truncated: false,
            visited: 0,
            returned: 0,
        }
    }

    fn omnibox() -> mechanism::A11yNode {
        node_at(
            "/0/1",
            "Address and search bar",
            "AXTextField",
            &["showing", "focused"],
        )
    }

    fn web_area() -> mechanism::A11yNode {
        node_at("/0/2", "Sign in", "AXWebArea", &["showing"])
    }

    fn page_field() -> mechanism::A11yNode {
        node_at("/0/2/0/3", "Code", "AXTextField", &["showing", "focused"])
    }

    #[test]
    fn focused_site_is_page_below_a_web_area_and_chrome_beside_it() {
        let tree = tree_of(vec![omnibox(), web_area(), page_field()]);
        let never = || panic!("a page walk settles it; the app name must not be consulted");
        assert_eq!(
            classify_focused_site(&tree, "/0/2/0/3", never),
            FocusedNodeSite::Page
        );
        assert_eq!(
            classify_focused_site(&tree, "/0/2", never),
            FocusedNodeSite::Page,
            "the web-area root itself is page, not chrome"
        );
        assert_eq!(
            classify_focused_site(&tree, "/0/1", never),
            FocusedNodeSite::BrowserChrome
        );
        assert!(
            !node_inside_web_area(&tree, "/0/20"),
            "prefix must end at a path separator"
        );
    }

    #[test]
    fn focused_site_without_a_web_area_falls_back_to_the_app_name() {
        let tree = tree_of(vec![omnibox()]);
        assert_eq!(
            classify_focused_site(&tree, "/0/1", || "Brave Browser".to_owned()),
            FocusedNodeSite::BrowserChrome,
            "an unbuilt web tree still leaves the omnibox as chrome"
        );
        for app in [
            "Google Chrome",
            "Chromium",
            "Microsoft Edge",
            "Safari",
            "Firefox",
        ] {
            assert_eq!(
                classify_focused_site(&tree, "/0/1", || app.to_owned()),
                FocusedNodeSite::BrowserChrome,
                "{app}"
            );
        }
        assert_eq!(
            classify_focused_site(&tree, "/0/1", || "Terminal".to_owned()),
            FocusedNodeSite::NotBrowser
        );
    }

    #[test]
    fn browser_chrome_refusal_is_typed_with_the_node_and_a_hint() {
        let error = browser_chrome_refusal("send-text", &omnibox());
        assert_eq!(error.code, "focused_node_is_browser_chrome");
        assert_eq!(error.code, BROWSER_CHROME_CODE);
        assert!(error.message.contains("send-text --window without --name"));
        let detail = error.detail.expect("detail");
        assert_eq!(detail["node"], "/0/1");
        assert_eq!(detail["role"], "AXTextField");
        assert_eq!(detail["name"], "Address and search bar");
        assert_eq!(detail["hint"], BROWSER_CHROME_HINT);
        assert!(
            detail["hint"]
                .as_str()
                .unwrap()
                .contains("--allow-browser-chrome")
        );
        assert_eq!(detail["effect"], "not_performed");
    }

    #[test]
    fn tab_close_gate_names_every_missing_part_and_refuses_before_any_read() {
        let bare = actuate_executor().execute(&Command::TabClose {
            target: TargetRef::Current,
            window: 0,
            title: None,
            index: None,
            exact: false,
            expect: None,
            port: None,
        });
        assert!(!bare.ok);
        assert_eq!(bare.command, "tab-close");
        let err = bare.error.as_ref().expect("typed");
        assert_eq!(err.code, "refused");
        let detail = err.detail.as_ref().expect("detail");
        assert_eq!(detail["reason"], "destructive_gate");
        assert_eq!(
            detail["missing"],
            serde_json::json!(["target", "selector", "postcondition"])
        );
        assert_eq!(detail["effect"], "not_performed");
        let inexact = tab_close_gate(7, Some("x"), None, false, Some("gone")).unwrap_err();
        assert_eq!(
            inexact.detail.unwrap()["missing"],
            serde_json::json!(["exact"])
        );
        let wrong = tab_close_gate(7, Some("x"), None, true, Some("closed")).unwrap_err();
        assert_eq!(wrong.code, "invalid_input");
        assert!(tab_close_gate(7, Some("x"), None, true, Some("gone")).is_ok());
        // --index is the exact selector on its own: no --exact needed.
        assert!(tab_close_gate(7, None, Some(2), false, Some("gone")).is_ok());
        let both = tab_close_gate(7, Some("x"), Some(2), true, Some("gone")).unwrap_err();
        assert_eq!(both.code, "invalid_input");
        assert!(both.message.contains("not both"), "{}", both.message);
        let no_post = tab_close_gate(7, None, Some(2), false, None).unwrap_err();
        assert_eq!(
            no_post.detail.unwrap()["missing"],
            serde_json::json!(["postcondition"])
        );
        // Observe-only authorization never reaches the strip.
        let denied = observe_executor().execute(&Command::TabClose {
            target: TargetRef::Current,
            window: 7,
            title: Some("x".into()),
            index: None,
            exact: true,
            expect: Some("gone".into()),
            port: None,
        });
        assert!(!denied.ok);
        // A grant denial is also `refused`, but never the destructive gate.
        let denied = denied.error.as_ref().unwrap();
        assert_ne!(
            denied.detail.as_ref().and_then(|d| d["reason"].as_str()),
            Some("destructive_gate")
        );
    }

    #[test]
    fn tab_close_button_is_the_clickable_button_child_of_the_row() {
        let tab = node_at(
            "/0/1/0/1",
            "Codex",
            "AXRadioButton",
            &["showing", "selected"],
        );
        let mut close = node_at("/0/1/0/1/0", "关闭", "AXButton", &["showing"]);
        close.parent_id = Some("/0/1/0/1".into());
        close.actions = vec!["click".into()];
        let mut other = node_at("/0/1/0/0", "Inbox", "AXRadioButton", &["showing"]);
        other.parent_id = Some("/0/1/0".into());
        let mut label = node_at("/0/1/0/1/1", "Codex", "AXStaticText", &["showing"]);
        label.parent_id = Some("/0/1/0/1".into());
        let tree = tree_of(vec![other, tab, close, label]);
        assert_eq!(
            tab_close_button(&tree, "/0/1/0/1").map(|n| n.id.as_str()),
            Some("/0/1/0/1/0")
        );
        assert!(tab_close_button(&tree, "/0/1/0/0").is_none());
        // A button that offers no click is not a close control on AX.
        let mut inert = tree.clone();
        inert.nodes[2].actions.clear();
        assert!(tab_close_button(&inert, "/0/1/0/1").is_none());
    }

    /// A Chromium strip: `Inbox` (0), `Codex` (1, selected, with its close
    /// button), `Notes` (2), `Codex` (3) -- a background duplicate.
    fn chromium_strip() -> mechanism::A11yTree {
        let mut strip = node_at("/0/1", "", "AXTabGroup", &["showing"]);
        strip.parent_id = Some("/0".into());
        let row = |id: &str, title: &str, selected: bool| {
            let mut row = node_at(
                id,
                title,
                "AXRadioButton",
                &["showing", if selected { "selected" } else { "unselected" }],
            );
            row.parent_id = Some("/0/1".into());
            row.actions = vec!["click".into()];
            row
        };
        let mut close = node_at("/0/1/1/0", "关闭", "AXButton", &["showing"]);
        close.parent_id = Some("/0/1/1".into());
        close.actions = vec!["click".into()];
        tree_of(vec![
            node_at("/0", "Codex", "AXWindow", &["showing"]),
            strip,
            row("/0/1/0", "Inbox", false),
            row("/0/1/1", "Codex", true),
            close,
            row("/0/1/2", "Notes", false),
            row("/0/1/3", "Codex", false),
        ])
    }

    #[test]
    fn tab_close_plan_selects_a_background_row_first_and_remembers_the_selection() {
        let tree = chromium_strip();
        let entries = crate::tab_strip::tab_strip_entries(&tree);
        assert_eq!(entries.len(), 4);
        // The selected row has its button: press it, nothing to restore
        // (the selection was on the closed tab itself).
        let selected = tab_close_plan(&tree, &entries, &entries[1]);
        assert_eq!(selected.button_id.as_deref(), Some("/0/1/1/0"));
        assert!(!selected.select_first);
        assert!(selected.target_selected);
        assert_eq!(selected.previously_selected, None);
        // A background row: no button, so select it first and come back
        // to `Codex` at index 1 afterwards.
        let background = tab_close_plan(&tree, &entries, &entries[2]);
        assert_eq!(background.button_id, None);
        assert!(background.select_first);
        assert!(!background.target_selected);
        assert_eq!(
            background.previously_selected,
            Some((1, "Codex".to_owned()))
        );
        assert_eq!(background.json()["restore_to"]["index"], 1);
        // Same-title duplicates are only reachable by index: the plan is
        // for row 3, not row 1.
        let by_index =
            crate::tab_strip::match_tab_exact(&entries, &crate::tab_strip::TabCloseSpec::Index(3))
                .expect("fourth");
        let duplicate = tab_close_plan(&tree, &entries, by_index);
        assert_eq!(duplicate.target_id, "/0/1/3");
        assert!(duplicate.select_first);
        assert!(matches!(
            crate::tab_strip::match_tab_exact(
                &entries,
                &crate::tab_strip::TabCloseSpec::Title("Codex".into()),
            ),
            Err(crate::tab_strip::TabMatchError::Ambiguous { count: 2, .. })
        ));
    }

    #[test]
    fn restore_row_follows_the_shifted_index_then_the_unique_title() {
        let rows = |titles: &[(&str, bool)]| -> Vec<serde_json::Value> {
            titles
                .iter()
                .enumerate()
                .map(|(index, (title, selected))| {
                    serde_json::json!({
                        "index": index, "id": format!("/0/1/{index}"),
                        "title": title, "selected": selected,
                    })
                })
                .collect()
        };
        // Closed index 2 (`Notes`): `Codex` at 1 stays at 1.
        let after = rows(&[("Inbox", false), ("Codex", false), ("Codex", true)]);
        let previous = (1usize, "Codex".to_owned());
        let row = restore_row(&after, &previous, 2).expect("row");
        assert_eq!(row["index"], 1);
        assert_eq!(row["selected"], false);
        // Closed index 0: the previously selected row shifts left by one.
        let row = restore_row(&after, &(2usize, "Codex".to_owned()), 0).expect("shifted");
        assert_eq!(row["index"], 1);
        // Index disagrees on the title: the unique title decides.
        let moved = rows(&[("Notes", false), ("Inbox", true)]);
        let row = restore_row(&moved, &(0usize, "Inbox".to_owned()), 3).expect("by title");
        assert_eq!(row["index"], 1);
        // Neither the index nor a unique title: unknown.
        let twice = rows(&[("Codex", false), ("Codex", false)]);
        assert_eq!(restore_row(&twice, &(5usize, "Codex".to_owned()), 0), None);
        // The previously selected tab was the closed one: nothing to restore.
        assert_eq!(restore_row(&after, &(2usize, "Codex".to_owned()), 2), None);
    }

    #[test]
    fn page_targets_profile_join_rejects_an_empty_substring() {
        let reply = observe_executor().execute(&Command::PageTargets {
            target: TargetRef::Current,
            port: Some(1),
            browser_profile: Some("  ".into()),
        });
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }
}
