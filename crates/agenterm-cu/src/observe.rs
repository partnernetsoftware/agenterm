//! Pure observation helpers shared by `tree --flat`, `query`, and the
//! `windows` inventory: flatten indices, node depth, filters, and paging with
//! `visited / matched / returned / truncated` counts. No mechanism calls live
//! here, so every rule is unit-tested without a desktop.

use std::collections::BTreeMap;

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
/// contract's kebab-case role and vice versa. Exact toolkit synonyms also
/// share one canonical role: GTK/AT-SPI reports the same button as either
/// `button` or `push button` across distributions, and both become `button`.
pub fn normalize_role(raw: &str) -> String {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix("AX").unwrap_or(trimmed);
    let compact: String = stripped
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    match compact.as_str() {
        "pushbutton" => "button".to_owned(),
        _ => compact,
    }
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
        if !text.trim().is_empty() {
            text_nodes += 1;
        }
        // AX-prefixed chrome roles are already chrome; anything else with a
        // page-like or nonempty non-chrome role counts as content.
        if is_page_content_role(&node.role)
            || (!is_chrome_role(&node.role) && !node.role.is_empty())
        {
            content_roles += 1;
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
        "empty-chrome is not an empty page; run query --window HANDLE --depth 12 --role WebArea then invoke by identity, or unlock --window HANDLE"
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
    "page JS is a second knife after AX WebArea query/invoke. This binary uses debugger Runtime.evaluate over CDP when --remote-debugging-port answers. Ordinary web control needs no browser extension. MAIN-world Function constructor is refused."
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

/// One MCU selector segment: `Role[idx]`, `Role@title`, `*@title`, `#desc`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectorSegment {
    pub role: Option<String>,
    pub index: usize,
    pub title: Option<String>,
    pub description: Option<String>,
}

const MAX_SELECTOR_SEGMENTS: usize = 32;

/// Parse MCU `tree`/`query`/`invoke` selector grammar. Split on `/`.
pub fn parse_selector(raw: &str) -> Result<Vec<SelectorSegment>, String> {
    let segments: Vec<&str> = raw
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if segments.is_empty() {
        return Err("selector is empty".to_owned());
    }
    if segments.len() > MAX_SELECTOR_SEGMENTS {
        return Err(format!("selector exceeds {MAX_SELECTOR_SEGMENTS} segments"));
    }
    segments.into_iter().map(parse_selector_segment).collect()
}

fn parse_selector_segment(segment: &str) -> Result<SelectorSegment, String> {
    if let Some(description) = segment.strip_prefix('#') {
        if description.is_empty() {
            return Err("description selector cannot be empty".to_owned());
        }
        return Ok(SelectorSegment {
            role: None,
            index: 0,
            title: None,
            description: Some(description.to_owned()),
        });
    }
    let at = segment.find('@');
    let bracket = segment.find('[');
    let role_end = match (bracket, at) {
        (Some(b), Some(a)) => b.min(a),
        (Some(b), None) => b,
        (None, Some(a)) => a,
        (None, None) => segment.len(),
    };
    let role_raw = segment[..role_end].trim();
    let role = if role_raw == "*" {
        None
    } else if role_raw.is_empty()
        || !role_raw
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
        || !role_raw
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch == ' ' || ch == '_' || ch == '-')
    {
        return Err(format!("invalid selector segment: {segment}"));
    } else {
        Some(role_raw.to_owned())
    };
    let rest = &segment[role_end..];
    let (index, title) = if let Some(after_bracket) = rest.strip_prefix('[') {
        let close = after_bracket
            .find(']')
            .ok_or_else(|| format!("invalid selector segment: {segment}"))?;
        let number = &after_bracket[..close];
        let index: usize = number
            .parse()
            .map_err(|_| format!("invalid selector segment: {segment}"))?;
        if index > 100_000 {
            return Err("selector index is too large".to_owned());
        }
        let tail = after_bracket[close + 1..].trim_start();
        let title = tail.strip_prefix('@').map(str::to_owned);
        if !tail.is_empty() && title.is_none() {
            return Err(format!("invalid selector segment: {segment}"));
        }
        (index, title)
    } else if let Some(title) = rest.strip_prefix('@') {
        (0, Some(title.to_owned()))
    } else if rest.is_empty() {
        (0, None)
    } else {
        return Err(format!("invalid selector segment: {segment}"));
    };
    Ok(SelectorSegment {
        role,
        index,
        title,
        description: None,
    })
}

fn selector_role_matches(have: &str, want: &str) -> bool {
    normalize_role(have) == normalize_role(want)
}

fn selector_child_matches(node: &A11yNode, seg: &SelectorSegment) -> bool {
    if let Some(want) = &seg.role
        && !selector_role_matches(&node.role, want)
    {
        return false;
    }
    if let Some(title) = &seg.title {
        let in_name = node.name.contains(title);
        let in_id = node
            .identifier
            .as_deref()
            .is_some_and(|id| id.contains(title));
        if !in_name && !in_id {
            return false;
        }
    }
    if let Some(description) = &seg.description {
        let in_id = node
            .identifier
            .as_deref()
            .is_some_and(|id| id.contains(description));
        if !in_id && !node.name.contains(description) {
            return false;
        }
    }
    true
}

fn children_of<'a>(tree: &'a A11yTree, parent_id: &str) -> Vec<&'a A11yNode> {
    tree.nodes
        .iter()
        .filter(|node| node.parent_id.as_deref() == Some(parent_id))
        .collect()
}

/// Walk MCU selector from `tree.root_id`. Each segment indexes matching
/// children (same-role sibling), not the flatten list.
pub fn walk_selector<'a>(
    tree: &'a A11yTree,
    selector: &str,
) -> Result<Option<&'a A11yNode>, String> {
    let path = parse_selector(selector)?;
    let Some(mut current) = tree.nodes.iter().find(|node| node.id == tree.root_id) else {
        return Ok(None);
    };
    for segment in &path {
        let matched: Vec<&A11yNode> = children_of(tree, &current.id)
            .into_iter()
            .filter(|kid| selector_child_matches(kid, segment))
            .collect();
        let Some(next) = matched.get(segment.index).copied() else {
            return Ok(None);
        };
        current = next;
    }
    Ok(Some(current))
}

fn subtree_ids(tree: &A11yTree, root_id: &str) -> Vec<String> {
    let mut ids = vec![root_id.to_owned()];
    let mut i = 0;
    while i < ids.len() {
        let parent = ids[i].clone();
        for child in children_of(tree, &parent) {
            if !ids.iter().any(|id| id == &child.id) {
                ids.push(child.id.clone());
            }
        }
        i += 1;
    }
    ids
}

/// Restrict a flatten list to the MCU selector hit and its descendants.
/// Parse errors are `Err`; a miss is an empty vec.
pub fn query_selector_scope<'a>(
    tree: &'a A11yTree,
    flat: &'a [FlatNode<'a>],
    selector: &str,
) -> Result<Vec<&'a FlatNode<'a>>, String> {
    let Some(hit) = walk_selector(tree, selector)? else {
        return Ok(Vec::new());
    };
    let ids = subtree_ids(tree, &hit.id);
    Ok(flat
        .iter()
        .filter(|entry| ids.iter().any(|id| id == &entry.node.id))
        .collect())
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

/// MCU `invoke set-selection` value `start:length` → ABI `(start, end)`.
pub fn parse_text_selection(raw: &str) -> Result<(i32, i32), String> {
    let Some((start, length)) = raw.split_once(':') else {
        return Err("set-selection value must be <start>:<length>".into());
    };
    let start: i32 = start
        .parse()
        .map_err(|_| "set-selection start is not an integer".to_owned())?;
    let length: i32 = length
        .parse()
        .map_err(|_| "set-selection length is not an integer".to_owned())?;
    if start < 0 || length < 0 || start > 10_000_000 || length > 10_000_000 {
        return Err("set-selection range must be within 0..10000000".into());
    }
    let end = start.saturating_add(length);
    if end > 10_000_000 {
        return Err("set-selection range must be within 0..10000000".into());
    }
    Ok((start, end))
}

/// The comparable core of an app or window-title app segment: lowercase,
/// no `.exe` suffix, ASCII alphanumerics only. `None` when fewer than four
/// characters survive, which is too little to match anything on.
fn app_token(raw: &str) -> Option<String> {
    let lower = raw.trim().to_ascii_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    let token: String = base.chars().filter(char::is_ascii_alphanumeric).collect();
    (token.len() >= 4).then_some(token)
}

/// The ` - ` that joins a Chromium window title's segments.
const SEPARATOR: &str = " - ";

/// Every byte offset in `title` where [`SEPARATOR`] starts, left to right.
/// Overlapping runs (` - - `) are all reported, which `str::match_indices`
/// would not do; every offset is a char boundary because the first byte is
/// ASCII space.
fn separator_offsets(title: &str) -> Vec<usize> {
    let bytes = title.as_bytes();
    (0..bytes.len().saturating_sub(SEPARATOR.len() - 1))
        .filter(|&i| &bytes[i..i + SEPARATOR.len()] == SEPARATOR.as_bytes())
        .collect()
}

/// Chromium-family AX/window titles look like
/// `App Store Connect - Brave Origin - profile-a`. The profile is the last
/// ` - ` segment; the one before it names the browser. Not a CDP profile id.
///
/// The app segment is compared **loosely** to the inventory's `app_name`,
/// because only macOS reports a display name there (`kCGWindowOwnerName`
/// = `Brave Origin`). Linux reports `/proc/<pid>/comm` (`brave`,
/// `chromium-browse` -- truncated at 15 bytes) and Windows the image name
/// (`brave.exe`), so an exact ` - {app} - ` marker never occurs off macOS
/// and every Chromium row answered `browser_profile: null` there. Both
/// sides reduce to an alphanumeric token of at least four characters and
/// match when one contains the other.
pub fn browser_profile_from_identity(app: &str, title: &str) -> Option<String> {
    let title = title.trim();
    let wanted = app_token(app)?;
    // Which " - " ends the app segment is not always the last one: a
    // profile name may itself contain " - " (`Grok - Brave Origin - my -
    // profile`), and splitting on the last two separators then reads `my`
    // as the app and loses the profile. So the split points are scanned
    // from the right and the first one whose *preceding* segment matches
    // the application wins; everything behind it is the profile.
    let profile = separator_offsets(title).into_iter().rev().find_map(|cut| {
        let (_, title_app) = title.get(..cut)?.rsplit_once(" - ")?;
        let found = app_token(title_app)?;
        (found == wanted || found.contains(&wanted) || wanted.contains(&found))
            .then(|| title.get(cut + SEPARATOR.len()..))
            .flatten()
    })?;
    let profile = profile.trim();
    if profile.is_empty() || profile.len() > 64 {
        return None;
    }
    if profile.contains('/') || profile.contains('\n') || profile.contains('\0') {
        return None;
    }
    Some(profile.to_owned())
}

pub fn looks_like_browser_app(app: &str) -> bool {
    let app = app.to_ascii_lowercase();
    app.contains("brave")
        || app.contains("chrome")
        || app.contains("chromium")
        || app.contains("edge")
        || app.contains("safari")
        || app.contains("firefox")
        || app.contains("arc")
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
        "browser_profile": browser_profile_from_identity(&window.app_name, &window.title),
    })
}

/// The same row plus its place in the desktop's front-to-back order, when
/// the host reports one.
///
/// A window missing from `stacking` gets no `z_index` and no
/// `occluded_percent` rather than a default: absent means "this host did
/// not say", and 0 would mean "frontmost and fully visible", which is a
/// very different claim.
pub fn window_row_json_with_stacking(
    window: &WindowInfo,
    stacking: &[crate::mechanism::window_enumerate::WindowStacking],
) -> serde_json::Value {
    let mut row = window_row_json(window);
    if let Some(place) = stacking.iter().find(|row| row.handle == window.handle)
        && let Some(object) = row.as_object_mut()
    {
        object.insert("z_index".into(), serde_json::json!(place.z_index));
        object.insert(
            "occluded_percent".into(),
            serde_json::json!(place.occluded_percent),
        );
    }
    // Which managed Spaces the window sits on. A window on another Space is
    // present but not on screen, which is neither minimized nor closed --
    // and an inventory that cannot say so sends an agent looking for a
    // window it will never see. Absent when the host has no such notion or
    // no SPI for it; never a default.
    #[cfg(target_os = "macos")]
    if let Ok(Some(spaces)) = crate::macos_spaces::spaces_for_window(window.handle)
        && let Some(object) = row.as_object_mut()
    {
        object.insert("spaces".into(), serde_json::json!(spaces));
    }
    row
}

/// One poll-diff event over two `windows` inventories.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowWatchEvent<'a> {
    pub kind: &'static str,
    pub window: &'a WindowInfo,
    pub fields: Vec<&'static str>,
}

fn window_changed_fields(before: &WindowInfo, after: &WindowInfo) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if before.title != after.title {
        fields.push("title");
    }
    if before.app_name != after.app_name {
        fields.push("app_name");
    }
    if before.process_id != after.process_id {
        fields.push("process_id");
    }
    if before.bounds != after.bounds {
        fields.push("bounds");
    }
    if before.focused != after.focused {
        fields.push("focused");
    }
    if before.minimized != after.minimized {
        fields.push("minimized");
    }
    fields
}

/// Diff two window inventories by native handle. Order is disappeared,
/// changed, appeared — matching MCU watch (lifecycle then field change).
pub fn diff_window_inventory<'a>(
    before: &'a [WindowInfo],
    after: &'a [WindowInfo],
) -> Vec<WindowWatchEvent<'a>> {
    let mut previous = BTreeMap::new();
    for window in before {
        previous.insert(window.handle, window);
    }
    let mut current = BTreeMap::new();
    for window in after {
        current.insert(window.handle, window);
    }
    let mut events = Vec::new();
    for (handle, window) in &previous {
        if !current.contains_key(handle) {
            events.push(WindowWatchEvent {
                kind: "disappeared",
                window,
                fields: Vec::new(),
            });
        }
    }
    for (handle, window) in &current {
        match previous.get(handle) {
            None => events.push(WindowWatchEvent {
                kind: "appeared",
                window,
                fields: Vec::new(),
            }),
            Some(was) => {
                let fields = window_changed_fields(was, window);
                if !fields.is_empty() {
                    events.push(WindowWatchEvent {
                        kind: "changed",
                        window,
                        fields,
                    });
                }
            }
        }
    }
    events
}

pub fn window_watch_event_json(
    seq: u64,
    t_ms: u64,
    event: &WindowWatchEvent<'_>,
) -> serde_json::Value {
    serde_json::json!({
        "seq": seq,
        "t_ms": t_ms,
        "kind": event.kind,
        "handle": event.window.handle,
        "ref": window_stable_ref(event.window),
        "title": event.window.title,
        "process_id": event.window.process_id,
        "app_name": event.window.app_name,
        "fields": event.fields,
    })
}

/// Running apps from a window inventory. Installed-but-not-running is not
/// in this list.
pub fn running_apps_json(windows: &[WindowInfo]) -> serde_json::Value {
    let mut by_app: BTreeMap<String, (Vec<u32>, Vec<serde_json::Value>)> = BTreeMap::new();
    for window in windows {
        let entry = by_app.entry(window.app_name.clone()).or_default();
        if !entry.0.contains(&window.process_id) {
            entry.0.push(window.process_id);
        }
        entry.1.push(window_row_json(window));
    }
    serde_json::Value::Array(
        by_app
            .into_iter()
            .map(|(app_name, (mut pids, wins))| {
                pids.sort_unstable();
                serde_json::json!({
                    "app_name": app_name,
                    "pids": pids,
                    "window_count": wins.len(),
                    "windows": wins,
                })
            })
            .collect(),
    )
}

/// `windows-watch` bounds. `duration_ms == 0` means one extra sample.
pub fn validate_windows_watch(
    duration_ms: u64,
    max_events: Option<usize>,
    interval_ms: Option<u64>,
) -> Result<(), String> {
    if duration_ms > MAX_OBSERVE_DURATION_MS {
        return Err(format!(
            "--duration-ms must be 0..={MAX_OBSERVE_DURATION_MS}, got {duration_ms}"
        ));
    }
    if let Some(max_events) = max_events
        && (max_events == 0 || max_events > MAX_OBSERVE_EVENTS)
    {
        return Err(format!(
            "--max-events must be 1..={MAX_OBSERVE_EVENTS}, got {max_events}"
        ));
    }
    if let Some(interval_ms) = interval_ms {
        if duration_ms == 0 {
            if interval_ms > MAX_OBSERVE_DURATION_MS {
                return Err(format!(
                    "--interval-ms must be 0..={MAX_OBSERVE_DURATION_MS}, got {interval_ms}"
                ));
            }
        } else if interval_ms < MIN_OBSERVE_INTERVAL_MS || interval_ms > duration_ms {
            return Err(format!(
                "--interval-ms must be {MIN_OBSERVE_INTERVAL_MS}..=duration, got {interval_ms}"
            ));
        }
    }
    Ok(())
}

pub fn windows_watch_interval_ms(duration_ms: u64, interval_ms: Option<u64>) -> u64 {
    interval_ms.unwrap_or(if duration_ms == 0 {
        0
    } else {
        DEFAULT_OBSERVE_INTERVAL_MS
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
// Focus resolution for the inventory (`windows --focused`, `focused-window`).
// ---------------------------------------------------------------------------

/// The application the host reports as frontmost (macOS: NSWorkspace's
/// `frontmostApplication`; other hosts: none yet).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrontmostApp {
    pub name: String,
    pub pid: u32,
    pub bundle_id: Option<String>,
}

impl FrontmostApp {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "pid": self.pid,
            "bundle_id": self.bundle_id,
        })
    }
}

/// Which window of the inventory is the focused one, and how that was
/// decided. `handle` is `None` only when no application is frontmost or
/// the frontmost one has no window in the inventory (a menu-bar-only app,
/// a window on another Space, an app the inventory cannot see); `reason`
/// says which.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FocusResolution {
    pub app: Option<FrontmostApp>,
    pub handle: Option<isize>,
    /// `inventory-mark` (the mechanism marked it), `ax-focused-window`
    /// (the frontmost app's own AXFocusedWindow), or
    /// `frontmost-app-front-window` (the frontmost app's topmost window in
    /// the stacking order, inventory order when there is none).
    pub via: Option<&'static str>,
    /// `no_frontmost_app` or `frontmost_app_has_no_inventory_window`.
    pub reason: Option<&'static str>,
}

impl FocusResolution {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "handle": self.handle,
            "via": self.via,
            "reason": self.reason,
        })
    }
}

/// Decide the focused window of `windows`, in this order:
///
/// 1. a row the mechanism already marked `focused` (first one wins);
/// 2. `ax_window` -- the frontmost app's own focused window -- when it is
///    in the inventory and belongs to that app's pid;
/// 3. the frontmost app's topmost inventory window: lowest `z_index` in
///    `stacking`, or its first inventory row when the host reports no
///    stacking order (the macOS list is front-to-back already);
/// 4. otherwise no window, with the reason.
///
/// The frontmost-app fallback exists because the mechanism's system-wide
/// accessibility read can fail wholesale from a process outside the GUI
/// session's front process chain (see `macos_focus`), and an inventory
/// that then says "nothing is focused" is wrong, not merely empty.
pub fn resolve_focus(
    windows: &[WindowInfo],
    stacking: &[crate::mechanism::window_enumerate::WindowStacking],
    app: Option<FrontmostApp>,
    ax_window: Option<isize>,
) -> FocusResolution {
    if let Some(marked) = windows.iter().find(|window| window.focused) {
        return FocusResolution {
            app,
            handle: Some(marked.handle),
            via: Some("inventory-mark"),
            reason: None,
        };
    }
    let Some(front) = app else {
        return FocusResolution {
            app: None,
            handle: None,
            via: None,
            reason: Some("no_frontmost_app"),
        };
    };
    if let Some(handle) = ax_window
        && windows
            .iter()
            .any(|window| window.handle == handle && window.process_id == front.pid)
    {
        return FocusResolution {
            app: Some(front),
            handle: Some(handle),
            via: Some("ax-focused-window"),
            reason: None,
        };
    }
    let mine: Vec<&WindowInfo> = windows
        .iter()
        .filter(|window| window.process_id == front.pid)
        .collect();
    if mine.is_empty() {
        return FocusResolution {
            app: Some(front),
            handle: None,
            via: None,
            reason: Some("frontmost_app_has_no_inventory_window"),
        };
    }
    let z = |window: &WindowInfo| {
        stacking
            .iter()
            .find(|row| row.handle == window.handle)
            .map(|row| row.z_index)
    };
    let top = mine
        .iter()
        .copied()
        .filter(|window| z(window).is_some())
        .min_by_key(|window| z(window))
        .unwrap_or(mine[0]);
    FocusResolution {
        app: Some(front),
        handle: Some(top.handle),
        via: Some("frontmost-app-front-window"),
        reason: None,
    }
}

/// Write the resolution back into the rows: exactly the resolved window
/// is `focused`, every other row is not. Without a resolved window the
/// rows are left as the mechanism reported them.
pub fn apply_focus(windows: &mut [WindowInfo], focus: &FocusResolution) {
    if let Some(handle) = focus.handle {
        for window in windows.iter_mut() {
            window.focused = window.handle == handle;
        }
    }
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

/// `selected` is known false only when the backend says so: macOS reads
/// `AXSelected`, Windows the SelectionItem pattern, Linux the AT-SPI
/// `Selectable` state. A node carrying neither word has no selection of
/// its own, which is not the same as being unselected.
pub fn selected_state(node: &A11yNode) -> Tri {
    if has_state(node, "selected") {
        Tri::True
    } else if has_state(node, "unselected") {
        Tri::False
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
    tree_changed_with(before, after, false)
}

/// Like [`tree_changed`], but a focus-only state flip is not a success
/// proof. Chromium custom switches often take AXPress, move focus, and
/// leave `checked` unchanged.
pub fn tree_changed_semantically(before: &A11yTree, after: &A11yTree) -> bool {
    tree_changed_with(before, after, true)
}

fn tree_changed_with(before: &A11yTree, after: &A11yTree, ignore_focus: bool) -> bool {
    if before.nodes.len() != after.nodes.len() {
        return true;
    }
    before.nodes.iter().zip(after.nodes.iter()).any(|(a, b)| {
        a.id != b.id
            || a.role != b.role
            || a.name != b.name
            || a.text != b.text
            || states_for_diff(&a.states, ignore_focus) != states_for_diff(&b.states, ignore_focus)
    })
}

fn states_for_diff(states: &[String], ignore_focus: bool) -> Vec<String> {
    let mut out: Vec<String> = states
        .iter()
        .filter(|state| !ignore_focus || !state.eq_ignore_ascii_case("focused"))
        .cloned()
        .collect();
    out.sort();
    out
}

/// Checkbox / switch / radio: `checked` is the business state. Focus is not.
pub fn is_toggle_control(node: &A11yNode) -> bool {
    if checked_state(node) != Tri::Unknown {
        return true;
    }
    matches!(
        normalize_role(&node.role).as_str(),
        "checkbox" | "switch" | "toggle" | "radiobutton" | "togglebutton"
    )
}

/// Proof for `invoke press` / tree-addressed `click` after a re-read.
pub struct PressProof {
    pub verified: bool,
    pub method: &'static str,
    pub reason: Option<&'static str>,
}

pub fn verify_press(
    before_node: &A11yNode,
    after_node: Option<&A11yNode>,
    before_tree: &A11yTree,
    after_tree: &A11yTree,
) -> PressProof {
    if is_toggle_control(before_node) {
        let Some(now) = after_node else {
            return PressProof {
                verified: false,
                method: "checked-readback",
                reason: Some("node_gone"),
            };
        };
        let was = checked_state(before_node);
        let is = checked_state(now);
        if was != Tri::Unknown && is != Tri::Unknown && was.as_bool() != is.as_bool() {
            return PressProof {
                verified: true,
                method: "checked-readback",
                reason: None,
            };
        }
        return PressProof {
            verified: false,
            method: "checked-readback",
            reason: Some("checked_unchanged"),
        };
    }
    if after_node.is_none() {
        return PressProof {
            verified: true,
            method: "tree-diff",
            reason: Some("node_gone"),
        };
    }
    if tree_changed_semantically(before_tree, after_tree) {
        PressProof {
            verified: true,
            method: "tree-diff",
            reason: None,
        }
    } else {
        PressProof {
            verified: false,
            method: "tree-diff",
            reason: Some("no_observable_change"),
        }
    }
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
    matches!(
        normalize_role(role).as_str(),
        // AT-SPI gives a checkable entry its own role; macOS and UIA keep
        // one `menu item` role and carry the mark separately. Missing
        // these made `menu inspect` omit items `menu invoke` would press.
        "menubaritem" | "menuitem" | "checkmenuitem" | "radiomenuitem"
    )
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
/// Whether a menu entry can be pressed, read so both state vocabularies
/// answer honestly.
///
/// macOS publishes exactly one of `enabled` / `disabled` on every node, so
/// "not disabled" reads correctly there. AT-SPI has no `disabled` label at
/// all -- a greyed-out GTK item simply omits `enabled` -- so "not
/// disabled" would call every AT-SPI item pressable, including the ones
/// `menu invoke` then refuses. Requiring the positive label agrees with
/// both, and errs toward refusing rather than toward promising a press.
fn menu_entry_enabled(node: &A11yNode) -> bool {
    has_state(node, "enabled") && !has_state(node, "disabled")
}

pub fn menu_items(tree: &A11yTree) -> Vec<MenuItem> {
    let by_id: std::collections::HashMap<&str, &A11yNode> = tree
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    // Which nodes a path segment can name. The two backends model the same
    // menu differently: macOS names the entry (`AXMenuBarItem` "File") and
    // hangs an `AXMenu` child off it that repeats the title, while AT-SPI
    // has no separate entry node at all -- GTK's "File" *is* a role-`menu`
    // node holding the items. So a named `menu` counts as an entry unless
    // it is the macOS duplicate, i.e. a `menu` owned directly by an item.
    let is_entry = |node: &A11yNode| -> bool {
        if is_menu_item_role(&node.role) {
            return true;
        }
        if !is_menu_role(&node.role) || node.name.is_empty() {
            return false;
        }
        !node
            .parent_id
            .as_deref()
            .and_then(|owner| by_id.get(owner))
            .is_some_and(|owner| is_menu_item_role(&owner.role))
    };
    // Nearest entry ancestor of each entry. Owning one is what "has a
    // submenu" means here -- not owning a `menu` node, which only the
    // macOS shape publishes.
    let mut parent_entry: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut owns_submenu: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for node in &tree.nodes {
        if !is_entry(node) {
            continue;
        }
        // Both walks here are over parent links a backend supplies, and a
        // backend can hand back a cycle -- a node whose parent chain leads
        // to itself. Measured on Windows: UIA gave the menu bar a parent
        // link that resolved back to an entry already on the path, and an
        // unguarded walk pushed names forever, taking `menu inspect` to
        // 2 GB and never returning. Every ancestor walk is bounded by the
        // node count and refuses to revisit.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        seen.insert(node.id.as_str());
        let mut cursor = node.parent_id.as_deref();
        while let Some(parent_id) = cursor {
            if !seen.insert(parent_id) {
                break;
            }
            let Some(parent) = by_id.get(parent_id) else {
                break;
            };
            if is_entry(parent) {
                // A node that is its own nearest entry ancestor has no
                // parent entry; recording one would be the cycle.
                if parent.id != node.id {
                    parent_entry.insert(node.id.as_str(), parent.id.as_str());
                    owns_submenu.insert(parent.id.as_str());
                }
                break;
            }
            cursor = parent.parent_id.as_deref();
        }
    }
    let mut items = Vec::new();
    for node in &tree.nodes {
        if !is_entry(node) {
            continue;
        }
        // Titles of the entry ancestors, nearest last. Bounded the same
        // way: `parent_entry` is built from backend links, so it is not
        // this code's place to assume the chain terminates.
        let mut path = vec![node.name.clone()];
        let mut depth = 0u32;
        let mut walked: std::collections::HashSet<&str> = std::collections::HashSet::new();
        walked.insert(node.id.as_str());
        let mut cursor = parent_entry.get(node.id.as_str()).copied();
        while let Some(parent_id) = cursor {
            if !walked.insert(parent_id) {
                break;
            }
            let Some(parent) = by_id.get(parent_id) else {
                break;
            };
            path.push(parent.name.clone());
            depth += 1;
            cursor = parent_entry.get(parent_id).copied();
        }
        path.reverse();
        items.push(MenuItem {
            index: items.len(),
            id: node.id.clone(),
            path,
            title: node.name.clone(),
            depth,
            enabled: menu_entry_enabled(node),
            checked: has_state(node, "checked"),
            has_submenu: owns_submenu.contains(node.id.as_str()),
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

    /// A backend can hand back a parent chain that leads back to itself.
    /// UIA did, on a real Notepad window: `menu inspect` then walked the
    /// ancestors forever, pushing a title on every lap, and reached 2 GB
    /// without returning. Terminating is not optional here.
    #[test]
    fn menu_items_terminate_on_a_parent_chain_that_cycles() {
        let self_parent = tree(
            vec![
                menu_node("/0", Some("/0"), "menu bar", "", &["enabled"]),
                menu_node("/0/0", Some("/0"), "menu", "File", &["enabled"]),
                menu_node("/0/0/0", Some("/0/0"), "menu item", "Open", &["enabled"]),
            ],
            false,
        );
        let items = menu_items(&self_parent);
        assert!(items.iter().all(|item| item.path.len() <= 3), "{items:?}");

        // A two-node cycle: each entry names the other as its parent.
        let mutual = tree(
            vec![
                menu_node("/a", Some("/b"), "menu", "A", &["enabled"]),
                menu_node("/b", Some("/a"), "menu", "B", &["enabled"]),
            ],
            false,
        );
        let looped = menu_items(&mutual);
        assert_eq!(looped.len(), 2);
        assert!(looped.iter().all(|item| item.path.len() <= 2), "{looped:?}");
    }

    /// AT-SPI has no separate bar-entry node: GTK's "File" is a role-`menu`
    /// node that directly holds the items, and the bar sits several widget
    /// levels down rather than at the tree root. The flattened paths must
    /// still be the paths `menu invoke` accepts.
    #[test]
    fn menu_items_read_the_at_spi_shape_with_no_separate_bar_entry() {
        let t = tree(
            vec![
                menu_node("/0", None, "menu bar", "", &["enabled", "sensitive"]),
                menu_node(
                    "/0/0",
                    Some("/0"),
                    "menu",
                    "File",
                    &["enabled", "sensitive"],
                ),
                menu_node(
                    "/0/0/0",
                    Some("/0/0"),
                    "menu item",
                    "Do Thing",
                    &["enabled", "sensitive", "selectable"],
                ),
                // As GTK actually publishes it: no `disabled` label, just
                // no `enabled` one.
                menu_node(
                    "/0/0/1",
                    Some("/0/0"),
                    "menu item",
                    "Disabled Thing",
                    &["selectable", "visible"],
                ),
                menu_node("/0/0/2", Some("/0/0"), "menu", "More", &["enabled"]),
                menu_node(
                    "/0/0/2/0",
                    Some("/0/0/2"),
                    "menu item",
                    "Deeper",
                    &["enabled"],
                ),
                menu_node(
                    "/0/0/3",
                    Some("/0/0"),
                    "check menu item",
                    "Marked Thing",
                    &["enabled", "checked"],
                ),
            ],
            false,
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
                "File/More/Deeper",
                "File/Marked Thing"
            ]
        );
        assert_eq!(items[0].depth, 0);
        assert!(items[0].has_submenu);
        assert_eq!(items[1].depth, 1);
        assert!(!items[1].has_submenu && items[1].enabled);
        assert!(!items[2].enabled);
        assert!(items[3].has_submenu);
        assert_eq!(items[4].depth, 2);
        assert!(items[5].checked && items[5].enabled);
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
        let mut focused_only = before.clone();
        focused_only.nodes[1].states = vec!["focused".into()];
        assert!(tree_changed(&before, &focused_only));
        assert!(!tree_changed_semantically(&before, &focused_only));
        let mut checkbox = node("/0/c", "checkbox", "workflow", &["click"]);
        checkbox.states = vec!["checked".into(), "enabled".into()];
        let mut still_checked = checkbox.clone();
        still_checked.states.push("focused".into());
        let mut unchecked = checkbox.clone();
        unchecked.states = vec!["unchecked".into(), "enabled".into()];
        let toggle_tree = tree(vec![checkbox.clone()], false);
        let focused_tree = tree(vec![still_checked.clone()], false);
        let flipped_tree = tree(vec![unchecked.clone()], false);
        let focus_only = verify_press(&checkbox, Some(&still_checked), &toggle_tree, &focused_tree);
        assert!(!focus_only.verified);
        assert_eq!(focus_only.method, "checked-readback");
        assert_eq!(focus_only.reason, Some("checked_unchanged"));
        let flipped = verify_press(&checkbox, Some(&unchecked), &toggle_tree, &flipped_tree);
        assert!(flipped.verified);
        assert_eq!(flipped.method, "checked-readback");
        assert!(is_toggle_control(&checkbox));
        assert!(!is_toggle_control(&node(
            "/0",
            "button",
            "OK",
            &["enabled"]
        )));
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
        assert_eq!(normalize_role("button"), "button");
        assert_eq!(normalize_role("push button"), "button");
        assert_eq!(normalize_role("PushButton"), "button");
        assert!(selector_role_matches("push button", "button"));
        assert!(selector_role_matches("button", "push button"));
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
    fn focus_resolution_prefers_the_mark_then_ax_then_the_front_window() {
        use crate::mechanism::window_enumerate::WindowStacking;
        let windows = vec![
            window(1, 100, "TextEdit", "fixture-1.txt", false),
            window(2, 200, "Brave Origin", "Codex", false),
            window(3, 200, "Brave Origin", "Inbox", false),
            window(4, 200, "Brave Origin", "Grok", false),
        ];
        let stacking = vec![
            WindowStacking {
                handle: 1,
                z_index: 0,
                occluded_percent: 0,
            },
            WindowStacking {
                handle: 3,
                z_index: 1,
                occluded_percent: 10,
            },
            WindowStacking {
                handle: 2,
                z_index: 2,
                occluded_percent: 50,
            },
        ];
        let brave = FrontmostApp {
            name: "Brave Origin".into(),
            pid: 200,
            bundle_id: Some("com.brave.Browser".into()),
        };
        // No frontmost app at all: nothing resolved, typed reason.
        let none = resolve_focus(&windows, &stacking, None, None);
        assert_eq!(none.handle, None);
        assert_eq!(none.reason, Some("no_frontmost_app"));
        assert_eq!(none.json()["reason"], "no_frontmost_app");
        // The frontmost app's own AX focused window wins when it is one of
        // its inventory rows.
        let ax = resolve_focus(&windows, &stacking, Some(brave.clone()), Some(4));
        assert_eq!(ax.handle, Some(4));
        assert_eq!(ax.via, Some("ax-focused-window"));
        // An AX handle that is not this app's window is not trusted: the
        // stacking order decides (3 is above 2; 4 has no z row).
        let foreign = resolve_focus(&windows, &stacking, Some(brave.clone()), Some(1));
        assert_eq!(foreign.handle, Some(3));
        assert_eq!(foreign.via, Some("frontmost-app-front-window"));
        // No stacking order: the first inventory row of that pid.
        let no_z = resolve_focus(&windows, &[], Some(brave.clone()), None);
        assert_eq!(no_z.handle, Some(2));
        // A frontmost app without inventory windows is explicit.
        let finder = FrontmostApp {
            name: "Finder".into(),
            pid: 300,
            bundle_id: None,
        };
        let orphan = resolve_focus(&windows, &stacking, Some(finder.clone()), Some(9));
        assert_eq!(orphan.handle, None);
        assert_eq!(orphan.reason, Some("frontmost_app_has_no_inventory_window"));
        assert_eq!(orphan.app.as_ref().map(|app| app.pid), Some(300));
        assert_eq!(finder.json()["name"], "Finder");
        // A mechanism mark beats everything and is kept as the source.
        let mut marked = windows.clone();
        marked[0].focused = true;
        let mark = resolve_focus(&marked, &stacking, Some(brave), Some(4));
        assert_eq!(mark.handle, Some(1));
        assert_eq!(mark.via, Some("inventory-mark"));
        // apply_focus marks exactly the resolved row.
        let mut rows = windows.clone();
        apply_focus(&mut rows, &ax);
        let focused: Vec<isize> = rows
            .iter()
            .filter(|w| w.focused)
            .map(|w| w.handle)
            .collect();
        assert_eq!(focused, [4]);
        let mut untouched = windows.clone();
        apply_focus(&mut untouched, &orphan);
        assert!(untouched.iter().all(|w| !w.focused));
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
    fn browser_profile_parses_chromium_window_titles() {
        assert_eq!(
            browser_profile_from_identity(
                "Brave Origin",
                "App Store Connect - Brave Origin - profile-a"
            )
            .as_deref(),
            Some("profile-a")
        );
        assert_eq!(
            browser_profile_from_identity("Brave Origin", "Grok - Brave Origin - work").as_deref(),
            Some("work")
        );
        assert_eq!(
            browser_profile_from_identity("Google Chrome", "Inbox - Google Chrome - Profile 1")
                .as_deref(),
            Some("Profile 1")
        );
        assert!(browser_profile_from_identity("Brave Origin", "App Store Connect").is_none());
        assert!(browser_profile_from_identity("TextEdit", "notes.txt").is_none());
        // Only macOS reports a display name in `app_name`. Windows reports
        // the image name and Linux `/proc/<pid>/comm` (capped at 15 bytes),
        // so an exact " - {app} - " marker never matches there and every
        // Chromium row answered `browser_profile: null` off macOS.
        assert_eq!(
            browser_profile_from_identity("brave.exe", "Grok - Brave Browser - work").as_deref(),
            Some("work")
        );
        assert_eq!(
            browser_profile_from_identity("chrome", "Inbox - Google Chrome - Profile 1").as_deref(),
            Some("Profile 1")
        );
        assert_eq!(
            browser_profile_from_identity("chromium-browse", "Docs - Chromium - profile-a")
                .as_deref(),
            Some("profile-a")
        );
        // The no-profile shape stays a miss: a title needs both the app
        // segment and the profile segment behind it.
        assert!(browser_profile_from_identity("brave.exe", "Grok - Brave Browser").is_none());
        // Two short tokens must not pair up just because one contains the
        // other: four characters is the floor on both sides.
        assert!(browser_profile_from_identity("vim", "notes - vim - 2").is_none());
        assert!(browser_profile_from_identity("bash", "log - sh - 2").is_none());
        // A different application is still not a match.
        assert!(browser_profile_from_identity("brave.exe", "Notes - TextEdit - work").is_none());
        // A profile name may itself contain " - ": the app segment, not the
        // last separator, decides where the profile starts.
        assert_eq!(
            browser_profile_from_identity("Brave Origin", "Grok - Brave Origin - my - profile")
                .as_deref(),
            Some("my - profile")
        );
        assert_eq!(
            browser_profile_from_identity("brave.exe", "Grok - Brave Browser - a - b - c")
                .as_deref(),
            Some("a - b - c")
        );
        // The scan stops at the first matching app segment, so a page title
        // that happens to repeat the browser name does not steal the split.
        assert_eq!(
            browser_profile_from_identity(
                "Brave Origin",
                "Brave Origin tips - Brave Origin - work"
            )
            .as_deref(),
            Some("work")
        );
        let rows = [
            window(
                1,
                1,
                "Brave Origin",
                "ASC - Brave Origin - profile-a",
                false,
            ),
            window(
                2,
                1,
                "Brave Origin",
                "Mail - Brave Origin - profile-a",
                false,
            ),
            window(3, 2, "Brave Origin", "Chat - Brave Origin - other", false),
        ];
        let profiles: Vec<_> = rows
            .iter()
            .map(|row| {
                window_row_json(row)["browser_profile"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(profiles, ["profile-a", "profile-a", "other"]);
    }

    #[test]
    fn window_ref_is_mcu_app_hash_handle() {
        let brave = window(14278, 1, "Brave Origin", "Exact Reply", true);
        assert_eq!(window_stable_ref(&brave), "Brave Origin#14278");
        let row = window_row_json(&brave);
        assert_eq!(row["ref"], "Brave Origin#14278");
        assert_eq!(row["handle"], 14278);
        assert!(row["browser_profile"].is_null());
        let profiled = window(
            16784,
            1,
            "Brave Origin",
            "App Store Connect - Brave Origin - profile-a",
            false,
        );
        assert_eq!(
            browser_profile_from_identity(&profiled.app_name, &profiled.title).as_deref(),
            Some("profile-a")
        );
        assert_eq!(window_row_json(&profiled)["browser_profile"], "profile-a");
        let other = window(9, 2, "Brave Origin", "Grok - Brave Origin - work", false);
        assert_eq!(
            browser_profile_from_identity(&other.app_name, &other.title).as_deref(),
            Some("work")
        );
        assert!(looks_like_browser_app("Brave Origin"));
        assert!(!looks_like_browser_app("TextEdit"));
        assert!(browser_profile_from_identity("TextEdit", "notes.txt").is_none());
        assert_eq!(parse_window_token("14278").unwrap(), 14278);
        assert_eq!(parse_window_token("Brave Origin#14278").unwrap(), 14278);
        assert_eq!(parse_window_token("  TextEdit#9  ").unwrap(), 9);
        assert!(parse_window_token("0").is_err());
        assert!(parse_window_token("#9").is_err());
        assert!(parse_window_token("Nope").is_err());
    }

    #[test]
    fn window_watch_diff_and_running_apps() {
        let a = window(1, 100, "TextEdit", "a.txt", false);
        let b = window(2, 200, "Brave Origin", "Chat", true);
        let a2 = window(1, 100, "TextEdit", "b.txt", true);
        let c = window(3, 300, "Finder", "Desktop", false);
        let before = [a.clone(), b.clone()];
        let after = [a2.clone(), c.clone()];
        let events = diff_window_inventory(&before, &after);
        let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"disappeared"));
        assert!(kinds.contains(&"appeared"));
        assert!(kinds.contains(&"changed"));
        let changed = events.iter().find(|e| e.kind == "changed").unwrap();
        assert_eq!(changed.window.handle, 1);
        assert!(changed.fields.contains(&"title"));
        assert!(changed.fields.contains(&"focused"));
        let apps = running_apps_json(&[a, b, a2]);
        assert_eq!(apps.as_array().unwrap().len(), 2);
        assert_eq!(apps[0]["app_name"], "Brave Origin");
        assert_eq!(apps[1]["app_name"], "TextEdit");
        assert_eq!(apps[1]["window_count"], 2);
        assert_eq!(apps[1]["pids"], serde_json::json!([100]));
        validate_windows_watch(0, Some(10), Some(0)).unwrap();
        assert!(validate_windows_watch(1, Some(0), None).is_err());
        assert_eq!(windows_watch_interval_ms(0, None), 0);
        assert_eq!(
            windows_watch_interval_ms(200, None),
            DEFAULT_OBSERVE_INTERVAL_MS
        );
        assert_eq!(parse_text_selection("12:3").unwrap(), (12, 15));
        assert_eq!(parse_text_selection("0:0").unwrap(), (0, 0));
        assert!(parse_text_selection("1").is_err());
        assert!(parse_text_selection("-1:2").is_err());
    }

    #[test]
    fn query_selector_walks_role_index_and_title() {
        let mut group = node("/0/0", "AXGroup", "", &[]);
        group.parent_id = Some("/0".into());
        let mut web = node("/0/0/0", "AXWebArea", "Exact Reply", &[]);
        web.parent_id = Some("/0/0".into());
        web.states.push("showing".into());
        let mut other = node("/0/0/1", "AXButton", "reload", &["AXPress"]);
        other.parent_id = Some("/0/0".into());
        let t = tree(
            vec![node("/0", "AXWindow", "w", &[]), group, web, other],
            false,
        );
        assert_eq!(
            parse_selector("AXGroup[0] / AXWebArea[0]").unwrap().len(),
            2
        );
        let hit = walk_selector(&t, "AXGroup[0] / AXWebArea[0]")
            .unwrap()
            .expect("webarea");
        assert_eq!(hit.role, "AXWebArea");
        assert_eq!(hit.name, "Exact Reply");
        let titled = walk_selector(&t, "AXGroup[0] / *@Exact")
            .unwrap()
            .expect("title");
        assert_eq!(titled.role, "AXWebArea");
        let flat = flatten(&t);
        let scoped = query_selector_scope(&t, &flat, "AXGroup[0] / AXWebArea[0]").unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].node.role, "AXWebArea");
        assert!(parse_selector("").is_err());
        assert!(parse_selector("!!!").is_err());
        assert!(walk_selector(&t, "AXMissing[0]").unwrap().is_none());
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
        assert!(!include_str!("executor/browser.rs").contains("eval("));
        assert!(!include_str!("executor/dispatch.rs").contains("eval("));
        for source in [
            include_str!("cdp/mod.rs"),
            include_str!("cdp/ws.rs"),
            include_str!("cdp/targets.rs"),
            include_str!("cdp/evaluate.rs"),
            include_str!("cdp/ax.rs"),
            include_str!("cdp/page.rs"),
        ] {
            assert!(!source.contains("eval("));
            assert!(!source.contains("new Function"));
        }
    }
}
