//! Pure observation helpers shared by `tree --flat`, `query`, and the
//! `windows` inventory: flatten indices, node depth, filters, and paging with
//! `visited / matched / returned / truncated` counts. No mechanism calls live
//! here, so every rule is unit-tested without a desktop.

use crate::mechanism::window_enumerate::WindowInfo;
use crate::mechanism::{A11yNode, A11yTree};

/// Largest node budget a caller may name (`--max-nodes`); mirrors the
/// platform contract's `MAX_TREE_NODE_BUDGET`.
pub const MAX_NODE_BUDGET: usize = 20_000;
/// Deepest `--depth` a caller may name (root = 0); mirrors the platform
/// contract's `MAX_TREE_DEPTH_BUDGET`.
pub const MAX_DEPTH_BUDGET: u32 = 64;
/// Page size when `--max` is absent.
pub const DEFAULT_PAGE_MAX: usize = 200;
/// Largest page a caller may name (`--max`).
pub const MAX_PAGE_MAX: usize = MAX_NODE_BUDGET;

/// Typed `invalid_input` text for an out-of-range tree budget, or `None`.
pub fn validate_budget(depth: Option<u32>, max_nodes: Option<usize>) -> Result<(), String> {
    if let Some(depth) = depth
        && depth > MAX_DEPTH_BUDGET
    {
        return Err(format!(
            "--depth must be 0..={MAX_DEPTH_BUDGET}, got {depth}"
        ));
    }
    if let Some(max_nodes) = max_nodes
        && (max_nodes == 0 || max_nodes > MAX_NODE_BUDGET)
    {
        return Err(format!(
            "--max-nodes must be 1..={MAX_NODE_BUDGET}, got {max_nodes}"
        ));
    }
    Ok(())
}

/// One page request over an ordered match list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Page {
    pub offset: usize,
    pub max: usize,
}

impl Page {
    /// Typed `invalid_input` text for a page outside `1..=MAX_PAGE_MAX`.
    pub fn new(offset: Option<usize>, max: Option<usize>) -> Result<Self, String> {
        let max = max.unwrap_or(DEFAULT_PAGE_MAX);
        if max == 0 || max > MAX_PAGE_MAX {
            return Err(format!("--max must be 1..={MAX_PAGE_MAX}, got {max}"));
        }
        Ok(Self {
            offset: offset.unwrap_or(0),
            max,
        })
    }

    /// The slice of `matched` this page returns, plus whether matches were
    /// left past its end.
    pub fn apply<'a, T>(&self, matched: &'a [T]) -> (&'a [T], bool) {
        let start = self.offset.min(matched.len());
        let end = start.saturating_add(self.max).min(matched.len());
        (&matched[start..end], end < matched.len())
    }
}

/// Depth of a path id: `/0` is 0, `/0/3/1` is 2. A malformed id counts its
/// separators anyway, so a node is never silently dropped.
pub fn node_depth(id: &str) -> u32 {
    let separators = id.matches('/').count();
    separators.saturating_sub(1) as u32
}

/// A node with its flatten index (position in the tree's walk order) and
/// depth. The index is what `tree --flat` numbers and what `query` reports,
/// so both name the same node the same way.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FlatNode<'a> {
    pub index: usize,
    pub depth: u32,
    #[serde(flatten)]
    pub node: &'a A11yNode,
}

pub fn flatten(tree: &A11yTree) -> Vec<FlatNode<'_>> {
    tree.nodes
        .iter()
        .enumerate()
        .map(|(index, node)| FlatNode {
            index,
            depth: node_depth(&node.id),
            node,
        })
        .collect()
}

/// The role spelling both the caller and the backend may use: `AXTextArea`,
/// `text-area`, `TextArea`, `text area` and `textarea` all normalize to
/// `textarea`, so a filter written in the platform's vocabulary matches the
/// contract's kebab-case role and vice versa.
pub fn normalize_role(raw: &str) -> String {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix("AX").unwrap_or(trimmed);
    stripped
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

/// Parse a comma-separated role list, dropping empty items.
pub fn parse_roles(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Parse `X,Y,W,H` for `--within`; width and height must be positive.
pub fn parse_within(raw: &str) -> Result<[i32; 4], String> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return Err(format!("--within expects X,Y,W,H, got {raw:?}"));
    }
    let mut values = [0i32; 4];
    for (slot, part) in values.iter_mut().zip(parts.iter()) {
        *slot = part
            .parse()
            .map_err(|_| format!("--within component {part:?} is not a signed 32-bit integer"))?;
    }
    if values[2] <= 0 || values[3] <= 0 {
        return Err(format!(
            "--within width and height must be positive, got {raw:?}"
        ));
    }
    Ok(values)
}

/// The filter half of `query`. Every field is an AND term; an absent field
/// matches everything.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeFilter {
    /// Normalized with [`normalize_role`]; empty means any role.
    pub roles: Vec<String>,
    /// Case-insensitive substring of `name` or `text`.
    pub text: Option<String>,
    /// Exact `name` or `text`.
    pub text_exact: Option<String>,
    /// Exact `identifier`.
    pub identifier: Option<String>,
    /// At least one action.
    pub actionable: bool,
    /// Node bounds intersect this `[x, y, w, h]` screen rectangle.
    pub within: Option<[i32; 4]>,
}

impl NodeFilter {
    pub fn from_parts(
        roles: &[String],
        text: Option<&str>,
        text_exact: Option<&str>,
        identifier: Option<&str>,
        actionable: bool,
        within: Option<[i32; 4]>,
    ) -> Self {
        Self {
            roles: roles.iter().map(|role| normalize_role(role)).collect(),
            text: text.map(|value| value.to_lowercase()),
            text_exact: text_exact.map(str::to_owned),
            identifier: identifier.map(str::to_owned),
            actionable,
            within,
        }
    }

    pub fn matches(&self, node: &A11yNode) -> bool {
        if !self.roles.is_empty() {
            let role = normalize_role(&node.role);
            if !self.roles.iter().any(|wanted| wanted == &role) {
                return false;
            }
        }
        if let Some(needle) = &self.text {
            let in_name = node.name.to_lowercase().contains(needle.as_str());
            let in_text = node
                .text
                .as_deref()
                .is_some_and(|text| text.to_lowercase().contains(needle.as_str()));
            if !in_name && !in_text {
                return false;
            }
        }
        if let Some(exact) = &self.text_exact
            && node.name != *exact
            && node.text.as_deref() != Some(exact.as_str())
        {
            return false;
        }
        if let Some(identifier) = &self.identifier
            && node.identifier.as_deref() != Some(identifier.as_str())
        {
            return false;
        }
        if self.actionable && node.actions.is_empty() {
            return false;
        }
        if let Some([x, y, w, h]) = self.within {
            let b = &node.bounds;
            let intersects = b.width > 0
                && b.height > 0
                && b.x < x.saturating_add(w)
                && b.x.saturating_add(b.width) > x
                && b.y < y.saturating_add(h)
                && b.y.saturating_add(b.height) > y;
            if !intersects {
                return false;
            }
        }
        true
    }
}

/// Counts every bounded list reply carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ListCounts {
    pub visited: usize,
    pub matched: usize,
    pub returned: usize,
    pub offset: usize,
    /// `scan_truncated || page_truncated`.
    pub truncated: bool,
    /// The underlying walk stopped at its budget.
    pub scan_truncated: bool,
    /// Matches exist past this page.
    pub page_truncated: bool,
}

/// Filter and page the flattened tree.
pub fn query<'a>(
    flat: &'a [FlatNode<'a>],
    filter: &NodeFilter,
    page: Page,
    scan_truncated: bool,
) -> (Vec<&'a FlatNode<'a>>, ListCounts) {
    let matched: Vec<&FlatNode<'_>> = flat
        .iter()
        .filter(|entry| filter.matches(entry.node))
        .collect();
    let (returned, page_truncated) = page.apply(&matched);
    let counts = ListCounts {
        visited: flat.len(),
        matched: matched.len(),
        returned: returned.len(),
        offset: page.offset,
        truncated: scan_truncated || page_truncated,
        scan_truncated,
        page_truncated,
    };
    (returned.to_vec(), counts)
}

/// The filter half of the `windows` inventory.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WindowFilter {
    pub pid: Option<u32>,
    /// Case-insensitive substring of `app_name`.
    pub app: Option<String>,
    /// Case-insensitive substring of `title`.
    pub title: Option<String>,
    pub focused: Option<bool>,
    pub minimized: Option<bool>,
}

impl WindowFilter {
    pub fn is_empty(&self) -> bool {
        self.pid.is_none()
            && self.app.is_none()
            && self.title.is_none()
            && self.focused.is_none()
            && self.minimized.is_none()
    }

    pub fn matches(&self, window: &WindowInfo) -> bool {
        if self.pid.is_some_and(|pid| pid != window.process_id) {
            return false;
        }
        if let Some(app) = &self.app
            && !window
                .app_name
                .to_lowercase()
                .contains(app.to_lowercase().as_str())
        {
            return false;
        }
        if let Some(title) = &self.title
            && !window
                .title
                .to_lowercase()
                .contains(title.to_lowercase().as_str())
        {
            return false;
        }
        if self
            .focused
            .is_some_and(|focused| focused != window.focused)
        {
            return false;
        }
        if self
            .minimized
            .is_some_and(|minimized| minimized != window.minimized)
        {
            return false;
        }
        true
    }
}

/// Filter and page a window inventory. `scan_truncated` is always false
/// today: the enumeration mechanism returns its whole bounded list.
pub fn inventory<'a>(
    windows: &'a [WindowInfo],
    filter: &WindowFilter,
    page: Page,
) -> (Vec<&'a WindowInfo>, ListCounts) {
    let matched: Vec<&WindowInfo> = windows
        .iter()
        .filter(|window| filter.matches(window))
        .collect();
    let (returned, page_truncated) = page.apply(&matched);
    let counts = ListCounts {
        visited: windows.len(),
        matched: matched.len(),
        returned: returned.len(),
        offset: page.offset,
        truncated: page_truncated,
        scan_truncated: false,
        page_truncated,
    };
    (returned.to_vec(), counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mechanism::A11yBounds;
    use crate::mechanism::window_enumerate::WindowBounds;

    fn node(id: &str, role: &str, name: &str, actions: &[&str]) -> A11yNode {
        A11yNode {
            id: id.to_owned(),
            parent_id: None,
            role: role.to_owned(),
            name: name.to_owned(),
            states: Vec::new(),
            bounds: A11yBounds {
                x: 10,
                y: 20,
                width: 100,
                height: 50,
            },
            actions: actions.iter().map(|action| (*action).to_owned()).collect(),
            text: None,
            identifier: None,
        }
    }

    fn tree(nodes: Vec<A11yNode>, truncated: bool) -> A11yTree {
        let returned = nodes.len();
        A11yTree {
            backend: "ax".into(),
            window_handle: Some(1),
            root_id: "/0".into(),
            nodes,
            truncated,
            visited: returned,
            returned,
        }
    }

    #[test]
    fn budget_bounds_are_the_contract_ranges() {
        assert!(validate_budget(None, None).is_ok());
        assert!(validate_budget(Some(0), Some(1)).is_ok());
        assert!(validate_budget(Some(64), Some(20_000)).is_ok());
        assert!(validate_budget(Some(65), None).is_err());
        assert!(validate_budget(None, Some(0)).is_err());
        assert!(validate_budget(None, Some(20_001)).is_err());
    }

    #[test]
    fn page_bounds_and_slicing() {
        assert!(Page::new(None, Some(0)).is_err());
        assert!(Page::new(None, Some(MAX_PAGE_MAX + 1)).is_err());
        let page = Page::new(None, None).expect("default page");
        assert_eq!(page.max, DEFAULT_PAGE_MAX);
        let page = Page::new(Some(1), Some(2)).expect("page");
        let items = [10, 20, 30, 40];
        assert_eq!(page.apply(&items), (&items[1..3], true));
        let page = Page::new(Some(2), Some(5)).expect("page");
        assert_eq!(page.apply(&items), (&items[2..4], false));
        let page = Page::new(Some(9), Some(5)).expect("page");
        assert_eq!(page.apply(&items), (&items[4..4], false));
    }

    #[test]
    fn depth_counts_path_segments_below_the_root() {
        assert_eq!(node_depth("/0"), 0);
        assert_eq!(node_depth("/0/3"), 1);
        assert_eq!(node_depth("/0/3/1"), 2);
        assert_eq!(node_depth(""), 0);
    }

    #[test]
    fn flatten_numbers_nodes_in_walk_order() {
        let t = tree(
            vec![
                node("/0", "window", "Untitled", &[]),
                node("/0/0", "scroll-area", "", &[]),
                node("/0/0/0", "text-area", "", &["focus"]),
            ],
            false,
        );
        let flat = flatten(&t);
        assert_eq!(
            flat.iter().map(|entry| entry.index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            flat.iter().map(|entry| entry.depth).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        let json = serde_json::to_value(&flat[2]).expect("serialize");
        assert_eq!(json["index"], 2);
        assert_eq!(json["depth"], 2);
        assert_eq!(json["id"], "/0/0/0");
        assert_eq!(json["role"], "text-area");
    }

    #[test]
    fn role_spellings_converge() {
        assert_eq!(normalize_role("AXTextArea"), "textarea");
        assert_eq!(normalize_role("text-area"), "textarea");
        assert_eq!(normalize_role(" Text Area "), "textarea");
        assert_eq!(normalize_role("AXButton"), "button");
        assert_eq!(normalize_role("push button"), "pushbutton");
        assert_eq!(
            parse_roles("AXTextArea, button,,"),
            vec!["AXTextArea".to_owned(), "button".to_owned()]
        );
    }

    #[test]
    fn within_parses_four_positive_components() {
        assert_eq!(
            parse_within("0, 0,900,700").expect("rect"),
            [0, 0, 900, 700]
        );
        assert!(parse_within("0,0,900").is_err());
        assert!(parse_within("0,0,0,700").is_err());
        assert!(parse_within("a,0,1,1").is_err());
    }

    #[test]
    fn filters_are_and_terms_over_the_same_nodes() {
        let mut focused = node("/0/0/0", "text-area", "", &["focus"]);
        focused.text = Some("345AXTREE".into());
        focused.identifier = Some("editor".into());
        let nodes = vec![
            node("/0", "window", "Untitled", &[]),
            node("/0/1", "button", "Fixture Press", &["click"]),
            focused,
        ];
        let t = tree(nodes, false);
        let flat = flatten(&t);
        let page = Page::new(None, None).expect("page");

        let by_role = NodeFilter::from_parts(&["AXTextArea".into()], None, None, None, false, None);
        let (hits, counts) = query(&flat, &by_role, page, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].index, 2);
        assert_eq!(counts.visited, 3);
        assert_eq!(counts.matched, 1);
        assert_eq!(counts.returned, 1);
        assert!(!counts.truncated);

        let by_text = NodeFilter::from_parts(&[], Some("fixture"), None, None, false, None);
        let (hits, _) = query(&flat, &by_text, page, false);
        assert_eq!(
            hits.iter().map(|hit| hit.index).collect::<Vec<_>>(),
            vec![1]
        );

        let by_value = NodeFilter::from_parts(&[], Some("axtree"), None, None, false, None);
        let (hits, _) = query(&flat, &by_value, page, false);
        assert_eq!(
            hits.iter().map(|hit| hit.index).collect::<Vec<_>>(),
            vec![2]
        );

        let exact = NodeFilter::from_parts(&[], None, Some("Fixture"), None, false, None);
        assert_eq!(query(&flat, &exact, page, false).0.len(), 0);
        let exact = NodeFilter::from_parts(&[], None, Some("Fixture Press"), None, false, None);
        assert_eq!(query(&flat, &exact, page, false).0.len(), 1);

        let by_identifier = NodeFilter::from_parts(&[], None, None, Some("editor"), false, None);
        assert_eq!(query(&flat, &by_identifier, page, false).0[0].index, 2);

        let actionable = NodeFilter::from_parts(&[], None, None, None, true, None);
        assert_eq!(query(&flat, &actionable, page, false).0.len(), 2);

        let inside = NodeFilter::from_parts(&[], None, None, None, false, Some([0, 0, 50, 50]));
        assert_eq!(query(&flat, &inside, page, false).0.len(), 3);
        let outside =
            NodeFilter::from_parts(&[], None, None, None, false, Some([500, 500, 10, 10]));
        assert_eq!(query(&flat, &outside, page, false).0.len(), 0);

        let combined =
            NodeFilter::from_parts(&["button".into()], Some("press"), None, None, true, None);
        assert_eq!(query(&flat, &combined, page, false).0[0].index, 1);
    }

    #[test]
    fn query_counts_report_both_truncation_sources() {
        let t = tree(
            vec![
                node("/0", "button", "a", &[]),
                node("/0/0", "button", "b", &[]),
                node("/0/1", "button", "c", &[]),
            ],
            true,
        );
        let flat = flatten(&t);
        let filter = NodeFilter::from_parts(&["button".into()], None, None, None, false, None);
        let page = Page::new(Some(1), Some(1)).expect("page");
        let (hits, counts) = query(&flat, &filter, page, t.truncated);
        assert_eq!(
            hits.iter().map(|hit| hit.index).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            counts,
            ListCounts {
                visited: 3,
                matched: 3,
                returned: 1,
                offset: 1,
                truncated: true,
                scan_truncated: true,
                page_truncated: true,
            }
        );
        let page = Page::new(Some(2), Some(5)).expect("page");
        let (_, counts) = query(&flat, &filter, page, false);
        assert!(!counts.truncated);
        assert!(!counts.page_truncated);
    }

    fn window(handle: isize, pid: u32, app: &str, title: &str, focused: bool) -> WindowInfo {
        WindowInfo {
            handle,
            title: title.to_owned(),
            process_id: pid,
            app_name: app.to_owned(),
            bounds: WindowBounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            focused,
            minimized: false,
        }
    }

    #[test]
    fn window_inventory_filters_and_pages() {
        let windows = vec![
            window(1, 100, "TextEdit", "fixture-1.txt", false),
            window(2, 100, "TextEdit", "Untitled", true),
            window(3, 200, "Brave Origin", "Extensions", false),
        ];
        let empty = WindowFilter::default();
        assert!(empty.is_empty());
        let page = Page::new(None, None).expect("page");
        let (all, counts) = inventory(&windows, &empty, page);
        assert_eq!(all.len(), 3);
        assert_eq!((counts.visited, counts.matched, counts.returned), (3, 3, 3));

        let by_pid = WindowFilter {
            pid: Some(100),
            ..WindowFilter::default()
        };
        assert!(!by_pid.is_empty());
        assert_eq!(inventory(&windows, &by_pid, page).0.len(), 2);

        let by_app = WindowFilter {
            app: Some("textedit".into()),
            title: Some("FIXTURE".into()),
            ..WindowFilter::default()
        };
        let (hits, _) = inventory(&windows, &by_app, page);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].handle, 1);

        let focused = WindowFilter {
            focused: Some(true),
            minimized: Some(false),
            ..WindowFilter::default()
        };
        assert_eq!(inventory(&windows, &focused, page).0[0].handle, 2);

        let paged = Page::new(Some(1), Some(1)).expect("page");
        let (hits, counts) = inventory(&windows, &empty, paged);
        assert_eq!(hits[0].handle, 2);
        assert_eq!(counts.returned, 1);
        assert!(counts.page_truncated);
        assert!(counts.truncated);
    }
}
