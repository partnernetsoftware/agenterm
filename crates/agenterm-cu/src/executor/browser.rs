//! Browser-specific verbs and helpers: `unlock` (web-tree poke), `page js`
//! / `page targets` (CDP), `page text` (visible words), the a11y tab strip
//! (`tab list` / `tab select`), and the browser-chrome guard the focused
//! text writers consult.

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
        "poke": "AXManualAccessibility + AXEnhancedUserInterface on the application, AXManualAccessibility on the window, then a renderer wake (hit-test + children reads)",
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
    selector: crate::page_js::TargetSelector,
) -> Result<serde_json::Value, CuError> {
    let expression = expression.unwrap_or("");
    let port = port.unwrap_or(crate::page_js::DEFAULT_PORT);
    crate::page_js::evaluate(port, expression, &selector)
        .map_err(|error| CuError::new(error.code, error.message).with_detail(error.detail))
}

/// `page targets`: the CDP `/json` inventory, so a caller can pick a
/// `--target-id` for a background tab without touching the desktop.
pub(super) fn page_targets_payload(port: Option<u16>) -> Result<serde_json::Value, CuError> {
    let port = port.unwrap_or(crate::page_js::DEFAULT_PORT);
    crate::page_js::targets(port)
        .map_err(|error| CuError::new(error.code, error.message).with_detail(error.detail))
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

/// `page text`: visible words in reading order, each with the node that
/// carries them, so the caller's next step is `invoke --node` /
/// `click --node`. The walk budget defaults to depth 64 / 6000 nodes.
pub(super) fn page_text_payload(
    window: isize,
    max_bytes: Option<usize>,
    within: Option<[i32; 4]>,
    depth: Option<u32>,
    max_nodes: Option<usize>,
) -> Result<serde_json::Value, CuError> {
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
        let none = observe_executor().execute(&Command::PageText {
            target: TargetRef::Current,
            window: 0,
            max_bytes: None,
            within: None,
            depth: None,
            max_nodes: None,
        });
        assert!(!none.ok);
        assert_eq!(none.command, "page-text");
        assert_eq!(none.error.as_ref().unwrap().code, "invalid_input");
        let bytes = observe_executor().execute(&Command::PageText {
            target: TargetRef::Current,
            window: 7,
            max_bytes: Some(0),
            within: None,
            depth: None,
            max_nodes: None,
        });
        assert_eq!(bytes.error.as_ref().unwrap().code, "invalid_input");
        let depth = observe_executor().execute(&Command::PageText {
            target: TargetRef::Current,
            window: 7,
            max_bytes: None,
            within: None,
            depth: Some(99),
            max_nodes: None,
        });
        assert_eq!(depth.error.as_ref().unwrap().code, "invalid_input");
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
}
