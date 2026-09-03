//! Browser tab strip through the accessibility tree (`tab list` /
//! `tab select`).
//!
//! macOS Chromium (Chrome, Brave, Edge) publishes only the *active* tab's
//! `web-area` in the AX tree; every other tab exists there solely as a
//! `radio-button` inside the tab-strip `tab-group` (name = tab title,
//! state `selected` / `unselected`). So AX verbs cannot reach a background
//! tab's content, but they can pick which tab is active: pressing that
//! radio button switches the tab inside the window without raising or
//! activating anything. This module is the pure matcher; the mechanism
//! (tree read, `AXPress`, read-back, receipt) lives in `executor`.

use serde_json::{Value, json};

use crate::mechanism::{A11yNode, A11yTree};
use crate::observe::{Tri, selected_state};

/// One tab-strip entry in strip order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabEntry<'a> {
    /// Position in the strip (0-based, the order the tree lists them).
    pub index: usize,
    pub node: &'a A11yNode,
}

impl TabEntry<'_> {
    pub fn title(&self) -> &str {
        &self.node.name
    }

    pub fn selected(&self) -> Tri {
        selected_state(self.node)
    }

    pub fn json(&self) -> Value {
        json!({
            "index": self.index,
            "id": self.node.id,
            "title": self.node.name,
            "selected": self.selected().json(),
            "role": self.node.role,
        })
    }
}

/// `(container role, item role)` pairs that are a browser tab strip on
/// some backend. macOS AX: `tab-group` / `radio-button` (Chromium); AT-SPI:
/// `page-tab-list` / `page-tab`; UIA: `tab` / `tab-item`. Roles compare
/// case-insensitively and accept the platform spelling (`AXRadioButton`).
const TAB_STRIP_ROLES: &[(&str, &str)] = &[
    ("tab-group", "radio-button"),
    ("page-tab-list", "page-tab"),
    ("tab", "tab-item"),
];

fn role_is(role: &str, wanted: &str) -> bool {
    let lower = role.trim().to_ascii_lowercase();
    let stripped = lower.strip_prefix("ax").unwrap_or(&lower);
    let compact: String = stripped.chars().filter(|ch| *ch != '-').collect();
    stripped == wanted || compact == wanted.replace('-', "")
}

/// The tab-strip entries of one window tree, in walk order: every item
/// node whose direct parent is a strip container. Nodes outside such a
/// container (a form's radio buttons, a settings page's tabs drawn in the
/// web area) are not tabs.
pub fn tab_strip_entries(tree: &A11yTree) -> Vec<TabEntry<'_>> {
    let mut entries = Vec::new();
    for node in &tree.nodes {
        let Some(parent_id) = node.parent_id.as_deref() else {
            continue;
        };
        let Some(parent) = tree
            .nodes
            .iter()
            .find(|candidate| candidate.id == parent_id)
        else {
            continue;
        };
        let is_tab = TAB_STRIP_ROLES
            .iter()
            .any(|(container, item)| role_is(&parent.role, container) && role_is(&node.role, item));
        if is_tab {
            entries.push(TabEntry {
                index: entries.len(),
                node,
            });
        }
    }
    entries
}

/// How `tab select` names its tab: exactly one of the two.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TabSpec {
    /// Case-insensitive substring of the tab title.
    Title(String),
    /// Position in the strip (0-based, as `tab list` numbers it).
    Index(usize),
}

impl TabSpec {
    pub fn from_parts(title: Option<&str>, index: Option<usize>) -> Result<Self, String> {
        match (title, index) {
            (Some(_), Some(_)) => Err("tab select takes --title SUB or --index N, not both".into()),
            (None, None) => Err("tab select requires --title SUB or --index N".into()),
            (Some(title), None) => {
                if title.trim().is_empty() {
                    Err("tab select --title must not be empty".into())
                } else {
                    Ok(Self::Title(title.to_owned()))
                }
            }
            (None, Some(index)) => Ok(Self::Index(index)),
        }
    }

    pub fn json(&self) -> Value {
        match self {
            Self::Title(title) => json!({ "title": title }),
            Self::Index(index) => json!({ "index": index }),
        }
    }

    fn scope(&self) -> String {
        match self {
            Self::Title(title) => format!("--title {title:?}"),
            Self::Index(index) => format!("--index {index}"),
        }
    }
}

/// Typed outcome of a tab lookup: the executor maps these to
/// `a11y_tab_not_found` / `a11y_tab_ambiguous`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TabMatchError {
    /// `reason` is `no_tab_strip` when the tree has no strip at all,
    /// otherwise `no_match`.
    NotFound {
        reason: &'static str,
        message: String,
    },
    Ambiguous {
        count: usize,
        message: String,
    },
}

/// Exactly one strip entry for `spec`. No strip in the tree, or no entry
/// matching, is `NotFound`; two or more title hits are `Ambiguous` (the
/// caller lists the candidates); an index past the end is `NotFound`.
pub fn match_tab<'a>(
    entries: &'a [TabEntry<'a>],
    spec: &TabSpec,
) -> Result<&'a TabEntry<'a>, TabMatchError> {
    if entries.is_empty() {
        return Err(TabMatchError::NotFound {
            reason: "no_tab_strip",
            message: "the window tree has no tab strip (no tab-group / radio-button rows)".into(),
        });
    }
    match spec {
        TabSpec::Index(index) => entries.get(*index).ok_or_else(|| TabMatchError::NotFound {
            reason: "no_match",
            message: format!(
                "tab --index {index} is out of range; the strip has {} tab(s)",
                entries.len()
            ),
        }),
        TabSpec::Title(title) => {
            let wanted = title.to_lowercase();
            let hits: Vec<&TabEntry<'a>> = entries
                .iter()
                .filter(|entry| entry.title().to_lowercase().contains(&wanted))
                .collect();
            match hits.as_slice() {
                [] => Err(TabMatchError::NotFound {
                    reason: "no_match",
                    message: format!(
                        "no tab title contains {}; {} tab(s) in the strip",
                        spec.scope(),
                        entries.len()
                    ),
                }),
                [one] => Ok(one),
                many => Err(TabMatchError::Ambiguous {
                    count: many.len(),
                    message: format!(
                        "{} tab titles contain {}; refusing to guess",
                        many.len(),
                        spec.scope()
                    ),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mechanism::A11yBounds;

    fn node(id: &str, parent: Option<&str>, role: &str, name: &str, states: &[&str]) -> A11yNode {
        A11yNode {
            id: id.into(),
            parent_id: parent.map(str::to_owned),
            role: role.into(),
            name: name.into(),
            states: states.iter().map(|s| (*s).to_owned()).collect(),
            bounds: A11yBounds {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            actions: vec!["click".into()],
            text: None,
            identifier: None,
        }
    }

    fn fake_tree(nodes: Vec<A11yNode>) -> A11yTree {
        let returned = nodes.len();
        A11yTree {
            backend: "ax".into(),
            window_handle: Some(7),
            root_id: "/0".into(),
            nodes,
            truncated: false,
            visited: returned,
            returned,
        }
    }

    fn chromium_tree() -> A11yTree {
        fake_tree(vec![
            node("/0", None, "window", "Codex", &["showing"]),
            node("/0/1", Some("/0"), "group", "", &["showing"]),
            node("/0/1/0", Some("/0/1"), "tab-group", "", &["showing"]),
            node(
                "/0/1/0/0",
                Some("/0/1/0"),
                "radio-button",
                "Inbox - Mail",
                &["showing", "unselected"],
            ),
            node(
                "/0/1/0/1",
                Some("/0/1/0"),
                "radio-button",
                "Codex",
                &["showing", "selected"],
            ),
            node(
                "/0/1/0/2",
                Some("/0/1/0"),
                "AXRadioButton",
                "Spec - Docs",
                &["showing", "unselected"],
            ),
            node(
                "/0/1/0/3",
                Some("/0/1/0"),
                "radio-button",
                "Notes - Docs",
                &["showing", "unselected"],
            ),
            // A web form's radio button is not a tab.
            node("/0/2", Some("/0"), "web-area", "Codex", &["showing"]),
            node(
                "/0/2/0",
                Some("/0/2"),
                "radio-button",
                "Docs option",
                &["showing", "unselected"],
            ),
        ])
    }

    #[test]
    fn strip_entries_are_radio_buttons_under_a_tab_group_only() {
        let tree = chromium_tree();
        let entries = tab_strip_entries(&tree);
        let titles: Vec<&str> = entries.iter().map(TabEntry::title).collect();
        assert_eq!(
            titles,
            ["Inbox - Mail", "Codex", "Spec - Docs", "Notes - Docs"]
        );
        assert_eq!(entries[2].index, 2);
        assert_eq!(entries[1].selected(), Tri::True);
        assert_eq!(entries[0].selected(), Tri::False);
        let row = entries[1].json();
        assert_eq!(row["index"], 1);
        assert_eq!(row["id"], "/0/1/0/1");
        assert_eq!(row["title"], "Codex");
        assert_eq!(row["selected"], true);
        // AT-SPI and UIA spellings are strips too.
        let atspi = fake_tree(vec![
            node("/0", None, "frame", "", &["showing"]),
            node("/0/0", Some("/0"), "page-tab-list", "", &["showing"]),
            node(
                "/0/0/0",
                Some("/0/0"),
                "page-tab",
                "One",
                &["showing", "selected"],
            ),
        ]);
        assert_eq!(tab_strip_entries(&atspi).len(), 1);
        let uia = fake_tree(vec![
            node("/0", None, "window", "", &["showing"]),
            node("/0/0", Some("/0"), "Tab", "", &["showing"]),
            node(
                "/0/0/0",
                Some("/0/0"),
                "TabItem",
                "One",
                &["showing", "selected"],
            ),
        ]);
        assert_eq!(tab_strip_entries(&uia).len(), 1);
    }

    #[test]
    fn title_match_is_case_insensitive_and_unique() {
        let tree = chromium_tree();
        let entries = tab_strip_entries(&tree);
        let hit = match_tab(&entries, &TabSpec::Title("inbox".into())).expect("one inbox");
        assert_eq!(hit.index, 0);
        let hit = match_tab(&entries, &TabSpec::Title("NOTES - docs".into())).expect("one notes");
        assert_eq!(hit.node.id, "/0/1/0/3");
        match match_tab(&entries, &TabSpec::Title("docs".into())) {
            Err(TabMatchError::Ambiguous { count, message }) => {
                assert_eq!(count, 2);
                assert!(message.contains("refusing to guess"), "{message}");
            }
            other => panic!("two docs tabs must be ambiguous: {other:?}"),
        }
        match match_tab(&entries, &TabSpec::Title("nowhere".into())) {
            Err(TabMatchError::NotFound { reason, .. }) => assert_eq!(reason, "no_match"),
            other => panic!("miss must be not-found: {other:?}"),
        }
        // The web-area radio button never counts, so "option" is a miss.
        assert!(match_tab(&entries, &TabSpec::Title("option".into())).is_err());
    }

    #[test]
    fn index_match_is_strip_order_and_bounded() {
        let tree = chromium_tree();
        let entries = tab_strip_entries(&tree);
        assert_eq!(
            match_tab(&entries, &TabSpec::Index(2))
                .expect("third")
                .title(),
            "Spec - Docs"
        );
        match match_tab(&entries, &TabSpec::Index(4)) {
            Err(TabMatchError::NotFound { reason, message }) => {
                assert_eq!(reason, "no_match");
                assert!(message.contains("out of range"), "{message}");
            }
            other => panic!("index past the end must be not-found: {other:?}"),
        }
    }

    #[test]
    fn a_window_without_a_strip_is_not_found_with_its_own_reason() {
        let plain = fake_tree(vec![
            node("/0", None, "window", "TextEdit", &["showing"]),
            node(
                "/0/0",
                Some("/0"),
                "radio-button",
                "Plain",
                &["showing", "selected"],
            ),
        ]);
        let entries = tab_strip_entries(&plain);
        assert!(entries.is_empty());
        match match_tab(&entries, &TabSpec::Index(0)) {
            Err(TabMatchError::NotFound { reason, .. }) => assert_eq!(reason, "no_tab_strip"),
            other => panic!("no strip must be typed: {other:?}"),
        }
    }

    #[test]
    fn spec_is_exactly_one_of_title_or_index() {
        assert_eq!(
            TabSpec::from_parts(Some("Codex"), None),
            Ok(TabSpec::Title("Codex".into()))
        );
        assert_eq!(TabSpec::from_parts(None, Some(3)), Ok(TabSpec::Index(3)));
        assert!(TabSpec::from_parts(Some("a"), Some(1)).is_err());
        assert!(TabSpec::from_parts(None, None).is_err());
        assert!(TabSpec::from_parts(Some("  "), None).is_err());
        assert_eq!(TabSpec::Index(3).json(), json!({ "index": 3 }));
    }
}
