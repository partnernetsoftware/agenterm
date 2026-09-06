//! Typed facade over the AgenTerm-owned terminal/session control plane.

use std::{
    fmt::Write as _,
    io::Read as _,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use agenterm_control_client::{ControlClient, ControlResponse, Intent};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::{CuError, TerminalScrollAction, TerminalWaitCondition, receipt::ReceiptLog};

const CAPTURE_MAX_BYTES: usize = 1_048_576;
const MAX_WAIT_MS: u64 = 86_400_000;

fn client() -> Result<ControlClient, CuError> {
    ControlClient::from_environment().map_err(|error| CuError::new(error.code, error.message))
}

pub(super) fn request(
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

pub(super) fn request_protocol(
    client: &ControlClient,
    args: Vec<String>,
    timeout: Duration,
) -> Result<ControlResponse, CuError> {
    let response = client
        .request_protocol(args, timeout)
        .map_err(|error| CuError::new(error.code, error.message))?;
    if response.ok {
        Ok(response)
    } else {
        let code = if response.error_code.is_empty() {
            "terminal_protocol_failed"
        } else {
            response.error_code.as_str()
        };
        Err(CuError::new(code, response.error).with_detail(json!({
            "category": response.error_category,
            "retryable": response.retryable,
        })))
    }
}

pub(super) fn parse_output(
    response: ControlResponse,
    code: &'static str,
) -> Result<Value, CuError> {
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
    terminal_inventory_with_client(&client)
}

pub(super) fn terminal_inventory_with_client(client: &ControlClient) -> Result<Value, CuError> {
    let response = request(
        client,
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

pub(super) fn terminal_new_payload(
    title: Option<&str>,
    parent: Option<&str>,
    detached: bool,
    child_command: &[String],
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    if title.is_some_and(|value| value.len() > 4_096) {
        return Err(CuError::new(
            "terminal_new_title_too_large",
            "terminal-new title exceeds 4096 bytes",
        ));
    }
    if let Some(parent) = parent {
        validate_tab(parent)?;
    }
    if child_command.len() > 256
        || child_command.iter().map(String::len).sum::<usize>() > CAPTURE_MAX_BYTES
    {
        return Err(CuError::new(
            "terminal_new_command_too_large",
            "terminal-new command exceeds 256 arguments or 1048576 bytes",
        ));
    }

    let client = client()?;
    terminal_new_with_client(&client, title, parent, detached, child_command, receipts)
}

pub(super) fn terminal_new_with_client(
    client: &ControlClient,
    title: Option<&str>,
    parent: Option<&str>,
    detached: bool,
    child_command: &[String],
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    let before = terminal_inventory_with_client(client)?;
    if let Some(parent) = parent
        && !inventory_contains(&before, parent)
    {
        return Err(CuError::new(
            "terminal_parent_not_found",
            "terminal-new parent is not present in the current server epoch",
        )
        .with_detail(json!({ "parent_id": parent })));
    }
    let ticket = receipts.reserve(
        "terminal-new",
        0,
        json!({
            "parent_id": parent,
            "title_bytes": title.map(str::len).unwrap_or(0),
            "detached": detached,
            "command_arguments": child_command.len(),
            "command_bytes": child_command.iter().map(String::len).sum::<usize>(),
            "server_scope_id": before["server_scope_id"],
            "server_epoch": before["server_epoch"],
        }),
    )?;
    let mut args = vec![
        "new-window".to_owned(),
        "-F".to_owned(),
        "#{window_id}".to_owned(),
    ];
    if let Some(title) = title {
        args.extend(["-n".to_owned(), title.to_owned()]);
    }
    if detached {
        args.push("-d".to_owned());
    }
    if let Some(parent) = parent {
        args.extend(["--parent".to_owned(), parent.to_owned()]);
    }
    if !child_command.is_empty() {
        args.push("--".to_owned());
        args.extend(child_command.iter().cloned());
    }
    let response = request(
        client,
        args,
        "command.new.window",
        Intent::Mutation,
        Duration::from_secs(10),
    );
    match response {
        Ok(response) => {
            let tab = response.output.trim().to_owned();
            let post = match terminal_inventory_with_client(client) {
                Ok(post) => post,
                Err(error) => {
                    let evidence = json!({
                        "tab_id": tab,
                        "performed": true,
                        "verified": false,
                        "postcheck_error": { "code": error.code, "message": error.message },
                        "receipt": ticket.json(),
                    });
                    receipts.complete(&ticket, "terminal-new", 0, false, evidence.clone())?;
                    return Err(CuError::new(
                        "terminal_new_unverified",
                        "AgenTerm accepted terminal creation but its post-state was unreadable",
                    )
                    .with_detail(evidence));
                }
            };
            let same_scope = post["server_scope_id"] == before["server_scope_id"];
            let same_epoch = post["server_epoch"] == before["server_epoch"];
            let parent_verified = parent.is_none_or(|expected| {
                inventory_tab(&post, &tab).and_then(|row| row["parent_id"].as_str())
                    == Some(expected)
            });
            let verified = validate_tab(&tab).is_ok()
                && same_scope
                && same_epoch
                && inventory_contains(&post, &tab)
                && parent_verified;
            let evidence = json!({
                "tab_id": tab,
                "parent_id": parent,
                "performed": true,
                "verified": verified,
                "verification": "same-scope-epoch-inventory",
                "server_scope_id": post["server_scope_id"],
                "server_epoch": post["server_epoch"],
                "receipt": ticket.json(),
            });
            receipts.complete(&ticket, "terminal-new", 0, verified, evidence.clone())?;
            if verified {
                Ok(evidence)
            } else {
                Err(CuError::new(
                    "terminal_new_unverified",
                    "AgenTerm accepted terminal creation but the new stable tab was not verified",
                )
                .with_detail(evidence))
            }
        }
        Err(error) => {
            receipts.complete(
                &ticket,
                "terminal-new",
                0,
                false,
                json!({ "performed": false, "error": { "code": error.code, "message": error.message } }),
            )?;
            Err(error.with_detail(json!({ "receipt": ticket.json() })))
        }
    }
}

pub(super) fn terminal_close_payload(
    tab: &str,
    expect_closed: bool,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    if !expect_closed {
        return Err(CuError::new(
            "terminal_close_intent_required",
            "terminal-close requires explicit --expect closed",
        ));
    }
    let client = client()?;
    terminal_close_with_client(&client, tab, expect_closed, receipts)
}

pub(super) fn terminal_close_with_client(
    client: &ControlClient,
    tab: &str,
    expect_closed: bool,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    if !expect_closed {
        return Err(CuError::new(
            "terminal_close_intent_required",
            "terminal-close requires explicit --expect closed",
        ));
    }
    let before = terminal_inventory_with_client(client)?;
    if !inventory_contains(&before, tab) {
        return Err(CuError::new(
            "terminal_tab_not_found",
            "terminal-close target is not present in the current server epoch",
        )
        .with_detail(json!({ "tab_id": tab })));
    }
    let ticket = receipts.reserve(
        "terminal-close",
        0,
        json!({
            "tab_id": tab,
            "expected": "closed",
            "server_scope_id": before["server_scope_id"],
            "server_epoch": before["server_epoch"],
        }),
    )?;
    let result = request(
        client,
        vec!["kill-window".to_owned(), "-t".to_owned(), tab.to_owned()],
        "command.kill.window",
        Intent::Mutation,
        Duration::from_secs(10),
    );
    let post = terminal_inventory_with_client(client);
    let post_verified = post.as_ref().is_ok_and(|snapshot| {
        snapshot["server_scope_id"] == before["server_scope_id"]
            && snapshot["server_epoch"] == before["server_epoch"]
            && !inventory_contains(snapshot, tab)
    });
    match result {
        Ok(response) if post_verified => {
            let post = post.expect("checked above");
            let evidence = json!({
                "tab_id": tab,
                "performed": true,
                "verified": true,
                "verification": "same-scope-epoch-inventory-absence",
                "server_scope_id": post["server_scope_id"],
                "server_epoch": post["server_epoch"],
                "control_receipt": response.receipt,
                "receipt": ticket.json(),
            });
            receipts.complete(&ticket, "terminal-close", 0, true, evidence.clone())?;
            Ok(evidence)
        }
        Ok(response) => {
            let evidence = json!({
                "tab_id": tab,
                "performed": true,
                "verified": false,
                "control_receipt": response.receipt,
                "postcheck_error": post.err().map(|error| json!({ "code": error.code, "message": error.message })),
                "receipt": ticket.json(),
            });
            receipts.complete(&ticket, "terminal-close", 0, false, evidence.clone())?;
            Err(CuError::new(
                "terminal_close_unverified",
                "AgenTerm accepted terminal close but exact tab disappearance was not verified",
            )
            .with_detail(evidence))
        }
        Err(error) if post_verified => {
            let post = post.expect("verified post-state is readable");
            let evidence = json!({
                "tab_id": tab,
                "performed": true,
                "verified": true,
                "verification": "same-scope-epoch-inventory-absence",
                "server_scope_id": post["server_scope_id"],
                "server_epoch": post["server_epoch"],
                "control_acknowledged": false,
                "control_error": { "code": error.code, "message": error.message },
                "receipt": ticket.json(),
            });
            receipts.complete(&ticket, "terminal-close", 0, true, evidence.clone())?;
            Ok(evidence)
        }
        Err(error) => {
            let evidence = json!({
                "tab_id": tab,
                "performed": post_verified,
                "verified": false,
                "error": { "code": error.code, "message": error.message },
                "receipt": ticket.json(),
            });
            receipts.complete(&ticket, "terminal-close", 0, false, evidence.clone())?;
            Err(error.with_detail(evidence))
        }
    }
}

fn inventory_tab<'a>(inventory: &'a Value, tab: &str) -> Option<&'a Value> {
    inventory["tabs"]
        .as_array()?
        .iter()
        .find(|row| row["id"].as_str() == Some(tab))
}

fn inventory_contains(inventory: &Value, tab: &str) -> bool {
    inventory_tab(inventory, tab).is_some()
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

pub(super) fn terminal_snapshot_payload(tab: &str) -> Result<Value, CuError> {
    validate_tab(tab)?;
    let client = client()?;
    terminal_snapshot_with_client(&client, tab)
}

pub(super) fn terminal_snapshot_with_client(
    client: &ControlClient,
    tab: &str,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    let response = request(
        client,
        vec!["ui-bootstrap".to_owned()],
        "ui.bootstrap",
        Intent::Query,
        Duration::from_secs(5),
    )?;
    let snapshot = parse_output(response, "terminal_snapshot_invalid")?;
    terminal_snapshot_from_bootstrap(client.server_scope_id(), tab, &snapshot)
}

fn terminal_snapshot_from_bootstrap(
    server_scope_id: &str,
    tab: &str,
    snapshot: &Value,
) -> Result<Value, CuError> {
    let epoch = snapshot["server_epoch"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CuError::new(
                "terminal_snapshot_invalid",
                "ui-bootstrap omitted server_epoch",
            )
        })?;
    let position_epoch = snapshot["position"]["server_epoch"].as_str();
    let sequence = snapshot["position"]["sequence"].as_u64();
    if position_epoch != Some(epoch) || sequence.is_none() {
        return Err(CuError::new(
            "terminal_snapshot_invalid",
            "ui-bootstrap returned an inconsistent event cursor",
        ));
    }
    let row = snapshot["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|row| row["id"].as_str() == Some(tab)))
        .cloned()
        .ok_or_else(|| {
            CuError::new(
                "terminal_tab_not_found",
                "terminal-snapshot target is not present in the current server epoch",
            )
            .with_detail(json!({ "tab_id": tab, "server_epoch": epoch }))
        })?;
    Ok(json!({
        "server_scope_id": server_scope_id,
        "server_epoch": epoch,
        "cursor": {
            "server_epoch": epoch,
            "sequence": sequence.expect("checked above"),
        },
        "tab": row,
        "snapshot_complete": snapshot["complete"],
        "snapshot_truncated": snapshot["truncated"],
        "read_kind": "bounded-structured-screen",
        "cursor_kind": "loss-aware-event-position",
        "identity": "server-scope+epoch+tab-id",
    }))
}

pub(super) fn terminal_scroll_payload(
    tab: &str,
    action: TerminalScrollAction,
    rows: Option<usize>,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    if rows == Some(0) || rows.is_some_and(|value| value > 1_000_000) {
        return Err(CuError::new(
            "terminal_scroll_rows_invalid",
            "terminal-scroll rows must be in 1..=1000000",
        ));
    }
    if matches!(
        action,
        TerminalScrollAction::Top | TerminalScrollAction::Bottom
    ) && rows.is_some()
    {
        return Err(CuError::new(
            "terminal_scroll_rows_invalid",
            "terminal-scroll top and bottom do not accept rows",
        ));
    }
    let client = client()?;
    terminal_scroll_with_client(&client, tab, action, rows, receipts)
}

fn terminal_screen_usize(snapshot: &Value, field: &str) -> Result<usize, CuError> {
    snapshot["tab"]["screen"][field]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            CuError::new(
                "terminal_scroll_snapshot_invalid",
                format!("terminal snapshot omitted a valid {field}"),
            )
        })
}

fn expected_scroll_offset(
    action: TerminalScrollAction,
    rows: Option<usize>,
    current: usize,
    maximum: usize,
    terminal_rows: usize,
) -> usize {
    let page = terminal_rows.saturating_sub(1).max(1);
    match action {
        TerminalScrollAction::Up => current.saturating_add(rows.unwrap_or(1)).min(maximum),
        TerminalScrollAction::Down => current.saturating_sub(rows.unwrap_or(1)),
        TerminalScrollAction::PageUp => current.saturating_add(rows.unwrap_or(page)).min(maximum),
        TerminalScrollAction::PageDown => current.saturating_sub(rows.unwrap_or(page)),
        TerminalScrollAction::Top => maximum,
        TerminalScrollAction::Bottom => 0,
    }
}

pub(super) fn terminal_scroll_with_client(
    client: &ControlClient,
    tab: &str,
    action: TerminalScrollAction,
    rows: Option<usize>,
    receipts: &mut ReceiptLog,
) -> Result<Value, CuError> {
    let before = terminal_snapshot_with_client(client, tab)?;
    if before["tab"]["screen"]["alternate_screen"].as_bool() == Some(true) {
        return Err(CuError::new(
            "terminal_viewport_unavailable",
            "local scrollback viewport is unavailable while the terminal owns the alternate screen",
        )
        .with_detail(json!({ "tab_id": tab, "reason": "alternate-screen" })));
    }
    let before_offset = terminal_screen_usize(&before, "scrollback_offset")?;
    let before_max = terminal_screen_usize(&before, "max_scrollback")?;
    let before_rows = terminal_screen_usize(&before, "rows")?;
    let before_columns = terminal_screen_usize(&before, "columns")?;
    let expected_offset =
        expected_scroll_offset(action, rows, before_offset, before_max, before_rows);
    let ticket = receipts.reserve(
        "terminal-scroll",
        0,
        json!({
            "tab_id": tab,
            "action": action.as_str(),
            "requested_rows": rows,
            "server_scope_id": before["server_scope_id"],
            "server_epoch": before["server_epoch"],
            "before_offset": before_offset,
            "before_max_offset": before_max,
        }),
    )?;
    let mut args = vec![
        "scroll-pane".to_owned(),
        "-t".to_owned(),
        tab.to_owned(),
        action.as_str().to_owned(),
    ];
    if let Some(rows) = rows {
        args.push(rows.to_string());
    }
    let result = request(
        client,
        args,
        "command.scroll.pane",
        Intent::Mutation,
        Duration::from_secs(5),
    );
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            receipts.complete(
                &ticket,
                "terminal-scroll",
                0,
                false,
                json!({ "performed": false, "error": { "code": error.code, "message": error.message } }),
            )?;
            return Err(error.with_detail(json!({ "receipt": ticket.json() })));
        }
    };
    let reported_offset = match response.output.trim().parse::<usize>() {
        Ok(offset) => offset,
        Err(_) => {
            let evidence = json!({
                "performed": true,
                "verified": false,
                "reply_bytes": response.output.len(),
                "postcheck_error": { "code": "terminal_scroll_reply_invalid" },
            });
            receipts.complete(&ticket, "terminal-scroll", 0, false, evidence.clone())?;
            return Err(CuError::new(
                "terminal_scroll_unverified",
                "AgenTerm scrolled the viewport but returned a non-numeric offset",
            )
            .with_detail(json!({ "evidence": evidence, "receipt": ticket.json() })));
        }
    };
    let after = match terminal_snapshot_with_client(client, tab) {
        Ok(after) => after,
        Err(error) => {
            let evidence = json!({
                "performed": true,
                "verified": false,
                "reported_offset": reported_offset,
                "postcheck_error": { "code": error.code, "message": error.message },
            });
            receipts.complete(&ticket, "terminal-scroll", 0, false, evidence.clone())?;
            return Err(CuError::new(
                "terminal_scroll_unverified",
                "AgenTerm scrolled the viewport but its exact post-state was unreadable",
            )
            .with_detail(json!({ "evidence": evidence, "receipt": ticket.json() })));
        }
    };
    let post_fields = (|| {
        Ok::<_, CuError>((
            terminal_screen_usize(&after, "scrollback_offset")?,
            terminal_screen_usize(&after, "max_scrollback")?,
            terminal_screen_usize(&after, "rows")?,
            terminal_screen_usize(&after, "columns")?,
        ))
    })();
    let (after_offset, after_max, after_rows, after_columns) = match post_fields {
        Ok(fields) => fields,
        Err(error) => {
            let evidence = json!({
                "performed": true,
                "verified": false,
                "reported_offset": reported_offset,
                "postcheck_error": { "code": error.code, "message": error.message },
            });
            receipts.complete(&ticket, "terminal-scroll", 0, false, evidence.clone())?;
            return Err(CuError::new(
                "terminal_scroll_unverified",
                "AgenTerm scrolled the viewport but returned an incomplete post-state",
            )
            .with_detail(json!({ "evidence": evidence, "receipt": ticket.json() })));
        }
    };
    let same_identity = after["server_scope_id"] == before["server_scope_id"]
        && after["server_epoch"] == before["server_epoch"]
        && after["tab"]["id"].as_str() == Some(tab);
    let verified = same_identity
        && reported_offset == after_offset
        && after_offset == expected_offset
        && before_max == after_max
        && before_rows == after_rows
        && before_columns == after_columns
        && after["tab"]["screen"]["alternate_screen"].as_bool() == Some(false);
    let evidence = json!({
        "tab_id": tab,
        "server_scope_id": after["server_scope_id"],
        "server_epoch": after["server_epoch"],
        "action": action.as_str(),
        "requested_rows": rows,
        "before_offset": before_offset,
        "after_offset": after_offset,
        "expected_offset": expected_offset,
        "before_max_offset": before_max,
        "after_max_offset": after_max,
        "rows": after_rows,
        "columns": after_columns,
        "performed": true,
        "viewport_changed": before_offset != after_offset,
        "effective_rows": before_offset.abs_diff(after_offset),
        "verified": verified,
        "verification": "same-scope-epoch-tab-and-structured-offset-readback",
        "control_receipt": response.receipt,
    });
    receipts.complete(&ticket, "terminal-scroll", 0, verified, evidence.clone())?;
    if verified {
        let mut payload = evidence;
        payload["receipt"] = ticket.json();
        Ok(payload)
    } else {
        Err(CuError::new(
            "terminal_scroll_unverified",
            "viewport mutation did not preserve and verify the exact terminal identity and grid",
        )
        .with_detail(json!({ "evidence": evidence, "receipt": ticket.json() })))
    }
}

pub(super) fn terminal_screenshot_payload(tab: &str, out: &str) -> Result<Value, CuError> {
    validate_tab(tab)?;
    let path = Path::new(out);
    if out.is_empty() || out.len() > 8_192 || out.as_bytes().contains(&0) || !path.is_absolute() {
        return Err(CuError::new(
            "terminal_screenshot_path_invalid",
            "terminal-screenshot --out must be an absolute 1..=8192-byte non-NUL path",
        ));
    }
    let client = client()?;
    let before = terminal_snapshot_with_client(&client, tab)?;
    let response = request(
        &client,
        vec![
            "screenshot-pane".to_owned(),
            "-t".to_owned(),
            tab.to_owned(),
            "-o".to_owned(),
            out.to_owned(),
            "--json".to_owned(),
        ],
        "command.screenshot.pane",
        Intent::Query,
        Duration::from_secs(15),
    )?;
    let product = parse_output(response, "terminal_screenshot_reply_invalid")?;
    let after = terminal_snapshot_with_client(&client, tab)?;
    let opened = agenterm_platform::filesystem_open::open_existing(
        path,
        agenterm_platform::filesystem_open::ExistingEntryType::File,
    )
    .map_err(|error| {
        CuError::new(
            "terminal_screenshot_output_invalid",
            format!("open published terminal screenshot without following links: {error}"),
        )
    })?;
    let opened_identity =
        agenterm_platform::file_identity::file_identity(&opened).map_err(|error| {
            CuError::new(
                "terminal_screenshot_output_invalid",
                format!("read published terminal screenshot identity: {error}"),
            )
        })?;
    let path_identity = agenterm_platform::file_identity::path_identity(path).map_err(|error| {
        CuError::new(
            "terminal_screenshot_output_invalid",
            format!("revalidate published terminal screenshot identity: {error}"),
        )
    })?;
    if !opened_identity.same_object(path_identity) {
        return Err(CuError::new(
            "terminal_screenshot_output_changed",
            "terminal screenshot path changed while its published file was being verified",
        ));
    }
    let byte_count = opened
        .metadata()
        .map_err(|error| {
            CuError::new(
                "terminal_screenshot_output_invalid",
                format!("inspect published terminal screenshot: {error}"),
            )
        })?
        .len();
    if !(24..=268_435_456).contains(&byte_count) {
        return Err(CuError::new(
            "terminal_screenshot_output_invalid",
            "published terminal screenshot size must be in 24..=268435456 bytes",
        ));
    }
    let mut file = opened;
    let mut header = [0_u8; 24];
    file.read_exact(&mut header).map_err(|error| {
        CuError::new(
            "terminal_screenshot_output_invalid",
            format!("read published terminal screenshot header: {error}"),
        )
    })?;
    if header[..8] != [137, 80, 78, 71, 13, 10, 26, 10] || &header[12..16] != b"IHDR" {
        return Err(CuError::new(
            "terminal_screenshot_output_invalid",
            "published terminal screenshot is not a PNG with an IHDR header",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(header);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            CuError::new(
                "terminal_screenshot_output_invalid",
                format!("hash published terminal screenshot: {error}"),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut sha256 = String::with_capacity(64);
    for byte in digest {
        write!(&mut sha256, "{byte:02x}").expect("writing into a String cannot fail");
    }
    let before_screen = &before["tab"]["screen"];
    let after_screen = &after["tab"]["screen"];
    let product_path = product["path"].as_str();
    let verified = product["schema_version"].as_u64() == Some(1)
        && product_path == Some(out)
        && product["tab_id"].as_str() == Some(tab)
        && product["target_active"].as_bool() == Some(true)
        && product["server_epoch"] == before["server_epoch"]
        && product["server_epoch"] == after["server_epoch"]
        && before["server_scope_id"] == after["server_scope_id"]
        && product["screen_generation"] == before_screen["generation"]
        && product["screen_generation"] == after_screen["generation"]
        && product["scrollback_offset"] == before_screen["scrollback_offset"]
        && product["scrollback_offset"] == after_screen["scrollback_offset"]
        && product["rows"] == before_screen["rows"]
        && product["rows"] == after_screen["rows"]
        && product["columns"] == before_screen["columns"]
        && product["columns"] == after_screen["columns"]
        && product["pixel_width"]
            .as_u64()
            .is_some_and(|value| value > 0)
        && product["pixel_height"]
            .as_u64()
            .is_some_and(|value| value > 0)
        && product["byte_count"].as_u64() == Some(byte_count)
        && product["sha256"].as_str() == Some(sha256.as_str());
    let evidence = json!({
        "path": out,
        "tab_id": tab,
        "server_scope_id": after["server_scope_id"],
        "server_epoch": after["server_epoch"],
        "screen_generation": product["screen_generation"],
        "scrollback_offset": product["scrollback_offset"],
        "rows": product["rows"],
        "columns": product["columns"],
        "pixel_width": product["pixel_width"],
        "pixel_height": product["pixel_height"],
        "bytes": byte_count,
        "sha256": sha256,
        "performed": true,
        "verified": verified,
        "focus_changed": false,
        "content_returned": false,
        "publication": "atomic-no-clobber",
        "identity": "server-scope+epoch+tab+screen-generation+scrollback+png-sha256",
    });
    if verified {
        Ok(evidence)
    } else {
        Err(CuError::new(
            "terminal_screenshot_unverified",
            "rendered screenshot did not match one stable active terminal frame and published PNG",
        )
        .with_detail(evidence))
    }
}

pub(super) fn terminal_events_payload(
    tab: &str,
    epoch: &str,
    after: u64,
    limit: usize,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    validate_event_request(epoch, limit)?;
    let client = client()?;
    terminal_events_with_client(&client, tab, epoch, after, limit)
}

pub(super) fn terminal_events_with_client(
    client: &ControlClient,
    tab: &str,
    epoch: &str,
    after: u64,
    limit: usize,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    validate_event_request(epoch, limit)?;
    let response = request(
        client,
        vec![
            "ui-deltas".to_owned(),
            "--epoch".to_owned(),
            epoch.to_owned(),
            "--after".to_owned(),
            after.to_string(),
            "--limit".to_owned(),
            limit.to_string(),
        ],
        "ui.deltas",
        Intent::Query,
        Duration::from_secs(5),
    )?;
    let batch = parse_output(response, "terminal_events_invalid")?;
    terminal_events_from_delta(client.server_scope_id(), tab, epoch, after, &batch)
}

pub(super) fn terminal_output_payload(
    tab: &str,
    cursor: &str,
    max_bytes: usize,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    if cursor != "earliest" && cursor != "current" && cursor.parse::<u64>().is_err() {
        return Err(CuError::new(
            "terminal_output_cursor_invalid",
            "terminal-output --cursor must be earliest, current, or a non-negative integer",
        ));
    }
    if !(1..=CAPTURE_MAX_BYTES).contains(&max_bytes) {
        return Err(CuError::new(
            "terminal_output_limit_invalid",
            "terminal-output --max-bytes must be in 1..=1048576",
        ));
    }
    let client = client()?;
    terminal_output_with_client(&client, tab, cursor, max_bytes)
}

pub(super) fn terminal_output_with_client(
    client: &ControlClient,
    tab: &str,
    cursor: &str,
    max_bytes: usize,
) -> Result<Value, CuError> {
    validate_tab(tab)?;
    if cursor != "earliest" && cursor != "current" && cursor.parse::<u64>().is_err() {
        return Err(CuError::new(
            "terminal_output_cursor_invalid",
            "terminal-output --cursor must be earliest, current, or a non-negative integer",
        ));
    }
    if !(1..=CAPTURE_MAX_BYTES).contains(&max_bytes) {
        return Err(CuError::new(
            "terminal_output_limit_invalid",
            "terminal-output --max-bytes must be in 1..=1048576",
        ));
    }
    let response = request(
        client,
        vec![
            "capture-output".to_owned(),
            "-t".to_owned(),
            tab.to_owned(),
            "--cursor".to_owned(),
            cursor.to_owned(),
            "--max-bytes".to_owned(),
            max_bytes.to_string(),
        ],
        "command.capture.output",
        Intent::Query,
        Duration::from_secs(5),
    )?;
    let mut output = parse_output(response, "terminal_output_invalid")?;
    if output["tab_id"].as_str() != Some(tab)
        || output["encoding"].as_str() != Some("base64")
        || output["data_base64"].as_str().is_none()
        || output["start_cursor"].as_u64().is_none()
        || output["next_cursor"].as_u64().is_none()
        || output["earliest_cursor"].as_u64().is_none()
        || output["current_cursor"].as_u64().is_none()
    {
        return Err(CuError::new(
            "terminal_output_invalid",
            "capture-output returned an inconsistent raw-output cursor",
        ));
    }
    output["server_scope_id"] = json!(client.server_scope_id());
    output["cursor_kind"] = json!("loss-aware-raw-output-byte-position");
    output["identity"] = json!("server-scope+tab-id+raw-output-cursor");
    output["content_read"] = json!(true);
    Ok(output)
}

fn validate_event_request(epoch: &str, limit: usize) -> Result<(), CuError> {
    if epoch.is_empty() || epoch.len() > 128 || epoch.chars().any(char::is_control) {
        return Err(CuError::new(
            "terminal_event_epoch_invalid",
            "terminal-events --epoch must be 1..=128 non-control bytes",
        ));
    }
    if !(1..=64).contains(&limit) {
        return Err(CuError::new(
            "terminal_event_limit_invalid",
            "terminal-events --limit must be in 1..=64",
        ));
    }
    Ok(())
}

fn terminal_events_from_delta(
    server_scope_id: &str,
    tab: &str,
    requested_epoch: &str,
    requested_after: u64,
    batch: &Value,
) -> Result<Value, CuError> {
    let epoch = batch["server_epoch"].as_str();
    let after = batch["after_sequence"].as_u64();
    let through = batch["through_sequence"].as_u64();
    let current = batch["current_sequence"].as_u64();
    if epoch != Some(requested_epoch)
        || after != Some(requested_after)
        || through.is_none()
        || current.is_none()
        || through > current
    {
        return Err(CuError::new(
            "terminal_events_invalid",
            "ui-deltas returned an inconsistent event cursor",
        ));
    }
    let all_events = batch["events"]
        .as_array()
        .ok_or_else(|| CuError::new("terminal_events_invalid", "ui-deltas omitted events"))?;
    let events = all_events
        .iter()
        .filter(|event| event["tab_id"].as_str() == Some(tab))
        .cloned()
        .collect::<Vec<_>>();
    let tab_updates = batch["tab_updates"]
        .as_array()
        .ok_or_else(|| CuError::new("terminal_events_invalid", "ui-deltas omitted tab_updates"))?
        .iter()
        .filter(|row| row["id"].as_str() == Some(tab))
        .cloned()
        .collect::<Vec<_>>();
    let closed = batch["closed_tab_ids"]
        .as_array()
        .ok_or_else(|| {
            CuError::new(
                "terminal_events_invalid",
                "ui-deltas omitted closed_tab_ids",
            )
        })?
        .iter()
        .any(|id| id.as_str() == Some(tab));
    Ok(json!({
        "server_scope_id": server_scope_id,
        "server_epoch": requested_epoch,
        "tab_id": tab,
        "cursor": {
            "server_epoch": requested_epoch,
            "sequence": through.expect("checked above"),
        },
        "current_sequence": current.expect("checked above"),
        "scanned_events": all_events.len(),
        "returned_events": events.len(),
        "events": events,
        "tab_updates": tab_updates,
        "closed": closed,
        "complete": batch["complete"],
        "truncated": batch["truncated"],
        "cursor_kind": "loss-aware-event-position",
        "cursor_advance": "all-scanned-events-including-other-tabs",
        "identity": "server-scope+epoch+tab-id",
    }))
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
    terminal_send_with_client(&client, tab, text, receipts)
}

pub(super) fn terminal_send_with_client(
    client: &ControlClient,
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
    let ticket = receipts.reserve(
        "terminal-send",
        0,
        json!({ "tab_id": tab, "text_bytes": text.len(), "before": "unknown" }),
    )?;
    let result = request(
        client,
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
    terminal_wait_with_client(&client, tab, condition, timeout_ms)
}

pub(super) fn terminal_wait_with_client(
    client: &ControlClient,
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
                    terminal_read_with_client(client, tab, CAPTURE_MAX_BYTES, request_timeout)?;
                value["text"]
                    .as_str()
                    .is_some_and(|text| text.contains(needle))
            }
            TerminalWaitCondition::Exited | TerminalWaitCondition::Finalized => {
                let response = request(
                    client,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_snapshot_binds_tab_to_server_cursor() {
        let value = terminal_snapshot_from_bootstrap(
            "scope-a",
            "@2",
            &json!({
                "server_epoch": "epoch-a",
                "position": { "server_epoch": "epoch-a", "sequence": 7 },
                "tabs": [
                    { "id": "@1", "screen": { "cursor": { "row": 0, "column": 0 } } },
                    { "id": "@2", "screen": { "cursor": { "row": 3, "column": 4 }, "complete": true, "truncated": false } }
                ],
                "complete": true,
                "truncated": false
            }),
        )
        .unwrap();
        assert_eq!(value["cursor"]["sequence"], 7);
        assert_eq!(value["tab"]["id"], "@2");
        assert_eq!(value["tab"]["screen"]["cursor"]["column"], 4);
        assert_eq!(value["cursor_kind"], "loss-aware-event-position");
    }

    #[test]
    fn event_page_filters_payload_but_advances_over_other_tabs() {
        let value = terminal_events_from_delta(
            "scope-a",
            "@2",
            "epoch-a",
            4,
            &json!({
                "server_epoch": "epoch-a",
                "after_sequence": 4,
                "through_sequence": 7,
                "current_sequence": 9,
                "events": [
                    { "sequence": 5, "kind": "terminal.output", "tab_id": "@1" },
                    { "sequence": 6, "kind": "terminal.output", "tab_id": "@2" },
                    { "sequence": 7, "kind": "focus.changed", "tab_id": null }
                ],
                "tab_updates": [
                    { "id": "@1" },
                    { "id": "@2", "screen": { "generation": 7 } }
                ],
                "closed_tab_ids": [],
                "complete": false,
                "truncated": true
            }),
        )
        .unwrap();
        assert_eq!(value["cursor"]["sequence"], 7);
        assert_eq!(value["scanned_events"], 3);
        assert_eq!(value["returned_events"], 1);
        assert_eq!(value["events"][0]["tab_id"], "@2");
        assert_eq!(value["tab_updates"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["cursor_advance"],
            "all-scanned-events-including-other-tabs"
        );
    }

    #[test]
    fn event_page_rejects_identity_drift() {
        let error = terminal_events_from_delta(
            "scope-a",
            "@2",
            "epoch-a",
            4,
            &json!({
                "server_epoch": "epoch-b",
                "after_sequence": 4,
                "through_sequence": 4,
                "current_sequence": 4,
                "events": [], "tab_updates": [], "closed_tab_ids": [],
                "complete": true, "truncated": false
            }),
        )
        .unwrap_err();
        assert_eq!(error.code, "terminal_events_invalid");
    }

    #[test]
    fn viewport_scroll_expectations_match_product_clamping() {
        assert_eq!(
            expected_scroll_offset(TerminalScrollAction::Up, Some(9), 4, 10, 24),
            10
        );
        assert_eq!(
            expected_scroll_offset(TerminalScrollAction::Down, Some(9), 4, 10, 24),
            0
        );
        assert_eq!(
            expected_scroll_offset(TerminalScrollAction::PageUp, None, 2, 100, 24),
            25
        );
        assert_eq!(
            expected_scroll_offset(TerminalScrollAction::PageDown, None, 25, 100, 24),
            2
        );
        assert_eq!(
            expected_scroll_offset(TerminalScrollAction::Top, None, 2, 100, 24),
            100
        );
        assert_eq!(
            expected_scroll_offset(TerminalScrollAction::Bottom, None, 100, 100, 24),
            0
        );
    }
}
