//! Mailbox address forms for this library.
//!
//! | Form | Meaning |
//! |------|---------|
//! | `shmbox:file:/path` | File-backed mmap slot (production path) |
//! | `shmbox:file:rel.slot` | Same, relative path |
//! | `shmbox:/abs/path` | Shorthand for `shmbox:file:/abs/path` |
//! | `shmbox:shm:name` | Named shared memory (`shm_open` / Windows named mapping) |
//!
//! Not accepted: `ipc://`, `tcp://`, bare `shm://`. Darwin production work
//! should use `file`; `shm` is available but reopen can fail on that host.

use crate::error::Error;
use crate::slot::SlotLoc;
use std::path::PathBuf;

/// Parsed rendezvous for one mailbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Address {
    File(PathBuf),
    Shm(String),
}

impl Address {
    pub fn parse(raw: &str) -> Result<Self, Error> {
        if raw.is_empty() {
            return Err(bad_address());
        }
        if let Some(rest) = raw.strip_prefix("shmbox:") {
            return parse_body(rest);
        }
        Err(bad_address())
    }

    pub fn display(&self) -> String {
        match self {
            Self::File(p) => format!("shmbox:file:{}", p.display()),
            Self::Shm(n) => format!("shmbox:shm:{n}"),
        }
    }

    pub fn to_loc(&self) -> SlotLoc {
        match self {
            Self::File(p) => SlotLoc::File(p.clone()),
            Self::Shm(n) => SlotLoc::Shm(n.clone()),
        }
    }
}

fn parse_body(rest: &str) -> Result<Address, Error> {
    if rest.is_empty() {
        return Err(bad_address());
    }
    if let Some(path) = rest.strip_prefix("file:") {
        if path.is_empty() {
            return Err(bad_address());
        }
        return Ok(Address::File(PathBuf::from(path)));
    }
    if let Some(name) = rest.strip_prefix("shm:") {
        if name.is_empty() || name.contains('/') {
            return Err(bad_address());
        }
        return Ok(Address::Shm(name.to_string()));
    }
    // `shmbox:/abs/...` → file
    if rest.starts_with('/') {
        return Ok(Address::File(PathBuf::from(rest)));
    }
    Err(bad_address())
}

fn bad_address() -> Error {
    Error::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "bad address",
    ))
}
