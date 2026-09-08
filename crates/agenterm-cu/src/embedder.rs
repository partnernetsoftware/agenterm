//! In-process ACU adapter shared by typed embedders and the command shell.
//!
//! The wire accepts a versioned `command` or `argv` envelope and returns one
//! [`CuReply`]. A naked [`Command`] remains a compatibility input. This module
//! delegates product meaning, effects, verification and receipts to the one
//! CLI parser / [`Executor`] path; it is not a second dispatcher and never
//! shells out.

use crate::{Authorization, Command, CuError, CuReply, Executor};

/// Version of the closed in-process request envelope.
pub const ACU_REQUEST_VERSION: u64 = 1;

/// MCP tool descriptor owned beside the canonical ACU command schema.
///
/// The transport includes this same source file, so MCP never grows a second
/// hand-written description of the command or its [`CuReply`].
pub const MCP_CAPABILITIES_TOOL_JSON: &str = include_str!("../contract/mcp-capabilities-tool.json");

/// Generic read-only MCP tool descriptor. The request still carries one
/// canonical [`Command`], so MCP, qjswasm and the CLI share the same schema and
/// [`Executor`] rather than growing a second observation dispatcher.
pub const MCP_OBSERVE_TOOL_JSON: &str = include_str!("../contract/mcp-observe-tool.json");

/// Decode one complete command and execute it through the supplied executor.
///
/// Supplying the executor keeps authority an upper-layer caller decision. A
/// Script profile is compatibility data and is never consulted here.
pub fn execute_json_with(executor: &Executor, command_json: &str) -> CuReply {
    let command = match serde_json::from_str::<Command>(command_json) {
        Ok(command) => command,
        Err(error) => return malformed_command(error.to_string()),
    };
    execute_command(executor, &command)
}

/// The one `Command -> Executor` adapter used by CLI and in-process clients.
pub fn execute_command(executor: &Executor, command: &Command) -> CuReply {
    executor.execute(command)
}

/// Decode and execute using the same ambient authorization source as a CLI
/// invocation with no explicit `--grant`.
///
/// This is the AgenTerm qjswasm embedder's composition callback. It preserves
/// fail-closed authorization and the full `CuReply`, including `ok:false`, as
/// ordinary data. MCP uses the closed `mcp_call` route in
/// [`execute_request_from_environment`] rather than this naked-Command helper.
pub fn execute_json_from_environment(command_json: &str) -> CuReply {
    let command = match serde_json::from_str::<Command>(command_json) {
        Ok(command) => command,
        Err(error) => return malformed_command(error.to_string()),
    };
    execute_command_from_environment(&command)
}

/// Decode one versioned embedded request and execute it through the same
/// parser/Executor path as the standalone CLI.
///
/// A naked `Command` remains accepted for ABI-v1 providers already deployed.
/// Once `acu_request` is present, the envelope is closed: an unknown version,
/// kind, missing field, extra field or wrong field type never falls back to
/// `Command` deserialization.
pub fn execute_request_from_environment(request_json: &str) -> CuReply {
    let value = match serde_json::from_str::<serde_json::Value>(request_json) {
        Ok(value) => value,
        Err(error) => return malformed_command(error.to_string()),
    };
    let Some(object) = value.as_object() else {
        return malformed_command("command must be a JSON object".to_owned());
    };
    if !object.contains_key("acu_request") {
        return match serde_json::from_value::<Command>(value) {
            Ok(command) => execute_command_from_environment(&command),
            Err(error) => malformed_command(error.to_string()),
        };
    }
    if object.get("acu_request").and_then(|value| value.as_u64()) != Some(ACU_REQUEST_VERSION) {
        return malformed_request("acu_request must be the integer 1");
    }
    match object.get("kind").and_then(|value| value.as_str()) {
        Some("command") if exact_keys(object, &["acu_request", "kind", "command"]) => {
            match strict_command(object.get("command").expect("exact command envelope")) {
                Ok(command) => execute_command_from_environment(&command),
                Err(message) => malformed_request(message),
            }
        }
        Some("argv") if exact_keys(object, &["acu_request", "kind", "argv"]) => {
            match object
                .get("argv")
                .cloned()
                .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
            {
                Some(argv) => crate::argv::execute_argv_from_environment(argv),
                None => malformed_request("argv envelope requires an array of strings"),
            }
        }
        Some("mcp_call") if exact_keys(object, &["acu_request", "kind", "name", "arguments"]) => {
            execute_mcp_call_from_environment(object)
        }
        Some("command" | "argv" | "mcp_call") => {
            malformed_request("request envelope fields do not match kind")
        }
        _ => malformed_request("request kind must be command, argv or mcp_call"),
    }
}

fn execute_mcp_call_from_environment(
    object: &serde_json::Map<String, serde_json::Value>,
) -> CuReply {
    let Some(arguments) = object
        .get("arguments")
        .and_then(serde_json::Value::as_object)
    else {
        return malformed_request("ACU MCP tool arguments must be an object");
    };
    match object.get("name").and_then(serde_json::Value::as_str) {
        Some("agenterm_acu_capabilities") if arguments.is_empty() => {
            execute_command_from_environment(&Command::Capabilities {
                target: crate::TargetRef::Current,
            })
        }
        Some("agenterm_acu_capabilities") => {
            malformed_request("agenterm_acu_capabilities arguments must be an empty object")
        }
        Some("agenterm_acu_observe") if exact_keys(arguments, &["command"]) => {
            let command = match strict_command(arguments.get("command").expect("exact arguments")) {
                Ok(command) => command,
                Err(message) => return malformed_request(message),
            };
            if command.required_grant() != crate::Grant::Observe {
                return CuReply::err(
                    &command,
                    CuError::new(
                        "mcp_observe_actuation_forbidden",
                        "agenterm_acu_observe accepts only commands whose canonical grant is observe",
                    ),
                );
            }
            execute_command_from_environment(&command)
        }
        Some("agenterm_acu_observe") => {
            malformed_request("agenterm_acu_observe arguments must contain exactly command")
        }
        _ => malformed_request("unknown ACU MCP tool"),
    }
}

fn strict_command(value: &serde_json::Value) -> Result<Command, String> {
    let command: Command = serde_json::from_value(value.clone())
        .map_err(|error| format!("command envelope contains an invalid Command: {error}"))?;
    let canonical = serde_json::to_value(&command)
        .map_err(|error| format!("command envelope cannot be canonicalized: {error}"))?;
    if canonical != *value {
        return Err(
            "command envelope must contain the exact canonical Command field set".to_owned(),
        );
    }
    Ok(command)
}

fn exact_keys(object: &serde_json::Map<String, serde_json::Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn execute_command_from_environment(command: &Command) -> CuReply {
    let mut unsupported = false;
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        if crate::auth::is_reserved_authority_env(key)
            && !key.eq_ignore_ascii_case("AGENTERM_CU_GRANT")
        {
            unsupported = true;
        }
    }
    if unsupported {
        return CuReply::err(
            command,
            CuError::new(
                "invalid_authorization",
                "unsupported authorization environment selector is present",
            ),
        );
    }
    let environment_grant = std::env::var("AGENTERM_CU_GRANT").ok();
    let authorization = match Authorization::try_from_sources(None, environment_grant.as_deref()) {
        Ok(authorization) => authorization,
        Err(error) => {
            return CuReply::err(
                command,
                CuError::new("invalid_authorization", error.to_string()),
            );
        }
    };
    execute_command(&Executor::new(authorization), command)
}

fn malformed_command(message: String) -> CuReply {
    CuReply {
        ok: false,
        target: String::new(),
        command: "acu.call".to_owned(),
        data: None,
        error: Some(CuError::new("invalid_command", message)),
    }
}

fn malformed_request(message: impl Into<String>) -> CuReply {
    CuReply {
        ok: false,
        target: String::new(),
        command: "acu.request".to_owned(),
        data: None,
        error: Some(CuError::new("invalid_acu_request", message)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{Grant, TargetRef};

    #[test]
    fn json_adapter_is_structurally_equal_to_direct_executor() {
        let command = Command::Capabilities {
            target: TargetRef::Current,
        };
        let executor = Executor::new(Authorization::new(BTreeSet::from([Grant::Observe])));
        let direct = executor.execute(&command);
        let encoded = serde_json::to_string(&command).expect("serialize command");
        let adapted = execute_json_with(&executor, &encoded);
        assert_eq!(
            serde_json::to_value(adapted).expect("adapted reply"),
            serde_json::to_value(direct).expect("direct reply")
        );
    }

    #[test]
    fn malformed_json_is_a_complete_cu_reply() {
        let executor = Executor::new(Authorization::new(BTreeSet::new()));
        let reply = execute_json_with(&executor, "{");
        assert!(!reply.ok);
        assert_eq!(reply.command, "acu.call");
        assert_eq!(reply.error.expect("error").code, "invalid_command");
    }

    #[test]
    fn versioned_command_and_legacy_command_have_the_same_reply() {
        let legacy = r#"{"verb":"capabilities","target":"current"}"#;
        let envelope = r#"{"acu_request":1,"kind":"command","command":{"verb":"capabilities","target":"current"}}"#;
        let legacy_reply = execute_request_from_environment(legacy);
        let envelope_reply = execute_request_from_environment(envelope);
        assert_eq!(
            serde_json::to_value(legacy_reply).unwrap(),
            serde_json::to_value(envelope_reply).unwrap()
        );
    }

    #[test]
    fn versioned_argv_uses_the_library_parser_and_executor() {
        let reply = execute_request_from_environment(
            r#"{"acu_request":1,"kind":"argv","argv":["--target","current","--grant","observe","capabilities"]}"#,
        );
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.command, "capabilities");
    }

    #[test]
    fn present_envelope_marker_never_falls_back_to_command_parsing() {
        for request in [
            r#"{"acu_request":2,"kind":"argv","argv":[]}"#,
            r#"{"acu_request":1,"kind":"other","argv":[]}"#,
            r#"{"acu_request":1,"kind":"argv","argv":[1]}"#,
            r#"{"acu_request":1,"kind":"argv","argv":[],"extra":true}"#,
            r#"{"acu_request":1,"kind":"command","argv":[]}"#,
            r#"{"acu_request":1,"kind":"command","command":{"verb":"capabilities","target":"current","typo":true}}"#,
            r#"{"acu_request":1,"kind":"mcp_call","name":"agenterm_acu_capabilities","arguments":{},"extra":true}"#,
        ] {
            let reply = execute_request_from_environment(request);
            assert!(!reply.ok, "{request}");
            assert_eq!(reply.command, "acu.request", "{request}");
            assert_eq!(
                reply.error.expect("request error").code,
                "invalid_acu_request",
                "{request}"
            );
        }
    }

    #[test]
    fn mcp_capabilities_uses_the_same_command_and_executor_reply() {
        let request = r#"{"acu_request":1,"kind":"mcp_call","name":"agenterm_acu_capabilities","arguments":{}}"#;
        let embedded = execute_request_from_environment(request);
        let direct = execute_request_from_environment(
            r#"{"acu_request":1,"kind":"command","command":{"verb":"capabilities","target":"current"}}"#,
        );
        assert_eq!(
            serde_json::to_value(embedded).unwrap(),
            serde_json::to_value(direct).unwrap()
        );
    }

    #[test]
    fn mcp_contract_is_valid_and_matches_the_owned_tool_name() {
        let descriptor: serde_json::Value =
            serde_json::from_str(MCP_CAPABILITIES_TOOL_JSON).expect("MCP descriptor JSON");
        assert_eq!(descriptor["name"], "agenterm_acu_capabilities");
        assert_eq!(descriptor["annotations"]["readOnlyHint"], true);
        assert_eq!(descriptor["inputSchema"]["additionalProperties"], false);

        let observe: serde_json::Value =
            serde_json::from_str(MCP_OBSERVE_TOOL_JSON).expect("MCP observe descriptor JSON");
        assert_eq!(observe["name"], "agenterm_acu_observe");
        assert_eq!(observe["annotations"]["readOnlyHint"], false);
        assert_eq!(observe["annotations"]["destructiveHint"], true);
        assert_eq!(observe["annotations"]["openWorldHint"], true);
        assert_eq!(observe["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            observe["inputSchema"]["properties"]["command"]["required"],
            serde_json::json!(["verb", "target"])
        );
        assert_eq!(
            observe["inputSchema"]["properties"]["command"]["properties"]["target"]["enum"],
            serde_json::json!(["current", "ssh", "vnc", "rdp"])
        );
    }

    #[test]
    fn mcp_observe_uses_the_same_command_and_rejects_actuation_before_dispatch() {
        let observe = r#"{"acu_request":1,"kind":"mcp_call","name":"agenterm_acu_observe","arguments":{"command":{"verb":"runtime-status","target":"current"}}}"#;
        let direct = r#"{"acu_request":1,"kind":"command","command":{"verb":"runtime-status","target":"current"}}"#;
        assert_eq!(
            serde_json::to_value(execute_request_from_environment(observe)).unwrap(),
            serde_json::to_value(execute_request_from_environment(direct)).unwrap()
        );

        let rejected = execute_request_from_environment(
            r#"{"acu_request":1,"kind":"mcp_call","name":"agenterm_acu_observe","arguments":{"command":{"verb":"clipboard-clear","target":"current","apply":true}}}"#,
        );
        assert!(!rejected.ok);
        assert_eq!(
            rejected.error.expect("typed rejection").code,
            "mcp_observe_actuation_forbidden"
        );
    }
}
