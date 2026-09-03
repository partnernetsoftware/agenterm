//! Page JavaScript second knife: Chromium CDP `Runtime.evaluate`.
//!
//! Never runs the expression in this process (no MAIN-world Function
//! constructor). Needs a listener from `--remote-debugging-port`. That
//! port answers any local process, so a caller should open it only while
//! it is needed.
//!
//! Target selection: `/json` lists every tab as a `page` target whether
//! or not it is the active one, and `Runtime.evaluate` over that target's
//! websocket runs in a background tab as well. Picking a tab here
//! (`TargetSelector`) therefore never selects or raises anything — unlike
//! the AX tree, where macOS Chromium exposes only the active tab's
//! `web-area` (see `tab_strip`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{Value, json};

pub const DEFAULT_PORT: u16 = 9222;
pub const MAX_EXPRESSION_BYTES: usize = 4096;
pub const MAX_RESULT_BYTES: usize = 65_536;

pub fn backend() -> &'static str {
    crate::observe::page_js_backend()
}

pub fn reason() -> &'static str {
    crate::observe::page_js_unsupported_reason()
}

#[derive(Debug)]
pub struct PageJsError {
    pub code: &'static str,
    pub message: String,
    pub detail: Value,
}

impl PageJsError {
    fn typed(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: json!({
                "backend": backend(),
                "ax_default": true,
            }),
        }
    }

    /// Merge structured context into `detail`, keeping the backend fields.
    fn with_detail(mut self, extra: Value) -> Self {
        if let (Some(map), Some(more)) = (self.detail.as_object_mut(), extra.as_object()) {
            for (key, value) in more {
                map.insert(key.clone(), value.clone());
            }
        }
        self
    }
}

/// One entry of the Chrome `/json` target list, as this binary shapes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageTarget {
    pub id: String,
    pub url: String,
    pub title: String,
    /// CDP `type` (`page`, `iframe`, `service_worker`, ...).
    pub kind: String,
    /// CDP `attached` when the listener reports it.
    pub attached: Option<bool>,
    pub ws_url: Option<String>,
}

impl PageTarget {
    pub fn is_page(&self) -> bool {
        self.kind == "page"
    }

    /// The listing row: identity plus whether a websocket is offered
    /// (the URL itself is a local capability handle and is not echoed).
    pub fn json(&self) -> Value {
        json!({
            "id": self.id,
            "url": self.url,
            "title": self.title,
            "type": self.kind,
            "attached": self.attached,
            "websocket": self.ws_url.is_some(),
        })
    }

    /// The identity the `page-js` reply carries for the chosen target.
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
            kind: item["type"].as_str().unwrap_or_default().to_owned(),
            attached: item["attached"].as_bool(),
            ws_url: item["webSocketDebuggerUrl"]
                .as_str()
                .filter(|url| !url.is_empty())
                .map(str::to_owned),
        })
        .collect()
}

/// Which page target `page-js` evaluates on. At most one field is set;
/// all empty keeps the first-page default.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetSelector {
    /// Exact CDP target id.
    pub id: Option<String>,
    /// Case-insensitive substring of the target URL.
    pub url: Option<String>,
    /// Case-insensitive substring of the target title.
    pub title: Option<String>,
}

impl TargetSelector {
    pub fn is_empty(&self) -> bool {
        self.id.is_none() && self.url.is_none() && self.title.is_none()
    }

    fn count(&self) -> usize {
        usize::from(self.id.is_some())
            + usize::from(self.url.is_some())
            + usize::from(self.title.is_some())
    }

    pub fn json(&self) -> Value {
        json!({ "id": self.id, "url": self.url, "title": self.title })
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
        true
    }
}

/// Pick the page target `page-js` runs on. With an empty selector this is
/// today's first page target that offers a websocket (falling back to the
/// first target of any type, as before). With a selector, exactly one page
/// target must match: none is `cdp_target_not_found`, two or more is
/// `cdp_target_ambiguous`; both carry the page candidates in `detail`.
pub fn select_target<'a>(
    targets: &'a [PageTarget],
    selector: &TargetSelector,
) -> Result<&'a PageTarget, PageJsError> {
    if selector.count() > 1 {
        return Err(PageJsError::typed(
            "invalid_input",
            "page-js takes at most one of --target-id, --target-url, --target-title",
        ));
    }
    if selector.is_empty() {
        return targets
            .iter()
            .find(|target| target.is_page() && target.ws_url.is_some())
            .or_else(|| targets.first().filter(|target| target.ws_url.is_some()))
            .ok_or_else(|| {
                PageJsError::typed(
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
        [] => Err(PageJsError::typed(
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
                return Err(PageJsError::typed(
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
        many => Err(PageJsError::typed(
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

/// The CDP target inventory on `127.0.0.1:port` (`page targets`). No
/// listener is typed `unsupported`, the same as `page-js`.
pub fn targets(port: u16) -> Result<Value, PageJsError> {
    let list = http_get_json(port, "/json").or_else(|_| http_get_json(port, "/json/list"))?;
    let targets = parse_targets(&list);
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

/// Evaluate `expression` through CDP on `127.0.0.1:port`, on the page
/// target `selector` names (an empty selector keeps the first page).
/// `Runtime.evaluate` runs on background tabs as well: nothing here
/// activates a window or selects a tab.
pub fn evaluate(
    port: u16,
    expression: &str,
    selector: &TargetSelector,
) -> Result<Value, PageJsError> {
    if expression.is_empty() {
        return Err(PageJsError::typed(
            "invalid_input",
            "page-js requires --expression EXPR",
        ));
    }
    if expression.len() > MAX_EXPRESSION_BYTES {
        return Err(PageJsError::typed(
            "invalid_input",
            format!("page-js --expression must be 1..={MAX_EXPRESSION_BYTES} bytes"),
        ));
    }
    let list = http_get_json(port, "/json").or_else(|_| http_get_json(port, "/json/list"))?;
    let targets = parse_targets(&list);
    let target = select_target(&targets, selector)?;
    let ws = target.ws_url.as_deref().unwrap_or_default();
    let (host, path) = split_ws_url(ws).ok_or_else(|| {
        PageJsError::typed(
            "unsupported",
            "CDP webSocketDebuggerUrl is not ws://host/path",
        )
    })?;
    let value = runtime_evaluate(&host, &path, expression)?;
    Ok(json!({
        "backend": backend(),
        "port": port,
        "via": "Runtime.evaluate",
        "target": target.identity_json(),
        "selector": selector.json(),
        "focus_changed": false,
        "value": value,
    }))
}

fn split_ws_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("ws://")?;
    let (host, path) = rest.split_once('/')?;
    Some((host.to_owned(), format!("/{path}")))
}

fn http_get_json(port: u16, path: &str) -> Result<Value, PageJsError> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|_| {
        PageJsError::typed(
            "unsupported",
            format!("no CDP listener on 127.0.0.1:{port}; relaunch Chromium with --remote-debugging-port={port}"),
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|_| PageJsError::typed("unsupported", "CDP HTTP write failed"))?;
    let body = read_http_body(&mut stream, MAX_RESULT_BYTES + 2048).map_err(|reason| {
        PageJsError::typed("unsupported", format!("CDP HTTP read failed: {reason}"))
    })?;
    serde_json::from_slice(&body)
        .map_err(|_| PageJsError::typed("unsupported", "CDP /json did not return JSON"))
}

/// Read one HTTP/1.1 response body from `stream` without relying on the
/// server closing the socket. Chromium's DevTools HTTP server ignores
/// `Connection: close` and keeps the connection open, so a `read_to_end`
/// only returns when the read timeout fires. Honors `Content-Length`,
/// decodes `Transfer-Encoding: chunked`, and only falls back to
/// read-until-EOF when neither header is present. `limit` bounds the body.
fn read_http_body<R: Read>(stream: &mut R, limit: usize) -> Result<Vec<u8>, String> {
    let mut raw = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find_header_end(&raw) {
            break pos;
        }
        if raw.len() > 64 * 1024 {
            return Err("response headers exceed 64 KiB".into());
        }
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("header read: {e}"))?;
        if n == 0 {
            return Err("connection closed before headers ended".into());
        }
        raw.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
    let mut body = raw[header_end + 4..].to_vec();
    let mut content_length = None;
    let mut chunked = false;
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "content-length" {
            content_length = value.parse::<usize>().ok();
        } else if name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
    }
    if let Some(len) = content_length {
        if len > limit {
            return Err(format!("body of {len} bytes exceeds bound {limit}"));
        }
        while body.len() < len {
            let n = stream
                .read(&mut chunk)
                .map_err(|e| format!("body read: {e}"))?;
            if n == 0 {
                return Err("connection closed before Content-Length was satisfied".into());
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body.truncate(len);
        return Ok(body);
    }
    if chunked {
        return decode_chunked(stream, body, limit);
    }
    // No framing header: the server must close. Bound the read anyway.
    while body.len() <= limit {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("body read: {e}"))?;
        if n == 0 {
            return Ok(body);
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Err(format!("unframed body exceeds bound {limit}"))
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Decode a `Transfer-Encoding: chunked` body; `pending` is whatever body
/// bytes arrived together with the headers.
fn decode_chunked<R: Read>(
    stream: &mut R,
    mut pending: Vec<u8>,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // need a full "<hex>\r\n" size line
        let line_end = loop {
            if let Some(pos) = pending.windows(2).position(|w| w == b"\r\n") {
                break pos;
            }
            let n = stream
                .read(&mut chunk)
                .map_err(|e| format!("chunk size read: {e}"))?;
            if n == 0 {
                return Err("connection closed inside chunked body".into());
            }
            pending.extend_from_slice(&chunk[..n]);
        };
        let size_text = String::from_utf8_lossy(&pending[..line_end]).to_string();
        let size_hex = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| format!("bad chunk size {size_text:?}"))?;
        pending.drain(..line_end + 2);
        if size == 0 {
            return Ok(out);
        }
        if out.len() + size > limit {
            return Err(format!("chunked body exceeds bound {limit}"));
        }
        while pending.len() < size + 2 {
            let n = stream
                .read(&mut chunk)
                .map_err(|e| format!("chunk read: {e}"))?;
            if n == 0 {
                return Err("connection closed inside chunk".into());
            }
            pending.extend_from_slice(&chunk[..n]);
        }
        out.extend_from_slice(&pending[..size]);
        pending.drain(..size + 2);
    }
}

fn runtime_evaluate(host: &str, path: &str, expression: &str) -> Result<Value, PageJsError> {
    let mut stream = TcpStream::connect(host).map_err(|_| {
        PageJsError::typed(
            "unsupported",
            format!("CDP websocket connect failed: {host}"),
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|_| PageJsError::typed("unsupported", "CDP websocket handshake write failed"))?;
    let mut head = [0u8; 2048];
    let n = stream
        .read(&mut head)
        .map_err(|_| PageJsError::typed("unsupported", "CDP websocket handshake read failed"))?;
    let header = String::from_utf8_lossy(&head[..n]);
    if !header.contains("101") {
        return Err(PageJsError::typed(
            "unsupported",
            "CDP websocket handshake was not 101",
        ));
    }
    let payload = json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": {
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": false,
        }
    })
    .to_string();
    stream
        .write_all(&ws_client_text_frame(payload.as_bytes()))
        .map_err(|_| PageJsError::typed("unsupported", "CDP websocket frame write failed"))?;
    let raw = read_ws_text(&mut stream)?;
    let msg: Value = serde_json::from_str(&raw)
        .map_err(|_| PageJsError::typed("unsupported", "CDP Runtime.evaluate reply is not JSON"))?;
    if let Some(err) = msg.get("error") {
        return Err(PageJsError::typed(
            "unsupported",
            format!(
                "CDP Runtime.evaluate error: {}",
                err["message"].as_str().unwrap_or("unknown")
            ),
        ));
    }
    Ok(msg["result"]["result"]["value"].clone())
}

fn ws_client_text_frame(payload: &[u8]) -> Vec<u8> {
    let mask = [0x37, 0xfa, 0x21, 0x3d];
    let mut out = Vec::new();
    out.push(0x81);
    if payload.len() < 126 {
        out.push(0x80 | payload.len() as u8);
    } else {
        out.push(0x80 | 126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        out.push(b ^ mask[i % 4]);
    }
    out
}

fn read_ws_text(stream: &mut TcpStream) -> Result<String, PageJsError> {
    let mut hdr = [0u8; 2];
    stream
        .read_exact(&mut hdr)
        .map_err(|_| PageJsError::typed("unsupported", "CDP websocket frame header missing"))?;
    let opcode = hdr[0] & 0x0f;
    if opcode != 0x1 {
        return Err(PageJsError::typed(
            "unsupported",
            "CDP websocket frame is not text",
        ));
    }
    let mut len = (hdr[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream
            .read_exact(&mut ext)
            .map_err(|_| PageJsError::typed("unsupported", "CDP websocket length missing"))?;
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        return Err(PageJsError::typed(
            "unsupported",
            "CDP websocket frame exceeds page-js bound",
        ));
    }
    if len > MAX_RESULT_BYTES {
        return Err(PageJsError::typed(
            "unsupported",
            "CDP Runtime.evaluate result exceeds 64KiB",
        ));
    }
    let masked = hdr[1] & 0x80 != 0;
    let mut mask = [0u8; 4];
    if masked {
        stream
            .read_exact(&mut mask)
            .map_err(|_| PageJsError::typed("unsupported", "CDP websocket mask missing"))?;
    }
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .map_err(|_| PageJsError::typed("unsupported", "CDP websocket payload missing"))?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    String::from_utf8(payload)
        .map_err(|_| PageJsError::typed("unsupported", "CDP websocket payload is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<PageTarget> {
        parse_targets(&json!([
            {"type": "iframe", "id": "F1", "url": "https://ads.example/frame", "title": "frame",
             "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/F1"},
            {"type": "page", "id": "A1", "url": "https://mail.example/inbox", "title": "Inbox - Mail",
             "attached": false, "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/A1"},
            {"type": "page", "id": "B2", "url": "https://docs.example/Spec", "title": "Spec - Docs",
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
        assert_eq!(
            split_ws_url("ws://127.0.0.1:9222/devtools/page/main"),
            Some(("127.0.0.1:9222".into(), "/devtools/page/main".into()))
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
        let both = TargetSelector {
            id: Some("A1".into()),
            url: Some("mail".into()),
            title: None,
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
    fn missing_listener_is_typed_without_main_world() {
        let err = evaluate(1, "1+1", &TargetSelector::default()).expect_err("port 1 is not CDP");
        assert_eq!(err.code, "unsupported");
        assert_eq!(err.detail["backend"], "debugger-runtime-evaluate");
        assert!(err.message.contains("remote-debugging-port"));
        let err = targets(1).expect_err("port 1 is not CDP");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("remote-debugging-port"));
    }
}

#[cfg(test)]
mod http_body_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn content_length_body_does_not_wait_for_eof() {
        // A reader that would block forever after the body: Cursor over exact
        // bytes plus a guard that panics if read past the framed body.
        struct NoEof(Cursor<Vec<u8>>);
        impl Read for NoEof {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.read(buf)?;
                if n == 0 {
                    panic!("read past the framed body: the server keeps the socket open");
                }
                Ok(n)
            }
        }
        let payload = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 12\r\n\r\n[{\"id\":\"a\"}]";
        let mut reader = NoEof(Cursor::new(payload.to_vec()));
        let body = read_http_body(&mut reader, 1 << 20).expect("framed body");
        assert_eq!(body, b"[{\"id\":\"a\"}]");
    }

    #[test]
    fn chunked_body_is_decoded() {
        let payload = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n[{\"i\r\n8\r\nd\":\"a\"}]\r\n0\r\n\r\n";
        let mut reader = Cursor::new(payload.to_vec());
        let body = read_http_body(&mut reader, 1 << 20).expect("chunked body");
        assert_eq!(body, b"[{\"id\":\"a\"}]");
    }

    #[test]
    fn unframed_body_reads_until_eof() {
        let payload = b"HTTP/1.1 200 OK\r\n\r\n{\"ok\":true}";
        let mut reader = Cursor::new(payload.to_vec());
        let body = read_http_body(&mut reader, 1 << 20).expect("eof body");
        assert_eq!(body, b"{\"ok\":true}");
    }

    #[test]
    fn oversized_content_length_is_refused_before_reading() {
        let payload = b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\n\r\n";
        let mut reader = Cursor::new(payload.to_vec());
        let err = read_http_body(&mut reader, 1024).unwrap_err();
        assert!(err.contains("exceeds bound"), "{err}");
    }
}
