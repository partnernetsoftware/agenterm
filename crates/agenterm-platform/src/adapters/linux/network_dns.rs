//! Linux resolver inventory with explicit systemd-resolved stub detection.

use std::{
    fs::File,
    io::{Read, Take},
    net::IpAddr,
    path::Path,
};

use crate::contract::network_dns::{
    NETWORK_DNS_SCAN_CEILING, NETWORK_DNS_TEXT_CEILING, NetworkDnsCoverage, NetworkDnsError,
    NetworkDnsErrorKind, NetworkDnsFamily, NetworkDnsInventory, NetworkDnsResolver,
    NetworkDnsSearchDomain,
};

const RESOLV_CONF: &str = "/etc/resolv.conf";
const SYSTEMD_UPSTREAM: &str = "/run/systemd/resolve/resolv.conf";
const FILE_CEILING: u64 = 64 * 1024;

pub(crate) fn enumerate_native() -> Result<NetworkDnsInventory, NetworkDnsError> {
    let primary = read_bounded(Path::new(RESOLV_CONF))?;
    let primary_parsed = parse_resolver_file(&primary)?;
    let systemd_stub = primary_parsed
        .resolvers
        .iter()
        .any(|row| row.address.is_loopback() && row.address.to_string() == "127.0.0.53");
    if systemd_stub {
        match read_bounded(Path::new(SYSTEMD_UPSTREAM)) {
            Ok(upstream) => {
                let mut parsed = parse_resolver_file(&upstream)?;
                parsed.provider = "systemd-resolved-upstream";
                parsed.coverage = NetworkDnsCoverage::SystemEffective;
                return Ok(parsed);
            }
            Err(error) if error.kind() == NetworkDnsErrorKind::Unavailable => {
                let mut parsed = primary_parsed;
                parsed.provider = "resolv.conf-systemd-stub";
                parsed.coverage = NetworkDnsCoverage::StubOnly;
                return Ok(parsed);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(primary_parsed)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, NetworkDnsError> {
    let file = File::open(path).map_err(|error| {
        NetworkDnsError::new(
            NetworkDnsErrorKind::Unavailable,
            format!("{} could not be opened: {error}", path.display()),
        )
    })?;
    let mut bytes = Vec::new();
    let mut reader: Take<File> = file.take(FILE_CEILING + 1);
    reader.read_to_end(&mut bytes).map_err(|error| {
        NetworkDnsError::new(
            NetworkDnsErrorKind::Unavailable,
            format!("{} could not be read: {error}", path.display()),
        )
    })?;
    if bytes.len() as u64 > FILE_CEILING {
        return Err(NetworkDnsError::new(
            NetworkDnsErrorKind::ResourceLimit,
            format!("{} exceeds the {FILE_CEILING}-byte ceiling", path.display()),
        ));
    }
    Ok(bytes)
}

fn parse_resolver_file(bytes: &[u8]) -> Result<NetworkDnsInventory, NetworkDnsError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        NetworkDnsError::new(
            NetworkDnsErrorKind::MalformedSnapshot,
            "resolver file is not valid UTF-8",
        )
    })?;
    let mut inventory = NetworkDnsInventory {
        resolvers: Vec::new(),
        search_domains: Vec::new(),
        visited: 0,
        read_errors: 0,
        truncated_scan: false,
        truncated: false,
        complete: false,
        provider: "resolv.conf",
        coverage: NetworkDnsCoverage::ResolverFile,
    };
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("nameserver") => {
                if !reserve_row(&mut inventory) {
                    break;
                }
                match fields.next().and_then(parse_scoped_ip) {
                    Some((address, scope_id, interface)) if fields.next().is_none() => {
                        inventory.resolvers.push(NetworkDnsResolver {
                            family: family(address),
                            address,
                            port: 53,
                            interface,
                            interface_native_id: None,
                            scope_id,
                            service: None,
                            resolver_index: None,
                        });
                    }
                    _ => inventory.read_errors += 1,
                }
            }
            Some("search" | "domain") => {
                for value in fields {
                    if !reserve_row(&mut inventory) {
                        break;
                    }
                    if valid_domain(value) {
                        inventory.search_domains.push(NetworkDnsSearchDomain {
                            domain: value.trim_end_matches('.').to_owned(),
                            interface: None,
                            interface_native_id: None,
                            service: None,
                            resolver_index: None,
                        });
                    } else {
                        inventory.read_errors += 1;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(inventory)
}

fn reserve_row(inventory: &mut NetworkDnsInventory) -> bool {
    if inventory.visited == NETWORK_DNS_SCAN_CEILING {
        inventory.truncated_scan = true;
        return false;
    }
    inventory.visited += 1;
    true
}

fn family(address: IpAddr) -> NetworkDnsFamily {
    match address {
        IpAddr::V4(_) => NetworkDnsFamily::Ipv4,
        IpAddr::V6(_) => NetworkDnsFamily::Ipv6,
    }
}

fn parse_scoped_ip(value: &str) -> Option<(IpAddr, Option<u32>, Option<String>)> {
    let (address, scope) = value
        .split_once('%')
        .map_or((value, None), |(address, scope)| (address, Some(scope)));
    let scope_id = scope.and_then(|value| value.parse().ok());
    let interface = scope
        .filter(|value| scope_id.is_none() && !value.is_empty())
        .map(str::to_owned);
    Some((address.parse().ok()?, scope_id, interface))
}

fn valid_domain(value: &str) -> bool {
    let value = value.trim_end_matches('.');
    !value.is_empty()
        && value.len() <= NETWORK_DNS_TEXT_CEILING.min(253)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resolvers_and_search_without_treating_options_as_dns() {
        let inventory = parse_resolver_file(
            b"nameserver 127.0.0.53\nnameserver 2001:db8::53\nsearch example.test corp.test.\noptions edns0\n",
        )
        .unwrap();
        assert_eq!(inventory.resolvers.len(), 2);
        assert_eq!(inventory.search_domains.len(), 2);
        assert_eq!(inventory.search_domains[1].domain, "corp.test");
        assert_eq!(inventory.visited, 4);
        assert_eq!(inventory.read_errors, 0);
    }

    #[test]
    fn malformed_relevant_rows_are_counted_not_fabricated() {
        let inventory =
            parse_resolver_file(b"nameserver nope\nsearch ok.test bad/value\n").unwrap();
        assert!(inventory.resolvers.is_empty());
        assert_eq!(inventory.search_domains.len(), 1);
        assert_eq!(inventory.visited, 3);
        assert_eq!(inventory.read_errors, 2);
    }
}
