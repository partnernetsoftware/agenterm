//! `agenterm-cu` — computer-use foundation (PRD_02_28/29/30/31).
//!
//! Orchestrator agents should drive desktops through structured observation and
//! actuation, not screenshot/OCR coordinate guessing. See `README.md`.

pub mod audio_control;
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
pub(crate) mod device_lease_ipc;
pub(crate) mod device_lease_owner;
pub(crate) mod device_lease_store;
pub mod dynlib;
pub mod executor;
pub mod file_move_transactions;
pub mod file_transactions;
pub mod grant_management;
pub mod host_actions;
pub mod hotkeys;
pub mod idempotency_store;
pub mod login_session;
#[cfg(target_os = "macos")]
pub mod macos_focus;
pub mod macos_spaces;
pub(crate) mod managed_job_ipc;
pub(crate) mod managed_job_owner;
pub(crate) mod managed_job_store;

#[doc(hidden)]
pub const MANAGED_JOB_OWNER_ARG: &str = "--agenterm-cu-internal-managed-job-owner";
#[doc(hidden)]
pub const DEVICE_LEASE_OWNER_ARG: &str = "--agenterm-cu-internal-device-lease-owner";
pub const DEVICE_IO_FIXTURE_ARG: &str = "--agenterm-cu-internal-device-io-fixture";

#[doc(hidden)]
pub fn run_managed_job_owner() -> i32 {
    match managed_job_ipc::run_resident(std::io::stdin().lock()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

#[doc(hidden)]
pub fn run_device_lease_owner() -> i32 {
    match device_lease_ipc::run_resident(std::io::stdin().lock()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Runs the invocation-owned Unix PTY echo fixture used by the public qjswasm
/// device-lease court. The public receipt contains only its opaque token.
#[doc(hidden)]
pub fn run_device_io_test_fixture(args: &[String]) -> i32 {
    #[cfg(unix)]
    {
        use std::io::Write as _;

        if args.len() != 2 {
            return 2;
        }
        let lifetime_ms = match args[1].parse::<u64>() {
            Ok(value) if (1_000..=300_000).contains(&value) => value,
            _ => return 2,
        };
        let fixture = match agenterm_platform::device_io::create_test_fixture(
            std::path::Path::new(&args[0]),
            std::time::Duration::from_millis(lifetime_ms),
        ) {
            Ok(fixture) => fixture,
            Err(error) => {
                eprintln!("{}", error.code());
                return 3;
            }
        };
        let mut stdout = std::io::stdout().lock();
        if writeln!(
            stdout,
            "{{\"schema_version\":1,\"token\":\"{}\"}}",
            fixture.token()
        )
        .and_then(|()| stdout.flush())
        .is_err()
        {
            return 4;
        }
        match fixture.run() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{}", error.code());
                5
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        2
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
    Command, DeviceInventorySelector, FileTransactionAction, OrderRelation, PermissionAction,
    PermissionKind, PointerButton, SetupAction, TerminalWaitCondition, WaitCondition,
};
pub use executor::{Executor, RequestIdentity};
pub use rdp_transport::RdpEndpoint;
pub use reply::{CuError, CuReply};
pub use ssh_transport::SshEndpoint;
pub use target::TargetRef;
pub use vnc_transport::VncEndpoint;
