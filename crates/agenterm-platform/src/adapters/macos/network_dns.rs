//! macOS effective resolver inventory from the fixed `scutil --dns` provider,
//! with per-service identity joined from the fixed
//! `networksetup -listallhardwareports` device map.

use std::{
    collections::BTreeMap,
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
const NETWORKSETUP: &str = "/usr/sbin/networksetup";
const PROVIDER_SCUTIL: &str = "scutil --dns";
const PROVIDER_JOINED: &str = "scutil --dns + networksetup -listallhardwareports";
const OUTPUT_CEILING: u64 = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn enumerate_native() -> Result<NetworkDnsInventory, NetworkDnsError> {
    let bytes = run_fixed_provider(SCUTIL, "scutil", &["--dns"])?;
    let mut inventory = parse_scutil(&bytes)?;
    join_service_identity(&mut inventory);
    Ok(inventory)
}

/// Second bounded provider: `networksetup -listallhardwareports` maps each
/// device to its Hardware Port, which is the per-service identity macOS
/// exposes. Its failure never discards the resolver rows already read from
/// `scutil`; instead every row keeps `service: None`, `provider` stays the
/// scutil-only label and `read_errors` gains one so `complete` turns false.
fn join_service_identity(inventory: &mut NetworkDnsInventory) {
    let joined = run_fixed_provider(NETWORKSETUP, "networksetup", &["-listallhardwareports"])
        .and_then(|bytes| parse_hardware_ports(&bytes));
    apply_join_outcome(inventory, joined);
}

fn apply_join_outcome(
    inventory: &mut NetworkDnsInventory,
    joined: Result<(BTreeMap<String, String>, usize), NetworkDnsError>,
) {
    match joined {
        Ok((map, skipped)) => {
            mark_service_join_incomplete(inventory, skipped);
            inventory.provider = PROVIDER_JOINED;
            apply_service_map(inventory, &map);
        }
        Err(_) => {
            mark_service_join_incomplete(inventory, 1);
            inventory.provider = PROVIDER_SCUTIL;
        }
    }
}

/// Preserve the inventory-wide `read_errors <= visited` contract while
/// recording that one supplemental provider could not describe every row.
/// When there are no DNS rows, there is no service-bearing row for the failed
/// join to make incomplete.
fn mark_service_join_incomplete(inventory: &mut NetworkDnsInventory, failures: usize) {
    if failures > 0 && inventory.read_errors < inventory.visited {
        inventory.read_errors += 1;
    }
}

fn run_fixed_provider(
    program: &str,
    label: &str,
    args: &[&str],
) -> Result<Vec<u8>, NetworkDnsError> {
    let metadata = std::fs::symlink_metadata(program)
        .map_err(|_| unavailable(format!("the fixed {program} provider is unavailable")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(unavailable(format!(
            "the fixed {program} provider is not a direct regular file"
        )));
    }
    let mut command = ContainedHeadlessCommand::new(program);
    // Pin the C locale so key names such as `Hardware Port:` stay stable.
    command
        .args(args)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .capture_output();
    let mut child = command
        .spawn()
        .map_err(|error| unavailable(format!("{label} could not be started: {error}")))?;
    let stdout = match child.take_stdout() {
        Some(stdout) => stdout,
        None => {
            let _ = child.terminate_and_wait(CLEANUP_TIMEOUT);
            return Err(unavailable(format!(
                "{label} stdout capture was unavailable"
            )));
        }
    };
    let stderr = match child.take_stderr() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            let _ = child.terminate_and_wait(CLEANUP_TIMEOUT);
            return Err(unavailable(format!(
                "{label} stderr capture was unavailable"
            )));
        }
    };
    let stdout_thread = thread::spawn(move || read_bounded(stdout, "stdout"));
    let stderr_thread = thread::spawn(move || read_bounded(stderr, "stderr"));
    let deadline = Instant::now()
        .checked_add(TIMEOUT)
        .ok_or_else(|| timeout(format!("{label} deadline overflow")))?;
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break Ok(exit),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                break match child.terminate_and_wait(CLEANUP_TIMEOUT) {
                    Ok(()) => Err(timeout(format!("{label} exceeded its 10-second deadline"))),
                    Err(error) => Err(timeout(format!(
                        "{label} timed out and cleanup failed: {error}"
                    ))),
                };
            }
            Err(error) => {
                break match child.terminate_and_wait(CLEANUP_TIMEOUT) {
                    Ok(()) => Err(unavailable(format!("{label} wait failed: {error}"))),
                    Err(cleanup) => Err(unavailable(format!(
                        "{label} wait failed: {error}; cleanup failed: {cleanup}"
                    ))),
                };
            }
        }
    };
    let stdout_result = stdout_thread
        .join()
        .map_err(|_| unavailable(format!("{label} stdout reader panicked")));
    let stderr_result = stderr_thread
        .join()
        .map_err(|_| unavailable(format!("{label} stderr reader panicked")));
    // Join both capture threads before propagating either error. Dropping one
    // JoinHandle after the child exits would detach a reader and make cleanup
    // ownership ambiguous on an otherwise typed failure path.
    let stdout = stdout_result??;
    let stderr = stderr_result??;
    let exit = outcome?;
    if !matches!(exit, ProcessExit::Code(0)) {
        let detail = String::from_utf8_lossy(&stderr);
        return Err(unavailable(format!(
            "{label} exited {:?}: {}",
            exit,
            detail.trim()
        )));
    }
    Ok(stdout)
}

fn read_bounded(mut stream: impl Read, stream_name: &str) -> Result<Vec<u8>, NetworkDnsError> {
    let mut bytes = Vec::new();
    stream
        .by_ref()
        .take(OUTPUT_CEILING + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| unavailable(format!("provider {stream_name} read failed: {error}")))?;
    if bytes.len() as u64 > OUTPUT_CEILING {
        return Err(NetworkDnsError::new(
            NetworkDnsErrorKind::ResourceLimit,
            format!("provider {stream_name} exceeds the 1 MiB ceiling"),
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
        provider: PROVIDER_SCUTIL,
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

/// Device → Hardware Port map parsed from `networksetup -listallhardwareports`.
///
/// Blocks are separated by blank lines and carry `Hardware Port:` /
/// `Device:` / `Ethernet Address:` lines. Only the first two are read; the
/// hardware address is deliberately never retained. The first mapping for a
/// device wins so repeated blocks cannot flip an already-joined identity.
/// Returns the map plus the number of blocks skipped because a required
/// field was missing or exceeded the text ceiling.
fn parse_hardware_ports(
    bytes: &[u8],
) -> Result<(BTreeMap<String, String>, usize), NetworkDnsError> {
    let text = std::str::from_utf8(bytes).map_err(|_| malformed("networksetup output encoding"))?;
    let mut map = BTreeMap::new();
    let mut skipped = 0usize;
    let mut port: Option<String> = None;
    let mut device: Option<String> = None;
    let mut saw_field = false;
    let flush = |port: &mut Option<String>,
                 device: &mut Option<String>,
                 saw_field: &mut bool,
                 map: &mut BTreeMap<String, String>,
                 skipped: &mut usize| {
        if !*saw_field {
            return;
        }
        match (device.take(), port.take()) {
            (Some(device), Some(port)) => {
                map.entry(device).or_insert(port);
            }
            _ => *skipped += 1,
        }
        *saw_field = false;
    };
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('*').trim_start();
        if line.is_empty() {
            flush(
                &mut port,
                &mut device,
                &mut saw_field,
                &mut map,
                &mut skipped,
            );
            continue;
        }
        if let Some(value) = property(line, "Hardware Port") {
            saw_field = true;
            port = bounded_text(value);
        } else if let Some(value) = property(line, "Device") {
            saw_field = true;
            device = bounded_text(value);
        }
        if map.len() >= NETWORK_DNS_SCAN_CEILING {
            break;
        }
    }
    flush(
        &mut port,
        &mut device,
        &mut saw_field,
        &mut map,
        &mut skipped,
    );
    Ok((map, skipped))
}

fn bounded_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= NETWORK_DNS_TEXT_CEILING).then(|| value.to_owned())
}

/// Fill `service` on every row that names an interface present in the map.
/// Rows without an interface, or whose device has no hardware port, keep
/// `service: None`; resolver and search-domain data are never touched.
fn apply_service_map(inventory: &mut NetworkDnsInventory, map: &BTreeMap<String, String>) {
    for row in &mut inventory.resolvers {
        row.service = row
            .interface
            .as_deref()
            .and_then(|interface| map.get(interface))
            .cloned();
    }
    for row in &mut inventory.search_domains {
        row.service = row
            .interface
            .as_deref()
            .and_then(|interface| map.get(interface))
            .cloned();
    }
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

    #[test]
    fn hardware_port_map_joins_multiple_blocks_and_skips_incomplete_ones() {
        let raw = b"
Hardware Port: Wi-Fi
Device: en0
Ethernet Address: aa:bb:cc:dd:ee:ff

Hardware Port: Thunderbolt Bridge
Device: bridge0
Ethernet Address: 11:22:33:44:55:66

Hardware Port: Orphan Without Device
Ethernet Address: 00:00:00:00:00:00

Device: en9

VLAN Configurations
===================
";
        let (map, skipped) = parse_hardware_ports(raw).unwrap();
        assert_eq!(map.get("en0").map(String::as_str), Some("Wi-Fi"));
        assert_eq!(
            map.get("bridge0").map(String::as_str),
            Some("Thunderbolt Bridge")
        );
        assert_eq!(map.len(), 2, "{map:?}");
        assert_eq!(skipped, 2, "orphan port and orphan device are both skipped");
        assert!(
            !map.values().any(|value| value.contains(':')),
            "hardware addresses must never be retained"
        );
    }

    #[test]
    fn hardware_port_map_tolerates_star_prefixes_blank_runs_and_keeps_first_duplicate() {
        let raw = b"

* Hardware Port: Wi-Fi
* Device: en0


Hardware Port: USB 10/100/1000 LAN
Device: en0

Hardware Port:
Device: en5
";
        let (map, skipped) = parse_hardware_ports(raw).unwrap();
        assert_eq!(map.get("en0").map(String::as_str), Some("Wi-Fi"));
        assert_eq!(map.len(), 1);
        assert_eq!(skipped, 1, "empty Hardware Port value is a skipped block");
    }

    #[test]
    fn hardware_port_map_rejects_non_utf8_and_bounds_text() {
        assert_eq!(
            parse_hardware_ports(&[0xff, 0xfe]).unwrap_err().kind(),
            NetworkDnsErrorKind::MalformedSnapshot
        );
        let long = "x".repeat(NETWORK_DNS_TEXT_CEILING + 1);
        let raw = format!("Hardware Port: {long}\nDevice: en0\n");
        let (map, skipped) = parse_hardware_ports(raw.as_bytes()).unwrap();
        assert!(map.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn service_map_fills_only_rows_with_a_known_interface() {
        let raw = br#"
resolver #1
  search domain[0] : example.test
  nameserver[0] : 192.0.2.53
  if_index : 14 (en0)
resolver #2
  nameserver[0] : 192.0.2.54
  if_index : 15 (en7)
resolver #3
  nameserver[0] : 192.0.2.55
"#;
        let mut inventory = parse_scutil(raw).unwrap();
        let map: BTreeMap<String, String> = [("en0".to_owned(), "Wi-Fi".to_owned())]
            .into_iter()
            .collect();
        apply_service_map(&mut inventory, &map);
        assert_eq!(inventory.resolvers[0].service.as_deref(), Some("Wi-Fi"));
        assert_eq!(
            inventory.search_domains[0].service.as_deref(),
            Some("Wi-Fi")
        );
        assert_eq!(
            inventory.resolvers[1].service, None,
            "interface without a hardware port stays null"
        );
        assert_eq!(
            inventory.resolvers[2].service, None,
            "no interface stays null"
        );
        assert_eq!(inventory.resolvers[0].address.to_string(), "192.0.2.53");
        assert_eq!(inventory.provider, PROVIDER_SCUTIL);
    }

    #[test]
    fn networksetup_failure_keeps_resolvers_and_degrades_honestly() {
        let raw = br#"
resolver #1
  nameserver[0] : 192.0.2.53
  if_index : 14 (en0)
"#;
        let mut inventory = parse_scutil(raw).unwrap();
        assert_eq!(inventory.read_errors, 0);
        apply_join_outcome(
            &mut inventory,
            Err(unavailable(
                "the fixed /usr/sbin/networksetup provider is unavailable",
            )),
        );
        assert_eq!(inventory.resolvers.len(), 1, "resolver rows survive");
        assert_eq!(inventory.resolvers[0].service, None);
        assert_eq!(inventory.provider, PROVIDER_SCUTIL);
        assert_eq!(
            inventory.read_errors, 1,
            "one read error marks the join as incomplete"
        );
        assert_eq!(inventory.coverage, NetworkDnsCoverage::SystemEffective);

        let mut joined = parse_scutil(raw).unwrap();
        let map: BTreeMap<String, String> = [("en0".to_owned(), "Wi-Fi".to_owned())]
            .into_iter()
            .collect();
        apply_join_outcome(&mut joined, Ok((map, 2)));
        assert_eq!(joined.resolvers[0].service.as_deref(), Some("Wi-Fi"));
        assert_eq!(joined.provider, PROVIDER_JOINED);
        assert_eq!(
            joined.read_errors, 1,
            "a skipped supplemental block marks the joined inventory incomplete"
        );
    }

    #[test]
    fn supplemental_failure_preserves_inventory_count_invariants() {
        let mut empty = parse_scutil(b"").unwrap();
        apply_join_outcome(
            &mut empty,
            Err(unavailable(
                "the fixed /usr/sbin/networksetup provider is unavailable",
            )),
        );
        assert_eq!(empty.visited, 0);
        assert_eq!(empty.read_errors, 0);
        assert_eq!(empty.provider, PROVIDER_SCUTIL);

        let raw = br#"
resolver #1
  nameserver[0] : 192.0.2.53
  if_index : 14 (en0)
"#;
        let mut one_row = parse_scutil(raw).unwrap();
        apply_join_outcome(&mut one_row, Ok((BTreeMap::new(), usize::MAX)));
        assert_eq!(one_row.visited, 1);
        assert_eq!(one_row.read_errors, 1);
    }
}
