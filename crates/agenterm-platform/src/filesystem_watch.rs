//! Bounded native filesystem change observation for one directory path.

use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemWatchEvent {
    pub t_ms: u64,
    pub kind: String,
    pub name: String,
    pub mask: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemWatchResult {
    pub provider: String,
    pub mode: String,
    pub path: String,
    pub duration_ms: u64,
    pub max_events: usize,
    pub events: Vec<FilesystemWatchEvent>,
    pub emitted: usize,
    pub completed: bool,
    pub truncated: bool,
    /// The caller's borrowed stop probe became pending between native
    /// observation rounds. This is a mechanism fact, not product policy.
    pub cancelled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemWatchError {
    pub kind: FilesystemWatchErrorKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesystemWatchErrorKind {
    Unsupported,
    InvalidInput,
    NotDirectory,
    Native,
}

impl std::fmt::Display for FilesystemWatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FilesystemWatchError {}

/// Watch one existing directory for bounded create/modify/delete events.
pub fn watch_directory(
    path: &Path,
    duration_ms: u64,
    max_events: usize,
) -> Result<FilesystemWatchResult, FilesystemWatchError> {
    crate::selected::filesystem_watch::watch_directory(path, duration_ms, max_events)
}

/// Watch one directory while sampling a borrowed, product-neutral stop probe
/// between bounded native observation rounds.
pub fn watch_directory_controlled(
    path: &Path,
    duration_ms: u64,
    max_events: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<FilesystemWatchResult, FilesystemWatchError> {
    crate::selected::filesystem_watch::watch_directory_controlled(
        path,
        duration_ms,
        max_events,
        cancelled,
    )
}
