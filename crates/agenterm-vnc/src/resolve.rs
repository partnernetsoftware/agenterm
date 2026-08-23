//! Turning a host into addresses, including the `.local` names that the
//! ordinary resolver can miss.
//!
//! Why this is not just `TcpStream::connect(host)`: `getaddrinfo` is the only
//! resolver tokio uses, and on a machine where a VPN client owns DNS (Tailscale
//! points the system at `100.100.100.100`) queries for `.local` names are
//! answered by that resolver rather than falling through to multicast DNS. The
//! name then fails to resolve even though the host is one hop away and macOS's
//! own tooling finds it. Bonjour is consulted directly in that case.

use std::net::{SocketAddr, ToSocketAddrs};

/// Resolve `host:port` to at least one address.
///
/// Ordinary resolution is tried first, so nothing changes for names and
/// literals that already work. The mDNS fallback is reserved for `.local`,
/// which is the only namespace it is authoritative for.
pub(crate) fn resolve(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    let system = (host, port).to_socket_addrs();
    match system {
        Ok(addresses) => {
            let addresses: Vec<_> = addresses.collect();
            if !addresses.is_empty() {
                return Ok(addresses);
            }
        }
        Err(error) if !is_mdns_name(host) => return Err(error),
        Err(_) => {}
    }

    if let Some(addresses) = resolve_mdns(host, port) {
        return Ok(addresses);
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("could not resolve {host}"),
    ))
}

/// Whether a name belongs to the multicast DNS namespace.
///
/// A bare `m4pro` is included: macOS presents Bonjour hosts under a short name
/// too, and appending `.local` is what the system resolver would have done.
fn is_mdns_name(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_err()
        && (host.ends_with(".local") || !host.contains('.'))
}

/// Ask macOS's own resolver, which answers from the Bonjour cache.
///
/// `dscacheutil` is used rather than a Bonjour binding because it needs no
/// dependency and no `unsafe`, and this path runs once per connection attempt
/// rather than in any hot loop.
#[cfg(target_os = "macos")]
fn resolve_mdns(host: &str, port: u16) -> Option<Vec<SocketAddr>> {
    let name = if host.ends_with(".local") {
        host.to_owned()
    } else {
        format!("{host}.local")
    };

    let output = std::process::Command::new("dscacheutil")
        .args(["-q", "host", "-a", "name", &name])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);

    // Prefer IPv4: the IPv6 answers here are link-local, which need a scope id
    // this lookup does not report, so connecting to them fails.
    let mut addresses: Vec<SocketAddr> = text
        .lines()
        .filter_map(|line| line.strip_prefix("ip_address: "))
        .filter_map(|value| value.trim().parse::<std::net::Ipv4Addr>().ok())
        .map(|ip| SocketAddr::from((ip, port)))
        .collect();
    addresses.dedup();

    (!addresses.is_empty()).then_some(addresses)
}

#[cfg(not(target_os = "macos"))]
fn resolve_mdns(_host: &str, _port: u16) -> Option<Vec<SocketAddr>> {
    // Other platforms resolve `.local` through nss-mdns or systemd-resolved,
    // which `getaddrinfo` already consults.
    None
}
