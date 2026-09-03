//! Text and key writers: `send-text`, `copy`, `paste`, `send-keys`.
//! `--name` addresses one showing node; `--window` alone writes the
//! showing focused node (guarded against browser chrome); neither keeps
//! the plain focused inject.

use super::*;

/// `send-text` with `--name` writes through native AT-SPI
/// `EditableText` (`SetTextContents` / `InsertText`) or, when the named
/// showing node exposes `Text` + `editable` but not `EditableText`
/// (Chrome 151, WebKitGTK/Reasonix `<textarea>`), through AT-SPI `Text`
/// plus the toolkit set-value. Success is confirmed by `Text.GetText`.
/// The WebKit eval helper's `OK` and `last_text_write_via` are write-path
/// reports; `wait --text-equals` must poll GetText again. A named showing
/// node with no writeable text interface typed-fails
/// (`a11y_text_unavailable`) and never falls through to XTest /
/// `input_inject::type_text`.
///
/// `--window` without `--name` writes that same path on the showing
/// focused node — the same innermost `Text.GetText` candidate
/// `get-text --window` reads — so `focus --name X` then
/// `send-text --window H TEXT` then `get-text --window H` closes the
/// loop on agenterm-con `Command` (native `EditableText`), Chrome
/// `GetTextField`, and the Reasonix composer (`Message Reasonix…`
/// under `scripts/reasonix-desktop-a11y.sh`). WebKit 2.52 still has no
/// `EditableText`; the write is AT-SPI `Text` plus the eval-helper
/// set-value (`id=composer-input`). Never XTest when `--window` is set.
/// Without `--window` it stays the plain "type into whatever is
/// focused" inject.
pub(super) fn send_text(
    text: &str,
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
    allow_browser_chrome: bool,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if let Some(resolved) = resolve_actuation_node(window, None, name, role, "send-text")? {
        return send_text_to_node(text, window, resolved);
    }
    if role.filter(|value| !value.is_empty()).is_some() {
        return Err(CuError::new(
            "invalid_input",
            "send-text --role requires --name <pattern>",
        ));
    }
    if let Some(handle) = window {
        let target = focused_write_target(handle)?;
        return focused_text_write(
            target,
            "send-text",
            allow_browser_chrome,
            receipts,
            |node| send_text_to_node(text, window, node),
        );
    }
    mechanism::input_inject::type_text(text).map_err(map_mechanism_err)?;
    Ok(serde_json::json!({ "typed": text }))
}

pub(super) fn send_text_to_node(
    text: &str,
    window: Option<isize>,
    resolved: ResolvedNode,
) -> Result<serde_json::Value, CuError> {
    mechanism::set_node_text(window, &resolved.node_id, text).map_err(map_mechanism_err)?;
    let _ = mechanism::accessibility_tree::drain_bus();
    let via = mechanism::accessibility_tree::last_text_write_via().unwrap_or_default();
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "send-text",
        "typed": text,
        "via": via,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `copy --name` reads AT-SPI `Text.GetText` (`agt_a11y_node_get_text`)
/// from the unique showing named node and publishes that UTF-8 onto the
/// native clipboard (`agt_clipboard_set_text`). On Linux X11 the owner
/// process stays in the `SetSelectionOwner` event loop so a later
/// `paste --name` (no `--text`) can `ConvertSelection`. A named showing
/// node with no Text interface typed-fails (`a11y_text_unavailable`) and
/// never falls through to XTest / `--coords` / screenshot.
///
/// `--window` without `--name` copies that same GetText path on the
/// showing focused node — the same innermost `Text.GetText` candidate
/// `get-text --window` reads — so `focus --name X` then
/// `copy --window H` then `paste --window H` / `get-text --window H`
/// closes the loop on agenterm-con `Command` (`via=gettext` on a second
/// con that never steals the resident control socket), Chrome
/// `GetTextField`, and the Reasonix composer (`Message Reasonix…` under
/// `scripts/reasonix-desktop-a11y.sh`, `via=gettext`). Never XTest when
/// `--window` is set. Without `--window` copy is invalid: there is no
/// plain "copy whatever is focused" inject verb. `matched.text` is the
/// resolve-time snapshot; the copied payload is independent GetText.
/// Live close-the-circuit: seed unique string → focused copy → clear →
/// focused paste → independent GetText equals seed. Paste after copy on
/// Reasonix still uses the WebKit eval-helper set-value path; con paste
/// restore is native `EditableText` (`via=editable-text`); only
/// independent GetText proves the restore.
pub(super) fn copy(
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let resolved = if let Some(resolved) = resolve_actuation_node(window, None, name, role, "copy")?
    {
        resolved
    } else if role.filter(|value| !value.is_empty()).is_some() {
        return Err(CuError::new(
            "invalid_input",
            "copy --role requires --name <pattern>",
        ));
    } else if window.is_some() {
        let (resolved, _current) = get_text_focused(window)?;
        resolved
    } else {
        return Err(CuError::new(
            "invalid_input",
            "copy requires --window <handle> [--name <pattern>]",
        ));
    };
    let text = mechanism::get_node_text(window, &resolved.node_id).map_err(map_mechanism_err)?;
    mechanism::clipboard::publish_text(&text).map_err(map_mechanism_err)?;
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "copy",
        "text": text,
        "via": "gettext",
        "clipboard": true,
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `paste --name` writes clipboard text into the unique showing named
/// field through the same native AT-SPI `EditableText` / `Text` path as
/// named `send-text`. `--text` only seeds the clipboard; the field write
/// always reads `agt_clipboard_get_text` first. A named showing node with
/// no writeable text interface typed-fails (`a11y_text_unavailable`) and
/// never falls through to XTest / `--coords` / screenshot.
///
/// `--window` without `--name` writes that same clipboard path on the
/// showing focused node — the same innermost `Text.GetText` candidate
/// `get-text --window` reads — so `focus --name X` then
/// `paste --window H` (optional `--text` seed) then
/// `get-text --window H` closes the loop on agenterm-con `Command`
/// (native `EditableText`, `via=editable-text` on a second con that
/// never steals the resident control socket), Chrome `GetTextField`,
/// and the Reasonix composer (`Message Reasonix…` under
/// `scripts/reasonix-desktop-a11y.sh`, eval-helper set-value,
/// `via=text`). Never XTest when `--window` is set. Without `--window`
/// paste is invalid: there is no plain "paste into whatever is focused"
/// inject verb. A miss or an ambiguous name writes nothing.
/// `matched.text` is the resolve-time snapshot; independent
/// `get-text --window` / `wait --text-equals` must poll `Text.GetText`.
pub(super) fn paste(
    seed: Option<&str>,
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
    allow_browser_chrome: bool,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if let Some(resolved) = resolve_actuation_node(window, None, name, role, "paste")? {
        return paste_to_node(seed, window, resolved);
    }
    if role.filter(|value| !value.is_empty()).is_some() {
        return Err(CuError::new(
            "invalid_input",
            "paste --role requires --name <pattern>",
        ));
    }
    let Some(handle) = window else {
        return Err(CuError::new(
            "invalid_input",
            "paste requires --window <handle> [--name <pattern>]",
        ));
    };
    let target = focused_write_target(handle)?;
    focused_text_write(target, "paste", allow_browser_chrome, receipts, |node| {
        paste_to_node(seed, window, node)
    })
}

/// Seed the clipboard when asked, then write its text into `resolved`.
fn paste_to_node(
    seed: Option<&str>,
    window: Option<isize>,
    resolved: ResolvedNode,
) -> Result<serde_json::Value, CuError> {
    if let Some(seed) = seed {
        mechanism::clipboard::set_text(seed).map_err(map_mechanism_err)?;
    }
    let pasted = mechanism::clipboard::get_text().map_err(map_mechanism_err)?;
    mechanism::set_node_text(window, &resolved.node_id, &pasted).map_err(map_mechanism_err)?;
    let _ = mechanism::accessibility_tree::drain_bus();
    let via = mechanism::accessibility_tree::last_text_write_via().unwrap_or_default();
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "paste",
        "typed": pasted,
        "via": via,
        "clipboard": true,
        "seeded": seed.is_some(),
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// `send-keys` with `--name` delivers the chord through native AT-SPI
/// Device/key events (`DeviceEventListener.NotifyEvent`). A named showing
/// node with no key interface typed-fails (`a11y_key_unavailable`) and
/// never falls through to XTest / `input_inject::send_keys`.
///
/// `--window` without `--name` targets the showing focused node — the
/// same innermost `Text.GetText` candidate `get-text --window` reads —
/// so `focus --name X` then `send-keys --window H KEYS` then
/// `get-text --window H` closes the loop on agenterm-con `Command`
/// (native `EditableText`, `via=editable-text` on a second con that
/// never steals the resident control socket), Chrome `GetTextField`,
/// and the Reasonix composer (`Message Reasonix…` under
/// `scripts/reasonix-desktop-a11y.sh`). Prefer
/// `DeviceEventListener.NotifyEvent`. When that interface is absent
/// (con Command; Chrome renderer entry; WebKitGTK textarea) and `KEYS`
/// is plain typeable text, write through the same AT-SPI
/// `EditableText` / `Text` path as focused `send-text` so the typed
/// string is still native AT-SPI and never XTest. Special chords
/// (`enter`, `ctrl+a`, …) without a key interface still typed-fail.
/// Without `--window` it stays the plain "send to whatever is focused"
/// inject.
pub(super) fn send_keys(
    keys: &str,
    window: Option<isize>,
    name: Option<&str>,
    role: Option<&str>,
    allow_browser_chrome: bool,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if let Some(resolved) = resolve_actuation_node(window, None, name, role, "send-keys")? {
        return send_keys_to_node(keys, window, resolved);
    }
    if role.filter(|value| !value.is_empty()).is_some() {
        return Err(CuError::new(
            "invalid_input",
            "send-keys --role requires --name <pattern>",
        ));
    }
    if let Some(handle) = window {
        let target = focused_write_target(handle)?;
        return focused_text_write(
            target,
            "send-keys",
            allow_browser_chrome,
            receipts,
            |node| send_keys_to_focused_node(keys, window, node),
        );
    }
    mechanism::input_inject::send_keys(keys).map_err(map_mechanism_err)?;
    Ok(serde_json::json!({ "keys": keys }))
}

/// What the local backend actually did to deliver a chord. Linux and
/// Windows put the keys on the wire (AT-SPI `DeviceEventController`, UIA
/// focus + `SendInput`); macOS cannot hand a keystroke to an application it
/// refuses to activate, so it performs the AX action the chord *means*
/// (`AXConfirm` / `AXCancel`) and must not claim a key was delivered. This
/// names the mechanism of the local target only -- a remote worker sends
/// its own label back in its reply.
pub(super) fn local_key_delivery_via() -> &'static str {
    if cfg!(target_os = "macos") {
        "ax-action"
    } else {
        "device-event"
    }
}

pub(super) fn send_keys_to_node(
    keys: &str,
    window: Option<isize>,
    resolved: ResolvedNode,
) -> Result<serde_json::Value, CuError> {
    mechanism::send_node_keys(window, &resolved.node_id, keys).map_err(map_mechanism_err)?;
    let _ = mechanism::accessibility_tree::drain_bus();
    let mut payload = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "node": resolved.node_id,
        "window": window,
        "action": "send-keys",
        "keys": keys,
        "via": local_key_delivery_via(),
    });
    attach_name_match(&mut payload, &resolved);
    Ok(payload)
}

/// Focused-node key delivery: Device/key first; plain typeable text may
/// fall back to AT-SPI EditableText/Text write when the node has no
/// `DeviceEventListener` (con Command, Chrome GetTextField, Reasonix
/// composer). Never XTest.
pub(super) fn send_keys_to_focused_node(
    keys: &str,
    window: Option<isize>,
    resolved: ResolvedNode,
) -> Result<serde_json::Value, CuError> {
    match mechanism::send_node_keys(window, &resolved.node_id, keys) {
        Ok(()) => {
            let _ = mechanism::accessibility_tree::drain_bus();
            let mut payload = serde_json::json!({
                "addressing": "accessibility-tree",
                "mechanism": "libagenterm",
                "node": resolved.node_id,
                "window": window,
                "action": "send-keys",
                "keys": keys,
                "via": local_key_delivery_via(),
            });
            attach_name_match(&mut payload, &resolved);
            Ok(payload)
        }
        Err(error) if focused_keys_may_use_text_write(keys, &error) => {
            mechanism::set_node_text(window, &resolved.node_id, keys).map_err(map_mechanism_err)?;
            let _ = mechanism::accessibility_tree::drain_bus();
            let via = mechanism::accessibility_tree::last_text_write_via().unwrap_or_default();
            let mut payload = serde_json::json!({
                "addressing": "accessibility-tree",
                "mechanism": "libagenterm",
                "node": resolved.node_id,
                "window": window,
                "action": "send-keys",
                "keys": keys,
                "via": via,
            });
            attach_name_match(&mut payload, &resolved);
            Ok(payload)
        }
        Err(error) => Err(map_mechanism_err(error)),
    }
}

/// Plain typeable text (no modifier chords / named special keys) may use
/// the AT-SPI Text write path when Device/key is missing or the chord
/// parser rejects a multi-character literal. `enter` / `ctrl+a` stay on
/// the Device/key typed-fail contract.
pub(super) fn focused_keys_may_use_text_write(
    keys: &str,
    error: &mechanism::MechanismError,
) -> bool {
    if !is_plain_typeable_text(keys) {
        return false;
    }
    match error {
        mechanism::MechanismError::Failed { code, .. } => {
            code == "a11y_key_unavailable" || code == "invalid_input"
        }
        mechanism::MechanismError::Unsupported { .. } => false,
    }
}

/// Printable text payload for focused send-keys Text fallback: no `+`
/// modifier chords, not a single named special key token (`enter`,
/// `tab`, …). Multi-character literals and single printable letters
/// qualify so Chrome can close `send-keys --window` → `get-text --window`
/// without DeviceEventListener or XTest.
pub(super) fn is_plain_typeable_text(keys: &str) -> bool {
    if keys.is_empty() || keys.contains('+') {
        return false;
    }
    if is_named_special_key(keys) {
        return false;
    }
    keys.chars().all(|ch| !ch.is_control())
}

pub(super) fn is_named_special_key(keys: &str) -> bool {
    matches!(
        keys.to_ascii_lowercase().as_str(),
        "backspace"
            | "tab"
            | "enter"
            | "return"
            | "escape"
            | "esc"
            | "space"
            | "home"
            | "left"
            | "up"
            | "right"
            | "down"
            | "delete"
            | "del"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
    )
}

// ---------------------------------------------------------------------------
// The focused (`--window`, no `--name`) write path shared by the three
// writers: resolve the showing focused node in one tree read, reserve the
// receipt, apply the browser-chrome guard, write, complete the receipt.
// ---------------------------------------------------------------------------

/// The showing focused node of one window, with the tree it was resolved
/// in, so the guard judges the same snapshot the node came from.
pub(super) struct FocusedWriteTarget {
    pub(super) window: isize,
    pub(super) tree: mechanism::A11yTree,
    pub(super) resolved: ResolvedNode,
}

fn focused_write_target(window: isize) -> Result<FocusedWriteTarget, CuError> {
    let (tree, resolved, _current) = get_text_focused_in_tree(Some(window))?;
    Ok(FocusedWriteTarget {
        window,
        tree,
        resolved,
    })
}

/// One focused write under a receipt. The receipt is reserved after the
/// node is known and before anything is judged or touched, so the
/// browser-chrome refusal leaves a `failed` line (`performed: false`,
/// reason `focused_node_is_browser_chrome`) exactly like a mechanism
/// refusal would, and a write that went through leaves `completed`.
/// `allow_browser_chrome` lets chrome through and marks the reply
/// `browser_chrome: "allowed"`.
pub(super) fn focused_text_write(
    target: FocusedWriteTarget,
    verb: &str,
    allow_browser_chrome: bool,
    receipts: &mut ReceiptLog,
    write: impl FnOnce(ResolvedNode) -> Result<serde_json::Value, CuError>,
) -> Result<serde_json::Value, CuError> {
    let FocusedWriteTarget {
        window,
        tree,
        resolved,
    } = target;
    // The focused resolver always carries the node it picked; a target
    // without one is a programming error, not a mechanism state.
    let Some(node) = resolved.matched.clone() else {
        return Err(CuError::new(
            "invalid_input",
            format!("internal: {verb} focused target carries no resolved node"),
        ));
    };
    let site = classify_focused_site(&tree, &node.id, || window_app_name(Some(window)));
    let ticket = receipts.reserve(
        verb,
        window,
        serde_json::json!({
            "action": verb,
            "node": { "id": node.id, "role": node.role, "name": node.name },
            "addressing": "focused-node",
            "site": focused_site_label(site),
            "allow_browser_chrome": allow_browser_chrome,
            "before": observe::node_state_json(&node),
        }),
    )?;
    if site == FocusedNodeSite::BrowserChrome && !allow_browser_chrome {
        let error = browser_chrome_refusal(verb, &node);
        receipts.complete(
            &ticket,
            verb,
            window,
            false,
            serde_json::json!({
                "performed": false,
                "verification": { "method": "none", "reason": BROWSER_CHROME_CODE },
                "error": error_payload(&error),
            }),
        )?;
        return Err(with_receipt(error, ticket.json()));
    }
    match write(resolved) {
        Ok(mut payload) => {
            let via = payload.get("via").cloned();
            receipts.complete(
                &ticket,
                verb,
                window,
                true,
                serde_json::json!({
                    "performed": true,
                    "verification": { "method": "mechanism-status", "reason": null },
                    "via": via,
                }),
            )?;
            if site == FocusedNodeSite::BrowserChrome {
                payload["browser_chrome"] = serde_json::json!("allowed");
            }
            payload["receipt"] = ticket.json();
            Ok(payload)
        }
        Err(error) => {
            receipts.complete(
                &ticket,
                verb,
                window,
                false,
                serde_json::json!({
                    "performed": false,
                    "verification": { "method": "none", "reason": "mechanism_failed" },
                    "error": error_payload(&error),
                }),
            )?;
            Err(with_receipt(error, ticket.json()))
        }
    }
}

fn focused_site_label(site: FocusedNodeSite) -> &'static str {
    match site {
        FocusedNodeSite::Page => "page",
        FocusedNodeSite::BrowserChrome => "browser-chrome",
        FocusedNodeSite::NotBrowser => "not-browser",
    }
}

/// Attach the receipt identity to an error without dropping the detail it
/// already carries.
fn with_receipt(mut error: CuError, receipt: serde_json::Value) -> CuError {
    let mut detail = error.detail.take().unwrap_or_else(|| serde_json::json!({}));
    match detail.as_object_mut() {
        Some(object) => {
            object.insert("receipt".into(), receipt);
        }
        None => detail = serde_json::json!({ "cause": detail, "receipt": receipt }),
    }
    error.with_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actuation_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: None,
            name: None,
            role: None,
            allow_browser_chrome: false,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn name_send_text_missing_node_is_typed_and_types_nothing() {
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
            allow_browser_chrome: false,
        };
        let attempts_before = mechanism::write_ledger::attempts();
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not type into the wrong place");
        assert_eq!(
            mechanism::write_ledger::attempts(),
            attempts_before,
            "a failed --name match must hand nothing to the mechanism"
        );
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn name_and_role_send_text_miss_performs_no_write_on_any_path() {
        // The live report: `send-text --window H --name 代码 --role AXTextField
        // -- CODE` answered a11y_node_not_found. Whatever the window holds,
        // a miss must reach neither the node writer, the focused-node
        // writer, nor input injection. send-keys and paste share the gate.
        let attempts_before = mechanism::write_ledger::attempts();
        for command in [
            Command::SendText {
                target: TargetRef::Current,
                text: "HARMLESS".into(),
                window: Some(-1),
                name: Some("agenterm-no-such-field".into()),
                role: Some("AXTextField".into()),
                allow_browser_chrome: false,
            },
            Command::SendKeys {
                target: TargetRef::Current,
                keys: "enter".into(),
                window: Some(-1),
                name: Some("agenterm-no-such-field".into()),
                role: None,
                allow_browser_chrome: false,
            },
            Command::Paste {
                target: TargetRef::Current,
                window: Some(-1),
                name: Some("agenterm-no-such-field".into()),
                role: None,
                allow_browser_chrome: false,
                text: Some("HARMLESS".into()),
            },
        ] {
            let reply = actuate_executor().execute(&command);
            assert!(!reply.ok, "{} must fail on a missing name", command.verb());
            let code = reply.error.as_ref().unwrap().code.as_str();
            assert_ne!(code, "usage", "{}", command.verb());
            assert_eq!(
                mechanism::write_ledger::attempts(),
                attempts_before,
                "{} with a missing --name handed a write to the mechanism",
                command.verb()
            );
        }
    }

    #[test]
    fn name_send_text_requires_window() {
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: None,
            name: Some("Address and search bar".into()),
            role: None,
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn send_text_role_without_name_is_typed() {
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: Some(1),
            name: None,
            role: Some("entry".into()),
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("--role requires --name"),
            "role-without-name message should name the addressing contract"
        );
    }

    #[test]
    fn send_text_window_without_name_does_not_xtest() {
        // A synthetic window must take the focused AT-SPI path, not
        // input_inject::type_text. Success here would mean XTest spray.
        let command = Command::SendText {
            target: TargetRef::Current,
            text: "hello".into(),
            window: Some(-1),
            name: None,
            role: None,
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(
            !reply.ok,
            "send-text --window without --name must not fall through to XTest"
        );
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_text_unavailable"
                    | "a11y_backend_failed"
                    | "unsupported"
                    | "failed"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn focused_copy_without_live_focus_fails_typed() {
        // --window without --name is focused copy, not a missing-name usage
        // error. Without a real tree/focus it typed-fails on the a11y path.
        let command = Command::Copy {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_text_unavailable"
                    | "a11y_backend_failed"
                    | "dylib_load"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn copy_role_requires_name() {
        let command = Command::Copy {
            target: TargetRef::Current,
            window: Some(1),
            name: None,
            role: Some("entry".into()),
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_copy_requires_window() {
        let command = Command::Copy {
            target: TargetRef::Current,
            window: None,
            name: Some("FixtureSource".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_copy_missing_node_is_typed_and_copies_nothing() {
        let command = Command::Copy {
            target: TargetRef::Current,
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not seed the clipboard");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn copy_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::Copy {
            target: TargetRef::Current,
            window: Some(1),
            name: Some("FixtureSource".into()),
            role: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn focused_paste_without_live_focus_fails_typed() {
        // --window without --name is focused paste, not a missing-name usage
        // error. Without a real tree/focus it typed-fails on the a11y path.
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: Some(1),
            name: None,
            role: None,
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_text_unavailable"
                    | "a11y_backend_failed"
                    | "dylib_load"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn paste_role_requires_name() {
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: Some(1),
            name: None,
            role: Some("entry".into()),
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_paste_requires_window() {
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: None,
            name: Some("FixtureField".into()),
            role: None,
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn name_paste_missing_node_is_typed_and_writes_nothing() {
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(
            !reply.ok,
            "missing name must not paste into the wrong place"
        );
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn paste_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::Paste {
            target: TargetRef::Current,
            text: Some("hello".into()),
            window: Some(1),
            name: Some("FixtureField".into()),
            role: None,
            allow_browser_chrome: false,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn name_send_keys_requires_window() {
        let command = Command::SendKeys {
            target: TargetRef::Current,
            keys: "enter".into(),
            window: None,
            name: Some("Address and search bar".into()),
            role: None,
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn send_keys_role_without_name_is_typed() {
        let command = Command::SendKeys {
            target: TargetRef::Current,
            keys: "k".into(),
            window: Some(1),
            name: None,
            role: Some("entry".into()),
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("--role requires --name"),
            "role-without-name message should name the addressing contract"
        );
    }

    #[test]
    fn send_keys_window_without_name_does_not_xtest() {
        // A synthetic window must take the focused AT-SPI path, not
        // input_inject::send_keys. Success here would mean XTest spray.
        let command = Command::SendKeys {
            target: TargetRef::Current,
            keys: "314GATE".into(),
            window: Some(-1),
            name: None,
            role: None,
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(
            !reply.ok,
            "send-keys --window without --name must not fall through to XTest"
        );
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_key_unavailable"
                    | "a11y_text_unavailable"
                    | "a11y_backend_failed"
                    | "dylib_load"
                    | "unsupported"
                    | "failed"
                    | "invalid_input"
            ),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn plain_typeable_text_accepts_gate_literal() {
        assert!(is_plain_typeable_text("314GATE123456"));
        assert!(is_plain_typeable_text("k"));
        assert!(!is_plain_typeable_text("enter"));
        assert!(!is_plain_typeable_text("ctrl+a"));
        assert!(!is_plain_typeable_text(""));
    }

    #[test]
    fn name_send_keys_missing_node_is_typed_and_sends_nothing() {
        let command = Command::SendKeys {
            target: TargetRef::Current,
            keys: "enter".into(),
            window: Some(-1),
            name: Some("agenterm-no-such-node".into()),
            role: None,
            allow_browser_chrome: false,
        };
        let reply = actuate_executor().execute(&command);
        assert!(!reply.ok, "missing name must not send keys somewhere else");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(
                code,
                "a11y_node_not_found"
                    | "a11y_tree_empty"
                    | "a11y_window_gone"
                    | "a11y_backend_failed"
                    | "unsupported"
            ),
            "unexpected code: {code}"
        );
    }

    use std::cell::Cell;

    fn tree_of(nodes: Vec<mechanism::A11yNode>) -> mechanism::A11yTree {
        mechanism::A11yTree {
            backend: "ax".into(),
            window_handle: Some(7),
            root_id: "/0".into(),
            nodes,
            truncated: false,
            visited: 0,
            returned: 0,
        }
    }

    fn omnibox() -> mechanism::A11yNode {
        node_at(
            "/0/1",
            "Address and search bar",
            "AXTextField",
            &["showing", "focused"],
        )
    }

    fn web_area() -> mechanism::A11yNode {
        node_at("/0/2", "Sign in", "AXWebArea", &["showing"])
    }

    fn page_field() -> mechanism::A11yNode {
        node_at("/0/2/0/3", "Code", "AXTextField", &["showing", "focused"])
    }

    fn target(tree: mechanism::A11yTree, focused: mechanism::A11yNode) -> FocusedWriteTarget {
        FocusedWriteTarget {
            window: 7,
            resolved: ResolvedNode {
                node_id: focused.id.clone(),
                matched: Some(focused),
                backend: Some(tree.backend.clone()),
            },
            tree,
        }
    }

    fn scratch_receipts(label: &str) -> (ReceiptLog, PathBuf) {
        let audit = audit_scratch(label);
        let dir = audit.parent().expect("scratch root").to_path_buf();
        (
            ReceiptLog::open_in(&dir, TargetRef::Current).expect("receipt log"),
            audit,
        )
    }

    fn receipt_lines(receipts: &ReceiptLog) -> Vec<serde_json::Value> {
        std::fs::read_to_string(receipts.path())
            .expect("receipt file")
            .lines()
            .map(|line| serde_json::from_str(line).expect("receipt line"))
            .collect()
    }

    #[test]
    fn focused_write_into_browser_chrome_is_refused_and_receipted() {
        let (mut receipts, scratch) = scratch_receipts("chrome-refused");
        let attempts_before = mechanism::write_ledger::attempts();
        let wrote = Cell::new(false);
        let error = focused_text_write(
            target(
                tree_of(vec![omnibox(), web_area(), page_field()]),
                omnibox(),
            ),
            "send-text",
            false,
            &mut receipts,
            |_| {
                wrote.set(true);
                Ok(serde_json::json!({ "typed": "CODE" }))
            },
        )
        .expect_err("the omnibox is browser chrome");
        assert!(!wrote.get(), "a refusal must not reach the writer");
        assert_eq!(mechanism::write_ledger::attempts(), attempts_before);
        assert_eq!(error.code, "focused_node_is_browser_chrome");
        let detail = error.detail.as_ref().expect("detail");
        assert_eq!(detail["node"], "/0/1");
        assert_eq!(detail["role"], "AXTextField");
        assert_eq!(detail["name"], "Address and search bar");
        assert_eq!(detail["hint"], BROWSER_CHROME_HINT);
        assert!(detail["receipt"]["id"].is_string(), "{detail}");
        let lines = receipt_lines(&receipts);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0]["phase"], "reserved");
        assert_eq!(lines[0]["verb"], "send-text");
        assert_eq!(lines[0]["window"], 7);
        assert_eq!(lines[0]["site"], "browser-chrome");
        assert_eq!(lines[0]["allow_browser_chrome"], false);
        assert_eq!(lines[0]["node"]["id"], "/0/1");
        assert_eq!(lines[1]["phase"], "failed");
        assert_eq!(lines[1]["receipt_id"], lines[0]["receipt_id"]);
        assert_eq!(lines[1]["verified"], false);
        assert_eq!(lines[1]["performed"], false);
        assert_eq!(
            lines[1]["verification"]["reason"],
            "focused_node_is_browser_chrome"
        );
        assert_eq!(lines[1]["error"]["code"], "focused_node_is_browser_chrome");
        remove_audit_scratch(&scratch);
    }

    #[test]
    fn allow_browser_chrome_writes_the_omnibox_and_says_so() {
        let (mut receipts, scratch) = scratch_receipts("chrome-allowed");
        let payload = focused_text_write(
            target(tree_of(vec![omnibox(), web_area()]), omnibox()),
            "paste",
            true,
            &mut receipts,
            |node| {
                assert_eq!(node.node_id, "/0/1");
                Ok(serde_json::json!({ "typed": "https://example.test", "via": "editable-text" }))
            },
        )
        .expect("deliberate chrome write");
        assert_eq!(payload["browser_chrome"], "allowed");
        assert!(payload["receipt"]["id"].is_string());
        let lines = receipt_lines(&receipts);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["allow_browser_chrome"], true);
        assert_eq!(lines[1]["phase"], "completed");
        assert_eq!(lines[1]["verb"], "paste");
        assert_eq!(lines[1]["performed"], true);
        assert_eq!(lines[1]["via"], "editable-text");
        remove_audit_scratch(&scratch);
    }

    #[test]
    fn focused_write_inside_the_page_passes_unmarked() {
        let (mut receipts, scratch) = scratch_receipts("page-field");
        let payload = focused_text_write(
            target(
                tree_of(vec![omnibox(), web_area(), page_field()]),
                page_field(),
            ),
            "send-keys",
            false,
            &mut receipts,
            |node| {
                assert_eq!(node.node_id, "/0/2/0/3");
                Ok(serde_json::json!({ "keys": "CODE", "via": "device-event" }))
            },
        )
        .expect("a page control is the intended target");
        assert!(payload.get("browser_chrome").is_none());
        let lines = receipt_lines(&receipts);
        assert_eq!(lines[0]["site"], "page");
        assert_eq!(lines[1]["phase"], "completed");
        remove_audit_scratch(&scratch);
    }

    #[test]
    fn focused_write_mechanism_failure_is_receipted_as_failed() {
        let (mut receipts, scratch) = scratch_receipts("mechanism-failed");
        let error = focused_text_write(
            target(tree_of(vec![web_area(), page_field()]), page_field()),
            "send-text",
            false,
            &mut receipts,
            |_| Err(CuError::new("a11y_text_unavailable", "no writeable Text")),
        )
        .expect_err("the writer's failure is the reply");
        assert_eq!(error.code, "a11y_text_unavailable");
        assert!(error.detail.as_ref().unwrap()["receipt"]["id"].is_string());
        let lines = receipt_lines(&receipts);
        assert_eq!(lines[1]["phase"], "failed");
        assert_eq!(lines[1]["performed"], false);
        assert_eq!(lines[1]["verification"]["reason"], "mechanism_failed");
        assert_eq!(lines[1]["error"]["code"], "a11y_text_unavailable");
        remove_audit_scratch(&scratch);
    }
}
