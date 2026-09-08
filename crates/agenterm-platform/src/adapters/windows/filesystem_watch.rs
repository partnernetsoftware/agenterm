//! Windows has no inotify equivalent in this facade yet.

use std::path::Path;

use crate::filesystem_watch::{
    FilesystemWatchError, FilesystemWatchErrorKind, FilesystemWatchResult,
};

pub fn watch_directory(
    _path: &Path,
    _duration_ms: u64,
    _max_events: usize,
) -> Result<FilesystemWatchResult, FilesystemWatchError> {
    Err(FilesystemWatchError {
        kind: FilesystemWatchErrorKind::Unsupported,
        message: "filesystem watch is not mapped on Windows yet".into(),
    })
}
