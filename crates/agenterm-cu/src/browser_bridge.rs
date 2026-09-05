//! ACU-owned Chromium MV3 and Native Messaging bridge contracts.
//!
//! This module contains no installer, daemon or browser-launch policy. The
//! `agenterm-cu` binary is the native host; callers compose these pure pieces.

mod assets;
mod host;
mod installer;
mod registry;

pub use assets::{
    ExtensionAsset, ExtensionMaterializationPlan, MaterializationError, extension_assets,
    native_host_manifest,
};
pub use host::{
    BridgeHostError, BridgeResponse, BridgeStatus, BridgeWireError, ConnectionInventory,
    RequestLedger, list_live_connections, run_native_host, send_to_connection,
};
pub use installer::{
    BrowserBridgeInstall, BrowserBridgeInstallError, BrowserBridgeInstallPaths,
    install_for_current_user,
};
pub use registry::{
    ConnectionEndpoint, ConnectionEntry, ConnectionId, ConnectionRegistry, ProcessIdentity,
    RegistryError, StaleCleanup,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PROTOCOL_VERSION: u32 = 1;
pub const REQUEST_MAX_BYTES: usize = 1024 * 1024;
pub const NATIVE_MESSAGE_MAX_BYTES: usize = REQUEST_MAX_BYTES;
pub const ACU_NATIVE_HOST_NAME: &str = "software.partnernet.agenterm_acu.browser_bridge";
pub const ACU_EXTENSION_ID: &str = "knofdkmmpkbnjhdkcjddbakbpmgpmjpe";
pub const DEBUG_READ_MAX_FRAMES: u16 = 64;
pub const DEBUG_READ_MAX_DEPTH: u8 = 20;
pub const DEBUG_READ_MAX_SCAN: u32 = 5_000;
pub const DEBUG_READ_MAX_RESULTS: u16 = 1_000;
pub const TAB_MAX_RESULTS: usize = 512;
pub const TAB_TITLE_MAX_BYTES: usize = 4 * 1024;
pub const TAB_URL_MAX_BYTES: usize = 8 * 1024;
/// One browser-created host retains at most this many terminal replies for
/// exact replay. With the one-MiB frame ceiling this also bounds replay memory.
pub const REQUEST_LEDGER_MAX_ENTRIES: usize = 32;
const COMMANDS: &[&str] = &["status", "tabs", "debug-read"];

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeRequest {
    pub protocol: u32,
    pub id: String,
    pub command: String,
    pub args: Map<String, Value>,
}

impl BridgeRequest {
    pub fn validate(&self) -> Result<(), BridgeProtocolError> {
        if self.protocol != PROTOCOL_VERSION {
            return Err(BridgeProtocolError::new(
                "browser_bridge_protocol_mismatch",
                "browser bridge protocol version does not match",
            ));
        }
        validate_text(
            &self.id,
            96,
            false,
            "browser_bridge_request_id_invalid",
            "request id",
        )?;
        if !self
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        {
            return Err(BridgeProtocolError::new(
                "browser_bridge_request_id_invalid",
                "browser bridge request id contains a forbidden character",
            ));
        }
        if !COMMANDS.contains(&self.command.as_str()) {
            return Err(BridgeProtocolError::new(
                "browser_bridge_command_unknown",
                "browser bridge command is not in this protocol version",
            ));
        }
        match self.command.as_str() {
            "status" | "tabs" if !self.args.is_empty() => Err(BridgeProtocolError::new(
                "browser_bridge_args_invalid",
                "this command takes an empty args object",
            )),
            "debug-read" => {
                let req: DebugReadRequest =
                    serde_json::from_value(Value::Object(self.args.clone())).map_err(|e| {
                        BridgeProtocolError::new(
                            "browser_bridge_args_invalid",
                            format!("debug-read args are invalid: {e}"),
                        )
                    })?;
                req.validate()
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DebugReadRequest {
    /// Exact Chromium tab id; fuzzy title/URL matching is excluded.
    pub tab_id: u32,
    pub max_frames: u16,
    pub max_depth: u8,
    pub max_scan: u32,
    pub max_results: u16,
}

impl DebugReadRequest {
    pub fn validate(&self) -> Result<(), BridgeProtocolError> {
        bound(self.tab_id, 1, u32::MAX, "tab_id")?;
        bound(self.max_frames, 1, DEBUG_READ_MAX_FRAMES, "max_frames")?;
        bound(self.max_depth, 1, DEBUG_READ_MAX_DEPTH, "max_depth")?;
        bound(self.max_scan, 1, DEBUG_READ_MAX_SCAN, "max_scan")?;
        bound(self.max_results, 1, DEBUG_READ_MAX_RESULTS, "max_results")
    }
}

fn bound<T: Copy + Ord + std::fmt::Display>(
    value: T,
    min: T,
    max: T,
    field: &str,
) -> Result<(), BridgeProtocolError> {
    if value < min || value > max {
        return Err(BridgeProtocolError::new(
            "browser_bridge_debug_read_limit_invalid",
            format!("{field} must be in {min}..={max}"),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DebugReadNode {
    pub frame_id: String,
    pub backend_node_id: u64,
    pub depth: u8,
    pub role: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserTab {
    pub tab_id: u32,
    pub window_id: u32,
    pub active: bool,
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TabsResult {
    pub tabs: Vec<BrowserTab>,
    pub truncated: bool,
}

impl TabsResult {
    pub fn validate(&self) -> Result<(), BridgeProtocolError> {
        if self.tabs.len() > TAB_MAX_RESULTS {
            return Err(BridgeProtocolError::new(
                "browser_bridge_tabs_result_overflow",
                "tab inventory exceeds its fixed result bound",
            ));
        }
        for tab in &self.tabs {
            if tab.tab_id == 0 || tab.window_id == 0 {
                return Err(BridgeProtocolError::new(
                    "browser_bridge_tab_identity_invalid",
                    "tab inventory contains a non-exact tab or window identity",
                ));
            }
            validate_text(
                &tab.title,
                TAB_TITLE_MAX_BYTES,
                true,
                "browser_bridge_control_value",
                "tab title",
            )?;
            validate_text(
                &tab.url,
                TAB_URL_MAX_BYTES,
                true,
                "browser_bridge_control_value",
                "tab URL",
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PresentationObservation {
    pub tab_active_before: bool,
    pub tab_active_after: bool,
    pub window_focused_before: bool,
    pub window_focused_after: bool,
    /// Must be false: debug-read never asks Chromium to activate or focus.
    pub activation_requested: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DetachOutcome {
    Detached,
    AlreadyDetached,
    Failed { code: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DebugReadResult {
    pub tab_id: u32,
    pub frame_count: u16,
    pub scanned: u32,
    pub truncated: bool,
    pub nodes: Vec<DebugReadNode>,
    pub presentation: PresentationObservation,
    /// Always present so cleanup is observable even when detachment failed.
    pub detach: DetachOutcome,
}

/// Failure payload returned after an attach/read attempt. Keeping detachment
/// beside the typed failure prevents callers from mistaking a failed read for
/// proven cleanup.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DebugReadFailure {
    pub tab_id: u32,
    pub code: String,
    pub detach: DetachOutcome,
}

impl DebugReadFailure {
    pub fn validate_for(&self, req: &DebugReadRequest) -> Result<(), BridgeProtocolError> {
        req.validate()?;
        if self.tab_id != req.tab_id {
            return Err(BridgeProtocolError::new(
                "browser_bridge_debug_read_target_mismatch",
                "failure does not belong to the exact requested tab",
            ));
        }
        validate_text(
            &self.code,
            96,
            false,
            "browser_bridge_control_value",
            "failure code",
        )?;
        validate_detach(&self.detach)
    }
}

impl DebugReadResult {
    pub fn validate_for(&self, req: &DebugReadRequest) -> Result<(), BridgeProtocolError> {
        req.validate()?;
        if self.tab_id != req.tab_id {
            return Err(BridgeProtocolError::new(
                "browser_bridge_debug_read_target_mismatch",
                "result does not belong to the exact requested tab",
            ));
        }
        if self.frame_count > req.max_frames
            || self.frame_count == 0
            || self.scanned > req.max_scan
            || self.nodes.len() > usize::from(req.max_results)
            || self.nodes.len() > self.scanned as usize
        {
            return Err(BridgeProtocolError::new(
                "browser_bridge_debug_read_result_overflow",
                "result exceeds its request bounds",
            ));
        }
        if self.presentation.activation_requested {
            return Err(BridgeProtocolError::new(
                "browser_bridge_debug_read_activation_forbidden",
                "debug-read must not request activation or focus",
            ));
        }
        if self.presentation.tab_active_before != self.presentation.tab_active_after
            || self.presentation.window_focused_before != self.presentation.window_focused_after
        {
            return Err(BridgeProtocolError::new(
                "browser_bridge_debug_read_presentation_changed",
                "debug-read did not preserve tab activation and window focus",
            ));
        }
        if matches!(self.detach, DetachOutcome::Failed { .. }) {
            return Err(BridgeProtocolError::new(
                "browser_bridge_debug_read_detach_failed",
                "successful debug-read requires proven debugger detachment",
            ));
        }
        for node in &self.nodes {
            if node.backend_node_id == 0 || node.depth > req.max_depth {
                return Err(BridgeProtocolError::new(
                    "browser_bridge_debug_read_result_overflow",
                    "node exceeds the requested depth",
                ));
            }
            for (field, text, max, empty) in [
                ("frame_id", node.frame_id.as_str(), 256, false),
                ("role", node.role.as_str(), 256, false),
                ("name", node.name.as_str(), 16 * 1024, true),
            ] {
                validate_text(text, max, empty, "browser_bridge_control_value", field)?;
            }
        }
        validate_detach(&self.detach)
    }
}

fn validate_detach(detach: &DetachOutcome) -> Result<(), BridgeProtocolError> {
    if let DetachOutcome::Failed { code } = detach {
        validate_text(
            code,
            96,
            false,
            "browser_bridge_control_value",
            "detach code",
        )?;
    }
    Ok(())
}

fn validate_text(
    value: &str,
    max: usize,
    allow_empty: bool,
    code: &'static str,
    field: &str,
) -> Result<(), BridgeProtocolError> {
    if (!allow_empty && value.is_empty())
        || value.len() > max
        || value.chars().any(char::is_control)
    {
        return Err(BridgeProtocolError::new(
            code,
            format!("{field} must be bounded UTF-8 without control values"),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeProtocolError {
    pub code: &'static str,
    pub message: String,
}
impl BridgeProtocolError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Default)]
pub struct NativeMessageDecoder {
    pending: Vec<u8>,
}
impl NativeMessageDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, BridgeProtocolError> {
        self.pending.extend_from_slice(chunk);
        let mut messages = Vec::new();
        loop {
            if self.pending.len() < 4 {
                break;
            }
            let size =
                u32::from_le_bytes(self.pending[..4].try_into().expect("four bytes")) as usize;
            if size > NATIVE_MESSAGE_MAX_BYTES {
                return Err(BridgeProtocolError::new(
                    "browser_bridge_message_too_large",
                    format!("native message declares {size} bytes"),
                ));
            }
            let len = 4usize.checked_add(size).ok_or_else(|| {
                BridgeProtocolError::new(
                    "browser_bridge_message_too_large",
                    "native message length overflow",
                )
            })?;
            if self.pending.len() < len {
                break;
            }
            let value = serde_json::from_slice(&self.pending[4..len]).map_err(|e| {
                BridgeProtocolError::new(
                    "browser_bridge_message_invalid",
                    format!("native message is not valid JSON: {e}"),
                )
            })?;
            self.pending.drain(..len);
            messages.push(value);
        }
        Ok(messages)
    }
}

pub fn decode_request(value: Value) -> Result<BridgeRequest, BridgeProtocolError> {
    let request: BridgeRequest = serde_json::from_value(value).map_err(|e| {
        BridgeProtocolError::new(
            "browser_bridge_request_invalid",
            format!("native request has an invalid shape: {e}"),
        )
    })?;
    request.validate()?;
    Ok(request)
}

pub fn encode_native_message(value: &Value) -> Result<Vec<u8>, BridgeProtocolError> {
    let body = serde_json::to_vec(value).map_err(|e| {
        BridgeProtocolError::new(
            "browser_bridge_message_invalid",
            format!("native message cannot be encoded: {e}"),
        )
    })?;
    if body.len() > REQUEST_MAX_BYTES {
        return Err(BridgeProtocolError::new(
            "browser_bridge_request_too_large",
            format!("native request contains {} bytes", body.len()),
        ));
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(
        &u32::try_from(body.len())
            .expect("request bound fits u32")
            .to_le_bytes(),
    );
    frame.extend_from_slice(&body);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(command: &str) -> BridgeRequest {
        BridgeRequest {
            protocol: 1,
            id: "acu:42.1".into(),
            command: command.into(),
            args: Map::new(),
        }
    }
    fn debug_req() -> DebugReadRequest {
        DebugReadRequest {
            tab_id: 7,
            max_frames: 64,
            max_depth: 20,
            max_scan: 5_000,
            max_results: 1_000,
        }
    }

    #[test]
    fn only_truthful_commands_and_shapes_validate() {
        req("status").validate().unwrap();
        req("tabs").validate().unwrap();
        for command in ["windows", "read", "debug-invoke", "click", "type", "nav"] {
            assert_eq!(
                req(command).validate().unwrap_err().code,
                "browser_bridge_command_unknown"
            );
        }
        let mut bad = req("status");
        bad.args.insert("extra".into(), json!(true));
        assert_eq!(
            bad.validate().unwrap_err().code,
            "browser_bridge_args_invalid"
        );
        for value in [
            json!({"protocol":1,"id":"x","command":"status","args":[]}),
            json!({"protocol":1,"id":"x","command":"status","args":{},"extra":true}),
        ] {
            assert_eq!(
                decode_request(value).unwrap_err().code,
                "browser_bridge_request_invalid"
            );
        }
    }

    #[test]
    fn debug_read_limits_are_closed() {
        let valid = debug_req();
        valid.validate().unwrap();
        for bad in [
            DebugReadRequest {
                tab_id: 0,
                ..valid.clone()
            },
            DebugReadRequest {
                max_frames: 65,
                ..valid.clone()
            },
            DebugReadRequest {
                max_depth: 21,
                ..valid.clone()
            },
            DebugReadRequest {
                max_scan: 5_001,
                ..valid.clone()
            },
            DebugReadRequest {
                max_results: 1_001,
                ..valid.clone()
            },
        ] {
            assert_eq!(
                bad.validate().unwrap_err().code,
                "browser_bridge_debug_read_limit_invalid"
            );
        }
    }

    fn result() -> DebugReadResult {
        DebugReadResult {
            tab_id: 7,
            frame_count: 1,
            scanned: 1,
            truncated: false,
            nodes: vec![DebugReadNode {
                frame_id: "frame-1".into(),
                backend_node_id: 9,
                depth: 2,
                role: "heading".into(),
                name: "Account".into(),
            }],
            presentation: PresentationObservation {
                tab_active_before: false,
                tab_active_after: false,
                window_focused_before: false,
                window_focused_after: false,
                activation_requested: false,
            },
            detach: DetachOutcome::Detached,
        }
    }

    #[test]
    fn result_proves_exact_target_bounds_background_and_detach() {
        let mut value = result();
        value.validate_for(&debug_req()).unwrap();
        value.tab_id = 8;
        assert_eq!(
            value.validate_for(&debug_req()).unwrap_err().code,
            "browser_bridge_debug_read_target_mismatch"
        );
        let mut value = result();
        value.presentation.activation_requested = true;
        assert_eq!(
            value.validate_for(&debug_req()).unwrap_err().code,
            "browser_bridge_debug_read_activation_forbidden"
        );
        let mut value = result();
        value.presentation.tab_active_after = true;
        assert_eq!(
            value.validate_for(&debug_req()).unwrap_err().code,
            "browser_bridge_debug_read_presentation_changed"
        );
        let mut value = result();
        value.detach = DetachOutcome::Failed {
            code: "detach_failed".into(),
        };
        assert_eq!(
            value.validate_for(&debug_req()).unwrap_err().code,
            "browser_bridge_debug_read_detach_failed"
        );
        let mut value = result();
        value.nodes[0].name = "bad\nvalue".into();
        assert_eq!(
            value.validate_for(&debug_req()).unwrap_err().code,
            "browser_bridge_control_value"
        );
        let mut value = result();
        value.nodes[0].depth = 21;
        assert_eq!(
            value.validate_for(&debug_req()).unwrap_err().code,
            "browser_bridge_debug_read_result_overflow"
        );
        let mut value = result();
        value.frame_count = 0;
        assert_eq!(
            value.validate_for(&debug_req()).unwrap_err().code,
            "browser_bridge_debug_read_result_overflow"
        );
        let mut value = result();
        value.nodes[0].backend_node_id = 0;
        assert_eq!(
            value.validate_for(&debug_req()).unwrap_err().code,
            "browser_bridge_debug_read_result_overflow"
        );
        let mut encoded = serde_json::to_value(result()).unwrap();
        encoded["nodes"][0]["value"] = serde_json::json!("form secret");
        assert!(serde_json::from_value::<DebugReadResult>(encoded).is_err());
    }

    #[test]
    fn failed_read_still_requires_exact_target_and_detach_outcome() {
        let mut failure = DebugReadFailure {
            tab_id: 7,
            code: "browser_bridge_debug_read_failed".into(),
            detach: DetachOutcome::Failed {
                code: "detach_failed".into(),
            },
        };
        failure.validate_for(&debug_req()).unwrap();
        failure.tab_id = 8;
        assert_eq!(
            failure.validate_for(&debug_req()).unwrap_err().code,
            "browser_bridge_debug_read_target_mismatch"
        );
    }

    #[test]
    fn framing_handles_split_combined_and_oversized_input() {
        assert_eq!(NATIVE_MESSAGE_MAX_BYTES, REQUEST_MAX_BYTES);
        let one = encode_native_message(&json!({"n":1})).unwrap();
        let two = encode_native_message(&json!({"n":2})).unwrap();
        let mut decoder = NativeMessageDecoder::default();
        assert!(decoder.push(&one[..2]).unwrap().is_empty());
        let mut rest = one[2..].to_vec();
        rest.extend_from_slice(&two);
        assert_eq!(
            decoder.push(&rest).unwrap(),
            vec![json!({"n":1}), json!({"n":2})]
        );
        let mut decoder = NativeMessageDecoder::default();
        let declared = u32::try_from(NATIVE_MESSAGE_MAX_BYTES + 1).unwrap();
        assert_eq!(
            decoder.push(&declared.to_le_bytes()).unwrap_err().code,
            "browser_bridge_message_too_large"
        );
    }

    #[test]
    fn decoder_accepts_combined_frames_beyond_one_frame_limit() {
        let payload = json!({"value":"x".repeat(NATIVE_MESSAGE_MAX_BYTES / 2)});
        let body = serde_json::to_vec(&payload).unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
        frame.extend_from_slice(&body);
        let mut combined = frame.clone();
        combined.extend_from_slice(&frame);
        assert_eq!(
            NativeMessageDecoder::default()
                .push(&combined)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn outbound_request_has_independent_bound() {
        let error =
            encode_native_message(&json!({"value":"x".repeat(REQUEST_MAX_BYTES)})).unwrap_err();
        assert_eq!(error.code, "browser_bridge_request_too_large");
    }

    #[test]
    fn tabs_are_bounded_and_contain_no_control_text() {
        let valid = BrowserTab {
            tab_id: 4,
            window_id: 2,
            active: false,
            title: "Documentation".into(),
            url: "https://example.invalid/".into(),
        };
        TabsResult {
            tabs: vec![valid.clone()],
            truncated: false,
        }
        .validate()
        .unwrap();
        let mut invalid = valid;
        invalid.title = "bad\ncaption".into();
        assert_eq!(
            TabsResult {
                tabs: vec![invalid],
                truncated: false
            }
            .validate()
            .unwrap_err()
            .code,
            "browser_bridge_control_value"
        );
        assert_eq!(
            TabsResult {
                tabs: vec![
                    BrowserTab {
                        tab_id: 1,
                        window_id: 1,
                        active: false,
                        title: String::new(),
                        url: String::new(),
                    };
                    TAB_MAX_RESULTS + 1
                ],
                truncated: true,
            }
            .validate()
            .unwrap_err()
            .code,
            "browser_bridge_tabs_result_overflow"
        );
    }
}
