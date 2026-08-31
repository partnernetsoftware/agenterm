//! Accessibility / control-tree facade.

pub use crate::contract::accessibility_tree::{
    AccessibilityBounds, AccessibilityEvent, AccessibilityMenuReceipt, AccessibilityNode,
    AccessibilityNodeAction, AccessibilitySelection, AccessibilityTree, AccessibilityTreeBudget,
    AccessibilityTreeError, MAX_OBSERVE_EVENTS, MAX_TREE_DEPTH_BUDGET, MAX_TREE_NODE_BUDGET,
};

/// `Available` when the host stack can be walked now. A stack that exists but
/// is refused by the OS (macOS Accessibility permission) answers
/// `Failed { code: "a11y_permission_denied", .. }` with the repair path in
/// the message, never `Unsupported` and never an empty tree.
pub fn capability_status() -> crate::CapabilityStatus {
    crate::selected::accessibility_tree::capability_status()
}

/// Walk with the adapter's own default bounds (see [`tree_for_window_bounded`]).
pub fn tree_for_window(
    window_handle: Option<isize>,
) -> Result<AccessibilityTree, AccessibilityTreeError> {
    tree_for_window_bounded(window_handle, AccessibilityTreeBudget::default())
}

/// Walk one window (or every root when `None`) under `budget`. Depth and node
/// budgets apply while the backend is being read; the result reports
/// `truncated` / `visited` / `returned`. An out-of-range budget is typed
/// `invalid_input` before any backend call.
pub fn tree_for_window_bounded(
    window_handle: Option<isize>,
    budget: AccessibilityTreeBudget,
) -> Result<AccessibilityTree, AccessibilityTreeError> {
    budget.validate()?;
    crate::selected::accessibility_tree::tree_for_window(window_handle, budget)
}

pub fn perform_node_action(
    window_handle: Option<isize>,
    node_id: &str,
    action: AccessibilityNodeAction,
) -> Result<(), AccessibilityTreeError> {
    crate::selected::accessibility_tree::perform_node_action(window_handle, node_id, action)
}

/// Walk the menu bar of the application owning `window_handle` under
/// `budget` (macOS: `AXMenuBar` → `AXMenuBarItem` → `AXMenu` → `AXMenuItem`)
/// without opening a menu on screen or activating the application. Node
/// ids are rooted at the menu bar (`/0`), a separate id space from the
/// window tree. Hosts without a background menu mechanism answer typed
/// `Unsupported`.
pub fn menu_tree_for_window(
    window_handle: Option<isize>,
    budget: AccessibilityTreeBudget,
) -> Result<AccessibilityTree, AccessibilityTreeError> {
    budget.validate()?;
    crate::selected::accessibility_tree::menu_tree_for_window(window_handle, budget)
}

/// Press the menu item at `path` (menu title, then item titles, exact) in
/// the application owning `window_handle`, in the background. Every
/// segment must resolve to exactly one enabled item before anything is
/// pressed (`a11y_menu_item_not_found` / `a11y_menu_item_ambiguous` /
/// `a11y_menu_item_disabled`), the last one must be a leaf
/// (`a11y_menu_item_not_leaf`), and a one-segment path is `invalid_input`
/// because pressing a bare menu bar item would open it on screen.
pub fn invoke_menu_path(
    window_handle: Option<isize>,
    path: &[String],
) -> Result<AccessibilityMenuReceipt, AccessibilityTreeError> {
    crate::selected::accessibility_tree::invoke_menu_path(window_handle, path)
}

/// The application's own focused control (macOS: `AXFocusedUIElement`) as
/// a node whose id is its child-index path below `window_handle`'s window,
/// read without requiring the application to be frontmost. No focused
/// element is `a11y_focus_unavailable`; one outside that window is
/// `a11y_focus_outside_window`.
pub fn focused_node_for_window(
    window_handle: Option<isize>,
) -> Result<AccessibilityNode, AccessibilityTreeError> {
    crate::selected::accessibility_tree::focused_node_for_window(window_handle)
}

/// Write `text` through the host accessibility text interface (Linux:
/// AT-SPI `EditableText` `SetTextContents` / `InsertText`, or AT-SPI `Text`
/// plus the toolkit set-value when EditableText is absent: Chrome renderer
/// AX or the WebKitGTK eval helper). Never injects
/// keystrokes. A node without a writeable text interface fails typed.
pub fn set_node_text(
    window_handle: Option<isize>,
    node_id: &str,
    text: &str,
) -> Result<(), AccessibilityTreeError> {
    crate::selected::accessibility_tree::set_node_text(window_handle, node_id, text)
}

/// Read the node's independent accessible text (Linux: AT-SPI `Text.GetText`).
/// This is not the resolve-time snapshot `text` field and is not the
/// `send-text` reply's `matched.text`. A node with no Text interface
/// fails typed.
pub fn get_node_text(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<String, AccessibilityTreeError> {
    crate::selected::accessibility_tree::get_node_text(window_handle, node_id)
}

/// One-shot AT-SPI `Component.ScrollTo(TopEdge)` (Linux). Missing / false /
/// `UnknownMethod` fails typed (`a11y_scroll_unavailable`). Never Action
/// `scroll*`, XTest wheel, or `GenerateMouseEvent`.
pub fn scroll_node(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<(), AccessibilityTreeError> {
    crate::selected::accessibility_tree::scroll_node(window_handle, node_id)
}

/// Independent AT-SPI `Component.GetExtents(Screen)` for one resolved
/// child-index path. Not a tree-snapshot `bounds` field. Empty extents
/// (width/height <= 0) or a failed GetExtents fail typed.
pub fn get_node_extents(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<AccessibilityBounds, AccessibilityTreeError> {
    crate::selected::accessibility_tree::get_node_extents(window_handle, node_id)
}

/// One-shot AT-SPI `Text.SetSelection(0, start, end)` (Linux). Missing
/// Text / `UnknownMethod` fails typed (`a11y_selection_unavailable`).
/// SetSelection false fails typed (`a11y_selection_no_effect`). Never
/// XTest, mouse-drag, or `--coords`.
pub fn set_node_selection(
    window_handle: Option<isize>,
    node_id: &str,
    start: i32,
    end: i32,
) -> Result<(), AccessibilityTreeError> {
    crate::selected::accessibility_tree::set_node_selection(window_handle, node_id, start, end)
}

/// Independent AT-SPI `Text.GetNSelections` + `GetSelection(0)` for one
/// resolved child-index path. Not the set-selection reply. Missing Text
/// fails typed (`a11y_selection_unavailable`). `n == 0` is empty success.
pub fn get_node_selection(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<AccessibilitySelection, AccessibilityTreeError> {
    crate::selected::accessibility_tree::get_node_selection(window_handle, node_id)
}

/// One-shot AT-SPI `Text.SetCaretOffset` (Linux). Missing Text /
/// `UnknownMethod` fails typed (`a11y_caret_unavailable`). SetCaretOffset
/// false fails typed (`a11y_caret_no_effect`). Never XTest, `--coords`,
/// or screenshot.
pub fn set_node_caret_offset(
    window_handle: Option<isize>,
    node_id: &str,
    offset: i32,
) -> Result<(), AccessibilityTreeError> {
    crate::selected::accessibility_tree::set_node_caret_offset(window_handle, node_id, offset)
}

/// Independent AT-SPI `Text.CaretOffset` / `GetCaretOffset` for one
/// resolved child-index path. Not the set-caret reply. Missing Text
/// fails typed (`a11y_caret_unavailable`).
pub fn get_node_caret_offset(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<i32, AccessibilityTreeError> {
    crate::selected::accessibility_tree::get_node_caret_offset(window_handle, node_id)
}

/// Route of the last successful `set_node_text` on this thread.
/// Linux: `"editable-text"` or `"text"`. Other hosts: `"editable-text"`.
pub fn last_text_write_via() -> &'static str {
    crate::selected::accessibility_tree::last_text_write_via()
}

/// Deliver `keys` through the host accessibility Device/key interface
/// (Linux: AT-SPI `DeviceEventListener` `NotifyEvent`). Never injects
/// XTest. A node without that interface fails typed.
pub fn send_node_keys(
    window_handle: Option<isize>,
    node_id: &str,
    keys: &str,
) -> Result<(), AccessibilityTreeError> {
    crate::selected::accessibility_tree::send_node_keys(window_handle, node_id, keys)
}

/// Ask the application owning `window_handle` to build its full
/// accessibility tree.
///
/// A browser engine keeps its web tree unbuilt until an assistive client
/// asks for it, so a Chromium or WebKit window answers a walk with a
/// handful of chrome nodes and no page: "empty chrome" is not an empty
/// page. macOS spells the request `AXManualAccessibility` on the owning
/// application element.
///
/// **The call's own status is not evidence.** AppKit answers
/// `kAXErrorAttributeUnsupported` for this attribute even when the poke
/// lands (measured on WebKit: three nodes before, fourteen after, same
/// `-25205`). A caller proves the poke by re-reading the tree, which is
/// what `agenterm-cu unlock` does.
pub fn poke_manual_accessibility(
    window_handle: Option<isize>,
) -> Result<(), AccessibilityTreeError> {
    crate::selected::accessibility_tree::poke_manual_accessibility(window_handle)
}

/// Watch one window for `duration`, collecting the events the backend
/// itself reports rather than the differences between two tree walks.
///
/// Blocking and bounded: it returns when the duration elapses or
/// `max_events` have arrived, whichever comes first. A backend with no
/// notification mechanism answers `Unsupported`, and the caller falls back
/// to poll-diff and says which mode it used -- the two are not equally
/// good and a reply that hid the difference would be the lie.
pub fn observe_window(
    window_handle: Option<isize>,
    duration: std::time::Duration,
    max_events: usize,
) -> Result<Vec<AccessibilityEvent>, AccessibilityTreeError> {
    crate::selected::accessibility_tree::observe_window(window_handle, duration, max_events)
}

pub fn drain_bus() {
    crate::selected::accessibility_tree::drain_bus()
}
