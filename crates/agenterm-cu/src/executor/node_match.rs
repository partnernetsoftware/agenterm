//! Name / role matching over a tree snapshot: the unique-showing-node rule
//! `wait`, the `--name` verbs and `invoke` all share.

use super::*;

pub(super) fn name_scope(pattern: &str, role: Option<&str>) -> String {
    match role {
        Some(role) => format!("name contains '{pattern}' and role '{role}'"),
        None => format!("name contains '{pattern}'"),
    }
}

pub(super) fn showing_name_matches<'a>(
    nodes: &'a [mechanism::A11yNode],
    pattern: &str,
    role: Option<&str>,
) -> Vec<&'a mechanism::A11yNode> {
    let name_pat = pattern.to_ascii_lowercase();
    let role_pat = role.map(str::to_ascii_lowercase);
    nodes
        .iter()
        .filter(|node| node_matches(node, &name_pat, role_pat.as_deref()))
        .collect()
}

pub(super) fn require_unique_showing_node<'a>(
    nodes: &'a [mechanism::A11yNode],
    pattern: &str,
    role: Option<&str>,
) -> Result<&'a mechanism::A11yNode, CuError> {
    let matches = showing_name_matches(nodes, pattern, role);
    match matches.len() {
        1 => Ok(matches[0]),
        count => Err(name_match_error(pattern, role, count)),
    }
}

pub(super) fn name_match_error(pattern: &str, role: Option<&str>, count: usize) -> CuError {
    if count == 0 {
        return CuError::new(
            "a11y_node_not_found",
            format!(
                "no showing accessibility node with {}",
                name_scope(pattern, role)
            ),
        );
    }
    CuError::new(
        "a11y_node_ambiguous",
        format!(
            "{count} showing accessibility nodes with {}",
            name_scope(pattern, role)
        ),
    )
    .with_count(count)
}

pub(super) fn node_matches(
    node: &mechanism::A11yNode,
    name_pat: &str,
    role_pat: Option<&str>,
) -> bool {
    if !node_is_showing(node) {
        return false;
    }
    if !node.name.to_ascii_lowercase().contains(name_pat) {
        return false;
    }
    match role_pat {
        Some(role) => node.role.to_ascii_lowercase().contains(role),
        None => true,
    }
}

pub(super) fn node_is_showing(node: &mechanism::A11yNode) -> bool {
    node.states
        .iter()
        .any(|state| state.eq_ignore_ascii_case("showing") || state.eq_ignore_ascii_case("visible"))
}

pub(super) fn target_error(error: observe::TargetError) -> CuError {
    match error {
        observe::TargetError::Invalid(message) => CuError::new("invalid_input", message),
        observe::TargetError::Missing(message) => CuError::new("a11y_node_not_found", message),
        observe::TargetError::Ambiguous { count, scope } => CuError::new(
            "ambiguous",
            format!("{count} showing accessibility nodes with {scope}; refusing to guess"),
        )
        .with_count(count)
        .with_detail(serde_json::json!({ "matches": count })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_match_is_case_insensitive_and_requires_showing() {
        let shown = node("Reload this page", "push button", &["showing", "enabled"]);
        assert!(node_matches(&shown, "reload", None));
        assert!(node_matches(&shown, "reload", Some("push button")));
        assert!(!node_matches(&shown, "reload", Some("entry")));
        assert!(!node_matches(&shown, "bookmark", None));

        let hidden = node("Reload this page", "push button", &["enabled"]);
        assert!(!node_matches(&hidden, "reload", None));
    }

    #[test]
    fn find_showing_node_reuses_wait_matcher() {
        let nodes = vec![
            node("hidden Reload", "button", &["enabled"]),
            node("Reload this page", "push button", &["showing", "enabled"]),
        ];
        let matched =
            require_unique_showing_node(&nodes, "reload", Some("button")).expect("shown match");
        assert_eq!(matched.name, "Reload this page");
        let missing = require_unique_showing_node(&nodes, "reload", Some("entry")).unwrap_err();
        assert_eq!(missing.code, "a11y_node_not_found");
        assert_eq!(missing.count, None);
    }

    #[test]
    fn two_showing_nodes_named_alike_are_ambiguous() {
        let nodes = vec![
            node_at("/0/1", "Tab search", "push button", &["showing", "enabled"]),
            node_at("/0/2", "Tab search", "push button", &["visible", "enabled"]),
        ];
        let err = require_unique_showing_node(&nodes, "Tab search", None).unwrap_err();
        assert_eq!(err.code, "a11y_node_ambiguous");
        assert_eq!(err.count, Some(2));
        assert!(
            err.message.contains("2"),
            "ambiguous error must carry the match count: {}",
            err.message
        );

        // A hidden duplicate must not count; only showing/visible nodes do.
        let one_showing = vec![
            node_at("/0/1", "Tab search", "push button", &["showing"]),
            node_at("/0/2", "Tab search", "push button", &["enabled"]),
        ];
        let matched = require_unique_showing_node(&one_showing, "Tab search", None)
            .expect("hidden twin is not a match");
        assert_eq!(matched.id, "/0/1");
    }

    #[test]
    fn name_send_keys_two_showing_matches_are_ambiguous() {
        // `send-keys --name` resolves through this exact matcher, so two
        // showing hits must abort before any chord reaches the display.
        let nodes = vec![
            node_at("/0/1", "Address and search bar", "entry", &["showing"]),
            node_at("/0/2", "Address and search bar", "entry", &["visible"]),
        ];
        let err = require_unique_showing_node(&nodes, "Address and search bar", None).unwrap_err();
        assert_eq!(err.code, "a11y_node_ambiguous");
        assert_eq!(err.count, Some(2));
    }
}
