//! Typed facade over the AgenTerm-owned terminal/session control plane.

use std::{
    thread,
    time::{Duration, Instant},
};

use agenterm_control_client::{ControlClient, ControlResponse, Intent};
use serde_json::{Value, json};

use crate::{CuError, TerminalWaitCondition, receipt::ReceiptLog};

const CAPTURE_MAX_BYTES: usize = 1_048_576;
const MAX_WAIT_MS: u64 = 86_400_000;

fn client() -> Result<ControlClient, CuError> {
    ControlClient::from_environment().map_err(|error| CuError::new(error.code, error.message))
}

fn request(
    client: &ControlClient,
    args: Vec<String>,
    operation: &str,
    intent: Intent,
    timeout: Duration,
) -> Result<ControlResponse, CuError> {
    let response = client
        .request(args, operation, intent, timeout)
        .map_err(|error| CuError::new(error.code, error.message))?;
    if response.ok {
        Ok(response)
    } else {
        let code = if response.error_code.is_empty() {
            "terminal_control_failed"
        } else {
            response.error_code.as_str()
        };
        Err(CuError::new(code, response.error).with_detail(json!({
            "category": response.error_category,
            "retryable": response.retryable,
            "control_receipt": response.receipt,
        })))
    }
}

fn parse_output(response: ControlResponse, code: &'static str) -> Result<Value, CuError> {
    serde_json::from_str(&response.output)
        .map_err(|error| CuError::new(code, format!("invalid AgenTerm control JSON: {error}")))
}

fn validate_tab(tab: &str) -> Result<(), CuError> {
    let Some(number) = tab.strip_prefix('@') else {
        return Err(CuError::new(
            "terminal_tab_invalid",
            "terminal tab must be a stable @N id",
        ));
    };
    if number.is_empty() || number.parse::<u64>().is_err() {
        return Err(CuError::new(
            "terminal_tab_invalid",
            "terminal tab must be a stable @N id",
        ));
    }
    Ok(())
}

pub(super) fn terminal_list_payload() -> Result<Value, CuError> {
    let client = client()?;
    let response = request(
        &client,
        vec!["ui-bootstrap".to_owned()],
        "ui.bootstrap",
        Intent::Query,
        Duration::from_secs(5),
    )?;
    let snapshot = parse_output(response, "terminal_inventory_invalid")?;
    let tabs = snapshot["tabs"]
        .as_array()
        .ok_or_else(|| CuError::new("terminal_inventory_invalid", "ui-bootstrap omitted tabs"))?
        .iter()
        .map(|tab| {
            json!({
                "id": tab["id"],
                "index": tab["index"],
                "parent_id": tab["parent_id"],
                "title": tab["title"],
                "process_id": tab["process_id"],
                "dead": tab["dead"],
                "exit_code": tab["exit_code"],
                "rows": tab["screen"]["rows"],
                "columns": tab["screen"]["columns"],
                "screen_generation": tab["screen"]["generation"],
                "screen_complete": tab["screen"]["complete"],
                "screen_truncated": tab["screen"]["truncated"],
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "server_scope_id": client.server_scope_id(),
        "server_epoch": snapshot["server_epoch"],
        "position": snapshot["position"],
        "active_tab_id": snapshot["active_tab_id"],
        "tabs": tabs,
        "identity": "server-scope+epoch+tab-id",
    }))
}

pub(super) fn terminal_read_payload(tab: &str, max_bytes: usize) -> Result<Value, CuError> {
    validate_tab(tab)?;
    if !(1..=CAPTURE_MAX_BYTES).contains(&max_bytes) {
        return Err(CuError::new(
            "terminal_read_limit_invalid",
            "terminal-read --max-bytes must be in 1..=1048576",
        ));
    }
    let client = client()?;
    let response = request(
        &client,
        vec![
            "capture-pane".to_owned(),
            "-p".to_owned(),
            "-t".to_owned(),
            tab.to_owned(),
            "--max-bytes".to_owned(),
            max_bytes.to_string(),
            "--json".to_owned(),
        ],
        "command.capture.pane",
        Intent::Query,
        Duration::from_secs(5),
    )?;
    let mut capture = parse_output(response, "terminal_capture_invalid")?;
    capture["read_kind"] = json!("bounded-screen-snapshot");
    capture["incremental_cursor"] = Value::Null;
    Ok(capture)
}

pub(super) fn terminal_send_payload(
    tab: &str,
    text: &str,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    if text.is_empty() {
        return Err(CuError::new(
            "terminal_send_empty",
            "terminal-send text must not be empty",
        ));
    }
    if text.len() > CAPTURE_MAX_BYTES {
        return Err(CuError::new(
            "terminal_send_too_large",
            "terminal-send text exceeds 1048576 bytes",
        ));
    }
    let client = client()?;
    let ticket = receipts.reserve(
        "terminal-send",
        0,
        json!({ "tab_id": tab, "text_bytes": text.len(), "before": "unknown" }),
    )?;
    let result = request(
        &client,
        vec![
            "send-keys".to_owned(),
            "-t".to_owned(),
            tab.to_owned(),
            "-l".to_owned(),
            "--".to_owned(),
            text.to_owned(),
        ],
        "command.send.keys",
        Intent::Mutation,
        Duration::from_secs(5),
    );
    match result {
        Ok(response) => {
            let expected_tab = tab.trim_start_matches('@').parse::<u64>().ok();
            let control_verified = response.receipt.as_ref().is_some_and(|receipt| {
                receipt["outcome"].as_str() == Some("committed")
                    && receipt["resolved"]["tab_id"].as_u64() == expected_tab
            });
            receipts.complete(
                &ticket,
                "terminal-send",
                0,
                control_verified,
                json!({
                    "performed": true,
                    "verified": control_verified,
                    "control_receipt": response.receipt,
                }),
            )?;
            let payload = json!({
                "tab_id": tab,
                "text_bytes": text.len(),
                "performed": true,
                "verified": control_verified,
                "verification": "agenterm-control-receipt",
                "receipt": ticket.json(),
            });
            if control_verified {
                Ok(payload)
            } else {
                Err(CuError::new(
                    "terminal_send_unverified",
                    "AgenTerm accepted terminal input without a matching committed control receipt",
                )
                .with_detail(payload))
            }
        }
        Err(error) => {
            receipts.complete(
                &ticket,
                "terminal-send",
                0,
                false,
                json!({ "performed": false, "error": { "code": error.code, "message": error.message } }),
            )?;
            Err(error.with_detail(json!({ "receipt": ticket.json() })))
        }
    }
}

pub(super) fn terminal_wait_payload(
    tab: &str,
    condition: &TerminalWaitCondition,
    timeout_ms: u64,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    if matches!(condition, TerminalWaitCondition::Contains(text) if text.is_empty()) {
        return Err(CuError::new(
            "terminal_wait_condition_invalid",
            "terminal-wait --contains must not be empty",
        ));
    }
    if !(1..=MAX_WAIT_MS).contains(&timeout_ms) {
        return Err(CuError::new(
            "terminal_wait_limit_invalid",
            "terminal-wait --timeout-ms must be in 1..=86400000",
        ));
    }
    let client = client()?;
    let started = Instant::now();
    let deadline = started + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(CuError::new(
                "terminal_wait_timeout",
                "terminal condition was not met before the deadline",
            )
            .with_detail(
                json!({ "tab_id": tab, "condition": condition, "timeout_ms": timeout_ms }),
            ));
        }
        let request_timeout = remaining.min(Duration::from_secs(5));
        let matched = match condition {
            TerminalWaitCondition::Contains(needle) => {
                let value =
                    terminal_read_with_client(&client, tab, CAPTURE_MAX_BYTES, request_timeout)?;
                value["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(needle))
            }
            TerminalWaitCondition::Exited | TerminalWaitCondition::Finalized => {
                let response = request(
                    &client,
                    vec!["inspect".to_owned(), "-t".to_owned(), tab.to_owned()],
                    "command.inspect",
                    Intent::Query,
                    request_timeout,
                )?;
                let value = parse_output(response, "terminal_inspect_invalid")?;
                match condition {
                    TerminalWaitCondition::Exited => {
                        value["windows"].as_array().is_some_and(|rows| {
                            !rows.is_empty()
                                && rows.iter().all(|row| row["dead"].as_bool() == Some(true))
                        })
                    }
                    TerminalWaitCondition::Finalized => {
                        value["windows"].as_array().is_some_and(|rows| {
                            !rows.is_empty()
                                && rows
                                    .iter()
                                    .all(|row| row["finalized"].as_bool() == Some(true))
                        })
                    }
                    TerminalWaitCondition::Contains(_) => unreachable!(),
                }
            }
        };
        if matched {
            return Ok(json!({
                "tab_id": tab,
                "condition": condition,
                "state": "matched",
                "completed": true,
                "elapsed_ms": started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            }));
        }
        thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

fn terminal_read_with_client(
    client: &ControlClient,
    tab: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<Value, CuError> {
    let response = request(
        client,
        vec![
            "capture-pane".to_owned(),
            "-p".to_owned(),
            "-t".to_owned(),
            tab.to_owned(),
            "--max-bytes".to_owned(),
            max_bytes.to_string(),
            "--json".to_owned(),
        ],
        "command.capture.pane",
        Intent::Query,
        timeout,
    )?;
    parse_output(response, "terminal_capture_invalid")
}
