//! Bounded host-resolution and TCP reachability observation.
//!
//! System resolvers are allowed to block indefinitely. The public caller
//! therefore owns a short-lived copy of this executable, sends one bounded
//! request, and kills *and reaps* that exact child at the overall deadline.

use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    process::{Child, ChildStdout, Command as ProcessCommand, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

#[cfg(not(test))]
use crate::dynlib;
use crate::reply::CuError;

pub const WORKER_ARG: &str = "--agenterm-cu-internal-network-probe-worker";
pub const FIXTURE_ARG: &str = "--agenterm-cu-internal-network-probe-fixture";
const MAX_HOST_BYTES: usize = 253;
const MAX_ADDRESSES: usize = 64;
const MAX_WORKERS: usize = 4;
const MAX_REQUEST_BYTES: usize = 4096;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProbeRequest {
    host: String,
    port: u16,
    attempts: u8,
    timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WorkerReply {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<WorkerError>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WorkerError {
    code: String,
    message: String,
}

#[derive(Debug)]
struct WorkerSlot;

impl WorkerSlot {
    fn acquire() -> Result<Self, CuError> {
        let acquired = ACTIVE_WORKERS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_WORKERS).then_some(count + 1)
        });
        acquired.map(|_| Self).map_err(|_| {
            CuError::new(
                "resolver_saturated",
                format!("network probe worker limit {MAX_WORKERS} is busy"),
            )
        })
    }
}

impl Drop for WorkerSlot {
    fn drop(&mut self) {
        ACTIVE_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn payload(
    host: &str,
    port: u16,
    attempts: u8,
    timeout_ms: u64,
) -> Result<serde_json::Value, CuError> {
    validate(host, port, attempts, timeout_ms)?;
    let _slot = WorkerSlot::acquire()?;
    let request = ProbeRequest {
        host: host.to_owned(),
        port,
        attempts,
        timeout_ms,
    };
    let encoded = serde_json::to_vec(&request)
        .map_err(|error| CuError::new("network_probe_protocol", error.to_string()))?;
    if encoded.len() > MAX_REQUEST_BYTES {
        return Err(CuError::new(
            "network_probe_protocol",
            "internal request exceeds its byte ceiling",
        ));
    }

    let executable = std::env::current_exe().map_err(|error| {
        CuError::new(
            "network_probe_worker_spawn",
            format!("cannot resolve current executable: {error}"),
        )
    })?;
    let mut command = ProcessCommand::new(executable);
    command
        .arg(WORKER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    agenterm_platform::process_spawn::configure_owned_headless_command(&mut command)
        .map_err(|error| CuError::new("network_probe_worker_spawn", error))?;
    let mut child = command.spawn().map_err(|error| {
        CuError::new(
            "network_probe_worker_spawn",
            format!("cannot start owned resolver worker: {error}"),
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        reap_after_kill(&mut child);
        CuError::new("network_probe_protocol", "worker stdout unavailable")
    })?;
    let reader = match spawn_bounded_reader(stdout) {
        Ok(reader) => reader,
        Err(error) => {
            reap_after_kill(&mut child);
            return Err(error);
        }
    };
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("worker stdin unavailable"))
        .and_then(|mut stdin| stdin.write_all(&encoded));
    if let Err(error) = write_result {
        reap_after_kill(&mut child);
        let _ = reader.join();
        return Err(CuError::new(
            "network_probe_protocol",
            format!("cannot send worker request: {error}"),
        ));
    }

    let overall_ms = timeout_ms
        .checked_mul(u64::from(attempts) + 1)
        .and_then(|value| value.checked_add(250))
        .ok_or_else(|| CuError::new("network_probe_limit", "overall deadline overflow"))?;
    let bytes = wait_and_read(&mut child, reader, Duration::from_millis(overall_ms))?;
    let reply: WorkerReply = serde_json::from_slice(&bytes).map_err(|error| {
        CuError::new(
            "network_probe_protocol",
            format!("invalid worker response: {error}"),
        )
    })?;
    match (reply.ok, reply.data, reply.error) {
        (true, Some(data), None) => Ok(data),
        (false, None, Some(error)) => Err(CuError::new(error.code, error.message)),
        _ => Err(CuError::new(
            "network_probe_protocol",
            "worker response has an inconsistent shape",
        )),
    }
}

fn validate(host: &str, port: u16, attempts: u8, timeout_ms: u64) -> Result<(), CuError> {
    if host.is_empty()
        || host.len() > MAX_HOST_BYTES
        || host.trim() != host
        || host.chars().any(char::is_whitespace)
    {
        return Err(CuError::new(
            "network_probe_limit",
            "host must be one bare non-whitespace value of 1..=253 bytes",
        ));
    }
    if port == 0 || !(1..=20).contains(&attempts) || !(100..=60_000).contains(&timeout_ms) {
        return Err(CuError::new(
            "network_probe_limit",
            "port must be 1..=65535, attempts 1..=20 and timeout-ms 100..=60000",
        ));
    }
    Ok(())
}

fn spawn_bounded_reader(
    mut stdout: ChildStdout,
) -> Result<thread::JoinHandle<std::io::Result<Vec<u8>>>, CuError> {
    thread::Builder::new()
        .name("agenterm-cu-network-probe-reader".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .by_ref()
                .take((MAX_RESPONSE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .map_err(|error| {
            CuError::new(
                "network_probe_worker_spawn",
                format!("cannot start bounded worker reader: {error}"),
            )
        })
}

fn wait_and_read(
    child: &mut Child,
    reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    timeout: Duration,
) -> Result<Vec<u8>, CuError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let _ = reader.join();
                    return Err(CuError::new(
                        "network_probe_worker_failed",
                        format!("owned resolver worker exited with {status}"),
                    ));
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                reap_after_kill(child);
                let _ = reader.join();
                return Err(CuError::new(
                    "network_probe_timeout",
                    "owned resolver worker exceeded the overall deadline and was reaped",
                ));
            }
            Err(error) => {
                reap_after_kill(child);
                let _ = reader.join();
                return Err(CuError::new(
                    "network_probe_worker_wait",
                    format!("cannot observe owned resolver worker: {error}"),
                ));
            }
        }
    }
    let bytes = reader
        .join()
        .map_err(|_| CuError::new("network_probe_protocol", "worker reader panicked"))?
        .map_err(|error| CuError::new("network_probe_protocol", error.to_string()))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(CuError::new(
            "network_probe_protocol",
            "worker response exceeds its byte ceiling",
        ));
    }
    Ok(bytes)
}

fn reap_after_kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs the internal worker. The binary intercepts [`WORKER_ARG`] before the
/// public CLI parser, so this protocol never appears in the verb catalog.
pub fn run_worker_stdio() -> i32 {
    let mut bytes = Vec::new();
    let read = std::io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes);
    let reply = match read {
        Ok(_) if bytes.len() <= MAX_REQUEST_BYTES => match serde_json::from_slice(&bytes) {
            Ok(request) => execute_request(&request),
            Err(error) => worker_error("network_probe_protocol", error.to_string()),
        },
        Ok(_) => worker_error(
            "network_probe_protocol",
            "internal request exceeds its byte ceiling",
        ),
        Err(error) => worker_error("network_probe_protocol", error.to_string()),
    };
    let mut stdout = std::io::stdout().lock();
    match serde_json::to_writer(&mut stdout, &reply)
        .and_then(|()| stdout.flush().map_err(serde_json::Error::io))
    {
        Ok(()) => 0,
        Err(_) => 2,
    }
}

/// Invocation-owned loopback listener for the three public OS journeys. This
/// is deliberately absent from the public verb catalog and refuses to start
/// without the test-fixture marker.
pub fn run_loopback_fixture(args: &[String]) -> i32 {
    if std::env::var_os("AGENTERM_CU_INTERNAL_TEST_FIXTURE").as_deref()
        != Some(std::ffi::OsStr::new("1"))
        || args.len() != 2
    {
        return 2;
    }
    let Ok(attempts) = args[0].parse::<u8>() else {
        return 2;
    };
    let Ok(timeout_ms) = args[1].parse::<u64>() else {
        return 2;
    };
    if !(1..=20).contains(&attempts) || !(100..=60_000).contains(&timeout_ms) {
        return 2;
    }
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(_) => return 3,
    };
    if listener.set_nonblocking(true).is_err() {
        return 3;
    }
    let port = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(_) => return 3,
    };
    let mut stdout = std::io::stdout().lock();
    if writeln!(stdout, "{{\"port\":{port}}}").is_err() || stdout.flush().is_err() {
        return 4;
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut accepted = 0_u8;
    while accepted < attempts {
        match listener.accept() {
            Ok((stream, _)) => {
                drop(stream);
                accepted += 1;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return 5;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return 3,
        }
    }
    0
}

fn execute_request(request: &ProbeRequest) -> WorkerReply {
    if let Err(error) = validate(
        &request.host,
        request.port,
        request.attempts,
        request.timeout_ms,
    ) {
        return worker_error(&error.code, error.message);
    }
    let started = Instant::now();
    let resolved = match resolve(&request.host, request.port) {
        Ok(addresses) => addresses,
        Err(error) => return error,
    };
    let timeout = Duration::from_millis(request.timeout_ms);
    let mut rows = Vec::with_capacity(request.attempts as usize);
    let mut connected = 0_u8;
    for attempt in 0..request.attempts {
        let address = resolved[usize::from(attempt) % resolved.len()];
        let attempt_started = Instant::now();
        let result = TcpStream::connect_timeout(&address, timeout);
        let latency_ms = attempt_started.elapsed().as_millis() as u64;
        let (outcome, error_kind) = match result {
            Ok(stream) => {
                connected += 1;
                drop(stream);
                ("connected", None)
            }
            Err(error) => (
                if error.kind() == std::io::ErrorKind::TimedOut {
                    "timeout"
                } else if error.kind() == std::io::ErrorKind::ConnectionRefused {
                    "refused"
                } else {
                    "unreachable"
                },
                Some(format!("{:?}", error.kind()).to_ascii_lowercase()),
            ),
        };
        rows.push(serde_json::json!({
            "attempt": attempt + 1,
            "address": address.ip().to_string(),
            "family": family(address.ip()),
            "outcome": outcome,
            "error_kind": error_kind,
            "latency_ms": latency_ms,
        }));
    }
    WorkerReply {
        ok: true,
        data: Some(serde_json::json!({
            "provider": "system-resolver-owned-worker",
            "host": request.host,
            "port": request.port,
            "resolved_addresses": resolved.iter().map(|address| serde_json::json!({
                "address": address.ip().to_string(),
                "family": family(address.ip()),
            })).collect::<Vec<_>>(),
            "address_count": resolved.len(),
            "attempt_count": request.attempts,
            "connected_count": connected,
            "unreachable_count": request.attempts - connected,
            "status": if connected > 0 { "reachable" } else { "unreachable" },
            "attempts": rows,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "address_set_frozen": true,
        })),
        error: None,
    }
}

#[cfg(not(test))]
fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, WorkerReply> {
    type Resolve = unsafe extern "C" fn(
        *const u8,
        usize,
        u16,
        *mut dynlib::agt_network_address,
        usize,
        *mut usize,
    ) -> i32;

    let lib = dynlib::load()
        .map_err(|error| worker_error("dns_mechanism_unavailable", error.message.clone()))?;
    let version = lib
        .abi_version()
        .map_err(|error| worker_error("dns_mechanism_unavailable", error))?;
    if (version & 0xffff) < u32::from(dynlib::NETWORK_RESOLVE_ABI_MINOR) {
        return Err(worker_error(
            "dns_mechanism_unavailable",
            format!(
                "libagenterm ABI 1.{} lacks network resolution",
                version & 0xffff
            ),
        ));
    }
    let call = unsafe { lib.sym::<Resolve>(b"agt_network_resolve") }
        .map_err(|error| worker_error("dns_mechanism_unavailable", error))?;
    let mut required = 0_usize;
    let first = unsafe {
        call(
            host.as_ptr(),
            host.len(),
            port,
            std::ptr::null_mut(),
            0,
            &mut required,
        )
    };
    if first != dynlib::AGT_FAILED || required == 0 {
        return Err(worker_error(
            "dns_resolution_failed",
            lib.last_error_message(),
        ));
    }
    if required > MAX_ADDRESSES {
        return Err(worker_error(
            "dns_result_too_large",
            format!("system resolver returned more than {MAX_ADDRESSES} unique addresses"),
        ));
    }
    let mut records = vec![dynlib::agt_network_address::default(); required];
    let status = unsafe {
        call(
            host.as_ptr(),
            host.len(),
            port,
            records.as_mut_ptr(),
            records.len(),
            &mut required,
        )
    };
    if status != dynlib::AGT_OK || required != records.len() {
        return Err(worker_error(
            "dns_resolution_failed",
            lib.last_error_message(),
        ));
    }
    records.into_iter().map(address_from_record).collect()
}

#[cfg(not(test))]
fn address_from_record(record: dynlib::agt_network_address) -> Result<SocketAddr, WorkerReply> {
    let ip = match record.family {
        4 if record.address[4..].iter().all(|byte| *byte == 0) => {
            IpAddr::V4(std::net::Ipv4Addr::new(
                record.address[0],
                record.address[1],
                record.address[2],
                record.address[3],
            ))
        }
        6 => IpAddr::V6(std::net::Ipv6Addr::from(record.address)),
        _ => {
            return Err(worker_error(
                "dns_protocol_invalid",
                "libagenterm returned an invalid address record",
            ));
        }
    };
    Ok(SocketAddr::new(ip, record.port))
}

#[cfg(test)]
fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, WorkerReply> {
    use std::{collections::BTreeSet, net::ToSocketAddrs};
    let unique: BTreeSet<_> = (host, port)
        .to_socket_addrs()
        .map_err(|error| worker_error("dns_resolution_failed", error.to_string()))?
        .collect();
    if unique.is_empty() || unique.len() > MAX_ADDRESSES {
        return Err(worker_error(
            "dns_resolution_failed",
            "invalid resolver result count",
        ));
    }
    Ok(unique.into_iter().collect())
}

fn worker_error(code: &str, message: impl Into<String>) -> WorkerReply {
    WorkerReply {
        ok: false,
        data: None,
        error: Some(WorkerError {
            code: code.to_owned(),
            message: message.into(),
        }),
    }
}

fn family(address: IpAddr) -> &'static str {
    match address {
        IpAddr::V4(_) => "ipv4",
        IpAddr::V6(_) => "ipv6",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Barrier, Mutex};

    #[test]
    fn closed_port_is_a_successful_unreachable_observation() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let reply = execute_request(&ProbeRequest {
            host: "127.0.0.1".into(),
            port,
            attempts: 2,
            timeout_ms: 100,
        });
        assert!(reply.ok);
        let data = reply.data.unwrap();
        assert_eq!(data["status"], "unreachable");
        assert_eq!(data["attempt_count"], 2);
        assert_eq!(data["attempts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn listener_is_reached_with_exact_attempt_count() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = thread::spawn(move || {
            for _ in 0..2 {
                listener.accept().unwrap();
            }
        });
        let reply = execute_request(&ProbeRequest {
            host: "127.0.0.1".into(),
            port,
            attempts: 2,
            timeout_ms: 500,
        });
        acceptor.join().unwrap();
        assert!(reply.ok);
        let data = reply.data.unwrap();
        assert_eq!(data["status"], "reachable");
        assert_eq!(data["connected_count"], 2);
    }

    #[test]
    fn limits_fail_before_resolution() {
        let error = validate("bad host", 443, 3, 3000).unwrap_err();
        assert_eq!(error.code, "network_probe_limit");
        let error = validate("example.invalid", 443, 21, 3000).unwrap_err();
        assert_eq!(error.code, "network_probe_limit");
    }

    #[test]
    fn thirty_two_concurrent_requests_cannot_exceed_four_worker_slots() {
        let start = Arc::new(Barrier::new(33));
        let hold = Arc::new(Barrier::new(33));
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let threads: Vec<_> = (0..32)
            .map(|_| {
                let start = start.clone();
                let hold = hold.clone();
                let outcomes = outcomes.clone();
                thread::spawn(move || {
                    start.wait();
                    let slot = WorkerSlot::acquire();
                    outcomes.lock().unwrap().push(slot.is_ok());
                    hold.wait();
                    drop(slot);
                })
            })
            .collect();
        start.wait();
        hold.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        let outcomes = outcomes.lock().unwrap();
        assert_eq!(outcomes.iter().filter(|value| **value).count(), MAX_WORKERS);
        assert_eq!(outcomes.len(), 32);
    }

    #[test]
    fn stalled_owned_worker_is_killed_and_reaped_inside_the_bound() {
        let executable = std::env::current_exe().unwrap();
        let mut command = ProcessCommand::new(executable);
        command
            .args([
                "--exact",
                "network_probe::tests::owned_stall_fixture",
                "--nocapture",
            ])
            .env("AGENTERM_CU_TEST_NETWORK_STALL", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        agenterm_platform::process_spawn::configure_owned_headless_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let reader = spawn_bounded_reader(child.stdout.take().unwrap()).unwrap();
        let started = Instant::now();
        let error = wait_and_read(&mut child, reader, Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.code, "network_probe_timeout");
        assert!(started.elapsed() < Duration::from_millis(350));
        assert!(child.try_wait().unwrap().is_some(), "child must be reaped");
    }

    #[test]
    fn owned_stall_fixture() {
        if std::env::var_os("AGENTERM_CU_TEST_NETWORK_STALL").is_some() {
            thread::sleep(Duration::from_secs(60));
        }
    }
}
