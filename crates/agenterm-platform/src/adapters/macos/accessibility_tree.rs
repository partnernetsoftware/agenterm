//! macOS Accessibility (AX) tree client — observe and semantic actuation.
//!
//! A `cfg(macos)` AX walk for `agenterm-cu --target current tree` / `query`
//! with typed permission / timeout / bound failures. The walk is bounded
//! while it reads: a caller's depth and node budget stop the breadth-first
//! traversal at the boundary and the reply says `truncated` instead of
//! failing or silently looking complete. Per-node actions come from
//! `AXUIElementCopyActionNames`. Live evidence: `scripts/qjs/cu-macos-smoke.qjs`.
//!
//! Actuation (slice 2 of `plan/design-mcu-absorption.md`) is semantic only:
//! `AXPress` / `AXIncrement` / `AXDecrement`, an `AXValue` write, a pop-up
//! option chosen by pressing the matching menu item, and desired-state
//! `set-checked` / `set-expanded` that read before acting and read back
//! after. Nothing here activates the application, raises a window
//! (`AXRaise` is never sent), moves the pointer, or posts a CGEvent; a node
//! that does not offer an action answers a typed `Unsupported`. No
//! screenshot fallback and no silent reuse of AT-SPI or UIA.
//!
//! Slice 3 adds the background half of the same discipline: the
//! application's `AXMenuBar` is walked and a menu path is pressed without
//! opening a menu on screen or activating the app (`menu_tree_for_window` /
//! `invoke_menu_path`), and the application's own `AXFocusedUIElement` is
//! read back as a window-relative node (`focused_node_for_window`).

#![cfg(target_os = "macos")]

use std::collections::VecDeque;
use std::ffi::{CStr, c_void};
use std::time::{Duration, Instant};

use crate::CapabilityStatus;
use crate::contract::accessibility_tree::{
    AccessibilityBounds, AccessibilityMenuReceipt, AccessibilityNode, AccessibilityNodeAction,
    AccessibilitySelection, AccessibilityTree, AccessibilityTreeBudget, AccessibilityTreeError,
};

type CfTypeRef = *const c_void;
type CfArrayRef = *const c_void;
type CfDictionaryRef = *const c_void;
type CfStringRef = *const c_void;
type CfIndex = isize;
type AxUiElementRef = *const c_void;
type AxValueRef = *const c_void;
type CgWindowId = u32;

/// Default node budget when the caller names none. A larger tree is not an
/// error: the walk stops here and reports `truncated`.
const MAX_NODES: usize = 1_000;
/// Default depth budget (root = 0) when the caller names none.
const MAX_DEPTH: usize = 32;
/// Where to grant the permission this adapter needs. Quoted verbatim in the
/// typed denial so an agent can relay it without guessing.
const ACCESSIBILITY_REPAIR_PATH: &str = "System Settings > Privacy & Security > Accessibility: enable the process that runs agenterm-cu (or its parent terminal / launcher), then rerun";
const MAX_SIBLINGS_PER_LEVEL: usize = 1_000;
const MAX_NODE_ID_BYTES: usize = 4_096;
const MAX_STRING_BYTES: usize = 16 * 1024;
const MAX_TOTAL_STRING_BYTES: usize = 2 * 1024 * 1024;
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
/// Wall-clock bound for one actuation including its read-back polls.
const ACTION_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a desired-state action waits for the toolkit to publish the new
/// state before calling the action ineffective.
const READBACK_WINDOW: Duration = Duration::from_millis(1_500);
const READBACK_POLL: Duration = Duration::from_millis(25);
/// How deep below a pop-up the option search looks (`AXMenu` → `AXMenuItem`
/// is two levels; one spare for a grouped menu).
const OPTION_SEARCH_DEPTH: usize = 3;
/// Largest `set-value` payload, mirroring the Windows adapter's bound.
const MAX_SET_VALUE_BYTES: usize = 64 * 1024;

const AX_SUCCESS: i32 = 0;
/// `kAXErrorFailure`: AppKit answers this for an attribute an element does
/// not provide (an NSScrollView scroller's `AXDescription`, for one). On an
/// attribute read it means "no value here", the same as unsupported.
const AX_ERROR_FAILURE: i32 = -25200;
const AX_ERROR_API_DISABLED: i32 = -25211;
const AX_ERROR_INVALID_UI_ELEMENT: i32 = -25202;
const AX_ERROR_ATTRIBUTE_UNSUPPORTED: i32 = -25205;
const AX_ERROR_CANNOT_COMPLETE: i32 = -25204;
const AX_ERROR_NOT_IMPLEMENTED: i32 = -25208;
const AX_ERROR_NO_VALUE: i32 = -25212;

/// How many `AXParent` hops a focused element may be below its window
/// before the adapter stops looking for the window.
const MAX_FOCUS_ANCESTORS: usize = 64;

const AX_VALUE_CGPOINT: u32 = 1;
const AX_VALUE_CGSIZE: u32 = 2;
/// `kAXValueCFRangeType`: how AX carries a text selection / insertion point.
const AX_VALUE_CFRANGE: u32 = 4;

/// Longest chord accepted, mirroring the Windows adapter's bound.
const MAX_KEYS_BYTES: usize = 256;

const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT32: i32 = 3;
const K_CF_NUMBER_SINT64: i32 = 4;
const K_CF_NUMBER_DOUBLE: i32 = 13;

#[repr(C)]
#[derive(Clone, Copy)]
struct CgPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CfRange {
    location: CfIndex,
    length: CfIndex,
}

// One `#[link]` per framework is the documented way to attach several of them
// to a single extern block; clippy reads the repeated attribute name as a
// copy-paste slip. Same false positive as foreign_windows.rs / hotkeys.rs.
#[allow(clippy::duplicated_attributes)]
#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CfArrayRef;

    fn CFRelease(cf: CfTypeRef);
    fn CFRetain(cf: CfTypeRef) -> CfTypeRef;
    fn CFArrayGetCount(array: CfArrayRef) -> CfIndex;
    fn CFArrayGetValueAtIndex(array: CfArrayRef, idx: CfIndex) -> CfTypeRef;
    fn CFDictionaryGetValue(dict: CfDictionaryRef, key: CfTypeRef) -> CfTypeRef;
    fn CFStringCreateWithCString(alloc: CfTypeRef, c_str: *const i8, encoding: u32) -> CfStringRef;
    fn CFStringGetCStringPtr(s: CfStringRef, encoding: u32) -> *const i8;
    fn CFStringGetCString(s: CfStringRef, buf: *mut i8, size: CfIndex, encoding: u32) -> bool;
    fn CFNumberGetValue(number: CfTypeRef, the_type: CfIndex, value_ptr: *mut c_void) -> bool;
    fn CFGetTypeID(cf: CfTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(boolean: CfTypeRef) -> u8;
    fn CFNumberGetTypeID() -> usize;

    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> AxUiElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AxUiElementRef,
        attribute: CfStringRef,
        value: *mut CfTypeRef,
    ) -> i32;
    fn AXUIElementCopyAttributeNames(element: AxUiElementRef, names: *mut CfTypeRef) -> i32;
    fn AXUIElementCopyActionNames(element: AxUiElementRef, names: *mut CfTypeRef) -> i32;
    fn AXValueGetValue(value: AxValueRef, typ: u32, value_ptr: *mut c_void) -> u8;
    fn AXValueCreate(typ: u32, value_ptr: *const c_void) -> AxValueRef;
    fn _AXUIElementGetWindow(element: AxUiElementRef, out: *mut CgWindowId) -> i32;

    fn AXUIElementPerformAction(element: AxUiElementRef, action: CfStringRef) -> i32;
    fn AXUIElementSetAttributeValue(
        element: AxUiElementRef,
        attribute: CfStringRef,
        value: CfTypeRef,
    ) -> i32;
    fn AXUIElementIsAttributeSettable(
        element: AxUiElementRef,
        attribute: CfStringRef,
        settable: *mut u8,
    ) -> i32;
    fn CFNumberCreate(alloc: CfTypeRef, the_type: CfIndex, value_ptr: *const c_void) -> CfTypeRef;
    fn CFEqual(a: CfTypeRef, b: CfTypeRef) -> u8;
    static kCFBooleanTrue: CfTypeRef;
}

struct CfOwned(CfTypeRef);

impl CfOwned {
    fn from_create(ptr: CfTypeRef) -> Option<Self> {
        if ptr.is_null() { None } else { Some(Self(ptr)) }
    }

    fn retain(ptr: CfTypeRef) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            unsafe {
                CFRetain(ptr);
            }
            Some(Self(ptr))
        }
    }

    fn as_ptr(&self) -> CfTypeRef {
        self.0
    }

    fn as_ax(&self) -> AxUiElementRef {
        self.0 as AxUiElementRef
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CFRelease(self.0);
            }
            self.0 = std::ptr::null();
        }
    }
}

struct Budget {
    deadline: Instant,
    total_string_bytes: usize,
}

impl Budget {
    fn new(duration: Duration) -> Self {
        Self {
            deadline: Instant::now() + duration,
            total_string_bytes: 0,
        }
    }

    fn check(&self) -> Result<(), AccessibilityTreeError> {
        if Instant::now() >= self.deadline {
            return Err(AccessibilityTreeError::failed(
                "a11y_tree_timeout",
                "AX tree snapshot exceeded its wall-clock deadline",
            ));
        }
        Ok(())
    }

    fn account_string(&mut self, value: &str) -> Result<(), AccessibilityTreeError> {
        if value.len() > MAX_STRING_BYTES {
            return Err(limit_error(
                "a11y_string_limit",
                format!("AX string exceeds {MAX_STRING_BYTES} UTF-8 bytes"),
            ));
        }
        self.total_string_bytes = self
            .total_string_bytes
            .checked_add(value.len())
            .ok_or_else(|| limit_error("a11y_string_limit", "string-byte budget overflow"))?;
        if self.total_string_bytes > MAX_TOTAL_STRING_BYTES {
            return Err(limit_error(
                "a11y_string_limit",
                format!("AX tree exceeds {MAX_TOTAL_STRING_BYTES} aggregate string bytes"),
            ));
        }
        Ok(())
    }

    fn account_node(&mut self, node: &AccessibilityNode) -> Result<(), AccessibilityTreeError> {
        self.account_string(&node.id)?;
        if let Some(parent) = &node.parent_id {
            self.account_string(parent)?;
        }
        self.account_string(&node.role)?;
        self.account_string(&node.name)?;
        for state in &node.states {
            self.account_string(state)?;
        }
        for action in &node.actions {
            self.account_string(action)?;
        }
        if let Some(text) = &node.text {
            self.account_string(text)?;
        }
        Ok(())
    }
}

fn limit_error(code: &'static str, message: impl ToString) -> AccessibilityTreeError {
    AccessibilityTreeError::failed(code, message)
}

fn cfstr(name: &str) -> CfStringRef {
    let c = std::ffi::CString::new(name).expect("AX attribute key must not contain NUL");
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
}

fn cf_string(value: CfTypeRef) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe {
        if CFGetTypeID(value) != CFStringGetTypeID() {
            return String::new();
        }
        let ptr = CFStringGetCStringPtr(value as CfStringRef, K_CF_STRING_ENCODING_UTF8);
        if !ptr.is_null() {
            return CStr::from_ptr(ptr).to_string_lossy().into_owned();
        }
        let mut buf = [0i8; 4096];
        if CFStringGetCString(
            value as CfStringRef,
            buf.as_mut_ptr(),
            buf.len() as CfIndex,
            K_CF_STRING_ENCODING_UTF8,
        ) {
            return CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
        }
    }
    String::new()
}

fn cf_i64(value: CfTypeRef) -> Option<i64> {
    if value.is_null() {
        return None;
    }
    let mut out = 0i64;
    let ok = unsafe {
        CFNumberGetValue(
            value,
            K_CF_NUMBER_SINT64 as CfIndex,
            &mut out as *mut i64 as *mut c_void,
        )
    };
    if ok {
        return Some(out);
    }
    let mut out32 = 0i32;
    let ok = unsafe {
        CFNumberGetValue(
            value,
            K_CF_NUMBER_SINT32 as CfIndex,
            &mut out32 as *mut i32 as *mut c_void,
        )
    };
    if ok { Some(i64::from(out32)) } else { None }
}

fn dict_get(dict: CfDictionaryRef, key: &str) -> CfTypeRef {
    unsafe {
        let k = cfstr(key);
        let v = CFDictionaryGetValue(dict, k as CfTypeRef);
        CFRelease(k as CfTypeRef);
        v
    }
}

fn map_ax_status(status: i32, operation: &str) -> Result<(), AccessibilityTreeError> {
    if status == AX_SUCCESS {
        return Ok(());
    }
    let (code, detail) = match status {
        AX_ERROR_API_DISABLED => (
            "a11y_permission_denied",
            "Accessibility permission is not granted for this process",
        ),
        AX_ERROR_INVALID_UI_ELEMENT => ("a11y_node_recycled", "AX element is no longer valid"),
        AX_ERROR_ATTRIBUTE_UNSUPPORTED | AX_ERROR_NOT_IMPLEMENTED | AX_ERROR_NO_VALUE => {
            return Err(AccessibilityTreeError::Unsupported {
                reason: format!("{operation}: AX attribute unavailable (status {status})").into(),
            });
        }
        AX_ERROR_CANNOT_COMPLETE => (
            "a11y_tree_timeout",
            "AX could not complete the request within its provider bound",
        ),
        _ => (
            "a11y_backend_failed",
            "AX call failed with an unexpected status",
        ),
    };
    Err(AccessibilityTreeError::failed(
        code,
        format!("{operation}: {detail} (AXError {status})"),
    ))
}

fn permission_denied() -> AccessibilityTreeError {
    AccessibilityTreeError::failed(
        "a11y_permission_denied",
        format!(
            "AXIsProcessTrusted() is false: Accessibility permission is not granted. {ACCESSIBILITY_REPAIR_PATH}"
        ),
    )
}

fn require_trusted() -> Result<(), AccessibilityTreeError> {
    if unsafe { AXIsProcessTrusted() } == 0 {
        return Err(permission_denied());
    }
    Ok(())
}

/// The AX mechanism is compiled into this adapter, so it is never
/// `Unsupported` here. Without Accessibility permission the OS refuses every
/// AX read, which is a typed denial carrying the repair path — not an empty
/// tree and not a missing adapter.
pub(crate) fn capability_status() -> CapabilityStatus {
    match require_trusted() {
        Ok(()) => CapabilityStatus::Available,
        Err(AccessibilityTreeError::Failed { code, message }) => {
            CapabilityStatus::Failed { code, message }
        }
        Err(AccessibilityTreeError::Unsupported { reason }) => {
            CapabilityStatus::Unsupported { reason }
        }
    }
}

/// `None` walks every on-screen CG window under the same node/depth/string/time
/// bounds as a window-scoped snapshot. `Some(handle)` scopes to that CGWindowID.
///
/// `budget` applies while reading: no child is fetched below `max_depth`, and
/// the breadth-first walk stops once `max_nodes` nodes are read. Either cut
/// sets `truncated`; `visited` counts nodes read from AX.
pub(crate) fn tree_for_window(
    window_handle: Option<isize>,
    budget_request: AccessibilityTreeBudget,
) -> Result<AccessibilityTree, AccessibilityTreeError> {
    require_trusted()?;
    let max_nodes = budget_request.max_nodes.unwrap_or(MAX_NODES);
    let max_depth = budget_request
        .max_depth
        .map(|depth| depth as usize)
        .unwrap_or(MAX_DEPTH);
    let mut budget = Budget::new(SNAPSHOT_TIMEOUT);
    let roots = resolve_roots(window_handle, &mut budget)?;
    if roots.is_empty() {
        return Err(AccessibilityTreeError::failed(
            "a11y_tree_empty",
            match window_handle {
                Some(handle) => format!("no AX window matched CGWindowID {handle}"),
                None => "no on-screen AX windows were found".to_owned(),
            },
        ));
    }

    let (nodes, truncated) = walk_bounded(roots, max_nodes, max_depth, &mut budget)?;
    finish_tree(window_handle, nodes, truncated)
}

/// Breadth-first walk of `roots` under the node / depth budget, reading each
/// element once. Returns the nodes in walk order and whether a budget cut
/// the walk short.
fn walk_bounded(
    roots: Vec<CfOwned>,
    max_nodes: usize,
    max_depth: usize,
    budget: &mut Budget,
) -> Result<(Vec<AccessibilityNode>, bool), AccessibilityTreeError> {
    let mut truncated = false;
    let mut nodes = Vec::new();
    let mut queue: VecDeque<(CfOwned, String, Option<String>, usize)> = VecDeque::new();
    for (index, root) in roots.into_iter().enumerate() {
        let id = format!("/{index}");
        budget.account_string(&id)?;
        queue.push_back((root, id, None, 0));
    }

    while let Some((element, id, parent_id, depth)) = queue.pop_front() {
        budget.check()?;
        if nodes.len() >= max_nodes {
            // Something was still queued: the node budget cut the walk.
            truncated = true;
            break;
        }
        if id.len() > MAX_NODE_ID_BYTES {
            return Err(limit_error(
                "a11y_node_id_limit",
                format!("AX node id exceeds {MAX_NODE_ID_BYTES} bytes"),
            ));
        }

        let node = match read_node(element.as_ax(), id.clone(), parent_id.clone(), budget) {
            Ok(node) => node,
            Err(error) if parent_id.is_some() && is_snapshot_branch_loss(&error) => continue,
            Err(error) => return Err(error),
        };
        budget.account_node(&node)?;
        nodes.push(node);

        let children = match copy_children(element.as_ax(), budget) {
            Ok(children) => children,
            Err(error) if parent_id.is_some() && is_snapshot_branch_loss(&error) => continue,
            Err(error) => return Err(error),
        };
        if !children.is_empty() && depth >= max_depth {
            // Children exist below the depth budget; they are not fetched.
            truncated = true;
            continue;
        }
        if children.len() > MAX_SIBLINGS_PER_LEVEL {
            return Err(limit_error(
                "a11y_node_limit",
                format!("AX node has more than {MAX_SIBLINGS_PER_LEVEL} children"),
            ));
        }
        for (child_index, child) in children.into_iter().enumerate() {
            budget.check()?;
            if nodes.len().saturating_add(queue.len()) >= max_nodes {
                // No room left to even queue this child: the node budget
                // cuts the walk here.
                truncated = true;
                break;
            }
            let child_id = format!("{id}/{child_index}");
            queue.push_back((child, child_id, Some(id.clone()), depth + 1));
        }
    }

    Ok((nodes, truncated))
}

fn finish_tree(
    window_handle: Option<isize>,
    nodes: Vec<AccessibilityNode>,
    truncated: bool,
) -> Result<AccessibilityTree, AccessibilityTreeError> {
    if nodes.is_empty() {
        return Err(AccessibilityTreeError::failed(
            "a11y_tree_empty",
            "AX returned no nodes",
        ));
    }

    let root_id = nodes
        .first()
        .map(|node| node.id.clone())
        .unwrap_or_else(|| "/0".to_owned());

    let returned = nodes.len();
    Ok(AccessibilityTree {
        backend: "ax",
        window_handle,
        root_id,
        nodes,
        truncated,
        visited: returned,
        returned,
    })
}

pub(crate) fn drain_bus() {}

/// The application element that owns `handle`, plus its pid.
fn application_for_handle(
    handle: isize,
    budget: &mut Budget,
) -> Result<(CfOwned, u32), AccessibilityTreeError> {
    budget.check()?;
    let pid = owner_pid(handle)?;
    let app = unsafe { AXUIElementCreateApplication(pid as i32) };
    let app = CfOwned::from_create(app as CfTypeRef).ok_or_else(|| {
        AccessibilityTreeError::failed(
            "a11y_backend_failed",
            "AXUIElementCreateApplication returned null",
        )
    })?;
    Ok((app, pid))
}

fn require_handle(
    window_handle: Option<isize>,
    what: &str,
) -> Result<isize, AccessibilityTreeError> {
    window_handle.ok_or_else(|| {
        AccessibilityTreeError::failed(
            "invalid_input",
            format!("{what} needs a window handle to name the application"),
        )
    })
}

/// The application's `AXMenuBar` element, or a typed failure when the
/// application publishes none.
fn menu_bar_for_handle(
    handle: isize,
    budget: &mut Budget,
) -> Result<(CfOwned, u32), AccessibilityTreeError> {
    let (app, pid) = application_for_handle(handle, budget)?;
    let Some(bar) = copy_attribute(app.as_ax(), "AXMenuBar", budget)? else {
        return Err(AccessibilityTreeError::failed(
            "a11y_menu_unavailable",
            format!("pid {pid} publishes no AXMenuBar"),
        ));
    };
    Ok((bar, pid))
}

/// Walk the menu bar of the application owning `window_handle` under
/// `budget`, without opening a menu on screen or activating the
/// application: AppKit publishes a closed menu's `AXMenu` / `AXMenuItem`
/// children through AX. Node ids are rooted at the menu bar (`/0`), a
/// separate id space from the window tree.
pub(crate) fn menu_tree_for_window(
    window_handle: Option<isize>,
    budget_request: AccessibilityTreeBudget,
) -> Result<AccessibilityTree, AccessibilityTreeError> {
    require_trusted()?;
    let handle = require_handle(window_handle, "menu walk")?;
    let max_nodes = budget_request.max_nodes.unwrap_or(MAX_NODES);
    let max_depth = budget_request
        .max_depth
        .map(|depth| depth as usize)
        .unwrap_or(MAX_DEPTH);
    let mut budget = Budget::new(SNAPSHOT_TIMEOUT);
    let (bar, _pid) = menu_bar_for_handle(handle, &mut budget)?;
    let (nodes, truncated) = walk_bounded(vec![bar], max_nodes, max_depth, &mut budget)?;
    finish_tree(window_handle, nodes, truncated)
}

/// The unique `AXMenu` child of a menu bar item / menu item, or `None`
/// when the item opens no submenu.
fn submenu_of(
    item: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Option<CfOwned>, AccessibilityTreeError> {
    let mut menus: Vec<CfOwned> = Vec::new();
    for child in copy_children(item, budget)? {
        let role = attribute_string(child.as_ax(), "AXRole", budget)?.unwrap_or_default();
        if normalize_role(&role) == "menu" {
            menus.push(child);
        }
    }
    Ok(menus.into_iter().next())
}

/// Resolve `path` (menu bar item title, then item titles) segment by
/// segment, requiring exactly one enabled match at each level. Every
/// refusal happens before anything is pressed.
fn resolve_menu_path(
    bar: AxUiElementRef,
    path: &[String],
    budget: &mut Budget,
) -> Result<CfOwned, AccessibilityTreeError> {
    let mut container = CfOwned::retain(bar as CfTypeRef).ok_or_else(|| {
        AccessibilityTreeError::failed("a11y_backend_failed", "menu bar element is null")
    })?;
    let mut current: Option<CfOwned> = None;
    for (level, segment) in path.iter().enumerate() {
        budget.check()?;
        let walked = path[..level].join("/");
        if level > 0 {
            let item = current
                .take()
                .expect("a resolved item precedes every later level");
            container = submenu_of(item.as_ax(), budget)?.ok_or_else(|| {
                AccessibilityTreeError::failed(
                    "a11y_menu_item_not_found",
                    format!("menu item {walked:?} opens no submenu"),
                )
            })?;
        }
        let mut hits: Vec<CfOwned> = Vec::new();
        for child in copy_children(container.as_ax(), budget)? {
            let title = attribute_string(child.as_ax(), "AXTitle", budget)?.unwrap_or_default();
            if &title == segment {
                hits.push(child);
            }
        }
        let scope = if walked.is_empty() {
            "the menu bar".to_owned()
        } else {
            format!("menu {walked:?}")
        };
        let item = match hits.len() {
            1 => hits.pop().expect("one hit"),
            0 => {
                return Err(AccessibilityTreeError::failed(
                    "a11y_menu_item_not_found",
                    format!("no item titled {segment:?} in {scope}"),
                ));
            }
            count => {
                return Err(AccessibilityTreeError::failed(
                    "a11y_menu_item_ambiguous",
                    format!("{count} items titled {segment:?} in {scope}; refusing to guess"),
                ));
            }
        };
        if attribute_bool(item.as_ax(), "AXEnabled", budget)? == Some(false) {
            return Err(AccessibilityTreeError::failed(
                "a11y_menu_item_disabled",
                format!("menu item {segment:?} in {scope} is disabled"),
            ));
        }
        current = Some(item);
    }
    Ok(current.expect("a non-empty path resolves to an item"))
}

fn menu_mark(
    item: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Option<String>, AccessibilityTreeError> {
    Ok(attribute_string(item, "AXMenuItemMarkChar", budget)?.filter(|mark| !mark.is_empty()))
}

/// Press the menu item at `path` in the application owning `window_handle`
/// without opening a menu or activating the application. The path must
/// name at least a menu and one item (pressing a bare menu bar item would
/// open it on screen), every segment must resolve to exactly one enabled
/// item, and the final item must be a leaf (`a11y_menu_item_not_leaf`
/// otherwise). The receipt carries the item's mark before and after.
pub(crate) fn invoke_menu_path(
    window_handle: Option<isize>,
    path: &[String],
) -> Result<AccessibilityMenuReceipt, AccessibilityTreeError> {
    require_trusted()?;
    let handle = require_handle(window_handle, "menu invoke")?;
    if path.len() < 2 || path.iter().any(|segment| segment.is_empty()) {
        return Err(AccessibilityTreeError::failed(
            "invalid_input",
            "a menu path needs a menu title and at least one non-empty item title",
        ));
    }
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let (bar, _pid) = menu_bar_for_handle(handle, &mut budget)?;
    let item = resolve_menu_path(bar.as_ax(), path, &mut budget)?;
    if submenu_of(item.as_ax(), &mut budget)?.is_some() {
        return Err(AccessibilityTreeError::failed(
            "a11y_menu_item_not_leaf",
            format!(
                "menu item {:?} opens a submenu; name one of its items",
                path.join("/")
            ),
        ));
    }
    let mark_before = menu_mark(item.as_ax(), &mut budget)?;
    perform_named_action(item.as_ax(), "AXPress", &mut budget)?;
    // Re-resolve rather than trust the pressed element: a menu that
    // rebuilt itself publishes fresh elements.
    let mark_after = match resolve_menu_path(bar.as_ax(), path, &mut budget) {
        Ok(again) => menu_mark(again.as_ax(), &mut budget)?,
        Err(_) => None,
    };
    Ok(AccessibilityMenuReceipt {
        mark_before,
        mark_after,
    })
}

/// The window-relative child-index path of `element`, found by walking
/// `AXParent` up to `window` and locating each hop in its parent's
/// `AXChildren` (`CFEqual`). `None` when `window` is not an ancestor.
fn path_below_window(
    window: AxUiElementRef,
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Option<String>, AccessibilityTreeError> {
    let mut chain: Vec<CfOwned> = Vec::new();
    let mut current = CfOwned::retain(element as CfTypeRef).ok_or_else(|| {
        AccessibilityTreeError::failed("a11y_backend_failed", "focused element is null")
    })?;
    let mut hops = 0usize;
    loop {
        budget.check()?;
        if unsafe { CFEqual(current.as_ptr(), window as CfTypeRef) } != 0 {
            break;
        }
        hops += 1;
        if hops > MAX_FOCUS_ANCESTORS {
            return Ok(None);
        }
        let Some(parent) = copy_attribute(current.as_ax(), "AXParent", budget)? else {
            return Ok(None);
        };
        chain.push(current);
        current = parent;
    }
    // `current` is the window; `chain` holds element .. child-of-window.
    let mut id = String::from("/0");
    let mut parent = current;
    for hop in chain.into_iter().rev() {
        budget.check()?;
        let children = copy_children(parent.as_ax(), budget)?;
        let Some(index) = children
            .iter()
            .position(|child| unsafe { CFEqual(child.as_ptr(), hop.as_ptr()) } != 0)
        else {
            return Ok(None);
        };
        id.push('/');
        id.push_str(&index.to_string());
        parent = hop;
    }
    Ok(Some(id))
}

/// The application's own focused element (`AXFocusedUIElement`) as a node
/// whose id is the child-index path below `window_handle`'s AX window —
/// the same numbering `tree` uses — without requiring the application to
/// be frontmost. No focused element is `a11y_focus_unavailable`; a focused
/// element outside that window is `a11y_focus_outside_window`.
pub(crate) fn focused_node_for_window(
    window_handle: Option<isize>,
) -> Result<AccessibilityNode, AccessibilityTreeError> {
    require_trusted()?;
    let handle = require_handle(window_handle, "focused read")?;
    let mut budget = Budget::new(SNAPSHOT_TIMEOUT);
    let window = ax_element_for_handle(handle, &mut budget)?;
    let (app, pid) = application_for_handle(handle, &mut budget)?;
    let Some(focused) = copy_attribute(app.as_ax(), "AXFocusedUIElement", &mut budget)? else {
        return Err(AccessibilityTreeError::failed(
            "a11y_focus_unavailable",
            format!("pid {pid} publishes no AXFocusedUIElement"),
        ));
    };
    let Some(id) = path_below_window(window.as_ax(), focused.as_ax(), &mut budget)? else {
        return Err(AccessibilityTreeError::failed(
            "a11y_focus_outside_window",
            format!("pid {pid}'s focused element is not inside window {handle}"),
        ));
    };
    let parent_id = id
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_owned())
        .filter(|parent| !parent.is_empty());
    read_node(focused.as_ax(), id, parent_id, &mut budget)
}

/// Perform one semantic action on the element at `node_id` (a child-index
/// path from the window root, as `tree` numbers it). Resolution happens at
/// call time, so a stale path is `a11y_node_not_found`, never a guess.
///
/// Background invariant: nothing here activates the application or raises
/// the window. `Focus` writes `AXFocused` on the element, which moves the
/// first responder *inside* the owning application only.
pub(crate) fn perform_node_action(
    window_handle: Option<isize>,
    node_id: &str,
    action: AccessibilityNodeAction,
) -> Result<(), AccessibilityTreeError> {
    require_trusted()?;
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    match action {
        AccessibilityNodeAction::Click | AccessibilityNodeAction::Press => {
            perform_named_action(element.as_ax(), "AXPress", &mut budget)
        }
        AccessibilityNodeAction::Increment => {
            perform_named_action(element.as_ax(), "AXIncrement", &mut budget)
        }
        AccessibilityNodeAction::Decrement => {
            perform_named_action(element.as_ax(), "AXDecrement", &mut budget)
        }
        AccessibilityNodeAction::Focus => focus_element(element.as_ax(), &mut budget),
        AccessibilityNodeAction::SetValue(value) => set_value(element.as_ax(), &value, &mut budget),
        AccessibilityNodeAction::SelectOption(option) => {
            select_option(element.as_ax(), &option, &mut budget)
        }
        AccessibilityNodeAction::SetChecked(desired) => {
            set_checked(element.as_ax(), desired, &mut budget)
        }
        AccessibilityNodeAction::SetExpanded(desired) => {
            set_expanded(element.as_ax(), desired, &mut budget)
        }
        // The contract is `non_exhaustive`; a variant this adapter does not
        // know is typed, not silently mapped to something else.
        #[allow(unreachable_patterns)]
        other => Err(AccessibilityTreeError::Unsupported {
            reason: format!("macOS AX has no mapping for action {}", other.name()).into(),
        }),
    }
}

/// `AXValue` write for `send-text --name` / `paste`: the same settable-check
/// plus read-back as `set-value`.
pub(crate) fn set_node_text(
    window_handle: Option<isize>,
    node_id: &str,
    text: &str,
) -> Result<(), AccessibilityTreeError> {
    require_trusted()?;
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    set_value(element.as_ax(), text, &mut budget)
}

/// Independent `AXValue` read for `get-text`: a text value verbatim, a
/// numeric value as decimal text, no value as `a11y_text_unavailable`.
pub(crate) fn get_node_text(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<String, AccessibilityTreeError> {
    require_trusted()?;
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    match read_ax_value(element.as_ax(), &mut budget)? {
        AxValue::Text(text) => Ok(text),
        AxValue::Number(number) => Ok(format_number(number)),
        AxValue::Bool(flag) => Ok(if flag { "1" } else { "0" }.to_owned()),
        AxValue::None | AxValue::Other => Err(AccessibilityTreeError::failed(
            "a11y_text_unavailable",
            format!("node {node_id} has no readable AXValue text"),
        )),
    }
}

/// Parse `/0/2/5` into child indices. The first index selects the root
/// (`0` for a window-scoped call).
fn parse_node_path(node_id: &str) -> Result<Vec<usize>, AccessibilityTreeError> {
    let invalid = |detail: &str| {
        AccessibilityTreeError::failed(
            "invalid_input",
            format!("node id {node_id:?} is not a child-index path: {detail}"),
        )
    };
    if node_id.len() > MAX_NODE_ID_BYTES {
        return Err(invalid("too long"));
    }
    let Some(rest) = node_id.strip_prefix('/') else {
        return Err(invalid("must start with '/'"));
    };
    if rest.is_empty() {
        return Err(invalid("empty path"));
    }
    rest.split('/')
        .map(|part| {
            part.parse::<usize>()
                .map_err(|_| invalid(&format!("segment {part:?} is not an index")))
        })
        .collect()
}

/// Resolve a child-index path to a live element, walking `AXChildren` one
/// level per segment (no whole-tree snapshot).
fn resolve_node(
    window_handle: Option<isize>,
    node_id: &str,
    budget: &mut Budget,
) -> Result<CfOwned, AccessibilityTreeError> {
    let indices = parse_node_path(node_id)?;
    let roots = resolve_roots(window_handle, budget)?;
    let not_found = |detail: String| {
        AccessibilityTreeError::failed(
            "a11y_node_not_found",
            format!("node path {node_id} does not resolve: {detail}"),
        )
    };
    let mut current = roots
        .into_iter()
        .nth(indices[0])
        .ok_or_else(|| not_found(format!("no root at index {}", indices[0])))?;
    for (level, index) in indices.iter().enumerate().skip(1) {
        budget.check()?;
        let children = copy_children(current.as_ax(), budget)?;
        let count = children.len();
        current = children.into_iter().nth(*index).ok_or_else(|| {
            not_found(format!("segment {level} asks for child {index} of {count}"))
        })?;
    }
    Ok(current)
}

/// The raw `AXActionNames` of an element (not normalized).
fn raw_action_names(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Vec<String>, AccessibilityTreeError> {
    budget.check()?;
    unsafe {
        let mut names: CfTypeRef = std::ptr::null();
        let status = AXUIElementCopyActionNames(element, &mut names);
        if status == AX_ERROR_API_DISABLED {
            return Err(permission_denied());
        }
        if status == AX_ERROR_INVALID_UI_ELEMENT {
            return Err(AccessibilityTreeError::failed(
                "a11y_node_recycled",
                "AXActionNames: AX element disappeared",
            ));
        }
        let Some(names) = CfOwned::from_create(names) else {
            return Ok(Vec::new());
        };
        let array = names.as_ptr() as CfArrayRef;
        let count = CFArrayGetCount(array);
        let mut out = Vec::new();
        for i in 0..count {
            let item = CFArrayGetValueAtIndex(array, i);
            let raw = cf_string(item);
            if !raw.is_empty() {
                out.push(raw);
            }
        }
        Ok(out)
    }
}

/// Perform a named AX action the element advertises. A node that does not
/// list the action is a typed `Unsupported`, not a blind attempt.
fn perform_named_action(
    element: AxUiElementRef,
    action: &str,
    budget: &mut Budget,
) -> Result<(), AccessibilityTreeError> {
    let offered = raw_action_names(element, budget)?;
    if !offered.iter().any(|name| name == action) {
        return Err(AccessibilityTreeError::Unsupported {
            reason: format!(
                "node does not offer {action} (AXActionNames: {})",
                if offered.is_empty() {
                    "none".to_owned()
                } else {
                    offered.join(", ")
                }
            )
            .into(),
        });
    }
    budget.check()?;
    let status = unsafe {
        let key = cfstr(action);
        let status = AXUIElementPerformAction(element, key);
        CFRelease(key as CfTypeRef);
        status
    };
    if status == AX_ERROR_API_DISABLED {
        return Err(permission_denied());
    }
    map_ax_status(status, action)
}

fn attribute_settable(
    element: AxUiElementRef,
    name: &str,
    budget: &mut Budget,
) -> Result<bool, AccessibilityTreeError> {
    budget.check()?;
    let mut settable = 0u8;
    let status = unsafe {
        let key = cfstr(name);
        let status = AXUIElementIsAttributeSettable(element, key, &mut settable);
        CFRelease(key as CfTypeRef);
        status
    };
    if status == AX_ERROR_API_DISABLED {
        return Err(permission_denied());
    }
    if status == AX_ERROR_INVALID_UI_ELEMENT {
        return Err(AccessibilityTreeError::failed(
            "a11y_node_recycled",
            format!("{name}: AX element disappeared"),
        ));
    }
    Ok(status == AX_SUCCESS && settable != 0)
}

fn set_attribute(
    element: AxUiElementRef,
    name: &str,
    value: CfTypeRef,
    budget: &mut Budget,
) -> Result<(), AccessibilityTreeError> {
    budget.check()?;
    let status = unsafe {
        let key = cfstr(name);
        let status = AXUIElementSetAttributeValue(element, key, value);
        CFRelease(key as CfTypeRef);
        status
    };
    if status == AX_ERROR_API_DISABLED {
        return Err(permission_denied());
    }
    map_ax_status(status, &format!("set {name}"))
}

fn no_effect(what: &str, expected: &str, observed: &str) -> AccessibilityTreeError {
    AccessibilityTreeError::failed(
        "a11y_action_no_effect",
        format!("{what}: read-back is {observed} after asking for {expected}"),
    )
}

/// Poll `read` until it returns `true` or the read-back window closes.
fn wait_for_readback<F>(budget: &mut Budget, mut read: F) -> Result<bool, AccessibilityTreeError>
where
    F: FnMut(&mut Budget) -> Result<bool, AccessibilityTreeError>,
{
    let stop = Instant::now() + READBACK_WINDOW;
    loop {
        if read(budget)? {
            return Ok(true);
        }
        if Instant::now() >= stop {
            return Ok(false);
        }
        budget.check()?;
        std::thread::sleep(READBACK_POLL);
    }
}

/// `AXFocused = true` on the element: first responder moves inside the
/// owning application; the application itself is never activated.
fn focus_element(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<(), AccessibilityTreeError> {
    if attribute_bool(element, "AXFocused", budget)? == Some(true) {
        return Ok(());
    }
    if !attribute_settable(element, "AXFocused", budget)? {
        return Err(AccessibilityTreeError::Unsupported {
            reason: "node does not accept focus (AXFocused is not settable)".into(),
        });
    }
    set_attribute(element, "AXFocused", unsafe { kCFBooleanTrue }, budget)?;
    if wait_for_readback(budget, |budget| {
        Ok(attribute_bool(element, "AXFocused", budget)? == Some(true))
    })? {
        Ok(())
    } else {
        Err(no_effect("AXFocused", "true", "false"))
    }
}

/// Write `AXValue` and read it back. A numeric target (slider, stepper)
/// takes the decimal text of the number and is compared numerically.
fn set_value(
    element: AxUiElementRef,
    text: &str,
    budget: &mut Budget,
) -> Result<(), AccessibilityTreeError> {
    if text.len() > MAX_SET_VALUE_BYTES {
        return Err(limit_error(
            "a11y_text_limit",
            format!("value exceeds {MAX_SET_VALUE_BYTES} UTF-8 bytes"),
        ));
    }
    if !attribute_settable(element, "AXValue", budget)? {
        return Err(AccessibilityTreeError::Unsupported {
            reason: "node does not accept a value write (AXValue is not settable)".into(),
        });
    }
    match read_ax_value(element, budget)? {
        AxValue::Number(_) => {
            let wanted: f64 = text.trim().parse().map_err(|_| {
                AccessibilityTreeError::failed(
                    "invalid_input",
                    format!("node holds a numeric AXValue; {text:?} is not a number"),
                )
            })?;
            if !wanted.is_finite() {
                return Err(AccessibilityTreeError::failed(
                    "invalid_input",
                    "numeric AXValue must be finite",
                ));
            }
            let number = unsafe {
                CFNumberCreate(
                    std::ptr::null(),
                    K_CF_NUMBER_DOUBLE as CfIndex,
                    &wanted as *const f64 as *const c_void,
                )
            };
            let Some(number) = CfOwned::from_create(number) else {
                return Err(AccessibilityTreeError::failed(
                    "a11y_backend_failed",
                    "CFNumberCreate returned null",
                ));
            };
            set_attribute(element, "AXValue", number.as_ptr(), budget)?;
            let mut observed = String::new();
            if wait_for_readback(budget, |budget| {
                Ok(match read_ax_value(element, budget)? {
                    AxValue::Number(now) => {
                        observed = format_number(now);
                        (now - wanted).abs() <= 1e-9 * wanted.abs().max(1.0)
                    }
                    other => {
                        observed = format!("{other:?}");
                        false
                    }
                })
            })? {
                Ok(())
            } else {
                Err(no_effect("AXValue", &format_number(wanted), &observed))
            }
        }
        _ => {
            let value = cfstr(text);
            let Some(value) = CfOwned::from_create(value as CfTypeRef) else {
                return Err(AccessibilityTreeError::failed(
                    "a11y_backend_failed",
                    "CFStringCreateWithCString returned null",
                ));
            };
            set_attribute(element, "AXValue", value.as_ptr(), budget)?;
            let mut observed = String::new();
            if wait_for_readback(budget, |budget| {
                Ok(match read_ax_value(element, budget)? {
                    AxValue::Text(now) => {
                        let hit = now == text;
                        observed = now;
                        hit
                    }
                    AxValue::None if text.is_empty() => true,
                    other => {
                        observed = format!("{other:?}");
                        false
                    }
                })
            })? {
                Ok(())
            } else {
                Err(no_effect(
                    "AXValue",
                    &format!("{text:?}"),
                    &format!("{observed:?}"),
                ))
            }
        }
    }
}

/// Current checked state from `AXValue` 0 / 1 / 2 (`None`: not a
/// two-state control).
fn checked_value(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Option<f64>, AccessibilityTreeError> {
    let role = attribute_string(element, "AXRole", budget)?.unwrap_or_default();
    if !is_checkable_role(&normalize_role(&role)) {
        return Ok(None);
    }
    Ok(match read_ax_value(element, budget)? {
        AxValue::Number(number) => Some(number),
        AxValue::Bool(flag) => Some(if flag { 1.0 } else { 0.0 }),
        _ => None,
    })
}

/// Desired-state, idempotent: read, press only when the state differs, read
/// back. `mixed` differs from both `true` and `false`.
fn set_checked(
    element: AxUiElementRef,
    desired: bool,
    budget: &mut Budget,
) -> Result<(), AccessibilityTreeError> {
    let Some(current) = checked_value(element, budget)? else {
        return Err(AccessibilityTreeError::Unsupported {
            reason: "node exposes no checked state (not a check box / radio button / switch)"
                .into(),
        });
    };
    let matches = |number: f64| (number as i64) == i64::from(desired);
    if matches(current) {
        return Ok(());
    }
    perform_named_action(element, "AXPress", budget)?;
    let mut observed = current;
    if wait_for_readback(budget, |budget| {
        Ok(match checked_value(element, budget)? {
            Some(now) => {
                observed = now;
                matches(now)
            }
            None => false,
        })
    })? {
        Ok(())
    } else {
        Err(no_effect(
            "checked",
            if desired { "checked" } else { "unchecked" },
            checked_state_name(observed),
        ))
    }
}

fn expanded_value(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Option<bool>, AccessibilityTreeError> {
    let role = attribute_string(element, "AXRole", budget)?.unwrap_or_default();
    let role = normalize_role(&role);
    let value = read_ax_value(element, budget)?;
    expanded_state(element, &role, &value, budget)
}

/// Desired-state, idempotent, like [`set_checked`] over `AXExpanded` or a
/// disclosure triangle's value.
fn set_expanded(
    element: AxUiElementRef,
    desired: bool,
    budget: &mut Budget,
) -> Result<(), AccessibilityTreeError> {
    let Some(current) = expanded_value(element, budget)? else {
        return Err(AccessibilityTreeError::Unsupported {
            reason: "node exposes no expanded state (no AXExpanded, not a disclosure triangle)"
                .into(),
        });
    };
    if current == desired {
        return Ok(());
    }
    // Prefer writing the attribute when the toolkit allows it; otherwise
    // the primary action toggles it.
    if attribute_settable(element, "AXExpanded", budget)? {
        let flag = if desired {
            unsafe { kCFBooleanTrue }
        } else {
            let zero = 0i64;
            unsafe {
                CFNumberCreate(
                    std::ptr::null(),
                    K_CF_NUMBER_SINT64 as CfIndex,
                    &zero as *const i64 as *const c_void,
                )
            }
        };
        set_attribute(element, "AXExpanded", flag, budget)?;
        if !desired {
            unsafe { CFRelease(flag) };
        }
    } else {
        perform_named_action(element, "AXPress", budget)?;
    }
    let mut observed = current;
    if wait_for_readback(budget, |budget| {
        Ok(match expanded_value(element, budget)? {
            Some(now) => {
                observed = now;
                now == desired
            }
            None => false,
        })
    })? {
        Ok(())
    } else {
        Err(no_effect(
            "expanded",
            if desired { "expanded" } else { "collapsed" },
            if observed { "expanded" } else { "collapsed" },
        ))
    }
}

/// Elements below `root` (bounded depth and count) whose `AXTitle` equals
/// `title`.
fn find_titled_descendants(
    root: AxUiElementRef,
    title: &str,
    budget: &mut Budget,
) -> Result<Vec<CfOwned>, AccessibilityTreeError> {
    let mut hits = Vec::new();
    let mut queue: VecDeque<(CfOwned, usize)> = VecDeque::new();
    let Some(root) = CfOwned::retain(root as CfTypeRef) else {
        return Ok(hits);
    };
    queue.push_back((root, 0));
    let mut visited = 0usize;
    while let Some((element, depth)) = queue.pop_front() {
        budget.check()?;
        visited += 1;
        if visited > MAX_NODES {
            break;
        }
        if depth > 0 {
            let name = attribute_string(element.as_ax(), "AXTitle", budget)?;
            if name.as_deref() == Some(title) {
                hits.push(CfOwned::retain(element.as_ptr()).expect("non-null element"));
                continue;
            }
        }
        if depth >= OPTION_SEARCH_DEPTH {
            continue;
        }
        for child in copy_children(element.as_ax(), budget)? {
            queue.push_back((child, depth + 1));
        }
    }
    Ok(hits)
}

/// The pop-up's current selection as text, if it publishes one.
fn selection_text(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Option<String>, AccessibilityTreeError> {
    Ok(match read_ax_value(element, budget)? {
        AxValue::Text(text) => Some(text),
        _ => None,
    })
}

/// Choose the option titled exactly `option`: already selected is a no-op;
/// otherwise open the pop-up with `AXPress` (AppKit returns at once and
/// publishes an `AXMenu` child), press the unique matching item, and read
/// the selection back. No match closes the menu again (`AXCancel`) and
/// fails typed; two matches refuse before pressing anything.
fn select_option(
    element: AxUiElementRef,
    option: &str,
    budget: &mut Budget,
) -> Result<(), AccessibilityTreeError> {
    if option.is_empty() || option.len() > MAX_SET_VALUE_BYTES {
        return Err(AccessibilityTreeError::failed(
            "invalid_input",
            "select-option needs a non-empty option name",
        ));
    }
    if selection_text(element, budget)?.as_deref() == Some(option) {
        return Ok(());
    }
    let mut hits = find_titled_descendants(element, option, budget)?;
    let mut opened = false;
    if hits.is_empty() {
        let offered = raw_action_names(element, budget)?;
        let opener = ["AXPress", "AXShowMenu"]
            .into_iter()
            .find(|name| offered.iter().any(|have| have == name));
        let Some(opener) = opener else {
            return Err(AccessibilityTreeError::Unsupported {
                reason:
                    "node has no options and offers neither AXPress nor AXShowMenu to reveal any"
                        .into(),
            });
        };
        perform_named_action(element, opener, budget)?;
        opened = true;
        wait_for_readback(budget, |budget| {
            hits = find_titled_descendants(element, option, budget)?;
            Ok(!hits.is_empty())
        })?;
    }
    let outcome = match hits.len() {
        0 => Err(AccessibilityTreeError::failed(
            "a11y_option_not_found",
            format!("no option titled {option:?} under the node"),
        )),
        1 => perform_named_action(hits[0].as_ax(), "AXPress", budget),
        count => Err(AccessibilityTreeError::failed(
            "a11y_option_ambiguous",
            format!("{count} options titled {option:?} under the node; refusing to guess"),
        )),
    };
    if outcome.is_err() && opened {
        // Leave the application as it was: close the menu we opened.
        for child in copy_children(element, budget)? {
            let offered = raw_action_names(child.as_ax(), budget)?;
            if offered.iter().any(|name| name == "AXCancel") {
                let _ = perform_named_action(child.as_ax(), "AXCancel", budget);
            }
        }
    }
    outcome?;
    let mut observed = String::new();
    if wait_for_readback(budget, |budget| {
        Ok(match selection_text(element, budget)? {
            Some(now) => {
                let hit = now == option;
                observed = now;
                hit
            }
            None => false,
        })
    })? {
        Ok(())
    } else {
        Err(no_effect(
            "selection",
            &format!("{option:?}"),
            &format!("{observed:?}"),
        ))
    }
}

pub(crate) fn last_text_write_via() -> &'static str {
    "ax-value"
}

/// Deliver one chord to a node, semantically.
///
/// macOS has no way to hand a keystroke to an application that is not the
/// active one. This was measured, not assumed: an accessory app whose
/// window is ordered front reports `keyWindow = no`, and key events posted
/// to its pid with `CGEventPostToPid` never reach its `sendEvent:` at all.
/// The only route that would work is activating the application first,
/// which is exactly the invariant this adapter exists to keep -- and a
/// global `CGEventPost` would be worse, landing the chord in whatever the
/// user happens to be typing in.
///
/// So the chords that have an AX action equivalent are delivered as that
/// action -- `enter` / `return` as `AXConfirm` and `esc` / `escape` as
/// `AXCancel`, which is what those keys *mean* to a control -- and every
/// other chord is refused typed rather than posted into the void. Text
/// belongs in `set-value` / `send-text`, which writes through AX and reads
/// back.
pub(crate) fn send_node_keys(
    window_handle: Option<isize>,
    node_id: &str,
    keys: &str,
) -> Result<(), AccessibilityTreeError> {
    require_trusted()?;
    if keys.len() > MAX_KEYS_BYTES {
        return Err(limit_error(
            "a11y_key_limit",
            format!("key chord exceeds {MAX_KEYS_BYTES} UTF-8 bytes"),
        ));
    }
    let Some(action) = semantic_action_for_chord(keys) else {
        // Typed `Failed`, not `Unsupported`: the ABI collapses the latter
        // to "mechanism unavailable on this host", which would hide the
        // one thing the caller needs -- that *this chord* has no AX
        // equivalent while `enter` and `escape` do.
        return Err(AccessibilityTreeError::failed(
            "a11y_key_unavailable",
            format!(
                "macOS delivers {keys:?} only to the active application, and this adapter never \
                 activates one; only chords with an AX action equivalent (enter, escape) are \
                 mapped. Use send-text / invoke set-value for text."
            ),
        ));
    };
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    match perform_named_action(element.as_ax(), action, &mut budget) {
        Err(AccessibilityTreeError::Unsupported { reason }) => Err(AccessibilityTreeError::failed(
            "a11y_key_unavailable",
            reason,
        )),
        other => other,
    }
}

/// The AX action a chord means, or `None` when it means nothing AX can say.
/// Modifiers rule the chord out: `cmd+enter` is not `AXConfirm`, it is a
/// different command the control never hears about.
fn semantic_action_for_chord(keys: &str) -> Option<&'static str> {
    match keys.trim().to_ascii_lowercase().as_str() {
        "enter" | "return" => Some("AXConfirm"),
        "esc" | "escape" => Some("AXCancel"),
        _ => None,
    }
}

/// `AXScrollToVisible` on the resolved node — the AX spelling of AT-SPI
/// `Component.ScrollTo(TopEdge)`. A node that does not offer the action is
/// `a11y_scroll_unavailable`; never a synthetic wheel event.
pub(crate) fn scroll_node(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<(), AccessibilityTreeError> {
    require_trusted()?;
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    match perform_named_action(element.as_ax(), "AXScrollToVisible", &mut budget) {
        Err(AccessibilityTreeError::Unsupported { reason }) => Err(AccessibilityTreeError::failed(
            "a11y_scroll_unavailable",
            reason,
        )),
        other => other,
    }
}

/// Independent `AXPosition` + `AXSize` read for `get-extents`: the live
/// element is re-read, not the snapshot's `bounds` field. A node with no
/// geometry answers `a11y_extents_unavailable` rather than a zero rect.
pub(crate) fn get_node_extents(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<AccessibilityBounds, AccessibilityTreeError> {
    require_trusted()?;
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    let bounds = read_bounds(element.as_ax(), &mut budget)?;
    if bounds.width <= 0 || bounds.height <= 0 {
        return Err(AccessibilityTreeError::failed(
            "a11y_extents_unavailable",
            format!(
                "node {node_id} publishes no AXPosition/AXSize rect ({}x{})",
                bounds.width, bounds.height
            ),
        ));
    }
    Ok(bounds)
}

/// `AXSelectedTextRange` write for `select`. One range, set through the
/// accessibility API: no mouse drag, no shift-arrow keystrokes. A control
/// that does not publish a settable selected range is
/// `a11y_selection_unavailable`; a range the toolkit declines to take is
/// `a11y_selection_no_effect`.
pub(crate) fn set_node_selection(
    window_handle: Option<isize>,
    node_id: &str,
    start: i32,
    end: i32,
) -> Result<(), AccessibilityTreeError> {
    require_trusted()?;
    if start < 0 || end < start {
        return Err(AccessibilityTreeError::failed(
            "invalid_input",
            format!("selection {start}..{end} is not an ordered non-negative range"),
        ));
    }
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    let length = i64::from(end - start) as CfIndex;
    write_selected_range(
        element.as_ax(),
        i64::from(start) as CfIndex,
        length,
        "a11y_selection_unavailable",
        &mut budget,
    )?;
    match read_selected_range(element.as_ax(), &mut budget)? {
        Some(range) if range.location == i64::from(start) as CfIndex && range.length == length => {
            Ok(())
        }
        Some(range) => Err(AccessibilityTreeError::failed(
            "a11y_selection_no_effect",
            format!(
                "read-back is {}..{} after asking for {start}..{end}",
                range.location,
                range.location.saturating_add(range.length)
            ),
        )),
        None => Err(AccessibilityTreeError::failed(
            "a11y_selection_no_effect",
            "AXSelectedTextRange is unreadable after the write",
        )),
    }
}

/// Independent `AXSelectedTextRange` read for `get-selection`.
///
/// AX carries at most one range and spells "nothing selected" as a
/// zero-length range sitting at the insertion point; AT-SPI spells it
/// `n == 0` with both endpoints left at zero. This reports the AT-SPI
/// shape so one vocabulary reads the same on both backends — the caret's
/// own position stays available through `get_node_caret_offset`.
pub(crate) fn get_node_selection(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<AccessibilitySelection, AccessibilityTreeError> {
    require_trusted()?;
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    let Some(range) = read_selected_range(element.as_ax(), &mut budget)? else {
        return Err(AccessibilityTreeError::failed(
            "a11y_selection_unavailable",
            format!("node {node_id} publishes no AXSelectedTextRange"),
        ));
    };
    if range.length <= 0 {
        return Ok(AccessibilitySelection {
            n: 0,
            start: 0,
            end: 0,
        });
    }
    Ok(AccessibilitySelection {
        n: 1,
        start: clamp_index(range.location),
        end: clamp_index(range.location.saturating_add(range.length)),
    })
}

/// `AXSelectedTextRange` write with zero length for `set-caret`: the
/// insertion point moves without selecting anything.
pub(crate) fn set_node_caret_offset(
    window_handle: Option<isize>,
    node_id: &str,
    offset: i32,
) -> Result<(), AccessibilityTreeError> {
    require_trusted()?;
    if offset < 0 {
        return Err(AccessibilityTreeError::failed(
            "invalid_input",
            format!("caret offset {offset} is negative"),
        ));
    }
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    write_selected_range(
        element.as_ax(),
        i64::from(offset) as CfIndex,
        0,
        "a11y_caret_unavailable",
        &mut budget,
    )?;
    match read_selected_range(element.as_ax(), &mut budget)? {
        Some(range) if range.location == i64::from(offset) as CfIndex => Ok(()),
        Some(range) => Err(AccessibilityTreeError::failed(
            "a11y_caret_no_effect",
            format!(
                "read-back caret is {} after asking for {offset}",
                range.location
            ),
        )),
        None => Err(AccessibilityTreeError::failed(
            "a11y_caret_no_effect",
            "AXSelectedTextRange is unreadable after the write",
        )),
    }
}

/// Independent `AXSelectedTextRange` read for `get-caret`: the range's
/// location, which is where typing would land whether or not text is
/// selected.
pub(crate) fn get_node_caret_offset(
    window_handle: Option<isize>,
    node_id: &str,
) -> Result<i32, AccessibilityTreeError> {
    require_trusted()?;
    let mut budget = Budget::new(ACTION_TIMEOUT);
    let element = resolve_node(window_handle, node_id, &mut budget)?;
    let Some(range) = read_selected_range(element.as_ax(), &mut budget)? else {
        return Err(AccessibilityTreeError::failed(
            "a11y_caret_unavailable",
            format!("node {node_id} publishes no AXSelectedTextRange"),
        ));
    };
    Ok(clamp_index(range.location))
}

/// `AXSelectedTextRange` as a `CFRange`. `Ok(None)` means the element does
/// not publish the attribute (it is not a text control) — the caller turns
/// that into the typed `unavailable` code for its own verb.
fn read_selected_range(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Option<CfRange>, AccessibilityTreeError> {
    let Some(value) = copy_attribute(element, "AXSelectedTextRange", budget)? else {
        return Ok(None);
    };
    let mut range = CfRange {
        location: 0,
        length: 0,
    };
    let ok = unsafe {
        AXValueGetValue(
            value.as_ptr() as AxValueRef,
            AX_VALUE_CFRANGE,
            &mut range as *mut CfRange as *mut c_void,
        )
    };
    if ok == 0 {
        return Ok(None);
    }
    Ok(Some(range))
}

/// Write `AXSelectedTextRange`. The attribute must be settable first:
/// a read-only text view answers the caller's `unavailable` code instead
/// of a write that silently does nothing.
fn write_selected_range(
    element: AxUiElementRef,
    location: CfIndex,
    length: CfIndex,
    unavailable: &'static str,
    budget: &mut Budget,
) -> Result<(), AccessibilityTreeError> {
    if !attribute_settable(element, "AXSelectedTextRange", budget)? {
        return Err(AccessibilityTreeError::failed(
            unavailable,
            "AXSelectedTextRange is not settable on this node",
        ));
    }
    let range = CfRange { location, length };
    let value = CfOwned::from_create(unsafe {
        AXValueCreate(AX_VALUE_CFRANGE, &range as *const CfRange as *const c_void)
    } as CfTypeRef)
    .ok_or_else(|| {
        AccessibilityTreeError::failed(
            "a11y_backend_failed",
            "AXValueCreate(CFRange) returned null",
        )
    })?;
    set_attribute(element, "AXSelectedTextRange", value.as_ptr(), budget)
}

/// A `CFIndex` offset as the contract's `i32`. AX offsets are text
/// positions, so the clamp is a guard against a nonsense value, not a
/// range this code expects to meet.
fn clamp_index(value: CfIndex) -> i32 {
    value.clamp(0, i64::from(i32::MAX) as CfIndex) as i32
}

fn resolve_roots(
    window_handle: Option<isize>,
    budget: &mut Budget,
) -> Result<Vec<CfOwned>, AccessibilityTreeError> {
    match window_handle {
        Some(handle) => {
            let element = ax_element_for_handle(handle, budget)?;
            Ok(vec![element])
        }
        None => all_on_screen_window_roots(budget),
    }
}

fn all_on_screen_window_roots(budget: &mut Budget) -> Result<Vec<CfOwned>, AccessibilityTreeError> {
    let windows = enumerate_cg_windows()?;
    let mut roots = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for window in windows {
        budget.check()?;
        if !seen.insert(window.id) {
            continue;
        }
        match ax_element_for_handle(window.id as isize, budget) {
            Ok(element) => roots.push(element),
            Err(error) if is_snapshot_branch_loss(&error) => continue,
            Err(AccessibilityTreeError::Failed { code, .. })
                if code == "a11y_permission_denied" =>
            {
                return Err(permission_denied());
            }
            Err(_) => continue,
        }
        if roots.len() >= MAX_NODES {
            return Err(limit_error(
                "a11y_node_limit",
                format!("AX tree exceeds {MAX_NODES} window roots"),
            ));
        }
    }
    Ok(roots)
}

struct CgWindow {
    id: u32,
    pid: u32,
}

fn enumerate_cg_windows() -> Result<Vec<CgWindow>, AccessibilityTreeError> {
    unsafe {
        let array = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
            0,
        );
        let Some(array) = CfOwned::from_create(array) else {
            return Err(AccessibilityTreeError::failed(
                "a11y_backend_failed",
                "CGWindowListCopyWindowInfo returned null",
            ));
        };
        let count = CFArrayGetCount(array.as_ptr() as CfArrayRef);
        let mut out = Vec::new();
        for i in 0..count {
            let item = CFArrayGetValueAtIndex(array.as_ptr() as CfArrayRef, i);
            if item.is_null() {
                continue;
            }
            let dict = item as CfDictionaryRef;
            let layer = cf_i64(dict_get(dict, "kCGWindowLayer")).unwrap_or(0);
            if layer != 0 {
                continue;
            }
            let id = cf_i64(dict_get(dict, "kCGWindowNumber")).unwrap_or(0);
            if id == 0 {
                continue;
            }
            let pid = cf_i64(dict_get(dict, "kCGWindowOwnerPID")).unwrap_or(0);
            if pid <= 0 {
                continue;
            }
            out.push(CgWindow {
                id: id as u32,
                pid: pid as u32,
            });
        }
        Ok(out)
    }
}

fn owner_pid(handle: isize) -> Result<u32, AccessibilityTreeError> {
    let target = u32::try_from(handle).map_err(|_| {
        AccessibilityTreeError::failed(
            "a11y_window_gone",
            format!("window handle {handle} is not a CGWindowID"),
        )
    })?;
    for window in enumerate_cg_windows()? {
        if window.id == target {
            return Ok(window.pid);
        }
    }
    Err(AccessibilityTreeError::failed(
        "a11y_window_gone",
        format!("no on-screen window for CGWindowID {target}"),
    ))
}

fn ax_element_for_handle(
    handle: isize,
    budget: &mut Budget,
) -> Result<CfOwned, AccessibilityTreeError> {
    budget.check()?;
    let pid = owner_pid(handle)?;
    let target = handle as u32;
    unsafe {
        let app = AXUIElementCreateApplication(pid as i32);
        let Some(app) = CfOwned::from_create(app as CfTypeRef) else {
            return Err(AccessibilityTreeError::failed(
                "a11y_backend_failed",
                "AXUIElementCreateApplication returned null",
            ));
        };
        let windows_key = cfstr("AXWindows");
        let mut windows: CfTypeRef = std::ptr::null();
        let status = AXUIElementCopyAttributeValue(app.as_ax(), windows_key, &mut windows);
        CFRelease(windows_key as CfTypeRef);
        if status == AX_ERROR_API_DISABLED {
            return Err(permission_denied());
        }
        map_ax_status(status, "AXWindows")?;
        let Some(windows) = CfOwned::from_create(windows) else {
            return Err(AccessibilityTreeError::failed(
                "a11y_tree_empty",
                format!("AXWindows was null for pid {pid}"),
            ));
        };
        let count = CFArrayGetCount(windows.as_ptr() as CfArrayRef);
        for i in 0..count {
            budget.check()?;
            let el = CFArrayGetValueAtIndex(windows.as_ptr() as CfArrayRef, i);
            if el.is_null() {
                continue;
            }
            let mut id = 0u32;
            if _AXUIElementGetWindow(el as AxUiElementRef, &mut id) == AX_SUCCESS && id == target {
                return CfOwned::retain(el).ok_or_else(|| {
                    AccessibilityTreeError::failed(
                        "a11y_backend_failed",
                        "failed to retain AX window element",
                    )
                });
            }
        }
    }
    Err(AccessibilityTreeError::failed(
        "a11y_window_gone",
        format!("no AX window for CGWindowID {handle}"),
    ))
}

fn copy_attribute(
    element: AxUiElementRef,
    name: &str,
    budget: &mut Budget,
) -> Result<Option<CfOwned>, AccessibilityTreeError> {
    budget.check()?;
    unsafe {
        let key = cfstr(name);
        let mut value: CfTypeRef = std::ptr::null();
        let status = AXUIElementCopyAttributeValue(element, key, &mut value);
        CFRelease(key as CfTypeRef);
        if status == AX_ERROR_API_DISABLED {
            return Err(permission_denied());
        }
        if status == AX_ERROR_ATTRIBUTE_UNSUPPORTED
            || status == AX_ERROR_NO_VALUE
            || status == AX_ERROR_NOT_IMPLEMENTED
            || status == AX_ERROR_FAILURE
        {
            return Ok(None);
        }
        if status == AX_ERROR_INVALID_UI_ELEMENT {
            return Err(AccessibilityTreeError::failed(
                "a11y_node_recycled",
                format!("{name}: AX element disappeared"),
            ));
        }
        if status == AX_ERROR_CANNOT_COMPLETE {
            return Err(AccessibilityTreeError::failed(
                "a11y_tree_timeout",
                format!("{name}: AX could not complete"),
            ));
        }
        if status != AX_SUCCESS {
            return Err(AccessibilityTreeError::failed(
                "a11y_backend_failed",
                format!("{name}: AXError {status}"),
            ));
        }
        Ok(CfOwned::from_create(value))
    }
}

fn copy_children(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Vec<CfOwned>, AccessibilityTreeError> {
    let Some(array) = copy_attribute(element, "AXChildren", budget)? else {
        return Ok(Vec::new());
    };
    unsafe {
        let count = CFArrayGetCount(array.as_ptr() as CfArrayRef);
        if count < 0 {
            return Ok(Vec::new());
        }
        if count as usize > MAX_SIBLINGS_PER_LEVEL {
            return Err(limit_error(
                "a11y_node_limit",
                format!("AXChildren count exceeds {MAX_SIBLINGS_PER_LEVEL}"),
            ));
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            budget.check()?;
            let child = CFArrayGetValueAtIndex(array.as_ptr() as CfArrayRef, i);
            if child.is_null() {
                continue;
            }
            if let Some(owned) = CfOwned::retain(child) {
                out.push(owned);
            }
        }
        Ok(out)
    }
}

fn read_node(
    element: AxUiElementRef,
    id: String,
    parent_id: Option<String>,
    budget: &mut Budget,
) -> Result<AccessibilityNode, AccessibilityTreeError> {
    budget.check()?;
    let role = attribute_string(element, "AXRole", budget)?.unwrap_or_default();
    let role = normalize_role(&role);
    let identifier = attribute_string(element, "AXIdentifier", budget)?.filter(|s| !s.is_empty());
    let name = attribute_string(element, "AXTitle", budget)?
        .filter(|s| !s.is_empty())
        .or(attribute_string(element, "AXDescription", budget)?.filter(|s| !s.is_empty()))
        .or_else(|| identifier.clone())
        .unwrap_or_default();
    let value = read_ax_value(element, budget)?;
    let checkable = is_checkable_role(&role);
    let (text, text_truncated) = match &value {
        AxValue::Text(raw) => bounded_text_preview(raw),
        // A slider / stepper / progress value is its number; a check box's
        // 0 / 1 / 2 is a state (below), not text.
        AxValue::Number(number) if !checkable && role != "disclosure-triangle" => {
            (Some(format_number(*number)), false)
        }
        _ => (None, false),
    };
    let bounds = read_bounds(element, budget)?;
    let mut states = read_states(element, budget, &bounds)?;
    if text_truncated {
        // A window-sized text value (a terminal buffer, a long document) is
        // previewed, not copied whole: the snapshot stays bounded and the
        // node says so. The full value is a `get-text` read, not a tree.
        states.push("text-truncated".to_owned());
    }
    // Two-way control states: both directions are reported so a caller can
    // tell "off" from "not observable" (contract doc on `states`).
    if checkable && let AxValue::Number(number) = &value {
        states.push(checked_state_name(*number).to_owned());
    }
    match expanded_state(element, &role, &value, budget)? {
        Some(true) => states.push("expanded".to_owned()),
        Some(false) => states.push("collapsed".to_owned()),
        None => {}
    }
    // A menu item's check mark (`AXMenuItemMarkChar`) is its checked state;
    // an unmarked item reports nothing, since most items are never markable.
    if role == "menu-item" && menu_mark(element, budget)?.is_some() {
        states.push("checked".to_owned());
    }
    let actions = read_actions(element, budget)?;

    Ok(AccessibilityNode {
        id,
        parent_id,
        role,
        name,
        states,
        bounds,
        actions,
        text,
        identifier,
    })
}

fn attribute_string(
    element: AxUiElementRef,
    name: &str,
    budget: &mut Budget,
) -> Result<Option<String>, AccessibilityTreeError> {
    let Some(value) = copy_attribute(element, name, budget)? else {
        return Ok(None);
    };
    let text = cf_string(value.as_ptr());
    if text.len() > MAX_STRING_BYTES {
        return Err(limit_error(
            "a11y_string_limit",
            format!("{name} exceeds {MAX_STRING_BYTES} UTF-8 bytes"),
        ));
    }
    // Number-typed AXValue (e.g. sliders) is not text content.
    if text.is_empty() {
        unsafe {
            if CFGetTypeID(value.as_ptr()) == CFNumberGetTypeID() {
                return Ok(None);
            }
        }
    }
    Ok(Some(text))
}

/// The typed shape of an element's `AXValue`.
#[derive(Clone, Debug, PartialEq)]
enum AxValue {
    /// No value, or an unsupported attribute.
    None,
    Text(String),
    Number(f64),
    Bool(bool),
    /// A CF type this adapter does not read (an AXValue struct, an array).
    Other,
}

fn read_ax_value(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<AxValue, AccessibilityTreeError> {
    let Some(value) = copy_attribute(element, "AXValue", budget)? else {
        return Ok(AxValue::None);
    };
    Ok(classify_cf_value(value.as_ptr()))
}

fn classify_cf_value(value: CfTypeRef) -> AxValue {
    if value.is_null() {
        return AxValue::None;
    }
    unsafe {
        let type_id = CFGetTypeID(value);
        if type_id == CFStringGetTypeID() {
            return AxValue::Text(cf_string(value));
        }
        if type_id == CFBooleanGetTypeID() {
            return AxValue::Bool(CFBooleanGetValue(value) != 0);
        }
        if type_id == CFNumberGetTypeID() {
            let mut out = 0f64;
            if CFNumberGetValue(
                value,
                K_CF_NUMBER_DOUBLE as CfIndex,
                &mut out as *mut f64 as *mut c_void,
            ) {
                return AxValue::Number(out);
            }
            return cf_i64(value)
                .map(|n| AxValue::Number(n as f64))
                .unwrap_or(AxValue::Other);
        }
    }
    AxValue::Other
}

/// A text value cut at a UTF-8 boundary to `MAX_STRING_BYTES`: the preview
/// and whether it was cut. An empty value is `None`.
fn bounded_text_preview(raw: &str) -> (Option<String>, bool) {
    if raw.is_empty() {
        return (None, false);
    }
    let truncated = raw.len() > MAX_STRING_BYTES;
    if !truncated {
        return (Some(raw.to_owned()), false);
    }
    let mut cut = MAX_STRING_BYTES;
    while !raw.is_char_boundary(cut) {
        cut -= 1;
    }
    (Some(raw[..cut].to_owned()), true)
}

/// Decimal text of a numeric `AXValue`: integers print without a fraction.
fn format_number(number: f64) -> String {
    if number.is_finite() && number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

fn is_checkable_role(role: &str) -> bool {
    matches!(role, "check-box" | "radio-button" | "toggle" | "switch")
}

/// AppKit publishes a check box's state as `AXValue` 0 / 1 / 2.
fn checked_state_name(number: f64) -> &'static str {
    match number as i64 {
        0 => "unchecked",
        1 => "checked",
        _ => "mixed",
    }
}

/// `Some(expanded)` when the element publishes an expansion state: the
/// `AXExpanded` boolean, or a disclosure triangle's `AXValue` 0 / 1.
fn expanded_state(
    element: AxUiElementRef,
    role: &str,
    value: &AxValue,
    budget: &mut Budget,
) -> Result<Option<bool>, AccessibilityTreeError> {
    if let Some(expanded) = attribute_bool(element, "AXExpanded", budget)? {
        return Ok(Some(expanded));
    }
    if role == "disclosure-triangle" {
        return Ok(match value {
            AxValue::Number(number) => Some(*number != 0.0),
            AxValue::Bool(flag) => Some(*flag),
            _ => None,
        });
    }
    Ok(None)
}

fn read_bounds(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<AccessibilityBounds, AccessibilityTreeError> {
    let pos = copy_attribute(element, "AXPosition", budget)?;
    let size = copy_attribute(element, "AXSize", budget)?;
    let (Some(pos), Some(size)) = (pos, size) else {
        return Ok(AccessibilityBounds {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
    };
    let mut point = CgPoint { x: 0.0, y: 0.0 };
    let mut cg_size = CgSize {
        width: 0.0,
        height: 0.0,
    };
    let pok = unsafe {
        AXValueGetValue(
            pos.as_ptr() as AxValueRef,
            AX_VALUE_CGPOINT,
            &mut point as *mut CgPoint as *mut c_void,
        )
    };
    let sok = unsafe {
        AXValueGetValue(
            size.as_ptr() as AxValueRef,
            AX_VALUE_CGSIZE,
            &mut cg_size as *mut CgSize as *mut c_void,
        )
    };
    if pok == 0 || sok == 0 {
        return Ok(AccessibilityBounds {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        });
    }
    Ok(AccessibilityBounds {
        x: point.x.round() as i32,
        y: point.y.round() as i32,
        width: cg_size.width.round().max(0.0) as i32,
        height: cg_size.height.round().max(0.0) as i32,
    })
}

fn read_states(
    element: AxUiElementRef,
    budget: &mut Budget,
    bounds: &AccessibilityBounds,
) -> Result<Vec<String>, AccessibilityTreeError> {
    let mut states = Vec::new();
    // Both directions, like the two-way control states: `disabled` is a
    // read `false`, its absence means AXEnabled was not published.
    match attribute_bool(element, "AXEnabled", budget)? {
        Some(false) => states.push("disabled".to_owned()),
        _ => states.push("enabled".to_owned()),
    }
    // Presence of AXFocused means the element participates in focus; the
    // boolean value is the current focus state.
    match attribute_bool(element, "AXFocused", budget)? {
        Some(true) => {
            states.push("focusable".to_owned());
            states.push("focused".to_owned());
        }
        Some(false) => states.push("focusable".to_owned()),
        None => {}
    }
    if !attribute_bool(element, "AXHidden", budget)?.unwrap_or(false)
        && bounds.width > 0
        && bounds.height > 0
    {
        states.push("showing".to_owned());
        states.push("visible".to_owned());
    }
    if attribute_bool(element, "AXSelected", budget)?.unwrap_or(false) {
        states.push("selected".to_owned());
    }
    Ok(states)
}

fn attribute_bool(
    element: AxUiElementRef,
    name: &str,
    budget: &mut Budget,
) -> Result<Option<bool>, AccessibilityTreeError> {
    let Some(value) = copy_attribute(element, name, budget)? else {
        return Ok(None);
    };
    unsafe {
        if CFGetTypeID(value.as_ptr()) == CFBooleanGetTypeID() {
            return Ok(Some(CFBooleanGetValue(value.as_ptr()) != 0));
        }
    }
    Ok(None)
}

/// The element's performable actions, from `AXUIElementCopyActionNames`
/// (there is no `AXActions` attribute; reading one always came back empty).
/// An element that reports none yields an empty list; a recycled element or
/// a denied call fails typed like any other attribute read.
fn read_actions(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Vec<String>, AccessibilityTreeError> {
    budget.check()?;
    unsafe {
        let mut names: CfTypeRef = std::ptr::null();
        let status = AXUIElementCopyActionNames(element, &mut names);
        if status == AX_ERROR_API_DISABLED {
            return Err(permission_denied());
        }
        if status == AX_ERROR_INVALID_UI_ELEMENT {
            return Err(AccessibilityTreeError::failed(
                "a11y_node_recycled",
                "AXActionNames: AX element disappeared",
            ));
        }
        if status != AX_SUCCESS || names.is_null() {
            return Ok(Vec::new());
        }
        let Some(names) = CfOwned::from_create(names) else {
            return Ok(Vec::new());
        };
        actions_from_array(names.as_ptr() as CfArrayRef, budget)
    }
}

fn actions_from_array(
    array: CfArrayRef,
    budget: &mut Budget,
) -> Result<Vec<String>, AccessibilityTreeError> {
    unsafe {
        let count = CFArrayGetCount(array);
        let mut out = Vec::new();
        for i in 0..count {
            budget.check()?;
            let item = CFArrayGetValueAtIndex(array, i);
            if item.is_null() {
                continue;
            }
            let raw = cf_string(item);
            if raw.is_empty() {
                continue;
            }
            if let Some(normalized) = normalize_action(&raw)
                && !out.iter().any(|existing| existing == &normalized)
            {
                budget.account_string(&normalized)?;
                out.push(normalized);
            }
        }
        Ok(out)
    }
}

fn normalize_role(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_prefix = trimmed.strip_prefix("AX").unwrap_or(trimmed);
    if without_prefix.is_empty() {
        return "unknown".to_owned();
    }
    // AXStaticText -> statictext; keep a short stable token.
    without_prefix
        .chars()
        .flat_map(|ch| {
            if ch.is_uppercase() {
                vec!['-', ch.to_ascii_lowercase()]
            } else {
                vec![ch.to_ascii_lowercase()]
            }
        })
        .collect::<String>()
        .trim_start_matches('-')
        .to_owned()
}

fn normalize_action(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mapped = match trimmed {
        "AXPress" | "press" | "click" => "click",
        "AXRaise" | "raise" | "focus" => "focus",
        "AXConfirm" | "confirm" => "confirm",
        "AXCancel" | "cancel" => "cancel",
        "AXShowMenu" | "showMenu" => "show-menu",
        "AXPick" | "pick" => "pick",
        other => {
            let stripped = other.strip_prefix("AX").unwrap_or(other);
            return Some(
                stripped
                    .chars()
                    .flat_map(|ch| {
                        if ch.is_uppercase() {
                            vec!['-', ch.to_ascii_lowercase()]
                        } else {
                            vec![ch.to_ascii_lowercase()]
                        }
                    })
                    .collect::<String>()
                    .trim_start_matches('-')
                    .to_owned(),
            );
        }
    };
    Some(mapped.to_owned())
}

fn is_snapshot_branch_loss(error: &AccessibilityTreeError) -> bool {
    matches!(
        error,
        AccessibilityTreeError::Failed { code, .. }
            if code == "a11y_node_recycled"
                || code == "a11y_window_gone"
                || code == "a11y_tree_timeout"
    )
}

// Silence unused-import noise if attribute-name probing is added later.
#[allow(dead_code)]
fn copy_attribute_names(
    element: AxUiElementRef,
    budget: &mut Budget,
) -> Result<Vec<String>, AccessibilityTreeError> {
    budget.check()?;
    unsafe {
        let mut names: CfTypeRef = std::ptr::null();
        let status = AXUIElementCopyAttributeNames(element, &mut names);
        if status == AX_ERROR_API_DISABLED {
            return Err(AccessibilityTreeError::failed(
                "a11y_permission_denied",
                "AX attribute names denied",
            ));
        }
        if status != AX_SUCCESS || names.is_null() {
            return Ok(Vec::new());
        }
        let names = CfOwned::from_create(names).unwrap();
        let count = CFArrayGetCount(names.as_ptr() as CfArrayRef);
        let mut out = Vec::new();
        for i in 0..count {
            let item = CFArrayGetValueAtIndex(names.as_ptr() as CfArrayRef, i);
            let s = cf_string(item);
            if !s.is_empty() {
                out.push(s);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        checked_state_name, format_number, is_checkable_role, normalize_action, normalize_role,
        parse_node_path,
    };

    #[test]
    fn node_paths_parse_or_fail_typed() {
        assert_eq!(parse_node_path("/0").unwrap(), vec![0]);
        assert_eq!(parse_node_path("/0/2/5").unwrap(), vec![0, 2, 5]);
        for bad in ["", "/", "0/1", "/a", "/0//1"] {
            let error = parse_node_path(bad).unwrap_err();
            assert!(
                matches!(error, super::AccessibilityTreeError::Failed { ref code, .. } if code == "invalid_input"),
                "{bad:?} -> {error:?}"
            );
        }
    }

    #[test]
    fn numbers_and_check_states_format_stably() {
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(-2.0), "-2");
        assert_eq!(format_number(0.5), "0.5");
        assert_eq!(checked_state_name(0.0), "unchecked");
        assert_eq!(checked_state_name(1.0), "checked");
        assert_eq!(checked_state_name(2.0), "mixed");
        assert!(is_checkable_role("check-box"));
        assert!(is_checkable_role("radio-button"));
        assert!(!is_checkable_role("button"));
    }

    #[test]
    fn normalizes_ax_roles() {
        assert_eq!(normalize_role("AXButton"), "button");
        assert_eq!(normalize_role("AXStaticText"), "static-text");
        assert_eq!(normalize_role("AXTextField"), "text-field");
        assert_eq!(normalize_role(""), "unknown");
    }

    #[test]
    fn normalizes_ax_actions() {
        assert_eq!(normalize_action("AXPress").as_deref(), Some("click"));
        assert_eq!(normalize_action("AXRaise").as_deref(), Some("focus"));
        assert_eq!(normalize_action("AXShowMenu").as_deref(), Some("show-menu"));
    }
}
