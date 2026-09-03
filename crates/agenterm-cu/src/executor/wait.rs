//! `wait`: window-inventory conditions, `--node-name-contains`,
//! `--text-equals` / `--text-contains` (independent `Text.GetText`), and
//! `--expect` (the `verify` matcher polled).

use super::*;

pub(super) fn wait(
    timeout_ms: u64,
    condition: &WaitCondition,
) -> Result<serde_json::Value, CuError> {
    match condition {
        WaitCondition::Expect { window, expect } => {
            return wait_expect(timeout_ms, *window, expect);
        }
        WaitCondition::NodeNameContains {
            pattern,
            role,
            window,
        } => return wait_node(timeout_ms, pattern, role.as_deref(), *window),
        WaitCondition::NodeTextEquals {
            expected,
            name,
            role,
            window,
        } => {
            return wait_node_text(
                timeout_ms,
                expected,
                name,
                role.as_deref(),
                *window,
                NodeTextMatch::Equals,
            );
        }
        WaitCondition::NodeTextContains {
            substring,
            name,
            role,
            window,
        } => {
            return wait_node_text(
                timeout_ms,
                substring,
                name,
                role.as_deref(),
                *window,
                NodeTextMatch::Contains,
            );
        }
        _ => {}
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let poll = Duration::from_millis(50);
    let mut last_observation = serde_json::json!({ "windows": [] });

    while Instant::now() < deadline {
        let windows =
            mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
        last_observation = serde_json::json!({ "window_count": windows.len(), "windows": windows });
        if condition_met(condition, &windows) {
            return Ok(serde_json::json!({
                "met": true,
                "observation": last_observation,
            }));
        }
        thread::sleep(poll);
    }

    Ok(serde_json::json!({
        "met": false,
        "timeout_ms": timeout_ms,
        "observation": last_observation,
    }))
}

pub(super) fn condition_met(condition: &WaitCondition, windows: &[WindowInfo]) -> bool {
    match condition {
        WaitCondition::WindowCountGte { count } => windows.len() >= *count,
        WaitCondition::WindowTitleContains { pattern } => {
            let pat = pattern.to_ascii_lowercase();
            windows
                .iter()
                .any(|window| window.title.to_ascii_lowercase().contains(&pat))
        }
        WaitCondition::FocusedHandle { handle } => windows
            .iter()
            .any(|window| window.focused && window.handle == *handle),
        // Polled against the accessibility tree, not the window list.
        WaitCondition::Expect { .. }
        | WaitCondition::NodeNameContains { .. }
        | WaitCondition::NodeTextEquals { .. }
        | WaitCondition::NodeTextContains { .. } => false,
    }
}

/// Polls `tree` until exactly one showing node whose name contains `pattern`
/// (and whose role matches `role`, when given) appears. Two or more showing
/// hits fail typed (`a11y_node_ambiguous`) instead of taking the first.
/// Timeout is a typed failure so loop-until callers break on `ok:false`
/// instead of retrying blind.
pub(super) fn wait_node(
    timeout_ms: u64,
    pattern: &str,
    role: Option<&str>,
    window: Option<isize>,
) -> Result<serde_json::Value, CuError> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let poll = Duration::from_millis(50);
    let mut polls = 0usize;
    let mut last_node_count = 0usize;
    let mut last_error: Option<CuError> = None;

    loop {
        polls += 1;
        match mechanism::tree_for_window(window) {
            Ok(tree) => {
                last_node_count = tree.nodes.len();
                last_error.take();
                let matches = showing_name_matches(&tree.nodes, pattern, role);
                match matches.len() {
                    0 => {}
                    1 => {
                        return Ok(serde_json::json!({
                            "met": true,
                            "addressing": "accessibility-tree",
                            "mechanism": "libagenterm",
                            "backend": tree.backend,
                            "window": window,
                            "polls": polls,
                            "node": matches[0],
                            "observation": { "node_count": last_node_count },
                        }));
                    }
                    count => return Err(name_match_error(pattern, role, count)),
                }
            }
            // The tree can be missing outright; that is not something more
            // polling will fix.
            Err(mechanism::MechanismError::Unsupported { .. }) => {
                return Err(map_mechanism_err(mechanism::MechanismError::Unsupported {
                    reason: "accessibility-tree mechanism unavailable".to_owned(),
                }));
            }
            // A scoped window may not have an AT-SPI root yet — keep polling and
            // report the last failure if we run out of time.
            Err(error) => last_error = Some(map_mechanism_err(error)),
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(poll);
    }

    let detail = match last_error {
        Some(error) => format!("last tree read failed: {} ({})", error.message, error.code),
        None => format!("last tree read had {last_node_count} nodes"),
    };
    Err(CuError::new(
        "timeout",
        format!(
            "no showing accessibility node with {} after {timeout_ms}ms ({polls} polls, {detail})",
            name_scope(pattern, role)
        ),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NodeTextMatch {
    Equals,
    Contains,
}

impl NodeTextMatch {
    fn flag(self) -> &'static str {
        match self {
            Self::Equals => "--text-equals",
            Self::Contains => "--text-contains",
        }
    }

    fn matches(self, text: &str, expected: &str) -> bool {
        match self {
            Self::Equals => text == expected,
            Self::Contains => text.contains(expected),
        }
    }

    fn timeout_verb(self) -> &'static str {
        match self {
            Self::Equals => "did not reach text",
            Self::Contains => "did not contain",
        }
    }
}

/// Polls AT-SPI `Text.GetText` (`agt_a11y_node_get_text`) on the unique
/// showing node addressed by `name` until that independent text equals
/// `expected` (`--text-equals`) or contains it (`--text-contains`). The
/// tree snapshot `node.text`, a prior `send-text` / `paste` / `copy`
/// `matched.text`, `last_text_write_via`, and the WebKit eval helper's
/// queued-job `OK` (Reasonix composer) are not this predicate. Timeout
/// is typed so loop-until callers break on `ok:false`.
pub(super) fn wait_node_text(
    timeout_ms: u64,
    expected: &str,
    name: &str,
    role: Option<&str>,
    window: Option<isize>,
    match_kind: NodeTextMatch,
) -> Result<serde_json::Value, CuError> {
    if window.is_none() {
        return Err(CuError::new(
            "invalid_input",
            format!("wait {} requires --window <handle>", match_kind.flag()),
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let poll = Duration::from_millis(50);
    let mut polls = 0usize;
    let mut last_node_count = 0usize;
    let mut last_text: Option<String> = None;
    let mut last_error: Option<CuError> = None;

    loop {
        polls += 1;
        match mechanism::tree_for_window(window) {
            Ok(tree) => {
                last_node_count = tree.nodes.len();
                last_error.take();
                let matches = showing_name_matches(&tree.nodes, name, role);
                match matches.len() {
                    0 => {}
                    1 => match mechanism::get_node_text(window, &matches[0].id) {
                        Ok(text) => {
                            last_text = Some(text.clone());
                            if match_kind.matches(&text, expected) {
                                return Ok(text_equals_success(
                                    &tree.backend,
                                    window,
                                    polls,
                                    matches[0],
                                    &text,
                                    last_node_count,
                                ));
                            }
                        }
                        Err(error @ mechanism::MechanismError::Unsupported { .. }) => {
                            return Err(map_mechanism_err(error));
                        }
                        Err(error) => last_error = Some(map_mechanism_err(error)),
                    },
                    count => return Err(name_match_error(name, role, count)),
                }
            }
            Err(error @ mechanism::MechanismError::Unsupported { .. }) => {
                return Err(map_mechanism_err(error));
            }
            Err(error) => last_error = Some(map_mechanism_err(error)),
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(poll);
    }

    Err(CuError::new(
        "timeout",
        format!(
            "accessibility node with {} {} {expected:?} after {timeout_ms}ms ({polls} polls, {})",
            name_scope(name, role),
            match_kind.timeout_verb(),
            text_equals_timeout_detail(last_text.as_deref(), last_error.as_ref(), last_node_count,)
        ),
    ))
}

/// Success payload for `--text-equals` / `--text-contains`. `gettext` is
/// the only text authority: snapshot `node.text` is overwritten so a
/// sidecar tree walk or `send-text` / `paste` `matched.text` cannot be
/// mistaken for the hit. Published `text` is the full independent GetText.
pub(super) fn text_equals_success(
    backend: &str,
    window: Option<isize>,
    polls: usize,
    node: &mechanism::A11yNode,
    gettext: &str,
    node_count: usize,
) -> serde_json::Value {
    let mut node = node.clone();
    node.text = Some(gettext.to_owned());
    serde_json::json!({
        "met": true,
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": backend,
        "window": window,
        "polls": polls,
        "node": node,
        "text": gettext,
        "via": "gettext",
        "observation": {
            "node_count": node_count,
            "text": gettext,
        },
    })
}

pub(super) fn text_equals_timeout_detail(
    last_text: Option<&str>,
    last_error: Option<&CuError>,
    last_node_count: usize,
) -> String {
    match (last_text, last_error) {
        (Some(text), _) => format!("last GetText={text:?}"),
        (None, Some(error)) => {
            format!("last GetText failed: {} ({})", error.message, error.code)
        }
        (None, None) => format!("last tree read had {last_node_count} nodes"),
    }
}

/// Poll the same matcher until every expectation is met. A missing node
/// keeps polling; ambiguity and an unobservable state fail closed at once.
pub(super) fn wait_expect(
    timeout_ms: u64,
    window: isize,
    expect: &[crate::command::Expectation],
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "wait --expect requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if expect.is_empty() {
        return Err(invalid_input(
            "wait requires a non-empty --expect array".into(),
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let poll = Duration::from_millis(50);
    let mut polls = 0usize;
    let mut last;
    loop {
        polls += 1;
        match mechanism::tree_for_window(Some(window)) {
            Ok(tree) => {
                let flat = observe::flatten(&tree);
                let mut results = Vec::with_capacity(expect.len());
                let mut all_met = true;
                for expectation in expect {
                    let verdict = check_one(&flat, expectation)?;
                    if verdict.unknown {
                        return Err(CuError::new(
                            "unsupported",
                            "an expected state is not observable on its node; more polling cannot make it so",
                        )
                        .with_detail(serde_json::json!({ "reason": "state_unobservable", "item": verdict.item })));
                    }
                    all_met &= verdict.met;
                    results.push(verdict.item);
                }
                last = serde_json::json!({
                    "backend": tree.backend,
                    "visited": tree.visited,
                    "truncated": tree.truncated,
                    "results": results,
                });
                if all_met {
                    return Ok(serde_json::json!({
                        "met": true,
                        "verified": true,
                        "addressing": "accessibility-tree",
                        "mechanism": "libagenterm",
                        "window": window,
                        "polls": polls,
                        "observation": last,
                    }));
                }
            }
            Err(mechanism::MechanismError::Unsupported { reason }) => {
                return Err(map_mechanism_err(mechanism::MechanismError::Unsupported {
                    reason,
                }));
            }
            Err(error) => {
                let error = map_mechanism_err(error);
                if error.code == "denied" {
                    return Err(error);
                }
                last = serde_json::json!({ "tree_error": error_payload(&error) });
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(poll);
    }
    Err(CuError::new(
        "timeout",
        format!("expectations not met after {timeout_ms}ms ({polls} polls)"),
    )
    .with_detail(serde_json::json!({ "observation": last })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_wait_timeout_is_a_typed_failure() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeNameContains {
                pattern: "agenterm-no-such-node".into(),
                role: None,
                window: Some(-1),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "timeout must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(code, "timeout" | "unsupported"),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn node_text_equals_timeout_is_a_typed_failure() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextEquals {
                expected: "agenterm-no-such-text".into(),
                name: "agenterm-no-such-node".into(),
                role: None,
                window: Some(-1),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "timeout must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(code, "timeout" | "unsupported"),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn text_equals_success_publishes_gettext_not_snapshot_text() {
        let mut snapshot = node("Message Reasonix…", "text", &["showing", "editable"]);
        snapshot.text = Some("stale-snapshot".into());
        snapshot.id = "/0/0/0/0/0/0/0/0/8/1/0".into();
        let payload =
            text_equals_success("at-spi2", Some(4194318), 2, &snapshot, "RXWAIT-TYPED", 130);
        assert_eq!(payload["via"], "gettext");
        assert_eq!(payload["text"], "RXWAIT-TYPED");
        assert_eq!(payload["observation"]["text"], "RXWAIT-TYPED");
        assert_eq!(payload["node"]["text"], "RXWAIT-TYPED");
        assert_ne!(payload["via"], "text");
        assert_ne!(payload["node"]["text"], "stale-snapshot");
    }

    #[test]
    fn text_equals_timeout_reports_last_gettext() {
        assert_eq!(
            text_equals_timeout_detail(Some("RXWAIT-TYPED"), None, 130),
            "last GetText=\"RXWAIT-TYPED\""
        );
        let failed = CuError::new("a11y_text_unavailable", "no Text.GetText");
        assert_eq!(
            text_equals_timeout_detail(None, Some(&failed), 130),
            "last GetText failed: no Text.GetText (a11y_text_unavailable)"
        );
    }

    #[test]
    fn node_text_equals_requires_window() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextEquals {
                expected: "x".into(),
                name: "FixtureField".into(),
                role: None,
                window: None,
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn node_text_contains_timeout_is_a_typed_failure() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextContains {
                substring: "agenterm-no-such-sub".into(),
                name: "agenterm-no-such-node".into(),
                role: None,
                window: Some(-1),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "timeout must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(code, "timeout" | "unsupported"),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn node_text_contains_requires_window() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextContains {
                substring: "GATE".into(),
                name: "FixtureField".into(),
                role: None,
                window: None,
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("--text-contains"),
            "missing-window message should name the flag"
        );
    }

    #[test]
    fn text_contains_matches_substring_of_independent_gettext() {
        assert!(NodeTextMatch::Contains.matches("34aGATEXXXX", "GATE"));
        assert!(!NodeTextMatch::Contains.matches("34aGATEXXXX", "NOPE"));
        assert!(!NodeTextMatch::Equals.matches("34aGATEXXXX", "GATE"));
        assert!(NodeTextMatch::Equals.matches("34aGATEXXXX", "34aGATEXXXX"));
    }

    #[test]
    fn text_contains_success_publishes_full_gettext_not_substring() {
        let mut snapshot = node("FixtureField", "entry", &["showing", "editable"]);
        snapshot.text = Some("stale-snapshot".into());
        let payload =
            text_equals_success("at-spi2", Some(4194318), 2, &snapshot, "34aGATEXXXX", 12);
        assert_eq!(payload["via"], "gettext");
        assert_eq!(payload["text"], "34aGATEXXXX");
        assert!(payload["text"].as_str().unwrap().contains("GATE"));
        assert_ne!(payload["text"], "GATE");
        assert_ne!(payload["node"]["text"], "stale-snapshot");
    }
}
