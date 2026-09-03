//! Chromium DevTools Protocol client: the second knife after the AX tree.
//!
//! Never runs page JavaScript in this process (no MAIN-world Function
//! constructor). Needs a listener from `--remote-debugging-port`. That
//! port answers any local process, so a caller should open it only while
//! it is needed.
//!
//! Why CDP at all: macOS Chromium publishes only the active tab's
//! `web-area` in the AX tree, so a background tab in a background window
//! can be read or acted on only through its CDP target. `/json` lists
//! every tab as a `page` target whether or not it is the active one, and
//! every method here runs over that target's own websocket. Nothing in
//! this module calls `Target.activateTarget` or `Page.bringToFront`
//! unless a verb passes `--activate` explicitly; every reply carries
//! `focus_changed` so the caller never has to guess.
//!
//! Layout:
//! - `http`: the `/json` reader (Chromium keeps the socket open, so the
//!   body is framed by `Content-Length` / chunked, never read-to-EOF).
//! - `ws`: one websocket to one target; `Transport` is the seam the
//!   session logic is tested through with fake transcripts.
//! - `targets`: the `/json` inventory and the `--target-id | --target-url
//!   | --target-title` selector (`cdp_target_not_found` /
//!   `cdp_target_ambiguous`).
//! - `evaluate`: `page-js` (`Runtime.evaluate`).
//! - `ax`: pure shaping of `Accessibility.getFullAXTree` into `page text`
//!   rows and `page find` matches.
//! - `page`: the background-tab verbs `text` / `find` / `click` / `fill` /
//!   `nav` / `screenshot` over a session.

pub mod ax;
pub mod evaluate;
pub mod http;
pub mod page;
pub mod targets;
pub mod ws;

use serde_json::{Value, json};

pub use evaluate::evaluate;
pub use targets::{
    PageTarget, TargetSelector, first_page_ws_url, parse_targets, select_target, targets,
    targets_payload,
};
pub use ws::{Session, Transport};

pub const DEFAULT_PORT: u16 = 9222;
pub const MAX_EXPRESSION_BYTES: usize = 4096;
/// Bound on a `page-js` result frame (the older 64 KiB contract).
pub const MAX_RESULT_BYTES: usize = 65_536;

/// The `backend` name every CDP reply / typed error carries in `detail`.
pub fn backend() -> &'static str {
    crate::observe::page_js_backend()
}

pub fn reason() -> &'static str {
    crate::observe::page_js_unsupported_reason()
}

/// A typed CDP failure: `code` is the reply's error code, `detail` always
/// carries the backend name (and `ax_default: true`, because the AX tree
/// stays the first knife).
#[derive(Debug)]
pub struct CdpError {
    pub code: &'static str,
    pub message: String,
    pub detail: Value,
}

/// The older spelling of this type, kept for readers of the `page-js` docs.
pub type PageJsError = CdpError;

impl CdpError {
    pub fn typed(code: &'static str, message: impl Into<String>) -> Self {
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
    pub fn with_detail(mut self, extra: Value) -> Self {
        if let (Some(map), Some(more)) = (self.detail.as_object_mut(), extra.as_object()) {
            for (key, value) in more {
                map.insert(key.clone(), value.clone());
            }
        }
        self
    }

    /// Re-type an error (a `cdp_method_failed` from `Page.captureScreenshot`
    /// becomes `cdp_screenshot_unavailable`) without losing its detail.
    pub fn recode(mut self, code: &'static str, message: impl Into<String>) -> Self {
        self.code = code;
        self.message = message.into();
        self
    }

    /// A CDP `error` reply to `method`: the protocol's own code and message
    /// are kept in `detail` so a caller never parses prose.
    pub fn method_failed(method: &str, error: &Value) -> Self {
        let message = error["message"].as_str().unwrap_or("unknown");
        Self::typed(
            "cdp_method_failed",
            format!("CDP {method} failed: {message}"),
        )
        .with_detail(json!({
            "method": method,
            "cdp_code": error["code"],
            "cdp_message": message,
            "cdp_data": error["data"],
        }))
    }

    /// The CDP method this error came from, when it is a method failure.
    pub fn failed_method(&self) -> Option<&str> {
        if self.code == "cdp_method_failed" {
            self.detail["method"].as_str()
        } else {
            None
        }
    }

    pub fn no_listener(port: u16) -> Self {
        Self::typed(
            "unsupported",
            format!(
                "no CDP listener on 127.0.0.1:{port}; relaunch Chromium with --remote-debugging-port={port}"
            ),
        )
    }
}

impl From<CdpError> for crate::reply::CuError {
    fn from(error: CdpError) -> Self {
        crate::reply::CuError::new(error.code, error.message).with_detail(error.detail)
    }
}
