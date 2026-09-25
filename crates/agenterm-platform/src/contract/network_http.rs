//! Product-neutral outbound HTTP/HTTPS request contract.
//!
//! Types here name only bytes, strings, status codes and durations. No OS
//! type, no `ureq` type and no product policy appears in this module, so a
//! consumer can describe a request and interpret a failure without linking a
//! transport.

use std::time::Duration;

/// Wall-clock ceiling for one whole request/response exchange.
///
/// 0.1.9's `rh.http.*` catalog capped this at 10s (`MAX_HTTP_TIMEOUT` in
/// `src/script_catalog.rs`). That is a **deliberate divergence**: 10s bounds a
/// script's reachability probe, but a non-streaming model completion of a few
/// thousand tokens routinely takes minutes, so a 10s ceiling would not bound
/// the call — it would guarantee it fails. 600s keeps the exchange finite and
/// cancellable-by-deadline while covering a slow generation. The *default*
/// stays at 0.1.9's 2s, so a probe-shaped caller sees no behavior change.
pub const NETWORK_HTTP_MAX_TIMEOUT: Duration = Duration::from_secs(600);

/// Default exchange timeout, unchanged from 0.1.9's `DEFAULT_HTTP_TIMEOUT`.
pub const NETWORK_HTTP_DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// Ceiling on retained response-body bytes.
///
/// 0.1.9 capped this at 256 KiB (`MAX_HTTP_BODY_BYTES`). That is the second
/// **deliberate divergence**: a model API response carries a full completion
/// plus per-token metadata and JSON escaping, and a long tool-calling answer
/// exceeds 256 KiB well before it is unusual. 8 MiB was chosen as roughly an
/// order of magnitude above the largest plausible single non-streaming
/// completion, while remaining one bounded allocation that cannot exhaust the
/// 8 GB developer host this repository targets (see `.cargo/config.toml`). The
/// *default* stays at 0.1.9's 64 KiB (`DEFAULT_HTTP_BODY_BYTES`), so callers
/// must opt in to the larger bound.
pub const NETWORK_HTTP_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Default retained response-body bytes, unchanged from 0.1.9.
pub const NETWORK_HTTP_DEFAULT_RESPONSE_BYTES: usize = 64 * 1024;

/// Ceiling on request-body bytes, unchanged from 0.1.9's
/// `MAX_HTTP_REQUEST_BODY_BYTES`. Not diverged: only the response ceiling and
/// the timeout were authorized to move.
pub const NETWORK_HTTP_MAX_REQUEST_BODY_BYTES: usize = 256 * 1024;

/// Maximum caller-supplied request headers, unchanged from 0.1.9.
pub const NETWORK_HTTP_MAX_HEADERS: usize = 64;

/// Maximum total request-header bytes, unchanged from 0.1.9.
pub const NETWORK_HTTP_MAX_HEADER_BYTES: usize = 32 * 1024;

/// Maximum request URL length in bytes, unchanged from 0.1.9.
pub const NETWORK_HTTP_MAX_URL_BYTES: usize = 8 * 1024;

/// Default redirect budget, unchanged from 0.1.9.
pub const NETWORK_HTTP_DEFAULT_REDIRECTS: u32 = 5;

/// Maximum redirect budget, unchanged from 0.1.9.
pub const NETWORK_HTTP_MAX_REDIRECTS: u32 = 10;

/// The request methods this capability accepts. An unmodelled method is a
/// typed rejection rather than a pass-through string.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NetworkHttpMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}

impl NetworkHttpMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
        }
    }

    /// Whether this method may carry a request body.
    pub const fn allows_body(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch | Self::Delete)
    }
}

/// One fully described outbound exchange. Every bound is part of the request,
/// so validation is a pure function of this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkHttpRequest {
    pub method: NetworkHttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout: Duration,
    pub max_redirects: u32,
    pub max_response_bytes: usize,
}

impl NetworkHttpRequest {
    /// A request carrying the 0.1.9 defaults, not the ceilings.
    pub fn new(method: NetworkHttpMethod, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout: NETWORK_HTTP_DEFAULT_TIMEOUT,
            max_redirects: NETWORK_HTTP_DEFAULT_REDIRECTS,
            max_response_bytes: NETWORK_HTTP_DEFAULT_RESPONSE_BYTES,
        }
    }

    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    #[must_use]
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub const fn max_redirects(mut self, max_redirects: u32) -> Self {
        self.max_redirects = max_redirects;
        self
    }

    #[must_use]
    pub const fn max_response_bytes(mut self, max_response_bytes: usize) -> Self {
        self.max_response_bytes = max_response_bytes;
        self
    }
}

/// The delivered response. `truncated` is explicit: a body cut at the caller's
/// ceiling is never silently presented as complete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub truncated: bool,
}

impl NetworkHttpResponse {
    /// Whether the status line is 2xx. Non-2xx is a delivered response, not an
    /// error: a model API's 429 or 503 carries a body the caller must read.
    pub const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

/// Programmatic failure classes. Every pre-flight rejection happens before any
/// socket is opened.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NetworkHttpErrorKind {
    /// A caller-supplied bound is outside its declared range.
    InvalidLimit,
    /// The URL is empty, over-long, not absolute `http`/`https`, or malformed.
    InvalidUrl,
    /// A header name or value is empty, over-long, or contains control bytes.
    InvalidHeader,
    /// A body was supplied for a method that must not carry one.
    InvalidBody,
    /// The exchange exceeded the caller's timeout.
    Timeout,
    /// The host name did not resolve.
    HostNotFound,
    /// TLS negotiation or certificate verification failed.
    Tls,
    /// The configured proxy is unusable.
    Proxy,
    /// The redirect budget was exhausted, or a redirect could not be followed.
    Redirect,
    /// The peer spoke malformed HTTP.
    Protocol,
    /// Connect, read or write failed at the socket layer.
    Transport,
    /// The response body exceeded the caller's ceiling in a way that could not
    /// be framed as a truncation.
    ResponseTooLarge,
    /// A failure this mapping does not classify. The detail text is the only
    /// information available.
    Unclassified,
}

impl NetworkHttpErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidLimit => "invalid-limit",
            Self::InvalidUrl => "invalid-url",
            Self::InvalidHeader => "invalid-header",
            Self::InvalidBody => "invalid-body",
            Self::Timeout => "timeout",
            Self::HostNotFound => "host-not-found",
            Self::Tls => "tls",
            Self::Proxy => "proxy",
            Self::Redirect => "redirect",
            Self::Protocol => "protocol",
            Self::Transport => "transport",
            Self::ResponseTooLarge => "response-too-large",
            Self::Unclassified => "unclassified",
        }
    }
}

/// A typed kind for programmatic handling **and** the underlying detail text.
///
/// `plan/archive/precision-audit.md` item 70 recorded that the removed 0.1.9
/// implementation collapsed every transport error into a fixed generic code and
/// discarded the underlying `Display` text, which leaves a caller with nothing
/// to diagnose. It also recorded the real reason that was done: some transport
/// error variants echo the request URL, which for an explicit proxy can embed
/// credentials. Both concerns are satisfied here — the detail text is carried,
/// with any absolute URL redacted out of it by the facade.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkHttpError {
    kind: NetworkHttpErrorKind,
    detail: String,
}

impl NetworkHttpError {
    pub(crate) fn new(kind: NetworkHttpErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> NetworkHttpErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for NetworkHttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "network HTTP {}: {}",
            self.kind.as_str(),
            self.detail
        )
    }
}

impl std::error::Error for NetworkHttpError {}
