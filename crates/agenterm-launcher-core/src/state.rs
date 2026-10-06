//! Launcher state machine input → state. The UI renders this; it never
//! decides on its own.

use crate::probe::{Handshake, Probe};
use crate::version::Version;
use crate::{HOSTIF_MAJOR, REQUIRED_CAPABILITIES};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherState {
    /// No MiniCon found. Download requires user confirmation.
    Missing,
    /// Pre-HOSTIF MiniCon. Upgrade requires user confirmation.
    Legacy,
    /// HOSTIF major or capabilities do not fit. Upgrade requires confirmation.
    Incompatible { reason: String },
    /// Usable; a newer release exists (non-blocking banner).
    UpdateAvailable { local: String, latest: String },
    /// Usable and current. Also used when the remote is older (no downgrade).
    Current { local: String, remote_older: bool },
    /// Usable local copy, latest unknown (network bounded wait expired).
    Offline { local: String },
    /// Nothing usable and the network is unavailable.
    OfflineMissing,
}

pub struct Inputs<'a> {
    pub probe: Option<&'a Probe>,
    pub handshake: Option<&'a Handshake>,
    /// `None` = latest manifest could not be fetched within the bound.
    pub latest_version: Option<&'a str>,
}

pub fn decide(i: &Inputs) -> LauncherState {
    let info = match i.probe {
        None => {
            return if i.latest_version.is_some() {
                LauncherState::Missing
            } else {
                LauncherState::OfflineMissing
            };
        }
        Some(Probe::Legacy) => return LauncherState::Legacy,
        Some(Probe::Hostif(info)) => info,
    };
    match Version::parse(&info.hostif) {
        Some(v) if v.major() == HOSTIF_MAJOR => {}
        _ => {
            return LauncherState::Incompatible { reason: format!("hostif {}", info.hostif) };
        }
    }
    if let Some(h) = i.handshake {
        let missing: Vec<&str> = REQUIRED_CAPABILITIES
            .iter()
            .copied()
            .filter(|c| !h.capabilities.iter().any(|x| x == c))
            .collect();
        if !missing.is_empty() {
            return LauncherState::Incompatible { reason: format!("missing {}", missing.join(",")) };
        }
    }
    let local = info.version.clone();
    let Some(latest) = i.latest_version else {
        return LauncherState::Offline { local };
    };
    match (Version::parse(&local), Version::parse(latest)) {
        (Some(l), Some(r)) if r > l => {
            LauncherState::UpdateAvailable { local, latest: latest.to_owned() }
        }
        (Some(l), Some(r)) => LauncherState::Current { local, remote_older: r < l },
        _ => LauncherState::Current { local, remote_older: false },
    }
}

impl LauncherState {
    /// Whether this state may lead to a download only after user consent.
    pub fn needs_consent_to_download(&self) -> bool {
        matches!(
            self,
            LauncherState::Missing
                | LauncherState::Legacy
                | LauncherState::Incompatible { .. }
                | LauncherState::UpdateAvailable { .. }
        )
    }
}
