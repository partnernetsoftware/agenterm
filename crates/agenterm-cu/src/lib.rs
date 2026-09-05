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
pub mod file_move_transactions;
pub mod file_transactions;
pub mod grant_management;
pub mod host_actions;
pub mod hotkeys;
pub mod idempotency_store;
#[cfg(target_os = "macos")]
pub mod macos_focus;
pub mod macos_spaces;
pub(crate) mod managed_job_ipc;
pub(crate) mod managed_job_owner;
pub(crate) mod managed_job_store;

#[doc(hidden)]
pub const MANAGED_JOB_OWNER_ARG: &str = "--agenterm-cu-internal-managed-job-owner";

#[doc(hidden)]
pub fn run_managed_job_owner() -> i32 {
    match managed_job_ipc::run_resident(std::io::stdin().lock()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}
pub mod mcu_surface;
pub mod mechanism;
pub mod network_probe;
pub mod observe;
pub mod page_text;
pub mod place;
pub mod privilege_apply;
pub mod privilege_plan;
pub mod pty_snapshot;
pub mod rdp_transport;
pub mod receipt;
pub mod reply;
pub mod runtime_coordinator;
pub mod setup_entrypoint;
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
pub use command::{
    Command, FileTransactionAction, OrderRelation, PermissionAction, PermissionKind, PointerButton,
    SetupAction, TerminalWaitCondition, WaitCondition,
};
pub use executor::{Executor, RequestIdentity};
pub use rdp_transport::RdpEndpoint;
pub use reply::{CuError, CuReply};
pub use ssh_transport::SshEndpoint;
pub use target::TargetRef;
pub use vnc_transport::VncEndpoint;
