//! Page JavaScript second knife: Chromium CDP `Runtime.evaluate`.
//!
//! Never runs the expression in this process (no MAIN-world Function
//! constructor). Needs a listener from `--remote-debugging-port`.

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
}

/// Pick the first page target websocket URL from a Chrome `/json` body.
pub fn first_page_ws_url(list: &Value) -> Option<&str> {
    let items = list.as_array()?;
    for item in items {
        if item["type"].as_str() != Some("page") {
            continue;
        }
        if let Some(url) = item["webSocketDebuggerUrl"].as_str()
            && !url.is_empty()
        {
            return Some(url);
        }
    }
    items
        .first()
        .and_then(|item| item["webSocketDebuggerUrl"].as_str())
        .filter(|url| !url.is_empty())
}

/// Evaluate `expression` through CDP on `127.0.0.1:port`.
pub fn evaluate(port: u16, expression: &str) -> Result<Value, PageJsError> {
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
    let Some(ws) = first_page_ws_url(&list) else {
        return Err(PageJsError::typed(
            "unsupported",
            "CDP has no page target with webSocketDebuggerUrl",
        ));
    };
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
    let mut buf = Vec::new();
    stream
        .take(MAX_RESULT_BYTES as u64 + 2048)
        .read_to_end(&mut buf)
        .map_err(|_| PageJsError::typed("unsupported", "CDP HTTP read failed"))?;
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body)
        .map_err(|_| PageJsError::typed("unsupported", "CDP /json did not return JSON"))
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

    #[test]
    fn first_page_ws_url_prefers_page_type() {
        let list = json!([
            {"type": "iframe", "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/iframe"},
            {"type": "page", "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/main"}
        ]);
        assert_eq!(
            first_page_ws_url(&list),
            Some("ws://127.0.0.1:9222/devtools/page/main")
        );
        assert_eq!(
            split_ws_url("ws://127.0.0.1:9222/devtools/page/main"),
            Some(("127.0.0.1:9222".into(), "/devtools/page/main".into()))
        );
    }

    #[test]
    fn missing_listener_is_typed_without_main_world() {
        let err = evaluate(1, "1+1").expect_err("port 1 is not CDP");
        assert_eq!(err.code, "unsupported");
        assert_eq!(err.detail["backend"], "debugger-runtime-evaluate");
        assert!(err.message.contains("remote-debugging-port"));
    }
}
