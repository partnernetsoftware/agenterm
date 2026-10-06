//! Immutable version directories plus an atomic `current.json` pointer.
//!
//! `<root>/<version>/` is written once (staged dir → rename) and never
//! modified, so a running MiniCon is never overwritten (Windows locks running
//! executables). Switching or rolling back only rewrites the pointer.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pointer {
    pub current: String,
    pub previous: Option<String>,
}

const POINTER: &str = "current.json";

pub fn read_pointer(root: &Path) -> io::Result<Option<Pointer>> {
    match fs::read(root.join(POINTER)) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn write_pointer(root: &Path, p: &Pointer) -> io::Result<()> {
    let tmp = root.join(".current.json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(p)?)?;
    fs::rename(&tmp, root.join(POINTER))
}

/// Move a verified staging directory into place as `<root>/<version>/`.
/// An existing version directory is left untouched (immutable).
pub fn commit_version(root: &Path, staged: &Path, version: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(root)?;
    let dest = root.join(version);
    if !dest.exists() {
        fs::rename(staged, &dest)?;
    }
    Ok(dest)
}

/// Point `current` at `version`, remembering the old one for rollback.
pub fn activate(root: &Path, version: &str) -> io::Result<Pointer> {
    if !root.join(version).is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("version {version} not installed")));
    }
    let old = read_pointer(root)?;
    let p = Pointer {
        current: version.to_owned(),
        previous: old.map(|o| o.current).filter(|c| c != version),
    };
    write_pointer(root, &p)?;
    Ok(p)
}

/// Swap back to `previous`. Errors when there is nothing to roll back to.
pub fn rollback(root: &Path) -> io::Result<Pointer> {
    let p = read_pointer(root)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "nothing installed"))?;
    let prev = p
        .previous
        .clone()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no previous version"))?;
    activate(root, &prev)
}
