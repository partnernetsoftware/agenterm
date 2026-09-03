//! `page text`: the visible text of a window in reading order, shaped
//! from the accessibility tree so an agent never needs a screenshot to
//! read a page. Every row names the node that carries the text (`id`),
//! so the next step is `invoke --node` / `click --node`, never `--coords`.
//!
//! Two facts the shaping exists for:
//! - Chromium puts a web node's visible string in `AXValue` (`text`), not
//!   `AXTitle` (`name`): a `static-text` row has an empty `name` and its
//!   words in `text`. Reading `name` alone shows an empty page.
//! - The platform walk is breadth-first under a node budget, so `nodes`
//!   arrive level by level, not in document order. Reading order is the
//!   child-index path (`/0/3/1` before `/0/3/1/0` before `/0/4`).

use serde_json::{Value, json};

use crate::mechanism::{A11yNode, A11yTree};

/// Default byte budget for the joined text (`--max-bytes`).
pub const DEFAULT_MAX_BYTES: usize = 16 * 1024;
/// Largest `--max-bytes` a caller may name.
pub const MAX_MAX_BYTES: usize = 1024 * 1024;
/// Default walk depth when the caller names none. Deeper than the
/// platform default (32): real pages nest past that (measured 42 on an
/// Azure portal page), and depth alone costs nothing extra.
pub const DEFAULT_DEPTH: u32 = crate::observe::MAX_DEPTH_BUDGET;
/// Default node budget when the caller names none. The platform default
/// (1000) is spent on browser chrome by a breadth-first walk before deep
/// web content is reached; 6000 read a 774-node page in 0.26 s.
pub const DEFAULT_MAX_NODES: usize = 6_000;

/// Roles whose `name` is the concatenation or label of what is inside
/// them, so it is never a row of its own (their descendants are).
const CONTAINER_ROLES: &[&str] = &[
    "",
    "application",
    "cell",
    "column",
    "document",
    "generic",
    "group",
    "layout-area",
    "list",
    "outline",
    "region",
    "row",
    "scroll-area",
    "section",
    "splitter",
    "tab-group",
    "table",
    "toolbar",
    "unknown",
    "web-area",
    "window",
];

pub fn validate_max_bytes(max_bytes: Option<usize>) -> Result<usize, String> {
    match max_bytes {
        None => Ok(DEFAULT_MAX_BYTES),
        Some(0) => Err("--max-bytes must be 1..=1048576, got 0".into()),
        Some(value) if value > MAX_MAX_BYTES => Err(format!(
            "--max-bytes must be 1..={MAX_MAX_BYTES}, got {value}"
        )),
        Some(value) => Ok(value),
    }
}

fn is_showing(node: &A11yNode) -> bool {
    node.states
        .iter()
        .any(|state| state.eq_ignore_ascii_case("showing") || state.eq_ignore_ascii_case("visible"))
}

fn intersects(node: &A11yNode, within: Option<[i32; 4]>) -> bool {
    let b = &node.bounds;
    if b.width <= 0 || b.height <= 0 {
        return false;
    }
    let Some([x, y, w, h]) = within else {
        return true;
    };
    b.x < x.saturating_add(w)
        && b.x.saturating_add(b.width) > x
        && b.y < y.saturating_add(h)
        && b.y.saturating_add(b.height) > y
}

fn is_container(role: &str) -> bool {
    let normalized = crate::observe::normalize_role(role);
    CONTAINER_ROLES
        .iter()
        .any(|container| crate::observe::normalize_role(container) == normalized)
}

fn collapse_ws(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Roles whose `AXValue` is a number or a state, not their words: a
/// Chromium heading's value is its level (`"1"`), so the name is the text.
const VALUE_IS_NOT_TEXT_ROLES: &[&str] = &["heading", "progress-indicator", "slider", "stepper"];

/// The string a node shows: its value first, else its accessible name
/// for a non-container role.
fn label(node: &A11yNode) -> Option<String> {
    let normalized = crate::observe::normalize_role(&node.role);
    let value_is_text = !VALUE_IS_NOT_TEXT_ROLES
        .iter()
        .any(|role| crate::observe::normalize_role(role) == normalized);
    if value_is_text && let Some(text) = node.text.as_deref() {
        let text = collapse_ws(text);
        if !text.is_empty() {
            return Some(text);
        }
    }
    if is_container(&node.role) {
        return None;
    }
    let name = collapse_ws(&node.name);
    (!name.is_empty()).then_some(name)
}

fn path(id: &str) -> Vec<usize> {
    id.split('/')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap_or(usize::MAX))
        .collect()
}

/// One emitted row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextRow {
    pub id: String,
    pub role: String,
    pub text: String,
    /// The accessible name when it differs from `text` (a field's label,
    /// a link's title over its value).
    pub name: Option<String>,
    pub bounds: [i32; 4],
    pub focused: bool,
    pub actionable: bool,
}

impl TextRow {
    pub fn json(&self) -> Value {
        let mut row = json!({
            "id": self.id,
            "role": self.role,
            "text": self.text,
            "bounds": { "x": self.bounds[0], "y": self.bounds[1], "width": self.bounds[2], "height": self.bounds[3] },
        });
        if let Some(name) = &self.name {
            row["name"] = json!(name);
        }
        if self.focused {
            row["focused"] = json!(true);
        }
        if self.actionable {
            row["actionable"] = json!(true);
        }
        row
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reading {
    pub rows: Vec<TextRow>,
    /// Bytes of `text` across `rows`.
    pub bytes: usize,
    /// Rows were dropped because `max_bytes` was reached.
    pub truncated: bool,
    /// Showing nodes with a label that were considered.
    pub candidates: usize,
    /// Candidates dropped because an emitted ancestor already carries
    /// their words (a link's inner static text).
    pub merged: usize,
}

/// Visible text in reading order, bounded by `max_bytes` and optionally
/// by a screen rectangle. Order is the child-index path (document order),
/// not the breadth-first walk order the platform returns. A candidate whose
/// words are already part of the nearest emitted ancestor's text is merged
/// into that ancestor (so a link is one row, not a link plus its text).
pub fn read(tree: &A11yTree, within: Option<[i32; 4]>, max_bytes: usize) -> Reading {
    let mut ordered: Vec<&A11yNode> = tree.nodes.iter().collect();
    ordered.sort_by_cached_key(|node| path(&node.id));
    let mut rows: Vec<TextRow> = Vec::new();
    // (id, text) of emitted rows, innermost last, for ancestor merging.
    let mut emitted: Vec<(String, String)> = Vec::new();
    let mut bytes = 0usize;
    let mut truncated = false;
    let mut candidates = 0usize;
    let mut merged = 0usize;
    for node in ordered {
        if !is_showing(node) || !intersects(node, within) {
            continue;
        }
        let Some(text) = label(node) else {
            continue;
        };
        candidates += 1;
        while let Some((ancestor, _)) = emitted.last()
            && !node.id.starts_with(&format!("{ancestor}/"))
        {
            emitted.pop();
        }
        if let Some((_, ancestor_text)) = emitted.last()
            && ancestor_text.contains(text.as_str())
        {
            merged += 1;
            continue;
        }
        if bytes + text.len() > max_bytes {
            truncated = true;
            break;
        }
        bytes += text.len();
        let name = collapse_ws(&node.name);
        let name = (!name.is_empty() && name != text).then_some(name);
        rows.push(TextRow {
            id: node.id.clone(),
            role: node.role.clone(),
            text: text.clone(),
            name,
            bounds: [
                node.bounds.x,
                node.bounds.y,
                node.bounds.width,
                node.bounds.height,
            ],
            focused: node
                .states
                .iter()
                .any(|state| state.eq_ignore_ascii_case("focused")),
            actionable: node
                .actions
                .iter()
                .any(|action| action.eq_ignore_ascii_case("click")),
        });
        emitted.push((node.id.clone(), text));
    }
    Reading {
        rows,
        bytes,
        truncated,
        candidates,
        merged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mechanism::A11yBounds;

    fn node(
        id: &str,
        role: &str,
        name: &str,
        text: Option<&str>,
        bounds: [i32; 4],
        states: &[&str],
    ) -> A11yNode {
        A11yNode {
            id: id.into(),
            parent_id: id.rsplit_once('/').map(|(parent, _)| parent.to_owned()),
            role: role.into(),
            name: name.into(),
            states: states.iter().map(|s| (*s).to_owned()).collect(),
            bounds: A11yBounds {
                x: bounds[0],
                y: bounds[1],
                width: bounds[2],
                height: bounds[3],
            },
            actions: if matches!(role, "link" | "button") {
                vec!["click".into()]
            } else {
                Vec::new()
            },
            text: text.map(str::to_owned),
            identifier: None,
        }
    }

    /// A Chromium-shaped page in the platform's breadth-first order.
    fn page() -> A11yTree {
        let s = &["showing", "visible"];
        let nodes = vec![
            node("/0", "window", "Sign in", None, [0, 0, 800, 600], s),
            node("/0/1", "web-area", "Sign in", None, [0, 60, 800, 540], s),
            node("/0/0", "toolbar", "", None, [0, 0, 800, 60], s),
            node(
                "/0/1/0",
                "group",
                "Heading Enter code Code Next",
                None,
                [0, 60, 800, 540],
                s,
            ),
            node(
                "/0/0/0",
                "text-field",
                "Address and search bar",
                Some("example.test"),
                [10, 10, 700, 30],
                s,
            ),
            node(
                "/0/1/0/2",
                "text-field",
                "Code",
                Some(""),
                [20, 200, 300, 30],
                &["showing", "visible", "focused"],
            ),
            node(
                "/0/1/0/0",
                "heading",
                "Enter code",
                Some("2"),
                [20, 100, 300, 30],
                s,
            ),
            node(
                "/0/1/0/1",
                "static-text",
                "",
                Some("  Type the   code shown  "),
                [20, 150, 300, 20],
                s,
            ),
            node("/0/1/0/3", "button", "Next", None, [20, 250, 100, 30], s),
            node(
                "/0/1/0/4",
                "link",
                "Privacy Statement",
                None,
                [20, 500, 150, 20],
                s,
            ),
            node(
                "/0/1/0/5",
                "static-text",
                "",
                Some("hidden"),
                [0, 0, 0, 0],
                s,
            ),
            node(
                "/0/1/0/6",
                "static-text",
                "",
                Some("offscreen"),
                [20, 900, 100, 20],
                &["enabled"],
            ),
            node(
                "/0/1/0/0/0",
                "static-text",
                "",
                Some("Enter code"),
                [20, 100, 300, 30],
                s,
            ),
            node(
                "/0/1/0/4/0",
                "static-text",
                "",
                Some("Privacy Statement"),
                [20, 500, 150, 20],
                s,
            ),
            node(
                "/0/1/0/3/0",
                "static-text",
                "",
                Some("Next"),
                [20, 250, 100, 30],
                s,
            ),
        ];
        let returned = nodes.len();
        A11yTree {
            backend: "ax".into(),
            window_handle: Some(1),
            root_id: "/0".into(),
            nodes,
            truncated: false,
            visited: returned,
            returned,
        }
    }

    #[test]
    fn rows_are_document_order_with_value_text_and_merged_children() {
        let reading = read(&page(), None, DEFAULT_MAX_BYTES);
        let texts: Vec<(&str, &str)> = reading
            .rows
            .iter()
            .map(|row| (row.role.as_str(), row.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            [
                ("text-field", "example.test"),
                ("heading", "Enter code"),
                ("static-text", "Type the code shown"),
                ("text-field", "Code"),
                ("button", "Next"),
                ("link", "Privacy Statement"),
            ]
        );
        assert!(!reading.truncated);
        assert_eq!(
            reading.merged, 3,
            "link / heading / button inner text merged"
        );
        assert_eq!(reading.candidates, 9);
        // A heading's AXValue is its level, so its name is its text.
        assert_eq!(reading.rows[1].text, "Enter code");
        // The omnibox keeps its label next to its value.
        assert_eq!(
            reading.rows[0].name.as_deref(),
            Some("Address and search bar")
        );
        assert!(reading.rows[4].actionable);
        assert!(!reading.rows[1].actionable);
        let row = reading.rows[5].json();
        assert_eq!(row["id"], "/0/1/0/4");
        assert_eq!(row["bounds"]["x"], 20);
        assert_eq!(row["actionable"], true);
        assert!(
            row.get("name").is_none(),
            "name equal to text is not repeated"
        );
    }

    #[test]
    fn empty_field_shows_its_label_but_containers_never_speak_for_children() {
        let reading = read(&page(), None, DEFAULT_MAX_BYTES);
        // The focused empty code field has no value, so its label is its
        // row (that is the `<input>` an agent needs to find); the group's
        // concatenated name is not a row, nor is anything hidden / offscreen.
        assert!(reading.rows.iter().all(|row| row.role != "group"));
        let field = reading
            .rows
            .iter()
            .find(|row| row.id == "/0/1/0/2")
            .expect("empty field row");
        assert_eq!(field.text, "Code");
        assert!(field.focused);
        assert_eq!(field.json()["focused"], true);
        assert!(field.json().get("name").is_none());
        assert!(
            reading
                .rows
                .iter()
                .all(|row| row.text != "hidden" && row.text != "offscreen")
        );
    }

    #[test]
    fn within_and_max_bytes_bound_the_reading() {
        let reading = read(&page(), Some([0, 90, 400, 200]), DEFAULT_MAX_BYTES);
        let texts: Vec<&str> = reading.rows.iter().map(|row| row.text.as_str()).collect();
        assert_eq!(texts, ["Enter code", "Type the code shown", "Code", "Next"]);
        let small = read(&page(), None, 25);
        assert!(small.truncated);
        assert_eq!(
            small.rows.len(),
            2,
            "example.test + Enter code fit in 25 bytes"
        );
        assert_eq!(small.bytes, 22);
        assert_eq!(validate_max_bytes(None), Ok(DEFAULT_MAX_BYTES));
        assert!(validate_max_bytes(Some(0)).is_err());
        assert!(validate_max_bytes(Some(MAX_MAX_BYTES + 1)).is_err());
    }
}
