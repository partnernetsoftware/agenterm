//! Bounded outbound HTTP/HTTPS client capability.
//!
//! # Why this capability has no per-OS adapter
//!
//! Every other `network-*` capability in this crate reads native host state and
//! therefore owns `adapters/{windows,linux,macos}` files plus a `selected.rs`
//! arm. This one deliberately does **not**, and its absence is not an
//! oversight. `ureq` is a portable pure-Rust client; the only difference
//! between the hosts is which TLS provider verifies the peer, and that
//! difference is expressed entirely as target-specific Cargo features in this
//! crate's manifest — Windows `native-tls` (SChannel/platform roots), Unix
//! `rustls` (WebPKI), both with `default-features = false`, exactly as 0.1.9
//! shipped it and as `prd/PRD_02_20_native_platform.md` records under
//! "target-specific ureq feature trees". There is no OS call for an adapter to
//! own, so only the contract and facade halves of the house pattern exist here.
//! Adding an adapter layer would mean three identical files selecting nothing.
//!
//! # Shape
//!
//! Every caller-supplied bound is validated before a socket is opened. A
//! non-2xx status is a delivered response, not an error — a model API's 429 or
//! 503 carries a body the caller must read. Response bodies are read through a
//! bounded reader that reports truncation explicitly rather than either
//! allocating without limit or presenting a cut body as complete.

use std::io::Read;

pub use crate::contract::network_http::{
    NETWORK_HTTP_DEFAULT_REDIRECTS, NETWORK_HTTP_DEFAULT_RESPONSE_BYTES,
    NETWORK_HTTP_DEFAULT_TIMEOUT, NETWORK_HTTP_MAX_HEADER_BYTES, NETWORK_HTTP_MAX_HEADERS,
    NETWORK_HTTP_MAX_REDIRECTS, NETWORK_HTTP_MAX_REQUEST_BODY_BYTES,
    NETWORK_HTTP_MAX_RESPONSE_BYTES, NETWORK_HTTP_MAX_TIMEOUT, NETWORK_HTTP_MAX_URL_BYTES,
    NetworkHttpError, NetworkHttpErrorKind, NetworkHttpMethod, NetworkHttpRequest,
    NetworkHttpResponse,
};

/// Longest single header name or value this capability accepts. Derived from
/// the total header budget rather than invented: one header may not consume the
/// whole allowance.
const MAX_SINGLE_HEADER_FIELD_BYTES: usize = 8 * 1024;

fn error(kind: NetworkHttpErrorKind, detail: impl Into<String>) -> NetworkHttpError {
    NetworkHttpError::new(kind, detail)
}

/// Check every caller-supplied bound and the request shape. This is a pure
/// function: it opens no socket and resolves no name, so a caller can reject a
/// malformed request without network access and a test can prove each bound in
/// isolation.
pub fn validate(request: &NetworkHttpRequest) -> Result<(), NetworkHttpError> {
    validate_url(&request.url)?;
    validate_headers(&request.headers)?;

    if !request.body.is_empty() && !request.method.allows_body() {
        return Err(error(
            NetworkHttpErrorKind::InvalidBody,
            format!(
                "{} must not carry a request body, got {} bytes",
                request.method.as_str(),
                request.body.len()
            ),
        ));
    }
    if request.body.len() > NETWORK_HTTP_MAX_REQUEST_BODY_BYTES {
        return Err(error(
            NetworkHttpErrorKind::InvalidLimit,
            format!(
                "request body of {} bytes exceeds {NETWORK_HTTP_MAX_REQUEST_BODY_BYTES}",
                request.body.len()
            ),
        ));
    }
    if request.timeout.is_zero() || request.timeout > NETWORK_HTTP_MAX_TIMEOUT {
        return Err(error(
            NetworkHttpErrorKind::InvalidLimit,
            format!(
                "timeout must be above zero and at most {}s, got {}s",
                NETWORK_HTTP_MAX_TIMEOUT.as_secs(),
                request.timeout.as_secs()
            ),
        ));
    }
    if request.max_redirects > NETWORK_HTTP_MAX_REDIRECTS {
        return Err(error(
            NetworkHttpErrorKind::InvalidLimit,
            format!(
                "max_redirects must be at most {NETWORK_HTTP_MAX_REDIRECTS}, got {}",
                request.max_redirects
            ),
        ));
    }
    if !(1..=NETWORK_HTTP_MAX_RESPONSE_BYTES).contains(&request.max_response_bytes) {
        return Err(error(
            NetworkHttpErrorKind::InvalidLimit,
            format!(
                "max_response_bytes must be between 1 and {NETWORK_HTTP_MAX_RESPONSE_BYTES}, got {}",
                request.max_response_bytes
            ),
        ));
    }
    Ok(())
}

fn validate_url(url: &str) -> Result<(), NetworkHttpError> {
    if url.is_empty() {
        return Err(error(NetworkHttpErrorKind::InvalidUrl, "URL is empty"));
    }
    if url.len() > NETWORK_HTTP_MAX_URL_BYTES {
        return Err(error(
            NetworkHttpErrorKind::InvalidUrl,
            format!(
                "URL of {} bytes exceeds {NETWORK_HTTP_MAX_URL_BYTES}",
                url.len()
            ),
        ));
    }
    if url.bytes().any(|byte| byte <= 0x20 || byte == 0x7f) {
        return Err(error(
            NetworkHttpErrorKind::InvalidUrl,
            "URL contains whitespace or control bytes",
        ));
    }
    let authority = match url.split_once("://") {
        Some(("http", rest)) | Some(("https", rest)) => rest,
        Some((scheme, _)) => {
            return Err(error(
                NetworkHttpErrorKind::InvalidUrl,
                format!("scheme {scheme:?} is not http or https"),
            ));
        }
        None => {
            return Err(error(
                NetworkHttpErrorKind::InvalidUrl,
                "URL is not absolute with an http or https scheme",
            ));
        }
    };
    let host = authority
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    if host.is_empty() {
        return Err(error(NetworkHttpErrorKind::InvalidUrl, "URL has no host"));
    }
    Ok(())
}

fn validate_headers(headers: &[(String, String)]) -> Result<(), NetworkHttpError> {
    if headers.len() > NETWORK_HTTP_MAX_HEADERS {
        return Err(error(
            NetworkHttpErrorKind::InvalidLimit,
            format!(
                "{} request headers exceed {NETWORK_HTTP_MAX_HEADERS}",
                headers.len()
            ),
        ));
    }
    let mut total = 0usize;
    for (name, value) in headers {
        if name.is_empty() {
            return Err(error(
                NetworkHttpErrorKind::InvalidHeader,
                "header name is empty",
            ));
        }
        if name.len() > MAX_SINGLE_HEADER_FIELD_BYTES || value.len() > MAX_SINGLE_HEADER_FIELD_BYTES
        {
            return Err(error(
                NetworkHttpErrorKind::InvalidHeader,
                format!("header {name:?} exceeds {MAX_SINGLE_HEADER_FIELD_BYTES} bytes"),
            ));
        }
        // Reject CR, LF, NUL and every other control byte before the transport
        // sees them: a header value carrying CRLF is request splitting.
        if name
            .bytes()
            .chain(value.bytes())
            .any(|byte| byte < 0x20 || byte == 0x7f)
        {
            return Err(error(
                NetworkHttpErrorKind::InvalidHeader,
                format!("header {name:?} contains control bytes"),
            ));
        }
        total = total.saturating_add(name.len() + value.len() + 4);
    }
    if total > NETWORK_HTTP_MAX_HEADER_BYTES {
        return Err(error(
            NetworkHttpErrorKind::InvalidLimit,
            format!(
                "request headers total {total} bytes, exceeding {NETWORK_HTTP_MAX_HEADER_BYTES}"
            ),
        ));
    }
    Ok(())
}

/// Perform one bounded exchange.
///
/// Validation runs first and completely; nothing below is reached by an invalid
/// request.
pub fn request(request: &NetworkHttpRequest) -> Result<NetworkHttpResponse, NetworkHttpError> {
    validate(request)?;

    let config = ureq::Agent::config_builder()
        .timeout_global(Some(request.timeout))
        .max_redirects(request.max_redirects)
        .max_redirects_will_error(true)
        // A 4xx/5xx is a delivered response this capability hands back with its
        // body, not a transport failure. Product policy interprets the status.
        .http_status_as_error(false)
        .build();
    let agent: ureq::Agent = config.into();

    let mut builder = ureq::http::Request::builder()
        .method(request.method.as_str())
        .uri(request.url.as_str());
    for (name, value) in &request.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let wire = builder
        .body(request.body.as_slice())
        .map_err(|failure| map_http_error(&failure))?;

    let mut response = agent
        .run(wire)
        .map_err(|failure| map_ureq_error(&failure))?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let (body, truncated) = read_bounded(
        &mut response.body_mut().as_reader(),
        request.max_response_bytes,
    )?;
    Ok(NetworkHttpResponse {
        status,
        headers,
        body,
        truncated,
    })
}

/// Read at most `ceiling` bytes, reporting whether the peer had more to send.
///
/// The body is framed by this ceiling, not by the peer closing the socket:
/// `docs/agenterm-rust-casebook.md` records that a `read_to_end` client only
/// terminates when the server closes, so every bounded read here stops at the
/// ceiling and says so. One extra byte is probed past the ceiling purely to
/// distinguish "exactly `ceiling` bytes" from "truncated".
fn read_bounded(
    reader: &mut impl Read,
    ceiling: usize,
) -> Result<(Vec<u8>, bool), NetworkHttpError> {
    let mut body = Vec::new();
    let mut chunk = [0u8; 8 * 1024];
    while body.len() <= ceiling {
        let want = (ceiling + 1 - body.len()).min(chunk.len());
        let read = reader
            .read(&mut chunk[..want])
            .map_err(|failure| map_body_read_error(&failure))?;
        if read == 0 {
            return Ok((body, false));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(ceiling);
    Ok((body, true))
}

/// Classify a failure that happened while streaming the body.
///
/// `agent.run` returns once the headers are in, so the response body is read
/// after that call has already succeeded and its failures arrive here as
/// `io::Error` rather than as a `ureq::Error`. A deadline that expires mid-body
/// is therefore easy to mislabel: it is the same finished timeout the caller
/// would have seen before the headers, and reporting it as `Transport` would
/// tell a caller matching on `kind()` that a spent deadline was a transient
/// socket failure worth retrying. So the io error kind is inspected, and a
/// `ureq::Error` that the transport wrapped in an `io::Error` is unwrapped and
/// classified by the same table the pre-header path uses.
fn map_body_read_error(failure: &std::io::Error) -> NetworkHttpError {
    if let Some(wrapped) = failure
        .get_ref()
        .and_then(|source| source.downcast_ref::<ureq::Error>())
    {
        return map_ureq_error(wrapped);
    }
    let kind = match failure.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            NetworkHttpErrorKind::Timeout
        }
        _ => NetworkHttpErrorKind::Transport,
    };
    error(
        kind,
        redact_detail(&format!("response body read failed: {failure}")),
    )
}

fn map_http_error(failure: &ureq::http::Error) -> NetworkHttpError {
    // `http::Error` at this point can only come from the method, URI or a
    // header field, all of which validation already accepted in shape; a
    // remaining rejection is the `http` crate's own stricter grammar.
    error(
        NetworkHttpErrorKind::InvalidHeader,
        redact_detail(&failure.to_string()),
    )
}

/// Classify a transport failure and carry its own text.
///
/// The `kind` is for programs; the detail is for humans. See the contract's
/// `NetworkHttpError` docs for why both exist and why the detail is redacted.
fn map_ureq_error(failure: &ureq::Error) -> NetworkHttpError {
    let detail = redact_detail(&failure.to_string());
    let kind = match failure {
        ureq::Error::Timeout(_) => NetworkHttpErrorKind::Timeout,
        ureq::Error::HostNotFound => NetworkHttpErrorKind::HostNotFound,
        ureq::Error::Tls(_) => NetworkHttpErrorKind::Tls,
        ureq::Error::InvalidProxyUrl => NetworkHttpErrorKind::Proxy,
        ureq::Error::TooManyRedirects | ureq::Error::RedirectFailed => {
            NetworkHttpErrorKind::Redirect
        }
        ureq::Error::Protocol(_) => NetworkHttpErrorKind::Protocol,
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed => NetworkHttpErrorKind::Transport,
        ureq::Error::BadUri(_) => NetworkHttpErrorKind::InvalidUrl,
        ureq::Error::Http(_) => NetworkHttpErrorKind::InvalidHeader,
        ureq::Error::BodyExceedsLimit(_) => NetworkHttpErrorKind::ResponseTooLarge,
        _ => NetworkHttpErrorKind::Unclassified,
    };
    error(kind, detail)
}

/// Remove absolute URLs from a failure's own text.
///
/// `plan/archive/precision-audit.md` item 70 recorded that some transport error
/// variants echo the request URL, which for an explicit proxy can embed
/// credentials. That is the one thing worth dropping; the rest of the text is
/// the diagnostic value the same item said had been thrown away.
fn redact_detail(detail: &str) -> String {
    let mut out = String::with_capacity(detail.len());
    let mut rest = detail;
    while let Some(position) = rest.find("://") {
        let head = &rest[..position];
        let scheme_start = head
            .rfind(|character: char| !character.is_ascii_alphanumeric() && character != '+')
            .map_or(0, |index| index + character_width(head, index));
        out.push_str(&head[..scheme_start]);
        out.push_str("<URL>");
        let tail = &rest[position + 3..];
        let end = tail
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '"' | '\'' | ')' | ',')
            })
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Byte width of the character starting at `index` in `text`.
fn character_width(text: &str, index: usize) -> usize {
    text[index..]
        .chars()
        .next()
        .map_or(1, |character| character.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    fn get(url: &str) -> NetworkHttpRequest {
        NetworkHttpRequest::new(NetworkHttpMethod::Get, url)
    }

    fn valid() -> NetworkHttpRequest {
        get("http://127.0.0.1:1/")
    }

    #[test]
    fn accepts_a_request_at_its_defaults() {
        let request = valid();
        assert_eq!(request.timeout, NETWORK_HTTP_DEFAULT_TIMEOUT);
        assert_eq!(request.max_redirects, NETWORK_HTTP_DEFAULT_REDIRECTS);
        assert_eq!(
            request.max_response_bytes,
            NETWORK_HTTP_DEFAULT_RESPONSE_BYTES
        );
        assert_eq!(validate(&request), Ok(()));
    }

    #[test]
    fn rejects_zero_and_over_ceiling_timeouts() {
        assert_eq!(
            validate(&valid().timeout(Duration::ZERO))
                .unwrap_err()
                .kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        assert_eq!(
            validate(&valid().timeout(NETWORK_HTTP_MAX_TIMEOUT + Duration::from_secs(1)))
                .unwrap_err()
                .kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        assert_eq!(validate(&valid().timeout(NETWORK_HTTP_MAX_TIMEOUT)), Ok(()));
    }

    #[test]
    fn rejects_zero_and_over_ceiling_response_bounds() {
        assert_eq!(
            validate(&valid().max_response_bytes(0)).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        assert_eq!(
            validate(&valid().max_response_bytes(NETWORK_HTTP_MAX_RESPONSE_BYTES + 1))
                .unwrap_err()
                .kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        assert_eq!(
            validate(&valid().max_response_bytes(NETWORK_HTTP_MAX_RESPONSE_BYTES)),
            Ok(())
        );
    }

    #[test]
    fn rejects_an_over_ceiling_redirect_budget() {
        assert_eq!(
            validate(&valid().max_redirects(NETWORK_HTTP_MAX_REDIRECTS + 1))
                .unwrap_err()
                .kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        assert_eq!(
            validate(&valid().max_redirects(NETWORK_HTTP_MAX_REDIRECTS)),
            Ok(())
        );
        // Zero redirects is a legitimate "do not follow" request, not a bound
        // violation.
        assert_eq!(validate(&valid().max_redirects(0)), Ok(()));
    }

    #[test]
    fn rejects_more_headers_than_the_bound() {
        let mut request = valid();
        for index in 0..=NETWORK_HTTP_MAX_HEADERS {
            request = request.header(format!("x-probe-{index}"), "v");
        }
        assert_eq!(request.headers.len(), NETWORK_HTTP_MAX_HEADERS + 1);
        assert_eq!(
            validate(&request).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
        request.headers.pop();
        assert_eq!(validate(&request), Ok(()));
    }

    #[test]
    fn rejects_headers_totalling_more_than_the_byte_bound() {
        let mut request = valid();
        // Eight 8 KiB values are 64 KiB, over the 32 KiB total, while each
        // single field stays inside its own per-field bound.
        for index in 0..8 {
            request = request.header(
                format!("x-bulk-{index}"),
                "v".repeat(MAX_SINGLE_HEADER_FIELD_BYTES),
            );
        }
        assert_eq!(
            validate(&request).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
    }

    #[test]
    fn rejects_control_bytes_and_empty_names_in_headers() {
        assert_eq!(
            validate(&valid().header("x-split", "a\r\nInjected: 1"))
                .unwrap_err()
                .kind(),
            NetworkHttpErrorKind::InvalidHeader
        );
        assert_eq!(
            validate(&valid().header("x-nul\0", "v"))
                .unwrap_err()
                .kind(),
            NetworkHttpErrorKind::InvalidHeader
        );
        assert_eq!(
            validate(&valid().header("", "v")).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidHeader
        );
        assert_eq!(
            validate(&valid().header("x-long", "v".repeat(MAX_SINGLE_HEADER_FIELD_BYTES + 1)))
                .unwrap_err()
                .kind(),
            NetworkHttpErrorKind::InvalidHeader
        );
    }

    #[test]
    fn rejects_urls_that_are_empty_relative_over_long_or_wrongly_schemed() {
        assert_eq!(
            validate(&get("")).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidUrl
        );
        assert_eq!(
            validate(&get("/relative")).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidUrl
        );
        assert_eq!(
            validate(&get("file:///etc/hosts")).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidUrl
        );
        assert_eq!(
            validate(&get("http:///nohost")).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidUrl
        );
        assert_eq!(
            validate(&get("http://127.0.0.1/ space"))
                .unwrap_err()
                .kind(),
            NetworkHttpErrorKind::InvalidUrl
        );
        let long = format!(
            "http://127.0.0.1/{}",
            "p".repeat(NETWORK_HTTP_MAX_URL_BYTES)
        );
        assert!(long.len() > NETWORK_HTTP_MAX_URL_BYTES);
        assert_eq!(
            validate(&get(&long)).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidUrl
        );
        assert_eq!(validate(&get("https://localhost:8443/v1/x?q=1")), Ok(()));
    }

    #[test]
    fn rejects_a_body_on_a_method_that_cannot_carry_one() {
        assert_eq!(
            validate(&get("http://127.0.0.1:1/").body(b"data".to_vec()))
                .unwrap_err()
                .kind(),
            NetworkHttpErrorKind::InvalidBody
        );
        assert_eq!(
            validate(
                &NetworkHttpRequest::new(NetworkHttpMethod::Post, "http://127.0.0.1:1/")
                    .body(b"data".to_vec())
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_a_request_body_over_the_ceiling() {
        let request = NetworkHttpRequest::new(NetworkHttpMethod::Post, "http://127.0.0.1:1/")
            .body(vec![b'x'; NETWORK_HTTP_MAX_REQUEST_BODY_BYTES + 1]);
        assert_eq!(
            validate(&request).unwrap_err().kind(),
            NetworkHttpErrorKind::InvalidLimit
        );
    }

    #[test]
    fn request_rejects_bounds_before_touching_the_network() {
        // Port 1 on loopback has no listener, so reaching the transport at all
        // would produce a Transport error instead of InvalidLimit.
        let failure = request(&valid().timeout(Duration::ZERO)).unwrap_err();
        assert_eq!(failure.kind(), NetworkHttpErrorKind::InvalidLimit);
    }

    #[test]
    fn reads_a_body_shorter_than_the_ceiling_untruncated() {
        let mut source = Cursor::new(b"hello".to_vec());
        assert_eq!(
            read_bounded(&mut source, 64).unwrap(),
            (b"hello".to_vec(), false)
        );
    }

    #[test]
    fn frames_a_body_exactly_at_the_ceiling_without_claiming_truncation() {
        let mut source = Cursor::new(vec![b'a'; 32]);
        let (body, truncated) = read_bounded(&mut source, 32).unwrap();
        assert_eq!(body.len(), 32);
        assert!(!truncated);
    }

    #[test]
    fn truncates_a_body_over_the_ceiling_and_reports_it() {
        let mut source = Cursor::new(vec![b'a'; 4096]);
        let (body, truncated) = read_bounded(&mut source, 100).unwrap();
        assert_eq!(body.len(), 100);
        assert!(truncated);
    }

    /// A reader whose every `read` fails with a caller-chosen io error kind,
    /// standing in for a deadline or a socket dying mid-body.
    struct FailingReader(std::io::ErrorKind);

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(self.0, "body stalled at byte 0"))
        }
    }

    #[test]
    fn a_deadline_that_expires_mid_body_is_a_timeout_not_a_transport_failure() {
        // `agent.run` has already returned by the time the body is read, so a
        // spent deadline arrives here as an io error. Reporting it as Transport
        // would tell a caller matching on kind() to retry a finished deadline.
        for kind in [std::io::ErrorKind::TimedOut, std::io::ErrorKind::WouldBlock] {
            let mut source = FailingReader(kind);
            let failure = read_bounded(&mut source, 100).unwrap_err();
            assert_eq!(failure.kind(), NetworkHttpErrorKind::Timeout);
            assert!(
                failure.detail().contains("body stalled at byte 0"),
                "the underlying text must survive: {}",
                failure.detail()
            );
        }
    }

    #[test]
    fn a_broken_socket_mid_body_stays_a_transport_failure() {
        let mut source = FailingReader(std::io::ErrorKind::ConnectionReset);
        let failure = read_bounded(&mut source, 100).unwrap_err();
        assert_eq!(failure.kind(), NetworkHttpErrorKind::Transport);
        assert!(failure.detail().contains("body stalled at byte 0"));
    }

    #[test]
    fn error_kinds_carry_stable_strings_and_detail_text() {
        assert_eq!(NetworkHttpErrorKind::Timeout.as_str(), "timeout");
        assert_eq!(NetworkHttpErrorKind::Tls.as_str(), "tls");
        let failure = error(NetworkHttpErrorKind::Transport, "socket refused");
        assert_eq!(failure.kind(), NetworkHttpErrorKind::Transport);
        assert_eq!(failure.detail(), "socket refused");
        assert_eq!(
            failure.to_string(),
            "network HTTP transport: socket refused"
        );
    }

    #[test]
    fn maps_each_ureq_variant_to_its_own_kind_and_keeps_the_detail() {
        let cases: Vec<(ureq::Error, NetworkHttpErrorKind)> = vec![
            (
                ureq::Error::HostNotFound,
                NetworkHttpErrorKind::HostNotFound,
            ),
            (ureq::Error::Tls("handshake"), NetworkHttpErrorKind::Tls),
            (ureq::Error::InvalidProxyUrl, NetworkHttpErrorKind::Proxy),
            (
                ureq::Error::TooManyRedirects,
                NetworkHttpErrorKind::Redirect,
            ),
            (ureq::Error::RedirectFailed, NetworkHttpErrorKind::Redirect),
            (
                ureq::Error::ConnectionFailed,
                NetworkHttpErrorKind::Transport,
            ),
            (
                ureq::Error::Io(std::io::Error::other("broken pipe")),
                NetworkHttpErrorKind::Transport,
            ),
            (
                ureq::Error::BadUri("nope".into()),
                NetworkHttpErrorKind::InvalidUrl,
            ),
            (
                ureq::Error::BodyExceedsLimit(9),
                NetworkHttpErrorKind::ResponseTooLarge,
            ),
        ];
        for (failure, expected) in cases {
            let display = failure.to_string();
            let mapped = map_ureq_error(&failure);
            assert_eq!(mapped.kind(), expected, "variant {display:?}");
            // The recorded lesson: the underlying text is carried, not dropped
            // for a fixed generic code.
            assert!(
                !mapped.detail().is_empty(),
                "variant {display:?} lost its detail"
            );
        }
        // One variant carries its own text verbatim, proving pass-through.
        assert!(
            map_ureq_error(&ureq::Error::Tls("certificate expired"))
                .detail()
                .contains("certificate expired")
        );
    }

    #[test]
    fn redacts_absolute_urls_but_keeps_the_rest_of_the_detail() {
        assert_eq!(
            redact_detail("connect failed for http://user:<TOKEN>@proxy.invalid:8080/x now"),
            "connect failed for <URL> now"
        );
        assert_eq!(
            redact_detail("io error: connection reset by peer"),
            "io error: connection reset by peer"
        );
        assert!(!redact_detail("tls error on https://host.invalid/v1").contains("host.invalid"));
    }

    /// Serve one canned HTTP/1.1 response on loopback and return its port.
    ///
    /// The response is framed with `Content-Length`, which is the point: a
    /// client that needed the socket closed to finish would hang here. The
    /// listener keeps the connection open after writing, exactly as the
    /// casebook's DevTools case did.
    fn serve_once(response: Vec<u8>) -> (u16, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let handle = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 4096];
            // Read the request head; stop at the blank line.
            let mut seen = Vec::new();
            loop {
                match stream.read(&mut request) {
                    Ok(0) => break,
                    Ok(read) => {
                        seen.extend_from_slice(&request[..read]);
                        if seen.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = stream.write_all(&response);
            let _ = stream.flush();
            // Hold the socket briefly so the client cannot depend on EOF.
            std::thread::sleep(Duration::from_millis(50));
        });
        (port, handle)
    }

    fn framed(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn performs_a_loopback_request_and_returns_a_framed_body() {
        let (port, handle) = serve_once(framed("pong"));
        let outcome = request(
            &get(&format!("http://127.0.0.1:{port}/probe"))
                .header("x-probe", "1")
                .timeout(Duration::from_secs(5)),
        )
        .expect("loopback exchange");
        handle.join().ok();
        assert_eq!(outcome.status, 200);
        assert!(outcome.is_success());
        assert_eq!(outcome.body, b"pong");
        assert!(!outcome.truncated);
        assert!(
            outcome
                .headers
                .iter()
                .any(|(name, value)| name == "content-type" && value == "text/plain")
        );
    }

    #[test]
    fn truncates_a_loopback_body_at_the_callers_ceiling() {
        let (port, handle) = serve_once(framed(&"z".repeat(4096)));
        let outcome = request(
            &get(&format!("http://127.0.0.1:{port}/big"))
                .timeout(Duration::from_secs(5))
                .max_response_bytes(256),
        )
        .expect("loopback exchange");
        handle.join().ok();
        assert_eq!(outcome.body.len(), 256);
        assert!(outcome.truncated);
    }

    #[test]
    fn delivers_a_non_2xx_status_as_a_response_not_an_error() {
        let body = "{\"error\":\"rate limited\"}";
        let response = format!(
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes();
        let (port, handle) = serve_once(response);
        let outcome = request(
            &get(&format!("http://127.0.0.1:{port}/limited")).timeout(Duration::from_secs(5)),
        )
        .expect("429 is a delivered response");
        handle.join().ok();
        assert_eq!(outcome.status, 429);
        assert!(!outcome.is_success());
        assert_eq!(outcome.body, body.as_bytes());
    }

    #[test]
    fn reports_a_closed_loopback_port_as_a_transport_failure() {
        // Bind, learn the port, then drop the listener so nothing is listening.
        let port = {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
            listener.local_addr().expect("local addr").port()
        };
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
        let failure =
            request(&get(&format!("http://127.0.0.1:{port}/gone")).timeout(Duration::from_secs(5)))
                .unwrap_err();
        assert_eq!(failure.kind(), NetworkHttpErrorKind::Transport);
        assert!(!failure.detail().is_empty());
    }
}
