//! `agenterm-cu` — computer-use foundation (PRD_02_28/29/30/31).
//!
//! Orchestrator agents should drive desktops through structured observation and
//! actuation, not screenshot/OCR coordinate guessing. See `README.md`.

pub mod audit;
pub mod auth;
pub mod auth_store;
#[cfg(target_os = "macos")]
pub mod ax_guide;
pub mod browser_bridge;
pub mod browser_profiles;
pub mod browser_session;
pub mod browser_session_owner;
pub mod cdp;
pub mod command;
pub mod dynlib;
pub mod executor;
pub mod grant_management;
pub mod host_actions;
pub mod hotkeys;
pub mod idempotency_store;
#[cfg(target_os = "macos")]
pub mod macos_focus;
pub mod macos_spaces;
#[allow(
    dead_code,
    reason = "internal managed-job cohort under staged integration"
)]
pub(crate) mod managed_job_owner;
#[allow(
    dead_code,
    reason = "internal managed-job cohort under staged integration"
)]
pub(crate) mod managed_job_store;
pub mod mcu_surface;
pub mod mechanism;
pub mod network_probe;
pub mod observe;
pub mod page_text;
pub mod place;
pub mod pty_snapshot;
pub mod rdp_transport;
pub mod receipt;
pub mod reply;
pub mod runtime_coordinator;
pub mod snapshot;
pub mod ssh_transport;
#[cfg(target_os = "macos")]
pub mod status_menu;
pub mod tab_strip;
pub mod target;
pub mod target_binding;
pub mod vnc_transport;
#[doc(hidden)]
pub mod worker_wire;

pub use auth::{Authorization, Grant};
pub use command::{Command, OrderRelation, PointerButton, TerminalWaitCondition, WaitCondition};
pub use executor::{Executor, RequestIdentity};
pub use rdp_transport::RdpEndpoint;
pub use reply::{CuError, CuReply};
pub use ssh_transport::SshEndpoint;
pub use target::TargetRef;
pub use vnc_transport::VncEndpoint;
