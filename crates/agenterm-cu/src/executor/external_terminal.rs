//! Exact-window adoption of external desktop terminals.
//!
//! This is deliberately separate from `terminal-*`, whose authority is an
//! AgenTerm server epoch plus stable `@tab`.  Here the authority is one native
//! top-level window and its owning process, revalidated around every tree read.

use super::*;
use crate::execution_control::ExecutionControl;

use regex::Regex;

const TREE_DEPTH: u32 = 16;
const TREE_NODES: usize = 5_000;
const MAX_TEXT_BYTES: usize = 1_048_576;
const MAX_PATTERN_BYTES: usize = 4_096;
const MAX_SEND_BYTES: usize = 65_536;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExternalWindowIdentity {
    handle: isize,
    pid: u32,
    start_identity: String,
    app: String,
}

impl ExternalWindowIdentity {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "handle": self.handle,
            "pid": self.pid,
            "start_identity": self.start_identity,
            "app": self.app,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TerminalBuffer {
    node: String,
    role: String,
    backend: String,
    text: String,
}

fn bind_window(window: isize) -> Result<ExternalWindowIdentity, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "term requires a non-zero window handle from `windows`".into(),
        ));
    }
    let rows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let row = rows
        .into_iter()
        .find(|row| row.handle == window)
        .ok_or_else(|| {
            CuError::new(
                "terminal_window_gone",
                format!("window {window} is not in the current top-level inventory"),
            )
        })?;
    let start_identity = match agenterm_platform::process_observation::observe(row.process_id) {
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: Some(identity),
        } => identity,
        agenterm_platform::process_observation::ProcessObservation::Live {
            start_identity: None,
        } => {
            return Err(CuError::new(
                "terminal_process_identity_unavailable",
                "the external terminal owner has no stable process-start identity",
            ));
        }
        agenterm_platform::process_observation::ProcessObservation::Dead { .. } => {
            return Err(CuError::new(
                "terminal_window_gone",
                "the external terminal owner exited during identity binding",
            ));
        }
        agenterm_platform::process_observation::ProcessObservation::Unknown { reason } => {
            return Err(CuError::new(
                "terminal_process_identity_unavailable",
                reason,
            ));
        }
        _ => {
            return Err(CuError::new(
                "terminal_process_identity_unavailable",
                "the external terminal owner has an unsupported observation state",
            ));
        }
    };
    Ok(ExternalWindowIdentity {
        handle: row.handle,
        pid: row.process_id,
        start_identity,
        app: row.app_name,
    })
}

fn revalidate_window(expected: &ExternalWindowIdentity) -> Result<(), CuError> {
    let observed = bind_window(expected.handle)?;
    if observed == *expected {
        return Ok(());
    }
    Err(CuError::new(
        "terminal_window_identity_changed",
        "external terminal window identity changed while the operation was in flight",
    )
    .with_detail(serde_json::json!({
        "expected": expected.json(),
        "observed": observed.json(),
    })))
}

fn terminal_role(role: &str) -> bool {
    matches!(
        observe::normalize_role(role).as_str(),
        "textarea" | "scrollarea" | "terminal" | "document"
    )
}

fn shallow_terminal_tree(tree: &mechanism::A11yTree) -> bool {
    tree.visited == 1
        && !tree.truncated
        && tree.returned == 1
        && tree.nodes.len() == 1
        && observe::normalize_role(&tree.nodes[0].role) == "frame"
}

fn terminal_buffer_not_found_error(tree: &mechanism::A11yTree) -> CuError {
    let error = CuError::new(
        "terminal_buffer_not_found",
        "the exact window exposes no showing terminal text buffer",
    );
    if shallow_terminal_tree(tree) {
        return error.with_detail(crate::host_limit::terminal_a11y_shallow_tree_detail(
            &serde_json::json!({
                "backend": tree.backend,
                "visited": tree.visited,
                "returned": tree.returned,
                "truncated": tree.truncated,
                "root_role": tree.nodes[0].role,
            }),
        ));
    }
    error
}

fn select_candidate(mut candidates: Vec<TerminalBuffer>) -> Result<TerminalBuffer, CuError> {
    if candidates.is_empty() {
        return Err(CuError::new(
            "terminal_buffer_not_found",
            "the exact window exposes no showing terminal text buffer",
        ));
    }
    candidates.sort_by(|left, right| {
        right
            .text
            .len()
            .cmp(&left.text.len())
            .then_with(|| {
                right
                    .node
                    .matches('/')
                    .count()
                    .cmp(&left.node.matches('/').count())
            })
            .then_with(|| left.node.cmp(&right.node))
    });
    if candidates.get(1).is_some_and(|other| {
        other.text.len() == candidates[0].text.len()
            && other.node.matches('/').count() == candidates[0].node.matches('/').count()
    }) {
        return Err(CuError::new(
            "terminal_buffer_ambiguous",
            "multiple equally plausible terminal text buffers are showing; refusing to guess",
        )
        .with_count(candidates.len()));
    }
    Ok(candidates.remove(0))
}

fn read_buffer(identity: &ExternalWindowIdentity) -> Result<TerminalBuffer, CuError> {
    revalidate_window(identity)?;
    let tree = mechanism::tree_for_window_bounded(
        Some(identity.handle),
        mechanism::TreeBudget {
            max_depth: Some(TREE_DEPTH),
            max_nodes: Some(TREE_NODES),
        },
    )
    .map_err(map_mechanism_err)?;
    if tree.truncated {
        return Err(CuError::new(
            "terminal_tree_truncated",
            "terminal buffer selection cannot be proven inside the bounded accessibility tree",
        )
        .with_detail(serde_json::json!({
            "visited": tree.visited,
            "returned": tree.returned,
            "max_depth": TREE_DEPTH,
            "max_nodes": TREE_NODES,
        })));
    }
    let mut candidates = Vec::new();
    let mut text_error = None;
    for node in tree
        .nodes
        .iter()
        .filter(|node| node_is_showing(node) && terminal_role(&node.role))
    {
        match mechanism::get_node_text(Some(identity.handle), &node.id) {
            Ok(text) => candidates.push(TerminalBuffer {
                node: node.id.clone(),
                role: node.role.clone(),
                backend: tree.backend.clone(),
                text,
            }),
            Err(mechanism::MechanismError::Failed { code, message })
                if code == "a11y_text_unavailable" =>
            {
                text_error.get_or_insert(CuError::new(code, message));
            }
            Err(error) => return Err(map_mechanism_err(error)),
        }
    }
    revalidate_window(identity)?;
    if candidates.is_empty() {
        return Err(text_error.unwrap_or_else(|| terminal_buffer_not_found_error(&tree)));
    }
    select_candidate(candidates)
}

fn utf8_suffix(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (&text[start..], true)
}

fn source_complete(backend: &str) -> bool {
    // AppKit returns the AXValue string itself. UIA and AT-SPI adapters use
    // bounded TextPattern/GetText reads without a completeness bit, so they
    // are observed prefixes and must never support an authoritative absence.
    backend == "ax"
}

fn shape_read(
    identity: &ExternalWindowIdentity,
    buffer: TerminalBuffer,
    tail: Option<usize>,
    raw: bool,
    max_bytes: usize,
) -> serde_json::Value {
    let mut lines = buffer.text.split('\n').collect::<Vec<_>>();
    if !raw {
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
    }
    let lines_total = lines.len();
    let selected = tail
        .map(|count| &lines[lines.len().saturating_sub(count)..])
        .unwrap_or(lines.as_slice())
        .join("\n");
    let (text, truncated_head) = utf8_suffix(&selected, max_bytes);
    let source_complete = source_complete(&buffer.backend);
    serde_json::json!({
        "addressing": "exact-desktop-window",
        "mechanism": "libagenterm-accessibility-text",
        "window_identity": identity.json(),
        "backend": buffer.backend,
        "node": buffer.node,
        "role": buffer.role,
        "lines": lines_total,
        "tail": tail,
        "raw": raw,
        "max_bytes": max_bytes,
        "source_complete": source_complete,
        "tail_scope": if source_complete { "complete-buffer" } else { "observed-prefix" },
        "truncated_head": truncated_head,
        "text": text,
    })
}

pub(super) fn term_read_payload(
    window: isize,
    tail: Option<usize>,
    raw: bool,
    max_bytes: usize,
) -> Result<serde_json::Value, CuError> {
    validate_read_bounds(tail, max_bytes)?;
    let identity = bind_window(window)?;
    let buffer = read_buffer(&identity)?;
    Ok(shape_read(&identity, buffer, tail, raw, max_bytes))
}

fn validate_read_bounds(tail: Option<usize>, max_bytes: usize) -> Result<(), CuError> {
    if !(1..=MAX_TEXT_BYTES).contains(&max_bytes) {
        return Err(invalid_input(
            "term --max-bytes must be in 1..=1048576".into(),
        ));
    }
    if tail.is_some_and(|value| !(1..=100_000).contains(&value)) {
        return Err(invalid_input(
            "term read --tail must be in 1..=100000".into(),
        ));
    }
    Ok(())
}

fn focused_handle() -> Result<Option<isize>, CuError> {
    let rows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    Ok(rows
        .into_iter()
        .find(|row| row.focused)
        .map(|row| row.handle))
}

fn wait_for_focus(window: isize, wanted: bool) -> Result<bool, CuError> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(1_500) {
        let focused = focused_handle()? == Some(window);
        if focused == wanted {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(false)
}

fn restore_focus(previous: Option<isize>, target: isize) -> Result<bool, CuError> {
    let Some(previous) = previous.filter(|handle| *handle != target) else {
        return Ok(true);
    };
    mechanism::window_op::activate(previous).map_err(map_mechanism_err)?;
    wait_for_focus(previous, true)
}

struct DeliveryFailure {
    error: CuError,
    input_dispatched: bool,
    focus_restored: Option<bool>,
}

fn send_foreground(
    identity: &ExternalWindowIdentity,
    node: &str,
    text: &str,
    enter: bool,
) -> Result<bool, DeliveryFailure> {
    let window = identity.handle;
    let previous = focused_handle().map_err(|error| DeliveryFailure {
        error,
        input_dispatched: false,
        focus_restored: None,
    })?;
    mechanism::window_op::activate(window).map_err(|error| DeliveryFailure {
        error: map_mechanism_err(error),
        input_dispatched: false,
        focus_restored: None,
    })?;
    match wait_for_focus(window, true) {
        Ok(true) => {}
        Ok(false) => {
            let restored = restore_focus(previous, window).ok();
            return Err(DeliveryFailure {
                error: CuError::new(
                    "terminal_foreground_unverified",
                    "the exact terminal window did not become the foreground owner",
                ),
                input_dispatched: false,
                focus_restored: restored,
            });
        }
        Err(error) => {
            let restored = restore_focus(previous, window).ok();
            return Err(DeliveryFailure {
                error,
                input_dispatched: false,
                focus_restored: restored,
            });
        }
    }
    if let Err(error) = revalidate_window(identity) {
        let restored = restore_focus(previous, window).ok();
        return Err(DeliveryFailure {
            error,
            input_dispatched: false,
            focus_restored: restored,
        });
    }
    if let Err(error) =
        mechanism::perform_node_action(Some(window), node, mechanism::NodeAction::Focus)
    {
        let restored = restore_focus(previous, window).ok();
        return Err(DeliveryFailure {
            error: map_mechanism_err(error),
            input_dispatched: false,
            focus_restored: restored,
        });
    }
    let focused = mechanism::focused_node(Some(window)).map_err(|error| DeliveryFailure {
        error: map_mechanism_err(error),
        input_dispatched: false,
        focus_restored: restore_focus(previous, window).ok(),
    })?;
    if focused.nodes.len() != 1 || focused.nodes[0].id != node {
        let restored = restore_focus(previous, window).ok();
        return Err(DeliveryFailure {
            error: CuError::new(
                "terminal_input_focus_unverified",
                "the selected terminal buffer did not become the application-local focused node",
            ),
            input_dispatched: false,
            focus_restored: restored,
        });
    }
    if let Err(error) = revalidate_window(identity) {
        let restored = restore_focus(previous, window).ok();
        return Err(DeliveryFailure {
            error,
            input_dispatched: false,
            focus_restored: restored,
        });
    }
    let mut input_dispatched = false;
    let effect = if !text.is_empty() {
        // Native injectors can report a short write only after a prefix has
        // left this process. Mark uncertainty before crossing that boundary.
        input_dispatched = true;
        match mechanism::input_inject::type_text(text).map_err(map_mechanism_err) {
            Ok(()) => Ok(()),
            Err(error) => Err(error),
        }
    } else {
        Ok(())
    }
    .and_then(|()| {
        if !enter {
            return Ok(());
        }
        input_dispatched = true;
        mechanism::input_inject::send_keys("enter").map_err(map_mechanism_err)?;
        Ok(())
    });
    let restored = restore_focus(previous, window);
    if let Err(error) = effect {
        return Err(DeliveryFailure {
            error,
            input_dispatched,
            focus_restored: restored.ok(),
        });
    }
    let restored = restored.map_err(|error| DeliveryFailure {
        error,
        input_dispatched,
        focus_restored: Some(false),
    })?;
    Ok(restored)
}

pub(super) fn term_send_payload(
    window: isize,
    text: &str,
    expect: Option<&str>,
    enter: bool,
    foreground: bool,
    verify_timeout_ms: u64,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if text.is_empty() && !enter {
        return Err(invalid_input(
            "term send requires non-empty text or the default Enter action".into(),
        ));
    }
    if text.len() > MAX_SEND_BYTES {
        return Err(invalid_input("term send text exceeds 65536 bytes".into()));
    }
    if !(1..=30_000).contains(&verify_timeout_ms) {
        return Err(invalid_input(
            "term send --verify-timeout-ms must be in 1..=30000".into(),
        ));
    }
    if !foreground {
        return Err(CuError::new(
            "terminal_background_input_unavailable",
            "no current host has a proven exact-window background literal-text provider; use explicit --foreground or keep the operation observational",
        ));
    }
    if std::env::var_os("AGENTERM_NO_ACTIVATE").is_some() {
        return Err(CuError::new(
            "terminal_foreground_forbidden",
            "AGENTERM_NO_ACTIVATE is set, so the explicit foreground terminal transaction was not started",
        ));
    }
    let expectation_source = match expect {
        Some(pattern) if pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES => {
            return Err(invalid_input(
                "term send --expect must be 1..=4096 UTF-8 bytes".into(),
            ));
        }
        Some(pattern) => pattern.to_owned(),
        None if text.is_empty() => {
            return Err(invalid_input(
                "term send with empty text requires --expect PATTERN for independent attribution"
                    .into(),
            ));
        }
        None => regex::escape(text),
    };
    let expectation = Regex::new(&expectation_source).map_err(|_| {
        CuError::new(
            "terminal_pattern_invalid",
            "term send --expect is not a valid bounded Rust regular expression",
        )
        .with_detail(serde_json::json!({ "pattern_bytes": expectation_source.len() }))
    })?;
    let identity = bind_window(window)?;
    let before = read_buffer(&identity)?;
    if expectation.is_match(&before.text) {
        return Err(CuError::new(
            "terminal_expectation_already_satisfied",
            "the send postcondition already matches the terminal buffer, so a new effect cannot be attributed",
        )
        .with_detail(serde_json::json!({
            "pattern_sha256": super::clipboard::clipboard_sha256_hex(expectation_source.as_bytes()),
            "pattern_bytes": expectation_source.len(),
            "content_disclosed": false,
        })));
    }
    revalidate_window(&identity)?;
    let ticket = receipts.reserve(
        "term-send",
        window,
        serde_json::json!({
            "action": "external-terminal-input",
            "window_identity": identity.json(),
            "node": before.node,
            "text_bytes": text.len(),
            "text_sha256": super::clipboard::clipboard_sha256_hex(text.as_bytes()),
            "expect_sha256": super::clipboard::clipboard_sha256_hex(expectation_source.as_bytes()),
            "expect_bytes": expectation_source.len(),
            "enter": enter,
            "foreground": foreground,
        }),
    )?;
    let delivery = send_foreground(&identity, &before.node, text, enter)
        .map(|restored| ("foreground-node-focus+inject+restore", restored));
    let (via, focus_restored) = match delivery {
        Ok(value) => value,
        Err(failure) => {
            let mapped = CuError::new(
                failure.error.code,
                "the foreground terminal transaction failed; literal input is redacted",
            );
            if !failure.input_dispatched {
                receipts.complete(
                    &ticket,
                    "term-send",
                    window,
                    false,
                    serde_json::json!({
                        "performed": false,
                        "focus_restored": failure.focus_restored,
                        "error": error_payload(&mapped),
                    }),
                )?;
            }
            return Err(mapped.with_detail(serde_json::json!({
                "receipt": ticket.json(),
                "input_dispatched": failure.input_dispatched,
                "focus_restored": failure.focus_restored,
                "outcome": if failure.input_dispatched { "unknown" } else { "failed" },
            })));
        }
    };
    let started = Instant::now();
    let deadline = started + Duration::from_millis(verify_timeout_ms);
    let mut after = before.text.clone();
    let mut after_node = before.node.clone();
    let mut polls = 0usize;
    while Instant::now() < deadline {
        polls += 1;
        let observed = match read_buffer(&identity) {
            Ok(buffer) => buffer,
            Err(error) => {
                return Err(CuError::new(
                    "terminal_input_outcome_unknown",
                    "terminal input was dispatched but independent buffer read-back failed",
                )
                .with_detail(serde_json::json!({
                    "receipt": ticket.json(),
                    "cause": error_payload(&error),
                    "outcome": "unknown",
                })));
            }
        };
        after_node = observed.node;
        after = observed.text;
        if after_node != before.node {
            return Err(CuError::new(
                "terminal_input_outcome_unknown",
                "terminal input was dispatched but the selected buffer identity changed before read-back",
            )
            .with_detail(serde_json::json!({
                "receipt": ticket.json(),
                "outcome": "unknown",
                "before_node_sha256": super::clipboard::clipboard_sha256_hex(before.node.as_bytes()),
                "after_node_sha256": super::clipboard::clipboard_sha256_hex(after_node.as_bytes()),
            })));
        }
        if expectation.is_match(&after) {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        thread::sleep(remaining.min(Duration::from_millis(50)));
    }
    let expectation_matched = expectation.is_match(&after);
    let verified = expectation_matched && after_node == before.node && focus_restored;
    let evidence = serde_json::json!({
        "performed": true,
        "verified": verified,
        "via": via,
        "focus_restored": focus_restored,
        "buffer_changed": after != before.text,
        "buffer_identity_unchanged": after_node == before.node,
        "expectation_newly_matched": expectation_matched,
        "polls": polls,
        "elapsed_ms": started.elapsed().as_millis(),
        "before_bytes": before.text.len(),
        "after_bytes": after.len(),
        "before_sha256": super::clipboard::clipboard_sha256_hex(before.text.as_bytes()),
        "after_sha256": super::clipboard::clipboard_sha256_hex(after.as_bytes()),
    });
    if !verified {
        return Err(CuError::new(
            "terminal_input_unverified",
            "terminal input was dispatched but its independent buffer postcondition was not observed",
        )
        .with_detail(serde_json::json!({
            "receipt": ticket.json(),
            "evidence": evidence,
            "outcome": "unknown",
        })));
    }
    receipts.complete(&ticket, "term-send", window, true, evidence.clone())?;
    Ok(serde_json::json!({
        "addressing": "exact-desktop-window",
        "window_identity": identity.json(),
        "node": before.node,
        "via": via,
        "performed": true,
        "verified": true,
        "focus_restored": focus_restored,
        "buffer_changed": true,
        "receipt": ticket.json(),
    }))
}

pub(super) fn term_wait_payload(
    window: isize,
    pattern: &str,
    timeout_ms: u64,
    interval_ms: u64,
    max_bytes: usize,
    control: ExecutionControl<'_>,
) -> Result<serde_json::Value, CuError> {
    term_wait_with_reader(
        window,
        pattern,
        timeout_ms,
        interval_ms,
        max_bytes,
        control,
        &read_buffer,
    )
}

/// The `term-wait` polling loop, parameterized over the buffer reader.
///
/// The reader is a borrowed function rather than a trait object so the real
/// `read_buffer` remains the only production path; the seam exists so the
/// cancellation cases can be driven against the ACTUAL loop instead of a copy.
fn term_wait_with_reader(
    window: isize,
    pattern: &str,
    timeout_ms: u64,
    interval_ms: u64,
    max_bytes: usize,
    control: ExecutionControl<'_>,
    reader: &dyn Fn(&ExternalWindowIdentity) -> Result<TerminalBuffer, CuError>,
) -> Result<serde_json::Value, CuError> {
    // Validation and identity binding complete BEFORE the token is consulted, so
    // those inherent refusals are never masked. The first buffer read is the first
    // cancellable observation and is therefore guarded inside `term_wait_bound`.
    validate_read_bounds(None, max_bytes)?;
    if pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES {
        return Err(invalid_input(
            "term wait PATTERN must be 1..=4096 UTF-8 bytes".into(),
        ));
    }
    if !(1..=86_400_000).contains(&timeout_ms) || !(10..=10_000).contains(&interval_ms) {
        return Err(invalid_input(
            "term wait requires timeout-ms 1..=86400000 and interval-ms 10..=10000".into(),
        ));
    }
    let expression = Regex::new(pattern).map_err(|_| {
        CuError::new(
            "terminal_pattern_invalid",
            "term wait PATTERN is not a valid bounded Rust regular expression",
        )
        .with_detail(serde_json::json!({ "pattern_bytes": pattern.len() }))
    })?;
    let identity = bind_window(window)?;
    term_wait_bound(
        &identity,
        pattern,
        &expression,
        timeout_ms,
        interval_ms,
        max_bytes,
        control,
        reader,
    )
}

/// The polling loop once the window identity is bound. Split out so the
/// cancellation cases can drive the REAL loop with an injected identity; the only
/// production caller is `term_wait_with_reader` above.
#[allow(clippy::too_many_arguments)]
fn term_wait_bound(
    identity: &ExternalWindowIdentity,
    pattern: &str,
    expression: &Regex,
    timeout_ms: u64,
    interval_ms: u64,
    max_bytes: usize,
    control: ExecutionControl<'_>,
    reader: &dyn Fn(&ExternalWindowIdentity) -> Result<TerminalBuffer, CuError>,
) -> Result<serde_json::Value, CuError> {
    let mut buffer_node: Option<String> = None;
    let started = Instant::now();
    let deadline = started + Duration::from_millis(timeout_ms);
    let mut polls = 0usize;
    loop {
        // PRE-EFFECT CANCEL: no buffer authority is consulted for this round when
        // the invocation has already been cancelled.
        control.check_observe()?;
        polls += 1;
        let buffer = reader(identity)?;
        let expected_node = buffer_node.get_or_insert_with(|| buffer.node.clone());
        if buffer.node != *expected_node {
            return Err(CuError::new(
                "terminal_buffer_identity_changed",
                "the selected terminal buffer changed while the wait was in flight",
            )
            .with_detail(serde_json::json!({
                "window_identity": identity.json(),
                "expected_node_sha256": super::clipboard::clipboard_sha256_hex(expected_node.as_bytes()),
                "observed_node_sha256": super::clipboard::clipboard_sha256_hex(buffer.node.as_bytes()),
            })));
        }
        let last_bytes = buffer.text.len();
        let last_sha256 = super::clipboard::clipboard_sha256_hex(buffer.text.as_bytes());
        if let Some(hit) = expression.find(&buffer.text) {
            let matched = &buffer.text[hit.start()..hit.end()];
            let (matched, match_truncated) = utf8_suffix(matched, max_bytes);
            return Ok(serde_json::json!({
                "addressing": "exact-desktop-window",
                "window_identity": identity.json(),
                "backend": buffer.backend,
                "node": buffer.node,
                "matched": matched,
                "match_truncated_head": match_truncated,
                "index": hit.start(),
                "polls": polls,
                "elapsed_ms": started.elapsed().as_millis(),
            }));
        }
        if Instant::now() >= deadline {
            let complete = source_complete(&buffer.backend);
            return Err(CuError::new(
                if complete { "terminal_wait_timeout" } else { "terminal_wait_inconclusive" },
                if complete {
                    "external terminal buffer did not match before the bounded deadline"
                } else {
                    "the bounded terminal provider exposed no match, but does not prove complete-buffer absence"
                },
            )
            .with_detail(serde_json::json!({
                "window_identity": identity.json(),
                "pattern_sha256": super::clipboard::clipboard_sha256_hex(pattern.as_bytes()),
                "pattern_bytes": pattern.len(),
                "last_buffer_sha256": last_sha256,
                "last_buffer_bytes": last_bytes,
                "polls": polls,
                "elapsed_ms": started.elapsed().as_millis(),
                "source_complete": complete,
                "content_disclosed": false,
            })));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        term_wait_pause(control, remaining.min(Duration::from_millis(interval_ms)))?;
    }
}

/// Slice width for the inter-round pause, matching the shared wait policy. The
/// token is observed before each slice and once at the end.
const TERM_WAIT_CANCEL_SLICE: Duration = Duration::from_millis(10);

fn term_wait_pause(control: ExecutionControl<'_>, pause: Duration) -> Result<(), CuError> {
    let pause_deadline = Instant::now() + pause;
    while Instant::now() < pause_deadline {
        control.check_observe()?;
        thread::sleep(
            TERM_WAIT_CANCEL_SLICE.min(pause_deadline.saturating_duration_since(Instant::now())),
        );
    }
    control.check_observe()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cancel_control(probe: &dyn Fn() -> bool) -> ExecutionControl<'_> {
        ExecutionControl::with_cancel_probe(probe)
    }

    /// A synthetic, already-bound window identity. Binding itself is proven by its
    /// own tests; these cases exercise the polling loop, which needs a bound
    /// identity but no real window.
    fn fixture_identity(handle: isize) -> ExternalWindowIdentity {
        ExternalWindowIdentity {
            handle,
            pid: 4_242,
            start_identity: "fixture-start-identity".into(),
            app: "Fixture Terminal".into(),
        }
    }

    /// Drives the REAL polling loop (`term_wait_bound`) with an injected identity
    /// and reader. The production `term_wait_payload` reaches the same function
    /// after `bind_window`, so these cases cannot pass against a decision the real
    /// loop would not take.
    fn run_bound_wait(
        pattern: &str,
        timeout_ms: u64,
        interval_ms: u64,
        max_bytes: usize,
        control: ExecutionControl<'_>,
        reader: &dyn Fn(&ExternalWindowIdentity) -> Result<TerminalBuffer, CuError>,
    ) -> Result<serde_json::Value, CuError> {
        let expression = Regex::new(pattern).expect("fixture pattern compiles");
        let identity = fixture_identity(7);
        term_wait_bound(
            &identity,
            pattern,
            &expression,
            timeout_ms,
            interval_ms,
            max_bytes,
            control,
            reader,
        )
    }

    /// Validation-only cases go through the production entry point, where they
    /// must fail before `bind_window` and therefore before any reader call.
    fn run_unbound_wait(
        pattern: &str,
        timeout_ms: u64,
        interval_ms: u64,
        control: ExecutionControl<'_>,
        reader: &dyn Fn(&ExternalWindowIdentity) -> Result<TerminalBuffer, CuError>,
    ) -> Result<serde_json::Value, CuError> {
        term_wait_with_reader(0, pattern, timeout_ms, interval_ms, 4096, control, reader)
    }

    #[test]
    fn invalid_request_is_not_masked_by_a_pre_cancel() {
        // VALIDATION BEFORE CANCEL: a malformed request is reported as itself.
        let control = cancel_control(&|| true);
        let reader = |_: &ExternalWindowIdentity| -> Result<TerminalBuffer, CuError> {
            panic!("validation must complete before any authority read")
        };
        let error = run_unbound_wait("fine", 0, 50, control, &reader)
            .expect_err("an invalid timeout must be refused");
        assert_eq!(error.code, "invalid_input");

        // An invalid PATTERN is likewise its own typed error before any read.
        let control = cancel_control(&|| true);
        let error = run_unbound_wait("", 1_000, 50, control, &reader)
            .expect_err("an empty pattern must be refused");
        assert_eq!(error.code, "invalid_input");

        // A valid request with an unbound window reports the bind failure, not the
        // token, and still never reads.
        let control = cancel_control(&|| true);
        let error = run_unbound_wait("fine", 1_000, 50, control, &reader)
            .expect_err("an unbindable window must be refused");
        assert_ne!(error.code, "cancelled");
    }

    #[test]
    fn pre_cancel_after_binding_stops_before_the_first_buffer_read() {
        let reads = std::cell::Cell::new(0usize);
        let reader = |_: &ExternalWindowIdentity| -> Result<TerminalBuffer, CuError> {
            reads.set(reads.get() + 1);
            panic!("a pre-cancelled wait must not read the buffer")
        };
        let error = run_bound_wait("ready", 60_000, 50, 4096, cancel_control(&|| true), &reader)
            .expect_err("a pre-cancelled bound wait must refuse");
        assert_eq!(error.code, "cancelled");
        assert_eq!(reads.get(), 0);
    }

    #[test]
    fn first_buffer_match_wins_over_a_cancel_set_by_that_read() {
        let cancelled = std::cell::Cell::new(false);
        let probe = || cancelled.get();
        let reader = |identity: &ExternalWindowIdentity| -> Result<TerminalBuffer, CuError> {
            cancelled.set(true);
            Ok(TerminalBuffer {
                node: identity.json().to_string(),
                role: "text-area".into(),
                backend: "fixture-complete".into(),
                text: "hello world".into(),
            })
        };
        let value = run_bound_wait("wor", 1_000, 50, 4096, cancel_control(&probe), &reader)
            .expect("a matched first buffer must be returned despite a set token");
        assert_eq!(value.get("matched"), Some(&serde_json::json!("wor")));
        assert_eq!(value.get("polls"), Some(&serde_json::json!(1)));
    }

    #[test]
    fn unmatched_first_buffer_plus_a_token_stops_before_a_second_read() {
        // First buffer does not match; the token is set during the sliced pause, so
        // the loop must exit without a second authority read.
        let reads = std::cell::Cell::new(0usize);
        let cancelled = std::cell::Cell::new(false);
        let probe = || cancelled.get();
        let reader = |identity: &ExternalWindowIdentity| -> Result<TerminalBuffer, CuError> {
            reads.set(reads.get() + 1);
            cancelled.set(true);
            Ok(TerminalBuffer {
                node: identity.json().to_string(),
                role: "text-area".into(),
                backend: "fixture-complete".into(),
                text: "hello world".into(),
            })
        };
        let started = Instant::now();
        let error = run_bound_wait("nope", 60_000, 50, 4096, cancel_control(&probe), &reader)
            .expect_err("an unmatched round plus a token must refuse the verb");
        assert_eq!(error.code, "cancelled");
        assert_eq!(
            reads.get(),
            1,
            "the token must stop the second authority read"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the cancel must not wait for the 60 s deadline"
        );
    }

    #[test]
    fn a_later_round_result_is_authoritative_over_a_late_token() {
        // The FIRST later round's reader flips the token and still returns a
        // buffer: that round's result must win, not the token it just set.
        let reads = std::cell::Cell::new(0usize);
        let cancelled = std::cell::Cell::new(false);
        let probe = || cancelled.get();
        let reader = |identity: &ExternalWindowIdentity| -> Result<TerminalBuffer, CuError> {
            reads.set(reads.get() + 1);
            if reads.get() == 2 {
                cancelled.set(true);
            }
            Ok(TerminalBuffer {
                node: identity.json().to_string(),
                role: "text-area".into(),
                backend: "fixture-complete".into(),
                // The first read misses; every later read matches.
                text: if reads.get() == 1 {
                    "nothing yet".into()
                } else {
                    "now it is ready".into()
                },
            })
        };
        let value = run_bound_wait("ready", 60_000, 50, 4096, cancel_control(&probe), &reader)
            .expect("a matched later round must return its result");
        assert_eq!(value.get("matched"), Some(&serde_json::json!("ready")));
        assert_eq!(value.get("polls"), Some(&serde_json::json!(2)));
    }

    #[test]
    fn a_later_round_reader_error_is_authoritative_over_a_late_token() {
        // A later round's reader failure must surface as itself rather than being
        // replaced by the token the same round set.
        let reads = std::cell::Cell::new(0usize);
        let cancelled = std::cell::Cell::new(false);
        let probe = || cancelled.get();
        let reader = |identity: &ExternalWindowIdentity| -> Result<TerminalBuffer, CuError> {
            reads.set(reads.get() + 1);
            if reads.get() == 1 {
                return Ok(TerminalBuffer {
                    node: identity.json().to_string(),
                    role: "text-area".into(),
                    backend: "fixture-complete".into(),
                    text: "nothing yet".into(),
                });
            }
            cancelled.set(true);
            Err(CuError::new(
                "terminal_reader_fixture_refusal",
                "the fixture reader refused the second round",
            ))
        };
        let error = run_bound_wait("ready", 60_000, 50, 4096, cancel_control(&probe), &reader)
            .expect_err("a reader refusal must surface");
        assert_eq!(error.code, "terminal_reader_fixture_refusal");
    }

    fn candidate(node: &str, role: &str, text: &str) -> TerminalBuffer {
        TerminalBuffer {
            node: node.into(),
            role: role.into(),
            backend: "fixture".into(),
            text: text.into(),
        }
    }

    #[test]
    fn longest_then_deepest_terminal_buffer_wins() {
        let hit = select_candidate(vec![
            candidate("/0", "scroll-area", "short"),
            candidate("/0/1", "text-area", "the longest text"),
            candidate("/0/2", "text-area", "medium"),
        ])
        .unwrap();
        assert_eq!(hit.node, "/0/1");
    }

    #[test]
    fn equally_plausible_buffers_are_ambiguous() {
        let error = select_candidate(vec![
            candidate("/0/1", "text-area", "same"),
            candidate("/0/2", "text-area", "same"),
        ])
        .unwrap_err();
        assert_eq!(error.code, "terminal_buffer_ambiguous");
    }

    #[test]
    fn read_shape_trims_padding_tails_and_utf8_bounds() {
        let identity = ExternalWindowIdentity {
            handle: 7,
            pid: 9,
            start_identity: "fixture-start".into(),
            app: "fixture".into(),
        };
        let json = shape_read(
            &identity,
            candidate("/0/1", "text-area", "one\n二二\n\n"),
            Some(1),
            false,
            3,
        );
        assert_eq!(json["lines"], 2);
        assert_eq!(json["text"], "二");
        assert_eq!(json["truncated_head"], true);
        assert_eq!(json["source_complete"], false);
        assert_eq!(json["tail_scope"], "observed-prefix");
    }

    #[test]
    fn terminal_roles_are_cross_backend_normalized() {
        assert!(terminal_role("AXTextArea"));
        assert!(terminal_role("scroll-area"));
        assert!(terminal_role("Terminal"));
        assert!(!terminal_role("button"));
    }

    #[test]
    fn shallow_terminal_tree_detects_single_frame_without_children() {
        let tree = mechanism::A11yTree {
            backend: "at-spi2".into(),
            window_handle: Some(7),
            root_id: "/0".into(),
            nodes: vec![mechanism::A11yNode {
                id: "/0".into(),
                parent_id: None,
                role: "frame".into(),
                subrole: None,
                name: "fixture".into(),
                states: Vec::new(),
                bounds: mechanism::A11yBounds {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 50,
                },
                actions: Vec::new(),
                text: None,
                identifier: None,
            }],
            truncated: false,
            visited: 1,
            returned: 1,
        };
        assert!(shallow_terminal_tree(&tree));
        let error = terminal_buffer_not_found_error(&tree);
        assert_eq!(error.code, "terminal_buffer_not_found");
        let detail = error.detail.expect("host-limit detail");
        assert_eq!(detail["limit"], "host");
        assert_eq!(detail["group"], "terminal");
        assert_eq!(detail["tree"]["visited"], 1);
    }

    #[test]
    fn public_patterns_retain_the_complete_unicode_grammar() {
        let expression = Regex::new(r"(?i)^\p{Script=Han}+\s+\p{Greek}+$").unwrap();
        assert!(expression.is_match("终端 ΒΗΤΑ"));
        assert!(!expression.is_match("terminal ΒΗΤΑ"));

        let word = Regex::new(r"\b\w+\b").unwrap();
        assert_eq!(word.find("…控制台…").unwrap().as_str(), "控制台");
    }
}
