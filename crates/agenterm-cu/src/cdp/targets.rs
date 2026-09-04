//! The `/json` target inventory and the one-tab selector every CDP verb
//! takes (`--target-id | --target-url | --target-title | --match`). Picking a
//! tab here never selects or raises anything — unlike the AX tree, where
//! macOS Chromium exposes only the active tab's `web-area` (see
//! `tab_strip`).

use serde_json::{Value, json};

use super::http::http_get_json;
use super::ws::{Session, TcpTransport};
use super::{CdpError, backend};

/// One entry of the Chrome `/json` target list, as this binary shapes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageTarget {
    pub id: String,
    pub url: String,
    pub title: String,
    /// Optional debugger-provided description; used by MCU-compatible
    /// `--match` but not repeated in the selected-target identity.
    pub description: String,
    /// CDP `type` (`page`, `iframe`, `service_worker`, ...).
    pub kind: String,
    /// CDP `attached` when the listener reports it.
    pub attached: Option<bool>,
    pub ws_url: Option<String>,
}

impl PageTarget {
    pub fn is_page(&self) -> bool {
        matches!(self.kind.as_str(), "page" | "webview" | "other")
    }

    /// The listing row: identity plus whether a websocket is offered
    /// (the URL itself is a local capability handle and is not echoed).
    pub fn json(&self) -> Value {
        json!({
            "id": self.id,
            "url": self.url,
            "title": self.title,
            "description": self.description,
            "type": self.kind,
            "attached": self.attached,
            "websocket": self.ws_url.is_some(),
        })
    }

    /// The identity every CDP reply carries for the chosen target.
    pub fn identity_json(&self) -> Value {
        json!({ "id": self.id, "url": self.url, "title": self.title })
    }
}

/// Every target of a Chrome `/json` body, in listing order. Entries that
/// are not objects are skipped; a missing field is empty, never a guess.
pub fn parse_targets(list: &Value) -> Vec<PageTarget> {
    let Some(items) = list.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|item| item.is_object())
        .map(|item| PageTarget {
            id: item["id"].as_str().unwrap_or_default().to_owned(),
            url: item["url"].as_str().unwrap_or_default().to_owned(),
            title: item["title"].as_str().unwrap_or_default().to_owned(),
            description: item["description"].as_str().unwrap_or_default().to_owned(),
            kind: item["type"].as_str().unwrap_or_default().to_owned(),
            attached: item["attached"].as_bool(),
            ws_url: item["webSocketDebuggerUrl"]
                .as_str()
                .filter(|url| !url.is_empty())
                .map(str::to_owned),
        })
        .collect()
}

/// Which page target a CDP verb runs on. At most one field is set; all
/// empty keeps the first-page default.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetSelector {
    /// Exact CDP target id.
    pub id: Option<String>,
    /// Case-insensitive substring of the target URL.
    pub url: Option<String>,
    /// Case-insensitive substring of the target title.
    pub title: Option<String>,
    /// Case-insensitive substring across title + URL + description. This is
    /// the MCU compatibility selector, but ambiguity now fails closed.
    pub match_any: Option<String>,
}

impl TargetSelector {
    pub fn is_empty(&self) -> bool {
        self.id.is_none() && self.url.is_none() && self.title.is_none() && self.match_any.is_none()
    }

    fn count(&self) -> usize {
        usize::from(self.id.is_some())
            + usize::from(self.url.is_some())
            + usize::from(self.title.is_some())
            + usize::from(self.match_any.is_some())
    }

    pub fn json(&self) -> Value {
        json!({ "id": self.id, "url": self.url, "title": self.title, "match": self.match_any })
    }

    fn matches(&self, target: &PageTarget) -> bool {
        if let Some(id) = &self.id {
            return target.id == *id;
        }
        if let Some(url) = &self.url {
            return target.url.to_lowercase().contains(&url.to_lowercase());
        }
        if let Some(title) = &self.title {
            return target.title.to_lowercase().contains(&title.to_lowercase());
        }
        if let Some(pattern) = &self.match_any {
            let pattern = pattern.to_lowercase();
            return target.title.to_lowercase().contains(&pattern)
                || target.url.to_lowercase().contains(&pattern)
                || target.description.to_lowercase().contains(&pattern);
        }
        true
    }
}

/// Pick the page target a verb runs on. With an empty selector this is
/// today's first page target that offers a websocket (falling back to the
/// first target of any type, as before). With a selector, exactly one page
/// target must match: none is `cdp_target_not_found`, two or more is
/// `cdp_target_ambiguous`; both carry the page candidates in `detail`.
pub fn select_target<'a>(
    targets: &'a [PageTarget],
    selector: &TargetSelector,
) -> Result<&'a PageTarget, CdpError> {
    if selector.count() > 1 {
        return Err(CdpError::typed(
            "invalid_input",
            "a CDP verb takes at most one of --target-id, --target-url, --target-title, --match",
        ));
    }
    if selector.is_empty() {
        return targets
            .iter()
            .find(|target| target.is_page() && target.ws_url.is_some())
            .or_else(|| targets.first().filter(|target| target.ws_url.is_some()))
            .ok_or_else(|| {
                CdpError::typed(
                    "unsupported",
                    "CDP has no page target with webSocketDebuggerUrl",
                )
            });
    }
    let pages: Vec<&PageTarget> = targets.iter().filter(|target| target.is_page()).collect();
    let hits: Vec<&PageTarget> = pages
        .iter()
        .copied()
        .filter(|target| selector.matches(target))
        .collect();
    let candidates = |list: &[&PageTarget]| -> Value {
        list.iter().map(|target| target.identity_json()).collect()
    };
    match hits.as_slice() {
        [] => Err(CdpError::typed(
            "cdp_target_not_found",
            format!(
                "no CDP page target matches {}; {} page target(s) listed",
                selector_scope(selector),
                pages.len()
            ),
        )
        .with_detail(json!({
            "selector": selector.json(),
            "candidates": candidates(&pages),
        }))),
        [one] => {
            if one.ws_url.is_none() {
                return Err(CdpError::typed(
                    "unsupported",
                    format!(
                        "CDP page target {} offers no webSocketDebuggerUrl (another client may be attached)",
                        one.id
                    ),
                )
                .with_detail(json!({ "target": one.identity_json() })));
            }
            Ok(one)
        }
        many => Err(CdpError::typed(
            "cdp_target_ambiguous",
            format!(
                "{} CDP page targets match {}; refusing to guess",
                many.len(),
                selector_scope(selector)
            ),
        )
        .with_detail(json!({
            "selector": selector.json(),
            "count": many.len(),
            "candidates": candidates(many),
        }))),
    }
}

fn selector_scope(selector: &TargetSelector) -> String {
    if let Some(id) = &selector.id {
        format!("--target-id {id:?}")
    } else if let Some(url) = &selector.url {
        format!("--target-url {url:?}")
    } else if let Some(title) = &selector.title {
        format!("--target-title {title:?}")
    } else if let Some(pattern) = &selector.match_any {
        format!("--match {pattern:?} across title, URL and description")
    } else {
        "the default (first page)".to_owned()
    }
}

/// Pick the first page target websocket URL from a Chrome `/json` body.
pub fn first_page_ws_url(list: &Value) -> Option<String> {
    let targets = parse_targets(list);
    select_target(&targets, &TargetSelector::default())
        .ok()
        .and_then(|target| target.ws_url.clone())
}

/// The `/json` list on `127.0.0.1:port`, parsed. No listener is typed
/// `unsupported` with the relaunch hint.
pub fn list_targets(port: u16) -> Result<Vec<PageTarget>, CdpError> {
    let list = http_get_json(port, "/json").or_else(|_| http_get_json(port, "/json/list"))?;
    Ok(parse_targets(&list))
}

/// The CDP target inventory on `127.0.0.1:port` (`page targets`).
pub fn targets(port: u16) -> Result<Value, CdpError> {
    let targets = list_targets(port)?;
    Ok(targets_payload(port, &targets))
}

/// The `page targets` reply shape: every target in listing order plus the
/// page count, so a caller can pick a `--target-id` without a second read.
pub fn targets_payload(port: u16, targets: &[PageTarget]) -> Value {
    json!({
        "backend": backend(),
        "port": port,
        "via": "/json",
        "returned": targets.len(),
        "pages": targets.iter().filter(|target| target.is_page()).count(),
        "targets": targets.iter().map(PageTarget::json).collect::<Vec<_>>(),
    })
}

/// Resolve `selector` on `port` and open one session to that target.
/// Nothing here activates the target.
pub fn connect_target(
    port: u16,
    selector: &TargetSelector,
) -> Result<(PageTarget, Session<TcpTransport>), CdpError> {
    let targets = list_targets(port)?;
    let target = select_target(&targets, selector)?.clone();
    let ws = target.ws_url.as_deref().unwrap_or_default();
    let session = super::ws::connect(ws)?;
    Ok((target, session))
}

/// The browser-level websocket (`/json/version` -> `webSocketDebuggerUrl`):
/// the one endpoint that accepts `Target.closeTarget` for any target of
/// the instance without attaching to that target first.
pub fn browser_ws_url(port: u16) -> Result<String, CdpError> {
    let version = http_get_json(port, "/json/version")?;
    version["webSocketDebuggerUrl"]
        .as_str()
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            CdpError::typed(
                "unsupported",
                "CDP /json/version offers no browser webSocketDebuggerUrl",
            )
        })
}

/// Every page target whose title equals `title` exactly (case-sensitive),
/// in listing order. `tab close --port` closes over CDP only when this is
/// exactly one: one port serves every profile of the instance, so a title
/// shared by two tabs (any window, any profile) cannot name a target.
pub fn page_targets_titled<'a>(targets: &'a [PageTarget], title: &str) -> Vec<&'a PageTarget> {
    targets
        .iter()
        .filter(|target| target.is_page() && target.title == title)
        .collect()
}

/// `Target.closeTarget` over `session`: `Ok(true)` when the browser
/// accepted the close (the tab is gone or going), `Ok(false)` when it
/// answered `success: false`. Nothing here activates anything.
pub fn close_target<T: super::ws::Transport>(
    session: &mut Session<T>,
    target_id: &str,
) -> Result<bool, CdpError> {
    let result = session.call("Target.closeTarget", json!({ "targetId": target_id }))?;
    Ok(result["success"].as_bool().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titled_page_targets_are_exact_and_pages_only() {
        let mut targets = fixture();
        assert_eq!(
            page_targets_titled(&targets, "Inbox - Mail")
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["A1"]
        );
        // Substring / case are not matches; an iframe is not a page.
        assert!(page_targets_titled(&targets, "Inbox").is_empty());
        assert!(page_targets_titled(&targets, "inbox - mail").is_empty());
        assert!(page_targets_titled(&targets, "frame").is_empty());
        // A second window's tab with the same title makes it not unique.
        targets.push(PageTarget {
            id: "D4".into(),
            url: "https://mail.example/other".into(),
            title: "Inbox - Mail".into(),
            description: String::new(),
            kind: "page".into(),
            attached: Some(false),
            ws_url: None,
        });
        assert_eq!(page_targets_titled(&targets, "Inbox - Mail").len(), 2);
    }

    #[test]
    fn close_target_reports_the_browser_answer_and_never_activates() {
        use super::super::ws::fake;
        let mut session = fake::session(|method, params| match method {
            "Target.closeTarget" => {
                assert_eq!(params["targetId"], "B2");
                Ok(json!({ "success": true }))
            }
            other => Err(format!("unexpected {other}")),
        });
        assert!(close_target(&mut session, "B2").expect("closed"));
        assert_eq!(session.transport.methods(), ["Target.closeTarget"]);
        let mut refused = fake::session(|_, _| Ok(json!({ "success": false })));
        assert!(!close_target(&mut refused, "B2").expect("answered"));
        let mut failing = fake::session(|_, _| Err("No target with given id found".into()));
        let err = close_target(&mut failing, "B2").expect_err("typed");
        assert_eq!(err.code, "cdp_method_failed");
        assert_eq!(err.failed_method(), Some("Target.closeTarget"));
        assert_eq!(browser_ws_url(1).expect_err("port 1").code, "unsupported");
    }

    fn fixture() -> Vec<PageTarget> {
        parse_targets(&json!([
            {"type": "iframe", "id": "F1", "url": "https://ads.example/frame", "title": "frame",
             "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/F1"},
            {"type": "page", "id": "A1", "url": "https://mail.example/inbox", "title": "Inbox - Mail",
             "attached": false, "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/A1"},
            {"type": "page", "id": "B2", "url": "https://docs.example/Spec", "title": "Spec - Docs", "description": "Blue project editor",
             "attached": false, "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/B2"},
            {"type": "page", "id": "C3", "url": "https://docs.example/notes", "title": "Notes - Docs",
             "attached": true, "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/C3"},
            {"type": "service_worker", "id": "S4", "url": "https://docs.example/sw.js", "title": "sw"}
        ]))
    }

    #[test]
    fn first_page_ws_url_prefers_page_type() {
        let list = json!([
            {"type": "iframe", "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/iframe"},
            {"type": "page", "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/main"}
        ]);
        assert_eq!(
            first_page_ws_url(&list).as_deref(),
            Some("ws://127.0.0.1:9222/devtools/page/main")
        );
        // No page at all: the first target with a websocket, as before.
        let only_iframe = json!([
            {"type": "iframe", "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/iframe"}
        ]);
        assert_eq!(
            first_page_ws_url(&only_iframe).as_deref(),
            Some("ws://127.0.0.1:9222/devtools/page/iframe")
        );
        assert_eq!(first_page_ws_url(&json!({"not": "a list"})), None);
    }

    #[test]
    fn empty_selector_keeps_first_page_default() {
        let targets = fixture();
        let hit = select_target(&targets, &TargetSelector::default()).expect("first page");
        assert_eq!(hit.id, "A1");
        assert_eq!(
            select_target(&[], &TargetSelector::default())
                .expect_err("nothing listed")
                .code,
            "unsupported"
        );
    }

    #[test]
    fn selector_by_id_is_exact_and_pages_only() {
        let targets = fixture();
        let by_id = TargetSelector {
            id: Some("C3".into()),
            ..TargetSelector::default()
        };
        assert_eq!(
            select_target(&targets, &by_id).expect("id hit").title,
            "Notes - Docs"
        );
        // An iframe id is not a page target even though it is listed.
        let frame = TargetSelector {
            id: Some("F1".into()),
            ..TargetSelector::default()
        };
        let err = select_target(&targets, &frame).expect_err("iframe is not a page");
        assert_eq!(err.code, "cdp_target_not_found");
        // Case matters for ids.
        let lower = TargetSelector {
            id: Some("c3".into()),
            ..TargetSelector::default()
        };
        assert_eq!(
            select_target(&targets, &lower).expect_err("exact id").code,
            "cdp_target_not_found"
        );
    }

    #[test]
    fn selector_by_url_and_title_are_case_insensitive_substrings() {
        let targets = fixture();
        let by_url = TargetSelector {
            url: Some("MAIL.EXAMPLE".into()),
            ..TargetSelector::default()
        };
        assert_eq!(select_target(&targets, &by_url).expect("url hit").id, "A1");
        let by_title = TargetSelector {
            title: Some("notes".into()),
            ..TargetSelector::default()
        };
        assert_eq!(
            select_target(&targets, &by_title).expect("title hit").id,
            "C3"
        );
        let by_any_description = TargetSelector {
            match_any: Some("BLUE PROJECT".into()),
            ..TargetSelector::default()
        };
        assert_eq!(
            select_target(&targets, &by_any_description)
                .expect("description hit")
                .id,
            "B2"
        );
        let by_any_url = TargetSelector {
            match_any: Some("mail.example".into()),
            ..TargetSelector::default()
        };
        assert_eq!(
            select_target(&targets, &by_any_url).expect("url hit").id,
            "A1"
        );
    }

    #[test]
    fn selector_misses_and_ambiguity_are_typed_with_candidates() {
        let targets = fixture();
        let none = TargetSelector {
            title: Some("nowhere".into()),
            ..TargetSelector::default()
        };
        let err = select_target(&targets, &none).expect_err("no hit");
        assert_eq!(err.code, "cdp_target_not_found");
        assert_eq!(err.detail["backend"], "debugger-runtime-evaluate");
        assert_eq!(err.detail["candidates"].as_array().map(Vec::len), Some(3));
        assert_eq!(err.detail["selector"]["title"], "nowhere");
        let many = TargetSelector {
            url: Some("docs.example".into()),
            ..TargetSelector::default()
        };
        let err = select_target(&targets, &many).expect_err("two docs pages");
        assert_eq!(err.code, "cdp_target_ambiguous");
        assert_eq!(err.detail["count"], 2);
        let ids: Vec<&str> = err.detail["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["B2", "C3"]);
        assert!(err.detail["candidates"][0]["url"].is_string());
        assert!(err.detail["candidates"][0]["title"].is_string());
        let match_many = TargetSelector {
            match_any: Some("docs".into()),
            ..TargetSelector::default()
        };
        let err = select_target(&targets, &match_many).expect_err("match ambiguity");
        assert_eq!(err.code, "cdp_target_ambiguous");
        assert_eq!(err.detail["selector"]["match"], "docs");
        assert_eq!(err.detail["count"], 2);
        let both = TargetSelector {
            id: Some("A1".into()),
            url: Some("mail".into()),
            title: None,
            match_any: None,
        };
        assert_eq!(
            select_target(&targets, &both).expect_err("exclusive").code,
            "invalid_input"
        );
    }

    #[test]
    fn targets_payload_lists_every_target_with_websocket_presence() {
        let targets = fixture();
        let payload = targets_payload(9222, &targets);
        assert_eq!(payload["via"], "/json");
        assert_eq!(payload["port"], 9222);
        assert_eq!(payload["returned"], 5);
        assert_eq!(payload["pages"], 3);
        let rows = payload["targets"].as_array().expect("targets");
        assert_eq!(rows[1]["id"], "A1");
        assert_eq!(rows[1]["type"], "page");
        assert_eq!(rows[1]["attached"], false);
        assert_eq!(rows[1]["websocket"], true);
        assert_eq!(rows[3]["attached"], true);
        assert_eq!(rows[4]["type"], "service_worker");
        assert_eq!(rows[4]["websocket"], false);
        assert!(rows[4]["attached"].is_null());
        // The websocket URL is a local capability handle, not listing data.
        assert!(!payload.to_string().contains("devtools"));
        assert_eq!(targets_payload(1, &[])["returned"], 0);
    }

    #[test]
    fn missing_listener_is_typed_for_targets_and_connect() {
        let err = targets(1).expect_err("port 1 is not CDP");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("remote-debugging-port"));
        let err = connect_target(1, &TargetSelector::default()).expect_err("port 1");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("remote-debugging-port"));
    }
}
