//! Bounded effective DNS resolver inventory.

use agenterm_platform::network_dns::{
    NetworkDnsErrorKind, NetworkDnsResolver, NetworkDnsSearchDomain,
};

use super::*;

const RESPONSE_CEILING_BYTES: usize = 1024 * 1024;

pub(super) fn network_dns_payload(max: usize) -> Result<serde_json::Value, CuError> {
    let inventory = agenterm_platform::network_dns::enumerate(max)
        .map_err(|error| CuError::new(error_code(error.kind()), error.detail()))?;
    let resolvers = inventory
        .resolvers
        .into_iter()
        .map(resolver_value)
        .collect::<Vec<_>>();
    let search_domains = inventory
        .search_domains
        .into_iter()
        .map(search_domain_value)
        .collect::<Vec<_>>();
    let returned = resolvers.len().saturating_add(search_domains.len());
    let payload = serde_json::json!({
        "resolvers": resolvers,
        "search_domains": search_domains,
        "visited": inventory.visited,
        "returned": returned,
        "truncated": inventory.truncated,
        "truncated_scan": inventory.truncated_scan,
        "read_errors": inventory.read_errors,
        "complete": inventory.complete,
        "provider": inventory.provider,
        "coverage": inventory.coverage.as_str(),
        "response_ceiling_bytes": RESPONSE_CEILING_BYTES,
    });
    if serde_json::to_vec(&payload)
        .map_err(|error| CuError::new("network_dns_encode_failed", error.to_string()))?
        .len()
        > RESPONSE_CEILING_BYTES
    {
        return Err(CuError::new(
            "network_dns_response_too_large",
            "DNS inventory exceeded its 1 MiB response ceiling",
        ));
    }
    Ok(payload)
}

fn resolver_value(row: NetworkDnsResolver) -> serde_json::Value {
    serde_json::json!({
        "family": row.family.as_str(),
        "address": row.address.to_string(),
        "port": row.port,
        "interface": row.interface,
        "interface_native_id": row.interface_native_id.map(|value| value.to_string()),
        "scope_id": row.scope_id,
        "service": row.service,
        "resolver_index": row.resolver_index,
    })
}

fn search_domain_value(row: NetworkDnsSearchDomain) -> serde_json::Value {
    serde_json::json!({
        "domain": row.domain,
        "interface": row.interface,
        "interface_native_id": row.interface_native_id.map(|value| value.to_string()),
        "service": row.service,
        "resolver_index": row.resolver_index,
    })
}

fn error_code(kind: NetworkDnsErrorKind) -> &'static str {
    match kind {
        NetworkDnsErrorKind::InvalidLimit => "network_dns_invalid_limit",
        NetworkDnsErrorKind::Unavailable => "network_dns_unavailable",
        NetworkDnsErrorKind::MalformedSnapshot => "network_dns_malformed_snapshot",
        NetworkDnsErrorKind::ResourceLimit => "network_dns_resource_limit",
        NetworkDnsErrorKind::Timeout => "network_dns_timeout",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn resolver_wide_identity_is_lossless_for_json_consumers() {
        let row = resolver_value(NetworkDnsResolver {
            family: agenterm_platform::network_dns::NetworkDnsFamily::Ipv4,
            address: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 53)),
            port: 53,
            interface: Some("fixture".into()),
            interface_native_id: Some(9_007_199_254_740_993),
            scope_id: None,
            service: None,
            resolver_index: Some(1),
        });
        assert_eq!(row["interface_native_id"], "9007199254740993");
        assert_eq!(row["address"], "192.0.2.53");
    }
}
