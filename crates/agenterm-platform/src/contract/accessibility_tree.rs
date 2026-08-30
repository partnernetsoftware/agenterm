//! Platform-neutral accessibility / control-tree contract.
//!
//! Windows maps to UIA, Linux to AT-SPI2, macOS to AX. Product callers use the
//! same node shape regardless of host backend.

use std::borrow::Cow;

/// Screen-space bounds in physical pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccessibilityBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Independent AT-SPI `Text` selection (`GetNSelections` + `GetSelection`).
/// `n == 0` is empty (start/end stay 0), not a missing interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccessibilitySelection {
    pub n: i32,
    pub start: i32,
    pub end: i32,
}

/// One node in a flattened accessibility tree.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccessibilityNode {
    /// Stable path id from the application root, e.g. `/0/2/5`.
    pub id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub name: String,
    pub states: Vec<String>,
    pub bounds: AccessibilityBounds,
    /// Action names exposed by the backend (`click`, `focus`, ...). An empty
    /// list means the backend reported none, never that it was not asked.
    pub actions: Vec<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub text: Option<String>,
    /// Toolkit identifier when the backend exposes one (macOS `AXIdentifier`).
    /// Distinct from `name`: it is an author-assigned handle, not a label.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub identifier: Option<String>,
}

/// Flattened control tree for one observation instant.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccessibilityTree {
    pub backend: &'static str,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub window_handle: Option<isize>,
    pub root_id: String,
    pub nodes: Vec<AccessibilityNode>,
    /// `true` when the walk stopped at the node or depth budget while the
    /// backend still had nodes to offer. `false` is a claim: every reachable
    /// node within the adapter's own limits is in `nodes`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub truncated: bool,
    /// Nodes the adapter read from the backend during this walk.
    #[cfg_attr(feature = "serde", serde(default))]
    pub visited: usize,
    /// Nodes in `nodes` (always `nodes.len()`; carried so ABI consumers that
    /// copy the metadata do not have to count).
    #[cfg_attr(feature = "serde", serde(default))]
    pub returned: usize,
}

/// Largest node budget a caller may request. Above this the walk is not a
/// bounded observation any more, so the contract refuses it typed.
pub const MAX_TREE_NODE_BUDGET: usize = 20_000;
/// Largest depth budget a caller may request (root is depth 0).
pub const MAX_TREE_DEPTH_BUDGET: u32 = 64;

/// Caller-supplied bounds for one tree walk. Both apply *during* traversal —
/// an adapter never builds an unbounded tree and prunes it afterwards. `None`
/// keeps the adapter's own default for that dimension.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AccessibilityTreeBudget {
    /// Deepest level to return (root = 0). Children below it are not fetched.
    pub max_depth: Option<u32>,
    /// Most nodes to return, `1..=MAX_TREE_NODE_BUDGET`.
    pub max_nodes: Option<usize>,
}

impl AccessibilityTreeBudget {
    /// Typed `invalid_input` for a budget outside the contract range.
    pub fn validate(&self) -> Result<(), AccessibilityTreeError> {
        if let Some(max_nodes) = self.max_nodes
            && (max_nodes == 0 || max_nodes > MAX_TREE_NODE_BUDGET)
        {
            return Err(AccessibilityTreeError::Failed {
                code: "invalid_input".into(),
                message: format!("max_nodes must be 1..={MAX_TREE_NODE_BUDGET}, got {max_nodes}"),
            });
        }
        if let Some(max_depth) = self.max_depth
            && max_depth > MAX_TREE_DEPTH_BUDGET
        {
            return Err(AccessibilityTreeError::Failed {
                code: "invalid_input".into(),
                message: format!("max_depth must be 0..={MAX_TREE_DEPTH_BUDGET}, got {max_depth}"),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AccessibilityNodeAction {
    Click,
    Focus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AccessibilityTreeError {
    Unsupported {
        reason: Cow<'static, str>,
    },
    Failed {
        code: Cow<'static, str>,
        message: String,
    },
}

impl AccessibilityTreeError {
    // Only a selected native backend constructs typed mechanism failures; the
    // neutral contract remains available when the selected backend is a stub.
    #[allow(dead_code)]
    pub(crate) fn failed(code: &'static str, message: impl ToString) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.to_string(),
        }
    }
}
