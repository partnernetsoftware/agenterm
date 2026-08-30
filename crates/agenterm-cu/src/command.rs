//! Abstract, target-agnostic command set (PRD_02_29).

use serde::{Deserialize, Serialize};

use crate::target::TargetRef;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PointerButton {
    #[default]
    Left,
    Right,
    Middle,
}

/// The `invoke` action vocabulary (absorbed from `moltbaby/skills/mcu`,
/// 2026-08-30): one spelling on every platform; a platform without a
/// mapping answers typed `unsupported`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvokeAction {
    Press,
    SetValue,
    SelectOption,
    SetChecked,
    SetExpanded,
    Increment,
    Decrement,
}

/// What an `invoke` action's `VALUE` positional must be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvokeValueKind {
    /// No value (`press`, `increment`, `decrement`).
    None,
    /// Free text (`set-value`, `select-option`).
    Text,
    /// `true` / `false` (`set-checked`, `set-expanded`).
    Flag,
}

impl InvokeAction {
    pub const ALL: [InvokeAction; 7] = [
        Self::Press,
        Self::SetValue,
        Self::SelectOption,
        Self::SetChecked,
        Self::SetExpanded,
        Self::Increment,
        Self::Decrement,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|action| action.as_str() == raw.trim())
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::SetValue => "set-value",
            Self::SelectOption => "select-option",
            Self::SetChecked => "set-checked",
            Self::SetExpanded => "set-expanded",
            Self::Increment => "increment",
            Self::Decrement => "decrement",
        }
    }

    pub fn value_kind(self) -> InvokeValueKind {
        match self {
            Self::Press | Self::Increment | Self::Decrement => InvokeValueKind::None,
            Self::SetValue | Self::SelectOption => InvokeValueKind::Text,
            Self::SetChecked | Self::SetExpanded => InvokeValueKind::Flag,
        }
    }
}

/// One `verify --expect` / `wait --expect` item: a target (at least one of
/// `node`, `index`, `name`, `identifier`, `role`; `role` narrows `name` /
/// `identifier` or stands alone) plus the states to compare. The shape is
/// closed: an unknown key fails at parse time, so a misspelled state can
/// never pass by being ignored.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    /// Case-insensitive substring of the accessible name (showing nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Exact toolkit identifier (showing nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    /// Role in either spelling (`AXCheckBox` / `check-box`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Exact node `text` (the value `query` reports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused: Option<bool>,
}

impl Expectation {
    pub fn has_target(&self) -> bool {
        self.node.is_some()
            || self.index.is_some()
            || self.name.is_some()
            || self.identifier.is_some()
            || self.role.is_some()
    }

    pub fn has_state(&self) -> bool {
        self.value.is_some()
            || self.checked.is_some()
            || self.expanded.is_some()
            || self.focused.is_some()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum Command {
    Capabilities {
        target: TargetRef,
    },
    /// Top-level window inventory. Without any filter or page field the
    /// reply `data` is the plain window array (unchanged shape); with one,
    /// `data` is the inventory object `{windows, visited, matched, returned,
    /// offset, truncated}` so a filtered read carries its counts.
    Windows {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        /// Case-insensitive substring of `app_name`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        /// Case-insensitive substring of `title`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focused: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minimized: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// Bounded control-tree observation. `depth` (root = 0) and `max_nodes`
    /// apply while the platform adapter walks the backend; the reply reports
    /// `truncated` / `visited` / `returned`. `flat` lists the same nodes in
    /// the same order with a `depth` and a flatten `index` per node — the
    /// numbering a later `invoke --index` addresses.
    Tree {
        target: TargetRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flat: bool,
    },
    /// Bounded, filtered flat node list over the same walk `tree` makes
    /// (same node ids and flatten indices). Filters: `role` (comma list;
    /// `AXTextArea` and `text-area` both match), `text` (case-insensitive
    /// substring of name or text) or `text_exact`, `identifier` (exact),
    /// `actionable` (at least one action), `within` (bounds intersect
    /// `[x, y, w, h]`). `offset` / `max` page the matches. The reply reports
    /// `visited / matched / returned / truncated`.
    Query {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        role: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_exact: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identifier: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        actionable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        within: Option<[i32; 4]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// One semantic action on one node of `window` through the platform
    /// a11y backend, never activating or raising the window. Exactly one of
    /// `node` (path id), `index` (flatten index) or `name` [+ `role`] /
    /// `identifier` addresses the target; two or more showing matches are
    /// `ambiguous`, none is `a11y_node_not_found`, an action the node does
    /// not offer is `unsupported`. The reply carries `verified` (the
    /// postcondition was read back) and a receipt (target, node, action,
    /// before / after state).
    Invoke {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identifier: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        action: InvokeAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        /// Address the application's own focused control (the node
        /// `focused` reports) instead of naming one; `role` may narrow it.
        /// PID, window and focused identity are bound in one observation
        /// before the action.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        focused: bool,
    },
    /// Background menu-bar inventory of the application owning `window`
    /// (macOS `AXMenuBar`, read without opening a menu or activating the
    /// app). `depth` counts menu levels (0 = bar items only, 1 = their
    /// items, default 1, at most 8); `max_nodes` bounds the walk
    /// (1..=5000). `title` is a case-insensitive substring unless `exact`;
    /// `enabled` filters on the item state. `offset` / `max` page the
    /// items; the reply reports `visited / matched / returned / truncated`.
    MenuInspect {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        exact: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<usize>,
    },
    /// Press the menu item at `path` (menu title, then item titles, exact)
    /// in the background: every segment must resolve to exactly one
    /// enabled item before anything is pressed, the last must be a leaf,
    /// and the reply carries `verified` (tree diff / mark read-back).
    MenuInvoke {
        target: TargetRef,
        window: isize,
        path: Vec<String>,
    },
    /// The application's own focused control inside `window` (identity,
    /// role, value preview), read without requiring the foreground. `role`
    /// binds the expected role (mismatch is typed `unverified`);
    /// `max_value_bytes` bounds the value preview (default 4096).
    Focused {
        target: TargetRef,
        window: isize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_value_bytes: Option<usize>,
    },
    /// A bounded, filtered event stream over the same bounded tree for
    /// `duration_ms`: the tree is polled every `interval_ms` and diffed
    /// (ValueChanged / TitleChanged / StateChanged / FocusChanged /
    /// Created / Destroyed), events carry a monotonic `seq` and `t_ms`,
    /// and the stream stops at `max_events` with `truncated: true`.
    Observe {
        target: TargetRef,
        window: isize,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_events: Option<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        notifications: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interval_ms: Option<u64>,
    },
    /// Read one tree and check every expectation against it. All met is
    /// `ok` with per-item results; a known mismatch is typed `unverified`;
    /// a state the node does not expose is typed `unsupported` (fail
    /// closed, never "probably fine").
    Verify {
        target: TargetRef,
        window: isize,
        expect: Vec<Expectation>,
    },
    Screenshot {
        target: TargetRef,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
    },
    /// Move the pointer to absolute target-session screen coordinates without
    /// pressing, releasing, clicking, dragging, or scrolling any button.
    PointerMove {
        target: TargetRef,
        x: i32,
        y: i32,
    },
    /// Observe the pointer's current absolute target-session screen
    /// coordinates without injecting input.
    PointerPosition {
        target: TargetRef,
    },
    Click {
        target: TargetRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        /// Accessible-name substring; resolved with the same showing/visible
        /// matcher as `WaitCondition::NodeNameContains` (exactly one match),
        /// then acted via `--node`. Two or more showing hits are
        /// `a11y_node_ambiguous`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        coords: Option<[i32; 2]>,
        #[serde(default)]
        degraded: bool,
        #[serde(default = "default_clicks")]
        clicks: u32,
        #[serde(default)]
        button: PointerButton,
    },
    Focus {
        target: TargetRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    SendText {
        target: TargetRef,
        text: String,
        /// Optional window scope. With `--name`, name-address the unique
        /// showing node. Without `--name`, write the showing focused node
        /// (same innermost Text candidate as `GetText` without `--name`).
        /// Neither flag keeps the plain focused inject.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Read the target session's native Unicode-text clipboard directly.
    /// This is independent of accessibility-node `copy` / `paste`; absence
    /// of Unicode text is a successful empty string and the native ABI owns
    /// the bounded whole-payload read.
    ClipboardRead {
        target: TargetRef,
    },
    /// Copy AT-SPI `Text.GetText` onto the native clipboard
    /// (`agt_clipboard_set_text`). With `--name`, the unique showing named
    /// node. With `--window` and no `--name`, the showing focused node
    /// (same innermost Text candidate as `GetText` without `--name`).
    /// Never XTest / `--coords` / screenshot when `--window` is set. A
    /// node with no Text interface typed-fails (`a11y_text_unavailable`).
    Copy {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Write clipboard text via native AT-SPI `EditableText` / `Text`.
    /// With `--name`, the unique showing named field. With `--window` and
    /// no `--name`, the showing focused node (same innermost Text
    /// candidate as `GetText` without `--name`). `--text` only seeds the
    /// clipboard; the field write always reads the clipboard. Never XTest
    /// / `--coords` when `--window` is set.
    Paste {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    SendKeys {
        target: TargetRef,
        keys: String,
        /// Optional window scope. With `--name`, name-address the unique
        /// showing node and deliver Device/key events. Without `--name`,
        /// target the showing focused node (same innermost Text candidate
        /// as `GetText` without `--name`). Neither flag keeps the plain
        /// focused inject.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// One-shot AT-SPI `Component.ScrollTo(TopEdge)` on the unique showing
    /// named node. Success is `via=scroll-to`. Missing / false /
    /// `UnknownMethod` typed-fails (`a11y_scroll_unavailable`). Never
    /// Action `scroll*`, XTest wheel, `--coords`, or screenshot.
    Scroll {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Independent AT-SPI `Component.GetExtents(Screen)` for the unique
    /// showing named node. Snapshot `node.bounds` do not count. Empty
    /// extents typed-fail (`a11y_extents_unavailable`).
    GetExtents {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// One-shot AT-SPI `Text.SetSelection(0, start, end)` on the unique
    /// showing named node. Success is `via=set-selection`. Missing Text /
    /// `UnknownMethod` typed-fails (`a11y_selection_unavailable`).
    /// SetSelection false typed-fails (`a11y_selection_no_effect`). Never
    /// XTest, mouse-drag, `--coords`, or screenshot. The reply is not
    /// proof; callers observe via `get-selection`.
    Select {
        target: TargetRef,
        start: i32,
        end: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Independent AT-SPI `Text.GetNSelections` + `GetSelection(0)` for
    /// the unique showing named node. Not the `select` reply payload.
    /// Missing Text typed-fails (`a11y_selection_unavailable`). `n == 0`
    /// is empty success.
    GetSelection {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// One-shot AT-SPI `Text.SetCaretOffset` on the unique showing named
    /// node. Success is `via=set-caret-offset`. Missing Text /
    /// `UnknownMethod` typed-fails (`a11y_caret_unavailable`).
    /// SetCaretOffset false typed-fails (`a11y_caret_no_effect`). Never
    /// XTest, `--coords`, or screenshot. The reply is not proof; callers
    /// observe via `get-caret`.
    SetCaret {
        target: TargetRef,
        offset: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Independent AT-SPI `Text.CaretOffset` / `GetCaretOffset` for the
    /// unique showing named node. Not the `set-caret` reply payload.
    /// Missing Text typed-fails (`a11y_caret_unavailable`).
    GetCaret {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// One-shot independent AT-SPI `Text.GetText` for the unique showing
    /// named node, or — with no `name` — for the node carrying the AT-SPI
    /// `focused` state. Not a `wait --text-equals` poll and not `send-text` /
    /// `paste` / `copy` `matched.text`, `last_text_write_via`, the WebKit
    /// eval helper's queued-job `OK`, or a tree snapshot `text`. Missing
    /// Text typed-fails (`a11y_text_unavailable`). Never XTest / `--coords`.
    GetText {
        target: TargetRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    Wait {
        target: TargetRef,
        timeout_ms: u64,
        #[serde(flatten)]
        condition: WaitCondition,
    },
    WindowPlace {
        target: TargetRef,
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "wait", rename_all = "kebab-case")]
pub enum WaitCondition {
    WindowCountGte {
        count: usize,
    },
    /// Polls the tree until every `verify` expectation is met. A missing
    /// target keeps polling; an ambiguous target or an unobservable state
    /// fails closed at once; the deadline is typed `timeout` carrying the
    /// last observation.
    Expect {
        window: isize,
        expect: Vec<Expectation>,
    },
    WindowTitleContains {
        pattern: String,
    },
    FocusedHandle {
        handle: isize,
    },
    /// Polls the accessibility tree until exactly one showing node matches.
    /// Two or more showing hits fail typed (`a11y_node_ambiguous`) instead of
    /// picking the first. Never falls back to pixels: addressing stays
    /// `accessibility-tree`.
    NodeNameContains {
        pattern: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
    },
    /// Polls AT-SPI `Text.GetText` on the unique showing node addressed by
    /// `--name` until that independent text equals `expected`. Snapshot
    /// `node.text`, `send-text` / `paste` / `copy` `matched.text`,
    /// `last_text_write_via`, and the WebKit eval helper's queued-job `OK`
    /// are not this condition.
    /// Timeout is typed. Never screenshot / XTest / `--coords`.
    NodeTextEquals {
        expected: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
    },
    /// Same independent `Text.GetText` poll as `NodeTextEquals`, but the
    /// hit is `gettext.contains(substring)`. Snapshot `node.text`,
    /// `send-text` / `paste` / `copy` `matched.text`, `last_text_write_via`,
    /// and the WebKit eval helper's queued-job `OK` are not this condition.
    /// Timeout is typed. Never screenshot / XTest / `--coords`.
    NodeTextContains {
        substring: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<isize>,
    },
}

fn default_clicks() -> u32 {
    1
}

impl Command {
    pub fn verb(&self) -> String {
        match self {
            Self::Capabilities { .. } => "capabilities".into(),
            Self::Windows { .. } => "windows".into(),
            Self::Tree { .. } => "tree".into(),
            Self::Query { .. } => "query".into(),
            Self::Invoke { .. } => "invoke".into(),
            Self::MenuInspect { .. } => "menu-inspect".into(),
            Self::MenuInvoke { .. } => "menu-invoke".into(),
            Self::Focused { .. } => "focused".into(),
            Self::Observe { .. } => "observe".into(),
            Self::Verify { .. } => "verify".into(),
            Self::Screenshot { .. } => "screenshot".into(),
            Self::PointerMove { .. } => "pointer-move".into(),
            Self::PointerPosition { .. } => "pointer-position".into(),
            Self::Click { .. } => "click".into(),
            Self::Focus { .. } => "focus".into(),
            Self::SendText { .. } => "send-text".into(),
            Self::ClipboardRead { .. } => "clipboard-read".into(),
            Self::Copy { .. } => "copy".into(),
            Self::Paste { .. } => "paste".into(),
            Self::SendKeys { .. } => "send-keys".into(),
            Self::Scroll { .. } => "scroll".into(),
            Self::GetExtents { .. } => "get-extents".into(),
            Self::Select { .. } => "select".into(),
            Self::GetSelection { .. } => "get-selection".into(),
            Self::SetCaret { .. } => "set-caret".into(),
            Self::GetCaret { .. } => "get-caret".into(),
            Self::GetText { .. } => "get-text".into(),
            Self::Wait { .. } => "wait".into(),
            Self::WindowPlace { .. } => "window-place".into(),
        }
    }

    pub fn target(&self) -> TargetRef {
        match self {
            Self::Capabilities { target, .. }
            | Self::Windows { target, .. }
            | Self::Tree { target, .. }
            | Self::Query { target, .. }
            | Self::Invoke { target, .. }
            | Self::MenuInspect { target, .. }
            | Self::MenuInvoke { target, .. }
            | Self::Focused { target, .. }
            | Self::Observe { target, .. }
            | Self::Verify { target, .. }
            | Self::Screenshot { target, .. }
            | Self::PointerMove { target, .. }
            | Self::PointerPosition { target, .. }
            | Self::Click { target, .. }
            | Self::Focus { target, .. }
            | Self::SendText { target, .. }
            | Self::ClipboardRead { target, .. }
            | Self::Copy { target, .. }
            | Self::Paste { target, .. }
            | Self::SendKeys { target, .. }
            | Self::Scroll { target, .. }
            | Self::GetExtents { target, .. }
            | Self::Select { target, .. }
            | Self::GetSelection { target, .. }
            | Self::SetCaret { target, .. }
            | Self::GetCaret { target, .. }
            | Self::GetText { target, .. }
            | Self::Wait { target, .. }
            | Self::WindowPlace { target, .. } => *target,
        }
    }

    pub fn required_grant(&self) -> crate::auth::Grant {
        match self {
            Self::PointerMove { .. }
            | Self::Invoke { .. }
            | Self::MenuInvoke { .. }
            | Self::Click { .. }
            | Self::Focus { .. }
            | Self::SendText { .. }
            | Self::Copy { .. }
            | Self::Paste { .. }
            | Self::SendKeys { .. }
            | Self::Scroll { .. }
            | Self::Select { .. }
            | Self::SetCaret { .. }
            | Self::WindowPlace { .. } => crate::auth::Grant::Actuate,
            _ => crate::auth::Grant::Observe,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Grant;

    #[test]
    fn clipboard_read_is_target_neutral_observation() {
        let command = Command::ClipboardRead {
            target: TargetRef::Vnc,
        };
        assert_eq!(command.verb(), "clipboard-read");
        assert_eq!(command.target(), TargetRef::Vnc);
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({ "verb": "clipboard-read", "target": "vnc" })
        );
    }

    #[test]
    fn pointer_move_is_target_neutral_actuation_with_explicit_coordinates() {
        let command = Command::PointerMove {
            target: TargetRef::Ssh,
            x: -320,
            y: 1440,
        };
        assert_eq!(command.verb(), "pointer-move");
        assert_eq!(command.target(), TargetRef::Ssh);
        assert_eq!(command.required_grant(), Grant::Actuate);
        let json = serde_json::to_value(&command).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "verb": "pointer-move",
                "target": "ssh",
                "x": -320,
                "y": 1440
            })
        );
        let decoded: Command = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(
            decoded,
            Command::PointerMove {
                target: TargetRef::Ssh,
                x: -320,
                y: 1440
            }
        ));
    }

    #[test]
    fn tree_defaults_keep_the_pre_budget_wire_shape() {
        // A pre-1.12 caller's `{"verb":"tree","target":"current","window":7}`
        // still decodes, and a default tree still encodes to exactly that.
        let decoded: Command = serde_json::from_value(
            serde_json::json!({ "verb": "tree", "target": "current", "window": 7 }),
        )
        .expect("deserialize");
        assert!(matches!(
            decoded,
            Command::Tree {
                target: TargetRef::Current,
                window: Some(7),
                depth: None,
                max_nodes: None,
                flat: false,
            }
        ));
        assert_eq!(
            serde_json::to_value(&decoded).expect("serialize"),
            serde_json::json!({ "verb": "tree", "target": "current", "window": 7 })
        );
        let bounded = Command::Tree {
            target: TargetRef::Ssh,
            window: Some(7),
            depth: Some(3),
            max_nodes: Some(5),
            flat: true,
        };
        assert_eq!(bounded.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&bounded).expect("serialize"),
            serde_json::json!({
                "verb": "tree", "target": "ssh", "window": 7,
                "depth": 3, "max_nodes": 5, "flat": true
            })
        );
    }

    #[test]
    fn query_is_observation_and_round_trips_its_filters() {
        let command = Command::Query {
            target: TargetRef::Vnc,
            window: 14278,
            depth: Some(12),
            max_nodes: Some(500),
            role: vec!["AXTextArea".into(), "button".into()],
            text: Some("Fixture".into()),
            text_exact: None,
            identifier: None,
            actionable: true,
            within: Some([0, 0, 900, 700]),
            offset: Some(2),
            max: Some(10),
        };
        assert_eq!(command.verb(), "query");
        assert_eq!(command.target(), TargetRef::Vnc);
        assert_eq!(command.required_grant(), Grant::Observe);
        let json = serde_json::to_value(&command).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "verb": "query", "target": "vnc", "window": 14278,
                "depth": 12, "max_nodes": 500,
                "role": ["AXTextArea", "button"], "text": "Fixture",
                "actionable": true, "within": [0, 0, 900, 700],
                "offset": 2, "max": 10
            })
        );
        let decoded: Command = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.verb(), "query");
        // The minimal wire form decodes with every filter at its default.
        let minimal: Command = serde_json::from_value(
            serde_json::json!({ "verb": "query", "target": "current", "window": 1 }),
        )
        .expect("deserialize minimal");
        assert!(matches!(
            minimal,
            Command::Query { window: 1, actionable: false, ref role, .. } if role.is_empty()
        ));
    }

    #[test]
    fn windows_inventory_filters_default_to_the_bare_verb() {
        let bare: Command =
            serde_json::from_value(serde_json::json!({ "verb": "windows", "target": "current" }))
                .expect("deserialize");
        assert!(matches!(
            bare,
            Command::Windows {
                pid: None,
                app: None,
                title: None,
                focused: None,
                minimized: None,
                offset: None,
                max: None,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(&bare).expect("serialize"),
            serde_json::json!({ "verb": "windows", "target": "current" })
        );
        let filtered = Command::Windows {
            target: TargetRef::Current,
            pid: Some(4242),
            app: Some("TextEdit".into()),
            title: None,
            focused: Some(true),
            minimized: Some(false),
            offset: None,
            max: Some(1),
        };
        assert_eq!(filtered.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&filtered).expect("serialize"),
            serde_json::json!({
                "verb": "windows", "target": "current", "pid": 4242,
                "app": "TextEdit", "focused": true, "minimized": false, "max": 1
            })
        );
    }

    #[test]
    fn invoke_is_actuation_and_verify_is_observation() {
        let invoke = Command::Invoke {
            target: TargetRef::Current,
            window: 7,
            node: None,
            index: None,
            name: Some("Fixture Check".into()),
            identifier: None,
            role: Some("AXCheckBox".into()),
            action: InvokeAction::SetChecked,
            value: Some("true".into()),
            focused: false,
        };
        assert_eq!(invoke.verb(), "invoke");
        assert_eq!(invoke.required_grant(), Grant::Actuate);
        let json = serde_json::to_value(&invoke).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "verb": "invoke", "target": "current", "window": 7,
                "name": "Fixture Check", "role": "AXCheckBox",
                "action": "set-checked", "value": "true"
            })
        );
        let decoded: Command = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(
            decoded,
            Command::Invoke {
                action: InvokeAction::SetChecked,
                window: 7,
                ..
            }
        ));
        assert_eq!(
            InvokeAction::parse("select-option"),
            Some(InvokeAction::SelectOption)
        );
        assert_eq!(InvokeAction::parse("raise"), None);
        assert_eq!(InvokeAction::Press.value_kind(), InvokeValueKind::None);
        assert_eq!(InvokeAction::SetValue.value_kind(), InvokeValueKind::Text);
        assert_eq!(
            InvokeAction::SetExpanded.value_kind(),
            InvokeValueKind::Flag
        );

        let verify = Command::Verify {
            target: TargetRef::Ssh,
            window: 7,
            expect: vec![Expectation {
                identifier: Some("fixture-check".into()),
                checked: Some(true),
                ..Expectation::default()
            }],
        };
        assert_eq!(verify.verb(), "verify");
        assert_eq!(verify.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&verify).expect("serialize"),
            serde_json::json!({
                "verb": "verify", "target": "ssh", "window": 7,
                "expect": [{ "identifier": "fixture-check", "checked": true }]
            })
        );
    }

    #[test]
    fn background_verbs_have_grants_and_closed_wire_shapes() {
        let inspect = Command::MenuInspect {
            target: TargetRef::Current,
            window: 7,
            depth: Some(2),
            max_nodes: None,
            title: Some("Do".into()),
            exact: false,
            enabled: Some(true),
            offset: None,
            max: Some(20),
        };
        assert_eq!(inspect.verb(), "menu-inspect");
        assert_eq!(inspect.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&inspect).expect("serialize"),
            serde_json::json!({
                "verb": "menu-inspect", "target": "current", "window": 7,
                "depth": 2, "title": "Do", "enabled": true, "max": 20
            })
        );
        let invoke = Command::MenuInvoke {
            target: TargetRef::Ssh,
            window: 7,
            path: vec!["File".into(), "Do Thing".into()],
        };
        assert_eq!(invoke.verb(), "menu-invoke");
        assert_eq!(invoke.required_grant(), Grant::Actuate);
        assert_eq!(
            serde_json::to_value(&invoke).expect("serialize"),
            serde_json::json!({
                "verb": "menu-invoke", "target": "ssh", "window": 7,
                "path": ["File", "Do Thing"]
            })
        );
        let focused = Command::Focused {
            target: TargetRef::Current,
            window: 7,
            role: Some("AXTextField".into()),
            max_value_bytes: Some(0),
        };
        assert_eq!(focused.verb(), "focused");
        assert_eq!(focused.required_grant(), Grant::Observe);
        let observe: Command = serde_json::from_value(serde_json::json!({
            "verb": "observe", "target": "current", "window": 7, "duration_ms": 1500,
            "notifications": ["ValueChanged"], "max_events": 50
        }))
        .expect("deserialize");
        assert!(matches!(
            observe,
            Command::Observe { window: 7, duration_ms: 1500, max_events: Some(50), ref notifications, .. }
                if notifications == &["ValueChanged".to_owned()]
        ));
        assert_eq!(observe.required_grant(), Grant::Observe);
        // A pre-1.14 invoke wire form still decodes with `focused` false.
        let older: Command = serde_json::from_value(serde_json::json!({
            "verb": "invoke", "target": "current", "window": 7,
            "identifier": "fixture-press", "action": "press"
        }))
        .expect("deserialize");
        assert!(matches!(older, Command::Invoke { focused: false, .. }));
    }

    #[test]
    fn expectation_shape_is_closed() {
        let parsed: Expectation =
            serde_json::from_str(r#"{"name":"Fixture","role":"AXButton","value":"x"}"#)
                .expect("known keys parse");
        assert!(parsed.has_target() && parsed.has_state());
        let unknown = serde_json::from_str::<Expectation>(r#"{"name":"a","cheked":true}"#);
        assert!(unknown.is_err(), "a misspelled state must not parse");
        let wait = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 500,
            condition: WaitCondition::Expect {
                window: 3,
                expect: vec![Expectation {
                    node: Some("/0/1".into()),
                    value: Some("pressed 1".into()),
                    ..Expectation::default()
                }],
            },
        };
        assert_eq!(wait.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&wait).expect("serialize"),
            serde_json::json!({
                "verb": "wait", "target": "current", "timeout_ms": 500,
                "wait": "expect", "window": 3,
                "expect": [{ "node": "/0/1", "value": "pressed 1" }]
            })
        );
    }

    #[test]
    fn pointer_position_is_target_neutral_observation() {
        let command = Command::PointerPosition {
            target: TargetRef::Vnc,
        };
        assert_eq!(command.verb(), "pointer-position");
        assert_eq!(command.target(), TargetRef::Vnc);
        assert_eq!(command.required_grant(), Grant::Observe);
        assert_eq!(
            serde_json::to_value(&command).expect("serialize"),
            serde_json::json!({ "verb": "pointer-position", "target": "vnc" })
        );
    }
}
