//! Pure contracts for an ACU-owned Chromium session. The resident owner and
//! durable registry consume these helpers; no user profile is opened or copied
//! here.

use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use agenterm_platform::filesystem::{
    host_directories, protect_private_directory, write_private_atomic,
};
use agenterm_platform::filesystem_open::{ExistingEntryType, open_existing_path};
use serde::{Deserialize, Serialize};

pub const ACTIVE_PORT_FILE: &str = "DevToolsActivePort";
pub const OWNER_MARKER_FILE: &str = ".agenterm-cu-browser-session";
pub const ACTIVE_PORT_MAX_BYTES: usize = 4096;
pub const REGISTRY_FILE: &str = "registry.json";
pub const OWNER_LOCK_FILE: &str = "owner.lock";
pub const STOP_FILE: &str = "stop.json";
pub const DONE_FILE: &str = "done.json";
pub const PROFILE_DIRECTORY: &str = "profile";
pub const REGISTRY_SCHEMA_VERSION: u32 = 1;
pub const REGISTRY_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserSessionPaths {
    pub directory: PathBuf,
    pub profile: PathBuf,
    pub registry: PathBuf,
    pub owner_lock: PathBuf,
    pub stop: PathBuf,
    pub done: PathBuf,
}

impl BrowserSessionPaths {
    fn under(root: &Path, name: &str) -> Self {
        let directory = root.join(name);
        Self {
            profile: directory.join(PROFILE_DIRECTORY),
            registry: directory.join(REGISTRY_FILE),
            owner_lock: directory.join(OWNER_LOCK_FILE),
            stop: directory.join(STOP_FILE),
            done: directory.join(DONE_FILE),
            directory,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSessionState {
    Starting,
    Ready,
    Stopping,
    Stopped,
    Failed,
    OrphanedUncertain,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_identity: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileObjectIdentity {
    pub filesystem_id: u64,
    pub object_id: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserSessionEndpoint {
    pub port: u16,
    pub browser_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserSessionRecord {
    pub schema_version: u32,
    pub generation: u64,
    pub name: String,
    pub session_nonce: String,
    pub state: BrowserSessionState,
    pub owner: ProcessIdentity,
    pub owner_spawn_mode: String,
    pub profile_identity: FileObjectIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser: Option<ProcessIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<BrowserSessionEndpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<String>,
}

impl BrowserSessionRecord {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != REGISTRY_SCHEMA_VERSION {
            return Err("browser_session_registry_version");
        }
        validate_session_name(&self.name)?;
        validate_nonce(&self.session_nonce)?;
        validate_process_identity(&self.owner)?;
        if let Some(browser) = &self.browser {
            validate_process_identity(browser)?;
        }
        if self.owner_spawn_mode.is_empty() || self.owner_spawn_mode.len() > 32 {
            return Err("browser_session_registry_invalid");
        }
        if self.last_error_code.as_ref().is_some_and(|code| {
            code.is_empty()
                || code.len() > 96
                || !code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        }) {
            return Err("browser_session_registry_invalid");
        }
        match (self.state, self.browser.as_ref(), self.endpoint.as_ref()) {
            (BrowserSessionState::Starting, None, None)
            | (BrowserSessionState::Failed, _, _)
            | (BrowserSessionState::OrphanedUncertain, _, _)
            | (BrowserSessionState::Stopped, _, _) => {}
            (
                BrowserSessionState::Ready | BrowserSessionState::Stopping,
                Some(_),
                Some(endpoint),
            ) if valid_endpoint(endpoint) => {}
            _ => return Err("browser_session_registry_invalid"),
        }
        Ok(())
    }
}

pub fn sessions_root(create: bool) -> io::Result<PathBuf> {
    let directories = host_directories().map_err(io::Error::other)?;
    let root = directories
        .local_data
        .join("agenterm")
        .join("browser-sessions");
    if create {
        fs::create_dir_all(&root)?;
        protect_private_directory(&root)?;
    }
    Ok(root)
}

pub fn session_paths(root: &Path, name: &str) -> Result<BrowserSessionPaths, &'static str> {
    validate_session_name(name)?;
    Ok(BrowserSessionPaths::under(root, name))
}

pub fn create_session_directories(paths: &BrowserSessionPaths) -> io::Result<()> {
    fs::create_dir_all(&paths.profile)?;
    protect_private_directory(&paths.directory)?;
    protect_private_directory(&paths.profile)
}

pub fn publish_record(path: &Path, record: &BrowserSessionRecord) -> io::Result<()> {
    record
        .validate()
        .map_err(|code| io::Error::new(io::ErrorKind::InvalidInput, code))?;
    let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    if bytes.len() > REGISTRY_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "browser session registry exceeds its byte ceiling",
        ));
    }
    write_private_atomic(path, &bytes)
}

pub fn read_record(path: &Path) -> io::Result<BrowserSessionRecord> {
    let file = open_existing_path(path, ExistingEntryType::File)?;
    let metadata = file.metadata()?;
    if metadata.len() > REGISTRY_MAX_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "browser session registry exceeds its byte ceiling",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((REGISTRY_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > REGISTRY_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "browser session registry exceeds its byte ceiling",
        ));
    }
    let record: BrowserSessionRecord = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    record
        .validate()
        .map_err(|code| io::Error::new(io::ErrorKind::InvalidData, code))?;
    Ok(record)
}

pub fn same_generation(left: &BrowserSessionRecord, right: &BrowserSessionRecord) -> bool {
    left.generation == right.generation
        && left.name == right.name
        && left.session_nonce == right.session_nonce
        && left.owner == right.owner
}

fn validate_nonce(nonce: &str) -> Result<(), &'static str> {
    if (16..=64).contains(&nonce.len()) && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("browser_session_registry_invalid")
    }
}

fn validate_process_identity(identity: &ProcessIdentity) -> Result<(), &'static str> {
    if identity.pid > 0
        && !identity.start_identity.is_empty()
        && identity.start_identity.len() <= 256
        && !identity.start_identity.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err("browser_session_registry_invalid")
    }
}

fn valid_endpoint(endpoint: &BrowserSessionEndpoint) -> bool {
    endpoint.port > 0
        && endpoint.browser_path.starts_with("/devtools/browser/")
        && endpoint.browser_path.len() <= 1024
        && endpoint
            .browser_path
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'#')
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevToolsEndpoint {
    pub port: u16,
    pub browser_path: String,
}

impl DevToolsEndpoint {
    pub fn websocket_url(&self) -> String {
        format!("ws://127.0.0.1:{}{}", self.port, self.browser_path)
    }
}

pub fn validate_session_name(name: &str) -> Result<(), &'static str> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 32
        || !bytes[0].is_ascii_lowercase()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err("browser_session_name_invalid");
    }
    Ok(())
}

pub fn parse_active_port(bytes: &[u8]) -> Result<DevToolsEndpoint, &'static str> {
    if bytes.is_empty() || bytes.len() > ACTIVE_PORT_MAX_BYTES {
        return Err("browser_debug_endpoint_invalid");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "browser_debug_endpoint_invalid")?;
    let mut lines = text.lines();
    let port = lines
        .next()
        .and_then(|line| line.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .ok_or("browser_debug_endpoint_invalid")?;
    let browser_path = lines
        .next()
        .filter(|line| line.starts_with("/devtools/browser/"))
        .filter(|line| {
            line.len() <= 1024
                && line
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() && byte != b'#')
        })
        .ok_or("browser_debug_endpoint_invalid")?;
    if lines.any(|line| !line.is_empty()) {
        return Err("browser_debug_endpoint_invalid");
    }
    Ok(DevToolsEndpoint {
        port,
        browser_path: browser_path.to_owned(),
    })
}

pub fn active_port_path(profile_root: &Path) -> PathBuf {
    profile_root.join(ACTIVE_PORT_FILE)
}

pub fn owned_launch_args(profile_root: &Path) -> Vec<String> {
    vec![
        format!("--user-data-dir={}", profile_root.display()),
        "--remote-debugging-port=0".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--no-startup-window".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn record(state: BrowserSessionState) -> BrowserSessionRecord {
        BrowserSessionRecord {
            schema_version: REGISTRY_SCHEMA_VERSION,
            generation: 7,
            name: "work".into(),
            session_nonce: "0123456789abcdef".into(),
            state,
            owner: ProcessIdentity {
                pid: 42,
                start_identity: "owner-start".into(),
            },
            owner_spawn_mode: "independent".into(),
            profile_identity: FileObjectIdentity {
                filesystem_id: 11,
                object_id: 12,
            },
            browser: None,
            endpoint: None,
            last_error_code: None,
        }
    }

    fn temporary_directory() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root")
            .join("target/browser-session-tests")
            .join(format!(
                "agenterm-cu-browser-session-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
    }

    #[test]
    fn session_name_is_a_closed_portable_component() {
        for valid in ["a", "browser-1", "work"] {
            validate_session_name(valid).unwrap();
        }
        for invalid in [
            "",
            "A",
            "1work",
            "work_space",
            "work/space",
            &"x".repeat(33),
        ] {
            assert_eq!(
                validate_session_name(invalid).unwrap_err(),
                "browser_session_name_invalid"
            );
        }
    }

    #[test]
    fn parses_chromium_active_port_without_guessing() {
        let endpoint = parse_active_port(b"43127\n/devtools/browser/abc-123\n").unwrap();
        assert_eq!(endpoint.port, 43127);
        assert_eq!(
            endpoint.websocket_url(),
            "ws://127.0.0.1:43127/devtools/browser/abc-123"
        );
    }

    #[test]
    fn rejects_bad_or_ambiguous_active_port_records() {
        for bytes in [
            b"0\n/devtools/browser/id\n".as_slice(),
            b"9222\n/devtools/page/id\n".as_slice(),
            b"9222\n/devtools/browser/id#fragment\n".as_slice(),
            b"9222\n/devtools/browser/id\nextra\n".as_slice(),
            b"not-a-port\n/devtools/browser/id\n".as_slice(),
        ] {
            assert_eq!(
                parse_active_port(bytes).unwrap_err(),
                "browser_debug_endpoint_invalid"
            );
        }
    }

    #[test]
    fn owned_launch_uses_random_port_file_contract() {
        let root = Path::new("browser-session-fixture");
        let args = owned_launch_args(root);
        assert!(args.contains(&"--remote-debugging-port=0".to_owned()));
        assert!(args.contains(&"--no-startup-window".to_owned()));
        assert!(
            args.iter()
                .any(|arg| arg == "--user-data-dir=browser-session-fixture")
        );
        assert_eq!(active_port_path(root), root.join(ACTIVE_PORT_FILE));
    }

    #[test]
    fn registry_state_requires_browser_identity_and_endpoint_together() {
        let mut starting = record(BrowserSessionState::Starting);
        starting.validate().unwrap();
        starting.state = BrowserSessionState::Ready;
        assert_eq!(
            starting.validate().unwrap_err(),
            "browser_session_registry_invalid"
        );

        starting.browser = Some(ProcessIdentity {
            pid: 43,
            start_identity: "browser-start".into(),
        });
        starting.endpoint = Some(BrowserSessionEndpoint {
            port: 43127,
            browser_path: "/devtools/browser/abc-123".into(),
        });
        starting.validate().unwrap();
    }

    #[test]
    fn registry_is_bounded_atomic_and_round_trips() {
        let root = temporary_directory();
        let paths = session_paths(&root, "work").unwrap();
        create_session_directories(&paths).unwrap();
        let first = record(BrowserSessionState::Starting);
        publish_record(&paths.registry, &first).unwrap();
        assert_eq!(read_record(&paths.registry).unwrap(), first);

        let mut second = first.clone();
        second.state = BrowserSessionState::Failed;
        second.last_error_code = Some("browser_start_failed".into());
        publish_record(&paths.registry, &second).unwrap();
        assert_eq!(read_record(&paths.registry).unwrap(), second);
        assert!(same_generation(&first, &second));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn registry_reader_rejects_a_final_symlink() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory();
        let paths = session_paths(&root, "work").unwrap();
        create_session_directories(&paths).unwrap();
        let outside = root.join("outside.json");
        fs::write(
            &outside,
            serde_json::to_vec(&record(BrowserSessionState::Starting)).unwrap(),
        )
        .unwrap();
        symlink(&outside, &paths.registry).unwrap();
        assert!(read_record(&paths.registry).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_identity_uses_generation_nonce_and_owner() {
        let first = record(BrowserSessionState::Starting);
        let mut state_update = first.clone();
        state_update.state = BrowserSessionState::Ready;
        state_update.browser = Some(ProcessIdentity {
            pid: 43,
            start_identity: "browser-start".into(),
        });
        state_update.endpoint = Some(BrowserSessionEndpoint {
            port: 43127,
            browser_path: "/devtools/browser/id".into(),
        });
        assert!(same_generation(&first, &state_update));
        state_update.session_nonce = "fedcba9876543210".into();
        assert!(!same_generation(&first, &state_update));
    }
}
