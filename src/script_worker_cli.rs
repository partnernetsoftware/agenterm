//! The engine-neutral script worker entry, reached through
//! `__agenterm-internal-engine worker`.
//!
//! These four modes lived inside `script_rh_cli_main::run_main` until
//! 2026-08-29, behind the token `rh` -- which was only ever the fixed
//! entry-point spelling, never a commitment to the rh engine: the worker picks
//! its real engine per invocation from `ScriptBackend::resolve`, so lua, sql
//! and qjswasm tasks all ran *through* the rh-named route. When rh left the
//! repository the route had to be re-pointed rather than deleted, or every
//! hosted script invocation for every engine would have gone with it. It now
//! has a name that says what it is.
//!
//! `worker_supervisor::SCRIPT_WORKER_ENGINE_ARGS` is the argv prefix the
//! hosted-worker spawn paths use to reach here; `src/bin/agenterm.rs`'s
//! `dispatch_engine` is the arm that lands.

use std::process::ExitCode;

/// `arguments` is this entry's own argv with the program name and the engine
/// token already stripped. The modes are argv-shape-sensitive on purpose
/// (exact single-argument matches): a worker is spawned by this product, not
/// typed by a person, and a near-miss is a bug to surface rather than a
/// spelling to forgive.
pub fn run_main(arguments: Vec<String>) -> ExitCode {
    match arguments.as_slice() {
        [mode, rest @ ..] if mode == "--internal-incremental-finalize" => worker_exit_code(
            crate::incremental_wrapper::finalize_incremental_manifest(rest),
        ),
        [mode] if mode == "--worker" => worker_exit_code(crate::run_legacy_worker_stdio()),
        [mode] if mode == "--framed-worker" => worker_exit_code(crate::run_framed_worker_stdio()),
        [command, ..] if command == "task" => {
            script_exit_code(crate::run_script_entry_with_args(arguments))
        }
        _ => {
            eprintln!(
                "agenterm script worker: expected --worker, --framed-worker, \
                 --internal-incremental-finalize, or task ...; got {arguments:?}"
            );
            ExitCode::from(2)
        }
    }
}

fn worker_exit_code(result: anyhow::Result<u8>) -> ExitCode {
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::from(2)
        }
    }
}

fn script_exit_code(code: i32) -> ExitCode {
    u8::try_from(code)
        .map(ExitCode::from)
        .unwrap_or(ExitCode::FAILURE)
}
