//! Interpret `minicon --version --json` and `--hostif-handshake` output.
//!
//! Legacy rule: non-zero exit, or stdout that is not the expected JSON, means
//! a MiniCon older than HOSTIF (`hostif = 0`). A MiniCon <= 0.2.3 may print
//! plain-text `--version` and exit 0; that is Legacy, not an error.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub hostif: String,
    pub os: String,
    pub arch: String,
    pub asset: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Handshake {
    pub hostif: String,
    pub version: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Answered the HOSTIF probe.
    Hostif(VersionInfo),
    /// Pre-HOSTIF MiniCon (or not a MiniCon at all); needs upgrade.
    Legacy,
}

pub fn parse_version_json(exit_code: i32, stdout: &str) -> Probe {
    if exit_code != 0 {
        return Probe::Legacy;
    }
    match serde_json::from_str::<VersionInfo>(stdout.trim()) {
        Ok(info) => Probe::Hostif(info),
        Err(_) => Probe::Legacy,
    }
}

pub fn parse_handshake(exit_code: i32, stdout: &str) -> Option<Handshake> {
    if exit_code != 0 {
        return None;
    }
    serde_json::from_str(stdout.trim()).ok()
}
