//! Stable current-user CLI entrypoint setup.

use std::path::PathBuf;

use crate::{
    command::SetupAction,
    reply::CuError,
    setup_entrypoint::{self, SetupMode},
};

pub(super) fn setup_payload(
    action: SetupAction,
    bin_dir: Option<&str>,
) -> Result<serde_json::Value, CuError> {
    let source = std::env::current_exe().map_err(|error| {
        CuError::new(
            "setup_entrypoint_source_invalid",
            format!("resolve current agenterm-cu executable failed: {error}"),
        )
    })?;
    let bin_dir = match bin_dir {
        Some(path) => PathBuf::from(path),
        None => setup_entrypoint::default_bin_dir()?,
    };
    let mode = match action {
        SetupAction::Check => SetupMode::Check,
        SetupAction::Apply => SetupMode::Apply,
    };
    setup_entrypoint::run(&source, &bin_dir, mode)
}
