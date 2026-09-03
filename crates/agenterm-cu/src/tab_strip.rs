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
/// some backend. macOS AX: `tab-group` / `radio-button` (Chromium); AT-SPI
/// and UIA: `page tab list` / `page tab` (both adapters spell them with
/// spaces). Roles compare on their alphanumeric core, so every separator
/// spelling and the platform prefix (`AXRadioButton`) match.
const TAB_STRIP_ROLES: &[(&str, &str)] = &[
    ("tab-group", "radio-button"),
    ("page-tab-list", "page-tab"),
    ("tab", "tab-item"),
];

fn role_is(role: &str, wanted: &str) -> bool {
    // The adapters do not agree on separators: macOS AX gives
    // `AXTabGroup`, AT-SPI and UIA both give `page tab list` with
    // spaces. Compare on the alphanumeric core, which is what
    // `observe::normalize_role` already does everywhere else.
    crate::observe::normalize_role(role) == crate::observe::normalize_role(wanted)
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

/// How `tab close` names its tab: exactly one of an exact title or a
/// strip index (0-based, the order `tab list` numbers), the index being
/// the way to name one of two same-title tabs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TabCloseSpec {
    Title(String),
    Index(usize),
}

impl TabCloseSpec {
    pub fn from_parts(title: Option<&str>, index: Option<usize>) -> Result<Self, String> {
        match (title, index) {
            (Some(_), Some(_)) => {
                Err("tab close takes --title T --exact or --index N, not both".into())
            }
            (None, None) => Err("tab close requires --title T --exact or --index N".into()),
            (Some(title), None) => {
                if title.trim().is_empty() {
                    Err("tab close --title must not be empty".into())
                } else {
                    Ok(Self::Title(title.to_owned()))
                }
            }
            (None, Some(index)) => Ok(Self::Index(index)),
        }
    }

    pub fn json(&self) -> Value {
        match self {
            Self::Title(title) => json!({ "title": title, "exact": true }),
            Self::Index(index) => json!({ "index": index }),
        }
    }
}

/// Exactly one strip entry for a `tab close` spec: case-sensitive title
/// equality (two equal titles are `Ambiguous`, with the index as the way
/// out) or the strip index.
pub fn match_tab_exact<'a>(
    entries: &'a [TabEntry<'a>],
    spec: &TabCloseSpec,
) -> Result<&'a TabEntry<'a>, TabMatchError> {
    if entries.is_empty() {
        return Err(TabMatchError::NotFound {
            reason: "no_tab_strip",
            message: "the window tree has no tab strip (no tab-group / radio-button rows)".into(),
        });
    }
    match spec {
        TabCloseSpec::Index(index) => entries.get(*index).ok_or_else(|| TabMatchError::NotFound {
            reason: "no_match",
            message: format!(
                "tab --index {index} is out of range; the strip has {} tab(s)",
                entries.len()
            ),
        }),
        TabCloseSpec::Title(title) => {
            let hits: Vec<&TabEntry<'a>> = entries
                .iter()
                .filter(|entry| entry.title() == title)
                .collect();
            match hits.as_slice() {
                [] => Err(TabMatchError::NotFound {
                    reason: "no_match",
                    message: format!(
                        "no tab title equals {title:?}; {} tab(s) in the strip",
                        entries.len()
                    ),
                }),
                [one] => Ok(one),
                many => Err(TabMatchError::Ambiguous {
                    count: many.len(),
                    message: format!(
                        "{} tab titles equal {title:?}; refusing to guess -- name one with --index N (from tab list)",
                        many.len()
                    ),
                }),
            }
        }
    }
}

/// Where the tab that sat at `previous` is after the tab at `closed` is
/// gone: unchanged before it, one to the left after it, nowhere when it
/// was the closed tab itself.
pub fn index_after_close(previous: usize, closed: usize) -> Option<usize> {
    match previous.cmp(&closed) {
        std::cmp::Ordering::Less => Some(previous),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(previous - 1),
    }
}

/// One strip row detached from its tree (`tab list` shape), so strips of
/// two reads -- or two windows -- can be compared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabRow {
    pub index: usize,
    pub title: String,
    pub selected: Tri,
}

impl TabRow {
    pub fn from_entry(entry: &TabEntry<'_>) -> Self {
        Self {
            index: entry.index,
            title: entry.title().to_owned(),
            selected: entry.selected(),
        }
    }

    pub fn from_tree(tree: &A11yTree) -> Vec<Self> {
        tab_strip_entries(tree)
            .iter()
            .map(Self::from_entry)
            .collect()
    }
}

/// The tab `browser open --url` added: the single selected row of `after`
/// when the strip grew by one row over `before`, or when that row's
/// title is not in `before` at all. `None` when nothing new can be told
/// apart (no single selected row, same size and a title already present).
pub fn new_tab_from_strips<'a>(before: &[TabRow], after: &'a [TabRow]) -> Option<&'a TabRow> {
    let selected: Vec<&TabRow> = after
        .iter()
        .filter(|row| row.selected == Tri::True)
        .collect();
    let [row] = selected.as_slice() else {
        return None;
    };
    if after.len() == before.len() + 1 || !before.iter().any(|old| old.title == row.title) {
        Some(row)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mechanism::A11yBounds;

    #[test]
    fn close_spec_is_exactly_one_of_exact_title_or_index() {
        assert_eq!(
            TabCloseSpec::from_parts(Some("Codex"), None),
            Ok(TabCloseSpec::Title("Codex".into()))
        );
        assert_eq!(
            TabCloseSpec::from_parts(None, Some(2)),
            Ok(TabCloseSpec::Index(2))
        );
        assert!(TabCloseSpec::from_parts(Some("a"), Some(1)).is_err());
        assert!(TabCloseSpec::from_parts(None, None).is_err());
        assert!(TabCloseSpec::from_parts(Some(" "), None).is_err());
        assert_eq!(
            TabCloseSpec::Title("Codex".into()).json(),
            json!({ "title": "Codex", "exact": true })
        );
        assert_eq!(TabCloseSpec::Index(2).json(), json!({ "index": 2 }));
    }

    #[test]
    fn exact_close_match_is_case_sensitive_and_indexes_duplicates() {
        let tree = fake_tree(vec![
            node("/0", None, "window", "Codex", &["showing"]),
            node("/0/1", Some("/0"), "tab-group", "", &["showing"]),
            node(
                "/0/1/0",
                Some("/0/1"),
                "radio-button",
                "Codex",
                &["showing", "selected"],
            ),
            node(
                "/0/1/1",
                Some("/0/1"),
                "radio-button",
                "Notes",
                &["showing", "unselected"],
            ),
            node(
                "/0/1/2",
                Some("/0/1"),
                "radio-button",
                "Codex",
                &["showing", "unselected"],
            ),
        ]);
        let entries = tab_strip_entries(&tree);
        assert_eq!(
            match_tab_exact(&entries, &TabCloseSpec::Title("Notes".into()))
                .expect("one")
                .index,
            1
        );
        // A substring or a case difference is not equality.
        assert!(matches!(
            match_tab_exact(&entries, &TabCloseSpec::Title("notes".into())),
            Err(TabMatchError::NotFound {
                reason: "no_match",
                ..
            })
        ));
        // Two equal titles: ambiguous, and the message points at --index.
        match match_tab_exact(&entries, &TabCloseSpec::Title("Codex".into())) {
            Err(TabMatchError::Ambiguous { count, message }) => {
                assert_eq!(count, 2);
                assert!(message.contains("--index"), "{message}");
            }
            other => panic!("duplicates must be ambiguous: {other:?}"),
        }
        assert_eq!(
            match_tab_exact(&entries, &TabCloseSpec::Index(2))
                .expect("third")
                .node
                .id,
            "/0/1/2"
        );
        assert!(matches!(
            match_tab_exact(&entries, &TabCloseSpec::Index(3)),
            Err(TabMatchError::NotFound {
                reason: "no_match",
                ..
            })
        ));
        assert!(matches!(
            match_tab_exact(&[], &TabCloseSpec::Index(0)),
            Err(TabMatchError::NotFound {
                reason: "no_tab_strip",
                ..
            })
        ));
    }

    #[test]
    fn index_after_close_shifts_only_the_tabs_to_the_right() {
        assert_eq!(index_after_close(0, 3), Some(0));
        assert_eq!(index_after_close(3, 3), None);
        assert_eq!(index_after_close(5, 3), Some(4));
    }

    #[test]
    fn new_tab_is_the_selected_row_that_the_strip_gained() {
        let row = |index: usize, title: &str, selected: bool| TabRow {
            index,
            title: title.into(),
            selected: if selected { Tri::True } else { Tri::False },
        };
        let before = vec![row(0, "Inbox", true), row(1, "Codex", false)];
        // One more row, the selected one: that is the new tab.
        let grown = vec![
            row(0, "Inbox", false),
            row(1, "Codex", false),
            row(2, "cu-real-1", true),
        ];
        let hit = new_tab_from_strips(&before, &grown).expect("new tab");
        assert_eq!((hit.index, hit.title.as_str()), (2, "cu-real-1"));
        // Same size but a title the strip did not have (a tab replaced by
        // navigation): still told apart by the title.
        let replaced = vec![row(0, "Inbox", false), row(1, "cu-real-2", true)];
        assert_eq!(
            new_tab_from_strips(&before, &replaced).map(|r| r.index),
            Some(1)
        );
        // Same size, known title: nothing new can be claimed.
        let same = vec![row(0, "Inbox", false), row(1, "Codex", true)];
        assert_eq!(new_tab_from_strips(&before, &same), None);
        // No single selected row: unknown.
        let none_selected = vec![
            row(0, "Inbox", false),
            row(1, "x", false),
            row(2, "y", false),
        ];
        assert_eq!(new_tab_from_strips(&before, &none_selected), None);
        // A window created for the profile: the empty before-strip.
        let fresh = vec![row(0, "cu-real-3", true)];
        assert_eq!(
            new_tab_from_strips(&[], &fresh).map(|r| r.title.as_str()),
            Some("cu-real-3")
        );
        let tree = fake_tree(vec![
            node("/0", None, "window", "", &["showing"]),
            node("/0/1", Some("/0"), "tab-group", "", &["showing"]),
            node(
                "/0/1/0",
                Some("/0/1"),
                "radio-button",
                "A",
                &["showing", "selected"],
            ),
        ]);
        assert_eq!(TabRow::from_tree(&tree), vec![row(0, "A", true)]);
    }

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

    /// The Linux AT-SPI and Windows UIA adapters both publish the strip
    /// as `page tab list` / `page tab` -- with **spaces**, not hyphens.
    /// A role comparison that only strips hyphens finds no strip at all
    /// there, which made `tab list` / `tab select` answer an empty strip
    /// on both platforms. The macOS spelling must keep working next to it.
    #[test]
    fn strip_roles_match_every_adapter_separator_spelling() {
        let spelled = |container: &str, item: &str| {
            fake_tree(vec![
                node("/0", None, "window", "", &["showing"]),
                node("/0/0", Some("/0"), container, "", &["showing"]),
                node(
                    "/0/0/0",
                    Some("/0/0"),
                    item,
                    "One",
                    &["showing", "selected"],
                ),
                node(
                    "/0/0/1",
                    Some("/0/0"),
                    item,
                    "Two",
                    &["showing", "unselected"],
                ),
            ])
        };
        // AT-SPI (Linux) and UIA (Windows) both emit these two strings.
        let atspi = spelled("page tab list", "page tab");
        let entries = tab_strip_entries(&atspi);
        let titles: Vec<&str> = entries.iter().map(TabEntry::title).collect();
        assert_eq!(
            titles,
            ["One", "Two"],
            "space-separated AT-SPI / UIA roles are a tab strip"
        );
        assert_eq!(entries[0].selected(), Tri::True);
        // The macOS AX spelling, plain and prefixed, still matches.
        assert_eq!(
            tab_strip_entries(&spelled("tab-group", "radio-button")).len(),
            2
        );
        assert_eq!(
            tab_strip_entries(&spelled("AXTabGroup", "AXRadioButton")).len(),
            2
        );
        // UIA's own `Tab` / `TabItem` control types, and hyphenated
        // AT-SPI, stay strips as well.
        assert_eq!(tab_strip_entries(&spelled("Tab", "Tab Item")).len(), 2);
        assert_eq!(
            tab_strip_entries(&spelled("page-tab-list", "page-tab")).len(),
            2
        );
        // A non-strip container is still not a strip.
        assert!(tab_strip_entries(&spelled("group", "radio button")).is_empty());
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
