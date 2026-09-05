//! `exec`: one JSON-serialized `Command`, the worker mode behind the ssh and
//! vnc transports.

use std::path::PathBuf;

use agenterm_cu::{CuReply, worker_wire};

use super::global::{authority_environment_flags, authorize};
use super::usage_err;

pub fn dispatch_json(args: &[String]) -> CuReply {
    let mut grant: Option<String> = None;
    let mut grant_id: Option<String> = None;
    let mut grant_store: Option<PathBuf> = None;
    let mut payload = None;
    let mut read_stdin = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(value) = arg.strip_prefix("--grant=") {
            if grant.is_some() {
                return usage_err("duplicate --grant");
            }
            grant = Some(value.to_owned());
            i += 1;
        } else if let Some(value) = arg.strip_prefix("--grant-id=") {
            if grant_id.is_some() {
                return usage_err("duplicate --grant-id");
            }
            if !agenterm_cu::grant_management::valid_grant_id(value) {
                return usage_err("--grant-id is invalid");
            }
            grant_id = Some(value.to_owned());
            i += 1;
        } else if let Some(value) = arg.strip_prefix("--grant-store=") {
            if grant_store.is_some() {
                return usage_err("duplicate --grant-store");
            }
            if value.is_empty() {
                return usage_err("--grant-store requires a path");
            }
            grant_store = Some(PathBuf::from(value));
            i += 1;
        } else if arg == "--grant" {
            if grant.is_some() {
                return usage_err("duplicate --grant");
            }
            i += 1;
            if let Some(value) = args.get(i) {
                grant = Some(value.clone());
                i += 1;
            }
        } else if arg == "--json" {
            i += 1;
            if let Some(value) = args.get(i) {
                if value == "-" {
                    read_stdin = true;
                } else {
                    payload = Some(value.clone());
                }
                i += 1;
            } else {
                read_stdin = true;
            }
        } else if arg == "--json-stdin" || arg == "-" {
            read_stdin = true;
            i += 1;
        } else if payload.is_none() {
            if arg == "-" {
                read_stdin = true;
            } else {
                payload = Some(arg.clone());
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    let raw = if read_stdin {
        let mut buf = String::new();
        let mut input = std::io::Read::take(
            std::io::stdin(),
            (worker_wire::MAX_WORKER_REQUEST_BYTES + 1) as u64,
        );
        if let Err(error) = std::io::Read::read_to_string(&mut input, &mut buf) {
            return usage_err(format!("could not read JSON command from stdin: {error}"));
        }
        buf
    } else {
        let Some(raw) = payload else {
            return usage_err(
                "exec requires a JSON command payload argument, --json '-', or --json-stdin",
            );
        };
        raw
    };
    let (command, request_identity) = match worker_wire::decode(&raw) {
        Ok(decoded) => decoded,
        Err(error) => {
            return CuReply {
                ok: false,
                target: "current".to_owned(),
                command: "exec".to_owned(),
                data: None,
                error: Some(error),
            };
        }
    };
    let (ambient, unsupported_authority_environment) = authority_environment_flags();
    let mut executor = match authorize(
        grant.as_deref(),
        grant_id,
        grant_store,
        &command,
        ambient,
        unsupported_authority_environment,
    ) {
        Ok(executor) => executor,
        Err(reply) => return *reply,
    };
    if let Some((request_identity, effect_scope)) = request_identity {
        executor = executor
            .with_request_identity(request_identity)
            .with_request_effect_scope(effect_scope);
    }
    executor.execute(&command)
}
