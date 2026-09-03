//! One websocket to one CDP target.
//!
//! `Transport` is the seam: `TcpTransport` speaks RFC 6455 client framing
//! over a `TcpStream`; the tests drive `Session` through a scripted
//! `FakeTransport` and never need a browser. `Session` numbers requests,
//! matches replies by id, and keeps the events that arrive in between so
//! a verb can wait for `Page.loadEventFired` after `Page.navigate`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::CdpError;

/// Bound on one inbound message. An `Accessibility.getFullAXTree` of a
/// large page runs to megabytes, and a screenshot is base64 PNG.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// How long one method call may wait for its reply.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);

pub trait Transport {
    fn send_text(&mut self, text: &str) -> Result<(), CdpError>;
    /// The next text message, or `None` when `timeout` passed before one
    /// arrived. A timeout in the middle of a frame is an error: the stream
    /// is out of sync and the session must not be reused.
    fn recv_text(&mut self, timeout: Duration) -> Result<Option<String>, CdpError>;
}

/// The wire: a handshaken websocket over loopback TCP.
#[derive(Debug)]
pub struct TcpTransport {
    stream: TcpStream,
}

impl TcpTransport {
    /// Connect to `ws://host/path` and complete the client handshake.
    pub fn connect(ws_url: &str) -> Result<Self, CdpError> {
        let (host, path) = split_ws_url(ws_url).ok_or_else(|| {
            CdpError::typed(
                "unsupported",
                "CDP webSocketDebuggerUrl is not ws://host/path",
            )
        })?;
        let mut stream = TcpStream::connect(host.as_str()).map_err(|_| {
            CdpError::typed(
                "unsupported",
                format!("CDP websocket connect failed: {host}"),
            )
        })?;
        stream.set_read_timeout(Some(CALL_TIMEOUT)).ok();
        stream.set_write_timeout(Some(CALL_TIMEOUT)).ok();
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream
            .write_all(req.as_bytes())
            .map_err(|_| CdpError::typed("unsupported", "CDP websocket handshake write failed"))?;
        // Read the 101 response headers byte-wise up to the blank line so
        // no frame bytes that follow are swallowed.
        let mut head = Vec::with_capacity(512);
        let mut byte = [0u8; 1];
        loop {
            let n = stream.read(&mut byte).map_err(|_| {
                CdpError::typed("unsupported", "CDP websocket handshake read failed")
            })?;
            if n == 0 {
                return Err(CdpError::typed(
                    "unsupported",
                    "CDP websocket handshake closed early",
                ));
            }
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            if head.len() > 16 * 1024 {
                return Err(CdpError::typed(
                    "unsupported",
                    "CDP websocket handshake headers exceed 16 KiB",
                ));
            }
        }
        let header = String::from_utf8_lossy(&head);
        if !header.starts_with("HTTP/1.1 101") {
            return Err(CdpError::typed(
                "unsupported",
                "CDP websocket handshake was not 101",
            ));
        }
        Ok(Self { stream })
    }

    fn read_exact_timed(&mut self, buf: &mut [u8], what: &str) -> Result<(), CdpError> {
        self.stream.read_exact(buf).map_err(|error| {
            CdpError::typed(
                "unsupported",
                format!("CDP websocket {what} missing: {error}"),
            )
        })
    }
}

pub fn split_ws_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("ws://")?;
    let (host, path) = rest.split_once('/')?;
    Some((host.to_owned(), format!("/{path}")))
}

/// One masked client frame (`opcode` 0x1 text, 0x9 ping, 0xA pong, 0x8 close).
pub fn client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x37, 0xfa, 0x21, 0x3d];
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | (opcode & 0x0f));
    if payload.len() < 126 {
        out.push(0x80 | payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        out.push(0x80 | 126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        out.push(b ^ mask[i % 4]);
    }
    out
}

impl Transport for TcpTransport {
    fn send_text(&mut self, text: &str) -> Result<(), CdpError> {
        self.stream
            .write_all(&client_frame(0x1, text.as_bytes()))
            .map_err(|_| CdpError::typed("unsupported", "CDP websocket frame write failed"))
    }

    fn recv_text(&mut self, timeout: Duration) -> Result<Option<String>, CdpError> {
        // Continuation frames are assembled; control frames are answered
        // or dropped in place.
        let mut message: Vec<u8> = Vec::new();
        let mut in_text = false;
        loop {
            self.stream
                .set_read_timeout(Some(timeout.max(Duration::from_millis(1))))
                .ok();
            let mut hdr = [0u8; 2];
            match self.stream.read_exact(&mut hdr) {
                Ok(()) => {}
                Err(error)
                    if !in_text
                        && message.is_empty()
                        && matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                {
                    return Ok(None);
                }
                Err(error) => {
                    return Err(CdpError::typed(
                        "unsupported",
                        format!("CDP websocket frame header missing: {error}"),
                    ));
                }
            }
            let fin = hdr[0] & 0x80 != 0;
            let opcode = hdr[0] & 0x0f;
            let mut len = (hdr[1] & 0x7f) as usize;
            if len == 126 {
                let mut ext = [0u8; 2];
                self.read_exact_timed(&mut ext, "length")?;
                len = u16::from_be_bytes(ext) as usize;
            } else if len == 127 {
                let mut ext = [0u8; 8];
                self.read_exact_timed(&mut ext, "length")?;
                let wide = u64::from_be_bytes(ext);
                if wide > MAX_MESSAGE_BYTES as u64 {
                    return Err(CdpError::typed(
                        "unsupported",
                        format!("CDP websocket frame of {wide} bytes exceeds {MAX_MESSAGE_BYTES}"),
                    ));
                }
                len = wide as usize;
            }
            if message.len() + len > MAX_MESSAGE_BYTES {
                return Err(CdpError::typed(
                    "unsupported",
                    format!("CDP websocket message exceeds {MAX_MESSAGE_BYTES} bytes"),
                ));
            }
            let masked = hdr[1] & 0x80 != 0;
            let mut mask = [0u8; 4];
            if masked {
                self.read_exact_timed(&mut mask, "mask")?;
            }
            let mut payload = vec![0u8; len];
            self.read_exact_timed(&mut payload, "payload")?;
            if masked {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= mask[i % 4];
                }
            }
            match opcode {
                0x1 => {
                    in_text = true;
                    message.extend_from_slice(&payload);
                }
                0x0 if in_text => message.extend_from_slice(&payload),
                0x2 => {
                    return Err(CdpError::typed(
                        "unsupported",
                        "CDP websocket sent a binary frame",
                    ));
                }
                0x8 => {
                    return Err(CdpError::typed(
                        "unsupported",
                        "CDP websocket closed by the browser",
                    ));
                }
                0x9 => {
                    self.stream
                        .write_all(&client_frame(0xA, &payload))
                        .map_err(|_| CdpError::typed("unsupported", "CDP websocket pong failed"))?;
                    continue;
                }
                0xA => continue,
                _ => {
                    return Err(CdpError::typed(
                        "unsupported",
                        format!("CDP websocket frame opcode {opcode:#x} is not text"),
                    ));
                }
            }
            if fin {
                return String::from_utf8(message).map(Some).map_err(|_| {
                    CdpError::typed("unsupported", "CDP websocket payload is not UTF-8")
                });
            }
        }
    }
}

/// One CDP session: request ids, reply matching, buffered events.
pub struct Session<T: Transport> {
    pub transport: T,
    next_id: u64,
    events: Vec<Value>,
    /// Per-call reply deadline (default `CALL_TIMEOUT`).
    pub call_timeout: Duration,
    /// Bytes of the largest message seen, for the reply's `bytes` fields.
    pub largest_message: usize,
}

impl<T: Transport> Session<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: 1,
            events: Vec::new(),
            call_timeout: CALL_TIMEOUT,
            largest_message: 0,
        }
    }

    /// How many methods this session has sent.
    pub fn calls_made(&self) -> u64 {
        self.next_id - 1
    }

    /// Send `method` and return its `result`. A CDP `error` reply is typed
    /// `cdp_method_failed`; events that arrive first are buffered.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, CdpError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "id": id, "method": method, "params": params }).to_string();
        self.transport.send_text(&request)?;
        let deadline = Instant::now() + self.call_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(self.timeout(method));
            }
            let Some(text) = self.transport.recv_text(remaining)? else {
                return Err(self.timeout(method));
            };
            let message = self.parse(&text)?;
            if message["id"].as_u64() == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(CdpError::method_failed(method, error));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            if message.get("method").is_some() {
                self.events.push(message);
            }
        }
    }

    /// The next `method` event (buffered first, then from the wire) within
    /// `timeout`; `None` when it did not arrive.
    pub fn wait_event(
        &mut self,
        method: &str,
        timeout: Duration,
    ) -> Result<Option<Value>, CdpError> {
        if let Some(pos) = self
            .events
            .iter()
            .position(|event| event["method"] == method)
        {
            return Ok(Some(self.events.remove(pos)));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let Some(text) = self.transport.recv_text(remaining)? else {
                return Ok(None);
            };
            let message = self.parse(&text)?;
            if message["method"] == method {
                return Ok(Some(message));
            }
            if message.get("method").is_some() {
                self.events.push(message);
            }
        }
    }

    /// Every buffered event, in arrival order (drained).
    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }

    fn parse(&mut self, text: &str) -> Result<Value, CdpError> {
        self.largest_message = self.largest_message.max(text.len());
        serde_json::from_str(text)
            .map_err(|_| CdpError::typed("unsupported", "CDP message is not JSON"))
    }

    fn timeout(&self, method: &str) -> CdpError {
        CdpError::typed(
            "cdp_timeout",
            format!(
                "CDP {method} answered nothing within {} ms",
                self.call_timeout.as_millis()
            ),
        )
        .with_detail(json!({ "method": method }))
    }
}

impl<T: Transport> std::fmt::Debug for Session<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("calls_made", &self.calls_made())
            .field("buffered_events", &self.events.len())
            .finish()
    }
}

/// A session over the wire to `ws_url`.
pub fn connect(ws_url: &str) -> Result<Session<TcpTransport>, CdpError> {
    Ok(Session::new(TcpTransport::connect(ws_url)?))
}

#[cfg(test)]
pub(crate) mod fake {
    //! A scripted transport: every request is handed to the script, which
    //! answers with zero or more raw messages (replies and events) that
    //! are queued for the next reads.

    use super::*;
    use std::collections::VecDeque;

    pub type Script = Box<dyn FnMut(u64, &str, &Value) -> Vec<Value>>;

    pub struct FakeTransport {
        pub sent: Vec<Value>,
        queue: VecDeque<String>,
        script: Script,
    }

    impl std::fmt::Debug for FakeTransport {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FakeTransport")
                .field("sent", &self.sent)
                .finish()
        }
    }

    impl FakeTransport {
        pub fn new(script: impl FnMut(u64, &str, &Value) -> Vec<Value> + 'static) -> Self {
            Self {
                sent: Vec::new(),
                queue: VecDeque::new(),
                script: Box::new(script),
            }
        }

        /// The methods sent so far, in order.
        pub fn methods(&self) -> Vec<String> {
            self.sent
                .iter()
                .map(|m| m["method"].as_str().unwrap_or("").to_owned())
                .collect()
        }
    }

    pub fn result(id: u64, result: Value) -> Value {
        json!({ "id": id, "result": result })
    }

    pub fn error(id: u64, code: i64, message: &str) -> Value {
        json!({ "id": id, "error": { "code": code, "message": message } })
    }

    pub fn event(method: &str, params: Value) -> Value {
        json!({ "method": method, "params": params })
    }

    impl Transport for FakeTransport {
        fn send_text(&mut self, text: &str) -> Result<(), CdpError> {
            let request: Value = serde_json::from_str(text).expect("request is JSON");
            let id = request["id"].as_u64().expect("request id");
            let method = request["method"].as_str().expect("method").to_owned();
            let params = request["params"].clone();
            self.sent.push(request);
            for message in (self.script)(id, &method, &params) {
                self.queue.push_back(message.to_string());
            }
            Ok(())
        }

        fn recv_text(&mut self, _timeout: Duration) -> Result<Option<String>, CdpError> {
            Ok(self.queue.pop_front())
        }
    }

    /// A session whose script answers every method with `answer(method,
    /// params)`; `Err(message)` becomes a CDP error reply.
    pub fn session(
        mut answer: impl FnMut(&str, &Value) -> Result<Value, String> + 'static,
    ) -> Session<FakeTransport> {
        Session::new(FakeTransport::new(move |id, method, params| {
            vec![match answer(method, params) {
                Ok(value) => result(id, value),
                Err(message) => error(id, -32000, &message),
            }]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[test]
    fn client_frames_are_masked_with_the_three_length_forms() {
        let short = client_frame(0x1, b"hi");
        assert_eq!(short[0], 0x81);
        assert_eq!(short[1], 0x80 | 2);
        assert_eq!(short.len(), 2 + 4 + 2);
        let medium = client_frame(0x1, &[b'x'; 300]);
        assert_eq!(medium[1], 0x80 | 126);
        assert_eq!(u16::from_be_bytes([medium[2], medium[3]]), 300);
        let long = client_frame(0x1, &[b'x'; 70_000]);
        assert_eq!(long[1], 0x80 | 127);
        assert_eq!(long.len(), 2 + 8 + 4 + 70_000);
        let pong = client_frame(0xA, b"p");
        assert_eq!(pong[0], 0x8A);
        assert_eq!(
            split_ws_url("ws://127.0.0.1:9222/devtools/page/main"),
            Some(("127.0.0.1:9222".into(), "/devtools/page/main".into()))
        );
        assert_eq!(split_ws_url("http://x/y"), None);
    }

    #[test]
    fn calls_are_numbered_and_matched_by_id_with_events_buffered() {
        let mut session = Session::new(FakeTransport::new(|id, method, _| match method {
            "Page.enable" => vec![
                event("Page.frameStartedLoading", json!({ "frameId": "F" })),
                result(id, json!({})),
            ],
            "Runtime.evaluate" => vec![result(id, json!({ "result": { "value": 2 } }))],
            _ => vec![error(id, -32601, "unknown method")],
        }));
        let enabled = session.call("Page.enable", json!({})).expect("enable");
        assert_eq!(enabled, json!({}));
        let two = session
            .call("Runtime.evaluate", json!({ "expression": "1+1" }))
            .expect("evaluate");
        assert_eq!(two["result"]["value"], 2);
        assert_eq!(session.calls_made(), 2);
        assert_eq!(session.transport.sent[0]["id"], 1);
        assert_eq!(session.transport.sent[1]["id"], 2);
        assert_eq!(session.transport.sent[1]["params"]["expression"], "1+1");
        let err = session.call("Nope.nothing", json!({})).expect_err("typed");
        assert_eq!(err.code, "cdp_method_failed");
        assert_eq!(err.failed_method(), Some("Nope.nothing"));
        assert_eq!(err.detail["cdp_code"], -32601);
        assert!(err.message.contains("unknown method"));
        // The event that arrived before the first reply is still there.
        let buffered = session
            .wait_event("Page.frameStartedLoading", Duration::from_millis(1))
            .expect("buffered")
            .expect("event");
        assert_eq!(buffered["params"]["frameId"], "F");
        assert!(session.take_events().is_empty());
    }

    #[test]
    fn wait_event_reads_past_unrelated_events_and_reports_absence() {
        let mut session = Session::new(FakeTransport::new(|id, method, _| match method {
            "Page.navigate" => vec![
                result(id, json!({ "frameId": "F" })),
                event("Page.frameNavigated", json!({})),
                event("Page.loadEventFired", json!({ "timestamp": 1.0 })),
            ],
            _ => vec![result(id, json!({}))],
        }));
        session
            .call("Page.navigate", json!({ "url": "about:blank" }))
            .expect("navigate");
        let loaded = session
            .wait_event("Page.loadEventFired", Duration::from_millis(5))
            .expect("read")
            .expect("load event");
        assert_eq!(loaded["params"]["timestamp"], 1.0);
        assert_eq!(
            session.take_events().len(),
            1,
            "frameNavigated stays buffered"
        );
        assert!(
            session
                .wait_event("Page.loadEventFired", Duration::from_millis(1))
                .expect("read")
                .is_none()
        );
    }

    #[test]
    fn a_silent_transport_is_a_typed_timeout_not_a_hang() {
        let mut session = Session::new(FakeTransport::new(|_, _, _| Vec::new()));
        session.call_timeout = Duration::from_millis(1);
        let err = session
            .call("Runtime.evaluate", json!({}))
            .expect_err("timeout");
        assert_eq!(err.code, "cdp_timeout");
        assert_eq!(err.detail["method"], "Runtime.evaluate");
    }

    #[test]
    fn missing_listener_is_typed_at_connect() {
        let err = TcpTransport::connect("ws://127.0.0.1:1/devtools/page/x").expect_err("port 1");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("connect failed"));
        assert_eq!(
            TcpTransport::connect("wss://x/y").expect_err("scheme").code,
            "unsupported"
        );
    }
}
