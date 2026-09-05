//! macOS effective resolver inventory from the fixed `scutil --dns` provider.

use std::{
    io::Read,
    net::IpAddr,
    thread,
    time::{Duration, Instant},
};

use crate::{
    contained_process::ContainedHeadlessCommand,
    contract::{
        network_dns::{
            NETWORK_DNS_SCAN_CEILING, NETWORK_DNS_TEXT_CEILING, NetworkDnsCoverage,
            NetworkDnsError, NetworkDnsErrorKind, NetworkDnsFamily, NetworkDnsInventory,
            NetworkDnsResolver, NetworkDnsSearchDomain,
        },
        process_spawn::ProcessExit,
    },
};

const SCUTIL: &str = "/usr/sbin/scutil";
const OUTPUT_CEILING: u64 = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn enumerate_native() -> Result<NetworkDnsInventory, NetworkDnsError> {
    let bytes = run_scutil()?;
    parse_scutil(&bytes)
}

fn run_scutil() -> Result<Vec<u8>, NetworkDnsError> {
    let metadata = std::fs::symlink_metadata(SCUTIL)
        .map_err(|_| unavailable("the fixed /usr/sbin/scutil provider is unavailable"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(unavailable(
            "the fixed /usr/sbin/scutil provider is not a direct regular file",
        ));
    }
    let mut command = ContainedHeadlessCommand::new(SCUTIL);
    command.arg("--dns").capture_output();
    let mut child = command
        .spawn()
        .map_err(|error| unavailable(format!("scutil could not be started: {error}")))?;
    let stdout = match child.take_stdout() {
        Some(stdout) => stdout,
        None => {
            let _ = child.terminate_and_wait(CLEANUP_TIMEOUT);
            return Err(unavailable("scutil stdout capture was unavailable"));
        }
    };
    let stderr = match child.take_stderr() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            let _ = child.terminate_and_wait(CLEANUP_TIMEOUT);
            return Err(unavailable("scutil stderr capture was unavailable"));
        }
    };
    let stdout_thread = thread::spawn(move || read_bounded(stdout));
    let stderr_thread = thread::spawn(move || read_bounded(stderr));
    let deadline = Instant::now()
        .checked_add(TIMEOUT)
        .ok_or_else(|| timeout("scutil deadline overflow"))?;
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break Ok(exit),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                break match child.terminate_and_wait(CLEANUP_TIMEOUT) {
                    Ok(()) => Err(timeout("scutil exceeded its 10-second deadline")),
                    Err(error) => Err(timeout(format!(
                        "scutil timed out and cleanup failed: {error}"
                    ))),
                };
            }
            Err(error) => {
                break match child.terminate_and_wait(CLEANUP_TIMEOUT) {
                    Ok(()) => Err(unavailable(format!("scutil wait failed: {error}"))),
                    Err(cleanup) => Err(unavailable(format!(
                        "scutil wait failed: {error}; cleanup failed: {cleanup}"
                    ))),
                };
            }
        }
    };
    let stdout_result = stdout_thread
        .join()
        .map_err(|_| unavailable("scutil stdout reader panicked"));
    let stderr_result = stderr_thread
        .join()
        .map_err(|_| unavailable("scutil stderr reader panicked"));
    // Join both capture threads before propagating either error. Dropping one
    // JoinHandle after the child exits would detach a reader and make cleanup
    // ownership ambiguous on an otherwise typed failure path.
    let stdout = stdout_result??;
    let stderr = stderr_result??;
    let exit = outcome?;
    if !matches!(exit, ProcessExit::Code(0)) {
        let detail = String::from_utf8_lossy(&stderr);
        return Err(unavailable(format!(
            "scutil exited {:?}: {}",
            exit,
            detail.trim()
        )));
    }
    Ok(stdout)
}

fn read_bounded(mut stream: impl Read) -> Result<Vec<u8>, NetworkDnsError> {
    let mut bytes = Vec::new();
    stream
        .by_ref()
        .take(OUTPUT_CEILING + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| unavailable(format!("scutil output read failed: {error}")))?;
    if bytes.len() as u64 > OUTPUT_CEILING {
        return Err(NetworkDnsError::new(
            NetworkDnsErrorKind::ResourceLimit,
            "scutil output exceeds the 1 MiB ceiling",
        ));
    }
    Ok(bytes)
}

#[derive(Default)]
struct ResolverBlock {
    index: Option<u32>,
    addresses: Vec<String>,
    domains: Vec<String>,
    port: u16,
    interface: Option<String>,
    interface_native_id: Option<u64>,
}

fn parse_scutil(bytes: &[u8]) -> Result<NetworkDnsInventory, NetworkDnsError> {
    let text = std::str::from_utf8(bytes).map_err(|_| malformed("scutil output encoding"))?;
    let mut inventory = NetworkDnsInventory {
        resolvers: Vec::new(),
        search_domains: Vec::new(),
        visited: 0,
        read_errors: 0,
        truncated_scan: false,
        truncated: false,
        complete: false,
        provider: "scutil --dns",
        coverage: NetworkDnsCoverage::SystemEffective,
    };
    let mut block = ResolverBlock {
        port: 53,
        ..ResolverBlock::default()
    };
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(value) = line.strip_prefix("resolver #") {
            flush_block(&mut inventory, &mut block);
            block.index = value.trim().parse().ok();
            block.port = 53;
        } else if let Some(value) = property(line, "nameserver[") {
            if value.len() <= NETWORK_DNS_TEXT_CEILING {
                block.addresses.push(value.to_owned());
            } else {
                inventory.read_errors += 1;
            }
        } else if let Some(value) = property(line, "search domain[") {
            if valid_domain(value) {
                block.domains.push(value.trim_end_matches('.').to_owned());
            } else {
                inventory.read_errors += 1;
            }
        } else if let Some(value) = line.strip_prefix("port") {
            if let Some(value) = value.split_once(':').map(|(_, value)| value.trim()) {
                match value.parse::<u16>() {
                    Ok(0) | Err(_) => inventory.read_errors += 1,
                    Ok(port) => block.port = port,
                }
            }
        } else if let Some(value) = line.strip_prefix("if_index")
            && let Some(value) = value.split_once(':').map(|(_, value)| value.trim())
        {
            let (number, name) = value.split_once(' ').unwrap_or((value, ""));
            block.interface_native_id = number.parse().ok();
            block.interface = name
                .trim()
                .strip_prefix('(')
                .and_then(|value| value.strip_suffix(')'))
                .filter(|value| !value.is_empty() && value.len() <= NETWORK_DNS_TEXT_CEILING)
                .map(str::to_owned);
        }
    }
    flush_block(&mut inventory, &mut block);
    Ok(inventory)
}

fn property<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)?
        .split_once(':')
        .map(|(_, value)| value.trim())
}

fn flush_block(inventory: &mut NetworkDnsInventory, block: &mut ResolverBlock) {
    for value in std::mem::take(&mut block.addresses) {
        if !reserve_row(inventory) {
            break;
        }
        match parse_scoped_ip(&value) {
            Some((address, scope_id)) => inventory.resolvers.push(NetworkDnsResolver {
                family: family(address),
                address,
                port: block.port,
                interface: block.interface.clone(),
                interface_native_id: block.interface_native_id,
                scope_id,
                service: None,
                resolver_index: block.index,
            }),
            None => inventory.read_errors += 1,
        }
    }
    for domain in std::mem::take(&mut block.domains) {
        if !reserve_row(inventory) {
            break;
        }
        inventory.search_domains.push(NetworkDnsSearchDomain {
            domain,
            interface: block.interface.clone(),
            interface_native_id: block.interface_native_id,
            service: None,
            resolver_index: block.index,
        });
    }
    block.index = None;
    block.interface = None;
    block.interface_native_id = None;
}

fn reserve_row(inventory: &mut NetworkDnsInventory) -> bool {
    if inventory.visited == NETWORK_DNS_SCAN_CEILING {
        inventory.truncated_scan = true;
        return false;
    }
    inventory.visited += 1;
    true
}

fn parse_scoped_ip(value: &str) -> Option<(IpAddr, Option<u32>)> {
    let (address, scope) = value
        .split_once('%')
        .map_or((value, None), |(address, scope)| {
            (address, scope.parse().ok())
        });
    Some((address.parse().ok()?, scope))
}

fn family(address: IpAddr) -> NetworkDnsFamily {
    match address {
        IpAddr::V4(_) => NetworkDnsFamily::Ipv4,
        IpAddr::V6(_) => NetworkDnsFamily::Ipv6,
    }
}

fn valid_domain(value: &str) -> bool {
    let value = value.trim_end_matches('.');
    !value.is_empty()
        && value.len() <= NETWORK_DNS_TEXT_CEILING.min(253)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn unavailable(detail: impl Into<String>) -> NetworkDnsError {
    NetworkDnsError::new(NetworkDnsErrorKind::Unavailable, detail)
}

fn malformed(detail: impl Into<String>) -> NetworkDnsError {
    NetworkDnsError::new(NetworkDnsErrorKind::MalformedSnapshot, detail)
}

fn timeout(detail: impl Into<String>) -> NetworkDnsError {
    NetworkDnsError::new(NetworkDnsErrorKind::Timeout, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scoped_resolvers_and_domains_by_resolver_block() {
        let raw = br#"
resolver #1
  search domain[0] : example.test
  nameserver[0] : 192.0.2.53
  if_index : 14 (en0)
  port : 53
resolver #2
  nameserver[0] : fe80::53%7
  port : 5353
"#;
        let inventory = parse_scutil(raw).unwrap();
        assert_eq!(inventory.resolvers.len(), 2);
        assert_eq!(inventory.resolvers[0].interface.as_deref(), Some("en0"));
        assert_eq!(inventory.resolvers[1].scope_id, Some(7));
        assert_eq!(inventory.resolvers[1].port, 5353);
        assert_eq!(inventory.search_domains[0].domain, "example.test");
        assert_eq!(inventory.visited, 3);
    }
}
