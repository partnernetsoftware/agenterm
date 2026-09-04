//! libagenterm (`agt_*`) mechanism boundary: every call goes through the
//! shared runtime dynamic library (`crate::dynlib`) — no `agenterm-platform`
//! or `agenterm-abi` static linking. Product commands (`cu tree`, `windows`,
//! `window-place`, …) stay above this layer.
//!
//! Public API shape is deliberately small and typed: callers get `WindowInfo`
//! / `A11yTree` / `Rect`-style values or a [`MechanismError`], never raw FFI
//! pointers.

use std::ffi::{CStr, CString};

use crate::dynlib::{self, agt_a11y_node, agt_error};

// ---------------------------------------------------------------------------
// Structured accessibility tree (unchanged public API).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct A11yBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

// `Deserialize` so a persisted `snapshot` baseline reads back into exactly
// the type the live walk produces: `diff` compares one shape, never a
// second parallel node struct that could drift from this one.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct A11yNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub name: String,
    pub states: Vec<String>,
    pub bounds: A11yBounds,
    pub actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Toolkit identifier (macOS `AXIdentifier`) when the backend exposes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct A11yTree {
    pub backend: String,
    pub window_handle: Option<isize>,
    pub root_id: String,
    pub nodes: Vec<A11yNode>,
    /// The walk stopped at the depth or node budget with nodes still unread.
    pub truncated: bool,
    /// Nodes the backend adapter read during the walk.
    pub visited: usize,
    /// Nodes in `nodes`.
    pub returned: usize,
}

/// Caller bounds for one tree walk, applied by the platform adapter while it
/// reads the backend (ABI 1.12 `agt_a11y_tree_snapshot_bounded`). `None`
/// keeps the adapter default for that dimension.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TreeBudget {
    /// Deepest level returned (root = 0).
    pub max_depth: Option<u32>,
    /// Most nodes returned.
    pub max_nodes: Option<usize>,
}

impl TreeBudget {
    pub fn is_default(&self) -> bool {
        self.max_depth.is_none() && self.max_nodes.is_none()
    }
}

/// One semantic node action. `Click` / `Focus` are the historical verbs
/// (ABI 1.x `agt_a11y_node_perform`); the rest are the `invoke` vocabulary
/// (ABI 1.13 `agt_a11y_node_invoke`). `SetChecked` / `SetExpanded` name a
/// desired state the platform adapter reads before acting on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeAction {
    Click,
    Focus,
    Press,
    SetValue(String),
    SelectOption(String),
    SetChecked(bool),
    SetExpanded(bool),
    Increment,
    Decrement,
    SetSelected(bool),
    Cancel,
    ShowDefaultUi,
}

impl NodeAction {
    /// The public verb spelling (`press`, `set-value`, ...).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Focus => "focus",
            Self::Press => "press",
            Self::SetValue(_) => "set-value",
            Self::SelectOption(_) => "select-option",
            Self::SetChecked(_) => "set-checked",
            Self::SetExpanded(_) => "set-expanded",
            Self::Increment => "increment",
            Self::Decrement => "decrement",
            Self::SetSelected(_) => "set-selected",
            Self::Cancel => "cancel",
            Self::ShowDefaultUi => "show-default-ui",
        }
    }

    /// `(abi kind, value payload)` for the loaded library.
    fn abi_parts(&self) -> (i32, Option<String>) {
        match self {
            Self::Click => (dynlib::AGT_A11Y_ACTION_CLICK, None),
            Self::Focus => (dynlib::AGT_A11Y_ACTION_FOCUS, None),
            Self::Press => (dynlib::AGT_A11Y_ACTION_PRESS, None),
            Self::SetValue(value) => (dynlib::AGT_A11Y_ACTION_SET_VALUE, Some(value.clone())),
            Self::SelectOption(option) => {
                (dynlib::AGT_A11Y_ACTION_SELECT_OPTION, Some(option.clone()))
            }
            Self::SetChecked(flag) => (
                dynlib::AGT_A11Y_ACTION_SET_CHECKED,
                Some(if *flag { "1" } else { "0" }.to_owned()),
            ),
            Self::SetExpanded(flag) => (
                dynlib::AGT_A11Y_ACTION_SET_EXPANDED,
                Some(if *flag { "1" } else { "0" }.to_owned()),
            ),
            Self::Increment => (dynlib::AGT_A11Y_ACTION_INCREMENT, None),
            Self::Decrement => (dynlib::AGT_A11Y_ACTION_DECREMENT, None),
            Self::SetSelected(flag) => (
                dynlib::AGT_A11Y_ACTION_SET_SELECTED,
                Some(if *flag { "1" } else { "0" }.to_owned()),
            ),
            Self::Cancel => (dynlib::AGT_A11Y_ACTION_CANCEL, None),
            Self::ShowDefaultUi => (dynlib::AGT_A11Y_ACTION_SHOW_DEFAULT_UI, None),
        }
    }
}

/// Typed failure for every mechanism call. `Unsupported` carries a human
/// reason; `Failed` mirrors the library's `{code, message}` error record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MechanismError {
    Unsupported { reason: String },
    Failed { code: String, message: String },
}

// ---------------------------------------------------------------------------
// Capability negotiation (discovery/metadata only).
// ---------------------------------------------------------------------------

/// Capabilities `cu` reports on. Values are the ABI capability ids.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Capability {
    WindowEnumerate,
    WindowOp,
    Screenshot,
    InputInject,
    AccessibilityTree,
    DesktopHost,
    WindowPlacementInspect,
}

impl Capability {
    fn abi_id(self) -> i32 {
        match self {
            Capability::WindowEnumerate => dynlib::AGT_CAP_WINDOW_ENUMERATE,
            Capability::WindowOp => dynlib::AGT_CAP_WINDOW_OP,
            Capability::Screenshot => dynlib::AGT_CAP_SCREENSHOT,
            Capability::InputInject => dynlib::AGT_CAP_INPUT_INJECT,
            Capability::AccessibilityTree => dynlib::AGT_CAP_ACCESSIBILITY_TREE,
            Capability::DesktopHost => dynlib::AGT_CAP_DESKTOP_HOST,
            Capability::WindowPlacementInspect => dynlib::AGT_CAP_WINDOW_PLACEMENT_INSPECT,
        }
    }
}

// ---------------------------------------------------------------------------
// Foreign-window placement inspection (ABI 1.10).
// ---------------------------------------------------------------------------

pub mod window_placement {
    use crate::dynlib;

    use super::{MechanismError, last_mechanism_error};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum PlacementRole {
        Standard,
        Dialog,
        Sheet,
        SystemDialog,
        Other,
        Unknown,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Support {
        Yes,
        No,
        Unknown,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct WindowSize {
        pub width: u32,
        pub height: u32,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum SizeConstraints {
        Explicit {
            min: Option<WindowSize>,
            max: Option<WindowSize>,
            increment: Option<WindowSize>,
        },
        ApplicationEnforced,
        Unknown,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct PlacementWindowInfo {
        pub handle: isize,
        pub process_id: u32,
        pub role: PlacementRole,
        pub movable: Support,
        pub resizable: Support,
        pub constraints: SizeConstraints,
    }

    pub fn inspect(
        handle: isize,
        expected_pid: u32,
    ) -> Result<PlacementWindowInfo, MechanismError> {
        let lib = dynlib::load().map_err(|error| MechanismError::Failed {
            code: "dylib_load".into(),
            message: error.message.clone(),
        })?;
        let version = lib
            .abi_version()
            .map_err(|message| MechanismError::Failed {
                code: "dylib_symbol".into(),
                message,
            })?;
        require_placement_abi(version)?;
        let query =
            unsafe { lib.sym::<super::WindowPlacementQuery>(b"agt_window_placement_query") }
                .map_err(|_| MechanismError::Unsupported {
                    reason: "ABI 1.10 window placement inspection symbol is unavailable".into(),
                })?;
        let mut record = dynlib::agt_window_placement_info_v1 {
            struct_size: std::mem::size_of::<dynlib::agt_window_placement_info_v1>() as u32,
            ..Default::default()
        };
        match unsafe { query(handle, expected_pid, &mut record) } {
            dynlib::AGT_OK => parse_record(record, handle, expected_pid),
            dynlib::AGT_UNSUPPORTED => Err(MechanismError::Unsupported {
                reason: "window placement inspection is unavailable on this host".into(),
            }),
            _ => Err(last_mechanism_error("agt_window_placement_query")),
        }
    }

    pub(super) fn require_placement_abi(version: u32) -> Result<(), MechanismError> {
        let major = version >> 16;
        let minor = (version & 0xffff) as u16;
        if major == 1 && minor >= dynlib::WINDOW_PLACEMENT_ABI_MINOR {
            Ok(())
        } else {
            Err(MechanismError::Unsupported {
                reason: format!(
                    "window placement inspection requires ABI 1.{}, loaded library reports {major}.{minor}",
                    dynlib::WINDOW_PLACEMENT_ABI_MINOR
                ),
            })
        }
    }

    pub(super) fn parse_record(
        record: dynlib::agt_window_placement_info_v1,
        expected_handle: isize,
        expected_pid: u32,
    ) -> Result<PlacementWindowInfo, MechanismError> {
        let invalid = |message: &str| MechanismError::Failed {
            code: "window_metadata_invalid".into(),
            message: message.into(),
        };
        if record.struct_size != std::mem::size_of::<dynlib::agt_window_placement_info_v1>() as u32
            || record.record_version != dynlib::AGT_WINDOW_PLACEMENT_RECORD_V1
            || record.handle != expected_handle
            || record.process_id != expected_pid
        {
            return Err(invalid(
                "placement record size, version, or identity does not match the query",
            ));
        }
        let role = match record.role {
            dynlib::AGT_WINDOW_ROLE_STANDARD => PlacementRole::Standard,
            dynlib::AGT_WINDOW_ROLE_DIALOG => PlacementRole::Dialog,
            dynlib::AGT_WINDOW_ROLE_SHEET => PlacementRole::Sheet,
            dynlib::AGT_WINDOW_ROLE_SYSTEM_DIALOG => PlacementRole::SystemDialog,
            dynlib::AGT_WINDOW_ROLE_OTHER => PlacementRole::Other,
            dynlib::AGT_WINDOW_ROLE_UNKNOWN => PlacementRole::Unknown,
            _ => return Err(invalid("placement record contains an invalid role")),
        };
        let parse_support = |raw| match raw {
            dynlib::AGT_WINDOW_SUPPORT_YES => Ok(Support::Yes),
            dynlib::AGT_WINDOW_SUPPORT_NO => Ok(Support::No),
            dynlib::AGT_WINDOW_SUPPORT_UNKNOWN => Ok(Support::Unknown),
            _ => Err(invalid(
                "placement record contains an invalid support value",
            )),
        };
        let movable = parse_support(record.movable)?;
        let resizable = parse_support(record.resizable)?;
        let known_flags = dynlib::AGT_WINDOW_CONSTRAINT_HAS_MIN
            | dynlib::AGT_WINDOW_CONSTRAINT_HAS_MAX
            | dynlib::AGT_WINDOW_CONSTRAINT_HAS_INCREMENT;
        if record.constraint_flags & !known_flags != 0 {
            return Err(invalid(
                "placement record contains unknown constraint flags",
            ));
        }
        let pair = |flag, width, height, name| {
            if record.constraint_flags & flag != 0 {
                if width == 0 || height == 0 {
                    return Err(invalid(&format!("{name} constraint must be nonzero")));
                }
                Ok(Some(WindowSize { width, height }))
            } else if width != 0 || height != 0 {
                Err(invalid(&format!(
                    "{name} dimensions are set without their flag"
                )))
            } else {
                Ok(None)
            }
        };
        let min = pair(
            dynlib::AGT_WINDOW_CONSTRAINT_HAS_MIN,
            record.min_width,
            record.min_height,
            "minimum",
        )?;
        let max = pair(
            dynlib::AGT_WINDOW_CONSTRAINT_HAS_MAX,
            record.max_width,
            record.max_height,
            "maximum",
        )?;
        let increment = pair(
            dynlib::AGT_WINDOW_CONSTRAINT_HAS_INCREMENT,
            record.increment_width,
            record.increment_height,
            "increment",
        )?;
        if let (Some(min), Some(max)) = (min, max)
            && (min.width > max.width || min.height > max.height)
        {
            return Err(invalid("minimum constraint exceeds maximum constraint"));
        }
        let constraints = match record.constraints_kind {
            dynlib::AGT_WINDOW_CONSTRAINTS_EXPLICIT => SizeConstraints::Explicit {
                min,
                max,
                increment,
            },
            dynlib::AGT_WINDOW_CONSTRAINTS_APPLICATION_ENFORCED if record.constraint_flags == 0 => {
                SizeConstraints::ApplicationEnforced
            }
            dynlib::AGT_WINDOW_CONSTRAINTS_UNKNOWN if record.constraint_flags == 0 => {
                SizeConstraints::Unknown
            }
            dynlib::AGT_WINDOW_CONSTRAINTS_APPLICATION_ENFORCED
            | dynlib::AGT_WINDOW_CONSTRAINTS_UNKNOWN => {
                return Err(invalid(
                    "non-explicit constraints must not carry dimensions",
                ));
            }
            _ => {
                return Err(invalid(
                    "placement record contains an invalid constraints kind",
                ));
            }
        };
        Ok(PlacementWindowInfo {
            handle: record.handle,
            process_id: record.process_id,
            role,
            movable,
            resizable,
            constraints,
        })
    }
}

/// Debug shape matches the old `agenterm-platform` `CapabilityStatus` so
/// `agenterm-cu capabilities` output stays stable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapabilityStatus {
    Available,
    Unsupported { reason: String },
    Failed { code: String, message: String },
}

/// Query one capability through `agt_capability_query`. `AGT_UNSUPPORTED`
/// becomes `Unsupported` with a stable reason (the ABI carries no reason
/// text); any unexpected status becomes `Failed`; a library load / symbol
/// resolution failure is reported verbatim so `agenterm-cu capabilities` stays
/// truthful when the dynamic library is missing.
pub fn capability_status(capability: Capability) -> CapabilityStatus {
    let query = match call_sym::<CapabilityQuery>(b"agt_capability_query") {
        Ok(f) => f,
        Err(error) => {
            return CapabilityStatus::Failed {
                code: error_code(&error),
                message: error_message(&error),
            };
        }
    };
    match unsafe { query(capability.abi_id()) } {
        dynlib::AGT_OK => CapabilityStatus::Available,
        dynlib::AGT_UNSUPPORTED => CapabilityStatus::Unsupported {
            reason: "host adapter unavailable".to_owned(),
        },
        // ABI 1.12: a mechanism the OS refuses (macOS Accessibility
        // permission) answers AGT_FAILED with its typed code and repair path
        // in agt_last_error; carry both instead of a generic failure.
        _ => {
            let error = last_mechanism_error("agt_capability_query");
            CapabilityStatus::Failed {
                code: error_code(&error),
                message: error_message(&error),
            }
        }
    }
}

fn error_code(error: &MechanismError) -> String {
    match error {
        MechanismError::Failed { code, .. } => code.clone(),
        MechanismError::Unsupported { .. } => "unsupported".to_owned(),
    }
}

fn error_message(error: &MechanismError) -> String {
    match error {
        MechanismError::Failed { message, .. } => message.clone(),
        MechanismError::Unsupported { reason } => reason.clone(),
    }
}

pub fn accessibility_tree_available() -> bool {
    matches!(
        capability_status(Capability::AccessibilityTree),
        CapabilityStatus::Available
    )
}

// ---------------------------------------------------------------------------
// Window enumeration + screens.
// ---------------------------------------------------------------------------

pub mod window_enumerate {
    use crate::dynlib;

    use super::{MechanismError, call_sym, fixed_field, map_status};

    /// Bounds of a top-level window in physical screen pixels (top-origin).
    #[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
    pub struct WindowBounds {
        pub x: i32,
        pub y: i32,
        pub width: u32,
        pub height: u32,
    }

    /// One display in top-origin coordinates (same space as [`WindowBounds`]).
    #[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
    pub struct ScreenInfo {
        pub frame: WindowBounds,
        pub visible: WindowBounds,
        pub primary: bool,
    }

    /// A snapshot of one visible top-level window.
    #[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
    pub struct WindowInfo {
        /// Native window handle (HWND on Windows), valid for the observation
        /// instant.
        pub handle: isize,
        pub title: String,
        pub process_id: u32,
        pub app_name: String,
        pub bounds: WindowBounds,
        pub focused: bool,
        pub minimized: bool,
    }

    /// `agt_window_enumerate`: two-stage (probe, allocate, fetch).
    pub fn enumerate_top_level() -> Result<Vec<WindowInfo>, MechanismError> {
        let f = call_sym::<super::WindowEnumerate>(b"agt_window_enumerate")?;
        let mut needed = 0usize;
        let status = unsafe { f(std::ptr::null_mut(), 0, &mut needed) };
        match status {
            // Zero items: `cap < required` is `0 < 0`, so the two-stage
            // probe answers OK rather than buffer_too_small. An empty
            // desktop is an empty list, not a failure -- this cost a
            // `windows` call on a display with no windows. AGT_FAILED
            // therefore always means a real failure here, never emptiness:
            // reading it as an empty list hid the failure instead.
            dynlib::AGT_OK => Ok(Vec::new()),
            dynlib::AGT_UNSUPPORTED => Err(MechanismError::Unsupported {
                reason: "window enumeration is unavailable on this host".to_owned(),
            }),
            dynlib::AGT_FAILED => {
                let mut capacity = needed;
                for _ in 0..4 {
                    let mut buf = vec![dynlib::agt_window_info::default(); capacity];
                    let mut got = 0usize;
                    let status = unsafe { f(buf.as_mut_ptr(), capacity, &mut got) };
                    if status == dynlib::AGT_OK {
                        buf.truncate(got);
                        return Ok(buf.iter().map(record_to_info).collect());
                    }
                    if let Some(grown) = retry_capacity(status, capacity, got) {
                        capacity = grown;
                        continue;
                    }
                    map_status("agt_window_enumerate fetch", status)?;
                }
                Err(MechanismError::Failed {
                    code: "window_churn".to_owned(),
                    message: "window count did not stabilize after bounded retries".to_owned(),
                })
            }
            other => Err(MechanismError::Failed {
                code: "unexpected_status".to_owned(),
                message: format!(
                    "agt_window_enumerate probe: expected AGT_FAILED (buffer_too_small), got {other}"
                ),
            }),
        }
    }

    /// One window's place in the desktop's front-to-back order.
    #[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
    pub struct WindowStacking {
        pub handle: isize,
        pub z_index: u32,
        pub occluded_percent: u32,
    }

    /// `agt_window_stacking_list` (ABI 1.17): two-stage, same shape as
    /// [`enumerate_top_level`]. A host that cannot report a real stacking
    /// order answers `Unsupported`, which the caller reports as an absent
    /// stacking rather than as z-index 0 for everything.
    pub fn stacking() -> Result<Vec<WindowStacking>, MechanismError> {
        let (major, minor) = super::loaded_abi_version()?;
        if major != 1 || minor < dynlib::WINDOW_STACKING_ABI_MINOR {
            return Err(MechanismError::Unsupported {
                reason: format!(
                    "window stacking requires ABI 1.{}, loaded library reports {major}.{minor}",
                    dynlib::WINDOW_STACKING_ABI_MINOR
                ),
            });
        }
        let f = call_sym::<super::WindowStackingList>(b"agt_window_stacking_list")?;
        let mut needed = 0usize;
        let status = unsafe { f(std::ptr::null_mut(), 0, &mut needed) };
        match status {
            // Zero items: `cap < required` is `0 < 0`, so the two-stage
            // probe answers OK rather than buffer_too_small. An empty
            // desktop is an empty list, not a failure -- this cost a
            // `windows` call on a display with no windows. AGT_FAILED
            // therefore always means a real failure here, never emptiness:
            // reading it as an empty list hid the failure instead.
            dynlib::AGT_OK => Ok(Vec::new()),
            dynlib::AGT_UNSUPPORTED => Err(MechanismError::Unsupported {
                reason: "this host reports no window stacking order".to_owned(),
            }),
            dynlib::AGT_FAILED => {
                let mut capacity = needed;
                for _ in 0..4 {
                    let mut buf = vec![dynlib::agt_window_stacking::default(); capacity];
                    let mut got = 0usize;
                    let status = unsafe { f(buf.as_mut_ptr(), capacity, &mut got) };
                    if status == dynlib::AGT_OK {
                        buf.truncate(got);
                        return Ok(buf
                            .iter()
                            .map(|row| WindowStacking {
                                handle: row.handle,
                                z_index: row.z_index,
                                occluded_percent: row.occluded_percent,
                            })
                            .collect());
                    }
                    if let Some(grown) = retry_capacity(status, capacity, got) {
                        capacity = grown;
                        continue;
                    }
                    map_status("agt_window_stacking_list fetch", status)?;
                }
                Err(MechanismError::Failed {
                    code: "window_churn".to_owned(),
                    message: "window count did not stabilize after bounded retries".to_owned(),
                })
            }
            other => Err(MechanismError::Failed {
                code: "unexpected_status".to_owned(),
                message: format!(
                    "agt_window_stacking_list probe: expected AGT_FAILED (buffer_too_small), got {other}"
                ),
            }),
        }
    }

    /// `agt_screen_list`: two-stage, same shape as [`enumerate_top_level`].
    pub fn list_screens() -> Result<Vec<ScreenInfo>, MechanismError> {
        let f = call_sym::<super::ScreenList>(b"agt_screen_list")?;
        let mut needed = 0usize;
        let status = unsafe { f(std::ptr::null_mut(), 0, &mut needed) };
        match status {
            // Zero items: `cap < required` is `0 < 0`, so the two-stage
            // probe answers OK rather than buffer_too_small. An empty
            // desktop is an empty list, not a failure -- this cost a
            // `windows` call on a display with no windows. AGT_FAILED
            // therefore always means a real failure here, never emptiness:
            // reading it as an empty list hid the failure instead.
            dynlib::AGT_OK => Ok(Vec::new()),
            dynlib::AGT_UNSUPPORTED => Err(MechanismError::Unsupported {
                reason: "screen enumeration is unavailable on this host".to_owned(),
            }),
            dynlib::AGT_FAILED => {
                let mut capacity = needed;
                for _ in 0..4 {
                    let mut buf = vec![dynlib::agt_screen_info::default(); capacity];
                    let mut got = 0usize;
                    let status = unsafe { f(buf.as_mut_ptr(), capacity, &mut got) };
                    if status == dynlib::AGT_OK {
                        buf.truncate(got);
                        return Ok(buf.iter().map(record_to_screen).collect());
                    }
                    if let Some(grown) = retry_capacity(status, capacity, got) {
                        capacity = grown;
                        continue;
                    }
                    map_status("agt_screen_list fetch", status)?;
                }
                Err(MechanismError::Failed {
                    code: "screen_churn".to_owned(),
                    message: "screen count did not stabilize after bounded retries".to_owned(),
                })
            }
            other => Err(MechanismError::Failed {
                code: "unexpected_status".to_owned(),
                message: format!(
                    "agt_screen_list probe: expected AGT_FAILED (buffer_too_small), got {other}"
                ),
            }),
        }
    }

    pub(super) fn retry_capacity(status: i32, capacity: usize, required: usize) -> Option<usize> {
        (status == dynlib::AGT_FAILED && required > capacity).then_some(required)
    }

    fn record_to_info(record: &dynlib::agt_window_info) -> WindowInfo {
        WindowInfo {
            handle: record.handle,
            title: fixed_field(&record.title, record.title_len),
            process_id: record.process_id,
            app_name: fixed_field(&record.app_name, record.app_name_len),
            bounds: WindowBounds {
                x: record.x,
                y: record.y,
                width: record.width,
                height: record.height,
            },
            focused: record.focused != 0,
            minimized: record.minimized != 0,
        }
    }

    fn record_to_screen(record: &dynlib::agt_screen_info) -> ScreenInfo {
        ScreenInfo {
            frame: WindowBounds {
                x: record.frame_x,
                y: record.frame_y,
                width: record.frame_width,
                height: record.frame_height,
            },
            visible: WindowBounds {
                x: record.visible_x,
                y: record.visible_y,
                width: record.visible_width,
                height: record.visible_height,
            },
            primary: record.primary != 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Native window operations.
// ---------------------------------------------------------------------------

pub mod window_op {
    use super::{MechanismError, map_status};

    /// `agt_native_window_show` state 1: Show (raise). MCU `orderwin` uses this.
    pub const SHOW: i32 = 1;

    /// Show/hide/minimize/maximize/restore a native window handle.
    pub fn show(handle: isize, state: i32) -> Result<(), MechanismError> {
        let f = super::call_sym::<super::WindowShow>(b"agt_native_window_show")?;
        let status = unsafe { f(handle, state) };
        map_status("agt_native_window_show", status)
    }

    /// Move/resize a native window handle.
    pub fn move_window(
        handle: isize,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), MechanismError> {
        let f = super::call_sym::<super::WindowMove>(b"agt_native_window_move")?;
        let status = unsafe { f(handle, x, y, width, height) };
        map_status("agt_native_window_move", status)
    }

    /// Read a native window handle's rectangle (physical pixels, top-origin).
    pub fn window_rect(
        handle: isize,
    ) -> Result<super::window_enumerate::WindowBounds, MechanismError> {
        let f = super::call_sym::<super::WindowRect>(b"agt_native_window_rect")?;
        let mut x = 0i32;
        let mut y = 0i32;
        let mut w = 0u32;
        let mut h = 0u32;
        let status = unsafe { f(handle, &mut x, &mut y, &mut w, &mut h) };
        map_status("agt_native_window_rect", status)?;
        Ok(super::window_enumerate::WindowBounds {
            x,
            y,
            width: w,
            height: h,
        })
    }

    /// Pin/unpin a native window handle above other windows.
    pub fn set_topmost(handle: isize, topmost: bool) -> Result<(), MechanismError> {
        let f = super::call_sym::<super::WindowSetTopmost>(b"agt_native_window_set_topmost")?;
        let status = unsafe { f(handle, i32::from(topmost)) };
        map_status("agt_native_window_set_topmost", status)
    }

    /// Whether a native window is minimized (ABI 1.25
    /// `agt_native_window_minimized`).
    ///
    /// This is a *separate* read from the window inventory on purpose. On
    /// macOS the inventory is `kCGWindowListOptionOnScreenOnly`, and a
    /// minimized window is not on screen: it leaves the list entirely
    /// rather than appearing with `minimized: true` (measured 2026-09-03 —
    /// CGWindowID 24140 vanished from `windows` while minimized and came
    /// back with the same id on restore). So a `minimize` / `restore`
    /// read-back that only consulted the inventory could say "gone", never
    /// "minimized"; this asks the window itself.
    pub fn minimized(handle: isize) -> Result<bool, MechanismError> {
        let (major, minor) = super::loaded_abi_version()?;
        if major != 1 || minor < crate::dynlib::WINDOW_MINIMIZED_ABI_MINOR {
            return Err(MechanismError::Unsupported {
                reason: format!(
                    "the minimized read requires ABI 1.{}, loaded library reports {major}.{minor}",
                    crate::dynlib::WINDOW_MINIMIZED_ABI_MINOR
                ),
            });
        }
        let f = super::call_sym::<super::WindowMinimized>(b"agt_native_window_minimized")?;
        let mut out = 0i32;
        let status = unsafe { f(handle, &mut out) };
        map_status("agt_native_window_minimized", status)?;
        Ok(out != 0)
    }

    /// Close a **native** window handle (distinct from the ABI's own
    /// `agt_window_close`).
    pub fn close(handle: isize) -> Result<(), MechanismError> {
        let f = super::call_sym::<super::WindowClose>(b"agt_native_window_close")?;
        let status = unsafe { f(handle) };
        map_status("agt_native_window_close", status)
    }
}

// ---------------------------------------------------------------------------
// Input injection.
// ---------------------------------------------------------------------------

/// Thread-local count of every actuation attempt this thread handed to
/// libagenterm (text writes, node actions, key / text injection). It is a
/// test seam and a diagnostic, not a receipt: it proves that a verb which
/// answered typed *did not reach the mechanism*, which a receipt file
/// cannot (a refusal writes none).
pub mod write_ledger {
    use std::cell::Cell;

    thread_local! {
        static ATTEMPTS: Cell<usize> = const { Cell::new(0) };
    }

    /// Record one attempt; called immediately before the FFI call.
    pub(crate) fn note() {
        ATTEMPTS.with(|count| count.set(count.get() + 1));
    }

    /// Attempts so far on this thread.
    pub fn attempts() -> usize {
        ATTEMPTS.with(Cell::get)
    }
}

pub mod input_inject {
    use super::{MechanismError, last_mechanism_error, map_status};
    use crate::dynlib;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum PointerButton {
        Left,
        Right,
        Middle,
    }

    impl PointerButton {
        fn abi_id(self) -> i32 {
            match self {
                PointerButton::Left => dynlib::AGT_INPUT_BUTTON_LEFT,
                PointerButton::Right => dynlib::AGT_INPUT_BUTTON_RIGHT,
                PointerButton::Middle => dynlib::AGT_INPUT_BUTTON_MIDDLE,
            }
        }
    }

    /// Move the pointer to absolute screen coordinates.
    pub fn pointer_move(x: i32, y: i32) -> Result<(), MechanismError> {
        let f = super::call_sym::<super::PointerMove>(b"agt_input_pointer_move")?;
        let status = unsafe { f(x, y) };
        map_status("agt_input_pointer_move", status)
    }

    /// Read absolute screen coordinates without injecting input.
    pub fn pointer_position() -> Result<(i32, i32), MechanismError> {
        let lib = dynlib::load().map_err(|error| MechanismError::Failed {
            code: "dylib_load".into(),
            message: error.message.clone(),
        })?;
        let version = lib
            .abi_version()
            .map_err(|message| MechanismError::Failed {
                code: "dylib_symbol".into(),
                message,
            })?;
        require_pointer_position_abi(version)?;
        let query = unsafe { lib.sym::<super::PointerPosition>(b"agt_input_pointer_position") }
            .map_err(|_| MechanismError::Unsupported {
                reason: "ABI 1.11 pointer-position symbol is unavailable".into(),
            })?;
        let mut x = 0;
        let mut y = 0;
        match unsafe { query(&mut x, &mut y) } {
            dynlib::AGT_OK => Ok((x, y)),
            dynlib::AGT_UNSUPPORTED => Err(MechanismError::Unsupported {
                reason: "pointer-position is unavailable on this host".into(),
            }),
            _ => Err(last_mechanism_error("agt_input_pointer_position")),
        }
    }

    pub(super) fn require_pointer_position_abi(version: u32) -> Result<(), MechanismError> {
        let major = version >> 16;
        let minor = (version & 0xffff) as u16;
        if major == 1 && minor >= dynlib::POINTER_POSITION_ABI_MINOR {
            Ok(())
        } else {
            Err(MechanismError::Unsupported {
                reason: format!(
                    "pointer-position requires ABI 1.{}, loaded library reports {major}.{minor}",
                    dynlib::POINTER_POSITION_ABI_MINOR
                ),
            })
        }
    }

    /// Click a pointer button at absolute screen coordinates.
    pub fn pointer_click(
        x: i32,
        y: i32,
        button: PointerButton,
        clicks: u32,
    ) -> Result<(), MechanismError> {
        let f = super::call_sym::<super::PointerClick>(b"agt_input_pointer_click")?;
        let status = unsafe { f(x, y, button.abi_id(), clicks) };
        map_status("agt_input_pointer_click", status)
    }

    /// Type UTF-8 text into the focused control.
    /// One press / bounded moves / release gesture (ABI 1.25
    /// `agt_input_pointer_drag`).
    ///
    /// `steps` is the number of intermediate moves between the press and
    /// the release; the library validates `1..=64` and the button code
    /// before it touches the pointer. Where a host can only deliver this
    /// by moving the user's real cursor (macOS: there is no window-local
    /// pointer injection at all) the caller must have opted in with
    /// `--degraded`, exactly as for `click --coords`.
    pub fn pointer_drag(
        from: (i32, i32),
        to: (i32, i32),
        button: PointerButton,
        steps: u32,
    ) -> Result<(), MechanismError> {
        let (major, minor) = super::loaded_abi_version()?;
        if major != 1 || minor < crate::dynlib::POINTER_DRAG_ABI_MINOR {
            return Err(MechanismError::Unsupported {
                reason: format!(
                    "pointer drag requires ABI 1.{}, loaded library reports {major}.{minor}",
                    crate::dynlib::POINTER_DRAG_ABI_MINOR
                ),
            });
        }
        let f = super::call_sym::<super::PointerDrag>(b"agt_input_pointer_drag")?;
        super::write_ledger::note();
        let status = unsafe { f(from.0, from.1, to.0, to.1, button.abi_id(), steps) };
        map_status("agt_input_pointer_drag", status)
    }

    pub fn type_text(text: &str) -> Result<(), MechanismError> {
        super::write_ledger::note();
        let f = super::call_sym::<super::InputTypeText>(b"agt_input_type_text")?;
        let status = unsafe { f(text.as_ptr(), text.len()) };
        map_status("agt_input_type_text", status)
    }

    /// Send a hotkey chord such as `ctrl+s`, `alt+f4` or `enter`.
    pub fn send_keys(keys: &str) -> Result<(), MechanismError> {
        super::write_ledger::note();
        let f = super::call_sym::<super::InputSendKeys>(b"agt_input_send_keys")?;
        let status = unsafe { f(keys.as_ptr(), keys.len()) };
        map_status("agt_input_send_keys", status)
    }
}

// ---------------------------------------------------------------------------
// Screenshots.
// ---------------------------------------------------------------------------

pub mod screenshot {
    use std::ffi::CString;
    use std::path::Path;

    use super::{MechanismError, map_status};
    use crate::dynlib;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct ScreenshotWriteResult {
        pub frame_width: u32,
        pub frame_height: u32,
        pub output_width: u32,
        pub output_height: u32,
        pub output_pixels: usize,
    }

    /// Capture a native window to a PNG at `path` (whole-window area).
    /// The ABI export writes the file itself; output dimensions are read back
    /// from the PNG header so the result shape matches the previous
    /// `agenterm-platform` path.
    pub fn capture_native_window_png(
        native_window: isize,
        path: &Path,
    ) -> Result<ScreenshotWriteResult, MechanismError> {
        if native_window == 0 {
            return Err(MechanismError::Failed {
                code: "bad_handle".to_owned(),
                message: "native_window is 0".to_owned(),
            });
        }
        let path_c = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| {
            MechanismError::Failed {
                code: "bad_path".to_owned(),
                message: "path contains an interior NUL byte".to_owned(),
            }
        })?;
        let f = super::call_sym::<super::CaptureWindow>(b"agt_screenshot_capture_window")?;
        let status = unsafe {
            f(
                native_window,
                path_c.as_ptr(),
                dynlib::AGT_SCREENSHOT_AREA_WINDOW,
                0,
                0,
                0,
                0,
            )
        };
        map_status("agt_screenshot_capture_window", status)?;
        let (output_width, output_height) = png_dimensions(path);
        let output_pixels = output_width as usize * output_height as usize;
        Ok(ScreenshotWriteResult {
            frame_width: output_width,
            frame_height: output_height,
            output_width,
            output_height,
            output_pixels,
        })
    }

    /// Capture one **region of the window's own capture** to a PNG at
    /// `path`. `left` / `top` are in the capture's pixel space (the origin
    /// is the window's top-left, not the screen's), which is what the ABI's
    /// client-area clip means; `zoom` converts screen coordinates into it.
    ///
    /// This reuses `agt_screenshot_capture_window` with `area_kind = 1`
    /// (ABI 1.0): the adapter clips the frame it already captured, so a
    /// region capture is never a second grab of the screen and never a
    /// full-screen fallback.
    pub fn capture_native_window_region_png(
        native_window: isize,
        path: &Path,
        left: i32,
        top: i32,
        width: i32,
        height: i32,
    ) -> Result<ScreenshotWriteResult, MechanismError> {
        if native_window == 0 {
            return Err(MechanismError::Failed {
                code: "bad_handle".to_owned(),
                message: "native_window is 0".to_owned(),
            });
        }
        if width <= 0 || height <= 0 {
            return Err(MechanismError::Failed {
                code: "bad_dimensions".to_owned(),
                message: format!("region {width}x{height} must have a positive width and height"),
            });
        }
        let path_c = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| {
            MechanismError::Failed {
                code: "bad_path".to_owned(),
                message: "path contains an interior NUL byte".to_owned(),
            }
        })?;
        let f = super::call_sym::<super::CaptureWindow>(b"agt_screenshot_capture_window")?;
        let status = unsafe {
            f(
                native_window,
                path_c.as_ptr(),
                dynlib::AGT_SCREENSHOT_AREA_CLIENT,
                left,
                top,
                width,
                height,
            )
        };
        map_status("agt_screenshot_capture_window", status)?;
        let (output_width, output_height) = png_dimensions(path);
        let output_pixels = output_width as usize * output_height as usize;
        Ok(ScreenshotWriteResult {
            frame_width: output_width,
            frame_height: output_height,
            output_width,
            output_height,
            output_pixels,
        })
    }

    /// Read the width/height from a PNG file's IHDR (fixed offset 16..24).
    /// Returns `(0, 0)` when the header cannot be read — the capture itself
    /// already succeeded, so a broken header must not fail the command.
    fn png_dimensions(path: &Path) -> (u32, u32) {
        let mut header = [0u8; 24];
        let Ok(mut file) = std::fs::File::open(path) else {
            return (0, 0);
        };
        use std::io::Read;
        if file.read_exact(&mut header).is_err() {
            return (0, 0);
        }
        // PNG signature (8) + IHDR length/type (8) + width/height (4 + 4).
        if &header[0..8] != b"\x89PNG\r\n\x1a\n" || &header[12..16] != b"IHDR" {
            return (0, 0);
        }
        let width = u32::from_be_bytes(header[16..20].try_into().unwrap_or([0; 4]));
        let height = u32::from_be_bytes(header[20..24].try_into().unwrap_or([0; 4]));
        (width, height)
    }
}

// ---------------------------------------------------------------------------
// Accessibility-tree extras (event bus / write route diagnostics).
// ---------------------------------------------------------------------------

pub mod accessibility_tree {
    use super::{MechanismError, call_sym, map_status, read_two_stage};

    /// Drain the accessibility event bus (no user-visible side effects).
    pub fn drain_bus() -> Result<(), MechanismError> {
        let f = call_sym::<super::DrainBus>(b"agt_a11y_drain_bus")?;
        let status = unsafe { f() };
        map_status("agt_a11y_drain_bus", status)
    }

    /// Route of the last successful text write on this thread (diagnostic
    /// string).
    pub fn last_text_write_via() -> Result<String, MechanismError> {
        let bytes = read_two_stage(|buf, cap, out_len| {
            let f = call_sym::<super::LastTextWriteVia>(b"agt_a11y_last_text_write_via")?;
            let status = unsafe { f(buf, cap, out_len) };
            Ok::<i32, MechanismError>(status)
        })?;
        String::from_utf8(bytes).map_err(|_| MechanismError::Failed {
            code: "bad_encoding".into(),
            message: "write-route string is not UTF-8".into(),
        })
    }
}

// ---------------------------------------------------------------------------
// Accessibility tree (public API unchanged).
// ---------------------------------------------------------------------------

pub fn tree_for_window(window: Option<isize>) -> Result<A11yTree, MechanismError> {
    tree_for_window_bounded(window, TreeBudget::default())
}

/// `(major, minor)` of the loaded library, or the typed load failure.
fn loaded_abi_version() -> Result<(u16, u16), MechanismError> {
    let lib = dynlib::load().map_err(|error| MechanismError::Failed {
        code: "dylib_load".into(),
        message: error.message.clone(),
    })?;
    let version = lib
        .abi_version()
        .map_err(|message| MechanismError::Failed {
            code: "dylib_symbol".into(),
            message,
        })?;
    Ok(((version >> 16) as u16, (version & 0xffff) as u16))
}

/// Snapshot one window (or every root) under `budget`. With an ABI 1.12
/// library the budget is applied by the adapter during its walk and the
/// reply carries `truncated` / `visited` / `returned` plus per-node
/// `identifier`. An older library still serves an unbounded-budget snapshot
/// (`truncated: false`, counts equal to the node count); an explicit budget
/// against it is typed `Unsupported`, never silently ignored.
pub fn tree_for_window_bounded(
    window: Option<isize>,
    budget: TreeBudget,
) -> Result<A11yTree, MechanismError> {
    let handle = window.unwrap_or(0);
    let mut count = 0usize;
    let (major, minor) = loaded_abi_version()?;
    let bounded_abi = major == 1 && minor >= dynlib::TREE_BUDGET_ABI_MINOR;
    if bounded_abi {
        let f = call_sym::<TreeSnapshotBounded>(b"agt_a11y_tree_snapshot_bounded")?;
        let max_depth = budget
            .max_depth
            .and_then(|depth| i32::try_from(depth).ok())
            .unwrap_or(dynlib::AGT_A11Y_DEPTH_DEFAULT);
        let max_nodes = budget
            .max_nodes
            .and_then(|nodes| u32::try_from(nodes).ok())
            .unwrap_or(dynlib::AGT_A11Y_NODES_DEFAULT);
        let status = unsafe { f(handle, max_depth, max_nodes, &mut count) };
        map_status("agt_a11y_tree_snapshot_bounded", status)?;
    } else {
        if !budget.is_default() {
            return Err(MechanismError::Unsupported {
                reason: format!(
                    "tree depth / node budget requires ABI 1.{}, loaded library reports {major}.{minor}",
                    dynlib::TREE_BUDGET_ABI_MINOR
                ),
            });
        }
        let f = call_sym::<TreeSnapshot>(b"agt_a11y_tree_snapshot")?;
        let status = unsafe { f(handle, &mut count) };
        map_status("agt_a11y_tree_snapshot", status)?;
    }
    read_snapshot(window, count, bounded_abi)
}

/// Read the thread-local snapshot the library just filled: metadata, then
/// every node. `bounded_abi` selects the ABI 1.12 metadata and identifier
/// reads.
fn read_snapshot(
    window: Option<isize>,
    count: usize,
    bounded_abi: bool,
) -> Result<A11yTree, MechanismError> {
    let backend = read_meta_string(dynlib::AGT_A11Y_META_BACKEND)?;
    let root_id = read_meta_string(dynlib::AGT_A11Y_META_ROOT_ID)?;
    let (truncated, visited, returned) = if bounded_abi {
        (
            read_meta_string(dynlib::AGT_A11Y_META_TRUNCATED)? == "1",
            read_meta_count(dynlib::AGT_A11Y_META_VISITED)?,
            read_meta_count(dynlib::AGT_A11Y_META_RETURNED)?,
        )
    } else {
        (false, count, count)
    };
    let mut nodes = Vec::with_capacity(count);
    for index in 0..count {
        nodes.push(read_node(index, bounded_abi)?);
    }
    Ok(A11yTree {
        backend,
        window_handle: window,
        root_id,
        nodes,
        truncated,
        visited,
        returned,
    })
}

/// Typed `Unsupported` unless the loaded library is ABI 1.14 or later.
fn require_menu_focus_abi(what: &str) -> Result<(), MechanismError> {
    let (major, minor) = loaded_abi_version()?;
    if major == 1 && minor >= dynlib::MENU_FOCUS_ABI_MINOR {
        return Ok(());
    }
    Err(MechanismError::Unsupported {
        reason: format!(
            "{what} requires ABI 1.{}, loaded library reports {major}.{minor}",
            dynlib::MENU_FOCUS_ABI_MINOR
        ),
    })
}

fn abi_budget(budget: TreeBudget) -> (i32, u32) {
    (
        budget
            .max_depth
            .and_then(|depth| i32::try_from(depth).ok())
            .unwrap_or(dynlib::AGT_A11Y_DEPTH_DEFAULT),
        budget
            .max_nodes
            .and_then(|nodes| u32::try_from(nodes).ok())
            .unwrap_or(dynlib::AGT_A11Y_NODES_DEFAULT),
    )
}

/// Background menu-bar walk of the application owning `window` (ABI 1.14
/// `agt_a11y_menu_snapshot`): the same node shape as `tree`, rooted at the
/// menu bar. Never opens a menu or activates the application.
pub fn menu_tree_for_window_bounded(
    window: Option<isize>,
    budget: TreeBudget,
) -> Result<A11yTree, MechanismError> {
    require_menu_focus_abi("menu inspect")?;
    let handle = window.unwrap_or(0);
    let (max_depth, max_nodes) = abi_budget(budget);
    let mut count = 0usize;
    let f = call_sym::<MenuSnapshot>(b"agt_a11y_menu_snapshot")?;
    let status = unsafe { f(handle, max_depth, max_nodes, &mut count) };
    map_status("agt_a11y_menu_snapshot", status)?;
    read_snapshot(window, count, true)
}

/// What the library observed on a pressed menu item: its check mark before
/// the press and after the path resolved again (`None` = unmarked).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuReceipt {
    pub mark_before: Option<char>,
    pub mark_after: Option<char>,
}

/// Press the menu item at `path` in the background (ABI 1.14
/// `agt_a11y_menu_invoke`). Refusals (`a11y_menu_item_not_found` /
/// `a11y_menu_item_ambiguous` / `a11y_menu_item_disabled` /
/// `a11y_menu_item_not_leaf`) happen before anything is pressed.
pub fn invoke_menu_path(
    window: Option<isize>,
    path: &[String],
) -> Result<MenuReceipt, MechanismError> {
    require_menu_focus_abi("menu invoke")?;
    let handle = window.unwrap_or(0);
    let mut payload = Vec::new();
    for segment in path {
        if segment.contains('\0') {
            return Err(MechanismError::Failed {
                code: "invalid_input".into(),
                message: "a menu title cannot contain NUL".into(),
            });
        }
        payload.extend_from_slice(segment.as_bytes());
        payload.push(0);
    }
    let f = call_sym::<MenuInvoke>(b"agt_a11y_menu_invoke")?;
    let mut before = 0u32;
    let mut after = 0u32;
    let status = unsafe {
        f(
            handle,
            payload.as_ptr(),
            payload.len(),
            &mut before,
            &mut after,
        )
    };
    map_status("agt_a11y_menu_invoke", status)?;
    let mark = |scalar: u32| (scalar != 0).then(|| char::from_u32(scalar)).flatten();
    Ok(MenuReceipt {
        mark_before: mark(before),
        mark_after: mark(after),
    })
}

/// The application's own focused control inside `window` as a one-node
/// tree (ABI 1.14 `agt_a11y_focused_snapshot`); the node id is its path in
/// the window tree, so `invoke --node` can address it.
pub fn focused_node(window: Option<isize>) -> Result<A11yTree, MechanismError> {
    require_menu_focus_abi("focused")?;
    let handle = window.unwrap_or(0);
    let mut count = 0usize;
    let f = call_sym::<FocusedSnapshot>(b"agt_a11y_focused_snapshot")?;
    let status = unsafe { f(handle, &mut count) };
    map_status("agt_a11y_focused_snapshot", status)?;
    read_snapshot(window, count, true)
}

/// Ask the application owning `window` to build its full accessibility
/// tree (ABI 1.15 `agt_a11y_manual_accessibility_poke`).
///
/// Success means the request was delivered, **never** that the tree grew:
/// AppKit answers `kAXErrorAttributeUnsupported` for this attribute even
/// when the poke lands, so the caller proves it by reading the tree again.
pub fn poke_manual_accessibility(window: isize) -> Result<(), MechanismError> {
    let (major, minor) = loaded_abi_version()?;
    if major != 1 || minor < dynlib::MANUAL_ACCESSIBILITY_ABI_MINOR {
        return Err(MechanismError::Unsupported {
            reason: format!(
                "unlock --poke requires ABI 1.{}, loaded library reports {major}.{minor}",
                dynlib::MANUAL_ACCESSIBILITY_ABI_MINOR
            ),
        });
    }
    let f = call_sym::<ManualAccessibilityPoke>(b"agt_a11y_manual_accessibility_poke")?;
    let status = unsafe { f(window) };
    map_status("agt_a11y_manual_accessibility_poke", status)?;
    Ok(())
}

/// One installed application.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct InstalledApp {
    pub name: String,
    pub path: String,
}

/// Every application this host has installed (ABI 1.21), plus whether the
/// adapter's bound cut the listing short.
///
/// `Unsupported` means the host cannot enumerate installed applications,
/// which is not the same as having none -- the caller reports the
/// difference.
pub fn list_installed_apps() -> Result<(Vec<InstalledApp>, bool), MechanismError> {
    require_app_inventory_abi("apps --all")?;
    let bytes = read_two_stage(|buf, cap, out_len| {
        let f = call_sym::<AppListInstalled>(b"agt_app_list_installed")?;
        let status = unsafe { f(buf, cap, out_len) };
        Ok::<i32, MechanismError>(status)
    })?;
    let listing = String::from_utf8(bytes).map_err(|_| MechanismError::Failed {
        code: "bad_encoding".into(),
        message: "installed application listing is not UTF-8".into(),
    })?;
    let mut apps = Vec::new();
    let mut truncated = false;
    for line in listing.lines() {
        if line == "\ttruncated" {
            truncated = true;
            continue;
        }
        let Some((name, path)) = line.split_once('\t') else {
            continue;
        };
        if name.is_empty() || path.is_empty() {
            continue;
        }
        apps.push(InstalledApp {
            name: name.to_owned(),
            path: path.to_owned(),
        });
    }
    Ok((apps, truncated))
}

/// Ask the host to start the application at `path` (ABI 1.21).
///
/// Success means the request was accepted, never that the application is
/// up: the launcher service owns the process it starts, so no pid comes
/// back. The caller finds it by looking for the window that appears.
pub fn launch_app(path: &str) -> Result<(), MechanismError> {
    require_app_inventory_abi("app launch")?;
    let f = call_sym::<AppLaunch>(b"agt_app_launch")?;
    let status = unsafe { f(path.as_ptr(), path.len()) };
    map_status("agt_app_launch", status)?;
    Ok(())
}

fn require_app_inventory_abi(what: &str) -> Result<(), MechanismError> {
    let (major, minor) = loaded_abi_version()?;
    if major == 1 && minor >= dynlib::APP_INVENTORY_ABI_MINOR {
        return Ok(());
    }
    Err(MechanismError::Unsupported {
        reason: format!(
            "{what} requires ABI 1.{}, loaded library reports {major}.{minor}",
            dynlib::APP_INVENTORY_ABI_MINOR
        ),
    })
}

/// Hide or unhide an application by pid (ABI 1.20).
///
/// The application-level verb: hiding steps the whole app aside, which is
/// neither minimizing a window nor closing one. Idempotent.
pub fn set_application_hidden(process_id: u32, hidden: bool) -> Result<(), MechanismError> {
    let (major, minor) = loaded_abi_version()?;
    if major != 1 || minor < dynlib::APPLICATION_HIDDEN_ABI_MINOR {
        return Err(MechanismError::Unsupported {
            reason: format!(
                "application hide requires ABI 1.{}, loaded library reports {major}.{minor}",
                dynlib::APPLICATION_HIDDEN_ABI_MINOR
            ),
        });
    }
    let f = call_sym::<ApplicationSetHidden>(b"agt_a11y_application_set_hidden")?;
    let status = unsafe { f(process_id, i32::from(hidden)) };
    map_status("agt_a11y_application_set_hidden", status)?;
    Ok(())
}

/// One event the backend itself reported.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct A11yEvent {
    pub notification: String,
    pub node_id: String,
    pub role: String,
    pub name: String,
    pub t_ms: u64,
}

/// Watch one window through the backend's own notifications (ABI 1.18
/// `agt_a11y_observe_window`). Blocking and bounded.
///
/// `Unsupported` here is the caller's cue to fall back to poll-diff and
/// say which mode it used; it is never an error to report to the user.
pub fn observe_window(
    window: isize,
    duration_ms: u64,
    max_events: usize,
) -> Result<Vec<A11yEvent>, MechanismError> {
    let (major, minor) = loaded_abi_version()?;
    if major != 1 || minor < dynlib::OBSERVE_NOTIFICATIONS_ABI_MINOR {
        return Err(MechanismError::Unsupported {
            reason: format!(
                "native observation requires ABI 1.{}, loaded library reports {major}.{minor}",
                dynlib::OBSERVE_NOTIFICATIONS_ABI_MINOR
            ),
        });
    }
    let f = call_sym::<ObserveWindow>(b"agt_a11y_observe_window")?;
    let mut count = 0usize;
    let status = unsafe { f(window, duration_ms, max_events, &mut count) };
    map_status("agt_a11y_observe_window", status)?;
    let time = call_sym::<ObserveEventTime>(b"agt_a11y_observe_event_time_ms")?;
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let mut t_ms = 0u64;
        let status = unsafe { time(index, &mut t_ms) };
        map_status("agt_a11y_observe_event_time_ms", status)?;
        out.push(A11yEvent {
            notification: observe_event_string(index, dynlib::AGT_A11Y_EVENT_STR_NOTIFICATION)?,
            node_id: observe_event_string(index, dynlib::AGT_A11Y_EVENT_STR_NODE_ID)?,
            role: observe_event_string(index, dynlib::AGT_A11Y_EVENT_STR_ROLE)?,
            name: observe_event_string(index, dynlib::AGT_A11Y_EVENT_STR_NAME)?,
            t_ms,
        });
    }
    Ok(out)
}

fn observe_event_string(index: usize, kind: i32) -> Result<String, MechanismError> {
    let bytes = read_two_stage(|buf, cap, out_len| {
        let f = call_sym::<ObserveEventString>(b"agt_a11y_observe_event_string")?;
        let status = unsafe { f(index, kind, buf, cap, out_len) };
        Ok::<i32, MechanismError>(status)
    })?;
    String::from_utf8(bytes).map_err(|_| MechanismError::Failed {
        code: "bad_encoding".into(),
        message: "observation event string is not UTF-8".into(),
    })
}

fn read_meta_count(field: i32) -> Result<usize, MechanismError> {
    let text = read_meta_string(field)?;
    text.trim().parse().map_err(|_| MechanismError::Failed {
        code: "bad_encoding".into(),
        message: format!("snapshot metadata field {field} is not a count: {text:?}"),
    })
}

/// `Click` / `Focus` go through `agt_a11y_node_perform` so an older library
/// still serves them; every `invoke` action needs ABI 1.13
/// `agt_a11y_node_invoke`, and an older library answers a typed
/// `Unsupported` rather than a silently different action.
pub fn perform_node_action(
    window: Option<isize>,
    node_id: &str,
    action: NodeAction,
) -> Result<(), MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    let (kind, value) = action.abi_parts();
    write_ledger::note();
    if matches!(action, NodeAction::Click | NodeAction::Focus) {
        let f = call_sym::<NodePerform>(b"agt_a11y_node_perform")?;
        let status = unsafe { f(handle, node_c.as_ptr(), kind) };
        map_status("agt_a11y_node_perform", status)?;
        return Ok(());
    }
    let (major, minor) = loaded_abi_version()?;
    if !(major == 1 && minor >= dynlib::NODE_INVOKE_ABI_MINOR) {
        return Err(MechanismError::Unsupported {
            reason: format!(
                "invoke {} requires ABI 1.{}, loaded library reports {major}.{minor}",
                action.name(),
                dynlib::NODE_INVOKE_ABI_MINOR
            ),
        });
    }
    let f = call_sym::<NodeInvoke>(b"agt_a11y_node_invoke")?;
    let payload = value.unwrap_or_default();
    let status = unsafe {
        f(
            handle,
            node_c.as_ptr(),
            kind,
            payload.as_ptr(),
            payload.len(),
        )
    };
    map_status("agt_a11y_node_invoke", status)?;
    Ok(())
}

pub fn set_node_text(
    window: Option<isize>,
    node_id: &str,
    text: &str,
) -> Result<(), MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    write_ledger::note();
    let f = call_sym::<NodeSetText>(b"agt_a11y_node_set_text")?;
    let status = unsafe { f(handle, node_c.as_ptr(), text.as_ptr(), text.len()) };
    map_status("agt_a11y_node_set_text", status)?;
    Ok(())
}

/// Independent AT-SPI `Text.GetText` for a resolved child-index path.
/// Does not reuse `set_node_text` confirmation, `last_text_write_via`, or
/// a tree snapshot `text` field. Distinguishes a real mechanism failure
/// from the two-stage empty-payload probe (`buffer_too_small` + required
/// == 0).
pub fn get_node_text(window: Option<isize>, node_id: &str) -> Result<String, MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    let f = call_sym::<NodeGetText>(b"agt_a11y_node_get_text")?;
    let mut required = 0usize;
    let status = unsafe {
        f(
            handle,
            node_c.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut required,
        )
    };
    if status == dynlib::AGT_UNSUPPORTED {
        return Err(MechanismError::Unsupported {
            reason: "agt_a11y_node_get_text: mechanism unavailable on this host".to_owned(),
        });
    }
    if status != dynlib::AGT_FAILED {
        return Err(last_mechanism_error("agt_a11y_node_get_text"));
    }
    let probe = last_mechanism_error("agt_a11y_node_get_text");
    match &probe {
        MechanismError::Failed { code, .. } if code == "buffer_too_small" => {}
        other => return Err(other.clone()),
    }
    if required == 0 {
        return Ok(String::new());
    }
    let mut buf = vec![0u8; required];
    let status = unsafe {
        f(
            handle,
            node_c.as_ptr(),
            buf.as_mut_ptr(),
            required,
            &mut required,
        )
    };
    map_status("agt_a11y_node_get_text", status)?;
    buf.truncate(required);
    String::from_utf8(buf).map_err(|_| MechanismError::Failed {
        code: "bad_encoding".into(),
        message: "node text is not UTF-8".into(),
    })
}

/// One-shot AT-SPI `Component.ScrollTo(TopEdge)` for a resolved child-index
/// path. Missing / false / `UnknownMethod` is `a11y_scroll_unavailable`.
pub fn scroll_node(window: Option<isize>, node_id: &str) -> Result<(), MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    let f = call_sym::<NodeScroll>(b"agt_a11y_node_scroll")?;
    let status = unsafe { f(handle, node_c.as_ptr()) };
    map_status("agt_a11y_node_scroll", status)?;
    Ok(())
}

/// Independent AT-SPI `Component.GetExtents(Screen)` for a resolved
/// child-index path. Not a tree-snapshot `bounds` field.
pub fn get_node_extents(
    window: Option<isize>,
    node_id: &str,
) -> Result<A11yBounds, MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    let f = call_sym::<NodeGetExtents>(b"agt_a11y_node_get_extents")?;
    let mut x = 0i32;
    let mut y = 0i32;
    let mut width = 0i32;
    let mut height = 0i32;
    let status = unsafe {
        f(
            handle,
            node_c.as_ptr(),
            &mut x,
            &mut y,
            &mut width,
            &mut height,
        )
    };
    map_status("agt_a11y_node_get_extents", status)?;
    if width <= 0 || height <= 0 {
        return Err(MechanismError::Failed {
            code: "a11y_extents_unavailable".into(),
            message: format!("Component.GetExtents returned empty rect {width}x{height}"),
        });
    }
    Ok(A11yBounds {
        x,
        y,
        width,
        height,
    })
}

/// Independent AT-SPI `Text` selection (`GetNSelections` + `GetSelection`).
/// `n == 0` is empty (start/end stay 0).
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct A11ySelection {
    pub n: i32,
    pub start: i32,
    pub end: i32,
}

/// One-shot AT-SPI `Text.SetSelection(0, start, end)` for a resolved
/// child-index path. Missing Text / `UnknownMethod` is
/// `a11y_selection_unavailable`. SetSelection false is
/// `a11y_selection_no_effect`.
pub fn set_node_selection(
    window: Option<isize>,
    node_id: &str,
    start: i32,
    end: i32,
) -> Result<(), MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    let f = call_sym::<NodeSetSelection>(b"agt_a11y_node_set_selection")?;
    let status = unsafe { f(handle, node_c.as_ptr(), start, end) };
    map_status("agt_a11y_node_set_selection", status)?;
    Ok(())
}

/// Independent AT-SPI `Text.GetNSelections` + `GetSelection(0)` for a
/// resolved child-index path. Not the set-selection reply payload.
pub fn get_node_selection(
    window: Option<isize>,
    node_id: &str,
) -> Result<A11ySelection, MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    let f = call_sym::<NodeGetSelection>(b"agt_a11y_node_get_selection")?;
    let mut n = 0i32;
    let mut start = 0i32;
    let mut end = 0i32;
    let status = unsafe { f(handle, node_c.as_ptr(), &mut n, &mut start, &mut end) };
    map_status("agt_a11y_node_get_selection", status)?;
    Ok(A11ySelection { n, start, end })
}

/// One-shot AT-SPI `Text.SetCaretOffset` for a resolved child-index
/// path. Missing Text / `UnknownMethod` is `a11y_caret_unavailable`.
/// SetCaretOffset false is `a11y_caret_no_effect`.
pub fn set_node_caret_offset(
    window: Option<isize>,
    node_id: &str,
    offset: i32,
) -> Result<(), MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    let f = call_sym::<NodeSetCaretOffset>(b"agt_a11y_node_set_caret_offset")?;
    let status = unsafe { f(handle, node_c.as_ptr(), offset) };
    map_status("agt_a11y_node_set_caret_offset", status)?;
    Ok(())
}

/// Independent AT-SPI `Text.CaretOffset` / `GetCaretOffset` for a
/// resolved child-index path. Not the set-caret reply payload.
pub fn get_node_caret_offset(window: Option<isize>, node_id: &str) -> Result<i32, MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    let f = call_sym::<NodeGetCaretOffset>(b"agt_a11y_node_get_caret_offset")?;
    let mut offset = 0i32;
    let status = unsafe { f(handle, node_c.as_ptr(), &mut offset) };
    map_status("agt_a11y_node_get_caret_offset", status)?;
    Ok(offset)
}

/// Resident menu and global-shortcut host through libagenterm.
pub mod desktop_host {
    use std::ffi::c_void;
    use std::ptr::null;

    use super::{MechanismError, call_sym, map_status};
    use crate::dynlib;

    #[derive(Clone, Copy, Debug)]
    pub struct ActionSpec<'a> {
        pub action_id: u32,
        pub label: &'a str,
        pub shortcut: Option<&'a str>,
    }

    pub struct DesktopHost {
        raw: *mut c_void,
    }

    impl DesktopHost {
        pub fn open(actions: &[ActionSpec<'_>]) -> Result<Self, MechanismError> {
            let records: Vec<_> = actions
                .iter()
                .map(|action| dynlib::agt_desktop_action {
                    action_id: action.action_id,
                    label: action.label.as_ptr(),
                    label_len: action.label.len(),
                    shortcut: action.shortcut.map_or(null(), str::as_ptr),
                    shortcut_len: action.shortcut.map_or(0, str::len),
                })
                .collect();
            let open = call_sym::<super::DesktopHostOpen>(b"agt_desktop_host_open")?;
            let mut raw = std::ptr::null_mut();
            let status = unsafe { open(records.as_ptr(), records.len(), &mut raw) };
            map_status("agt_desktop_host_open", status)?;
            if raw.is_null() {
                return Err(MechanismError::Failed {
                    code: "desktop_host_null".into(),
                    message: "agt_desktop_host_open succeeded without a host".into(),
                });
            }
            Ok(Self { raw })
        }

        pub fn poll(&mut self, timeout_ms: u32) -> Result<Option<u32>, MechanismError> {
            let poll = call_sym::<super::DesktopHostPoll>(b"agt_desktop_host_poll")?;
            let mut action_id = 0;
            let status = unsafe { poll(self.raw, timeout_ms, &mut action_id) };
            map_status("agt_desktop_host_poll", status)?;
            Ok((action_id != 0).then_some(action_id))
        }

        pub fn close(mut self) -> Result<(), MechanismError> {
            let result = self.close_inner();
            if result.is_ok() {
                self.raw = std::ptr::null_mut();
            }
            result
        }

        fn close_inner(&mut self) -> Result<(), MechanismError> {
            if self.raw.is_null() {
                return Ok(());
            }
            let close = call_sym::<super::DesktopHostClose>(b"agt_desktop_host_close")?;
            map_status("agt_desktop_host_close", unsafe { close(self.raw) })
        }
    }

    impl Drop for DesktopHost {
        fn drop(&mut self) {
            if self.close_inner().is_ok() {
                self.raw = std::ptr::null_mut();
            }
        }
    }
}

/// Clipboard through libagenterm (`agt_clipboard_*`). Named `paste` seeds
/// and reads here; named `copy` publishes GetText here. Neither injects
/// Ctrl+V or XTest.
pub mod clipboard {
    use super::{MechanismError, call_sym, last_mechanism_error, map_status};
    use crate::dynlib;

    /// Hidden `cu` argv that owns CLIPBOARD after `SetSelectionOwner` so a
    /// later process can `ConvertSelection`. Not a public command.
    pub const X11_CLIPBOARD_OWNER_ARG: &str = "__agenterm-internal-x11-clipboard-own";

    /// Env the owner process sets so `agt_clipboard_set_text` stays in the
    /// X11 selection event loop instead of returning and dropping the owner.
    pub const X11_CLIPBOARD_SERVE_ENV: &str = "PLATFORM_X11_CLIPBOARD_SERVE";

    pub fn set_text(text: &str) -> Result<(), MechanismError> {
        let f = call_sym::<super::ClipboardSetText>(b"agt_clipboard_set_text")?;
        let status = unsafe { f(text.as_ptr(), text.len()) };
        map_status("agt_clipboard_set_text", status)
    }

    /// Publish UTF-8 so a later `cu` process can read it. On Linux X11 the
    /// CLIPBOARD owner must outlive this caller; a detached `cu` owner
    /// answers `SelectionRequest`. Other hosts use in-process `set_text`.
    pub fn publish_text(text: &str) -> Result<(), MechanismError> {
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("DISPLAY").is_some() {
                return publish_x11_clipboard(text);
            }
        }
        set_text(text)
    }

    #[cfg(target_os = "linux")]
    fn publish_x11_clipboard(text: &str) -> Result<(), MechanismError> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        use std::thread;
        use std::time::{Duration, Instant};

        let exe = std::env::current_exe().map_err(|error| MechanismError::Failed {
            code: "clipboard_failed".into(),
            message: format!("could not locate agenterm-cu to persist CLIPBOARD: {error}"),
        })?;
        let exe_name = exe.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if exe_name != "agenterm-cu" && exe_name != "agenterm-cu.exe" {
            return set_text(text);
        }
        let mut child = Command::new(&exe);
        child
            .arg(X11_CLIPBOARD_OWNER_ARG)
            .env(X11_CLIPBOARD_SERVE_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            child.process_group(0);
        }
        let mut child = child.spawn().map_err(|error| MechanismError::Failed {
            code: "clipboard_failed".into(),
            message: format!("could not start CLIPBOARD owner: {error}"),
        })?;
        match child.stdin.take() {
            Some(mut stdin) => stdin.write_all(text.as_bytes()).map_err(|error| {
                let _ = child.kill();
                MechanismError::Failed {
                    code: "clipboard_failed".into(),
                    message: format!("could not send CLIPBOARD payload: {error}"),
                }
            })?,
            None => {
                let _ = child.kill();
                return Err(MechanismError::Failed {
                    code: "clipboard_failed".into(),
                    message: "CLIPBOARD owner stdin is missing".into(),
                });
            }
        }
        let deadline = Instant::now() + Duration::from_millis(2_000);
        loop {
            match get_text() {
                Ok(got) if got == text => return Ok(()),
                Ok(_) | Err(_) => {
                    if let Ok(Some(status)) = child.try_wait() {
                        return Err(MechanismError::Failed {
                            code: "clipboard_failed".into(),
                            message: format!("CLIPBOARD owner exited before serving ({status})"),
                        });
                    }
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        return Err(MechanismError::Failed {
                            code: "clipboard_failed".into(),
                            message: "CLIPBOARD owner did not become readable in time".into(),
                        });
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }

    /// Owner-process entry: publish `text` and (on X11 with the serve env)
    /// block in the selection loop until replaced.
    pub fn own_text(text: &str) -> Result<(), MechanismError> {
        set_text(text)
    }

    /// Two-stage `agt_clipboard_types` (ABI 1.19): the type names on the
    /// clipboard, in the host's own spelling.
    ///
    /// `Unsupported` means the host cannot enumerate types, which is a
    /// different fact from an empty clipboard -- the caller reports the
    /// difference rather than folding both into an empty list.
    pub fn available_types() -> Result<Vec<String>, MechanismError> {
        let (major, minor) = super::loaded_abi_version()?;
        if major != 1 || minor < dynlib::CLIPBOARD_TYPES_ABI_MINOR {
            return Err(MechanismError::Unsupported {
                reason: format!(
                    "clipboard type listing requires ABI 1.{}, loaded library reports {major}.{minor}",
                    dynlib::CLIPBOARD_TYPES_ABI_MINOR
                ),
            });
        }
        let bytes = super::read_two_stage(|buf, cap, out_len| {
            let f = call_sym::<super::ClipboardTypes>(b"agt_clipboard_types")?;
            let status = unsafe { f(buf, cap, out_len) };
            Ok::<i32, MechanismError>(status)
        })?;
        let listing = String::from_utf8(bytes).map_err(|_| MechanismError::Failed {
            code: "bad_encoding".into(),
            message: "clipboard type listing is not UTF-8".into(),
        })?;
        Ok(listing
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Two-stage `agt_clipboard_get_text`. No Unicode text is an empty
    /// success (`AGT_OK`, `out_len == 0`), not `buffer_too_small`.
    pub fn get_text() -> Result<String, MechanismError> {
        let f = call_sym::<super::ClipboardGetText>(b"agt_clipboard_get_text")?;
        let mut required = 0usize;
        let status = unsafe { f(std::ptr::null_mut(), 0, &mut required) };
        if status == dynlib::AGT_UNSUPPORTED {
            return Err(MechanismError::Unsupported {
                reason: "agt_clipboard_get_text: mechanism unavailable on this host".to_owned(),
            });
        }
        if status == dynlib::AGT_OK {
            return Ok(String::new());
        }
        if status != dynlib::AGT_FAILED {
            return Err(last_mechanism_error("agt_clipboard_get_text"));
        }
        let probe = last_mechanism_error("agt_clipboard_get_text");
        match &probe {
            MechanismError::Failed { code, .. } if code == "buffer_too_small" => {}
            other => return Err(other.clone()),
        }
        if required == 0 {
            return Ok(String::new());
        }
        let mut buf = vec![0u8; required];
        let status = unsafe { f(buf.as_mut_ptr(), required, &mut required) };
        map_status("agt_clipboard_get_text", status)?;
        buf.truncate(required);
        String::from_utf8(buf).map_err(|_| MechanismError::Failed {
            code: "bad_encoding".into(),
            message: "clipboard text is not UTF-8".into(),
        })
    }

    /// Two-stage `agt_clipboard_get` (ABI 1.23): one host type as raw bytes.
    pub fn get_type(type_name: &str, max_bytes: usize) -> Result<Vec<u8>, MechanismError> {
        let (major, minor) = super::loaded_abi_version()?;
        if major != 1 || minor < dynlib::CLIPBOARD_GET_ABI_MINOR {
            return Err(MechanismError::Unsupported {
                reason: format!(
                    "clipboard type read requires ABI 1.{}, loaded library reports {major}.{minor}",
                    dynlib::CLIPBOARD_GET_ABI_MINOR
                ),
            });
        }
        let f = call_sym::<super::ClipboardGet>(b"agt_clipboard_get")?;
        let type_bytes = type_name.as_bytes();
        let mut required = 0usize;
        let status = unsafe {
            f(
                type_bytes.as_ptr(),
                type_bytes.len(),
                std::ptr::null_mut(),
                0,
                &mut required,
            )
        };
        if status == dynlib::AGT_UNSUPPORTED {
            return Err(MechanismError::Unsupported {
                reason: "agt_clipboard_get: mechanism unavailable on this host".to_owned(),
            });
        }
        if status == dynlib::AGT_OK {
            return Ok(Vec::new());
        }
        if status != dynlib::AGT_FAILED {
            return Err(last_mechanism_error("agt_clipboard_get"));
        }
        let probe = last_mechanism_error("agt_clipboard_get");
        match &probe {
            MechanismError::Failed { code, .. } if code == "buffer_too_small" => {}
            other => return Err(other.clone()),
        }
        if required == 0 {
            return Ok(Vec::new());
        }
        if required > max_bytes {
            return Err(MechanismError::Failed {
                code: "clipboard_too_large".into(),
                message: format!(
                    "clipboard type payload is {required} bytes; max-bytes is {max_bytes}"
                ),
            });
        }
        let mut buf = vec![0u8; required];
        let status = unsafe {
            f(
                type_bytes.as_ptr(),
                type_bytes.len(),
                buf.as_mut_ptr(),
                required,
                &mut required,
            )
        };
        map_status("agt_clipboard_get", status)?;
        buf.truncate(required);
        Ok(buf)
    }

    fn require_set_abi() -> Result<(), MechanismError> {
        let (major, minor) = super::loaded_abi_version()?;
        if major != 1 || minor < dynlib::CLIPBOARD_SET_ABI_MINOR {
            return Err(MechanismError::Unsupported {
                reason: format!(
                    "clipboard type write requires ABI 1.{}, loaded library reports {major}.{minor}",
                    dynlib::CLIPBOARD_SET_ABI_MINOR
                ),
            });
        }
        Ok(())
    }

    pub fn set_type(type_name: &str, bytes: &[u8]) -> Result<(), MechanismError> {
        require_set_abi()?;
        let f = call_sym::<super::ClipboardSet>(b"agt_clipboard_set")?;
        let status = unsafe {
            f(
                type_name.as_ptr(),
                type_name.len(),
                bytes.as_ptr(),
                bytes.len(),
            )
        };
        map_status("agt_clipboard_set", status)
    }

    pub fn set_file(path: &str) -> Result<(), MechanismError> {
        require_set_abi()?;
        let f = call_sym::<super::ClipboardSetFile>(b"agt_clipboard_set_file")?;
        let status = unsafe { f(path.as_ptr(), path.len()) };
        map_status("agt_clipboard_set_file", status)
    }

    pub fn clear() -> Result<(), MechanismError> {
        require_set_abi()?;
        let f = call_sym::<super::ClipboardClear>(b"agt_clipboard_clear")?;
        let status = unsafe { f() };
        map_status("agt_clipboard_clear", status)
    }
}

pub fn send_node_keys(
    window: Option<isize>,
    node_id: &str,
    keys: &str,
) -> Result<(), MechanismError> {
    let handle = window.unwrap_or(0);
    let node_c = CStringOrStack::new(node_id)?;
    let f = call_sym::<NodeSendKeys>(b"agt_a11y_node_send_keys")?;
    let status = unsafe { f(handle, node_c.as_ptr(), keys.as_ptr(), keys.len()) };
    map_status("agt_a11y_node_send_keys", status)?;
    Ok(())
}

fn read_node(index: usize, with_identifier: bool) -> Result<A11yNode, MechanismError> {
    let mut record = agt_a11y_node {
        bounds_x: 0,
        bounds_y: 0,
        bounds_width: 0,
        bounds_height: 0,
        id: [0u8; 64],
        id_len: 0,
        id_truncated: 0,
        parent_id: [0u8; 64],
        parent_id_len: 0,
        parent_id_truncated: 0,
        has_parent: 0,
        actions_count: 0,
    };
    let f = call_sym::<TreeNode>(b"agt_a11y_tree_node")?;
    let status = unsafe { f(index, &mut record) };
    map_status("agt_a11y_tree_node", status)?;
    // The record's `id` / `parent_id` are fixed 64-byte arrays. A
    // truncated id is not a shortened id, it is a **wrong** one: two nodes
    // whose ids agree in the first 64 bytes become the same node, and a
    // node can become its own parent. Measured on Windows, where a UI
    // Automation runtime path is long: six non-client elements collapsed
    // onto one id, five of them self-parented, and the menu flattening
    // then walked that cycle to 2 GB. Read the whole id through the
    // two-stage string reader where the loaded library can supply it.
    let whole_ids = loaded_abi_version()
        .map(|(major, minor)| major == 1 && minor >= dynlib::NODE_ID_STRING_ABI_MINOR)
        .unwrap_or(false);
    let id = if whole_ids {
        read_node_string(index, dynlib::AGT_A11Y_STR_ID)?
    } else if record.id_truncated != 0 {
        // An older library cannot hand back the rest, and guessing would
        // hand back a collision. Refuse instead of addressing the wrong
        // node later.
        return Err(MechanismError::Failed {
            code: "a11y_node_id_truncated".into(),
            message: format!(
                "node {index}'s id does not fit the {} -byte record and this library is too old to send the rest; upgrade libagenterm to ABI 1.{}",
                record.id.len(),
                dynlib::NODE_ID_STRING_ABI_MINOR
            ),
        });
    } else {
        fixed_field(&record.id, record.id_len)
    };
    let parent_id = if record.has_parent == 0 {
        None
    } else if whole_ids {
        let parent = read_node_string(index, dynlib::AGT_A11Y_STR_PARENT_ID)?;
        if parent.is_empty() {
            None
        } else {
            Some(parent)
        }
    } else if record.parent_id_truncated != 0 {
        return Err(MechanismError::Failed {
            code: "a11y_node_id_truncated".into(),
            message: format!("node {index}'s parent id is truncated; upgrade libagenterm"),
        });
    } else {
        Some(fixed_field(&record.parent_id, record.parent_id_len))
    };
    let role = read_node_string(index, dynlib::AGT_A11Y_STR_ROLE)?;
    let name = read_node_string(index, dynlib::AGT_A11Y_STR_NAME)?;
    let text_raw = read_node_string(index, dynlib::AGT_A11Y_STR_TEXT)?;
    let text = if text_raw.is_empty() {
        None
    } else {
        Some(text_raw)
    };
    let states_raw = read_node_string(index, dynlib::AGT_A11Y_STR_STATES)?;
    let states = if states_raw.is_empty() {
        Vec::new()
    } else {
        states_raw.split(',').map(str::to_owned).collect()
    };
    let mut actions = Vec::with_capacity(record.actions_count as usize);
    for action_index in 0..record.actions_count as usize {
        actions.push(read_action_name(index, action_index)?);
    }
    let identifier = if with_identifier {
        let raw = read_node_string(index, dynlib::AGT_A11Y_STR_IDENTIFIER)?;
        (!raw.is_empty()).then_some(raw)
    } else {
        None
    };
    Ok(A11yNode {
        id,
        parent_id,
        role,
        name,
        states,
        bounds: A11yBounds {
            x: record.bounds_x,
            y: record.bounds_y,
            width: record.bounds_width,
            height: record.bounds_height,
        },
        actions,
        text,
        identifier,
    })
}

fn read_meta_string(field: i32) -> Result<String, MechanismError> {
    let bytes = read_two_stage(|buf, cap, out_len| {
        let f = call_sym::<MetaString>(b"agt_a11y_tree_meta_string")?;
        let status = unsafe { f(field, buf, cap, out_len) };
        Ok::<i32, MechanismError>(status)
    })?;
    String::from_utf8(bytes).map_err(|_| MechanismError::Failed {
        code: "bad_encoding".into(),
        message: "metadata string is not UTF-8".into(),
    })
}

fn read_node_string(node_index: usize, kind: i32) -> Result<String, MechanismError> {
    let bytes = read_two_stage(|buf, cap, out_len| {
        let f = call_sym::<NodeString>(b"agt_a11y_node_string")?;
        let status = unsafe { f(node_index, kind, buf, cap, out_len) };
        Ok::<i32, MechanismError>(status)
    })?;
    String::from_utf8(bytes).map_err(|_| MechanismError::Failed {
        code: "bad_encoding".into(),
        message: "node string is not UTF-8".into(),
    })
}

fn read_action_name(node_index: usize, action_index: usize) -> Result<String, MechanismError> {
    let bytes = read_two_stage(|buf, cap, out_len| {
        let f = call_sym::<NodeActionName>(b"agt_a11y_node_action_name")?;
        let status = unsafe { f(node_index, action_index, buf, cap, out_len) };
        Ok::<i32, MechanismError>(status)
    })?;
    String::from_utf8(bytes).map_err(|_| MechanismError::Failed {
        code: "bad_encoding".into(),
        message: "action name is not UTF-8".into(),
    })
}

fn read_two_stage(
    mut probe: impl FnMut(*mut u8, usize, *mut usize) -> Result<i32, MechanismError>,
) -> Result<Vec<u8>, MechanismError> {
    let mut required = 0usize;
    let status = probe(std::ptr::null_mut(), 0, &mut required)?;
    // AGT_UNSUPPORTED is self-describing and records no error, so reading
    // the error slot for it returns whatever was there last -- "ok: no
    // error" on a clean slot, which is not a reason for anything.
    if status == dynlib::AGT_UNSUPPORTED {
        return Err(MechanismError::Unsupported {
            reason: "this host does not offer the mechanism behind this read".to_owned(),
        });
    }
    if status != dynlib::AGT_FAILED {
        return Err(last_mechanism_error("two_stage_probe"));
    }
    // libagenterm treats cap==0 as a size probe even when the payload is
    // empty, so AGT_FAILED with `required` still 0 means one of two things:
    // the probe answered for an empty payload, or the call failed before it
    // could write out_len at all. Reading the second as the first told
    // `clipboard-read` that a host with no clipboard helper installed was
    // carrying zero types -- `types_available: true` about a probe that
    // never ran.
    //
    // One more call separates them without consulting the global error
    // slot, which an export that fails early never populates: read with a
    // real one-byte buffer. An empty payload fits (AGT_OK, out_len 0); a
    // failing call fails again and is reported as the failure it is.
    let mut buf = vec![0u8; required.max(1)];
    let capacity = buf.len();
    let status = probe(buf.as_mut_ptr(), capacity, &mut required)?;
    map_status("two_stage_read", status)?;
    buf.truncate(required);
    Ok(buf)
}

fn fixed_field(bytes: &[u8], len: u32) -> String {
    String::from_utf8_lossy(&bytes[..len as usize]).into_owned()
}

fn map_status(operation: &str, status: i32) -> Result<(), MechanismError> {
    match status {
        dynlib::AGT_OK => Ok(()),
        dynlib::AGT_UNSUPPORTED => Err(unsupported_reason(operation)),
        _ => Err(last_mechanism_error(operation)),
    }
}

/// Why this host cannot do it, in the adapter's own words when it left
/// them.
///
/// `AGT_UNSUPPORTED` records nothing by the old convention, so every
/// distinct reason -- "AT-SPI2 has no application-level hidden state",
/// "the entry sets Terminal=true" -- reached callers as one generic
/// sentence. Libraries that do leave a reason tag it `unsupported` under
/// the operation that produced it; anything else in the slot is a
/// leftover from an earlier call and is ignored.
fn unsupported_reason(operation: &str) -> MechanismError {
    let generic = || MechanismError::Unsupported {
        reason: format!("{operation}: mechanism unavailable on this host"),
    };
    let mut err = agt_error {
        operation: std::ptr::null(),
        code: std::ptr::null(),
        message: std::ptr::null(),
    };
    let read = call_sym::<LastError>(b"agt_last_error")
        .ok()
        .map(|f| unsafe { f(&mut err) })
        .unwrap_or(dynlib::AGT_FAILED);
    if read != dynlib::AGT_OK {
        return generic();
    }
    let recorded_operation = cstr_to_string(err.operation).unwrap_or_default();
    let code = cstr_to_string(err.code).unwrap_or_default();
    let message = cstr_to_string(err.message).unwrap_or_default();
    if code != "unsupported" || recorded_operation != operation || message.is_empty() {
        return generic();
    }
    MechanismError::Unsupported { reason: message }
}

fn last_mechanism_error(operation: &str) -> MechanismError {
    let mut err = agt_error {
        operation: std::ptr::null(),
        code: std::ptr::null(),
        message: std::ptr::null(),
    };
    let read = call_sym::<LastError>(b"agt_last_error")
        .ok()
        .map(|f| unsafe { f(&mut err) })
        .unwrap_or(dynlib::AGT_FAILED);
    if read != dynlib::AGT_OK {
        return MechanismError::Failed {
            code: "mechanism_failed".into(),
            message: format!("{operation} failed without error detail"),
        };
    }
    let code = cstr_to_string(err.code).unwrap_or_else(|| "mechanism_failed".to_string());
    let message = cstr_to_string(err.message).unwrap_or_default();
    MechanismError::Failed { code, message }
}

fn cstr_to_string(ptr: *const std::ffi::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(ptr) };
    cstr.to_str().ok().map(str::to_owned)
}

/// Resolve one exported symbol from the cached library, mapping a load or
/// symbol-resolution failure into a typed [`MechanismError`].
fn call_sym<T>(name: &[u8]) -> Result<libloading::Symbol<'static, T>, MechanismError> {
    let lib = dynlib::load().map_err(|error| MechanismError::Failed {
        code: "dylib_load".into(),
        message: format!(
            "{}; tried: {}",
            error.message,
            error
                .tried
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })?;
    unsafe { lib.sym(name) }.map_err(|message| MechanismError::Failed {
        code: "dylib_symbol".into(),
        message,
    })
}

/// NUL-terminated UTF-8 for node paths passed into FFI.
struct CStringOrStack(CString);

impl CStringOrStack {
    fn new(s: &str) -> Result<Self, MechanismError> {
        CString::new(s)
            .map_err(|_| MechanismError::Failed {
                code: "invalid_input".into(),
                message: "node id contains an interior NUL byte".into(),
            })
            .map(CStringOrStack)
    }

    fn as_ptr(&self) -> *const std::ffi::c_char {
        self.0.as_ptr()
    }
}

// ---------------------------------------------------------------------------
// Export signatures — identical to crates/agenterm-abi's exports.
// ---------------------------------------------------------------------------

type CapabilityQuery = unsafe extern "C" fn(i32) -> i32;
type DesktopHostOpen = unsafe extern "C" fn(
    *const dynlib::agt_desktop_action,
    usize,
    *mut *mut std::ffi::c_void,
) -> i32;
type DesktopHostPoll = unsafe extern "C" fn(*mut std::ffi::c_void, u32, *mut u32) -> i32;
type DesktopHostClose = unsafe extern "C" fn(*mut std::ffi::c_void) -> i32;
type WindowEnumerate = unsafe extern "C" fn(*mut dynlib::agt_window_info, usize, *mut usize) -> i32;
type WindowStackingList =
    unsafe extern "C" fn(*mut dynlib::agt_window_stacking, usize, *mut usize) -> i32;
type WindowPlacementQuery =
    unsafe extern "C" fn(isize, u32, *mut dynlib::agt_window_placement_info_v1) -> i32;
type ScreenList = unsafe extern "C" fn(*mut dynlib::agt_screen_info, usize, *mut usize) -> i32;
type WindowShow = unsafe extern "C" fn(isize, i32) -> i32;
type WindowMove = unsafe extern "C" fn(isize, i32, i32, u32, u32) -> i32;
type WindowRect = unsafe extern "C" fn(isize, *mut i32, *mut i32, *mut u32, *mut u32) -> i32;
type WindowSetTopmost = unsafe extern "C" fn(isize, i32) -> i32;
type WindowClose = unsafe extern "C" fn(isize) -> i32;
type WindowMinimized = unsafe extern "C" fn(isize, *mut i32) -> i32;
type PointerMove = unsafe extern "C" fn(i32, i32) -> i32;
type PointerPosition = unsafe extern "C" fn(*mut i32, *mut i32) -> i32;
type PointerClick = unsafe extern "C" fn(i32, i32, i32, u32) -> i32;
type PointerDrag = unsafe extern "C" fn(i32, i32, i32, i32, i32, u32) -> i32;
type InputTypeText = unsafe extern "C" fn(*const u8, usize) -> i32;
type InputSendKeys = unsafe extern "C" fn(*const u8, usize) -> i32;
type CaptureWindow =
    unsafe extern "C" fn(isize, *const std::ffi::c_char, i32, i32, i32, i32, i32) -> i32;
type DrainBus = unsafe extern "C" fn() -> i32;
type LastTextWriteVia = unsafe extern "C" fn(*mut u8, usize, *mut usize) -> i32;
type TreeSnapshot = unsafe extern "C" fn(isize, *mut usize) -> i32;
type TreeSnapshotBounded = unsafe extern "C" fn(isize, i32, u32, *mut usize) -> i32;
type MenuSnapshot = unsafe extern "C" fn(isize, i32, u32, *mut usize) -> i32;
type MenuInvoke = unsafe extern "C" fn(isize, *const u8, usize, *mut u32, *mut u32) -> i32;
type FocusedSnapshot = unsafe extern "C" fn(isize, *mut usize) -> i32;
type ManualAccessibilityPoke = unsafe extern "C" fn(isize) -> i32;
type ApplicationSetHidden = unsafe extern "C" fn(u32, i32) -> i32;
type AppListInstalled = unsafe extern "C" fn(*mut u8, usize, *mut usize) -> i32;
type AppLaunch = unsafe extern "C" fn(*const u8, usize) -> i32;
type ObserveWindow = unsafe extern "C" fn(isize, u64, usize, *mut usize) -> i32;
type ObserveEventString = unsafe extern "C" fn(usize, i32, *mut u8, usize, *mut usize) -> i32;
type ObserveEventTime = unsafe extern "C" fn(usize, *mut u64) -> i32;
type MetaString = unsafe extern "C" fn(i32, *mut u8, usize, *mut usize) -> i32;
type TreeNode = unsafe extern "C" fn(usize, *mut agt_a11y_node) -> i32;
type NodeString = unsafe extern "C" fn(usize, i32, *mut u8, usize, *mut usize) -> i32;
type NodeActionName = unsafe extern "C" fn(usize, usize, *mut u8, usize, *mut usize) -> i32;
type NodePerform = unsafe extern "C" fn(isize, *const std::ffi::c_char, i32) -> i32;
type NodeInvoke =
    unsafe extern "C" fn(isize, *const std::ffi::c_char, i32, *const u8, usize) -> i32;
type NodeSetText = unsafe extern "C" fn(isize, *const std::ffi::c_char, *const u8, usize) -> i32;
type NodeGetText =
    unsafe extern "C" fn(isize, *const std::ffi::c_char, *mut u8, usize, *mut usize) -> i32;
type NodeSendKeys = unsafe extern "C" fn(isize, *const std::ffi::c_char, *const u8, usize) -> i32;
type NodeScroll = unsafe extern "C" fn(isize, *const std::ffi::c_char) -> i32;
type NodeGetExtents = unsafe extern "C" fn(
    isize,
    *const std::ffi::c_char,
    *mut i32,
    *mut i32,
    *mut i32,
    *mut i32,
) -> i32;
type NodeSetSelection = unsafe extern "C" fn(isize, *const std::ffi::c_char, i32, i32) -> i32;
type NodeGetSelection =
    unsafe extern "C" fn(isize, *const std::ffi::c_char, *mut i32, *mut i32, *mut i32) -> i32;
type NodeSetCaretOffset = unsafe extern "C" fn(isize, *const std::ffi::c_char, i32) -> i32;
type NodeGetCaretOffset = unsafe extern "C" fn(isize, *const std::ffi::c_char, *mut i32) -> i32;
type ClipboardSetText = unsafe extern "C" fn(*const u8, usize) -> i32;
type ClipboardTypes = unsafe extern "C" fn(*mut u8, usize, *mut usize) -> i32;
type ClipboardGetText = unsafe extern "C" fn(*mut u8, usize, *mut usize) -> i32;
type ClipboardGet = unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize) -> i32;
type ClipboardSet = unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> i32;
type ClipboardSetFile = unsafe extern "C" fn(*const u8, usize) -> i32;
type ClipboardClear = unsafe extern "C" fn() -> i32;
type LastError = unsafe extern "C" fn(*mut agt_error) -> i32;

#[cfg(test)]
mod tests {
    use super::*;

    fn placement_record() -> dynlib::agt_window_placement_info_v1 {
        dynlib::agt_window_placement_info_v1 {
            struct_size: std::mem::size_of::<dynlib::agt_window_placement_info_v1>() as u32,
            record_version: dynlib::AGT_WINDOW_PLACEMENT_RECORD_V1,
            handle: 7,
            process_id: 42,
            role: dynlib::AGT_WINDOW_ROLE_STANDARD,
            movable: dynlib::AGT_WINDOW_SUPPORT_YES,
            resizable: dynlib::AGT_WINDOW_SUPPORT_YES,
            constraints_kind: dynlib::AGT_WINDOW_CONSTRAINTS_EXPLICIT,
            ..Default::default()
        }
    }

    #[test]
    fn placement_record_parses_all_typed_fields() {
        use window_placement::{PlacementRole, SizeConstraints, Support, WindowSize};
        let record = dynlib::agt_window_placement_info_v1 {
            role: dynlib::AGT_WINDOW_ROLE_DIALOG,
            movable: dynlib::AGT_WINDOW_SUPPORT_NO,
            resizable: dynlib::AGT_WINDOW_SUPPORT_UNKNOWN,
            constraint_flags: dynlib::AGT_WINDOW_CONSTRAINT_HAS_MIN
                | dynlib::AGT_WINDOW_CONSTRAINT_HAS_MAX
                | dynlib::AGT_WINDOW_CONSTRAINT_HAS_INCREMENT,
            min_width: 320,
            min_height: 200,
            max_width: 1600,
            max_height: 1200,
            increment_width: 8,
            increment_height: 16,
            ..placement_record()
        };
        let parsed = window_placement::parse_record(record, 7, 42).expect("valid record");
        assert_eq!(parsed.role, PlacementRole::Dialog);
        assert_eq!(parsed.movable, Support::No);
        assert_eq!(parsed.resizable, Support::Unknown);
        assert_eq!(
            parsed.constraints,
            SizeConstraints::Explicit {
                min: Some(WindowSize {
                    width: 320,
                    height: 200
                }),
                max: Some(WindowSize {
                    width: 1600,
                    height: 1200
                }),
                increment: Some(WindowSize {
                    width: 8,
                    height: 16
                }),
            }
        );
    }

    #[test]
    fn placement_record_rejects_invalid_flags_zero_reversed_and_enums() {
        let cases = [
            dynlib::agt_window_placement_info_v1 {
                constraint_flags: 1 << 31,
                ..placement_record()
            },
            dynlib::agt_window_placement_info_v1 {
                constraint_flags: dynlib::AGT_WINDOW_CONSTRAINT_HAS_MIN,
                min_width: 0,
                min_height: 200,
                ..placement_record()
            },
            dynlib::agt_window_placement_info_v1 {
                constraint_flags: dynlib::AGT_WINDOW_CONSTRAINT_HAS_MIN
                    | dynlib::AGT_WINDOW_CONSTRAINT_HAS_MAX,
                min_width: 900,
                min_height: 700,
                max_width: 800,
                max_height: 600,
                ..placement_record()
            },
            dynlib::agt_window_placement_info_v1 {
                role: 99,
                ..placement_record()
            },
            dynlib::agt_window_placement_info_v1 {
                movable: 99,
                ..placement_record()
            },
        ];
        for record in cases {
            let error = window_placement::parse_record(record, 7, 42).unwrap_err();
            assert!(matches!(
                error,
                MechanismError::Failed { ref code, .. } if code == "window_metadata_invalid"
            ));
        }
    }

    #[test]
    fn placement_record_refuses_mismatched_identity_and_nonexplicit_dimensions() {
        assert!(window_placement::parse_record(placement_record(), 8, 42).is_err());
        let short = dynlib::agt_window_placement_info_v1 {
            struct_size: 8,
            ..placement_record()
        };
        assert!(window_placement::parse_record(short, 7, 42).is_err());
        let record = dynlib::agt_window_placement_info_v1 {
            constraints_kind: dynlib::AGT_WINDOW_CONSTRAINTS_UNKNOWN,
            constraint_flags: dynlib::AGT_WINDOW_CONSTRAINT_HAS_MIN,
            min_width: 10,
            min_height: 10,
            ..placement_record()
        };
        assert!(window_placement::parse_record(record, 7, 42).is_err());
    }

    #[test]
    fn placement_old_minor_is_typed_unsupported() {
        let error = window_placement::require_placement_abi((1 << 16) | 9).unwrap_err();
        assert!(matches!(error, MechanismError::Unsupported { .. }));
        assert!(window_placement::require_placement_abi((1 << 16) | 10).is_ok());
    }

    #[test]
    fn pointer_position_old_minor_is_typed_unsupported() {
        let error = input_inject::require_pointer_position_abi((1 << 16) | 10).unwrap_err();
        assert!(matches!(error, MechanismError::Unsupported { .. }));
        assert!(input_inject::require_pointer_position_abi((1 << 16) | 11).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn windows_real_placement_query_rejects_stale_pid_when_available() {
        let Ok(windows) = window_enumerate::enumerate_top_level() else {
            eprintln!("SKIP: native window enumeration unavailable");
            return;
        };
        let Some(window) = windows.first() else {
            eprintln!("SKIP: no visible top-level window");
            return;
        };
        let stale_pid = if window.process_id == u32::MAX {
            window.process_id - 1
        } else {
            window.process_id + 1
        };
        match window_placement::inspect(window.handle, stale_pid) {
            Err(MechanismError::Failed { code, .. }) => assert_eq!(code, "window_stale"),
            Err(MechanismError::Unsupported { reason }) => {
                eprintln!("SKIP: {reason}");
            }
            Ok(info) => panic!("stale pid unexpectedly inspected: {info:?}"),
        }
    }

    /// An empty payload reads as empty, and the follow-up read never
    /// repeats the cap==0 size probe -- that would just answer
    /// buffer_too_small forever.
    #[test]
    fn two_stage_empty_payload_reads_empty_without_reprobing_with_cap_zero() {
        let mut calls = 0usize;
        let bytes = read_two_stage(|_buf, cap, out_len| {
            calls += 1;
            unsafe {
                *out_len = 0;
            }
            if calls == 1 {
                assert_eq!(cap, 0, "the size probe is the cap==0 call");
                return Ok::<i32, MechanismError>(dynlib::AGT_FAILED);
            }
            assert!(cap > 0, "the read must not repeat the cap==0 probe");
            Ok::<i32, MechanismError>(dynlib::AGT_OK)
        })
        .expect("empty two-stage probe should succeed");
        assert!(bytes.is_empty());
        assert_eq!(calls, 2);
    }

    /// A call that fails outright looks exactly like an empty payload at the
    /// probe -- AGT_FAILED with out_len untouched. It must not be reported
    /// as an empty result: that is how `clipboard-read` came to claim a host
    /// with no clipboard helper was carrying zero types.
    #[test]
    fn two_stage_failure_is_not_read_as_an_empty_payload() {
        let mut calls = 0usize;
        let result = read_two_stage(|_buf, _cap, _out_len| {
            calls += 1;
            Ok::<i32, MechanismError>(dynlib::AGT_FAILED)
        });
        assert!(result.is_err(), "a failing probe must not answer Ok(empty)");
        assert_eq!(calls, 2);
    }

    /// AGT_UNSUPPORTED records no error by convention, so reading the error
    /// slot for it returns whatever was there last.
    #[test]
    fn two_stage_unsupported_is_typed_without_reading_the_error_slot() {
        let result = read_two_stage(|_buf, _cap, _out_len| {
            Ok::<i32, MechanismError>(dynlib::AGT_UNSUPPORTED)
        });
        assert!(matches!(result, Err(MechanismError::Unsupported { .. })));
    }

    #[test]
    fn two_stage_record_fetch_retries_only_for_observed_growth() {
        assert_eq!(
            window_enumerate::retry_capacity(dynlib::AGT_FAILED, 8, 11),
            Some(11)
        );
        assert_eq!(
            window_enumerate::retry_capacity(dynlib::AGT_FAILED, 8, 8),
            None
        );
        assert_eq!(
            window_enumerate::retry_capacity(dynlib::AGT_OK, 8, 11),
            None
        );
    }
}
