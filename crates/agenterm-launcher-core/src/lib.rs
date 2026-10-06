//! AgenTerm 0.2.0.0 launcher core (placeholder, pre-"开工").
//!
//! Pure decision logic for `agenterm.com`: parse what a local MiniCon says
//! about itself, parse the release manifest, decide the launcher state, pick
//! the asset for this host, verify downloaded bytes and switch the installed
//! version through an atomic pointer. Network fetch, process spawn, platform
//! signer checks and the wry+tao UI live outside this crate and feed it data.
//!
//! Contract: MiniCon `plan/plan-hostif-v1.md`, implemented in MiniCon fb40827.

pub mod install;
pub mod manifest;
pub mod probe;
pub mod select;
pub mod state;
pub mod verify;
pub mod version;

/// HOSTIF major this launcher speaks.
pub const HOSTIF_MAJOR: u64 = 1;
/// Capabilities the assistant UI needs from MiniCon.
pub const REQUIRED_CAPABILITIES: &[&str] = &["exec", "mux", "pty"];
/// Manifest `schema` values this launcher understands.
pub const ACCEPTED_MANIFEST_SCHEMAS: &[u64] = &[1];
/// Manifest `kind` this launcher accepts.
pub const MANIFEST_KIND: &str = "minicon-release-candidate";
/// Upper bound for the manifest body; anything larger is refused.
pub const MANIFEST_MAX_BYTES: u64 = 256 * 1024;
