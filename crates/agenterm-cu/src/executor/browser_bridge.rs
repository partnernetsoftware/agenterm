use serde_json::{Map, Value, json};

use crate::{
    browser_bridge::{
        BridgeRequest, ConnectionId, DebugReadRequest, install_for_current_user,
        list_live_connections, send_to_connection,
    },
    reply::CuError,
};

pub(super) fn browser_bridge_setup_payload() -> Result<Value, CuError> {
    let executable = std::env::current_exe().map_err(|_| {
        CuError::new(
            "browser_bridge_current_executable_unavailable",
            "the running agenterm-cu executable could not be resolved",
        )
    })?;
    let receipt = install_for_current_user(&executable).map_err(|error| {
        let mut typed = CuError::new(error.code, "browser bridge setup failed");
        if let Some(receipt) = error.receipt {
            typed = typed.with_detail(json!({ "receipt": receipt }));
        }
        typed
    })?;
    serde_json::to_value(receipt).map_err(|_| {
        CuError::new(
            "browser_bridge_install_receipt_invalid",
            "the browser bridge setup receipt could not be serialized",
        )
    })
}

pub(super) fn browser_bridge_connections_payload() -> Result<Value, CuError> {
    let inventory = list_live_connections().map_err(host_error)?;
    serde_json::to_value(inventory).map_err(|_| {
        CuError::new(
            "browser_bridge_response_invalid",
            "the bounded browser bridge connection inventory could not be serialized",
        )
    })
}

pub(super) fn browser_bridge_debug_read_payload(
    connection_id: &ConnectionId,
    tab_id: u32,
    max_frames: u16,
    max_depth: u8,
    max_scan: u32,
    max_results: u16,
) -> Result<Value, CuError> {
    let request = DebugReadRequest {
        tab_id,
        max_frames,
        max_depth,
        max_scan,
        max_results,
    };
    request
        .validate()
        .map_err(|error| CuError::new(error.code, error.message))?;
    let args = serde_json::to_value(request).map_err(|_| {
        CuError::new(
            "browser_bridge_request_invalid",
            "the bounded debug-read request could not be serialized",
        )
    })?;
    let Value::Object(args) = args else {
        return Err(CuError::new(
            "browser_bridge_request_invalid",
            "the bounded debug-read request was not an object",
        ));
    };
    browser_bridge_request_payload(connection_id, "debug-read", args)
}

pub(super) fn browser_bridge_request_payload(
    connection_id: &ConnectionId,
    command: &str,
    args: Map<String, Value>,
) -> Result<Value, CuError> {
    let request = BridgeRequest {
        protocol: crate::browser_bridge::PROTOCOL_VERSION,
        id: request_id()?,
        command: command.to_owned(),
        args,
    };
    let response = send_to_connection(connection_id, &request).map_err(host_error)?;
    if let Some(error) = response.error {
        let detail = json!({
            "connection_id": connection_id,
            "tab_id": error.tab_id,
            "detach": error.detach,
        });
        return Err(CuError::new(
            error.code,
            "the browser extension refused the typed bridge request",
        )
        .with_detail(detail));
    }
    let result = response.result.ok_or_else(|| {
        CuError::new(
            "browser_bridge_response_invalid",
            "the browser bridge omitted its successful result",
        )
    })?;
    Ok(json!({
        "connection_id": connection_id,
        "result": result,
    }))
}

fn request_id() -> Result<String, CuError> {
    let random = agenterm_platform::entropy::secure_random_array::<32>().map_err(|_| {
        CuError::new(
            "browser_bridge_entropy_unavailable",
            "a fresh browser bridge request identity could not be generated",
        )
    })?;
    let mut id = String::with_capacity(68);
    id.push_str("acu-");
    for byte in random {
        use std::fmt::Write as _;
        write!(id, "{byte:02x}").expect("writing hexadecimal text to String cannot fail");
    }
    Ok(id)
}

fn host_error(error: crate::browser_bridge::BridgeHostError) -> CuError {
    CuError::new(
        error.code,
        "the exact browser bridge connection could not complete the request",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_request_identity_is_closed_and_bounded() {
        let id = request_id().expect("request id");
        assert_eq!(id.len(), 68);
        assert!(id.starts_with("acu-"));
        assert!(
            id[4..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }

    #[test]
    fn debug_read_bounds_fail_before_connection_io() {
        let connection_id = ConnectionId::parse(&"1".repeat(64)).unwrap();
        let error = browser_bridge_debug_read_payload(
            &connection_id,
            0,
            crate::browser_bridge::DEBUG_READ_MAX_FRAMES,
            crate::browser_bridge::DEBUG_READ_MAX_DEPTH,
            crate::browser_bridge::DEBUG_READ_MAX_SCAN,
            crate::browser_bridge::DEBUG_READ_MAX_RESULTS,
        )
        .unwrap_err();
        assert_eq!(error.code, "browser_bridge_debug_read_limit_invalid");
    }
}
