//! Product-facing process facade compatibility surface.
//!
//! Native process mechanics come from the external `agenterm-platform` crate.
//! AgenTerm-specific sibling-server discovery remains product policy here.

use std::process::{Command, Stdio};

#[allow(unused_imports)] // Compatibility exports consumed by product modules per target.
pub(crate) use agenterm_platform::contract::process::{
    ProcessError, ProcessErrorKind, ProcessInfo, ProcessObservation,
};
#[allow(unused_imports)] // Compatibility exports consumed by product modules per target.
pub(crate) use agenterm_platform::process::{
    ProcessTreeGuard, configure_breakaway_visible_command, configure_command,
    configure_owned_command, is_breakaway_denied, kill, list, observe,
    spawn_breakaway_visible_child, spawn_breakaway_visible_command, spawn_detached_child,
    spawn_detached_command, start_identity,
};

pub(crate) fn autostart_server(
    parameter_name: &str,
    parameter_value: &str,
    chassis_image: Option<&std::path::Path>,
) -> std::io::Result<bool> {
    autostart_server_impl(parameter_name, parameter_value, chassis_image)
}

/// Where an autostarted server's stderr goes.
///
/// Production discards it: the autostarted server is detached and outlives the
/// caller, so it has no console to write to. That is correct for a product and
/// wrong for a failing gate -- when the server dies during startup, the client
/// sees only `ConnectTimeout` and the reason is gone. `cli-smoke` spent a whole
/// 45-minute Candidate round reporting exactly that and nothing else.
///
/// `AGENTERM_AUTOSTART_STDERR_PATH` opts into keeping it. Unset -- every
/// production path -- behaves exactly as before.
fn autostart_stderr() -> std::io::Result<Stdio> {
    let Some(path) = std::env::var_os("AGENTERM_AUTOSTART_STDERR_PATH") else {
        return Ok(Stdio::null());
    };
    if path.is_empty() {
        return Ok(Stdio::null());
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    Ok(Stdio::from(file))
}

fn autostart_server_impl(
    parameter_name: &str,
    parameter_value: &str,
    chassis_image: Option<&std::path::Path>,
) -> std::io::Result<bool> {
    if !matches!(
        agenterm_platform::platform_kind(),
        agenterm_platform::PlatformKind::Windows
    ) {
        return Ok(false);
    }
    // Authority is `agenterm server` in a separate process of the same PE.
    // Replacing `agenterm.exe` on disk while that process lives may fail on
    // Windows until the authority is stopped (accepted product trade-off).
    let current = std::env::current_exe()?;
    let mut command = Command::new(current);
    command
        .arg("server")
        .arg(parameter_name)
        .arg(parameter_value);
    // The image travels on the authority's command line only, never in an
    // inherited environment variable that its PTY children would see.
    if let Some(image) = chassis_image {
        command.arg("--chassis-image").arg(image);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(autostart_stderr()?);
    // P0-3: detached spawn prefers Job breakaway so Keep-Server survives GUI
    // process-tree teardown. CallerJobFallback is an honest host limit.
    let spawn_mode = agenterm_platform::process::spawn_detached_command(&mut command)?;
    if spawn_mode == agenterm_platform::process::DetachedSpawnMode::CallerJobFallback {
        let _ = agenterm_platform::process::write_parent_console_stderr(
            "AgenTerm server started inside the caller's process job because Windows denied job breakaway; it may stop when that owning job closes.",
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// An autostarted server that dies during startup must be able to say why.
    /// Production keeps discarding its stderr -- it is detached and has no
    /// console -- but a gate can opt into keeping it, and without that opt-in a
    /// failed autostart reaches the reader as `ConnectTimeout` and nothing
    /// else. That cost a full Candidate round on 2026-09-20.
    #[test]
    fn autostart_stderr_is_discarded_until_a_path_opts_in() {
        let src = include_str!("process.rs");
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("production half of process.rs");
        assert!(
            production.contains(".stderr(autostart_stderr()?)"),
            "autostart must route stderr through the opt-in helper"
        );
        // Unset and empty both mean production behaviour. An empty value is
        // what a shell expansion of an undefined variable produces, and
        // treating it as a path would create a file named "" or fail the
        // spawn outright.
        let helper = production
            .split_once("fn autostart_stderr()")
            .expect("the helper must exist")
            .1;
        let body = helper
            .split_once("\nfn ")
            .expect("the helper must be followed by another item")
            .0;
        assert!(
            body.contains("AGENTERM_AUTOSTART_STDERR_PATH"),
            "the opt-in must be spelled by that environment variable"
        );
        assert_eq!(
            body.matches("Stdio::null()").count(),
            2,
            "both the unset and the empty case must fall back to null"
        );
        assert!(
            body.contains(".append(true)"),
            "a retried autostart must not truncate the previous attempt's reason"
        );
    }

    /// Structural proof that GUI autostart uses the platform detached path
    /// (breakaway + ACCESS_DENIED fallback), not a raw Command::spawn.
    #[test]
    fn autostart_server_source_uses_spawn_detached_command() {
        let src = include_str!("process.rs");
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("production half of process.rs");
        assert!(
            production.contains("spawn_detached_command"),
            "autostart_server must use platform detached/breakaway spawn"
        );
        assert!(
            !production.contains(".spawn()"),
            "autostart_server must not raw-spawn without platform flags"
        );
    }

    #[test]
    fn current_process_has_stable_observation_and_inventory_entry() {
        assert!(matches!(
            observe(std::process::id()),
            ProcessObservation::Live { .. }
        ));
        assert!(
            list()
                .expect("process inventory")
                .iter()
                .any(|entry| entry.id == std::process::id())
        );
    }

    /// G1 regression: product code must classify breakaway denial via
    /// `agenterm_platform::process::is_breakaway_denied` / spawn facades, not
    /// hard-coded `raw_os_error() == Some(5)`.
    #[test]
    fn product_sources_do_not_hardcode_breakaway_access_denied() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        let mut offenders = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(source) = fs::read_to_string(&path) else {
                    continue;
                };
                // Ignore this test module's own needle.
                if path.ends_with("platform\\process.rs") || path.ends_with("platform/process.rs") {
                    continue;
                }
                for (index, line) in source.lines().enumerate() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("//") {
                        continue;
                    }
                    if trimmed.contains("raw_os_error()")
                        && (trimmed.contains("Some(5)") || trimmed.contains("== 5"))
                    {
                        offenders.push(format!("{}:{}", path.display(), index + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "use agenterm_platform::process::is_breakaway_denied / \
             spawn_breakaway_visible_* instead of hard-coding ACCESS_DENIED:\n{}",
            offenders.join("\n")
        );
    }
}
