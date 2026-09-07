//! In-process ACU adapter shared by typed embedders and the command shell.
//!
//! The wire value is exactly [`Command`] in and [`CuReply`] out. This module
//! deliberately delegates product meaning, effects, verification and receipts
//! to [`Executor`]; it is not a second dispatcher and never shells out.

use crate::{Authorization, Command, CuError, CuReply, Executor};

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
/// ordinary data. MCP is not wired to this adapter yet.
pub fn execute_json_from_environment(command_json: &str) -> CuReply {
    let command = match serde_json::from_str::<Command>(command_json) {
        Ok(command) => command,
        Err(error) => return malformed_command(error.to_string()),
    };
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
            &command,
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
                &command,
                CuError::new("invalid_authorization", error.to_string()),
            );
        }
    };
    execute_command(&Executor::new(authorization), &command)
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
}
