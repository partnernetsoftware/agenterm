//! The DevTools HTTP endpoint (`/json`, `/json/list`): one bounded,
//! framed GET. Chromium's DevTools server ignores `Connection: close` and
//! keeps the socket open, so the body must be read by `Content-Length` /
//! chunked framing, never to EOF (a read-to-EOF only returns when the
//! read timeout fires -- measured 2026-09-03).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::Value;

use super::{CdpError, MAX_RESULT_BYTES};

pub fn http_get_json(port: u16, path: &str) -> Result<Value, CdpError> {
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).map_err(|_| CdpError::no_listener(port))?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|_| CdpError::typed("unsupported", "CDP HTTP write failed"))?;
    let body = read_http_body(&mut stream, MAX_RESULT_BYTES + 2048).map_err(|reason| {
        CdpError::typed("unsupported", format!("CDP HTTP read failed: {reason}"))
    })?;
    serde_json::from_slice(&body)
        .map_err(|_| CdpError::typed("unsupported", "CDP /json did not return JSON"))
}

/// Read one HTTP/1.1 response body from `stream` without relying on the
/// server closing the socket. Honors `Content-Length`, decodes
/// `Transfer-Encoding: chunked`, and only falls back to read-until-EOF when
/// neither header is present. `limit` bounds the body.
pub fn read_http_body<R: Read>(stream: &mut R, limit: usize) -> Result<Vec<u8>, String> {
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

#[cfg(test)]
mod tests {
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

    #[test]
    fn missing_listener_is_typed_with_the_relaunch_hint() {
        let err = http_get_json(1, "/json").expect_err("port 1 is not CDP");
        assert_eq!(err.code, "unsupported");
        assert!(err.message.contains("remote-debugging-port=1"));
        assert_eq!(err.detail["backend"], "debugger-runtime-evaluate");
    }
}
