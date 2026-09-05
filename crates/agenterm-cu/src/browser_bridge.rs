//! Versioned framing and request validation for the owned Chromium
//! Native-Messaging bridge. This is the transport-neutral protocol core; the
//! installer, host process and MV3 adapter remain separate product leaves.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PROTOCOL_VERSION: u32 = 1;
pub const NATIVE_MESSAGE_MAX_BYTES: usize = 8 * 1024 * 1024;
pub const REQUEST_MAX_BYTES: usize = 1024 * 1024;

const COMMANDS: &[&str] = &[
    "status",
    "tabs",
    "windows",
    "window-state",
    "open",
    "read",
    "debug-read",
    "debug-invoke",
    "debug-type",
    "debug-files",
    "click",
    "type",
    "invoke",
    "nav",
    "activate",
    "reload",
    "close",
];

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
        if self.id.is_empty()
            || self.id.len() > 96
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return Err(BridgeProtocolError::new(
                "browser_bridge_request_id_invalid",
                "browser bridge request id must be 1..=96 ASCII id characters",
            ));
        }
        if !COMMANDS.contains(&self.command.as_str()) {
            return Err(BridgeProtocolError::new(
                "browser_bridge_command_unknown",
                "browser bridge command is not in this protocol version",
            ));
        }
        Ok(())
    }
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
            let frame_len = 4usize.checked_add(size).ok_or_else(|| {
                BridgeProtocolError::new(
                    "browser_bridge_message_too_large",
                    "native message length overflow",
                )
            })?;
            if self.pending.len() < frame_len {
                break;
            }
            let value = serde_json::from_slice(&self.pending[4..frame_len]).map_err(|error| {
                BridgeProtocolError::new(
                    "browser_bridge_message_invalid",
                    format!("native message is not valid JSON: {error}"),
                )
            })?;
            self.pending.drain(..frame_len);
            messages.push(value);
        }
        Ok(messages)
    }
}

pub fn encode_native_message(value: &Value) -> Result<Vec<u8>, BridgeProtocolError> {
    let body = serde_json::to_vec(value).map_err(|error| {
        BridgeProtocolError::new(
            "browser_bridge_message_invalid",
            format!("native message cannot be encoded: {error}"),
        )
    })?;
    if body.len() > REQUEST_MAX_BYTES {
        return Err(BridgeProtocolError::new(
            "browser_bridge_request_too_large",
            format!("native request contains {} bytes", body.len()),
        ));
    }
    let size = u32::try_from(body.len()).expect("request limit fits u32");
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&size.to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> BridgeRequest {
        BridgeRequest {
            protocol: PROTOCOL_VERSION,
            id: "acu:42.1".into(),
            command: "status".into(),
            args: Map::new(),
        }
    }

    #[test]
    fn validates_version_id_and_closed_command_catalog() {
        request().validate().unwrap();
        for (invalid, code) in [
            (
                {
                    let mut value = request();
                    value.protocol += 1;
                    value
                },
                "browser_bridge_protocol_mismatch",
            ),
            (
                {
                    let mut value = request();
                    value.id = "bad id".into();
                    value
                },
                "browser_bridge_request_id_invalid",
            ),
            (
                {
                    let mut value = request();
                    value.command = "shell".into();
                    value
                },
                "browser_bridge_command_unknown",
            ),
        ] {
            assert_eq!(invalid.validate().unwrap_err().code, code);
        }
    }

    #[test]
    fn decoder_survives_split_and_combined_frames() {
        let one = encode_native_message(&json!({"n": 1})).unwrap();
        let two = encode_native_message(&json!({"n": 2})).unwrap();
        let mut decoder = NativeMessageDecoder::default();
        assert!(decoder.push(&one[..2]).unwrap().is_empty());
        let mut rest = one[2..].to_vec();
        rest.extend_from_slice(&two);
        assert_eq!(
            decoder.push(&rest).unwrap(),
            vec![json!({"n": 1}), json!({"n": 2})]
        );
    }

    #[test]
    fn decoder_rejects_oversize_before_waiting_for_body() {
        let mut decoder = NativeMessageDecoder::default();
        let declared = u32::try_from(NATIVE_MESSAGE_MAX_BYTES + 1).unwrap();
        let error = decoder.push(&declared.to_le_bytes()).unwrap_err();
        assert_eq!(error.code, "browser_bridge_message_too_large");
    }

    #[test]
    fn encoder_rejects_request_over_one_mibibyte() {
        let error =
            encode_native_message(&json!({"value": "x".repeat(REQUEST_MAX_BYTES)})).unwrap_err();
        assert_eq!(error.code, "browser_bridge_request_too_large");
    }
}
