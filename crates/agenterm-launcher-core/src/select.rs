//! Pick the release asset for this host: native first, `minicon.com` (APE)
//! only when no native asset exists for the host's `asset` key.
//!
//! macOS prefers the stapled `.dmg` (offline-verifiable `MiniCon.app`); the
//! `.tar.gz` holds a bare Mach-O that cannot carry a stapled ticket.

use crate::manifest::{Asset, Manifest};

pub const APE_NAME: &str = "minicon.com";

/// Extensions in preference order for a given asset key.
fn preferred_exts(asset_key: &str) -> &'static [&'static str] {
    if asset_key.starts_with("windows-") {
        &["zip"]
    } else if asset_key.starts_with("macos-") {
        &["dmg", "tar.gz"]
    } else {
        &["tar.gz"]
    }
}

/// Map Rust `std::env::consts` os/arch to MiniCon's asset key. Only used when
/// no MiniCon is installed yet (an installed one reports `asset` itself).
pub fn host_asset_key(os: &str, arch: &str) -> Option<String> {
    let arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        _ => return None,
    };
    match os {
        "macos" => Some("macos-universal".into()),
        "windows" | "linux" => Some(format!("{os}-{arch}")),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick<'a> {
    Native(&'a Asset),
    ApeFallback(&'a Asset),
    None,
}

pub fn pick<'a>(m: &'a Manifest, asset_key: Option<&str>) -> Pick<'a> {
    if let Some(key) = asset_key {
        for ext in preferred_exts(key) {
            let want = format!("minicon-{}-{}.{}", m.version, key, ext);
            if let Some(a) = m.assets.iter().find(|a| a.name == want) {
                return Pick::Native(a);
            }
        }
    }
    match m.assets.iter().find(|a| a.name == APE_NAME) {
        Some(a) => Pick::ApeFallback(a),
        None => Pick::None,
    }
}
