//! Pure observation helpers shared by `tree --flat`, `query`, and the
//! `windows` inventory: flatten indices, node depth, filters, and paging with
//! `visited / matched / returned / truncated` counts. No mechanism calls live
//! here, so every rule is unit-tested without a desktop.

use crate::command::Expectation;
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

/// AX chrome-only vs page content, absorbed from MCU `classifyAxTree`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AxAvailability {
    Content,
    EmptyChrome,
    Empty,
}

impl AxAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::EmptyChrome => "empty-chrome",
            Self::Empty => "empty",
        }
    }
}

fn is_chrome_role(role: &str) -> bool {
    matches!(
        normalize_role(role).as_str(),
        "window" | "group" | "button" | "image" | "statictext" | "toolbar" | "menubar"
    )
}

fn is_page_content_role(role: &str) -> bool {
    let n = normalize_role(role);
    matches!(
        n.as_str(),
        "webarea"
            | "heading"
            | "textarea"
            | "textfield"
            | "link"
            | "list"
            | "cell"
            | "edit"
            | "document"
    )
}

/// Classify a flattened AX tree the way MCU does: chrome-only Chromium
/// windows are `empty-chrome`, not an empty page.
pub fn classify_ax_tree(tree: &A11yTree) -> AxAvailability {
    if tree.nodes.is_empty() {
        return AxAvailability::Empty;
    }
    let mut text_nodes = 0usize;
    let mut content_roles = 0usize;
    for node in &tree.nodes {
        let text = format!(
            "{} {} {}",
            node.name,
            node.text.as_deref().unwrap_or(""),
            node.identifier.as_deref().unwrap_or("")
        );
        if text.trim().len() > 0 {
            text_nodes += 1;
        }
        if is_page_content_role(&node.role)
            || (!is_chrome_role(&node.role) && !node.role.is_empty())
        {
            // AX-prefixed chrome roles are already chrome; anything else
            // with a page-like role counts as content.
            if is_page_content_role(&node.role) {
                content_roles += 1;
            } else if !is_chrome_role(&node.role) {
                content_roles += 1;
            }
        }
    }
    if content_roles == 0 && text_nodes <= 2 {
        AxAvailability::EmptyChrome
    } else {
        AxAvailability::Content
    }
}

pub fn chromium_app(app: &str) -> bool {
    let lower = app.to_ascii_lowercase();
    lower.contains("brave")
        || lower.contains("chrome")
        || lower.contains("chromium")
        || lower.contains("msedge")
        || lower.contains("edge")
}

/// Next actions when the tree is chrome-only: deepen query, never a
/// screenshot and never "install the extension first".
pub fn empty_chrome_next_actions(ax: AxAvailability, app: &str) -> Vec<String> {
    if ax != AxAvailability::EmptyChrome && ax != AxAvailability::Empty {
        return Vec::new();
    }
    let mut actions = vec![
        "empty-chrome is not an empty page; run query --window HANDLE --depth 12 --role WebArea then invoke by identity"
            .to_owned(),
    ];
    if chromium_app(app) || app.is_empty() {
        actions.push(
            "ordinary web control is AX query/invoke; do not steer to a browser extension"
                .to_owned(),
        );
    }
    actions
}

fn is_page_identity_role(normalized: &str) -> bool {
    normalized == "webarea" || normalized == "heading"
}

/// MCU wait/verify: with a title substring, Heading and WebArea alias each
/// other so a WebArea title can satisfy a Heading predicate.
pub fn roles_match_for_page_identity(have: &str, want: &str, title_predicate: bool) -> bool {
    let have_n = normalize_role(have);
    let want_n = normalize_role(want);
    if have_n == want_n {
        return true;
    }
    title_predicate && is_page_identity_role(&have_n) && is_page_identity_role(&want_n)
}

/// Honest page-JS knife: debugger Runtime.evaluate is the MCU backend;
/// this binary does not evaluate page JavaScript (and never MAIN-world
/// eval or new Function, which chatgpt.com CSP swallows).
pub fn page_js_backend() -> &'static str {
    "debugger-runtime-evaluate"
}

pub fn page_js_unsupported_reason() -> &'static str {
    "page JS is a second knife after AX WebArea query/invoke; this binary does not evaluate page JavaScript. Ordinary web control needs no browser extension. A future knife would use debugger Runtime.evaluate; MAIN-world eval or new Function is refused."
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

/// MCU-style stable window spelling `App#handle` (spaces in the app name
/// allowed). `--window` still accepts a bare integer.
pub fn window_stable_ref(window: &WindowInfo) -> String {
    let app = window.app_name.trim();
    let app = if app.is_empty() { "App" } else { app };
    format!("{app}#{}", window.handle)
}

/// Parse `--window` as `N` or `App#N`. Does not talk to the desktop; the
/// numeric handle is what later verbs already consume.
pub fn parse_window_token(raw: &str) -> Result<isize, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("--window needs a handle (N or App#N)".to_owned());
    }
    if let Ok(handle) = raw.parse::<isize>() {
        if handle == 0 {
            return Err("--window handle must be non-zero".to_owned());
        }
        return Ok(handle);
    }
    let Some((app, number)) = raw.rsplit_once('#') else {
        return Err(format!(
            "--window value {raw:?} is not a handle N or MCU App#N"
        ));
    };
    if app.trim().is_empty() {
        return Err("--window App#N needs a non-empty app name".to_owned());
    }
    let handle: isize = number
        .parse()
        .map_err(|_| format!("--window value {raw:?} is not a handle N or MCU App#N"))?;
    if handle == 0 {
        return Err("--window handle must be non-zero".to_owned());
    }
    Ok(handle)
}

/// Window inventory row: native fields plus MCU `ref`.
pub fn window_row_json(window: &WindowInfo) -> serde_json::Value {
    serde_json::json!({
        "handle": window.handle,
        "ref": window_stable_ref(window),
        "title": window.title,
        "process_id": window.process_id,
        "app_name": window.app_name,
        "bounds": window.bounds,
        "focused": window.focused,
        "minimized": window.minimized,
    })
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

// ---------------------------------------------------------------------------
// Targets and expectations (`invoke`, `verify`, `wait --expect`).
// ---------------------------------------------------------------------------

/// How one `invoke` / `verify` item names its node.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetSpec {
    pub node: Option<String>,
    pub index: Option<usize>,
    /// Case-insensitive substring of `name`; showing nodes only.
    pub name: Option<String>,
    /// Exact `identifier`; showing nodes only.
    pub identifier: Option<String>,
    /// Either role spelling; showing nodes only when it stands alone.
    pub role: Option<String>,
    /// The application's own focused control (resolved by the executor
    /// through the platform, then bound by id / role / identifier in the
    /// same tree read); only `role` may accompany it.
    pub focused: bool,
}

impl TargetSpec {
    pub fn from_expectation(expectation: &Expectation) -> Self {
        Self {
            node: expectation.node.clone(),
            index: expectation.index,
            name: expectation.name.clone(),
            identifier: expectation.identifier.clone(),
            role: expectation.role.clone(),
            focused: false,
        }
    }

    /// The target as the receipt names it.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "node": self.node,
            "index": self.index,
            "name": self.name,
            "identifier": self.identifier,
            "role": self.role,
            "focused": self.focused,
        })
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(node) = &self.node {
            parts.push(format!("node {node}"));
        }
        if let Some(index) = self.index {
            parts.push(format!("index {index}"));
        }
        if let Some(name) = &self.name {
            parts.push(format!("name contains {name:?}"));
        }
        if let Some(identifier) = &self.identifier {
            parts.push(format!("identifier {identifier:?}"));
        }
        if let Some(role) = &self.role {
            parts.push(format!("role {role:?}"));
        }
        if self.focused {
            parts.push("the focused control".to_owned());
        }
        parts.join(" and ")
    }
}

/// Why a target did not resolve to exactly one node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetError {
    /// The spec itself is malformed (no target, or an exact address mixed
    /// with a search).
    Invalid(String),
    Missing(String),
    Ambiguous {
        count: usize,
        scope: String,
    },
}

fn node_is_showing(node: &A11yNode) -> bool {
    node.states
        .iter()
        .any(|state| state.eq_ignore_ascii_case("showing") || state.eq_ignore_ascii_case("visible"))
}

/// Resolve a target to exactly one node of the flattened tree. `node` and
/// `index` are exact addresses (no showing requirement, no other field);
/// `name` / `identifier` / `role` are a search over showing nodes whose
/// every given field matches, and two or more hits are ambiguous.
pub fn resolve_target<'a>(
    flat: &'a [FlatNode<'a>],
    spec: &TargetSpec,
) -> Result<&'a FlatNode<'a>, TargetError> {
    if spec.focused {
        return Err(TargetError::Invalid(
            "--focused is resolved through the platform's focused control, not a tree search"
                .to_owned(),
        ));
    }
    let searching = spec.name.is_some() || spec.identifier.is_some() || spec.role.is_some();
    let exact = spec.node.is_some() as u8 + spec.index.is_some() as u8;
    if exact == 0 && !searching {
        return Err(TargetError::Invalid(
            "a target needs --node, --index, --name [--role], --identifier [--role], --role or --focused [--role]"
                .to_owned(),
        ));
    }
    if exact > 1 || (exact == 1 && searching) {
        return Err(TargetError::Invalid(
            "--node / --index are exact addresses; do not combine them with each other or with --name / --identifier / --role"
                .to_owned(),
        ));
    }
    if let Some(node_id) = &spec.node {
        return flat
            .iter()
            .find(|entry| &entry.node.id == node_id)
            .ok_or_else(|| TargetError::Missing(format!("no node with id {node_id}")));
    }
    if let Some(index) = spec.index {
        return flat
            .get(index)
            .ok_or_else(|| TargetError::Missing(format!("no node at flatten index {index}")));
    }
    let name = spec.name.as_deref().map(str::to_lowercase);
    let role = spec.role.as_deref().map(normalize_role);
    let title_predicate = name.is_some();
    let hits: Vec<&FlatNode<'_>> = flat
        .iter()
        .filter(|entry| {
            let node = entry.node;
            node_is_showing(node)
                && name
                    .as_deref()
                    .is_none_or(|needle| node.name.to_lowercase().contains(needle))
                && spec
                    .identifier
                    .as_deref()
                    .is_none_or(|wanted| node.identifier.as_deref() == Some(wanted))
                && role.as_deref().is_none_or(|wanted| {
                    roles_match_for_page_identity(&node.role, wanted, title_predicate)
                })
        })
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => Err(TargetError::Missing(format!(
            "no showing node with {}",
            spec.describe()
        ))),
        count => Err(TargetError::Ambiguous {
            count,
            scope: spec.describe(),
        }),
    }
}

/// A two-way control state as the tree reports it. `Unknown` means the
/// backend published neither direction — the fail-closed answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tri {
    True,
    False,
    Mixed,
    Unknown,
}

impl Tri {
    pub fn json(self) -> serde_json::Value {
        match self {
            Self::True => serde_json::Value::Bool(true),
            Self::False => serde_json::Value::Bool(false),
            Self::Mixed => serde_json::Value::String("mixed".into()),
            Self::Unknown => serde_json::Value::Null,
        }
    }

    pub fn as_bool(self) -> Option<bool> {
        match self {
            Self::True => Some(true),
            Self::False => Some(false),
            Self::Mixed | Self::Unknown => None,
        }
    }
}

fn has_state(node: &A11yNode, wanted: &str) -> bool {
    node.states
        .iter()
        .any(|state| state.eq_ignore_ascii_case(wanted))
}

pub fn checked_state(node: &A11yNode) -> Tri {
    if has_state(node, "checked") {
        Tri::True
    } else if has_state(node, "unchecked") {
        Tri::False
    } else if has_state(node, "mixed") || has_state(node, "indeterminate") {
        Tri::Mixed
    } else {
        Tri::Unknown
    }
}

pub fn expanded_state(node: &A11yNode) -> Tri {
    if has_state(node, "expanded") {
        Tri::True
    } else if has_state(node, "collapsed") {
        Tri::False
    } else {
        Tri::Unknown
    }
}

/// `focused` is known false only when the node is `focusable` and not
/// `focused`; a node that is neither has no readable focus state.
pub fn focused_state(node: &A11yNode) -> Tri {
    if has_state(node, "focused") {
        Tri::True
    } else if has_state(node, "focusable") {
        Tri::False
    } else {
        Tri::Unknown
    }
}

/// The readable state of one node, as receipts and `verify` report it.
pub fn node_state_json(node: &A11yNode) -> serde_json::Value {
    serde_json::json!({
        "id": node.id,
        "role": node.role,
        "name": node.name,
        "identifier": node.identifier,
        "text": node.text,
        "states": node.states,
        "checked": checked_state(node).json(),
        "expanded": expanded_state(node).json(),
        "focused": focused_state(node).json(),
    })
}

/// One compared field. `met == None` is an unobservable state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct Check {
    pub field: &'static str,
    pub expected: serde_json::Value,
    pub observed: serde_json::Value,
    pub met: Option<bool>,
}

fn tri_check(field: &'static str, expected: bool, observed: Tri) -> Check {
    Check {
        field,
        expected: serde_json::Value::Bool(expected),
        observed: observed.json(),
        met: match observed {
            Tri::Unknown => None,
            other => Some(other.as_bool() == Some(expected)),
        },
    }
}

/// Compare the states an expectation names against one node. A node with
/// no `text` compared to an expected value is a known mismatch (empty),
/// not unknown: `query` always reports `text`.
pub fn check_expectation(node: &A11yNode, expectation: &Expectation) -> Vec<Check> {
    let mut checks = Vec::new();
    if let Some(value) = &expectation.value {
        let observed = node.text.clone().unwrap_or_default();
        checks.push(Check {
            field: "value",
            expected: serde_json::Value::String(value.clone()),
            observed: serde_json::Value::String(observed.clone()),
            met: Some(&observed == value),
        });
    }
    if let Some(checked) = expectation.checked {
        checks.push(tri_check("checked", checked, checked_state(node)));
    }
    if let Some(expanded) = expectation.expanded {
        checks.push(tri_check("expanded", expanded, expanded_state(node)));
    }
    if let Some(focused) = expectation.focused {
        checks.push(tri_check("focused", focused, focused_state(node)));
    }
    checks
}

/// The node with this path id, if the tree still has it.
pub fn node_by_id<'a>(tree: &'a A11yTree, id: &str) -> Option<&'a A11yNode> {
    tree.nodes.iter().find(|node| node.id == id)
}

/// Whether anything observable differs between two walks of the same
/// window: node set, roles, names, text or states. Bounds are ignored (a
/// layout pass is not a semantic change).
pub fn tree_changed(before: &A11yTree, after: &A11yTree) -> bool {
    if before.nodes.len() != after.nodes.len() {
        return true;
    }
    before.nodes.iter().zip(after.nodes.iter()).any(|(a, b)| {
        a.id != b.id
            || a.role != b.role
            || a.name != b.name
            || a.text != b.text
            || a.states != b.states
    })
}

/// Decimal value of a node's text, for `increment` / `decrement` receipts.
pub fn numeric_text(node: &A11yNode) -> Option<f64> {
    node.text.as_deref()?.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// Background menus (`menu inspect` / `menu invoke`).
// ---------------------------------------------------------------------------

/// Deepest menu level a caller may name (0 = bar items only).
pub const MAX_MENU_DEPTH: u32 = 8;
/// Menu level when `--depth` is absent: the items of every top-level menu.
pub const DEFAULT_MENU_DEPTH: u32 = 1;
/// Largest menu walk budget a caller may name.
pub const MAX_MENU_NODE_BUDGET: usize = 5_000;
/// Menu walk budget when `--max-nodes` is absent.
pub const DEFAULT_MENU_NODE_BUDGET: usize = 1_000;

/// Typed `invalid_input` text for an out-of-range menu budget.
pub fn validate_menu_budget(depth: Option<u32>, max_nodes: Option<usize>) -> Result<(), String> {
    if let Some(depth) = depth
        && depth > MAX_MENU_DEPTH
    {
        return Err(format!(
            "--depth must be 0..={MAX_MENU_DEPTH} menu levels, got {depth}"
        ));
    }
    if let Some(max_nodes) = max_nodes
        && (max_nodes == 0 || max_nodes > MAX_MENU_NODE_BUDGET)
    {
        return Err(format!(
            "--max-nodes must be 1..={MAX_MENU_NODE_BUDGET}, got {max_nodes}"
        ));
    }
    Ok(())
}

/// The node depth of a menu level: the bar is node depth 0, a bar item 1,
/// its `AXMenu` 2, an item 3, a submenu 4, its item 5, ... so menu level
/// `n` (0 = bar items) is node depth `1 + 2n`.
pub fn menu_node_depth(menu_depth: u32) -> u32 {
    1 + 2 * menu_depth
}

fn is_menu_item_role(role: &str) -> bool {
    matches!(normalize_role(role).as_str(), "menubaritem" | "menuitem")
}

fn is_menu_role(role: &str) -> bool {
    normalize_role(role) == "menu"
}

/// One menu item as `menu inspect` lists it: its exact title path from the
/// bar, its menu level (0 = a bar item), state and whether it opens a
/// submenu. `id` is the node id in the menu walk (a separate id space
/// from the window tree).
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct MenuItem {
    pub index: usize,
    pub id: String,
    pub path: Vec<String>,
    pub title: String,
    pub depth: u32,
    pub enabled: bool,
    pub checked: bool,
    pub has_submenu: bool,
}

/// Flatten a menu walk into items in walk order. `AXMenu` containers and
/// the bar itself are structure, not items.
pub fn menu_items(tree: &A11yTree) -> Vec<MenuItem> {
    let by_id: std::collections::HashMap<&str, &A11yNode> = tree
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut has_menu_child: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for node in &tree.nodes {
        if is_menu_role(&node.role)
            && let Some(parent) = node.parent_id.as_deref()
        {
            has_menu_child.insert(parent);
        }
    }
    let mut items = Vec::new();
    for node in &tree.nodes {
        if !is_menu_item_role(&node.role) {
            continue;
        }
        // Titles of the item ancestors (skipping menus), nearest last.
        let mut path = vec![node.name.clone()];
        let mut depth = 0u32;
        let mut cursor = node.parent_id.as_deref();
        while let Some(parent_id) = cursor {
            let Some(parent) = by_id.get(parent_id) else {
                break;
            };
            if is_menu_item_role(&parent.role) {
                path.push(parent.name.clone());
                depth += 1;
            }
            cursor = parent.parent_id.as_deref();
        }
        path.reverse();
        items.push(MenuItem {
            index: items.len(),
            id: node.id.clone(),
            path,
            title: node.name.clone(),
            depth,
            enabled: !has_state(node, "disabled"),
            checked: has_state(node, "checked"),
            has_submenu: has_menu_child.contains(node.id.as_str()),
        });
    }
    items
}

/// The filter half of `menu inspect`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MenuFilter {
    /// Case-insensitive substring of the title, or the exact title.
    pub title: Option<String>,
    pub exact: bool,
    pub enabled: Option<bool>,
}

impl MenuFilter {
    pub fn matches(&self, item: &MenuItem) -> bool {
        if let Some(title) = &self.title {
            let hit = if self.exact {
                &item.title == title
            } else {
                item.title
                    .to_lowercase()
                    .contains(title.to_lowercase().as_str())
            };
            if !hit {
                return false;
            }
        }
        if self.enabled.is_some_and(|enabled| enabled != item.enabled) {
            return false;
        }
        true
    }
}

/// Filter and page menu items.
pub fn menu_query<'a>(
    items: &'a [MenuItem],
    filter: &MenuFilter,
    page: Page,
    scan_truncated: bool,
) -> (Vec<&'a MenuItem>, ListCounts) {
    let matched: Vec<&MenuItem> = items.iter().filter(|item| filter.matches(item)).collect();
    let (returned, page_truncated) = page.apply(&matched);
    let counts = ListCounts {
        visited: items.len(),
        matched: matched.len(),
        returned: returned.len(),
        offset: page.offset,
        truncated: scan_truncated || page_truncated,
        scan_truncated,
        page_truncated,
    };
    (returned.to_vec(), counts)
}

/// Parse `--path`: a JSON array of titles (`["File","Save…"]`) when it
/// starts with `[`, otherwise `/`-separated titles. At least a menu and
/// one item, none empty.
pub fn parse_menu_path(raw: &str) -> Result<Vec<String>, String> {
    let segments: Vec<String> = if raw.trim_start().starts_with('[') {
        serde_json::from_str(raw)
            .map_err(|error| format!("--path JSON must be an array of titles: {error}"))?
    } else {
        raw.split('/').map(str::to_owned).collect()
    };
    if segments.len() < 2 {
        return Err(
            "--path needs a menu title and at least one item title (File/Save or [\"File\",\"Save\"])"
                .to_owned(),
        );
    }
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err("--path has an empty title segment".to_owned());
    }
    Ok(segments)
}

// ---------------------------------------------------------------------------
// Observation stream (`observe`): poll-diff over two bounded walks.
// ---------------------------------------------------------------------------

/// Longest observation window.
pub const MAX_OBSERVE_DURATION_MS: u64 = 120_000;
/// Most events one `observe` may emit.
pub const MAX_OBSERVE_EVENTS: usize = 5_000;
/// Events emitted when `--max-events` is absent.
pub const DEFAULT_OBSERVE_EVENTS: usize = 200;
/// Shortest and default poll interval.
pub const MIN_OBSERVE_INTERVAL_MS: u64 = 20;
pub const DEFAULT_OBSERVE_INTERVAL_MS: u64 = 50;
/// The notification vocabulary, in the spelling the reply uses.
pub const OBSERVE_NOTIFICATIONS: [&str; 6] = [
    "ValueChanged",
    "TitleChanged",
    "StateChanged",
    "FocusChanged",
    "Created",
    "Destroyed",
];

/// Typed `invalid_input` text for out-of-range observe bounds.
pub fn validate_observe(
    duration_ms: u64,
    max_events: Option<usize>,
    interval_ms: Option<u64>,
) -> Result<(), String> {
    if duration_ms == 0 || duration_ms > MAX_OBSERVE_DURATION_MS {
        return Err(format!(
            "--duration must be within 1..={MAX_OBSERVE_DURATION_MS} ms, got {duration_ms} ms"
        ));
    }
    if let Some(max_events) = max_events
        && (max_events == 0 || max_events > MAX_OBSERVE_EVENTS)
    {
        return Err(format!(
            "--max-events must be 1..={MAX_OBSERVE_EVENTS}, got {max_events}"
        ));
    }
    if let Some(interval_ms) = interval_ms
        && (interval_ms < MIN_OBSERVE_INTERVAL_MS || interval_ms > duration_ms)
    {
        return Err(format!(
            "--interval-ms must be {MIN_OBSERVE_INTERVAL_MS}..=duration, got {interval_ms}"
        ));
    }
    Ok(())
}

/// Parse `--notification A,B`: each name matches the vocabulary case-
/// insensitively, with an `AX` prefix and the AX spellings
/// (`AXFocusedUIElementChanged`, `AXUIElementDestroyed`) accepted.
pub fn parse_notifications(raw: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for item in raw
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let key = normalize_role(item);
        let hit = match key.as_str() {
            "focuseduielementchanged" | "focused" => Some("FocusChanged"),
            "uielementdestroyed" => Some("Destroyed"),
            _ => OBSERVE_NOTIFICATIONS
                .iter()
                .copied()
                .find(|name| normalize_role(name) == key),
        };
        match hit {
            Some(name) if !out.iter().any(|have| have == name) => out.push(name.to_owned()),
            Some(_) => {}
            None => {
                return Err(format!(
                    "unknown notification {item:?}; expected one of {}",
                    OBSERVE_NOTIFICATIONS.join(", ")
                ));
            }
        }
    }
    Ok(out)
}

/// One observed change between two walks of the same window.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ObserveEvent {
    pub notification: &'static str,
    pub node: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<&'static str>,
    pub before: serde_json::Value,
    pub after: serde_json::Value,
}

fn event_node(node: &A11yNode) -> serde_json::Value {
    serde_json::json!({
        "id": node.id,
        "role": node.role,
        "name": node.name,
        "identifier": node.identifier,
    })
}

fn states_without_focus(node: &A11yNode) -> Vec<&str> {
    node.states
        .iter()
        .map(String::as_str)
        .filter(|state| {
            !state.eq_ignore_ascii_case("focused") && !state.eq_ignore_ascii_case("focusable")
        })
        .collect()
}

/// Every semantic difference between `before` and `after`, in `before`
/// walk order for nodes present in both or gone, then `after` walk order
/// for new nodes. Bounds are ignored (layout is not a semantic change).
pub fn diff_events(before: &A11yTree, after: &A11yTree) -> Vec<ObserveEvent> {
    let after_by_id: std::collections::HashMap<&str, &A11yNode> = after
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let before_ids: std::collections::HashSet<&str> =
        before.nodes.iter().map(|node| node.id.as_str()).collect();
    let mut events = Vec::new();
    for was in &before.nodes {
        let Some(now) = after_by_id.get(was.id.as_str()) else {
            events.push(ObserveEvent {
                notification: "Destroyed",
                node: event_node(was),
                field: None,
                before: node_state_json(was),
                after: serde_json::Value::Null,
            });
            continue;
        };
        if was.role != now.role {
            // A different control now sits at this path: report it as a
            // replacement rather than a value change of the old one.
            events.push(ObserveEvent {
                notification: "Destroyed",
                node: event_node(was),
                field: None,
                before: node_state_json(was),
                after: serde_json::Value::Null,
            });
            events.push(ObserveEvent {
                notification: "Created",
                node: event_node(now),
                field: None,
                before: serde_json::Value::Null,
                after: node_state_json(now),
            });
            continue;
        }
        if was.text != now.text {
            events.push(ObserveEvent {
                notification: "ValueChanged",
                node: event_node(now),
                field: Some("text"),
                before: serde_json::json!(was.text),
                after: serde_json::json!(now.text),
            });
        }
        if was.name != now.name {
            events.push(ObserveEvent {
                notification: "TitleChanged",
                node: event_node(now),
                field: Some("name"),
                before: serde_json::json!(was.name),
                after: serde_json::json!(now.name),
            });
        }
        if focused_state(was) != focused_state(now) {
            events.push(ObserveEvent {
                notification: "FocusChanged",
                node: event_node(now),
                field: Some("focused"),
                before: focused_state(was).json(),
                after: focused_state(now).json(),
            });
        }
        if states_without_focus(was) != states_without_focus(now) {
            events.push(ObserveEvent {
                notification: "StateChanged",
                node: event_node(now),
                field: Some("states"),
                before: serde_json::json!(states_without_focus(was)),
                after: serde_json::json!(states_without_focus(now)),
            });
        }
    }
    for now in &after.nodes {
        if !before_ids.contains(now.id.as_str()) {
            events.push(ObserveEvent {
                notification: "Created",
                node: event_node(now),
                field: None,
                before: serde_json::Value::Null,
                after: node_state_json(now),
            });
        }
    }
    events
}

// ---------------------------------------------------------------------------
// Value previews (`focused --max-value-bytes`).
// ---------------------------------------------------------------------------

/// Preview bytes when `--max-value-bytes` is absent.
pub const DEFAULT_MAX_VALUE_BYTES: usize = 4_096;
/// Largest preview a caller may name.
pub const MAX_VALUE_BYTES_CEILING: usize = 1_048_576;

/// Typed `invalid_input` text for an out-of-range preview bound.
pub fn validate_max_value_bytes(max_value_bytes: Option<usize>) -> Result<(), String> {
    if let Some(bytes) = max_value_bytes
        && bytes > MAX_VALUE_BYTES_CEILING
    {
        return Err(format!(
            "--max-value-bytes must be 0..={MAX_VALUE_BYTES_CEILING}, got {bytes}"
        ));
    }
    Ok(())
}

/// The first `max_bytes` of `text` at a char boundary, and whether it was
/// cut. `0` keeps only the byte count (empty preview, cut when non-empty).
pub fn preview_value(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let mut cut = max_bytes;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (text[..cut].to_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mechanism::A11yBounds;
    use crate::mechanism::window_enumerate::WindowBounds;

    fn showing(mut node: A11yNode, extra: &[&str]) -> A11yNode {
        node.states.push("showing".into());
        node.states
            .extend(extra.iter().map(|state| (*state).to_owned()));
        node
    }

    #[test]
    fn targets_resolve_exactly_one_showing_node_or_fail_typed() {
        let mut twin_a = showing(node("/0/1", "button", "Fixture Twin", &["click"]), &[]);
        twin_a.identifier = Some("fixture-twin-a".into());
        let mut twin_b = showing(node("/0/2", "button", "Fixture Twin", &["click"]), &[]);
        twin_b.identifier = Some("fixture-twin-b".into());
        let hidden = node("/0/3", "button", "Fixture Hidden", &["click"]);
        let check = showing(
            node("/0/4", "check-box", "Fixture Check", &["click"]),
            &["unchecked"],
        );
        let t = tree(
            vec![
                node("/0", "window", "w", &[]),
                twin_a,
                twin_b,
                hidden,
                check,
            ],
            false,
        );
        let flat = flatten(&t);

        let by_node = TargetSpec {
            node: Some("/0/4".into()),
            ..TargetSpec::default()
        };
        assert_eq!(resolve_target(&flat, &by_node).unwrap().index, 4);
        let by_index = TargetSpec {
            index: Some(2),
            ..TargetSpec::default()
        };
        assert_eq!(resolve_target(&flat, &by_index).unwrap().node.id, "/0/2");
        let by_identifier = TargetSpec {
            identifier: Some("fixture-twin-b".into()),
            ..TargetSpec::default()
        };
        assert_eq!(
            resolve_target(&flat, &by_identifier).unwrap().node.id,
            "/0/2"
        );
        let by_name_role = TargetSpec {
            name: Some("fixture".into()),
            role: Some("AXCheckBox".into()),
            ..TargetSpec::default()
        };
        assert_eq!(
            resolve_target(&flat, &by_name_role).unwrap().node.id,
            "/0/4"
        );

        let ambiguous = TargetSpec {
            name: Some("Fixture Twin".into()),
            ..TargetSpec::default()
        };
        assert!(matches!(
            resolve_target(&flat, &ambiguous),
            Err(TargetError::Ambiguous { count: 2, .. })
        ));
        let hidden = TargetSpec {
            name: Some("Fixture Hidden".into()),
            ..TargetSpec::default()
        };
        assert!(matches!(
            resolve_target(&flat, &hidden),
            Err(TargetError::Missing(_))
        ));
        let missing_index = TargetSpec {
            index: Some(99),
            ..TargetSpec::default()
        };
        assert!(matches!(
            resolve_target(&flat, &missing_index),
            Err(TargetError::Missing(_))
        ));
        assert!(matches!(
            resolve_target(&flat, &TargetSpec::default()),
            Err(TargetError::Invalid(_))
        ));
        let mixed = TargetSpec {
            node: Some("/0/4".into()),
            name: Some("x".into()),
            ..TargetSpec::default()
        };
        assert!(matches!(
            resolve_target(&flat, &mixed),
            Err(TargetError::Invalid(_))
        ));
    }

    fn menu_node(
        id: &str,
        parent: Option<&str>,
        role: &str,
        name: &str,
        states: &[&str],
    ) -> A11yNode {
        let mut node = node(id, role, name, &["click"]);
        node.parent_id = parent.map(str::to_owned);
        node.states = states.iter().map(|state| (*state).to_owned()).collect();
        node
    }

    #[test]
    fn menu_items_carry_paths_levels_states_and_submenus() {
        let t = tree(
            vec![
                menu_node("/0", None, "menu-bar", "", &["enabled"]),
                menu_node("/0/0", Some("/0"), "menu-bar-item", "File", &["enabled"]),
                menu_node("/0/0/0", Some("/0/0"), "menu", "File", &["enabled"]),
                menu_node(
                    "/0/0/0/0",
                    Some("/0/0/0"),
                    "menu-item",
                    "Do Thing",
                    &["enabled"],
                ),
                menu_node(
                    "/0/0/0/1",
                    Some("/0/0/0"),
                    "menu-item",
                    "Disabled Thing",
                    &["disabled"],
                ),
                menu_node(
                    "/0/0/0/2",
                    Some("/0/0/0"),
                    "menu-item",
                    "More",
                    &["enabled"],
                ),
                menu_node("/0/0/0/2/0", Some("/0/0/0/2"), "menu", "More", &["enabled"]),
                menu_node(
                    "/0/0/0/2/0/0",
                    Some("/0/0/0/2/0"),
                    "menu-item",
                    "Deeper",
                    &["enabled", "checked"],
                ),
            ],
            true,
        );
        let items = menu_items(&t);
        let paths: Vec<String> = items.iter().map(|item| item.path.join("/")).collect();
        assert_eq!(
            paths,
            vec![
                "File",
                "File/Do Thing",
                "File/Disabled Thing",
                "File/More",
                "File/More/Deeper"
            ]
        );
        assert_eq!(items[0].depth, 0);
        assert!(items[0].has_submenu);
        assert_eq!(items[1].depth, 1);
        assert!(!items[1].has_submenu && items[1].enabled && !items[1].checked);
        assert!(!items[2].enabled);
        assert!(items[3].has_submenu);
        assert_eq!(items[4].depth, 2);
        assert!(items[4].checked);
        assert_eq!(menu_node_depth(0), 1);
        assert_eq!(menu_node_depth(2), 5);

        let exact = MenuFilter {
            title: Some("Do Thing".into()),
            exact: true,
            enabled: None,
        };
        let (hits, counts) = menu_query(&items, &exact, Page::new(None, None).unwrap(), true);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "/0/0/0/0");
        assert!(counts.truncated && counts.scan_truncated && !counts.page_truncated);
        let disabled = MenuFilter {
            title: Some("thing".into()),
            exact: false,
            enabled: Some(false),
        };
        let (hits, counts) =
            menu_query(&items, &disabled, Page::new(None, Some(1)).unwrap(), false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Disabled Thing");
        assert_eq!((counts.visited, counts.matched, counts.returned), (5, 1, 1));
        assert!(!counts.truncated);

        assert_eq!(
            parse_menu_path("File/Do Thing").unwrap(),
            vec!["File", "Do Thing"]
        );
        assert_eq!(
            parse_menu_path(r#"["File","Open Quickly…"]"#).unwrap(),
            vec!["File", "Open Quickly…"]
        );
        assert!(parse_menu_path("File").is_err());
        assert!(parse_menu_path("File//X").is_err());
        assert!(validate_menu_budget(Some(9), None).is_err());
        assert!(validate_menu_budget(None, Some(0)).is_err());
        assert!(validate_menu_budget(Some(8), Some(5000)).is_ok());
    }

    #[test]
    fn observe_diff_names_every_change_and_filters_parse() {
        let mut field = showing(node("/0/1", "text-field", "", &[]), &["focusable"]);
        field.text = Some("seed".into());
        let label = showing(node("/0/2", "static-text", "menu idle", &[]), &[]);
        let gone = showing(node("/0/3", "button", "Gone", &["click"]), &[]);
        let before = tree(
            vec![
                node("/0", "window", "w", &[]),
                field.clone(),
                label.clone(),
                gone,
            ],
            false,
        );

        let mut field_after = field.clone();
        field_after.text = Some("written".into());
        field_after.states.push("focused".into());
        let mut label_after = label.clone();
        label_after.name = "did thing 1".into();
        let mut checked = showing(node("/0/4", "check-box", "New", &["click"]), &["checked"]);
        checked.identifier = Some("new-box".into());
        let after = tree(
            vec![
                node("/0", "window", "w", &[]),
                field_after,
                label_after,
                checked,
            ],
            false,
        );
        let events = diff_events(&before, &after);
        let kinds: Vec<(&str, &str)> = events
            .iter()
            .map(|event| (event.notification, event.node["id"].as_str().unwrap_or("")))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("ValueChanged", "/0/1"),
                ("FocusChanged", "/0/1"),
                ("TitleChanged", "/0/2"),
                ("Destroyed", "/0/3"),
                ("Created", "/0/4"),
            ]
        );
        assert_eq!(events[0].before, serde_json::json!("seed"));
        assert_eq!(events[0].after, serde_json::json!("written"));
        assert_eq!(events[1].before, serde_json::json!(false));
        assert_eq!(events[1].after, serde_json::json!(true));
        assert!(diff_events(&after, &after).is_empty());

        assert_eq!(
            parse_notifications("valuechanged, AXFocusedUIElementChanged,ValueChanged").unwrap(),
            vec!["ValueChanged", "FocusChanged"]
        );
        assert!(parse_notifications("Moved").is_err());
        assert!(validate_observe(0, None, None).is_err());
        assert!(validate_observe(1000, Some(0), None).is_err());
        assert!(validate_observe(1000, None, Some(5)).is_err());
        assert!(validate_observe(1000, Some(50), Some(100)).is_ok());

        assert_eq!(preview_value("héllo", 2), ("h".to_owned(), true));
        assert_eq!(preview_value("héllo", 3), ("hé".to_owned(), true));
        assert_eq!(preview_value("héllo", 0), (String::new(), true));
        assert_eq!(preview_value("", 0), (String::new(), false));
        assert_eq!(preview_value("abc", 3), ("abc".to_owned(), false));
    }

    #[test]
    fn expectations_fail_closed_on_unobservable_state() {
        let mut field = showing(node("/0/1", "text-field", "", &[]), &["focusable"]);
        field.text = Some("written".into());
        let check = showing(
            node("/0/2", "check-box", "Fixture Check", &["click"]),
            &["checked"],
        );
        let button = showing(node("/0/3", "button", "Fixture Press", &["click"]), &[]);

        let value = Expectation {
            value: Some("written".into()),
            ..Expectation::default()
        };
        let checks = check_expectation(&field, &value);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].met, Some(true));
        let wrong = Expectation {
            value: Some("other".into()),
            ..Expectation::default()
        };
        assert_eq!(check_expectation(&field, &wrong)[0].met, Some(false));
        let focused = Expectation {
            focused: Some(false),
            ..Expectation::default()
        };
        assert_eq!(check_expectation(&field, &focused)[0].met, Some(true));

        let checked = Expectation {
            checked: Some(true),
            ..Expectation::default()
        };
        assert_eq!(check_expectation(&check, &checked)[0].met, Some(true));
        let unchecked = Expectation {
            checked: Some(false),
            ..Expectation::default()
        };
        assert_eq!(check_expectation(&check, &unchecked)[0].met, Some(false));
        // A button publishes no checked / expanded / focused state: unknown,
        // never "met".
        assert_eq!(check_expectation(&button, &checked)[0].met, None);
        let expanded = Expectation {
            expanded: Some(true),
            ..Expectation::default()
        };
        assert_eq!(check_expectation(&button, &expanded)[0].met, None);
        assert_eq!(check_expectation(&button, &focused)[0].met, None);
        assert_eq!(checked_state(&button), Tri::Unknown);
        assert_eq!(focused_state(&field), Tri::False);

        let state = node_state_json(&check);
        assert_eq!(state["checked"], serde_json::json!(true));
        assert_eq!(state["expanded"], serde_json::Value::Null);
    }

    #[test]
    fn tree_change_ignores_bounds_and_sees_text_and_states() {
        let before = tree(
            vec![
                node("/0", "window", "w", &[]),
                node("/0/1", "static-text", "", &[]),
            ],
            false,
        );
        let mut after = before.clone();
        assert!(!tree_changed(&before, &after));
        after.nodes[1].bounds.x += 5;
        assert!(!tree_changed(&before, &after));
        after.nodes[1].text = Some("pressed 1".into());
        assert!(tree_changed(&before, &after));
        let mut gone = before.clone();
        gone.nodes.pop();
        assert!(tree_changed(&before, &gone));
        assert_eq!(
            node_by_id(&before, "/0/1").map(|n| n.role.as_str()),
            Some("static-text")
        );
        let mut stepper = node("/0/2", "incrementor", "", &["increment"]);
        stepper.text = Some("4".into());
        assert_eq!(numeric_text(&stepper), Some(4.0));
    }

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

    #[test]
    fn window_ref_is_mcu_app_hash_handle() {
        let brave = window(14278, 1, "Brave Origin", "Exact Reply", true);
        assert_eq!(window_stable_ref(&brave), "Brave Origin#14278");
        let row = window_row_json(&brave);
        assert_eq!(row["ref"], "Brave Origin#14278");
        assert_eq!(row["handle"], 14278);
        assert_eq!(parse_window_token("14278").unwrap(), 14278);
        assert_eq!(parse_window_token("Brave Origin#14278").unwrap(), 14278);
        assert_eq!(parse_window_token("  TextEdit#9  ").unwrap(), 9);
        assert!(parse_window_token("0").is_err());
        assert!(parse_window_token("#9").is_err());
        assert!(parse_window_token("Nope").is_err());
    }

    #[test]
    fn empty_chrome_next_action_is_deeper_query_not_screenshot_or_extension() {
        let chrome = tree(
            vec![
                node("/0", "AXWindow", "Brave Origin", &["AXRaise"]),
                node("/0/0", "AXGroup", "", &[]),
                node("/0/1", "AXButton", "reload", &["AXPress"]),
            ],
            false,
        );
        assert_eq!(classify_ax_tree(&chrome), AxAvailability::EmptyChrome);
        let next = empty_chrome_next_actions(AxAvailability::EmptyChrome, "Brave Origin");
        let joined = next.join(" ");
        assert!(joined.contains("query"));
        assert!(joined.contains("WebArea"));
        assert!(!joined.to_ascii_lowercase().contains("screenshot"));
        assert!(!joined.contains("brave://extensions"));
        assert!(!joined.contains("debug-read"));
        let content = tree(
            vec![
                node("/0", "AXWindow", "w", &[]),
                showing(node("/0/1", "AXWebArea", "Nepal floods latest", &[]), &[]),
            ],
            false,
        );
        assert_eq!(classify_ax_tree(&content), AxAvailability::Content);
        assert!(empty_chrome_next_actions(AxAvailability::Content, "Brave Origin").is_empty());
        let payload = serde_json::json!({
            "ax": classify_ax_tree(&chrome).as_str(),
            "next_actions": empty_chrome_next_actions(
                classify_ax_tree(&chrome),
                "Brave Origin",
            ),
        });
        assert_eq!(payload["ax"], "empty-chrome");
        let next = payload["next_actions"][0].as_str().unwrap_or_default();
        assert!(next.contains("query") && next.contains("WebArea"));
        assert!(!next.to_ascii_lowercase().contains("screenshot"));
    }

    #[test]
    fn heading_title_includes_matches_webarea_title() {
        let web = showing(
            node(
                "/0/1",
                "AXWebArea",
                "Nepal floods latest: Head teacher",
                &[],
            ),
            &[],
        );
        let heading = showing(node("/0/2", "AXHeading", "Live Reporting", &[]), &[]);
        let button = showing(
            node("/0/3", "AXButton", "Nepal floods latest", &["press"]),
            &[],
        );
        let t = tree(
            vec![node("/0", "AXWindow", "w", &[]), web, heading, button],
            false,
        );
        let flat = flatten(&t);
        let heading_pred = TargetSpec {
            role: Some("AXHeading".into()),
            name: Some("Nepal".into()),
            ..TargetSpec::default()
        };
        let hit = resolve_target(&flat, &heading_pred).expect("WebArea title aliases Heading");
        assert_eq!(normalize_role(&hit.node.role), "webarea");
        let web_pred = TargetSpec {
            role: Some("AXWebArea".into()),
            name: Some("Nepal".into()),
            ..TargetSpec::default()
        };
        assert_eq!(
            resolve_target(&flat, &web_pred).unwrap().node.role,
            "AXWebArea"
        );
        let no_title = TargetSpec {
            role: Some("AXHeading".into()),
            ..TargetSpec::default()
        };
        assert_eq!(
            resolve_target(&flat, &no_title).unwrap().node.role,
            "AXHeading"
        );
        let button_pred = TargetSpec {
            role: Some("AXButton".into()),
            name: Some("Nepal".into()),
            ..TargetSpec::default()
        };
        assert_eq!(
            resolve_target(&flat, &button_pred).unwrap().node.role,
            "AXButton"
        );
        let from_wait: crate::command::Expectation =
            serde_json::from_str(r#"{"role":"AXHeading","titleIncludes":"Nepal"}"#)
                .expect("wait --expect titleIncludes");
        let wait_hit = resolve_target(&flat, &TargetSpec::from_expectation(&from_wait))
            .expect("shipped wait matcher aliases WebArea title");
        assert_eq!(normalize_role(&wait_hit.node.role), "webarea");
    }

    #[test]
    fn page_js_knife_is_debugger_evaluate_not_eval() {
        assert_eq!(page_js_backend(), "debugger-runtime-evaluate");
        assert!(page_js_unsupported_reason().contains("second knife"));
        assert!(page_js_unsupported_reason().contains("no browser extension"));
        assert!(!page_js_unsupported_reason().contains("eval("));
        assert!(!include_str!("command.rs").contains("eval("));
        assert!(!include_str!("executor.rs").contains("eval("));
    }
}
