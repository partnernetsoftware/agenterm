//! Selected-host bounded DNS resolver inventory.

pub use crate::contract::network_dns::{
    NETWORK_DNS_MAX_ROWS, NETWORK_DNS_SCAN_CEILING, NETWORK_DNS_TEXT_CEILING, NetworkDnsCoverage,
    NetworkDnsError, NetworkDnsErrorKind, NetworkDnsFamily, NetworkDnsInventory,
    NetworkDnsResolver, NetworkDnsSearchDomain,
};

pub fn enumerate(max_rows: usize) -> Result<NetworkDnsInventory, NetworkDnsError> {
    if !(1..=NETWORK_DNS_MAX_ROWS).contains(&max_rows) {
        return Err(NetworkDnsError::new(
            NetworkDnsErrorKind::InvalidLimit,
            format!("max_rows must be between 1 and {NETWORK_DNS_MAX_ROWS}, got {max_rows}"),
        ));
    }
    finish_inventory(crate::selected::network_dns::enumerate_native()?, max_rows)
}

fn finish_inventory(
    mut inventory: NetworkDnsInventory,
    max_rows: usize,
) -> Result<NetworkDnsInventory, NetworkDnsError> {
    if inventory.visited > NETWORK_DNS_SCAN_CEILING
        || inventory.read_errors > inventory.visited
        || inventory
            .resolvers
            .len()
            .saturating_add(inventory.search_domains.len())
            > inventory.visited
    {
        return Err(NetworkDnsError::new(
            NetworkDnsErrorKind::MalformedSnapshot,
            "provider returned incoherent DNS inventory counts",
        ));
    }
    inventory.resolvers.sort_by(|left, right| {
        left.resolver_index
            .cmp(&right.resolver_index)
            .then_with(|| left.interface_native_id.cmp(&right.interface_native_id))
            .then_with(|| left.interface.cmp(&right.interface))
            .then_with(|| left.family.cmp(&right.family))
            .then_with(|| ip_sort_key(left.address).cmp(&ip_sort_key(right.address)))
            .then_with(|| left.port.cmp(&right.port))
            .then_with(|| left.scope_id.cmp(&right.scope_id))
            .then_with(|| left.service.cmp(&right.service))
    });
    inventory.resolvers.dedup();
    inventory.search_domains.sort_by(|left, right| {
        left.resolver_index
            .cmp(&right.resolver_index)
            .then_with(|| left.interface_native_id.cmp(&right.interface_native_id))
            .then_with(|| left.interface.cmp(&right.interface))
            .then_with(|| left.domain.cmp(&right.domain))
            .then_with(|| left.service.cmp(&right.service))
    });
    inventory.search_domains.dedup();
    let available = inventory
        .resolvers
        .len()
        .saturating_add(inventory.search_domains.len());
    let resolver_count = inventory.resolvers.len().min(max_rows);
    inventory.resolvers.truncate(resolver_count);
    let remaining = max_rows.saturating_sub(resolver_count);
    inventory.search_domains.truncate(remaining);
    inventory.truncated = inventory.truncated_scan || available > max_rows;
    inventory.complete = !inventory.truncated
        && inventory.read_errors == 0
        && inventory.coverage != NetworkDnsCoverage::StubOnly;
    Ok(inventory)
}

fn ip_sort_key(address: std::net::IpAddr) -> [u8; 16] {
    match address {
        std::net::IpAddr::V4(address) => {
            let mut bytes = [0; 16];
            bytes[..4].copy_from_slice(&address.octets());
            bytes
        }
        std::net::IpAddr::V6(address) => address.octets(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_limits_before_native_enumeration() {
        assert_eq!(
            enumerate(0).unwrap_err().kind(),
            NetworkDnsErrorKind::InvalidLimit
        );
        assert_eq!(
            enumerate(NETWORK_DNS_MAX_ROWS + 1).unwrap_err().kind(),
            NetworkDnsErrorKind::InvalidLimit
        );
    }
}
