//! Pure shaping of a CDP `Accessibility.getFullAXTree` result: the same
//! `{id, role, text}` rows the AX `page text` verb returns, and the node
//! matcher behind `page find --text / --role`. No I/O; tested on fixture
//! trees.

use std::collections::HashMap;

use serde_json::{Value, json};

/// One node of the CDP accessibility tree, as this binary keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AxNode {
    pub id: String,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub ignored: bool,
    pub focused: bool,
    pub editable: bool,
    /// The DOM node behind this AX node (`backendDOMNodeId`): the handle
    /// `page click --node` / `page fill --node` take.
    pub backend_node_id: Option<u64>,
}

impl AxNode {
    /// The words this node carries: a field's value, else its name.
    pub fn text(&self) -> &str {
        match self.value.as_deref() {
            Some(value) if !value.is_empty() && VALUE_ROLES.contains(&self.role.as_str()) => value,
            _ => self.name.as_str(),
        }
    }

    /// `id` of a row: the backend DOM node id (what the actuators take),
    /// or the AX id prefixed when the node has no DOM node.
    pub fn row_id(&self) -> String {
        match self.backend_node_id {
            Some(id) => id.to_string(),
            None => format!("ax:{}", self.id),
        }
    }
}

/// Roles whose `value` (not `name`) is the text a reader wants.
const VALUE_ROLES: &[&str] = &["textbox", "searchbox", "combobox", "spinbutton", "slider"];

/// Roles that are never a text row of their own (containers, roots).
const CONTAINER_ROLES: &[&str] = &[
    "RootWebArea",
    "WebArea",
    "generic",
    "none",
    "presentation",
    "group",
    "section",
    "paragraph",
    "list",
    "listitem",
    "table",
    "row",
    "rowgroup",
    "navigation",
    "main",
    "banner",
    "contentinfo",
    "complementary",
    "form",
    "region",
    "article",
    "Iframe",
    "IframePresentational",
    "LayoutTable",
    "LayoutTableRow",
    "LayoutTableCell",
    "document",
];

/// Roles a click lands on: when a text match sits inside one of these
/// with the same words, the match is lifted to the control.
pub const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "link",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "checkbox",
    "radio",
    "switch",
    "option",
    "treeitem",
    "textbox",
    "searchbox",
    "combobox",
    "spinbutton",
    "slider",
    "listbox",
];

fn prop_bool(properties: &Value, name: &str) -> bool {
    properties
        .as_array()
        .into_iter()
        .flatten()
        .find(|property| property["name"] == name)
        .and_then(|property| property["value"]["value"].as_bool())
        .unwrap_or(false)
}

/// Parse the `nodes` array of a `getFullAXTree` result.
pub fn parse_tree(result: &Value) -> Vec<AxNode> {
    result["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node.is_object())
        .map(|node| AxNode {
            id: node["nodeId"].as_str().unwrap_or_default().to_owned(),
            parent: node["parentId"].as_str().map(str::to_owned),
            children: node["childIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            role: node["role"]["value"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            name: node["name"]["value"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            value: node["value"]["value"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| {
                    node["value"]["value"]
                        .as_f64()
                        .map(|number| number.to_string())
                }),
            ignored: node["ignored"].as_bool().unwrap_or(false),
            focused: prop_bool(&node["properties"], "focused"),
            editable: node["properties"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|property| {
                    property["name"] == "editable"
                        && property["value"]["value"]
                            .as_str()
                            .is_some_and(|value| value != "false")
                }),
            backend_node_id: node["backendDOMNodeId"].as_u64(),
        })
        .collect()
}

/// Indices in document order: a pre-order walk from the root (the first
/// node without a parent) through `childIds`, then any node the walk did
/// not reach, in listing order.
pub fn document_order(nodes: &[AxNode]) -> Vec<usize> {
    let index: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (node.id.as_str(), position))
        .collect();
    let mut seen = vec![false; nodes.len()];
    let mut order = Vec::with_capacity(nodes.len());
    let mut stack: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.parent.is_none())
        .map(|(position, _)| position)
        .rev()
        .collect();
    while let Some(position) = stack.pop() {
        if seen[position] {
            continue;
        }
        seen[position] = true;
        order.push(position);
        for child in nodes[position].children.iter().rev() {
            if let Some(&child_position) = index.get(child.as_str())
                && !seen[child_position]
            {
                stack.push(child_position);
            }
        }
    }
    order.extend((0..nodes.len()).filter(|position| !seen[*position]));
    order
}

/// One `page text` row shaped from the AX tree.
#[derive(Clone, Debug, PartialEq)]
pub struct TextRow {
    pub id: String,
    pub node: Option<u64>,
    pub role: String,
    pub text: String,
    pub name: Option<String>,
    pub focused: bool,
    pub editable: bool,
}

impl TextRow {
    pub fn json(&self) -> Value {
        let mut row = json!({
            "id": self.id,
            "role": self.role,
            "text": self.text,
        });
        if let Some(node) = self.node {
            row["node"] = json!(node);
        }
        if let Some(name) = &self.name {
            row["name"] = json!(name);
        }
        if self.focused {
            row["focused"] = json!(true);
        }
        if self.editable {
            row["editable"] = json!(true);
        }
        row
    }
}

/// The `page text` reading of one tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub rows: Vec<TextRow>,
    pub bytes: usize,
    pub truncated: bool,
    /// Nodes that carried words before merging.
    pub candidates: usize,
    /// Candidates folded into a named ancestor (a link's inner text).
    pub merged: usize,
}

fn is_candidate(node: &AxNode) -> bool {
    !node.ignored && !CONTAINER_ROLES.contains(&node.role.as_str()) && !node.text().is_empty()
}

/// Rows in document order: every non-ignored, non-container node with
/// words, minus those whose nearest worded ancestor already carries the
/// same words (a button's inner static text is the button row). Cut at
/// `max_bytes` of text.
pub fn text_rows(nodes: &[AxNode], max_bytes: usize) -> Reading {
    let by_id: HashMap<&str, &AxNode> = nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let mut reading = Reading::default();
    for position in document_order(nodes) {
        let node = &nodes[position];
        if !is_candidate(node) {
            continue;
        }
        reading.candidates += 1;
        let text = node.text();
        let mut merged = false;
        let mut cursor = node.parent.as_deref();
        while let Some(parent_id) = cursor {
            let Some(parent) = by_id.get(parent_id) else {
                break;
            };
            if is_candidate(parent) {
                merged = parent.text().to_lowercase().contains(&text.to_lowercase());
                break;
            }
            cursor = parent.parent.as_deref();
        }
        if merged {
            reading.merged += 1;
            continue;
        }
        if reading.bytes + text.len() > max_bytes {
            reading.truncated = true;
            break;
        }
        reading.bytes += text.len();
        let name = (!node.name.is_empty() && node.name != text).then(|| node.name.clone());
        reading.rows.push(TextRow {
            id: node.row_id(),
            node: node.backend_node_id,
            role: node.role.clone(),
            text: text.to_owned(),
            name,
            focused: node.focused,
            editable: node.editable,
        });
    }
    reading
}

/// What `page find` asks the AX tree for.
#[derive(Clone, Debug, PartialEq)]
pub enum AxQuery {
    /// Case-insensitive substring of the node's words.
    Text(String),
    /// Exact role (case-insensitive), optionally with a name substring.
    Role { role: String, name: Option<String> },
}

impl AxQuery {
    pub fn json(&self) -> Value {
        match self {
            Self::Text(text) => json!({ "text": text }),
            Self::Role { role, name } => json!({ "role": role, "name": name }),
        }
    }
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Matching nodes in document order.
///
/// A text query keeps the innermost node that carries the words (the
/// static text, not every ancestor whose name repeats it), then lifts a
/// match to its nearest interactive ancestor (`button`, `link`, ...) when
/// that ancestor's name carries the same words — a click wants the
/// control, not the glyphs. A role query is a plain filter.
pub fn find_nodes<'a>(nodes: &'a [AxNode], query: &AxQuery) -> Vec<&'a AxNode> {
    let by_id: HashMap<&str, &AxNode> = nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let order = document_order(nodes);
    match query {
        AxQuery::Role { role, name } => order
            .into_iter()
            .map(|position| &nodes[position])
            .filter(|node| !node.ignored && node.role.eq_ignore_ascii_case(role))
            .filter(|node| {
                name.as_deref()
                    .is_none_or(|needle| contains_ci(&node.name, needle))
            })
            .collect(),
        AxQuery::Text(needle) => {
            let hits: Vec<&AxNode> = order
                .into_iter()
                .map(|position| &nodes[position])
                .filter(|node| !node.ignored && contains_ci(node.text(), needle))
                .collect();
            let hit_ids: std::collections::HashSet<&str> =
                hits.iter().map(|node| node.id.as_str()).collect();
            let has_hit_descendant = |node: &AxNode| -> bool {
                let mut stack: Vec<&str> = node.children.iter().map(String::as_str).collect();
                while let Some(id) = stack.pop() {
                    if hit_ids.contains(id) {
                        return true;
                    }
                    if let Some(child) = by_id.get(id) {
                        stack.extend(child.children.iter().map(String::as_str));
                    }
                }
                false
            };
            let mut out: Vec<&AxNode> = Vec::new();
            for hit in hits.iter().copied() {
                if has_hit_descendant(hit) {
                    continue;
                }
                // Lift to the nearest interactive ancestor with the same words.
                let mut chosen = hit;
                let mut cursor = hit.parent.as_deref();
                let mut depth = 0;
                while let Some(parent_id) = cursor {
                    depth += 1;
                    if depth > 6 {
                        break;
                    }
                    let Some(parent) = by_id.get(parent_id) else {
                        break;
                    };
                    if INTERACTIVE_ROLES.contains(&parent.role.as_str()) {
                        if contains_ci(parent.text(), needle) {
                            chosen = parent;
                        }
                        break;
                    }
                    cursor = parent.parent.as_deref();
                }
                if !out.iter().any(|node| node.id == chosen.id) {
                    out.push(chosen);
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ax(id: &str, parent: Option<&str>, children: &[&str], role: &str, name: &str) -> Value {
        let mut node = json!({
            "nodeId": id,
            "role": { "type": "role", "value": role },
            "name": { "type": "computedString", "value": name },
            "childIds": children,
            "backendDOMNodeId": id.parse::<u64>().unwrap_or(0) + 100,
        });
        if let Some(parent) = parent {
            node["parentId"] = json!(parent);
        }
        node
    }

    /// <h1>Hello B</h1> <input aria-label="Search" value="hi"> <button><span>Go</span></button>
    /// <p>idle <a href>Docs</a></p>, listed out of document order on purpose.
    fn fixture() -> Vec<AxNode> {
        let mut nodes = vec![
            ax(
                "1",
                None,
                &["2", "4", "6", "9"],
                "RootWebArea",
                "cu-smoke-B",
            ),
            ax("2", Some("1"), &["3"], "heading", "Hello B"),
            ax("3", Some("2"), &[], "StaticText", "Hello B"),
            ax("4", Some("1"), &[], "textbox", "Search"),
            ax("6", Some("1"), &["7"], "button", "Go"),
            ax("7", Some("6"), &["8"], "generic", ""),
            ax("8", Some("7"), &[], "StaticText", "Go"),
            ax("9", Some("1"), &["10", "11"], "paragraph", ""),
            ax("10", Some("9"), &[], "StaticText", "idle "),
            ax("11", Some("9"), &["12"], "link", "Docs"),
            ax("12", Some("11"), &[], "StaticText", "Docs"),
        ];
        nodes[3]["value"] = json!({ "type": "string", "value": "hi" });
        nodes[3]["properties"] = json!([
            { "name": "focused", "value": { "type": "boolean", "value": true } },
            { "name": "editable", "value": { "type": "token", "value": "plaintext" } }
        ]);
        // Out of order: the reader must follow childIds, not the listing.
        nodes.swap(1, 9);
        parse_tree(&json!({ "nodes": nodes }))
    }

    #[test]
    fn text_rows_follow_document_order_and_merge_inner_text() {
        let nodes = fixture();
        let reading = text_rows(&nodes, 16 * 1024);
        let texts: Vec<(&str, &str)> = reading
            .rows
            .iter()
            .map(|row| (row.role.as_str(), row.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            [
                ("heading", "Hello B"),
                ("textbox", "hi"),
                ("button", "Go"),
                ("StaticText", "idle "),
                ("link", "Docs"),
            ]
        );
        assert_eq!(
            reading.candidates, 8,
            "root and containers are not candidates"
        );
        assert_eq!(reading.merged, 3);
        assert!(!reading.truncated);
        let field = &reading.rows[1];
        assert_eq!(field.name.as_deref(), Some("Search"));
        assert!(field.focused && field.editable);
        assert_eq!(field.id, "104");
        assert_eq!(field.node, Some(104));
        let row = field.json();
        assert_eq!(row["id"], "104");
        assert_eq!(row["node"], 104);
        assert_eq!(row["name"], "Search");
        assert_eq!(row["focused"], true);
        assert!(reading.rows[0].json().get("name").is_none());
    }

    #[test]
    fn text_rows_are_cut_at_the_byte_budget() {
        let nodes = fixture();
        let reading = text_rows(&nodes, 9);
        assert_eq!(
            reading.rows.len(),
            2,
            "Hello B (7) + hi (2) fit; Go does not"
        );
        assert!(reading.truncated);
        assert_eq!(reading.bytes, 9);
    }

    #[test]
    fn text_query_keeps_the_innermost_match_lifted_to_its_control() {
        let nodes = fixture();
        let go = find_nodes(&nodes, &AxQuery::Text("go".into()));
        assert_eq!(go.len(), 1);
        assert_eq!(go[0].role, "button");
        assert_eq!(go[0].backend_node_id, Some(106));
        let docs = find_nodes(&nodes, &AxQuery::Text("Docs".into()));
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].role, "link");
        // A heading is not interactive: the static text inside it is the hit.
        let hello = find_nodes(&nodes, &AxQuery::Text("hello".into()));
        assert_eq!(hello.len(), 1);
        assert_eq!(hello[0].role, "StaticText");
        assert_eq!(hello[0].id, "3");
        // The field's value matches too.
        let hi = find_nodes(&nodes, &AxQuery::Text("hi".into()));
        assert_eq!(hi.len(), 1);
        assert_eq!(hi[0].role, "textbox");
        assert!(find_nodes(&nodes, &AxQuery::Text("nowhere".into())).is_empty());
    }

    #[test]
    fn role_query_filters_by_role_then_name_substring() {
        let nodes = fixture();
        let statics = find_nodes(
            &nodes,
            &AxQuery::Role {
                role: "statictext".into(),
                name: None,
            },
        );
        assert_eq!(statics.len(), 4);
        assert_eq!(statics[0].name, "Hello B", "document order");
        let button = find_nodes(
            &nodes,
            &AxQuery::Role {
                role: "button".into(),
                name: Some("GO".into()),
            },
        );
        assert_eq!(button.len(), 1);
        assert_eq!(button[0].id, "6");
        assert!(
            find_nodes(
                &nodes,
                &AxQuery::Role {
                    role: "button".into(),
                    name: Some("stop".into()),
                }
            )
            .is_empty()
        );
        assert_eq!(
            AxQuery::Role {
                role: "button".into(),
                name: None
            }
            .json(),
            json!({ "role": "button", "name": null })
        );
    }

    #[test]
    fn parse_tolerates_missing_fields_and_ignored_nodes() {
        let nodes = parse_tree(&json!({ "nodes": [
            { "nodeId": "1", "ignored": true, "role": { "value": "generic" } },
            { "nodeId": "2", "role": { "value": "slider" }, "name": { "value": "Volume" },
              "value": { "type": "number", "value": 40 }, "childIds": [] },
            "not an object"
        ] }));
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].ignored);
        assert_eq!(nodes[0].backend_node_id, None);
        assert_eq!(nodes[0].row_id(), "ax:1");
        assert_eq!(nodes[1].text(), "40");
        assert_eq!(parse_tree(&json!({})).len(), 0);
        assert_eq!(document_order(&nodes), vec![0, 1]);
    }
}
