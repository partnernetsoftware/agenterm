//! `invoke`: one semantic node action with a read-back receipt.

use super::*;

// ---------------------------------------------------------------------------
// invoke / verify / wait --expect (PRD 29 default loop, PRD 31 invariants).
// ---------------------------------------------------------------------------

/// The platform action for an `invoke` verb plus its validated value.
pub(super) fn invoke_action(
    action: InvokeAction,
    value: Option<&str>,
) -> Result<mechanism::NodeAction, CuError> {
    match action.value_kind() {
        InvokeValueKind::None => {
            if value.is_some() {
                return Err(invalid_input(format!(
                    "invoke {} takes no value",
                    action.as_str()
                )));
            }
        }
        InvokeValueKind::Text => {
            if value.is_none() {
                return Err(invalid_input(format!(
                    "invoke {} requires a value",
                    action.as_str()
                )));
            }
        }
        InvokeValueKind::Flag => {
            if !matches!(value, Some("true") | Some("false")) {
                return Err(invalid_input(format!(
                    "invoke {} requires true or false",
                    action.as_str()
                )));
            }
        }
    }
    let flag = value == Some("true");
    let text = value.unwrap_or_default().to_owned();
    Ok(match action {
        InvokeAction::Press => mechanism::NodeAction::Press,
        InvokeAction::SetValue => mechanism::NodeAction::SetValue(text),
        InvokeAction::SelectOption => mechanism::NodeAction::SelectOption(text),
        InvokeAction::SetChecked => mechanism::NodeAction::SetChecked(flag),
        InvokeAction::SetExpanded => mechanism::NodeAction::SetExpanded(flag),
        InvokeAction::Increment => mechanism::NodeAction::Increment,
        InvokeAction::Decrement => mechanism::NodeAction::Decrement,
        InvokeAction::ScrollTo => {
            return Err(CuError::new(
                "invalid_input",
                "internal: invoke scroll-to uses agt_a11y_node_scroll, not NodeAction",
            ));
        }
        InvokeAction::SetSelection => {
            return Err(CuError::new(
                "invalid_input",
                "internal: invoke set-selection uses agt_a11y_node_set_selection, not NodeAction",
            ));
        }
        InvokeAction::SetSelected => mechanism::NodeAction::SetSelected(flag),
        InvokeAction::Cancel => mechanism::NodeAction::Cancel,
        InvokeAction::ShowDefaultUi => mechanism::NodeAction::ShowDefaultUi,
    })
}

/// The normalized action name a node must list before cu even asks the
/// backend (`set-value` / `select-option` are attribute writes the backend
/// alone can judge).
pub(super) fn required_node_action(action: InvokeAction) -> Option<&'static str> {
    match action {
        InvokeAction::Press | InvokeAction::SetChecked | InvokeAction::SetExpanded => Some("click"),
        InvokeAction::Increment => Some("increment"),
        InvokeAction::Decrement => Some("decrement"),
        InvokeAction::SetValue
        | InvokeAction::SelectOption
        | InvokeAction::SetSelected
        | InvokeAction::SetSelection
        | InvokeAction::ScrollTo
        | InvokeAction::Cancel
        | InvokeAction::ShowDefaultUi => None,
    }
}

/// One semantic action with a read-back receipt. Never activates or raises
/// the window: the only mechanism is the a11y node action.
pub(super) fn invoke_payload(
    window: isize,
    mut spec: observe::TargetSpec,
    action: InvokeAction,
    value: Option<&str>,
    selector: Option<&str>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "invoke requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if let Some(selector) = selector {
        if spec.node.is_some()
            || spec.index.is_some()
            || spec.name.is_some()
            || spec.identifier.is_some()
            || spec.focused
        {
            return Err(invalid_input(
                "invoke --selector cannot mix with --node/--index/--name/--identifier/--focused"
                    .into(),
            ));
        }
        let tree = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
        let hit = observe::walk_selector(&tree, selector).map_err(invalid_input)?;
        let Some(hit) = hit else {
            return Err(CuError::new(
                "a11y_node_not_found",
                format!("invoke --selector {selector:?} matched no node"),
            ));
        };
        spec.node = Some(hit.id.clone());
    }
    if action == InvokeAction::ScrollTo && value.is_some() {
        return Err(invalid_input("invoke scroll-to takes no value".into()));
    }
    if action == InvokeAction::SetSelection {
        let raw = value.ok_or_else(|| {
            invalid_input("invoke set-selection requires <start>:<length>".into())
        })?;
        observe::parse_text_selection(raw).map_err(invalid_input)?;
    }
    let node_action = if matches!(action, InvokeAction::ScrollTo | InvokeAction::SetSelection) {
        None
    } else {
        Some(invoke_action(action, value)?)
    };
    // `--focused`: the platform names the application's own focused control
    // first; the tree read that follows must still show the same identity
    // (id, role, identifier) at that path, so PID + window + focused
    // identity are bound in one observation before anything is pressed.
    let focused_identity = if spec.focused {
        if spec.node.is_some()
            || spec.index.is_some()
            || spec.name.is_some()
            || spec.identifier.is_some()
        {
            return Err(invalid_input(
                "--focused addresses the focused control; combine it only with --role".into(),
            ));
        }
        Some(focused_control(window, spec.role.as_deref())?.1)
    } else {
        None
    };
    let before = mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)?;
    let before_inventory_present = mechanism::window_enumerate::enumerate_top_level()
        .map(|rows| rows.iter().any(|row| row.handle == window))
        .unwrap_or(false);
    let target = match &focused_identity {
        Some(focused) => {
            let Some(now) = observe::node_by_id(&before, &focused.id) else {
                return Err(CuError::new(
                    "a11y_node_recycled",
                    format!(
                        "the focused control at {} is not in the window tree any more",
                        focused.id
                    ),
                ));
            };
            if now.role != focused.role || now.identifier != focused.identifier {
                return Err(CuError::new(
                    "a11y_node_recycled",
                    format!(
                        "the focused control at {} changed identity between reads ({} {:?} -> {} {:?})",
                        focused.id, focused.role, focused.identifier, now.role, now.identifier
                    ),
                ));
            }
            now.clone()
        }
        None => {
            let flat = observe::flatten(&before);
            let hit = observe::resolve_target(&flat, &spec).map_err(target_error)?;
            hit.node.clone()
        }
    };
    require_offered_action(&before.backend, &target, action)?;
    let performed = desired_state_precheck(node_action.as_ref(), &target)?;
    // The crash-persistent receipt is reserved here — after every refusal
    // that needs no mechanism, before the mechanism is touched — so a line
    // with no `completed` / `failed` partner means "uncertain", never "did
    // not happen".
    let node_json = serde_json::json!({
        "id": target.id,
        "role": target.role,
        "name": target.name,
        "identifier": target.identifier,
        "index": before.nodes.iter().position(|node| node.id == target.id),
    });
    let ticket = receipts.reserve(
        "invoke",
        window,
        serde_json::json!({
            "spec": spec.json(),
            "node": node_json,
            "action": action.as_str(),
            "value": value,
            "performed": performed,
            "before": observe::node_state_json(&target),
        }),
    )?;
    let mut mechanism_error = None;
    if performed {
        let result = if action == InvokeAction::ScrollTo {
            mechanism::scroll_node(Some(window), &target.id)
        } else if action == InvokeAction::SetSelection {
            let raw = value.unwrap_or("");
            let (start, end) = observe::parse_text_selection(raw).map_err(invalid_input)?;
            mechanism::set_node_selection(Some(window), &target.id, start, end)
        } else {
            mechanism::perform_node_action(
                Some(window),
                &target.id,
                node_action.clone().expect("mapped invoke action"),
            )
        };
        if let Err(error) = result {
            mechanism_error = Some(map_mechanism_err(error));
        }
    }
    let after = match mechanism::tree_for_window(Some(window)) {
        Ok(after) => after,
        Err(error) => {
            let error = map_mechanism_err(error);
            let after_inventory_absent = mechanism::window_enumerate::enumerate_top_level()
                .map(|rows| rows.iter().all(|row| row.handle != window))
                .unwrap_or(false);
            let disappeared = disappearance_is_verified(
                action,
                performed,
                mechanism_error.is_none(),
                before_inventory_present,
                after_inventory_absent,
                &error.code,
            );
            let verification = serde_json::json!({
                "method": "window-disappeared",
                "reason": if disappeared { None::<&str> } else { Some("after_tree_failed") },
                "before_inventory_present": before_inventory_present,
                "after_inventory_absent": after_inventory_absent,
                "after_tree_error": error_payload(&error),
            });
            let receipt = serde_json::json!({
                "addressing": "accessibility-tree",
                "mechanism": "libagenterm",
                "backend": before.backend,
                "window": window,
                "target": spec.json(),
                "node": node_json,
                "action": action.as_str(),
                "value": value,
                "performed": performed,
                "verified": disappeared,
                "verification": verification,
                "before": observe::node_state_json(&target),
                "after": serde_json::Value::Null,
                "tree_changed": disappeared,
                "receipt": ticket.json(),
            });
            receipts.complete(
                &ticket,
                "invoke",
                window,
                disappeared,
                serde_json::json!({
                    "after": serde_json::Value::Null,
                    "verification": verification,
                    "tree_changed": disappeared,
                    "error": if disappeared { None } else { Some(error_payload(&error)) },
                }),
            )?;
            if disappeared {
                return Ok(receipt);
            }
            return Err(error.with_detail(serde_json::json!({ "receipt": receipt })));
        }
    };
    let after_node = observe::node_by_id(&after, &target.id).cloned();
    if action == InvokeAction::SetSelection {
        let verified = mechanism_error.is_none();
        let verification = serde_json::json!({
            "method": "set-selection",
            "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { None::<&str> },
        });
        let after_state = after_node.as_ref().map(observe::node_state_json);
        let receipt = serde_json::json!({
            "addressing": "accessibility-tree",
            "mechanism": "libagenterm",
            "backend": before.backend,
            "window": window,
            "target": spec.json(),
            "node": node_json,
            "action": "set-selection",
            "via": "set-selection",
            "value": value,
            "performed": performed,
            "verified": verified,
            "verification": verification,
            "before": observe::node_state_json(&target),
            "after": after_state,
            "tree_changed": observe::tree_changed(&before, &after),
            "receipt": ticket.json(),
        });
        receipts.complete(
            &ticket,
            "invoke",
            window,
            verified,
            serde_json::json!({
                "after": after_state,
                "verification": verification,
                "error": mechanism_error.as_ref().map(error_payload),
            }),
        )?;
        if let Some(error) = mechanism_error {
            return Err(error.with_detail(serde_json::json!({ "receipt": receipt })));
        }
        return Ok(receipt);
    }
    if action == InvokeAction::ScrollTo {
        let verified = mechanism_error.is_none();
        let verification = serde_json::json!({
            "method": "scroll-to",
            "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { None::<&str> },
        });
        let after_state = after_node.as_ref().map(observe::node_state_json);
        let receipt = serde_json::json!({
            "addressing": "accessibility-tree",
            "mechanism": "libagenterm",
            "backend": before.backend,
            "window": window,
            "target": spec.json(),
            "node": node_json,
            "action": "scroll-to",
            "via": "scroll-to",
            "performed": performed,
            "verified": verified,
            "verification": verification,
            "before": observe::node_state_json(&target),
            "after": after_state,
            "tree_changed": observe::tree_changed(&before, &after),
            "receipt": ticket.json(),
        });
        receipts.complete(
            &ticket,
            "invoke",
            window,
            verified,
            serde_json::json!({
                "after": after_state,
                "verification": verification,
                "tree_changed": observe::tree_changed(&before, &after),
                "error": mechanism_error.as_ref().map(error_payload),
            }),
        )?;
        if let Some(error) = mechanism_error {
            return Err(error.with_detail(serde_json::json!({ "receipt": receipt })));
        }
        return Ok(receipt);
    }
    let node_action = node_action.expect("mapped invoke action");
    let (verified, method, reason) =
        invoke_verification(&node_action, &target, after_node.as_ref(), &before, &after);
    let verified = verified && mechanism_error.is_none();
    let verification = serde_json::json!({
        "method": method,
        "reason": if mechanism_error.is_some() { Some("mechanism_failed") } else { reason },
    });
    let after_state = after_node.as_ref().map(observe::node_state_json);
    let next_actions = if reason == Some("checked_unchanged") || reason == Some("state_mismatch") {
        serde_json::json!([
            "AX did not flip checked; Chromium custom switch is not a native checkbox",
            "re-query then retry, or mcu browser/CDP click on the DOM control",
        ])
    } else {
        serde_json::Value::Null
    };
    let receipt = serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": before.backend,
        "window": window,
        "target": spec.json(),
        "node": node_json,
        "action": action.as_str(),
        "value": value,
        "performed": performed,
        "verified": verified,
        "verification": verification,
        "before": observe::node_state_json(&target),
        "after": after_state,
        "tree_changed": observe::tree_changed(&before, &after),
        "next_actions": next_actions,
        "receipt": ticket.json(),
    });
    receipts.complete(
        &ticket,
        "invoke",
        window,
        verified,
        serde_json::json!({
            "after": after_state,
            "verification": verification,
            "tree_changed": observe::tree_changed(&before, &after),
            "error": mechanism_error.as_ref().map(error_payload),
        }),
    )?;
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(serde_json::json!({ "receipt": receipt })));
    }
    Ok(receipt)
}

fn disappearance_is_verified(
    action: InvokeAction,
    performed: bool,
    mechanism_succeeded: bool,
    before_inventory_present: bool,
    after_inventory_absent: bool,
    after_tree_error: &str,
) -> bool {
    matches!(action, InvokeAction::Press | InvokeAction::Cancel)
        && performed
        && mechanism_succeeded
        && before_inventory_present
        && after_inventory_absent
        && matches!(
            after_tree_error,
            "a11y_window_gone" | "a11y_window_not_addressable"
        )
}

/// Refuse to press what the node does not offer -- but only where an
/// empty action list is a *claim*. The contract says an empty list means
/// the backend reported none and never that it was not asked; the AT-SPI
/// adapter breaks that on purpose, skipping Action during the walk
/// because WebKitGTK hangs `GetActions`. So on that backend every node
/// reports no actions, and this guard refused `invoke press` on a live
/// GTK button -- measured against a real widget tree, where the whole
/// verb was unreachable through name addressing.
///
/// Where the walk does not read action names, the mechanism judges
/// instead: it asks the node itself and fails typed
/// (`a11y_action_unavailable`) if the action is missing. That is one
/// round trip later than this check, and honest, which this check was
/// not.
fn require_offered_action(
    backend: &str,
    target: &mechanism::A11yNode,
    action: InvokeAction,
) -> Result<(), CuError> {
    let backend_publishes_actions = backend != "at-spi2";
    if let Some(required) = required_node_action(action)
        && backend_publishes_actions
        && !target
            .actions
            .iter()
            .any(|offered| offered.eq_ignore_ascii_case(required))
    {
        return Err(CuError::new(
            "unsupported",
            format!(
                "node {} ({} {:?}) does not offer {} (actions: {})",
                target.id,
                target.role,
                target.name,
                action.as_str(),
                if target.actions.is_empty() {
                    "none".to_owned()
                } else {
                    target.actions.join(", ")
                }
            ),
        )
        .with_detail(serde_json::json!({
            "reason": "node_action_missing",
            "required": required,
            "offered": target.actions,
        })));
    }
    Ok(())
}

/// Desired-state verbs: an unobservable state is refused before any
/// action; an already-matching state is a verified no-op (`Ok(false)`:
/// nothing to perform).
fn desired_state_precheck(
    node_action: Option<&mechanism::NodeAction>,
    target: &mechanism::A11yNode,
) -> Result<bool, CuError> {
    let desired = match node_action {
        Some(mechanism::NodeAction::SetChecked(flag)) => {
            Some(("checked", *flag, observe::checked_state(target)))
        }
        Some(mechanism::NodeAction::SetExpanded(flag)) => {
            Some(("expanded", *flag, observe::expanded_state(target)))
        }
        Some(mechanism::NodeAction::SetSelected(flag)) => {
            Some(("selected", *flag, observe::selected_state(target)))
        }
        _ => None,
    };
    let mut performed = true;
    if let Some((field, flag, state)) = desired {
        match state {
            observe::Tri::Unknown => {
                return Err(CuError::new(
                    "unsupported",
                    format!(
                        "node {} ({} {:?}) exposes no {field} state; refusing to press blind",
                        target.id, target.role, target.name
                    ),
                )
                .with_detail(
                    serde_json::json!({ "reason": "state_unobservable", "state": field }),
                ));
            }
            observe::Tri::True | observe::Tri::False if state.as_bool() == Some(flag) => {
                performed = false;
            }
            _ => {}
        }
    }
    Ok(performed)
}

/// `(verified, method, reason)` for one performed node action, judged
/// from the node's read-back state and the whole-window tree diff.
fn invoke_verification(
    node_action: &mechanism::NodeAction,
    target: &mechanism::A11yNode,
    after_node: Option<&mechanism::A11yNode>,
    before: &mechanism::A11yTree,
    after: &mechanism::A11yTree,
) -> (bool, &'static str, Option<&'static str>) {
    match (node_action, after_node) {
        (mechanism::NodeAction::SetValue(wanted), Some(now))
        | (mechanism::NodeAction::SelectOption(wanted), Some(now)) => {
            let hit = now.text.as_deref() == Some(wanted.as_str());
            (
                hit,
                "value-readback",
                if hit { None } else { Some("value_mismatch") },
            )
        }
        (mechanism::NodeAction::SetChecked(wanted), Some(now)) => {
            let hit = observe::checked_state(now).as_bool() == Some(*wanted);
            (
                hit,
                "checked-readback",
                if hit { None } else { Some("state_mismatch") },
            )
        }
        (mechanism::NodeAction::SetSelected(wanted), Some(now)) => {
            let hit = observe::selected_state(now).as_bool() == Some(*wanted);
            (
                hit,
                "selected-readback",
                if hit { None } else { Some("state_mismatch") },
            )
        }
        (mechanism::NodeAction::SetExpanded(wanted), Some(now)) => {
            let hit = observe::expanded_state(now).as_bool() == Some(*wanted);
            (
                hit,
                "expanded-readback",
                if hit { None } else { Some("state_mismatch") },
            )
        }
        (mechanism::NodeAction::Increment, Some(now))
        | (mechanism::NodeAction::Decrement, Some(now)) => {
            match (observe::numeric_text(target), observe::numeric_text(now)) {
                (Some(was), Some(is)) if was != is => (true, "value-readback", None),
                (Some(_), Some(_)) => (false, "value-readback", Some("value_unchanged")),
                _ => (false, "value-readback", Some("value_unreadable")),
            }
        }
        (mechanism::NodeAction::Press, now) => {
            let proof = observe::verify_press(target, now, before, after);
            (proof.verified, proof.method, proof.reason)
        }
        (_, None) => (false, "node-readback", Some("node_gone")),
        _ => (false, "none", Some("unverifiable_action")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoke_scroll_to_is_not_unmapped_spelling() {
        let reply = actuate_executor().execute(&Command::Invoke {
            target: TargetRef::Current,
            window: -1,
            node: None,
            index: None,
            name: Some("agenterm-no-such-node".into()),
            role: None,
            identifier: None,
            focused: false,
            action: InvokeAction::ScrollTo,
            value: None,
            selector: None,
        });
        assert!(!reply.ok);
        assert_eq!(reply.command, "invoke");
        let err = reply.error.as_ref().expect("typed");
        assert_ne!(err.code, "usage");
        if let Some(reason) = err.detail.as_ref().and_then(|d| d["reason"].as_str()) {
            assert_ne!(reason, "node_action_unmapped");
        }
        assert!(!err.message.contains("not mapped on the libagenterm"));
    }

    #[test]
    fn a_successful_press_may_be_verified_by_exact_window_disappearance() {
        assert!(disappearance_is_verified(
            InvokeAction::Press,
            true,
            true,
            true,
            true,
            "a11y_window_not_addressable",
        ));
        assert!(disappearance_is_verified(
            InvokeAction::Cancel,
            true,
            true,
            true,
            true,
            "a11y_window_gone",
        ));
    }

    #[test]
    fn disappearance_never_hides_a_failed_or_unrelated_action() {
        for accepted in [
            disappearance_is_verified(
                InvokeAction::SetValue,
                true,
                true,
                true,
                true,
                "a11y_window_gone",
            ),
            disappearance_is_verified(
                InvokeAction::Press,
                false,
                true,
                true,
                true,
                "a11y_window_gone",
            ),
            disappearance_is_verified(
                InvokeAction::Press,
                true,
                false,
                true,
                true,
                "a11y_window_gone",
            ),
            disappearance_is_verified(
                InvokeAction::Press,
                true,
                true,
                false,
                true,
                "a11y_window_gone",
            ),
            disappearance_is_verified(
                InvokeAction::Press,
                true,
                true,
                true,
                false,
                "a11y_window_gone",
            ),
            disappearance_is_verified(
                InvokeAction::Press,
                true,
                true,
                true,
                true,
                "a11y_backend_failed",
            ),
        ] {
            assert!(!accepted);
        }
    }
}
