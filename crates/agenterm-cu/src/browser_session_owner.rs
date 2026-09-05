//! Resident owner for one isolated Chromium session.
//!
//! The public lifecycle launcher writes the sealed spec and starting registry,
//! then starts this same executable in detached-owner mode. The owner alone
//! holds the session lock, browser child handle, and process-tree guard.

use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use agenterm_platform::{
    contained_process::{ContainedChild, ContainedHeadlessCommand},
    filesystem::write_private_atomic,
    filesystem_open::{ExistingEntryType, open_existing_path},
    locking::PathLock,
    process::start_identity,
};
use serde::{Deserialize, Serialize};

use crate::browser_session::owned_launch_args;
use crate::browser_session::{
    ACTIVE_PORT_MAX_BYTES, BrowserSessionEndpoint, BrowserSessionPaths, BrowserSessionRecord,
    BrowserSessionState, FileObjectIdentity, ProcessIdentity, active_port_path, parse_active_port,
    publish_record, read_record, same_generation, session_paths,
};

pub const OWNER_ARG: &str = "--agenterm-cu-internal-browser-session-owner";
pub const SPEC_FILE: &str = "owner-spec.json";
pub const OWNER_SPEC_SCHEMA_VERSION: u32 = 1;
const OWNER_SPEC_MAX_BYTES: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const EXIT_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserOwnerSpec {
    pub schema_version: u32,
    pub generation: u64,
    pub name: String,
    pub session_nonce: String,
    pub executable: PathBuf,
    pub ready_timeout_ms: u64,
    pub ttl_ms: u64,
}

impl BrowserOwnerSpec {
    pub fn validate(&self) -> Result<(), &'static str> {
        crate::browser_session::validate_session_name(&self.name)?;
        if self.schema_version != OWNER_SPEC_SCHEMA_VERSION
            || self.generation == 0
            || !(1_000..=60_000).contains(&self.ready_timeout_ms)
            || !(1_000..=86_400_000).contains(&self.ttl_ms)
            || !self.executable.is_absolute()
            || !self.executable.is_file()
        {
            return Err("browser_owner_spec_invalid");
        }
        let probe = BrowserSessionRecord {
            schema_version: crate::browser_session::REGISTRY_SCHEMA_VERSION,
            generation: self.generation,
            name: self.name.clone(),
            session_nonce: self.session_nonce.clone(),
            state: BrowserSessionState::Starting,
            owner: ProcessIdentity {
                pid: 1,
                start_identity: "validation".into(),
            },
            owner_spawn_mode: "validation".into(),
            profile_identity: FileObjectIdentity {
                filesystem_id: 1,
                object_id: 1,
            },
            browser: None,
            endpoint: None,
            last_error_code: None,
        };
        probe.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserStopRequest {
    pub schema_version: u32,
    pub generation: u64,
    pub session_nonce: String,
    pub owner: ProcessIdentity,
    pub browser: ProcessIdentity,
}

struct OwnedBrowser {
    child: ContainedChild,
}

impl OwnedBrowser {
    fn terminate_and_reap(&mut self) -> Result<(), String> {
        self.child
            .terminate_and_wait(EXIT_WAIT)
            .map_err(|error| format!("browser cleanup failed: {error}"))
    }
}

impl Drop for OwnedBrowser {
    fn drop(&mut self) {
        let _ = self.terminate_and_reap();
    }
}

pub fn spec_path(paths: &BrowserSessionPaths) -> PathBuf {
    paths.directory.join(SPEC_FILE)
}

pub fn publish_spec(path: &Path, spec: &BrowserOwnerSpec) -> io::Result<()> {
    spec.validate()
        .map_err(|code| io::Error::new(io::ErrorKind::InvalidInput, code))?;
    let bytes = serde_json::to_vec(spec).map_err(io::Error::other)?;
    if bytes.len() > OWNER_SPEC_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "browser owner spec exceeds its byte ceiling",
        ));
    }
    write_private_atomic(path, &bytes)
}

pub fn read_spec(path: &Path) -> io::Result<BrowserOwnerSpec> {
    let file = open_existing_path(path, ExistingEntryType::File)?;
    let metadata = file.metadata()?;
    if metadata.len() > OWNER_SPEC_MAX_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "browser owner spec exceeds its byte ceiling",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((OWNER_SPEC_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > OWNER_SPEC_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "browser owner spec exceeds its byte ceiling",
        ));
    }
    let spec: BrowserOwnerSpec = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    spec.validate()
        .map_err(|code| io::Error::new(io::ErrorKind::InvalidData, code))?;
    Ok(spec)
}

/// Internal same-binary entry point. It never prints the spec, executable path,
/// profile path, or browser command line.
pub fn run_owner(args: &[String]) -> i32 {
    match run_owner_inner(args) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn run_owner_inner(args: &[String]) -> Result<(), String> {
    let [directory] = args else {
        return Err("browser_owner_usage".into());
    };
    let directory = Path::new(directory);
    let directory_metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    if !directory_metadata.is_dir() || directory_metadata.file_type().is_symlink() {
        return Err("browser_owner_directory_invalid".into());
    }
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("browser_owner_directory_invalid")?;
    let root = directory
        .parent()
        .ok_or("browser_owner_directory_invalid")?;
    let paths = session_paths(root, name).map_err(str::to_owned)?;
    let _owner_lock = PathLock::try_acquire(&paths.owner_lock).map_err(|_| "browser_owner_busy")?;
    let spec = read_spec(&spec_path(&paths)).map_err(|_| "browser_owner_spec_invalid")?;
    if spec.name != name {
        return Err("browser_owner_spec_invalid".into());
    }
    // The launcher only learns this owner's PID/start identity after spawn.
    // It publishes Starting while holding the registry lock; wait boundedly
    // instead of racing that publication.
    let starting = wait_for_starting_record(&paths, &spec, Duration::from_secs(5))?;
    validate_starting_owner(&spec, &starting)?;

    if let Err(code) = clear_stale_endpoint(&paths.profile) {
        let code = if code == "browser_debug_endpoint_invalid" {
            "browser_debug_endpoint_invalid"
        } else {
            "browser_debug_endpoint_unavailable"
        };
        return publish_failure(&paths, &starting, code);
    }
    let mut browser = match spawn_owned_browser(&spec, &paths.profile) {
        Ok(browser) => browser,
        Err(OwnedSpawnError::Clean(code)) => return publish_failure(&paths, &starting, code),
    };
    let browser_pid = browser.child.id();
    let browser_identity = match start_identity(browser_pid) {
        Ok(start_identity) => ProcessIdentity {
            pid: browser_pid,
            start_identity,
        },
        Err(_) => {
            return publish_after_cleanup(
                &paths,
                &starting,
                &mut browser,
                "browser_identity_unavailable",
            );
        }
    };

    let endpoint = match wait_for_endpoint(
        &mut browser.child,
        &paths.profile,
        Duration::from_millis(spec.ready_timeout_ms),
    ) {
        Ok(endpoint) => endpoint,
        Err(code) => {
            return publish_after_cleanup(&paths, &starting, &mut browser, code);
        }
    };
    if !browser_identity_is_live(&mut browser.child, &browser_identity) {
        return publish_after_cleanup(
            &paths,
            &starting,
            &mut browser,
            "browser_identity_changed_before_ready",
        );
    }
    let mut ready = starting.clone();
    ready.state = BrowserSessionState::Ready;
    ready.browser = Some(browser_identity);
    ready.endpoint = Some(BrowserSessionEndpoint {
        port: endpoint.port,
        browser_path: endpoint.browser_path,
    });
    publish_if_owned(&paths, &starting, &ready)?;

    let expires = Instant::now() + Duration::from_millis(spec.ttl_ms);
    loop {
        let requested = match stop_requested(&paths, &ready) {
            Ok(requested) => requested,
            Err(_) => {
                return publish_after_cleanup(
                    &paths,
                    &ready,
                    &mut browser,
                    "browser_stop_request_invalid",
                );
            }
        };
        if requested {
            let mut stopping = ready.clone();
            stopping.state = BrowserSessionState::Stopping;
            publish_if_owned(&paths, &ready, &stopping)?;
            if browser.terminate_and_reap().is_err() {
                return publish_uncertain(&paths, &ready, "browser_cleanup_failed");
            }
            let mut stopped = stopping;
            stopped.state = BrowserSessionState::Stopped;
            stopped.endpoint = None;
            publish_if_owned(&paths, &ready, &stopped)?;
            return Ok(());
        }
        match browser.child.try_wait() {
            Ok(Some(_)) => {
                // On Unix an exited root removes the identity that makes a
                // later process-group signal safe. Descendants may remain;
                // never collapse that uncertainty into a clean failure.
                return publish_uncertain(&paths, &ready, "browser_tree_owner_exited");
            }
            Ok(None) if Instant::now() < expires => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                if browser.terminate_and_reap().is_err() {
                    return publish_uncertain(&paths, &ready, "browser_cleanup_failed");
                }
                let mut stopped = ready.clone();
                stopped.state = BrowserSessionState::Stopped;
                stopped.endpoint = None;
                stopped.last_error_code = Some("browser_ttl_expired".into());
                publish_if_owned(&paths, &ready, &stopped)?;
                return Ok(());
            }
            Err(_) => {
                return publish_after_cleanup(&paths, &ready, &mut browser, "browser_wait_failed");
            }
        }
    }
}

enum OwnedSpawnError {
    Clean(&'static str),
}

fn spawn_owned_browser(
    spec: &BrowserOwnerSpec,
    profile: &Path,
) -> Result<OwnedBrowser, OwnedSpawnError> {
    let mut command = ContainedHeadlessCommand::new(&spec.executable);
    command.args(owned_launch_args(profile));
    let child = command
        .spawn()
        .map_err(|_| OwnedSpawnError::Clean("browser_owner_spawn_failed"))?;
    Ok(OwnedBrowser { child })
}

fn browser_identity_is_live(child: &mut ContainedChild, expected: &ProcessIdentity) -> bool {
    matches!(child.try_wait(), Ok(None))
        && start_identity(expected.pid).ok().as_deref() == Some(expected.start_identity.as_str())
}

fn clear_stale_endpoint(profile: &Path) -> Result<(), String> {
    let path = active_port_path(profile);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("browser_debug_endpoint_unavailable".into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("browser_debug_endpoint_invalid".into());
    }
    fs::remove_file(path).map_err(|_| "browser_debug_endpoint_unavailable".into())
}

fn validate_starting_owner(
    spec: &BrowserOwnerSpec,
    record: &BrowserSessionRecord,
) -> Result<(), String> {
    if record.state != BrowserSessionState::Starting
        || record.generation != spec.generation
        || record.name != spec.name
        || record.session_nonce != spec.session_nonce
        || record.owner.pid != std::process::id()
        || start_identity(record.owner.pid).ok().as_deref()
            != Some(record.owner.start_identity.as_str())
    {
        return Err("browser_owner_identity_mismatch".into());
    }
    Ok(())
}

fn wait_for_starting_record(
    paths: &BrowserSessionPaths,
    spec: &BrowserOwnerSpec,
    timeout: Duration,
) -> Result<BrowserSessionRecord, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match read_record(&paths.registry) {
            Ok(record) => {
                validate_starting_owner(spec, &record)?;
                return Ok(record);
            }
            Err(_) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Err(_) => return Err("browser_owner_registry_timeout".into()),
        }
    }
}

fn wait_for_endpoint(
    child: &mut ContainedChild,
    profile: &Path,
    timeout: Duration,
) -> Result<crate::browser_session::DevToolsEndpoint, &'static str> {
    let deadline = Instant::now() + timeout;
    loop {
        match open_existing_path(&active_port_path(profile), ExistingEntryType::File) {
            Ok(file) => {
                let metadata = file
                    .metadata()
                    .map_err(|_| "browser_debug_endpoint_unavailable")?;
                if metadata.len() > ACTIVE_PORT_MAX_BYTES as u64 {
                    return Err("browser_debug_endpoint_invalid");
                }
                let mut bytes = Vec::with_capacity(metadata.len() as usize);
                file.take((ACTIVE_PORT_MAX_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "browser_debug_endpoint_unavailable")?;
                if let Ok(endpoint) = parse_active_port(&bytes) {
                    return Ok(endpoint);
                }
                // Chromium may expose the file while its second line is still
                // being written. Invalid contents become a typed timeout unless
                // they exceed the hard byte ceiling above.
            }
            Err(error) if endpoint_open_error_is_transient(error.kind()) => {}
            Err(_) => return Err("browser_debug_endpoint_unavailable"),
        }
        match child.try_wait() {
            Ok(Some(_)) => return Err("browser_exited_before_ready"),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => return Err("browser_ready_timeout"),
            Err(_) => return Err("browser_wait_failed"),
        }
    }
}

fn endpoint_open_error_is_transient(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied | io::ErrorKind::WouldBlock
    )
}

fn stop_requested(
    paths: &BrowserSessionPaths,
    ready: &BrowserSessionRecord,
) -> Result<bool, String> {
    let file = match open_existing_path(&paths.stop, ExistingEntryType::File) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err("browser_stop_request_unavailable".into()),
    };
    let metadata = file
        .metadata()
        .map_err(|_| "browser_stop_request_unavailable")?;
    if metadata.len() > OWNER_SPEC_MAX_BYTES as u64 {
        return Err("browser_stop_request_invalid".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((OWNER_SPEC_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "browser_stop_request_unavailable")?;
    if bytes.len() > OWNER_SPEC_MAX_BYTES {
        return Err("browser_stop_request_invalid".into());
    }
    let request: BrowserStopRequest =
        serde_json::from_slice(&bytes).map_err(|_| "browser_stop_request_invalid")?;
    Ok(request.schema_version == OWNER_SPEC_SCHEMA_VERSION
        && request.generation == ready.generation
        && request.session_nonce == ready.session_nonce
        && request.owner == ready.owner
        && ready.browser.as_ref() == Some(&request.browser))
}

fn publish_if_owned(
    paths: &BrowserSessionPaths,
    generation: &BrowserSessionRecord,
    next: &BrowserSessionRecord,
) -> Result<(), String> {
    let current = read_record(&paths.registry).map_err(|_| "browser_owner_registry_invalid")?;
    if !same_generation(generation, &current) || !same_generation(generation, next) {
        return Err("browser_owner_generation_replaced".into());
    }
    publish_record(&paths.registry, next)
        .map_err(|_| "browser_owner_registry_publish_failed".into())
}

fn publish_failure(
    paths: &BrowserSessionPaths,
    generation: &BrowserSessionRecord,
    code: &'static str,
) -> Result<(), String> {
    let mut failed = generation.clone();
    failed.state = BrowserSessionState::Failed;
    failed.endpoint = None;
    failed.last_error_code = Some(code.into());
    publish_if_owned(paths, generation, &failed)?;
    Err(code.into())
}

fn publish_uncertain(
    paths: &BrowserSessionPaths,
    generation: &BrowserSessionRecord,
    code: &'static str,
) -> Result<(), String> {
    let mut uncertain = generation.clone();
    uncertain.state = BrowserSessionState::OrphanedUncertain;
    uncertain.endpoint = None;
    uncertain.last_error_code = Some(code.into());
    publish_if_owned(paths, generation, &uncertain)?;
    Err(code.into())
}

fn publish_after_cleanup(
    paths: &BrowserSessionPaths,
    generation: &BrowserSessionRecord,
    browser: &mut OwnedBrowser,
    code: &'static str,
) -> Result<(), String> {
    if browser.terminate_and_reap().is_ok() {
        publish_failure(paths, generation, code)
    } else {
        publish_uncertain(paths, generation, "browser_cleanup_failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temporary_profile() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let profile = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root")
            .join("target/browser-owner-tests")
            .join(format!(
                "profile-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&profile).unwrap();
        profile
    }

    #[test]
    fn owner_spec_rejects_relative_executable_and_unbounded_times() {
        let spec = BrowserOwnerSpec {
            schema_version: OWNER_SPEC_SCHEMA_VERSION,
            generation: 1,
            name: "work".into(),
            session_nonce: "0123456789abcdef".into(),
            executable: PathBuf::from("browser"),
            ready_timeout_ms: 999,
            ttl_ms: 1_000,
        };
        assert_eq!(spec.validate(), Err("browser_owner_spec_invalid"));
    }

    #[test]
    fn stale_stop_request_does_not_match_replacement() {
        let ready = BrowserSessionRecord {
            schema_version: crate::browser_session::REGISTRY_SCHEMA_VERSION,
            generation: 2,
            name: "work".into(),
            session_nonce: "fedcba9876543210".into(),
            state: BrowserSessionState::Ready,
            owner: ProcessIdentity {
                pid: 22,
                start_identity: "new-owner".into(),
            },
            owner_spawn_mode: "independent".into(),
            profile_identity: FileObjectIdentity {
                filesystem_id: 11,
                object_id: 12,
            },
            browser: Some(ProcessIdentity {
                pid: 23,
                start_identity: "browser".into(),
            }),
            endpoint: Some(BrowserSessionEndpoint {
                port: 43127,
                browser_path: "/devtools/browser/id".into(),
            }),
            last_error_code: None,
        };
        let request = BrowserStopRequest {
            schema_version: OWNER_SPEC_SCHEMA_VERSION,
            generation: 1,
            session_nonce: "0123456789abcdef".into(),
            owner: ProcessIdentity {
                pid: 20,
                start_identity: "old-owner".into(),
            },
            browser: ProcessIdentity {
                pid: 21,
                start_identity: "old-browser".into(),
            },
        };
        assert!(request.generation != ready.generation);
        assert!(request.session_nonce != ready.session_nonce);
        assert!(request.owner != ready.owner);
        assert!(ready.browser.as_ref() != Some(&request.browser));
    }

    #[test]
    fn stale_endpoint_is_removed_before_a_new_browser_starts() {
        let profile = temporary_profile();
        let endpoint = active_port_path(&profile);
        fs::write(&endpoint, b"43127\n/devtools/browser/old\n").unwrap();
        clear_stale_endpoint(&profile).unwrap();
        assert!(!endpoint.exists());
        fs::remove_dir_all(profile).unwrap();
    }

    #[test]
    fn endpoint_poll_retries_only_creation_and_sharing_races() {
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::WouldBlock,
        ] {
            assert!(endpoint_open_error_is_transient(kind));
        }
        for kind in [
            io::ErrorKind::InvalidData,
            io::ErrorKind::InvalidInput,
            io::ErrorKind::Other,
        ] {
            assert!(!endpoint_open_error_is_transient(kind));
        }
    }

    #[cfg(unix)]
    #[test]
    fn stale_endpoint_symlink_is_refused_not_followed() {
        use std::os::unix::fs::symlink;

        let profile = temporary_profile();
        let outside = profile.parent().unwrap().join("outside-port");
        fs::write(&outside, b"keep").unwrap();
        symlink(&outside, active_port_path(&profile)).unwrap();
        assert_eq!(
            clear_stale_endpoint(&profile).unwrap_err(),
            "browser_debug_endpoint_invalid"
        );
        assert_eq!(fs::read(&outside).unwrap(), b"keep");
        fs::remove_file(active_port_path(&profile)).unwrap();
        fs::remove_dir(profile).unwrap();
        fs::remove_file(outside).unwrap();
    }
}
